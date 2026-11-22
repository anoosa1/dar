import {useLayoutEffect} from "react";

const counts=new Map<string,number>();
/**
 * Adds classes to <body> while a component is mounted. Page-wide layout rules use
 * these instead of `body:has(...)`, which makes the browser restyle the whole page
 * after unrelated changes such as ticking a checkbox or opening a menu.
 */
export function useBodyClass(...names:string[]){
  const key=names.join(" ");
  useLayoutEffect(()=>{
    for(const name of names){counts.set(name,(counts.get(name)??0)+1);document.body.classList.add(name);}
    return()=>{for(const name of names){const left=(counts.get(name)??1)-1;if(left)counts.set(name,left);else{counts.delete(name);document.body.classList.remove(name);}}};
  },[key]);
}
