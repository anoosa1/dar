import assert from "node:assert/strict";
import {chromium} from "@playwright/test";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {login} from "./integration-helpers.ts";

const base=process.env.DAR_INTEGRATION_ORIGIN;if(!base)throw Error("Use an isolated fixture.");
const admin=await login(base),headers={...admin.headers,"content-type":"application/json"},endpoint=base+"/api/v1/admin/installedApps";
const read=async()=>{const r=await fetch(endpoint,{headers});assert.equal(r.status,200);return r.json();};
async function change(id,enabled,revision){return fetch(endpoint,{method:"PUT",headers:{...headers,"if-match":`"apps:${revision}"`},body:JSON.stringify({id,enabled,revision})});}
const initial=await read(),external=initial.items.find(app=>app.version&&app.canToggle);assert.ok(external);
const marker="launcher-"+randomUUID().slice(0,8),dataUrl=base+`/api/v1/apps/${external.id}/userData/${marker}`;
assert.equal((await fetch(dataUrl,{method:"PUT",headers:{...headers,"if-match":'"0"'},body:JSON.stringify({schemaVersion:1,value:{marker}})})).status,200);
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true}),errors=[];let page;
try{
  const context=await browser.newContext({viewport:{width:1440,height:950}});page=await context.newPage();page.on("pageerror",e=>errors.push(e.message));
  await page.goto(base+"/settings?section=installedApps");await page.getByLabel("Username",{exact:true}).fill("admin");await page.getByLabel("Password",{exact:true}).fill("a long isolated test password");await page.getByRole("button",{name:"Sign in",exact:true}).click();
  await page.getByRole("heading",{name:"Apps",exact:true}).waitFor();
  for(const id of ["files","contacts","calendar"]){
    const name=id[0].toUpperCase()+id.slice(1),row=page.getByRole("region",{name,exact:true});assert.equal(await row.getByRole("button").count(),0);assert.equal((await row.innerText()).trim(),name);
    assert.equal((await change(id,false,(await read()).revision)).status,403);
  }
  for(const theme of ["latte","mocha"]){
    await page.evaluate(value=>{document.documentElement.dataset.theme=value;},theme);
    for(const viewport of [{width:1440,height:950},{width:390,height:568}]){
      await page.setViewportSize(viewport);const opener=page.getByRole("button",{name:"Open applications",exact:true});await opener.click();const menu=page.getByRole("menu",{name:"Open applications",exact:true});await menu.waitFor();
      const boxes=await menu.getByRole("menuitem").evaluateAll(items=>items.map(item=>({x:item.getBoundingClientRect().x,y:item.getBoundingClientRect().y})));assert.ok(boxes.every((box,i)=>i===0||box.y>boxes[i-1].y&&Math.abs(box.x-boxes[0].x)<1));
      if(viewport.width===390){const box=await menu.boundingBox();assert.ok(box.x>=12&&box.x+box.width<=viewport.width-12&&box.y>=0&&box.y+box.height<=viewport.height-12,JSON.stringify(box));}
      await menu.getByRole("menuitem",{name:"Manage apps",exact:true}).waitFor();await page.keyboard.press("End");assert.equal(await menu.getByRole("menuitem",{name:"Manage apps",exact:true}).evaluate(el=>el===document.activeElement),true);await page.keyboard.press("Escape");assert.equal(await opener.evaluate(el=>el===document.activeElement),true);
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);await opener.click();await menu.waitFor();await page.screenshot({animations:"disabled",path:path.join(process.env.DAR_TEST_ROOT,`app-launcher-${theme}-${viewport.width}.png`)});await page.keyboard.press("Escape");
    }
  }
  await page.setViewportSize({width:1440,height:950});
  const stale=(await read()).revision;
  for(const name of ["Notes","Tasks",external.name]){await page.getByRole("button",{name:`Disable ${name}`,exact:true}).click();await page.getByRole("button",{name:`Enable ${name}`,exact:true}).waitFor();}
  assert.equal((await change("notes",true,stale)).status,409);
  await page.reload();await page.getByRole("button",{name:"Enable Notes",exact:true}).waitFor();
  await page.getByRole("button",{name:"Open applications",exact:true}).click();const menu=page.getByRole("menu",{name:"Open applications",exact:true});for(const name of ["Notes","Tasks",external.name])assert.equal(await menu.getByRole("menuitem",{name,exact:true}).count(),0);await page.keyboard.press("Escape");
  for(const route of ["/api/v1/notes/notebooks","/api/v1/tasks/items",`/api/v1/apps/${external.id}/userData/${marker}`])assert.equal((await fetch(base+route,{headers})).status,404);
  for(const q of ["Notes","Tasks",external.name]){const r=await(await fetch(base+`/api/v1/search?q=${encodeURIComponent(q)}`,{headers})).json();assert.ok(!r.items.some(item=>["/notes","/tasks",`/apps/${external.id}`].includes(item.href)));}
  await page.goto(base+"/notes");await page.getByRole("heading",{name:"App disabled",exact:true}).waitFor();
  const username="launcher-"+randomUUID().slice(0,6);assert.equal((await fetch(base+"/api/v1/auth/users",{method:"POST",headers,body:JSON.stringify({username,displayName:"Launcher user",password:"a long isolated test password"})})).status,201);const user=await login(base,username);assert.equal((await fetch(endpoint,{method:"PUT",headers:{...user.headers,"content-type":"application/json"},body:JSON.stringify({id:"notes",enabled:true,revision:(await read()).revision})})).status,403);
  const userContext=await browser.newContext({viewport:{width:390,height:568}}),userPage=await userContext.newPage();const cookie=user.headers.cookie.split("=");await userContext.addCookies([{name:cookie[0],value:cookie[1],url:base}]);await userPage.goto(base);await userPage.getByRole("button",{name:"Open applications",exact:true}).click();const userMenu=userPage.getByRole("menu",{name:"Open applications",exact:true});await userMenu.waitFor();assert.equal(await userMenu.getByRole("menuitem",{name:"Manage apps",exact:true}).count(),0);assert.equal(await userMenu.getByRole("menuitem",{name:"Files",exact:true}).getAttribute("aria-current"),"page");await userContext.close();
  console.info("PASS app menu desktop/phone/themes/keyboard, protected apps, persisted disabling, API/search enforcement and stale/unauthorized writes.");
}finally{
  for(const id of ["notes","tasks",external.id])assert.equal((await change(id,true,(await read()).revision)).status,200);
  assert.deepEqual((await(await fetch(dataUrl,{headers})).json()).value,{marker});await fetch(dataUrl,{method:"DELETE",headers:{...headers,"if-match":'"1"'}});
  await browser.close();
}
assert.deepEqual(errors,[]);
