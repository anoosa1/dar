import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,readFile,writeFile} from "node:fs/promises";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {createServer} from "node:net";
import {spawnSync,execFile} from "node:child_process";
import {promisify} from "node:util";
import {startServer} from "../runtime/server.ts";
import {login,connect} from "./integration-helpers.ts";
import {readProcedure} from "../client/api.ts";
import type {PhotoAsset,PhotoDetail,PhotoOverview,PhotoPage,PhotoStack,PhotoDuplicatePage,PhotoMoved} from "../shared/photos.ts";
import type {SearchResults} from "../shared/search.ts";
import type {LockedFolderState} from "../shared/locked-folder.ts";

const run=promisify(execFile);
/** Distinct pictures (different test patterns), a Live Photo pair, a RAW+JPEG pair and videos needing a remux or transcode. */
async function fixtures(dir:string){
  const ffmpeg=(args:string[])=>run("ffmpeg",["-nostdin","-v","error","-y",...args]);
  const still=async(source:string,file:string)=>{await ffmpeg(["-f","lavfi","-i",`${source}=size=480x360`,"-frames:v","1",path.join(dir,file)]);return readFile(path.join(dir,file));};
  const live=await still("testsrc2","live.jpg"),pair=await still("smptebars","pair.jpg"),sea=await still("mandelbrot","sea.jpg"),secret=await still("rgbtestsrc","secret.jpg");
  await ffmpeg(["-f","lavfi","-i","smptebars=size=480x360","-frames:v","1",path.join(dir,"pair.tiff")]);
  await ffmpeg(["-f","lavfi","-i","testsrc2=size=480x360:rate=10","-t","2","-pix_fmt","yuv420p","-c:v","libx264",path.join(dir,"live.mov")]);
  await ffmpeg(["-f","lavfi","-i","testsrc=size=320x240:rate=10","-t","2","-c:v","mpeg4",path.join(dir,"odd.mkv")]);
  return {live,pair,sea,secret,raw:await readFile(path.join(dir,"pair.tiff")),motion:await readFile(path.join(dir,"live.mov")),odd:await readFile(path.join(dir,"odd.mkv"))};
}

test("Photos library: trash, Locked folder, Live Photos, stacks, duplicates, video playback and the upload folder",{skip:!process.env.DAR_INTEGRATION_ORIGIN,timeout:300000},async()=>{
  const root=await mkdtemp("/tmp/dar-photos-library-"),name="dar-photos-"+randomUUID().slice(0,8),listener=createServer();await new Promise<void>(r=>listener.listen(0,"127.0.0.1",r));const port=(listener.address() as import("node:net").AddressInfo).port;await new Promise<void>(r=>listener.close(()=>r()));const base=`http://127.0.0.1:${port}`;
  const published=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"publish",name,"--js-path",path.resolve(import.meta.dirname,"../module/dist/bundle.js"),"--server",process.env.DAR_DATABASE_URL!,"--yes","--no-config"],{env:{...process.env,DAR_ISSUER:base+"/auth/issuer"},encoding:"utf8",timeout:30000});assert.equal(published.status,0,published.stdout+published.stderr);
  const media=await fixtures(root);
  const runtime=await startServer({host:"127.0.0.1",port,origin:base,root:path.join(root,"state"),databaseUrl:process.env.DAR_DATABASE_URL!,databaseName:name,serviceTokenFile:process.env.DAR_DATABASE_TOKEN_FILE!,bootstrapTokenFile:process.env.DAR_BOOTSTRAP_TOKEN_FILE!,development:true});
  try{
    const setup=await fetch(base+"/api/v1/auth/setup",{method:"POST",headers:{origin:base,"content-type":"application/json"},body:JSON.stringify({username:"admin",displayName:"Photos fixture",password:"a long isolated test password",bootstrapToken:await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!,"utf8")})});assert.equal(setup.status,200);
    const session=await login(base),headers={...session.headers,"content-type":"application/json"};
    const raw=(route:string,init:RequestInit={},as=headers)=>fetch(base+"/api/v1/photos"+route,{...init,headers:{...as,...init.headers}});
    const call=async<T>(route:string,body?:unknown,method=body===undefined?"GET":"POST",as=headers):Promise<T>=>{const response=await raw(route,{method,...body!==undefined?{body:JSON.stringify(body)}:{}},as);assert.ok(response.ok,`${method} ${route}: ${response.status} ${await response.clone().text()}`);return await response.json() as T;};
    const lockedApi=(action="",body?:unknown,as=headers)=>fetch(`${base}/api/v1/files/locked${action}`,{method:action?"POST":"GET",headers:as,...body?{body:JSON.stringify(body)}:{}});
    async function put(contents:Buffer,fileName:string,mediaType:string,parentId:string){
      const reservation=await fetch(base+"/api/v1/files/uploads",{method:"POST",headers,body:JSON.stringify({parentId,name:fileName,size:contents.length,mediaType})});assert.equal(reservation.status,201,await reservation.clone().text());
      const transfer=await fetch(`${base}/api/v1/files/uploads/${(await reservation.json()).id}`,{method:"PUT",headers:session.headers,body:new Uint8Array(contents)});assert.equal(transfer.status,200);
      const id=(await transfer.json() as {id:string}).id;assert.deepEqual(await call(`/assets/${id}/index`,{}),{indexed:true});return id;
    }
    const page=async(query="")=>(await call<PhotoPage>(`/assets${query}`)).items;
    const names=(items:PhotoAsset[])=>items.map(i=>i.name).sort();

    // Uploads go to pics/YYYY/MM by default; the folder is a per-account preference.
    const {parentId}=await call<{parentId:string}>("/upload-target",{year:2024,month:5});
    assert.equal((await call<PhotoOverview>("/overview")).uploadFolder,"pics");
    const ids={
      live:await put(media.live,"live.jpg","image/jpeg",parentId),motion:await put(media.motion,"live.mov","video/quicktime",parentId),
      pair:await put(media.pair,"pair.jpg","image/jpeg",parentId),raw:await put(media.raw,"pair.dng","image/x-adobe-dng",parentId),
      sea:await put(media.sea,"sea.jpg","image/jpeg",parentId),copy:await put(media.sea,"sea copy.jpg","image/jpeg",parentId),
      secret:await put(media.secret,"secret.jpg","image/jpeg",parentId),odd:await put(media.odd,"odd.mkv","video/x-matroska",parentId),
    };
    const db=await connect(session);
    try{
      const folderNames=readProcedure<{ancestors:{name:string}[];entry:{name:string}}>(await db.procedures.fileDetails({id:parentId}));
      assert.deepEqual([...folderNames.ancestors.map(a=>a.name),folderNames.entry.name],["pics","2024","05"]);
    }finally{db.disconnect();}

    // A Live Photo is one tile; RAW+JPEG stack under the JPEG.
    let library=await page();
    assert.deepEqual(names(library),["live.jpg","odd.mkv","pair.jpg","sea copy.jpg","sea.jpg","secret.jpg"]);
    const still=library.find(i=>i.id===ids.live)!;assert.equal(still.live,ids.motion);
    const cover=library.find(i=>i.id===ids.pair)!;assert.equal(cover.stack?.count,2);
    const stack=await call<PhotoStack>(`/stacks/${cover.stack!.id}`);assert.equal(stack.primaryId,ids.pair);assert.deepEqual(stack.items.map(i=>i.id).sort(),[ids.pair,ids.raw].sort());
    assert.equal((await call<PhotoOverview>("/overview")).counts.library,6);

    // Videos: H.264 in MOV is remuxed, MPEG-4 Part 2 in MKV is transcoded; both stream with ranges.
    assert.equal((await call<PhotoDetail>(`/assets/${ids.motion}`)).playback,"remux");
    const odd=await call<PhotoDetail>(`/assets/${ids.odd}`);assert.equal(odd.playback,"transcode");assert.equal(odd.videoCodec,"mpeg4");
    for(const id of [ids.odd,ids.motion]){
      assert.equal((await raw(`/assets/${id}/video`)).status,404);
      let state={state:"preparing",url:""};
      for(let i=0;i<120&&state.state==="preparing";i++){state=await call(`/assets/${id}/playback`,{});if(state.state==="preparing")await new Promise(r=>setTimeout(r,500));}
      assert.equal(state.state,"ready");
      const partial=await fetch(base+state.url,{headers:{...session.headers,range:"bytes=0-99"}});
      assert.equal(partial.status,206);assert.equal(partial.headers.get("content-type"),"video/mp4");assert.equal((await partial.arrayBuffer()).byteLength,100);
      const file=path.join(root,`${id}.mp4`);await writeFile(file,Buffer.from(await (await fetch(base+state.url,{headers:session.headers})).arrayBuffer()));
      assert.equal((await run("ffprobe",["-v","error","-select_streams","v:0","-show_entries","stream=codec_name","-of","csv=p=0",file])).stdout.trim(),"h264");
    }

    // Manual stacks: cover, change of cover, unstack.
    const manual=await call<{id:string}>("/stacks",{ids:[ids.secret,ids.odd]});
    library=await page();assert.ok(!library.some(i=>i.id===ids.odd));assert.equal(library.find(i=>i.id===ids.secret)!.stack?.count,2);
    await call(`/stacks/${manual.id}`,{primaryId:ids.odd},"PATCH");assert.ok((await page()).some(i=>i.id===ids.odd&&i.stack));
    await call(`/stacks/${manual.id}`,undefined,"DELETE");assert.equal((await page()).length,6);

    // Duplicates: identical copies group (the RAW+JPEG stack does not); keeping one merges albums and favorites and trashes the rest.
    const album=await call<{id:string}>("/albums",{name:"Coast",ids:[ids.copy]});await call("/assets",{ids:[ids.copy],favorite:true},"PATCH");
    const duplicates=await call<PhotoDuplicatePage>("/duplicates");
    assert.equal(duplicates.groups.length,1);assert.equal(duplicates.groups[0].identical,true);assert.deepEqual(duplicates.groups[0].items.map(i=>i.id).sort(),[ids.sea,ids.copy].sort());
    assert.equal(duplicates.groups[0].items[0].path,"pics/2024/05");
    assert.equal((await call<PhotoOverview>("/overview")).counts.duplicates,1);
    assert.equal((await raw("/duplicates/resolve",{method:"POST",body:JSON.stringify({key:duplicates.groups[0].key,keep:[ids.live]})})).status,409);
    const resolved=await call<{trashed:string[]}>("/duplicates/resolve",{key:duplicates.groups[0].key,keep:[ids.sea]});assert.deepEqual(resolved.trashed,[ids.copy]);
    const kept=await call<PhotoDetail>(`/assets/${ids.sea}`);assert.equal(kept.favorite,true);assert.deepEqual(kept.albums.map(a=>a.id),[album.id]);
    let overview=await call<PhotoOverview>("/overview");assert.equal(overview.counts.duplicates,0);assert.equal(overview.counts.trash,1);

    // Trash: list, restore, delete permanently; a Live Photo moves with its motion part.
    let trash=await page("?view=trash");assert.deepEqual(trash.map(i=>i.id),[ids.copy]);assert.ok(trash[0].trashedAt);
    assert.equal((await raw(`/assets/${ids.copy}/thumbnail`)).status,200);
    await call("/assets/restore",{ids:[ids.copy]});assert.ok((await page()).some(i=>i.id===ids.copy));
    assert.equal((await call<PhotoDuplicatePage>("/duplicates")).groups.length,1);
    await call("/duplicates/dismiss",{key:(await call<PhotoDuplicatePage>("/duplicates")).groups[0].key});assert.equal((await call<PhotoDuplicatePage>("/duplicates")).groups.length,0);
    await call("/assets/trash",{ids:[ids.copy]});assert.deepEqual(await call("/assets/delete",{ids:[ids.copy]}),{deleted:[ids.copy]});
    assert.equal((await raw(`/assets/${ids.copy}`)).status,404);
    assert.deepEqual((await call<{trashed:string[]}>("/assets/trash",{ids:[ids.live]})).trashed.sort(),[ids.live,ids.motion].sort());
    trash=await page("?view=trash");assert.deepEqual(trash.map(i=>i.id),[ids.live]);assert.equal(trash[0].live,ids.motion);
    assert.deepEqual((await call<{restored:string[]}>("/assets/restore",{ids:[ids.live]})).restored.sort(),[ids.live,ids.motion].sort());
    assert.equal((await page()).find(i=>i.id===ids.live)!.live,ids.motion);
    await call("/assets/trash",{ids:[ids.odd]});assert.deepEqual(await call("/trash/empty",{}),{deleted:1,remaining:false});assert.equal((await page("?view=trash")).length,0);

    // Locked folder: needs the account password, hides photos everywhere else, and moves them in Files.
    assert.deepEqual(await (await lockedApi()).json(),{folderId:"",unlockedUntil:0});
    assert.equal((await raw("/assets/lock",{method:"POST",body:JSON.stringify({ids:[ids.secret]})})).status,423);
    assert.equal((await raw("/assets?view=locked")).status,423);
    assert.equal((await lockedApi("/unlock",{password:"wrong password for this test"})).status,403);
    const unlocked=await (await lockedApi("/unlock",{password:"a long isolated test password"})).json() as LockedFolderState;
    assert.match(unlocked.folderId,/^[a-f0-9-]{36}$/);assert.ok(unlocked.unlockedUntil>Date.now()+9*60_000);
    const {moved}=await call<{moved:PhotoMoved[]}>("/assets/lock",{ids:[ids.secret]});assert.deepEqual(moved,[{id:ids.secret,parentId}]);
    assert.ok(!(await page()).some(i=>i.id===ids.secret));assert.deepEqual((await page("?view=locked")).map(i=>i.id),[ids.secret]);
    assert.equal((await call<PhotoDetail>(`/assets/${ids.secret}`)).locked,true);
    const found=await (await fetch(`${base}/api/v1/search?q=secret`,{headers})).json() as SearchResults;assert.equal(found.items.length,0);
    assert.equal((await raw("/assets/trash",{method:"POST",body:JSON.stringify({ids:[ids.secret]})})).status,404);
    const files=await connect(session);
    try{
      // Files: the unlocked session browses the Locked folder; it is not in All files and refuses the trash.
      await files.reducers.browse({parentId:"",section:"files",query:"",cursor:""});assert.ok(![...files.db.myFiles.iter()].some(e=>e.id===unlocked.folderId));
      await files.reducers.browse({parentId:unlocked.folderId,section:"files",query:"",cursor:""});assert.deepEqual([...files.db.myFiles.iter()].map(e=>e.name),["secret.jpg"]);
      const entry=[...files.db.myFiles.iter()][0];
      await assert.rejects(files.reducers.trashEntry({id:entry.id,expectedRevision:entry.revision}),/lockedFolder/);
      await assert.rejects(files.reducers.renameEntry({id:unlocked.folderId,name:"Open",expectedRevision:1}),/forbidden/);
      await assert.rejects(files.reducers.grantAccess({id:randomUUID(),fileId:entry.id,username:"admin",role:"read",expectedRevision:entry.revision}),/lockedFolder/);
    }finally{files.disconnect();}
    // Another browser session of the same account sees nothing until it unlocks too.
    const second=await login(base),secondHeaders={...second.headers,"content-type":"application/json"};
    assert.equal((await raw(`/assets/${ids.secret}/thumbnail`,{},secondHeaders)).status,423);
    assert.equal((await raw("/assets?view=locked",{},secondHeaders)).status,423);
    assert.equal((await fetch(`${base}/api/v1/files/${ids.secret}/content`,{headers:second.headers})).status,404);
    const other=await connect(second);
    try{
      await assert.rejects(other.reducers.browse({parentId:unlocked.folderId,section:"files",query:"",cursor:""}),/notFound/);
      assert.match(JSON.parse(await other.procedures.fileDetails({id:ids.secret})).error.code,/notFound/);
    }finally{other.disconnect();}
    // Undo returns the photo where it was; deleting inside the Locked folder is permanent.
    await call("/assets/unlock",{ids:[ids.secret],to:moved});assert.ok((await page()).some(i=>i.id===ids.secret));
    assert.equal((await call<PhotoDetail>(`/assets/${ids.secret}`)).parentId,parentId);
    await call("/assets/lock",{ids:[ids.secret]});await call("/assets/delete",{ids:[ids.secret]});assert.equal((await page("?view=locked")).length,0);
    assert.equal((await raw(`/assets/${ids.secret}`)).status,404);
    await lockedApi("/lock",{});assert.equal((await raw("/assets?view=locked")).status,423);

    // The upload folder preference.
    const preferences=await (await fetch(`${base}/api/v1/pim/preferences`,{headers})).json() as {revision:number};
    const save=(value:string,revision:number)=>fetch(`${base}/api/v1/pim/preferences`,{method:"PUT",headers:{...headers,"if-match":`"preferences:${revision}"`},body:JSON.stringify({photosUploadFolder:value})});
    assert.equal((await save("../escape",preferences.revision)).status,400);
    assert.equal((await save(" Media / Camera ",preferences.revision)).status,200);
    overview=await call<PhotoOverview>("/overview");assert.equal(overview.uploadFolder,"Media/Camera");
    const moved2=await call<{parentId:string}>("/upload-target",{year:2024,month:5});assert.notEqual(moved2.parentId,parentId);
  }finally{await runtime.close();}
});
