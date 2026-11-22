import assert from "node:assert/strict";
import {test} from "node:test";
import {randomUUID} from "node:crypto";
import {spawnSync} from "node:child_process";
import {mkdtemp,writeFile,rm} from "node:fs/promises";
import {tmpdir} from "node:os";
import path from "node:path";
import {connect,login,upload} from "./integration-helpers.ts";
import type {HistoryOverview} from "./shared/history.ts";
import type {GitActivity,GitBranches,GitCommitDetail,GitCommitPage,GitCompare,GitRepository,GitTag,GitTree} from "./shared/git.ts";
const origin=process.env.DAR_INTEGRATION_ORIGIN;

test("git app: repositories, trees, commits, branches, tags, compare, archives and aliases",{skip:!origin,timeout:240_000},async()=>{
  const base=origin!,session=await login(base),db=await connect(session),headers={...session.headers,"content-type":"application/json"};
  const work=await mkdtemp(path.join(tmpdir(),"dar-git-app-"));
  const get=async<T>(route:string,status=200)=>{const response=await fetch(`${base}/api/v1/git${route}`,{headers:session.headers});assert.equal(response.status,status,`${route}: ${await response.clone().text()}`);return (status===200?await response.json():undefined) as T;};
  const history=(folderId:string,rest="")=>`${base}/api/v1/files/history/${folderId}${rest}`;
  const env={...process.env,GIT_CONFIG_GLOBAL:"/dev/null",GIT_CONFIG_NOSYSTEM:"1",HOME:work,GIT_TERMINAL_PROMPT:"0",GIT_AUTHOR_NAME:"Ada Lovelace",GIT_AUTHOR_EMAIL:"ada@example.com",GIT_COMMITTER_NAME:"Ada Lovelace",GIT_COMMITTER_EMAIL:"ada@example.com"};
  let secret="";
  const run=(cwd:string,args:string[],ok=true,user="admin")=>{
    const result=spawnSync("git",["-c",`http.extraHeader=Authorization: Basic ${Buffer.from(`${user}:${secret}`).toString("base64")}`,"-c","init.defaultBranch=main",...args],{cwd,env,encoding:"utf8"});
    if(ok)assert.equal(result.status,0,`git ${args.join(" ")}\n${result.stderr}`);else assert.notEqual(result.status,0,`git ${args.join(" ")} should fail`);
    return {out:result.stdout.trim(),err:result.stderr};
  };
  try{
    const folderId=randomUUID(),projectsId=randomUUID(),name=`Site ${randomUUID().slice(0,6)}`,projects={name:`Projects ${randomUUID().slice(0,6)}`};
    await db.reducers.createFolder({id:projectsId,parentId:"",name:projects.name});
    await db.reducers.createFolder({id:folderId,parentId:projectsId,name});
    await upload(base,session,"README.md","# Site\n\n[![badge](x.svg)](y) Personal site and **blog**.\n\nMore text.\n",folderId);
    await upload(base,session,"index.html","<h1>Hi</h1>\n",folderId);
    assert.equal((await fetch(history(folderId),{method:"POST",headers,body:JSON.stringify({action:"track"})})).status,200);
    const state=await (await fetch(history(folderId),{headers:session.headers})).json() as HistoryOverview;
    assert.equal((await fetch(history(folderId,"/commits"),{method:"POST",headers,body:JSON.stringify({message:"Start the site",token:state.status!.token})})).status,201);
    secret=(await (await fetch(`${base}/api/v1/auth/app-passwords`,{method:"POST",headers,body:JSON.stringify({label:"Git app",scope:"write"})})).json() as {secret:string}).secret;

    // The list: description from the README, counts and both clone addresses.
    let repo=(await get<GitRepository[]>("/repositories")).find(item=>item.id===folderId)!;
    assert.ok(repo,"the tracked folder is a repository");
    assert.equal(repo.path,`${projects.name}/${name}`);
    assert.equal(repo.description,"Personal site and blog.");
    assert.deepEqual([repo.commits,repo.branches,repo.tags,repo.dirty],[1,1,0,0]);
    assert.equal(repo.clone.id,`${base}/dav/git/${folderId}.git`);
    assert.equal(repo.clone.alias,`${base}/dav/git/admin/${encodeURIComponent(projects.name)}/${encodeURIComponent(name)}.git`);
    assert.ok(repo.size>0);assert.equal(repo.weeks.reduce((a,b)=>a+b,0),1);

    // Clone by alias; the alias belongs to its user only.
    run(work,["clone","-q",repo.clone.alias,"site"]);const site=path.join(work,"site");
    run(work,["clone","-q",repo.clone.alias,"other"],false,"someone");
    run(work,["clone","-q",`${base}/dav/git/admin/${encodeURIComponent(projects.name)}/missing.git`,"missing"],false);

    // Work on main, a branch and a tag.
    await writeFile(path.join(site,"index.html"),"<h1>Hello</h1>\n<p>Welcome</p>\n");run(site,["commit","-qam","Greet visitors"]);
    run(site,["tag","-a","v1","-m","First version"]);
    run(site,["checkout","-q","-b","feature/rss"]);await writeFile(path.join(site,"rss.xml"),"<rss/>\n");run(site,["add","-A"]);run(site,["commit","-qm","Add a feed"]);
    run(site,["push","-q","origin","main","feature/rss","v1"]);
    run(site,["checkout","-q","main"]);await writeFile(path.join(site,"about.html"),"about\n");run(site,["add","-A"]);run(site,["commit","-qm","Add about"]);run(site,["push","-q","origin","main"]);
    const head=run(site,["rev-parse","main"]).out,greet=run(site,["rev-parse","main~1"]).out;

    repo=await get<GitRepository>(`/repositories/${folderId}`);
    assert.deepEqual([repo.commits,repo.branches,repo.tags],[3,2,1]);

    // Tree: entries with their newest commit, and the README.
    const tree=await get<GitTree>(`/repositories/${folderId}/tree`);
    assert.equal(tree.sha,head);
    assert.deepEqual(tree.entries.map(entry=>[entry.name,entry.last?.subject]),[["about.html","Add about"],["index.html","Greet visitors"],["README.md","Start the site"]]);
    assert.match(tree.readme!.text,/Personal site/);
    const branchTree=await get<GitTree>(`/repositories/${folderId}/tree?ref=feature%2Frss`);
    assert.ok(branchTree.entries.some(entry=>entry.name==="rss.xml"));
    await get(`/repositories/${folderId}/tree?ref=nope`,404);await get(`/repositories/${folderId}/tree?path=..%2Fx`,400);

    // Commits: where each came from, refs, line counts, and filters.
    const page=await get<GitCommitPage>(`/repositories/${folderId}/commits?ref=main`);
    assert.deepEqual(page.commits.map(commit=>[commit.subject,commit.source]),[["Add about","git"],["Greet visitors","git"],["Start the site","dar"]]);
    assert.deepEqual(page.commits[0].refs,[{name:"main",kind:"branch"}]);assert.deepEqual(page.commits[1].refs,[{name:"v1",kind:"tag"}]);
    assert.equal(page.commits[0].added,1);assert.equal(page.total,3);assert.equal(page.next,null);
    assert.deepEqual(page.authors,["Ada Lovelace","Test Administrator"]);
    assert.equal((await get<GitCommitPage>(`/repositories/${folderId}/commits`)).total,4,"all branches");
    assert.equal((await get<GitCommitPage>(`/repositories/${folderId}/commits?source=dar`)).commits.length,1);
    assert.equal((await get<GitCommitPage>(`/repositories/${folderId}/commits?author=Ada%20Lovelace&ref=main`)).total,2);
    assert.equal((await get<GitCommitPage>(`/repositories/${folderId}/commits?ref=main&limit=2`)).next,2);

    const detail=await get<GitCommitDetail>(`/repositories/${folderId}/commits/${greet}`);
    assert.equal(detail.files.length,1);assert.equal(detail.files[0].kind,"modified");assert.deepEqual([detail.files[0].added,detail.files[0].removed],[2,1]);
    assert.match(detail.files[0].patch!,/^@@ -1 \+1,2 @@\n-<h1>Hi<\/h1>\n\+<h1>Hello<\/h1>\n\+<p>Welcome<\/p>$/);
    await get(`/repositories/${folderId}/commits/${"0".repeat(40)}`,404);

    // Branches with divergence, tags, compare and downloads.
    const branches=await get<GitBranches>(`/repositories/${folderId}/branches`);
    assert.equal(branches.main?.sha,head);
    assert.deepEqual(branches.branches.map(branch=>[branch.name,branch.ahead,branch.behind]),[["feature/rss",1,1]]);
    const tags=await get<GitTag[]>(`/repositories/${folderId}/tags`);
    assert.deepEqual(tags.map(tag=>[tag.name,tag.target,tag.message,tag.annotated]),[["v1",greet,"First version",true]]);
    const compared=await get<GitCompare>(`/repositories/${folderId}/compare?base=main&head=feature%2Frss`);
    assert.deepEqual([compared.ahead.map(commit=>commit.subject),compared.behind,compared.fastForward,compared.files.map(file=>file.path)],[["Add a feed"],1,false,["rss.xml"]]);
    const zip=await fetch(`${base}/api/v1/git/repositories/${folderId}/archive?ref=v1`,{headers:session.headers});
    assert.equal(zip.status,200);assert.match(zip.headers.get("content-disposition")??"",/\.zip$/);
    assert.equal(Buffer.from(await zip.arrayBuffer()).subarray(0,2).toString(),"PK");
    const blob=await fetch(`${base}/api/v1/git/repositories/${folderId}/blob?ref=feature%2Frss&path=rss.xml&view`,{headers:session.headers});
    assert.equal(await blob.text(),"<rss/>\n");assert.match(blob.headers.get("content-security-policy")??"",/sandbox/);

    // Activity counts every commit this year.
    const activity=await get<GitActivity>("/activity");
    assert.ok(activity.events.some(event=>event.repoId===folderId&&event.subject==="Add about"));
    assert.ok(activity.times.length>=4);

    // Deleting: a stale ID is refused, main never goes, and a branch goes like a push --delete.
    const remove=(body:object)=>fetch(`${base}/api/v1/git/repositories/${folderId}/refs`,{method:"DELETE",headers,body:JSON.stringify(body)});
    assert.equal((await remove({name:"refs/heads/feature/rss",old:head})).status,409);
    assert.equal((await remove({name:"refs/heads/main",old:head})).status,403);
    assert.equal((await remove({name:"refs/heads/feature/rss",old:branches.branches[0].sha})).status,200);
    assert.deepEqual((await get<GitBranches>(`/repositories/${folderId}/branches`)).branches,[]);
    assert.doesNotMatch(run(site,["ls-remote",repo.clone.alias]).out,/feature\/rss/);
  }finally{db.disconnect();await rm(work,{recursive:true,force:true});}
});
