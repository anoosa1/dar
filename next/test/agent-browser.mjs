import assert from "node:assert/strict";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {chromium} from "@playwright/test";
import {choose} from "./ui-helpers.mjs";
const base=process.env.DAR_INTEGRATION_ORIGIN,root=process.env.DAR_TEST_ROOT;
if(!base||!root)throw Error("Use an isolated acceptance fixture.");
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true});
const context=await browser.newContext({viewport:{width:1440,height:950}}),page=await context.newPage(),errors=[];page.on("pageerror",e=>errors.push(e.message));
try{
 await page.goto(base);await page.getByLabel("Username",{exact:true}).fill("admin");await page.getByLabel("Password",{exact:true}).fill("a long isolated test password");await page.getByRole("button",{name:"Sign in",exact:true}).click();await page.getByRole("button",{name:"Open applications",exact:true}).click();await page.getByRole("menuitem",{name:"Agent",exact:true}).click();
 await page.getByRole("heading",{name:"Agent",exact:true}).waitFor();
 const marker="Browser project "+randomUUID().slice(0,6);
 await page.getByRole("button",{name:"New project",exact:true}).first().click();await page.getByLabel("Project name",{exact:true}).fill(marker);await page.getByLabel("Repository URL",{exact:true}).fill("https://example.test/you/repo.git");await page.getByRole("button",{name:"Create project",exact:true}).click();await page.getByRole("heading",{name:"Start a thread",exact:true}).waitFor();
 await page.getByRole("button",{name:"New thread",exact:true}).first().click();await page.getByLabel("Thread title",{exact:true}).fill("Keep this conversation");await page.getByRole("button",{name:"Create thread",exact:true}).click();await page.getByLabel("Message",{exact:true}).waitFor();
 const message="Build keyboard navigation.\n<script>window.agentInjected = true</script>",draft="A follow-up draft";
 await page.getByLabel("Message",{exact:true}).fill(message);await page.getByRole("button",{name:"Save message",exact:true}).click();await page.getByText("Message saved. It has not been sent to a model.",{exact:true}).waitFor();assert.equal(await page.evaluate(()=>window.agentInjected),undefined);
 await page.getByLabel("Message",{exact:true}).fill(draft);const deep=page.url();await page.reload();await page.getByText(message,{exact:true}).waitFor();assert.equal(await page.getByLabel("Message",{exact:true}).inputValue(),draft);
 const session=await(await page.request.get(base+"/api/v1/auth/session")).json(),threadId=new URL(deep).searchParams.get("thread"),history=await(await page.request.get(`${base}/api/v1/agent/threads/${threadId}/messages`)).json();assert.equal(history.items.at(-1).content,message);
 const projectId=new URL(deep).searchParams.get("project"),remote=await page.request.post(`${base}/api/v1/agent/threads/${threadId}/messages`,{headers:{origin:base,"x-csrf-token":session.csrfToken,"if-match":`"${history.thread.revision}"`},data:{id:randomUUID(),content:"From another window"}});assert.equal(remote.status(),201);
 await page.getByRole("button",{name:"Save message",exact:true}).click();await page.getByText(/This thread changed in another window/).waitFor();assert.equal(await page.getByLabel("Message",{exact:true}).inputValue(),draft);await page.getByRole("button",{name:"Refresh conversation",exact:true}).click();await page.getByText("From another window",{exact:true}).waitFor();await page.getByRole("button",{name:"Save message",exact:true}).click();await page.getByText("Message saved. It has not been sent to a model.",{exact:true}).waitFor();
 const uncertain="Saved while the acknowledgement was lost",endpoint=`${base}/api/v1/agent/threads/${threadId}/messages`;
 await page.route(endpoint,async route=>{if(route.request().method()==="POST"){await route.fetch();await route.fulfill({status:503,contentType:"application/json",body:JSON.stringify({error:{message:"Acknowledgement lost"}})});}else await route.continue();});
 await page.getByLabel("Message",{exact:true}).fill(uncertain);await page.getByRole("button",{name:"Save message",exact:true}).click();await page.getByText("Acknowledgement lost",{exact:true}).waitFor();await page.unroute(endpoint);
 await page.reload();await page.getByLabel("Conversation messages",{exact:true}).getByText(uncertain,{exact:true}).waitFor();assert.equal(await page.getByLabel("Message",{exact:true}).inputValue(),uncertain);await page.getByRole("button",{name:"Save message",exact:true}).click();await page.getByText("Message saved. It has not been sent to a model.",{exact:true}).waitFor();const replayed=await(await page.request.get(endpoint)).json();assert.equal(replayed.items.filter(m=>m.content===uncertain).length,1);
 await page.setViewportSize({width:390,height:844});await page.getByRole("button",{name:"Changes",exact:true}).click();await page.locator('.agent-review').waitFor({state:"visible"});await page.keyboard.press("Escape");await page.locator('.agent-conversation').waitFor({state:"visible"});assert.equal(await page.getByRole("button",{name:"Conversation",exact:true}).evaluate(el=>document.activeElement===el),true);
 for(const theme of ["latte","mocha"]){
 await page.evaluate(value=>{document.documentElement.dataset.theme=value;},theme);
 for(const [width,height]of [[1440,950],[900,760],[801,700],[390,844],[320,568]]){
  await page.setViewportSize({width,height});
  if(width<=800){for(const name of ["Projects","Changes","Terminal","Conversation"]){await page.getByRole("button",{name,exact:true}).click();const selector={Projects:".agent-sidebar",Changes:".agent-review",Terminal:".agent-terminal",Conversation:".agent-conversation"}[name];await page.locator(selector).waitFor({state:"visible"});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);}}
  const layout=await page.evaluate(()=>({overflow:document.documentElement.scrollWidth>innerWidth,height:document.documentElement.scrollHeight,header:document.querySelector('.topbar').getBoundingClientRect().top}));assert.equal(layout.overflow,false);assert.ok(layout.height<=height+1,JSON.stringify(layout));assert.equal(layout.header,0);
  await page.screenshot({path:path.join(root,`agent-${theme}-${width}.png`),fullPage:true});
 }
 }
 await page.setViewportSize({width:1440,height:950});await page.getByRole("button",{name:"New project",exact:true}).first().click();await choose(page.getByRole("combobox",{name:"Project source",exact:true}),"files");await page.locator('.agent-folder-picker li button').first().waitFor();await page.locator('.agent-folder-picker li button').first().click();await page.getByRole("button",{name:"Choose this folder",exact:true}).click();await page.getByRole("button",{name:"Create project",exact:true}).click();await page.getByRole("link",{name:"Open folder in Files",exact:true}).waitFor();
 const projects=await(await page.request.get(base+"/api/v1/agent/projects")).json();assert.ok(projects.items.some(p=>p.sourceType==="files"));assert.ok(projects.items.some(p=>p.id===projectId&&p.sourceType==="git"));
 await page.getByRole("button",{name:"Agent settings",exact:true}).click();await page.getByRole("dialog",{name:"Agent settings",exact:true}).waitFor();await page.keyboard.press("Escape");assert.equal(await page.getByRole("dialog",{name:"Agent settings",exact:true}).count(),0);
 assert.deepEqual(errors,[]);console.log("PASS Agent projects, sources, messages, drafts, conflicts and responsive panes.");
}finally{await browser.close();}
