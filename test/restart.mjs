import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import http from "node:http";
import net from "node:net";
import { mkdtemp, readFile, stat, readdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { login, upload, connect } from "./integration-helpers.ts";
import { database } from "./support/database.ts";
import { deliverReminders } from "./support/operator.ts";
import {identityAdmin,authorize,exchange} from "./identity-helpers.ts";
import * as oidc from "openid-client";
import {manageApps} from "./support/operator.ts";
import {randomUUID} from "node:crypto";
import {epub,png} from "./books-fixtures.ts";

if (!process.env.DAR_SPACETIME_BIN) throw new Error("DAR_SPACETIME_BIN is required.");
async function freePort() {
  const server = net.createServer(); await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address(); await new Promise(resolve => server.close(resolve)); return port;
}
const root = await mkdtemp(path.join(tmpdir(), "dar-restart-proof-")), port = await freePort(), databasePort = await freePort();
const origin = `http://127.0.0.1:${port}`, repo = path.resolve(import.meta.dirname, "..");
await manageApps(root,"apps-install",path.resolve(repo,"apps/prayer-times/dist"));
const environment = { ...process.env, DAR_ROOT: root, DAR_PORT: String(port), DAR_DATABASE_PORT: String(databasePort), DAR_DATABASE_NAME: "dar-restart" };
let child;
async function until(probe) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    try { if (await probe()) return; } catch {}
    if (child?.exitCode !== null && child?.exitCode !== undefined) throw new Error("Fixture stopped before readiness.");
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error("Fixture readiness timed out.");
}
async function start() {
  // rust/runtime/dev.sh starts SpacetimeDB, publishes the module (an in-place upgrade on later starts) and runs the runtime.
  child = spawn("bash", [path.resolve(repo, "rust/runtime/dev.sh")], { cwd: repo, env: { ...environment, DAR_PREBUILT: "1", DAR_ORIGIN: origin }, stdio: ["ignore", "pipe", "pipe"] });
  let output = ""; child.stdout.on("data", chunk => { output = (output + chunk).slice(-8000); }); child.stderr.on("data", chunk => { output = (output + chunk).slice(-8000); });
  try { await until(async () => (await fetch(`${origin}/api/v1/auth/status`, { signal: AbortSignal.timeout(500) })).ok); }
  catch (error) { throw new Error(`${error.message}\n${output}`); }
}
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  // This fixture has no systemd cgroup: own the runtime as well as its launcher.
  const {runtime}=JSON.parse(await readFile(path.join(root,"service-pids.json"),"utf8"));
  function signalRuntime(signal){try{process.kill(runtime,signal);}catch(error){if(error.code!=="ESRCH")throw error;}}
  signalRuntime("SIGTERM");
  await new Promise(resolve => {
    const runtimeTimer=setTimeout(()=>signalRuntime("SIGKILL"),5000);
    const timer = setTimeout(() => child.kill("SIGKILL"), 10_000);
    child.once("close", () => { clearTimeout(timer);clearTimeout(runtimeTimer); resolve(); }); child.kill("SIGTERM");
  });
  // The launcher can exit before its runtime's inherited pipes/lock finish closing.
  const deadline=Date.now()+5000;
  while(true){
    try{const record=JSON.parse(await readFile(path.join(root,"runtime.lock"),"utf8"));process.kill(record.pid,0);}
    catch(error){if(["ENOENT","ESRCH"].includes(error.code))break;throw error;}
    if(Date.now()>=deadline)throw Error("Fixture runtime still owns its root after shutdown.");
    await new Promise(resolve=>setTimeout(resolve,25));
  }
}
try {
  await start();
  const setup = await fetch(`${origin}/api/v1/auth/setup`, { method: "POST", headers: { origin, "content-type": "application/json" }, body: JSON.stringify({ username: "admin", displayName: "Restart proof", password: "a long isolated test password", bootstrapToken: await readFile(path.join(root, "secrets/bootstrap-token"), "utf8") }) });
  assert.equal(setup.status, 200);
  const session = await login(origin);
  async function setApp(id,enabled){const settings=await(await fetch(origin+"/api/v1/appSettings",{headers:session.headers})).json();const r=await fetch(origin+"/api/v1/admin/installedApps",{method:"PUT",headers:{...session.headers,"content-type":"application/json","if-match":`"apps:${settings.revision}"`},body:JSON.stringify({id,enabled,revision:settings.revision})});assert.equal(r.status,200,await r.clone().text());}
  const appEndpoint=origin+"/api/v1/apps/org.dar.prayer-times/userData/restartProof";
  assert.equal((await fetch(appEndpoint,{method:"PUT",headers:{...session.headers,"content-type":"application/json","if-match":'"0"'},body:JSON.stringify({schemaVersion:1,value:{location:"private restart proof"}})})).status,200);
  const identity=await identityAdmin(origin),client=await identity.client(),credential=await identity.credential();
  const activeTokens=await exchange(client,await authorize(origin,identity.session,client)),pendingCode=await authorize(origin,identity.session,client);
  const providerKeys=await (await fetch(`${origin}/oidc/jwks`)).json();
  const directoryResponse=await fetch(`${origin}/scim/v2/Users`,{method:"POST",headers:{authorization:`Bearer ${credential.secret}`,"content-type":"application/scim+json"},body:JSON.stringify({userName:"durable-directory",displayName:"Durable directory"})});assert.equal(directoryResponse.status,201);const directoryUser=await directoryResponse.json();
  const calendarResponse = await fetch(`${origin}/api/v1/calendar/calendars`, { method: "POST", headers: { ...session.headers, "content-type": "application/json" }, body: JSON.stringify({ name: "Durable calendar", color: "#2563eb" }) });
  assert.equal(calendarResponse.status,201); const calendar = await calendarResponse.json();
  const reminderStart = new Date(Date.now()+3605000).toISOString().slice(0,19), reminderEnd = new Date(Date.now()+7205000).toISOString().slice(0,19);
  const eventResponse = await fetch(`${origin}/api/v1/calendar/calendars/${calendar.id}/items`, { method: "POST", headers: { ...session.headers, "content-type": "application/json" }, body: JSON.stringify({ data: { title: "Reminder after crash", start: reminderStart, end: reminderEnd, timezone: "UTC", reminderMinutes: 60 } }) });
  assert.equal(eventResponse.status,201); const calendarEvent = await eventResponse.json();
  const workspaceHeaders={...session.headers,"content-type":"application/json"};
  async function createWorkspace(endpoint,value){const response=await fetch(origin+"/api/v1/"+endpoint,{method:"POST",headers:workspaceHeaders,body:JSON.stringify(value)});assert.equal(response.status,201,await response.clone().text());return {row:await response.json(),etag:response.headers.get("etag")};}
  const notebook=(await createWorkspace("notes/notebooks",{name:"Durable notes"})).row;
  const agentProject=(await createWorkspace("agent/projects",{id:randomUUID(),name:"Durable Agent project",sourceType:"git",sourceRef:"https://example.test/you/repo.git"})).row;
  const agentThread=(await createWorkspace("agent/threads",{id:randomUUID(),projectId:agentProject.id,title:"Durable conversation"})).row;
  const agentMessage={id:randomUUID(),content:"Keep this task through a runtime crash."};
  assert.equal((await fetch(`${origin}/api/v1/agent/threads/${agentThread.id}/messages`,{method:"POST",headers:{...workspaceHeaders,"if-match":'"1"'},body:JSON.stringify(agentMessage)})).status,201);
  const exactMarkdown="# Durable source\r\n\r\nUnchanged after a crash.\n";
  const note=await createWorkspace("notes/items",{collectionId:notebook.id,title:"Durable note",content:exactMarkdown,tags:["restart"],favorite:true});
  const trashed=await createWorkspace("notes/items",{collectionId:notebook.id,title:"Durable trash",content:exactMarkdown});
  assert.equal((await fetch(`${origin}/api/v1/notes/items/${trashed.row.id}`,{method:"DELETE",headers:{...workspaceHeaders,"if-match":trashed.etag}})).status,200);
  const task=(await createWorkspace("tasks/items",{collectionId:calendar.id,data:{title:"Task reminder after crash",start:reminderStart,due:reminderEnd,timezone:"UTC",reminderMinutes:60,recurrence:"FREQ=DAILY;COUNT=2"}})).row;
  const owner = await database(`http://127.0.0.1:${databasePort}`, "dar-restart", path.join(root,"secrets/database-token"));
  await deliverReminders(owner);
  const durable = await upload(origin, session, "Durable.txt", "Committed content survives process failure.");
  // A sync client's app password and a file it wrote over WebDAV.
  const appPassword=await (await fetch(`${origin}/api/v1/auth/app-passwords`,{method:"POST",headers:{...session.headers,"content-type":"application/json"},body:JSON.stringify({label:"Sync client",scope:"write"})})).json();
  const davHeaders={authorization:`Basic ${Buffer.from(`admin:${appPassword.secret}`).toString("base64")}`},davFile=`${appPassword.davUrl}Synced.txt`;
  const davPut=await fetch(davFile,{method:"PUT",headers:davHeaders,body:"Written by a sync client."});assert.equal(davPut.status,201,await davPut.clone().text());const davEtag=davPut.headers.get("etag");
  // Apps that keep their own state: a tracked folder and its Git repository, a book's reading
  // position and a photo's description.
  const repoFolder=randomUUID();{const live=await connect(session);try{await live.reducers.createFolder({id:repoFolder,parentId:"",name:"Durable repository"});}finally{live.disconnect();}}
  await upload(origin,session,"README.md","# Durable repository\n",repoFolder);
  const historyUrl=`${origin}/api/v1/files/history/${repoFolder}`;
  assert.equal((await fetch(historyUrl,{method:"POST",headers:workspaceHeaders,body:JSON.stringify({action:"track"})})).status,200);
  const tracked=await (await fetch(historyUrl,{headers:session.headers})).json();
  assert.equal((await fetch(historyUrl+"/commits",{method:"POST",headers:workspaceHeaders,body:JSON.stringify({message:"Durable start",token:tracked.status.token})})).status,201);
  const book=await upload(origin,session,"durable-voyage.epub",epub({title:"Durable Voyage",author:"Restart Fixture"}),"",undefined,undefined,"application/epub+zip");
  const bookState=await fetch(`${origin}/api/v1/books/${book.fileId}/state`,{method:"PUT",headers:workspaceHeaders,body:JSON.stringify({revision:0,status:"reading",position:{spine:1,fraction:0.5},progress:0.6,device:"Restart proof"})});
  assert.equal(bookState.status,200,await bookState.clone().text());
  const photoTarget=await (await fetch(`${origin}/api/v1/photos/upload-target`,{method:"POST",headers:workspaceHeaders,body:JSON.stringify({year:2024,month:4})})).json();
  const photo=await upload(origin,session,"durable.png",png(32,20,[20,90,160]),photoTarget.parentId,undefined,undefined,"image/png");
  assert.equal((await fetch(`${origin}/api/v1/photos/assets/${photo.fileId}/index`,{method:"POST",headers:workspaceHeaders,body:"{}"})).status,200);
  const photoDetail=await (await fetch(`${origin}/api/v1/photos/assets/${photo.fileId}`,{headers:session.headers})).json();
  const described=await fetch(`${origin}/api/v1/photos/assets/${photo.fileId}/description`,{method:"PUT",headers:workspaceHeaders,body:JSON.stringify({description:"Durable photo",revision:photoDetail.revision})});
  assert.equal(described.status,200,await described.clone().text());
  const reservationResponse = await fetch(`${origin}/api/v1/files/uploads`, { method: "POST", headers: { ...session.headers, "content-type": "application/json" }, body: JSON.stringify({ name: "Interrupted.txt", parentId: "", size: 4096, mediaType: "text/plain" }) });
  assert.equal(reservationResponse.status, 201); const reservation = await reservationResponse.json();
  const request = http.request(`${origin}/api/v1/files/uploads/${reservation.id}`, { method: "PUT", headers: { ...session.headers, "content-length": 4096 } });
  request.on("error", () => {}); request.write(Buffer.alloc(1024, "x"));
  await until(async () => (await stat(path.join(root, "uploads", `${reservation.id}.tmp`))).size >= 1024);
  const pids = JSON.parse(await readFile(path.join(root, "service-pids.json"), "utf8"));
  const processInfo = await readFile(`/proc/${pids.runtime}/stat`, "utf8");
  assert.equal(Number(processInfo.slice(processInfo.lastIndexOf(")") + 2).split(" ")[1]), child.pid);
  const stopped = new Promise(resolve => child.once("exit", resolve));
  await setApp("notes",false);await setApp("org.dar.prayer-times",false);
  process.kill(pids.runtime, "SIGKILL"); await stopped; request.destroy();
  await start();
  assert.equal((await fetch(origin+"/api/v1/notes/items",{headers:session.headers})).status,404);assert.equal((await fetch(appEndpoint,{headers:session.headers})).status,404);
  await setApp("notes",true);await setApp("org.dar.prayer-times",true);
  assert.deepEqual(await (await fetch(`${origin}/oidc/jwks`)).json(),providerKeys);
  const redeemed=await exchange(client,pendingCode);assert.ok(redeemed.id_token);
  // The provider session made before the restart (and, on the Rust stack, by the TypeScript stack) still signs the browser in silently.
  const silent=await authorize(origin,identity.session,client,{prompt:"none",jar:pendingCode.jar});assert.equal(silent.url.searchParams.get("error"),null,silent.url.href);assert.ok((await exchange(client,silent)).id_token);
assert.ok((await oidc.refreshTokenGrant(client.config,activeTokens.refresh_token)).access_token);
  const restoredDirectory=await fetch(`${origin}/scim/v2/Users/${directoryUser.id}`,{headers:{authorization:`Bearer ${credential.secret}`}});assert.equal(restoredDirectory.status,200);assert.equal((await restoredDirectory.json()).id,directoryUser.id);
  assert.deepEqual(await readdir(path.join(root, "uploads")), []);
  assert.equal(await (await fetch(`${origin}/api/v1/files/${durable.fileId}/content`, { headers: session.headers })).text(), "Committed content survives process failure.");
  const davGet=await fetch(davFile,{headers:davHeaders});assert.equal(davGet.status,200);assert.equal(await davGet.text(),"Written by a sync client.");assert.equal(davGet.headers.get("etag"),davEtag);
  const historyAfter=await (await fetch(historyUrl,{headers:session.headers})).text();assert.match(historyAfter,/Durable start/);
  const repoAlias=(await (await fetch(`${origin}/api/v1/git/repositories/${repoFolder}`,{headers:session.headers})).json()).clone.alias;
  const cloneRoot=await mkdtemp(path.join(tmpdir(),"dar-restart-clone-"));
  const cloned=spawnSync("git",["-c",`http.extraHeader=Authorization: Basic ${Buffer.from(`admin:${appPassword.secret}`).toString("base64")}`,"clone","-q",repoAlias,"repo"],{cwd:cloneRoot,encoding:"utf8",env:{...process.env,GIT_CONFIG_GLOBAL:"/dev/null",GIT_CONFIG_NOSYSTEM:"1",HOME:cloneRoot,GIT_TERMINAL_PROMPT:"0"}});
  assert.equal(cloned.status,0,cloned.stderr);assert.equal(await readFile(path.join(cloneRoot,"repo","README.md"),"utf8"),"# Durable repository\n");
  const bookAfter=(await (await fetch(`${origin}/api/v1/books/${book.fileId}`,{headers:session.headers})).json()).state;assert.deepEqual(bookAfter.position,{spine:1,fraction:0.5});assert.equal(bookAfter.status,"reading");
  const photoAfter=await (await fetch(`${origin}/api/v1/photos/assets/${photo.fileId}`,{headers:session.headers})).json();assert.equal(photoAfter.description,"Durable photo");
  assert.equal((await fetch(`${origin}/api/v1/photos/assets/${photo.fileId}/thumbnail`,{headers:session.headers})).status,200);
  assert.equal((await fetch(appPassword.davUrl,{method:"PROPFIND",headers:{...davHeaders,depth:"1"}})).status,207);
  const retry = await fetch(`${origin}/api/v1/files/uploads/${reservation.id}`, { method: "PUT", headers: session.headers, body: Buffer.alloc(4096, "r") });
  assert.equal(retry.status, 200, await retry.clone().text());
  const repeated = await fetch(`${origin}/api/v1/files/uploads/${reservation.id}`, { method: "PUT", headers: session.headers, body: Buffer.alloc(4096, "s") });
  assert.equal(repeated.status, 200);
  assert.equal(await (await fetch(`${origin}/api/v1/files/${reservation.fileId}/content`, { headers: session.headers })).text(), "r".repeat(4096));
  const db = await connect(session);
  try { assert.equal([...db.db.myFiles.iter()].some(entry => entry.id === durable.fileId), true); } finally { db.disconnect(); }
  await new Promise(resolve => setTimeout(resolve, Math.max(0,Date.parse(reminderStart+"Z")-3600000-Date.now()+25)));
  await deliverReminders(owner); await deliverReminders(owner);
  const inbox = await (await fetch(`${origin}/api/v1/pim/reminders`, { headers: session.headers })).json();
  assert.equal(inbox.items.filter(row => row.resourceId === calendarEvent.id).length,1);
  assert.equal(inbox.items.filter(row=>row.resourceId===task.id).length,1);
  const recoveredNote=await (await fetch(`${origin}/api/v1/notes/items/${note.row.id}`,{headers:session.headers})).json();assert.equal(recoveredNote.content,exactMarkdown);assert.equal(recoveredNote.uid,note.row.uid);assert.equal(recoveredNote.data.favorite,true);
  const recoveredTrash=await (await fetch(`${origin}/api/v1/notes/trash`,{headers:session.headers})).json();assert.ok(recoveredTrash.items.some(row=>row.id===trashed.row.id));
  const recoveredTask=await (await fetch(`${origin}/api/v1/tasks/items/${task.id}`,{headers:session.headers})).json();assert.equal(recoveredTask.content,task.content);assert.equal(recoveredTask.uid,task.uid);
  const recoveredAgent=await(await fetch(`${origin}/api/v1/agent/threads/${agentThread.id}/messages`,{headers:session.headers})).json();assert.equal(recoveredAgent.thread.projectId,agentProject.id);assert.equal(recoveredAgent.items[0].id,agentMessage.id);assert.equal(recoveredAgent.items[0].content,agentMessage.content);
  console.info("PASS forced runtime failure during upload, full process/database restart, stale-lock/temp reconciliation, durable bytes, persisted login, reconnection and idempotent upload completion.");
  assert.equal((await(await fetch(appEndpoint,{headers:session.headers})).json()).value.location,"private restart proof");
  await stop();await manageApps(root,"apps-disable","org.dar.prayer-times");await start();assert.equal((await fetch(appEndpoint,{headers:session.headers})).status,404);
  await stop();await manageApps(root,"apps-uninstall","org.dar.prayer-times");await manageApps(root,"apps-install",path.resolve(repo,"apps/prayer-times/dist"));await start();assert.equal((await(await fetch(appEndpoint,{headers:session.headers})).json()).value.location,"private restart proof");
  console.info("PASS external app data survives crash/restart and explicit disable/uninstall/reinstall without a core rebuild.");
  console.info("PASS Calendar resource and reminder watermark survive the same crash/restart; due delivery resumes without duplication.");
  console.info("PASS exact Notes source/metadata/private trash and recurring VTODO survive the same crash; task reminders catch up exactly once.");
  console.info("PASS Agent project, thread and exact saved message survive the same forced runtime/database restart.");
  console.info("PASS unredeemed OIDC code, active refresh token, client registration, signing keys, native binding and SCIM directory/credential survive the same forced process/database restart.");
} finally { await stop(); }
