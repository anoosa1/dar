import assert from "node:assert/strict";
import {test} from "node:test";
import {randomUUID} from "node:crypto";
import {spawnSync} from "node:child_process";
import {mkdtemp,writeFile,rm,mkdir} from "node:fs/promises";
import {tmpdir} from "node:os";
import path from "node:path";
import {connect,login,upload} from "./integration-helpers.ts";
import type {HistoryBranch,HistoryOverview} from "./shared/history.ts";
import type {GitIssue,GitIssueList,GitLabel,GitPullChanges,GitRepository} from "./shared/git.ts";
const origin=process.env.DAR_INTEGRATION_ORIGIN;

/**
 * Issues and pull requests (docs/git.md#issues-and-pull-requests): numbering, labels, comments,
 * references and access for a shared reader; pull requests merged from the web and by a plain push,
 * closed by a deleted branch, "Closes #n", and a pull request from an Agent branch.
 */
test("git tracker: issues, pull requests, sharing and Agent branches",{skip:!origin,timeout:600_000},async()=>{
  const base=origin!,session=await login(base),db=await connect(session),headers={...session.headers,"content-type":"application/json"};
  const work=await mkdtemp(path.join(tmpdir(),"dar-git-tracker-"));
  const env={...process.env,GIT_CONFIG_GLOBAL:"/dev/null",GIT_CONFIG_NOSYSTEM:"1",HOME:work,GIT_TERMINAL_PROMPT:"0",GIT_AUTHOR_NAME:"Ada Lovelace",GIT_AUTHOR_EMAIL:"ada@example.com",GIT_COMMITTER_NAME:"Ada Lovelace",GIT_COMMITTER_EMAIL:"ada@example.com"};
  let secret="";
  const git=(cwd:string,...args:string[])=>{const result=spawnSync("git",["-c",`http.extraHeader=Authorization: Basic ${Buffer.from(`admin:${secret}`).toString("base64")}`,"-c","init.defaultBranch=main",...args],{cwd,env,encoding:"utf8"});assert.equal(result.status,0,`git ${args.join(" ")}\n${result.stderr}`);return result.stdout.trim();};
  try{
    const folderId=randomUUID(),repoApi=`${base}/api/v1/git/repositories/${folderId}`;
    const api=async<T>(route:string,init:{method?:string;body?:unknown;as?:typeof session}={},status=200)=>{
      const who=init.as??session,response=await fetch(repoApi+route,{method:init.method??"GET",headers:init.body===undefined?who.headers:{...who.headers,"content-type":"application/json"},...init.body===undefined?{}:{body:JSON.stringify(init.body)}});
      const text=await response.text();assert.equal(response.status,status,`${init.method??"GET"} ${route}: ${text}`);return (text?JSON.parse(text):undefined) as T;
    };
    await db.reducers.createFolder({id:folderId,parentId:"",name:`Tracker ${randomUUID().slice(0,6)}`});
    const readme=await upload(base,session,"README.md","# Tracker\n",folderId);
    assert.equal((await fetch(`${base}/api/v1/files/history/${folderId}`,{method:"POST",headers,body:JSON.stringify({action:"track"})})).status,200);
    let state=await (await fetch(`${base}/api/v1/files/history/${folderId}`,{headers:session.headers})).json() as HistoryOverview;
    assert.equal((await fetch(`${base}/api/v1/files/history/${folderId}/commits`,{method:"POST",headers,body:JSON.stringify({message:"Start",token:state.status!.token})})).status,201);
    secret=(await (await fetch(`${base}/api/v1/auth/app-passwords`,{method:"POST",headers,body:JSON.stringify({label:"Tracker",scope:"write"})})).json() as {secret:string}).secret;

    // Labels, then issues numbered from 1; a reference records itself on the item it names.
    const bug=await api<GitLabel>("/labels",{method:"POST",body:{name:"bug",color:"rose",description:"Something is wrong"}},201);
    await api("/labels",{method:"POST",body:{name:"Bug",color:"blue"}},409);
    await api("/labels",{method:"POST",body:{name:"style",color:"neon"}},400);
    const docs=await api<GitLabel>("/labels",{method:"POST",body:{name:"docs",color:"teal"}},201);
    const first=await api<{number:number}>("/issues",{method:"POST",body:{kind:"issue",title:"  Search   is slow ",body:"Typing lags.",labels:[bug.id]}},201);
    assert.equal(first.number,1);
    const second=await api<{number:number}>("/issues",{method:"POST",body:{kind:"issue",title:"Document search",body:"Follows #1 and #99."}},201);
    assert.equal(second.number,2);
    let issue=await api<GitIssue>("/issues/1");
    assert.equal(issue.title,"Search is slow");assert.deepEqual(issue.labels,[bug.id]);assert.equal(issue.state,"open");
    assert.deepEqual(issue.timeline.map(item=>item.kind==="event"?[item.event,item.number]:["comment"]),[["referenced",2]]);
    assert.deepEqual(issue.permissions,{edit:true,close:true,label:true,merge:false,comment:true});

    // Comments, edits, renames, labels and close/reopen, guarded by the revision.
    const comment=await api<{id:string}>("/issues/1/comments",{method:"POST",body:{body:"Seen on large folders."}},201);
    await api(`/comments/${comment.id}`,{method:"PATCH",body:{body:"Seen on large folders too."}});
    issue=await api<GitIssue>("/issues/1");
    await api("/issues/1",{method:"PATCH",body:{revision:issue.revision-1,title:"Stale"}},409);
    await api("/issues/1",{method:"PATCH",body:{revision:issue.revision,title:"Search is slow on large folders",labels:[bug.id,docs.id],state:"closed"}});
    issue=await api<GitIssue>("/issues/1");
    assert.equal(issue.state,"closed");assert.equal(issue.comments,1);
    assert.deepEqual(issue.timeline.map(item=>item.kind==="event"?item.event:"comment"),["referenced","comment","renamed","labeled","closed"]);
    const edited=issue.timeline.find(item=>item.kind==="comment");assert.ok(edited?.kind==="comment"&&edited.editedAt&&edited.body.endsWith("too."));
    await api("/issues/1",{method:"PATCH",body:{revision:issue.revision,state:"open"}});
    let list=await api<GitIssueList>("/issues?kind=issue&state=open");
    assert.deepEqual(list.items.map(item=>item.number),[2,1]);assert.equal(list.open,2);assert.equal(list.closed,0);
    assert.deepEqual((await api<GitIssueList>(`/issues?kind=issue&state=open&label=${docs.id}`)).items.map(item=>item.number),[1]);
    assert.deepEqual(list.labels.map(label=>label.name),["bug","docs"]);

    // A reader the folder is shared with sees the repository, opens issues and comments, nothing more.
    const username=`tracker-${randomUUID().slice(0,8)}`;
    assert.equal((await fetch(`${base}/api/v1/auth/users`,{method:"POST",headers,body:JSON.stringify({username,displayName:"Tracker Reader",password:"a long isolated test password"})})).status,201);
    const reader=await login(base,username);
    await db.reducers.browse({parentId:"",section:"files",query:"",cursor:""});
    const folder=[...db.db.myFiles.iter()].find(row=>row.id===folderId)!;
    await db.reducers.grantAccess({id:randomUUID(),fileId:folderId,username,role:"read",expectedRevision:folder.revision});
    const shared=(await (await fetch(`${base}/api/v1/git/repositories`,{headers:reader.headers})).json() as GitRepository[]).find(repo=>repo.id===folderId)!;
    assert.equal(shared.role,"read");assert.equal(shared.owner,"Test Administrator");assert.equal(shared.clone.alias,`${base}/dav/git/${folderId}.git`);assert.equal(shared.path,folder.name);
    assert.equal(shared.openIssues,2);
    const asked=await api<{number:number}>("/issues",{method:"POST",as:reader,body:{kind:"issue",title:"Question from a reader"}},201);
    assert.equal(asked.number,3);
    await api("/issues",{method:"POST",as:reader,body:{kind:"issue",title:"With a label",labels:[bug.id]}},403);
    await api("/labels",{method:"POST",as:reader,body:{name:"mine",color:"plum"}},403);
    const theirs=await api<{id:string}>("/issues/1/comments",{method:"POST",as:reader,body:{body:"Me too"}},201);
    await api(`/comments/${theirs.id}`,{method:"PATCH",body:{body:"Not yours"}},403);
    issue=await api<GitIssue>("/issues/1",{as:reader});
    assert.deepEqual(issue.permissions,{edit:false,close:false,label:false,merge:false,comment:true});
    await api("/issues/1",{method:"PATCH",as:reader,body:{revision:issue.revision,state:"closed"}},403);
    await api(`/comments/${theirs.id}`,{method:"DELETE"});

    // A pull request from a pushed branch; the web merge closes it and the issue it names.
    git(work,"clone","-q",`${base}/dav/git/${folderId}.git`,"clone");const clone=path.join(work,"clone");
    git(clone,"checkout","-q","-b","feature/fast-search");await writeFile(path.join(clone,"search.txt"),"fast\n");git(clone,"add","-A");git(clone,"commit","-qm","Index names");git(clone,"push","-q","origin","feature/fast-search");
    await api("/issues",{method:"POST",as:reader,body:{kind:"pull",title:"Not allowed",base:"main",head:"feature/fast-search"}},403);
    await api("/issues",{method:"POST",body:{kind:"pull",title:"Missing",base:"main",head:"feature/none"}},404);
    const pull=await api<{number:number}>("/issues",{method:"POST",body:{kind:"pull",title:"Faster search",body:"Closes #1.",base:"main",head:"feature/fast-search"}},201);
    assert.equal(pull.number,4);
    await api("/issues",{method:"POST",body:{kind:"pull",title:"Twice",base:"main",head:"feature/fast-search"}},409);
    assert.deepEqual((await api<GitIssueList>("/issues?kind=pull&state=open")).items.map(item=>[item.number,item.base,item.head,item.agent]),[[4,"main","feature/fast-search",false]]);
    // Another push to the branch is recorded on the pull request.
    await writeFile(path.join(clone,"search.txt"),"faster\n");git(clone,"commit","-qam","Tune index");git(clone,"push","-q","origin","feature/fast-search");
    let changes=await api<GitPullChanges>(`/issues/${pull.number}/changes`);
    assert.equal(changes.compare!.ahead.length,2);assert.equal(changes.compare!.fastForward,true);assert.deepEqual(changes.compare!.files.map(file=>file.path),["search.txt"]);
    await api("/merge",{method:"POST",body:{base:"main",head:"feature/fast-search",baseSha:changes.compare!.baseSha,headSha:changes.compare!.headSha,mode:"merge",deleteBranch:true}});
    let merged=await api<GitIssue>(`/issues/${pull.number}`);
    assert.equal(merged.state,"merged");assert.equal(merged.headSha,changes.compare!.headSha);assert.ok(merged.mergedSha);
    assert.deepEqual(merged.timeline.filter(item=>item.kind==="event").map(item=>item.kind==="event"&&item.event),["pushed","merged"]);
    issue=await api<GitIssue>("/issues/1");assert.equal(issue.state,"closed");
    assert.ok(issue.timeline.some(item=>item.kind==="event"&&item.event==="closed"&&item.number===pull.number));
    // Its commits stay readable after the branch is gone.
    changes=await api<GitPullChanges>(`/issues/${pull.number}/changes`);assert.equal(changes.compare!.ahead.length,2);

    // A plain push that merges a branch closes its pull request as merged too.
    git(clone,"checkout","-q","main");git(clone,"pull","-q","--ff-only");
    git(clone,"checkout","-q","-b","topic");await writeFile(path.join(clone,"topic.txt"),"topic\n");git(clone,"add","-A");git(clone,"commit","-qm","Topic");git(clone,"push","-q","origin","topic");
    const byPush=await api<{number:number}>("/issues",{method:"POST",body:{kind:"pull",title:"Topic",base:"main",head:"topic"}},201);
    git(clone,"checkout","-q","main");git(clone,"merge","-q","--no-ff","-m","Merge topic","topic");git(clone,"push","-q","origin","main");
    assert.equal((await api<GitIssue>(`/issues/${byPush.number}`)).state,"merged");

    // Deleting a branch closes its pull request; reopening needs the branch back.
    git(clone,"checkout","-q","-b","doomed");await writeFile(path.join(clone,"doomed.txt"),"x\n");git(clone,"add","-A");git(clone,"commit","-qm","Doomed");git(clone,"push","-q","origin","doomed");
    const doomed=await api<{number:number}>("/issues",{method:"POST",body:{kind:"pull",title:"Doomed",base:"main",head:"doomed"}},201);
    git(clone,"push","-q","origin","--delete","doomed");
    let closed=await api<GitIssue>(`/issues/${doomed.number}`);
    assert.equal(closed.state,"closed");assert.ok(closed.timeline.some(item=>item.kind==="event"&&item.event==="branchDeleted"));
    await api(`/issues/${doomed.number}`,{method:"PATCH",body:{revision:closed.revision,state:"open"}},409);
    git(clone,"push","-q","origin","doomed");
    await api(`/issues/${doomed.number}`,{method:"PATCH",body:{revision:closed.revision,state:"open"}});
    closed=await api<GitIssue>(`/issues/${doomed.number}`);
    await api(`/issues/${doomed.number}`,{method:"PATCH",body:{revision:closed.revision,state:"closed"}});
    changes=await api<GitPullChanges>(`/issues/${doomed.number}/changes`);assert.deepEqual(changes.compare!.ahead.map(commit=>commit.subject),["Doomed"]);

    // A pull request from an Agent branch shows its changes and is merged by Agent's merge.
    const projectId=randomUUID(),threadId=randomUUID(),agent=(rest:string,value?:unknown)=>fetch(`${base}/api/v1/agent${rest}`,{method:value===undefined?"GET":"POST",headers,...value===undefined?{}:{body:JSON.stringify(value)}});
    assert.equal((await agent("/projects",{id:projectId,name:"Tracker project",sourceType:"files",sourceRef:folderId})).status,201);
    assert.equal((await agent("/threads",{id:threadId,projectId,title:"Tidy"})).status,201);
    assert.equal((await agent(`/threads/${threadId}/branch`)).status,200);
    await db.reducers.browse({parentId:folderId,section:"files",query:"",cursor:""});
    const readmeRow=[...db.db.myFiles.iter()].find(row=>row.id===readme.fileId)!,me=[...db.db.me.iter()][0].id,thread=path.join(process.env.DAR_TEST_ROOT!,"agent-workspaces",me,threadId);
    await mkdir(path.join(thread,"work"),{recursive:true});await writeFile(path.join(thread,"work","README.md"),"# Tidy\n");
    await writeFile(path.join(thread,"baseline.json"),JSON.stringify({source:{project:{id:projectId,ownerId:me,name:"Tracker project",sourceType:"files",sourceRef:folderId,createdAt:0},files:[{id:readme.fileId,path:"README.md",revision:readmeRow.revision,size:10,mediaType:"text/plain"}]},
      files:[{path:"README.md",content:Buffer.from("# Tracker\n").toString("base64")}]}));
    const staged=await agent(`/threads/${threadId}/branch`,{});assert.equal(staged.status,201,await staged.clone().text());
    const branch=(await staged.json() as {branch:HistoryBranch}).branch;
    await api("/issues",{method:"POST",body:{kind:"pull",title:"Into a branch",base:"topic",head:branch.name}},400);
    const fromAgent=await api<{number:number}>("/issues",{method:"POST",body:{kind:"pull",title:"Tidy the README",base:"main",head:branch.name}},201);
    changes=await api<GitPullChanges>(`/issues/${fromAgent.number}/changes`);
    assert.deepEqual(changes.agent!.changes.map(change=>[change.kind,change.path]),[["modified","README.md"]]);assert.equal(changes.agent!.threadId,threadId);
    assert.equal((await fetch(`${base}/api/v1/files/history/${folderId}/branches/merge`,{method:"POST",headers,body:JSON.stringify({name:branch.name,revision:changes.agent!.revision})})).status,200);
    assert.equal((await api<GitIssue>(`/issues/${fromAgent.number}`)).state,"merged");

    // Counts follow; the tracker goes with the folder's history.
    const repo=await api<GitRepository>("");assert.equal(repo.openIssues,2);assert.equal(repo.openPulls,0);
    state=await (await fetch(`${base}/api/v1/files/history/${folderId}`,{headers:session.headers})).json() as HistoryOverview;
    assert.equal((await fetch(`${base}/api/v1/files/history/${folderId}`,{method:"POST",headers,body:JSON.stringify({action:"untrack",revision:state.repo!.revision})})).status,200);
    assert.equal((await fetch(`${base}/api/v1/files/history/${folderId}`,{method:"POST",headers,body:JSON.stringify({action:"track"})})).status,200);
    list=await api<GitIssueList>("/issues?kind=issue&state=open");assert.equal(list.items.length,0);assert.equal(list.labels.length,0);
  }finally{db.disconnect();await rm(work,{recursive:true,force:true});}
});
