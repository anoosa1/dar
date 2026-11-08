import {Context} from "cordis";
import {pathToFileURL} from "node:url";
import path from "node:path";
import type {loadApps} from "./registry.ts";
import type {Activate,ServerApp} from "../../sdk/server.ts";
/** Trusted Node hooks receive Cordis-owned effects, not Dar's service credential. */
export async function activateApps(registry:Awaited<ReturnType<typeof loadApps>>){
  const handlers=new Map<string,ServerApp>(),contexts:Context[]=[];
  for(const [id,app] of registry.apps){
    if(!app.manifest.server)continue;
    const ctx=new Context(),controller=new AbortController();contexts.push(ctx);
    ctx.effect(()=>()=>controller.abort());
    let timer:ReturnType<typeof setTimeout>|undefined;
    try{
      const task=(async()=>{const module=await import(pathToFileURL(path.join(app.root,app.manifest.server!)).href+`?digest=${app.descriptor.digest}`) as {activate:Activate};if(typeof module.activate!=="function")throw Error("Missing activate");const handler=await module.activate({signal:controller.signal,effect:start=>{ctx.effect(start);}});if(typeof handler?.handle!=="function")throw Error("Missing handle");return handler;})();
      const handler=await Promise.race([task,new Promise<never>((_,reject)=>{timer=setTimeout(()=>reject(Error("App startup timed out")),5000);})]);handlers.set(id,handler);
    }catch{registry.apps.delete(id);const item=registry.inventory.find(i=>i.id===id);if(item)item.status="App startup failed. Disable or replace the package, then restart.";controller.abort();await ctx.fiber.dispose();}
    finally{clearTimeout(timer);}
  }
  return {handlers,close:async()=>{await Promise.allSettled(contexts.map(ctx=>ctx.fiber.dispose()));}};
}
