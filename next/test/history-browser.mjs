import assert from "node:assert/strict";
import {chromium} from "@playwright/test";
import {randomUUID} from "node:crypto";
import path from "node:path";
import {login,upload,connect} from "./integration-helpers.ts";
import {fileDetails} from "./ui-helpers.mjs";
const base=process.env.DAR_INTEGRATION_ORIGIN;if(!base)throw Error("Use an isolated instance.");
const admin=await login(base),username=`history-ui-${randomUUID().slice(0,6)}`,password="a long isolated test password";
assert.equal((await fetch(base+"/api/v1/auth/users",{method:"POST",headers:{...admin.headers,"content-type":"application/json"},body:JSON.stringify({username,displayName:"History tester",password})})).status,201);
const native=await login(base,username),db=await connect(native),folderId=randomUUID();
await db.reducers.createFolder({id:folderId,parentId:"",name:"Project"});db.disconnect();
const readme=await upload(base,native,"README.md","# Project\n\nFirst line\n",folderId);await upload(base,native,"photo.bin",Buffer.from([0,1,2,3]),folderId);
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true}),errors=[];
try{
  const context=await browser.newContext({viewport:{width:1440,height:1000}}),page=await context.newPage();page.setDefaultTimeout(15000);
  page.on("pageerror",e=>errors.push(e.message));page.on("console",m=>{if(m.type()==="error"&&!/401.*Unauthorized/.test(m.text()))errors.push(m.text());});
  await page.goto(base);await page.getByLabel("Username",{exact:true}).fill(username);await page.getByLabel("Password",{exact:true}).fill(password);await page.getByRole("button",{name:"Sign in",exact:true}).click();
  await fileDetails(page,"Project");const details=page.getByRole("complementary",{name:"File details",exact:true});
  await details.getByRole("tab",{name:"History",exact:true}).click();
  await details.getByRole("button",{name:"Track history",exact:true}).click();
  await details.getByText("History is on for this folder.",{exact:true}).waitFor();
  await details.getByText("README.md",{exact:true}).waitFor();
  await page.screenshot({animations:"disabled",path:path.join(process.env.DAR_TEST_ROOT,"history-status-1440.png")});

  // Review the changes in the editing pane, then commit with a message.
  await details.getByRole("button",{name:"Review and commit",exact:true}).click();
  const pane=page.getByRole("complementary",{name:"Commit changes",exact:true});
  assert.equal(await pane.getByLabel("Message",{exact:true}).evaluate(el=>el===document.activeElement),true);
  await pane.getByRole("button",{name:"README.md",exact:true}).click();
  await pane.getByRole("region",{name:"Changes in README.md",exact:true}).getByText("First line",{exact:true}).waitFor();
  await pane.getByRole("button",{name:"photo.bin",exact:true}).click();await pane.getByText(/No text preview/).waitFor();
  await pane.getByLabel("Message",{exact:true}).fill("Start the project");
  await page.screenshot({animations:"disabled",path:path.join(process.env.DAR_TEST_ROOT,"history-commit-1440.png")});
  await pane.getByRole("button",{name:"Commit",exact:true}).click();await pane.waitFor({state:"detached"});
  await details.getByText("Committed 2 changes.",{exact:true}).waitFor();await details.getByText("Everything is committed.",{exact:true}).waitFor();
  await details.getByRole("button",{name:/^Start the project/}).waitFor();

  // A change made elsewhere shows up; a snapshot commits it without a message.
  await upload(base,native,"README.md","# Project\n\nSecond line\n",folderId,readme.fileId,1);
  await details.getByRole("button",{name:"Snapshot",exact:true}).waitFor();
  await details.getByRole("button",{name:"Snapshot",exact:true}).click();await details.getByText("Snapshot of 1 change committed.",{exact:true}).waitFor();

  // Restoring the first commit brings back its content and records a restore.
  await details.getByRole("button",{name:/^Start the project/}).click();
  const changeset=page.getByRole("complementary",{name:"Commit",exact:true});
  await changeset.getByRole("button",{name:"README.md",exact:true}).click();await changeset.getByRole("region",{name:"Changes in README.md",exact:true}).waitFor();
  await changeset.getByRole("button",{name:"Restore folder to this point",exact:true}).click();
  const confirm=page.getByRole("dialog",{name:"Restore this folder?",exact:true});await confirm.getByRole("button",{name:"Restore folder",exact:true}).click();
  await details.getByText("Folder restored to “Start the project”.",{exact:true}).waitFor();
  await details.getByRole("button",{name:/^Restore to “Start the project”/}).waitFor();
  assert.equal(await(await fetch(`${base}/api/v1/files/${readme.fileId}/content`,{headers:native.headers})).text(),"# Project\n\nFirst line\n");

  // Keyboard: Escape closes an opened commit and returns focus to the history.
  await details.getByRole("button",{name:/^Snapshot/}).last().click();
  await page.getByRole("complementary",{name:"Snapshot",exact:true}).waitFor();await page.keyboard.press("Escape");
  await page.getByRole("complementary",{name:"Snapshot",exact:true}).waitFor({state:"detached"});

  for(const theme of ["latte","mocha"])for(const width of [1440,390]){
    await page.evaluate(value=>{document.documentElement.dataset.theme=value;},theme);await page.setViewportSize({width,height:844});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,`${theme} ${width}`);
    await page.screenshot({animations:"disabled",path:path.join(process.env.DAR_TEST_ROOT,`history-${theme}-${width}.png`)});
  }
  await page.setViewportSize({width:1440,height:1000});

  // Stopping history asks first and returns the folder to untracked.
  await details.getByRole("button",{name:"Stop keeping history",exact:true}).click();
  await page.getByRole("dialog",{name:"Stop keeping history?",exact:true}).getByRole("button",{name:"Delete history",exact:true}).click();
  await details.getByRole("button",{name:"Track history",exact:true}).waitFor();
  assert.deepEqual(errors,[]);
  console.info("PASS folder history browser checks.");
}finally{await browser.close();}
