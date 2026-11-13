/**
 * Builds article DOM from server-sanitized HTML through a second allowlist. DOMParser
 * documents are inert (no scripts run, no images load); only copied nodes reach the page.
 * Images load through Dar's proxy or are replaced by a note; links open in a new tab.
 */
const allowed=new Set(["a","abbr","b","blockquote","br","caption","cite","code","dd","del","dfn","div","dl","dt","em","figcaption","figure","h1","h2","h3","h4","h5","h6","hr","i","img","ins","kbd","li","mark","ol","p","pre","q","s","samp","small","span","strong","sub","sup","table","tbody","td","tfoot","th","thead","time","tr","u","ul"]);
const dropped=new Set(["script","style","iframe","object","embed","svg","math","form","template","noscript","link","meta","base","video","audio","canvas"]);
function safe(value:string|null,schemes=["http:","https:"]){if(!value)return "";try{const url=new URL(value);return schemes.includes(url.protocol)?url.href:"";}catch{return "";}}
export function renderArticle(html:string,images:"proxy"|"block"){
  const source=new DOMParser().parseFromString(`<!doctype html><body>${html}</body>`,"text/html"),out=document.createDocumentFragment();let hidden=0;
  const walk=(from:Node,to:Node,depth:number)=>{
    for(const child of [...from.childNodes]){
      if(child.nodeType===Node.TEXT_NODE){to.appendChild(document.createTextNode(child.textContent??""));continue;}
      if(child.nodeType!==Node.ELEMENT_NODE)continue;
      const element=child as Element,tag=element.tagName.toLowerCase();
      if(dropped.has(tag))continue;
      if(!allowed.has(tag)||depth>60){walk(element,to,depth);continue;}
      if(tag==="img"){
        const src=safe(element.getAttribute("src"));if(!src)continue;
        if(images==="block"){hidden++;continue;}
        const image=document.createElement("img");image.src=`/api/v1/news/image?url=${encodeURIComponent(src)}`;image.alt=element.getAttribute("alt")??"";image.loading="lazy";image.decoding="async";image.referrerPolicy="no-referrer";
        to.appendChild(image);continue;
      }
      const copy=document.createElement(tag);
      if(tag==="a"){const href=safe(element.getAttribute("href"),["http:","https:","mailto:"]);if(href){copy.setAttribute("href",href);copy.setAttribute("target","_blank");copy.setAttribute("rel","noopener noreferrer");}}
      for(const name of ["colspan","rowspan","start","datetime","title","dir"]){const value=element.getAttribute(name);if(value&&(name!=="dir"||value==="rtl"||value==="ltr")&&value.length<300)copy.setAttribute(name,value);}
      walk(element,copy,depth+1);to.appendChild(copy);
    }
  };
  walk(source.body,out,0);
  if(hidden){const note=document.createElement("p");note.className="news-images-hidden";note.textContent=`${hidden} image${hidden===1?"":"s"} hidden by your News settings.`;out.prepend(note);}
  return out;
}
