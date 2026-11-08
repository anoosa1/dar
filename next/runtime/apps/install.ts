import {mkdir,readFile,writeFile,lstat,rename,rm,open} from "node:fs/promises";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {readAppPackage,type PackageInput} from "./registry.ts";
import {createDurableFile,replaceDurableFile} from "../atomic.ts";
import {openRoot} from "../root.ts";
import {lockRoot} from "../lock.ts";
/** Offline operator changes: the root lock requires a deliberate service stop/restart. */
export async function manageApps(rootPath:string,command:string,argument?:string){
  const root=await openRoot(rootPath),release=await lockRoot(root);
  try{
    const file=path.join(root,"apps.json");let inputs:PackageInput[]=[];
    try{const stat=await lstat(file);if(!stat.isFile()||stat.isSymbolicLink()||(stat.mode&0o077))throw Error("Unsafe app registry.");inputs=JSON.parse(await readFile(file,"utf8"));if(!Array.isArray(inputs))throw Error("Invalid registry.");}catch(e){if((e as NodeJS.ErrnoException).code!=="ENOENT")throw e;}
    if(command==="apps-list")return Promise.all(inputs.map(async input=>{const app=await readAppPackage(input);return {id:app.manifest.id,version:app.manifest.version,enabled:input.enabled!==false,digest:app.descriptor.digest};}));
    if(command==="apps-install"&&argument){
      const app=await readAppPackage({path:path.resolve(argument)}),packages=path.join(root,"appPackages");await mkdir(packages,{mode:0o700}).catch((error:NodeJS.ErrnoException)=>{if(error.code!=="EEXIST")throw error;});
      const stat=await lstat(packages);if(!stat.isDirectory()||stat.isSymbolicLink())throw Error("Unsafe package directory.");
      const destination=path.join(packages,app.descriptor.digest),staging=path.join(packages,`.${randomUUID()}.tmp`);await mkdir(staging,{mode:0o700});
      try{
        for(const [name,bytes] of app.assets){const target=path.join(staging,name);await mkdir(path.dirname(target),{recursive:true,mode:0o700});await writeFile(target,bytes,{mode:0o600,flag:"wx"});const handle=await open(target,"r");try{await handle.sync();}finally{await handle.close();}}
        await createDurableFile(path.join(staging,"dar-app.json"),await readFile(path.join(app.root,"dar-app.json"),"utf8"));
        await readAppPackage({path:staging,digest:app.descriptor.digest});
        const directories=new Set([staging]);for(const name of app.assets.keys()){let directory=path.dirname(path.join(staging,name));while(directory.startsWith(staging)){directories.add(directory);if(directory===staging)break;directory=path.dirname(directory);}}
        for(const directory of [...directories].sort((a,b)=>b.length-a.length)){const handle=await open(directory,"r");try{await handle.sync();}finally{await handle.close();}}
        try{await rename(staging,destination);}catch(e){if(!["EEXIST","ENOTEMPTY"].includes((e as NodeJS.ErrnoException).code??""))throw e;await readAppPackage({path:destination,digest:app.descriptor.digest});}
        const parent=await open(packages,"r");try{await parent.sync();}finally{await parent.close();}
      }finally{await rm(staging,{recursive:true,force:true});}
      const retained:PackageInput[]=[];for(const input of inputs){const existing=await readAppPackage(input);if(existing.manifest.id!==app.manifest.id)retained.push(input);}inputs=[...retained,{path:destination,digest:app.descriptor.digest,enabled:true}];
    }else if(["apps-enable","apps-disable","apps-uninstall"].includes(command)&&argument){
      let found=false;const updated:PackageInput[]=[];
      for(const input of inputs){const app=await readAppPackage(input);if(app.manifest.id!==argument){updated.push(input);continue;}found=true;if(command!=="apps-uninstall")updated.push({...input,enabled:command==="apps-enable"});}
      if(!found)throw Error("App not found.");inputs=updated;
    }else throw Error("Usage: apps-list, apps-install <built-package-directory>, apps-enable|apps-disable|apps-uninstall <app-id>");
    try{await replaceDurableFile(file,JSON.stringify(inputs,null,2));}catch(e){if((e as NodeJS.ErrnoException).code!=="ENOENT")throw e;await createDurableFile(file,JSON.stringify(inputs,null,2));}
    return "App configuration saved. Restart Dar to apply it. App data is retained.";
  }finally{await release();}
}
