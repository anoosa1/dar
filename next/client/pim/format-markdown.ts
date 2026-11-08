/** Block commands affect complete source lines; inline commands wrap selection. */
export function formatMarkdown(source:string,start:number,end:number,before:string,after:string){
  if(["# ","- ","- [ ] ","> "].includes(before)&&!after){
    const lineStart=start===0?0:source.lastIndexOf("\n",start-1)+1,selectedEnd=end>start&&source[end-1]==="\n"?end-1:end;
    const newline=source.indexOf("\n",selectedEnd),lineEnd=newline<0?source.length:newline;
    const lines=source.slice(lineStart,lineEnd).split("\n"),replacement=lines.map(line=>before+line).join("\n");
    return {text:source.slice(0,lineStart)+replacement+source.slice(lineEnd),start:lineStart,end:lineStart+replacement.length};
  }
  return {text:source.slice(0,start)+before+source.slice(start,end)+after+source.slice(end),start:start+before.length,end:end+before.length};
}
