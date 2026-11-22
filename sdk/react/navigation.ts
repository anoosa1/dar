/**
 * Switching between built-in apps without reloading the page. The app shell
 * registers a navigator for the addresses it can show; anything else (installed
 * apps, shared links, downloads, other sites) and any page with unsaved changes
 * uses a normal page load, which keeps the browser's own leave-page warning.
 */
export interface Navigator {handles:(url:URL)=>boolean;open:(url:URL)=>void}
let navigator:Navigator|undefined;

export function setNavigator(next:Navigator){navigator=next;return()=>{if(navigator===next)navigator=undefined;};}

/** Whether an open editor or dialog would warn before the page unloads. */
function unsavedChanges(){
  // Editors guard unloads by cancelling `beforeunload`; ask them the same way.
  const check=new Event("beforeunload",{cancelable:true});window.dispatchEvent(check);return check.defaultPrevented;
}

/**
 * Opens a Dar address, in place when possible. `guarded` is for an editor's guard
 * that has already asked about its unsaved changes and is continuing.
 */
export function navigate(href:string,{guarded=false}:{guarded?:boolean}={}){
  const url=new URL(href,location.href);
  // An open note may ask to save or discard first; it then continues with `guarded`.
  if(!guarded&&!window.dispatchEvent(new CustomEvent("dar:navigate",{cancelable:true,detail:{href:url.href}})))return;
  // A guard that just saved, kept or discarded its changes has already settled them.
  if(url.origin===location.origin&&navigator?.handles(url)&&(guarded||!unsavedChanges())){navigator.open(url);return;}
  location.assign(url.href);
}

/** Follows links to built-in apps in place; returns a cleanup function. */
export function interceptLinks(){
  const click=(event:MouseEvent)=>{
    if(event.defaultPrevented||event.button!==0||event.metaKey||event.ctrlKey||event.shiftKey||event.altKey)return;
    const link=(event.target as Element|null)?.closest?.("a[href]") as HTMLAnchorElement|null;
    if(!link||link.hasAttribute("download")||(link.target&&link.target!=="_self"))return;
    const url=new URL(link.href);
    if(url.origin!==location.origin||!navigator?.handles(url))return;
    event.preventDefault();navigate(url.href);
  };
  document.addEventListener("click",click);return()=>document.removeEventListener("click",click);
}
