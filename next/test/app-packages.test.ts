import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,writeFile,symlink} from "node:fs/promises";
import {tmpdir} from "node:os";
import path from "node:path";
import {readAppPackage,loadApps} from "../runtime/apps/registry.ts";
import {activateApps} from "../runtime/apps/server.ts";
import {manageApps} from "../runtime/apps/install.ts";
const manifest={id:"org.example.clock",name:"Clock",version:"1.0.0",apiVersion:1,license:"MIT",client:"client.js",styles:[],files:["client.js"],permissions:["ownUserData:read"]};
test("packages reject escaping paths, changed digests, duplicate IDs and symlinks before activation",async()=>{
  const root=await mkdtemp(path.join(tmpdir(),"dar-app-package-")),file=path.join(root,"dar-app.json");
  await writeFile(file,JSON.stringify(manifest));await writeFile(path.join(root,"client.js"),"export function mount() { return () => {}; }");
  const first=await readAppPackage({path:root});assert.equal(first.descriptor.name,"Clock");
  await writeFile(path.join(root,"client.js"),"different bytes");await assert.rejects(readAppPackage({path:root,digest:first.descriptor.digest}),/digest/);
  await writeFile(file,JSON.stringify({...manifest,files:["../escape.js"],client:"../escape.js"}));await assert.rejects(readAppPackage({path:root}),/manifest/);
  await writeFile(file,JSON.stringify({...manifest,apiVersion:2}));await assert.rejects(readAppPackage({path:root}),/manifest/);
  await writeFile(file,JSON.stringify({...manifest,files:["link.js"],client:"link.js"}));await symlink(path.join(root,"client.js"),path.join(root,"link.js"));await assert.rejects(readAppPackage({path:root}),/symlink/);
  await writeFile(file,JSON.stringify(manifest));const registry=path.join(root,"registry.json");await writeFile(registry,JSON.stringify([{path:root},{path:root}]));const loaded=await loadApps(registry);assert.equal(loaded.apps.size,1);assert.match(loaded.inventory[1].status,/duplicate/);
});
test("optional server hooks dispose effects and a failed app leaves other apps available",async()=>{
  const root=await mkdtemp(path.join(tmpdir(),"dar-app-lifecycle-"));await writeFile(path.join(root,"package.json"),'{"type":"module"}');
  await writeFile(path.join(root,"client.js"),"export function mount(){};");
  await writeFile(path.join(root,"server.js"),'export function activate(ctx){ctx.effect(()=>()=>{globalThis.darAppDisposed=true;});return {handle:async()=>({ok:true})};}');
  await writeFile(path.join(root,"dar-app.json"),JSON.stringify({...manifest,server:"server.js",files:["client.js","server.js"]}));const file=path.join(root,"registry.json");await writeFile(file,JSON.stringify([{path:root}]));
  const registry=await loadApps(file),runtime=await activateApps(registry);assert.equal(runtime.handlers.size,1);await runtime.close();assert.equal((globalThis as {darAppDisposed?:boolean}).darAppDisposed,true);delete (globalThis as {darAppDisposed?:boolean}).darAppDisposed;
  await writeFile(path.join(root,"server.js"),'throw Error("broken app");');const broken=await loadApps(file),failed=await activateApps(broken);assert.equal(broken.apps.size,0);assert.match(broken.inventory[0].status,/startup failed/);await failed.close();
});
test("operator can reinstall, disable and remove a package without replacing the managed root",async()=>{
  const root=await mkdtemp(path.join(tmpdir(),"dar-app-installer-")),source=await mkdtemp(path.join(tmpdir(),"dar-app-source-"));await writeFile(path.join(source,"dar-app.json"),JSON.stringify(manifest));await writeFile(path.join(source,"client.js"),"export function mount(){}");
  for(let i=0;i<2;i++)await manageApps(root,"apps-install",source);
  await manageApps(root,"apps-disable",manifest.id);assert.equal((await manageApps(root,"apps-list") as {enabled:boolean}[])[0].enabled,false);
  await manageApps(root,"apps-uninstall",manifest.id);assert.deepEqual(await manageApps(root,"apps-list"),[]);
  await manageApps(root,"apps-install",source);assert.equal((await manageApps(root,"apps-list") as unknown[]).length,1);
});
