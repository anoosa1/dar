/**
 * Lanes for the commit graph. Commits come newest first; each row says which lane its commit sits
 * in and which lines cross the row: lines passing straight through, lines from above that end at
 * the commit, and lines from the commit down to its parents' lanes.
 */
export interface GraphRow {lane:number;/** A line comes down into the commit in its own lane. */top:boolean;through:number[];into:number[];out:number[];width:number}
export function graphRows(commits:{sha:string;parents:string[]}[]):GraphRow[]{
  const lanes:(string|null)[]=[],rows:GraphRow[]=[];
  const free=()=>{const index=lanes.indexOf(null);if(index>=0)return index;lanes.push(null);return lanes.length-1;};
  for(const commit of commits){
    let lane=lanes.indexOf(commit.sha);const top=lane>=0;
    const into=lanes.flatMap((value,index)=>value===commit.sha?[index]:[]);
    if(lane<0)lane=free();
    const through=lanes.flatMap((value,index)=>value!==null&&value!==commit.sha?[index]:[]);
    for(const index of into)lanes[index]=null;
    const out:number[]=[];
    commit.parents.forEach((parent,position)=>{
      const existing=lanes.indexOf(parent);
      // The first parent continues the commit's own lane unless another lane already waits for it.
      if(existing>=0){out.push(existing);return;}
      const target=position===0?lane:free();lanes[target]=parent;out.push(target);
    });
    while(lanes.length&&lanes.at(-1)===null)lanes.pop();
    rows.push({lane,top,through,into:into.filter(index=>index!==lane),out,width:Math.max(lane+1,lanes.length,...through.map(index=>index+1),...out.map(index=>index+1))});
  }
  return rows;
}
