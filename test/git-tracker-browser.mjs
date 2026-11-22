import assert from "node:assert/strict";
import {chromium} from "@playwright/test";
import {randomUUID} from "node:crypto";
import path from "node:path";
import {mkdtemp,rm,writeFile} from "node:fs/promises";
import {tmpdir} from "node:os";
import {spawnSync} from "node:child_process";
import {login,upload,connect} from "./integration-helpers.ts";
const base=process.env.DAR_INTEGRATION_ORIGIN;if(!base)throw Error("Use an isolated instance.");
const shots=name=>path.join(process.env.DAR_TEST_ROOT,name);

// An owner with a repository and a pushed branch, and a reader it is shared with.
const admin=await login(base),password="a long isolated test password",owner=`tracker-ui-${randomUUID().slice(0,6)}`,reader=`reader-ui-${randomUUID().slice(0,6)}`;
for(const [username,displayName] of [[owner,"Grace Hopper"],[reader,"Katherine Johnson"]])
  assert.equal((await fetch(base+"/api/v1/auth/users",{method:"POST",headers:{...admin.headers,"content-type":"application/json"},body:JSON.stringify({username,displayName,password})})).status,201);
const user=await login(base,owner),db=await connect(user),folderId=randomUUID(),headers={...user.headers,"content-type":"application/json"};
await db.reducers.createFolder({id:folderId,parentId:"",name:"Compiler"});
await upload(base,user,"README.md","# Compiler\n",folderId);
assert.equal((await fetch(`${base}/api/v1/files/history/${folderId}`,{method:"POST",headers,body:JSON.stringify({action:"track"})})).status,200);
const overview=await(await fetch(`${base}/api/v1/files/history/${folderId}`,{headers:user.headers})).json();
assert.equal((await fetch(`${base}/api/v1/files/history/${folderId}/commits`,{method:"POST",headers,body:JSON.stringify({message:"Start",token:overview.status.token})})).status,201);
await db.reducers.browse({parentId:"",section:"files",query:"",cursor:""});
const folder=[...db.db.myFiles.iter()].find(row=>row.id===folderId);
await db.reducers.grantAccess({id:randomUUID(),fileId:folderId,username:reader,role:"read",expectedRevision:folder.revision});db.disconnect();
const secret=(await(await fetch(`${base}/api/v1/auth/app-passwords`,{method:"POST",headers,body:JSON.stringify({label:"Git",scope:"write"})})).json()).secret;
const work=await mkdtemp(path.join(tmpdir(),"dar-tracker-ui-"));
const env={...process.env,GIT_CONFIG_GLOBAL:"/dev/null",GIT_CONFIG_NOSYSTEM:"1",HOME:work,GIT_TERMINAL_PROMPT:"0",GIT_AUTHOR_NAME:"Grace Hopper",GIT_AUTHOR_EMAIL:"grace@example.com",GIT_COMMITTER_NAME:"Grace Hopper",GIT_COMMITTER_EMAIL:"grace@example.com"};
const git=(cwd,...args)=>{const result=spawnSync("git",["-c",`http.extraHeader=Authorization: Basic ${Buffer.from(`${owner}:${secret}`).toString("base64")}`,...args],{cwd,env,encoding:"utf8"});assert.equal(result.status,0,`git ${args.join(" ")}\n${result.stderr}`);return result.stdout.trim();};
git(work,"clone","-q",`${base}/dav/git/${owner}/Compiler.git`,"compiler");const clone=path.join(work,"compiler");
git(clone,"checkout","-q","-b","fix/overflow");await writeFile(path.join(clone,"lexer.c"),"int read(void){\n  return checked();\n}\n");git(clone,"add","-A");git(clone,"commit","-qm","Check token length");git(clone,"push","-q","origin","fix/overflow");

const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true}),errors=[];
async function signIn(context,username){
  const page=await context.newPage();page.setDefaultTimeout(15000);
  page.on("pageerror",e=>errors.push(e.message));page.on("console",m=>{if(m.type()==="error"&&!/401.*Unauthorized/.test(m.text()))errors.push(m.text());});
  await page.goto(`${base}/git?repo=${folderId}&tab=issues`);
  await page.getByLabel("Username",{exact:true}).fill(username);await page.getByLabel("Password",{exact:true}).fill(password);await page.getByRole("button",{name:"Sign in",exact:true}).click();
  return page;
}
try{
  const context=await browser.newContext({viewport:{width:1440,height:1000}}),page=await signIn(context,owner);
  const repo=page.getByRole("main",{name:"Compiler",exact:true});
  await repo.getByRole("tab",{name:/^Issues/}).waitFor();
  await repo.getByRole("heading",{name:"No open issues"}).waitFor();

  // Labels, in the right editing pane.
  await repo.getByRole("button",{name:"Labels",exact:true}).click();
  const labels=page.getByRole("complementary",{name:"Labels",exact:true});
  await labels.getByLabel("Name",{exact:true}).fill("bug");await labels.getByLabel("Description",{exact:true}).fill("Something is wrong");
  await labels.getByRole("radio",{name:"Rose"}).check();await labels.getByRole("button",{name:"Create label"}).click();
  await labels.getByRole("list").getByText("bug",{exact:true}).waitFor();
  await page.screenshot({animations:"disabled",path:shots("git-labels-1440.png")});
  await labels.getByRole("button",{name:"Close"}).first().click();await labels.waitFor({state:"detached"});

  // A new issue, with a label and a Markdown description; it opens in the right sheet.
  await repo.getByRole("button",{name:"New issue",exact:true}).click();
  const editor=page.getByRole("complementary",{name:"New issue",exact:true});
  await editor.getByLabel("Title",{exact:true}).fill("Long identifiers overflow the buffer");
  await editor.getByLabel("Description",{exact:true}).fill("Names over **255** characters crash the lexer.");
  await editor.getByRole("checkbox",{name:/bug/}).check();
  await editor.getByRole("radio",{name:"Preview"}).click();await editor.getByText("255",{exact:true}).waitFor();
  await editor.getByRole("button",{name:"Open issue"}).click();await editor.waitFor({state:"detached"});
  let sheet=page.getByRole("complementary",{name:"Conversation",exact:true});
  await sheet.getByRole("heading",{name:"Issue #1"}).waitFor();
  await sheet.getByText("Long identifiers overflow the buffer").waitFor();
  assert.equal(new URL(page.url()).searchParams.get("issue"),"1");
  await repo.getByRole("list",{name:"Issues"}).getByRole("button",{name:/Long identifiers overflow the buffer/}).waitFor();

  // Comment, then close with a comment and reopen.
  await sheet.getByLabel("Add a comment",{exact:true}).fill("Reproduced with a 300 character name.");
  await sheet.getByRole("button",{name:"Comment",exact:true}).click();
  await sheet.getByRole("article",{name:`Comment by Grace Hopper`}).getByText("Reproduced with a 300 character name.").waitFor();
  await sheet.getByRole("button",{name:"Close issue"}).click();
  await sheet.getByText("closed this").waitFor();await sheet.getByRole("button",{name:"Reopen"}).click();await sheet.getByText("reopened this").waitFor();
  await page.screenshot({animations:"disabled",path:shots("git-issue-1440.png")});

  // A pull request from the pushed branch: the pane compares it; "Closes #1" is in the description.
  await repo.getByRole("tab",{name:/^Pull requests/}).click();
  await repo.getByRole("button",{name:"New pull request",exact:true}).click();
  const pullEditor=page.getByRole("complementary",{name:"New pull request",exact:true});
  await pullEditor.getByText("1 commit · 1 file · Can fast-forward").waitFor();
  assert.equal(await pullEditor.getByLabel("Title",{exact:true}).inputValue(),"Check token length");
  await pullEditor.getByLabel("Description",{exact:true}).fill("Limits names to 255 characters. Closes #1.");
  await page.screenshot({animations:"disabled",path:shots("git-new-pull-1440.png")});
  await pullEditor.getByRole("button",{name:"Open pull request"}).click();await pullEditor.waitFor({state:"detached"});
  sheet=page.getByRole("complementary",{name:"Conversation",exact:true});
  await sheet.getByRole("heading",{name:"Pull request #2"}).waitFor();
  await sheet.getByRole("radio",{name:/^Commits/}).click();await sheet.getByRole("button",{name:/Check token length/}).waitFor();
  await sheet.getByRole("radio",{name:/^Files/}).click();await sheet.getByRole("region",{name:"Changes to lexer.c"}).getByText("return checked();").waitFor();
  await page.screenshot({animations:"disabled",path:shots("git-pull-files-1440.png")});
  await sheet.getByRole("radio",{name:"Conversation"}).click();
  await sheet.getByRole("region",{name:"Merge fix/overflow into main"}).getByRole("radio",{name:"Merge commit"}).click();
  await sheet.getByRole("button",{name:"Merge into main"}).click();
  await sheet.getByText("Merged",{exact:true}).waitFor();await sheet.getByText(/merged this as/).waitFor();
  await page.screenshot({animations:"disabled",path:shots("git-pull-merged-1440.png")});
  // The issue it named is closed, and #1 in the description opens it.
  await sheet.getByRole("button",{name:"#1",exact:true}).click();
  await sheet.getByRole("heading",{name:"Issue #1"}).waitFor();await sheet.getByText("Closed",{exact:true}).waitFor();await sheet.getByText(/closed this with/).waitFor();

  // Saved: a reload reopens the same issue from the address.
  await page.reload();
  await page.getByRole("complementary",{name:"Conversation",exact:true}).getByRole("heading",{name:"Issue #1"}).waitFor();
  await page.getByRole("complementary",{name:"Conversation",exact:true}).getByText("Reproduced with a 300 character name.").waitFor();

  // Phones: the list fills the screen; an issue opens full screen with Back.
  const phone=await browser.newContext({viewport:{width:390,height:844},isMobile:true,hasTouch:true}),small=await signIn(phone,owner);
  await small.getByRole("tab",{name:/^Issues/}).click();await small.getByRole("radio",{name:/Closed/}).click();
  await small.getByRole("list",{name:"Issues"}).getByRole("button",{name:/Long identifiers/}).waitFor();
  await small.screenshot({animations:"disabled",path:shots("git-issues-390.png")});
  await small.getByRole("list",{name:"Issues"}).getByRole("button",{name:/Long identifiers/}).click();
  const smallSheet=small.getByRole("complementary",{name:"Conversation",exact:true});await smallSheet.getByRole("heading",{name:"Issue #1"}).waitFor();
  assert.ok(await small.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),"no horizontal scroll on phones");
  await small.screenshot({animations:"disabled",path:shots("git-issue-390.png")});
  await phone.close();

  // A reader sees the shared repository, comments, and has no label, close or merge controls.
  const other=await browser.newContext({viewport:{width:1440,height:1000}}),page2=await signIn(other,reader);
  await page2.getByRole("navigation",{name:"Shared repositories"}).getByRole("button",{name:/Compiler/}).waitFor();
  const shared=page2.getByRole("main",{name:"Compiler",exact:true});
  await shared.getByText(/Shared by Grace Hopper/).waitFor();
  assert.equal(await shared.getByRole("button",{name:"Labels",exact:true}).count(),0);
  await shared.getByRole("tab",{name:/^Issues/}).click();await shared.getByRole("radio",{name:/Closed/}).click();
  await shared.getByRole("list",{name:"Issues"}).getByRole("button",{name:/Long identifiers/}).click();
  const readerSheet=page2.getByRole("complementary",{name:"Conversation",exact:true});
  await readerSheet.getByLabel("Add a comment",{exact:true}).fill("Thanks for the fix.");await readerSheet.getByRole("button",{name:"Comment",exact:true}).click();
  await readerSheet.getByText("Thanks for the fix.").waitFor();
  assert.equal(await readerSheet.getByRole("button",{name:"Reopen"}).count(),0);
  assert.equal(await readerSheet.getByRole("button",{name:"Labels"}).count(),0);
  await other.close();
  assert.deepEqual(errors,[]);
  console.info(`PASS Git issues and pull requests in a real browser. Screenshots: ${shots("")}`);
}finally{await browser.close();await rm(work,{recursive:true,force:true});}
