/**
 * A small HTML tokenizer and allowlist sanitizer for feed content. Output contains only
 * known formatting elements, http(s) links/images resolved to absolute URLs and no
 * scripts, styles, event handlers, frames or forms. The browser re-sanitizes before display.
 */
export type Token={type:"text";text:string}|{type:"open";name:string;attrs:Record<string,string>;self:boolean}|{type:"close";name:string};
const raw=new Set(["script","style","textarea","title","noscript","iframe","xmp","template","noembed","noframes"]);
const entities:Record<string,string>={amp:"&",lt:"<",gt:">",quot:'"',apos:"'",nbsp:" ",hellip:"…",mdash:"—",ndash:"–",lsquo:"‘",rsquo:"’",ldquo:"“",rdquo:"”",laquo:"«",raquo:"»",copy:"©",reg:"®",trade:"™",bull:"•",middot:"·",euro:"€",pound:"£",yen:"¥",cent:"¢",deg:"°",times:"×",divide:"÷",shy:"­",zwj:"‍",zwnj:"‌",thinsp:" ",ensp:" ",emsp:" ",larr:"←",rarr:"→",uarr:"↑",darr:"↓",harr:"↔",hearts:"♥",para:"¶",sect:"§",iexcl:"¡",iquest:"¿",frac12:"½",frac14:"¼",frac34:"¾",plusmn:"±",micro:"µ",prime:"′",Prime:"″",
  aacute:"á",eacute:"é",iacute:"í",oacute:"ó",uacute:"ú",agrave:"à",egrave:"è",igrave:"ì",ograve:"ò",ugrave:"ù",acirc:"â",ecirc:"ê",icirc:"î",ocirc:"ô",ucirc:"û",auml:"ä",euml:"ë",iuml:"ï",ouml:"ö",uuml:"ü",atilde:"ã",otilde:"õ",ntilde:"ñ",ccedil:"ç",szlig:"ß",aring:"å",aelig:"æ",oslash:"ø",Aacute:"Á",Eacute:"É",Oacute:"Ó",Uacute:"Ú",Auml:"Ä",Ouml:"Ö",Uuml:"Ü",Ntilde:"Ñ",Ccedil:"Ç"};
export function decode(value:string){
  return value.replace(/&(#x[0-9a-f]{1,6}|#[0-9]{1,7}|[a-z][a-z0-9]{1,31});?/gi,(match,code:string)=>{
    if(code[0]==="#"){const n=code[1]==="x"||code[1]==="X"?parseInt(code.slice(2),16):parseInt(code.slice(1),10);return n>0&&n<=0x10ffff&&!(n>=0xd800&&n<=0xdfff)?String.fromCodePoint(n):"�";}
    return entities[code]??match;
  });
}
export const escapeHtml=(value:string)=>value.replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]!);

export function tokenize(html:string,limit=200000):Token[]{
  const tokens:Token[]=[];let i=0;
  const pushText=(text:string)=>{if(text)tokens.push({type:"text",text:decode(text)});};
  while(i<html.length&&tokens.length<limit){
    const lt=html.indexOf("<",i);if(lt<0){pushText(html.slice(i));break;}
    pushText(html.slice(i,lt));
    if(html.startsWith("<!--",lt)){const end=html.indexOf("-->",lt+4);i=end<0?html.length:end+3;continue;}
    if(html[lt+1]==="!"||html[lt+1]==="?"){const end=html.indexOf(">",lt);i=end<0?html.length:end+1;continue;}
    const close=html[lt+1]==="/",start=lt+(close?2:1),name=/^[a-zA-Z][a-zA-Z0-9:-]*/.exec(html.slice(start,start+64))?.[0];
    if(!name){pushText("<");i=lt+1;continue;}
    // Find the tag end outside quoted attribute values.
    let j=start+name.length,quote="";for(;j<html.length;j++){const c=html[j];if(quote){if(c===quote)quote="";}else if(c==='"'||c==="'")quote=c;else if(c===">")break;}
    const inner=html.slice(start+name.length,j),lower=name.toLowerCase();i=j+1;
    if(close){tokens.push({type:"close",name:lower});continue;}
    const attrs:Record<string,string>={};
    for(const m of inner.matchAll(/([^\s"'>\/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g)){const key=m[1].toLowerCase();if(!(key in attrs)&&Object.keys(attrs).length<40)attrs[key]=decode(m[2]??m[3]??m[4]??"");}
    tokens.push({type:"open",name:lower,attrs,self:/\/\s*$/.test(inner)});
    if(raw.has(lower)){const end=html.toLowerCase().indexOf("</"+lower,i);const body=html.slice(i,end<0?html.length:end);tokens.push({type:"text",text:lower==="title"?decode(body):""});i=end<0?html.length:end;}
  }
  return tokens;
}

const allowed=new Set(["a","abbr","b","blockquote","br","caption","cite","code","dd","del","dfn","div","dl","dt","em","figcaption","figure","h1","h2","h3","h4","h5","h6","hr","i","img","ins","kbd","li","mark","ol","p","pre","q","s","samp","small","span","strong","sub","sup","table","tbody","td","tfoot","th","thead","time","tr","u","ul"]);
const dropped=new Set([...raw,"svg","math","form","button","input","select","option","object","embed","applet","canvas","audio","video","head","frameset","frame","base","link","meta","source","track","param","picture>source","dialog","portal"]);
const voids=new Set(["br","hr","img","source","track","wbr","input","meta","link","base","col","area","param","embed"]);
export function absolute(value:string,base:string,schemes=["http:","https:"]){
  try{const url=new URL(value.trim(),base||undefined);return schemes.includes(url.protocol)&&!url.username&&!url.password&&url.href.length<=2048?url.href:"";}catch{return "";}
}

/** Re-serializes tokens through the allowlist, balancing tags and resolving URLs. */
export function sanitize(input:string|Token[],base:string,maxBytes=200*1024){
  const tokens=typeof input==="string"?tokenize(input):input,stack:string[]=[];let out="",skip=0,skipName="";
  for(const token of tokens){
    if(out.length>maxBytes)break;
    if(skip){if(token.type==="open"&&token.name===skipName&&!token.self)skip++;else if(token.type==="close"&&token.name===skipName)skip--;continue;}
    if(token.type==="text"){out+=escapeHtml(token.text);continue;}
    if(token.type==="open"){
      if(dropped.has(token.name)){if(!token.self&&!voids.has(token.name)){skip=1;skipName=token.name;}continue;}
      if(!allowed.has(token.name)||stack.length>=64)continue;
      const a=token.attrs,attrs:string[]=[],add=(key:string,value:string)=>{if(value)attrs.push(`${key}="${escapeHtml(value)}"`);};
      if(token.name==="a"){add("href",absolute(a.href??"",base,["http:","https:","mailto:"]));add("title",a.title?.slice(0,300)??"");}
      if(token.name==="img"){
        // Lazy-loading pages often keep the real image in data-src behind a placeholder.
        const src=absolute((a.src&&!a.src.startsWith("data:")?a.src:"")||a["data-src"]||a["data-original"]||(a.srcset??"").split(",")[0]?.trim().split(/\s+/)[0]||"",base);
        if(!src)continue;add("src",src);add("alt",a.alt?.slice(0,500)??"");add("title",a.title?.slice(0,300)??"");
        for(const key of ["width","height"])if(/^\d{1,4}$/.test(a[key]??""))add(key,a[key]);
      }
      if((token.name==="td"||token.name==="th"))for(const key of ["colspan","rowspan"])if(/^\d{1,2}$/.test(a[key]??""))add(key,a[key]);
      if(token.name==="ol"&&/^\d{1,6}$/.test(a.start??""))add("start",a.start);
      if(token.name==="time")add("datetime",(a.datetime??"").slice(0,64));
      if(token.name==="abbr")add("title",a.title?.slice(0,300)??"");
      if(a.dir==="rtl"||a.dir==="ltr")add("dir",a.dir);
      out+=`<${token.name}${attrs.length?" "+attrs.join(" "):""}>`;
      if(!voids.has(token.name)&&!token.self)stack.push(token.name);
      continue;
    }
    const index=stack.lastIndexOf(token.name);if(index<0)continue;
    while(stack.length>index)out+=`</${stack.pop()}>`;
  }
  while(stack.length)out+=`</${stack.pop()}>`;
  return out;
}
/** Visible text, whitespace collapsed. */
export function textOf(input:string|Token[],max=100000){
  const tokens=typeof input==="string"?tokenize(input):input;let out="",skip=0,skipName="";
  for(const token of tokens){
    if(skip){if(token.type==="close"&&token.name===skipName)skip--;continue;}
    if(token.type==="open"&&dropped.has(token.name)&&!token.self&&!voids.has(token.name)){skip=1;skipName=token.name;continue;}
    if(token.type==="text")out+=token.text;else if(token.type==="open"&&["br","p","div","li","h1","h2","h3","h4","tr"].includes(token.name))out+=" ";
    if(out.length>max)break;
  }
  return out.replace(/\s+/g," ").trim();
}

export interface Alternate {url:string;title:string;type:string}
export function discoverFeeds(html:string,base:string):Alternate[]{
  const found:Alternate[]=[];let pageBase=base;
  for(const token of tokenize(html,20000)){
    if(token.type!=="open")continue;
    if(token.name==="base"&&token.attrs.href)pageBase=absolute(token.attrs.href,base)||base;
    if(token.name==="link"&&/\balternate\b/i.test(token.attrs.rel??"")&&/^application\/(rss|atom|feed)\+(xml|json)$|^application\/json$|^text\/xml$|^application\/xml$/i.test((token.attrs.type??"").trim())){
      const url=absolute(token.attrs.href??"",pageBase);if(url&&!found.some(f=>f.url===url))found.push({url,title:(token.attrs.title??"").trim().slice(0,300),type:token.attrs.type.toLowerCase()});
    }
    if(token.name==="body")break;
  }
  return found.slice(0,10);
}

/**
 * Picks the main article from a web page: an <article>/articleBody/main element when it
 * holds real text, otherwise the container whose paragraphs hold the most text, with
 * navigation, comments and promotion containers penalized. Returns sanitized HTML.
 */
export function extractArticle(html:string,base:string){
  const tokens=tokenize(html),ends=new Map<number,number>(),parents=new Map<number,number>(),stack:{index:number;name:string}[]=[];
  for(let i=0;i<tokens.length;i++){
    const token=tokens[i];
    if(token.type==="open"&&!token.self&&!voids.has(token.name)){if(stack.length)parents.set(i,stack.at(-1)!.index);stack.push({index:i,name:token.name});}
    else if(token.type==="close"){let k=stack.length-1;while(k>=0&&stack[k].name!==token.name)k--;if(k<0)continue;while(stack.length>k){const open=stack.pop()!;ends.set(open.index,i);}}
  }
  const body=(start:number)=>tokens.slice(start+1,ends.get(start)??tokens.length);
  const negative=/comment|footer|sidebar|\bnav|menu|share|social|related|promo|advert|\bad-|newsletter|subscribe|cookie|popup|banner/i;
  const attrsOf=(t:Token)=>t.type==="open"?`${t.attrs.class??""} ${t.attrs.id??""}`:"";
  let best=-1,bestScore=0;
  for(let i=0;i<tokens.length;i++){
    const t=tokens[i];if(t.type!=="open"||!ends.has(i))continue;
    if(t.name==="article"||t.attrs.itemprop==="articleBody"||t.name==="main"||t.attrs.role==="main"){
      const length=textOf(body(i)).length,score=length*(t.attrs.itemprop==="articleBody"?3:t.name==="article"?2:1.2)*(negative.test(attrsOf(t))?0.3:1);
      if(length>400&&score>bestScore){best=i;bestScore=score;}
    }
  }
  if(best<0){
    const scores=new Map<number,number>();
    for(let i=0;i<tokens.length;i++){
      const t=tokens[i],parent=parents.get(i);if(t.type!=="open"||t.name!=="p"||!ends.has(i)||parent===undefined)continue;
      const length=textOf(body(i)).length;if(length>40)scores.set(parent,(scores.get(parent)??0)+length);
    }
    for(const [index,score] of scores){const adjusted=score*(negative.test(attrsOf(tokens[index]))?0.3:1);if(adjusted>bestScore){best=index;bestScore=adjusted;}}
  }
  if(best<0)return "";
  const content=sanitize(body(best),base);return textOf(content).length>=300?content:"";
}
