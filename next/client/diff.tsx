/** Line diffs shared by Agent's review and Files history. Display only: nothing here writes content. */
export type DiffLine={kind:" "|"-"|"+";text:string};

/** Minimal line diff. Large middles fall back to replacement to bound browser work. */
export function lineDiff(old:string[],next:string[]):DiffLine[]{
  let start=0,endOld=old.length,endNew=next.length;
  while(start<endOld&&start<endNew&&old[start]===next[start])start++;
  while(endOld>start&&endNew>start&&old[endOld-1]===next[endNew-1]){endOld--;endNew--;}
  const a=old.slice(start,endOld),b=next.slice(start,endNew),middle:DiffLine[]=[];
  if(a.length*b.length>2_000_000)middle.push(...a.map(text=>({kind:"-" as const,text})),...b.map(text=>({kind:"+" as const,text})));
  else{
    const width=b.length+1,table=new Uint32Array((a.length+1)*width);
    for(let i=a.length-1;i>=0;i--)for(let j=b.length-1;j>=0;j--)table[i*width+j]=a[i]===b[j]?table[(i+1)*width+j+1]+1:Math.max(table[(i+1)*width+j],table[i*width+j+1]);
    let i=0,j=0;while(i<a.length||j<b.length){
      if(i<a.length&&j<b.length&&a[i]===b[j]){middle.push({kind:" ",text:a[i]});i++;j++;}
      else if(i<a.length&&(j===b.length||table[(i+1)*width+j]>=table[i*width+j+1]))middle.push({kind:"-",text:a[i++]});
      else middle.push({kind:"+",text:b[j++]});
    }
  }
  return [...old.slice(0,start).map(text=>({kind:" " as const,text})),...middle,...old.slice(endOld).map(text=>({kind:" " as const,text}))];
}
/** Splits text into lines; the final newline ends the last line rather than starting another. */
export function textLines(value:string){const lines=value.split("\n");if(lines.at(-1)==="")lines.pop();return lines;}
/** Unified hunks with three context lines after the given header lines. */
export function unifiedHunks(old:string[],next:string[],head:string[]=[]){
  const lines=lineDiff(old,next),changed=lines.flatMap((line,index)=>line.kind===" "?[]:[index]);
  if(!changed.length)return head.join("\n");
  const ranges:[number,number][]=[];for(const index of changed){const from=Math.max(0,index-3),to=Math.min(lines.length,index+4),last=ranges.at(-1);if(last&&from<=last[1])last[1]=to;else ranges.push([from,to]);}
  const output=[...head];let oldLine=1,newLine=1,cursor=0;
  for(const [from,to] of ranges){
    for(;cursor<from;cursor++){if(lines[cursor].kind!=="+")oldLine++;if(lines[cursor].kind!=="-")newLine++;}
    const slice=lines.slice(from,to),oldCount=slice.filter(line=>line.kind!=="+").length,newCount=slice.filter(line=>line.kind!=="-").length;
    output.push(`@@ -${oldCount?oldLine:oldLine-1},${oldCount} +${newCount?newLine:newLine-1},${newCount} @@`,...slice.map(line=>line.kind+line.text));
  }
  return output.join("\n");
}
export function countLines(old:string[],next:string[]){const lines=lineDiff(old,next);return {added:lines.filter(line=>line.kind==="+").length,removed:lines.filter(line=>line.kind==="-").length};}

export function DiffStat({added,removed}:{added:number;removed:number}){
  return <span className="diff-stat"><span className="diff-added">+{added}</span><span className="diff-removed">−{removed}</span></span>;
}
/** Renders a unified patch, keeping its markers and hunk boundaries. */
export function Diff({source,wrap,label="File diff"}:{source:string;wrap:boolean;label?:string}){
  let oldLine=1,newLine=1,inHunk=false;
  return <div className={`code-diff${wrap?" code-diff-wrap":""}`} role="region" aria-label={label} tabIndex={0}>{source.split("\n").filter(line=>!line.startsWith("--- ")&&!line.startsWith("+++ ")).map((line,index)=>{
    let oldNumber:number|undefined,newNumber:number|undefined,kind="";
    if(line.startsWith("@@")){const hunk=/^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line);oldLine=hunk?Number(hunk[1]):1;newLine=hunk?Number(hunk[2]):1;inHunk=true;kind="hunk";}
    else if(inHunk){if(line.startsWith("-")){kind="removed";oldNumber=oldLine++;}else if(line.startsWith("+")){kind="added";newNumber=newLine++;}else if(line.startsWith(" ")){oldNumber=oldLine++;newNumber=newLine++;}}
    return <div key={index} className={`code-diff-line ${kind}`}><span className="code-diff-number" aria-hidden="true">{oldNumber}</span><span className="code-diff-number" aria-hidden="true">{newNumber}</span><code className="code-diff-text">{(kind==="hunk"||!inHunk?line:line.slice(1))||" "}</code></div>;
  })}</div>;
}
