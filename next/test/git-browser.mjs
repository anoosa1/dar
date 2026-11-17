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
const admin=await login(base),username=`git-ui-${randomUUID().slice(0,6)}`,password="a long isolated test password";
assert.equal((await fetch(base+"/api/v1/auth/users",{method:"POST",headers:{...admin.headers,"content-type":"application/json"},body:JSON.stringify({username,displayName:"Git Tester",password})})).status,201);
const user=await login(base,username),db=await connect(user),folderId=randomUUID(),headers={...user.headers,"content-type":"application/json"};
await db.reducers.createFolder({id:folderId,parentId:"",name:"Site"});db.disconnect();
await upload(base,user,"README.md","# Site\n\nPersonal site and blog.\n\n## Develop\n\nRun the build.\n",folderId);await upload(base,user,"index.html","<h1>Hi</h1>\n",folderId);
assert.equal((await fetch(`${base}/api/v1/files/history/${folderId}`,{method:"POST",headers,body:JSON.stringify({action:"track"})})).status,200);
const overview=await(await fetch(`${base}/api/v1/files/history/${folderId}`,{headers:user.headers})).json();
assert.equal((await fetch(`${base}/api/v1/files/history/${folderId}/commits`,{method:"POST",headers,body:JSON.stringify({message:"Start the site",token:overview.status.token})})).status,201);
const secret=(await(await fetch(`${base}/api/v1/auth/app-passwords`,{method:"POST",headers,body:JSON.stringify({label:"Git",scope:"write"})})).json()).secret;

// A history with a merged branch, an open branch and a tag, pushed with Git.
const work=await mkdtemp(path.join(tmpdir(),"dar-git-ui-")),alias=`${base}/dav/git/${username}/Site.git`;
const env={...process.env,GIT_CONFIG_GLOBAL:"/dev/null",GIT_CONFIG_NOSYSTEM:"1",HOME:work,GIT_TERMINAL_PROMPT:"0",GIT_AUTHOR_NAME:"Ada Lovelace",GIT_AUTHOR_EMAIL:"ada@example.com",GIT_COMMITTER_NAME:"Ada Lovelace",GIT_COMMITTER_EMAIL:"ada@example.com"};
const git=(cwd,...args)=>{const result=spawnSync("git",["-c",`http.extraHeader=Authorization: Basic ${Buffer.from(`${username}:${secret}`).toString("base64")}`,...args],{cwd,env,encoding:"utf8"});assert.equal(result.status,0,`git ${args.join(" ")}\n${result.stderr}`);return result.stdout.trim();};
git(work,"clone","-q",alias,"site");const site=path.join(work,"site");
git(site,"checkout","-q","-b","feature/search");await writeFile(path.join(site,"search.js"),"export const search=()=>[];\n");git(site,"add","-A");git(site,"commit","-qm","Add search");
git(site,"checkout","-q","main");await writeFile(path.join(site,"index.html"),"<h1>Hello</h1>\n<p>Welcome</p>\n");git(site,"commit","-qam","Greet visitors");
git(site,"merge","-q","--no-ff","-m","Merge branch 'feature/search'","feature/search");git(site,"tag","-a","v1.0","-m","First version");
git(site,"checkout","-q","-b","feature/rss");await writeFile(path.join(site,"rss.xml"),"<rss/>\n");git(site,"add","-A");git(site,"commit","-qm","Add a feed");
git(site,"checkout","-q","-b","feature/old","main");await writeFile(path.join(site,"old.txt"),"old\n");git(site,"add","-A");git(site,"commit","-qm","Old work");
git(site,"push","-q","origin","main","feature/rss","feature/old","v1.0");

const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true}),errors=[];
try{
  const context=await browser.newContext({viewport:{width:1440,height:1000}}),page=await context.newPage();page.setDefaultTimeout(15000);
  await context.grantPermissions(["clipboard-read","clipboard-write"],{origin:base});
  page.on("pageerror",e=>errors.push(e.message));page.on("console",m=>{if(m.type()==="error"&&!/401.*Unauthorized/.test(m.text()))errors.push(m.text());});
  await page.goto(base+"/git");await page.getByLabel("Username",{exact:true}).fill(username);await page.getByLabel("Password",{exact:true}).fill(password);await page.getByRole("button",{name:"Sign in",exact:true}).click();

  // Overview: a year of activity, the repository card and recent commits.
  const main=page.getByRole("main",{name:"Overview",exact:true});
  await main.getByText("6 commits in the last year",{exact:true}).waitFor();
  await main.getByRole("region",{name:"Recent activity",exact:true}).getByText("Add a feed",{exact:true}).waitFor();
  await page.getByRole("navigation",{name:"Git views"}).getByRole("button",{name:"Repositories"}).waitFor();
  await page.screenshot({animations:"disabled",path:shots("git-overview-1440.png")});

  // A repository opens on Code: files with their last commit, then the README.
  await main.getByRole("region",{name:"Repositories",exact:true}).getByRole("button",{name:/^Site/}).click();
  const repo=page.getByRole("main",{name:"Site",exact:true});
  await repo.getByRole("heading",{name:"Site",level:1}).waitFor();
  await repo.getByRole("button",{name:"index.html",exact:true}).waitFor();
  await repo.getByRole("region",{name:"README.md",exact:true}).getByRole("heading",{name:"Develop"}).waitFor();
  assert.equal(new URL(page.url()).searchParams.get("repo"),folderId);

  // Clone: the alias and the permanent address; Escape closes and returns focus.
  await repo.getByRole("button",{name:"Clone",exact:true}).click();
  const clone=page.getByRole("dialog",{name:"Clone",exact:true});
  assert.equal(await clone.getByLabel("Clone with HTTPS").inputValue(),alias);
  assert.equal(await clone.getByLabel(/^Permanent address/).inputValue(),`${base}/dav/git/${folderId}.git`);
  await page.screenshot({animations:"disabled",path:shots("git-clone-1440.png")});
  await clone.getByRole("button",{name:"Copy clone address",exact:true}).click();await page.getByText("Copied clone address",{exact:true}).waitFor();
  await page.keyboard.press("Escape");await clone.waitFor({state:"detached"});
  assert.equal(await repo.getByRole("button",{name:"Clone",exact:true}).evaluate(el=>el===document.activeElement),true);

  // A file opens in the right sheet.
  await repo.getByRole("button",{name:"index.html",exact:true}).click();
  const file=page.getByRole("complementary",{name:"File",exact:true});
  await file.getByLabel("Contents of index.html").getByText("<p>Welcome</p>").waitFor();
  await page.keyboard.press("Escape");await file.waitFor({state:"detached"});

  // Commits: the graph; a merge commit opens with its parents and diff.
  await repo.getByRole("tab",{name:/^Commits/}).click();
  const commits=repo.getByRole("list",{name:"Commits",exact:true});
  await commits.getByRole("button",{name:/Merge branch 'feature\/search'/}).click();
  const commit=page.getByRole("complementary",{name:"Commit",exact:true});
  await commit.getByText("Parents",{exact:true}).waitFor();
  await commit.getByRole("region",{name:"Changes to search.js",exact:true}).getByText("export const search=()=>[];").waitFor();
  assert.equal(await commits.locator(".git-lanes").count()>=5,true);
  assert.match(new URL(page.url()).searchParams.get("commit"),/^[0-9a-f]{40}$/);
  await page.screenshot({animations:"disabled",path:shots("git-graph-1440.png")});
  await repo.getByRole("radio",{name:"List",exact:true}).check();
  await repo.getByRole("heading",{level:3}).first().waitFor();
  await page.keyboard.press("Escape");

  // Branches: divergence, then merging from the web (fast-forward, deleting the branch) and deleting another branch.
  await repo.getByRole("tab",{name:/^Branches/}).click();
  const branches=repo.getByRole("region",{name:"Branches",exact:true});
  await branches.getByRole("listitem").filter({hasText:"feature/rss"}).getByText("1 ahead",{exact:true}).waitFor();
  await branches.getByRole("listitem").filter({hasText:"feature/rss"}).getByRole("button",{name:"Compare",exact:true}).click();
  const compare=page.getByRole("complementary",{name:"Compare",exact:true});
  const mergeBox=compare.getByRole("region",{name:"Merge feature/rss into main",exact:true});
  await compare.getByText("rss.xml",{exact:true}).waitFor();
  assert.equal(await mergeBox.getByRole("radio",{name:"Fast-forward",exact:true}).isChecked(),true);
  assert.equal(await mergeBox.getByLabel("Delete feature/rss after merging").isChecked(),true);
  await page.screenshot({animations:"disabled",path:shots("git-branches-1440.png")});
  await mergeBox.getByRole("button",{name:"Fast-forward main",exact:true}).click();
  await page.getByText("main now points at feature/rss",{exact:true}).waitFor();await compare.waitFor({state:"detached"});
  await branches.getByRole("listitem").filter({hasText:"feature/rss"}).waitFor({state:"detached"});
  assert.ok((await(await fetch(`${base}/api/v1/git/repositories/${folderId}/tree`,{headers:user.headers})).json()).entries.some(entry=>entry.name==="rss.xml"),"main has the merged file");
  await branches.getByRole("button",{name:"Actions for feature/old",exact:true}).click();
  await page.getByRole("menuitem",{name:"Delete branch"}).click();
  const confirm=page.getByRole("dialog",{name:"Delete feature/old?",exact:true});
  await confirm.getByText(/1 commit is only on feature\/old/).waitFor();
  await confirm.getByRole("button",{name:"Delete branch",exact:true}).click();
  await page.getByText("Branch deleted",{exact:true}).waitFor();
  await branches.getByText(/Push a branch with Git/).waitFor();
  assert.doesNotMatch(git(site,"ls-remote",alias),/feature\/(rss|old)/);

  // Restore from a commit's details: the folder goes back and a restore commit is recorded.
  await repo.getByRole("tab",{name:/^Commits/}).click();
  await repo.getByRole("button",{name:/^Start the site/}).click();
  await page.getByRole("complementary",{name:"Commit",exact:true}).getByRole("button",{name:"Restore folder to this point",exact:true}).click();
  await page.getByRole("dialog",{name:"Restore this folder?",exact:true}).getByRole("button",{name:"Restore folder",exact:true}).click();
  await page.getByText("Folder restored to “Start the site”",{exact:true}).waitFor();
  await repo.getByRole("button",{name:/^Restore to “Start the site”/}).waitFor();
  assert.deepEqual((await(await fetch(`${base}/api/v1/git/repositories/${folderId}/tree`,{headers:user.headers})).json()).entries.map(entry=>entry.name),["index.html","README.md"]);
  await page.keyboard.press("Escape");

  // Tags: a ZIP of the tag.
  await repo.getByRole("tab",{name:/^Tags/}).click();
  const zip=repo.getByRole("link",{name:"ZIP",exact:true});
  const response=await page.request.get(base+await zip.getAttribute("href"));
  assert.equal(response.status(),200);assert.equal((await response.body()).subarray(0,2).toString(),"PK");

  // A new repository: a folder with a README and its first commit.
  await page.getByRole("button",{name:"New repository",exact:true}).click();
  const pane=page.getByRole("complementary",{name:"New repository",exact:true});
  await pane.getByLabel("Name",{exact:true}).fill("Notes repo");
  await pane.getByRole("button",{name:"Create",exact:true}).click();await pane.waitFor({state:"detached"});
  const created=page.getByRole("main",{name:"Notes repo",exact:true});
  await created.getByRole("region",{name:"README.md",exact:true}).getByRole("heading",{name:"Notes repo"}).waitFor();
  await page.getByRole("navigation",{name:"Repositories",exact:true}).getByRole("button",{name:"Notes repo"}).waitFor();

  // Themes and widths: nothing scrolls sideways.
  await page.getByRole("navigation",{name:"Repositories",exact:true}).getByRole("button",{name:"Site"}).click();
  await page.getByRole("main",{name:"Site",exact:true}).getByRole("tab",{name:/^Commits/}).click();
  await page.getByRole("main",{name:"Site",exact:true}).getByRole("radio",{name:"Graph",exact:true}).check();
  for(const theme of ["latte","mocha"])for(const width of [1440,390]){
    await page.evaluate(value=>{document.documentElement.dataset.theme=value;},theme);await page.setViewportSize({width,height:844});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,`${theme} ${width}`);
    await page.screenshot({animations:"disabled",path:shots(`git-commits-${theme}-${width}.png`)});
  }
  await page.goto(base+"/git");await page.getByText("7 commits in the last year",{exact:true}).waitFor();await page.evaluate(()=>{document.documentElement.dataset.theme="mocha";});
  for(const width of [1440,390]){await page.setViewportSize({width,height:844});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);await page.screenshot({animations:"disabled",path:shots(`git-overview-mocha-${width}.png`)});}
  assert.deepEqual(errors,[]);
  console.info("PASS Git browser checks.");
}finally{await browser.close();await rm(work,{recursive:true,force:true});}
