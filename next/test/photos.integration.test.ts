import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,readFile} from "node:fs/promises";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {createServer} from "node:net";
import {spawnSync,execFile} from "node:child_process";
import {promisify} from "node:util";
import {startServer} from "../runtime/server.ts";
import {login,connect} from "./integration-helpers.ts";
import type {PhotoAlbum,PhotoDetail,PhotoOverview,PhotoPage} from "../shared/photos.ts";
import type {SearchResults} from "../shared/search.ts";

const run=promisify(execFile);
/** Media fixtures made locally with ffmpeg and exiftool (from the dev shell). */
async function fixtures(dir:string){
  const ffmpeg=(args:string[])=>run("ffmpeg",["-nostdin","-v","error","-y",...args]);
  await ffmpeg(["-f","lavfi","-i","testsrc=size=640x480","-frames:v","1",path.join(dir,"trip.jpg")]);
  await run("exiftool",["-q","-overwrite_original","-DateTimeOriginal=2024:04:05 10:20:30","-OffsetTimeOriginal=+03:00","-Make=Google","-Model=Pixel 9","-FNumber=1.8","-ISO=80","-GPSLatitude=41.0","-GPSLatitudeRef=N","-GPSLongitude=28.97","-GPSLongitudeRef=E","-Orientation#=6",path.join(dir,"trip.jpg")]);
  await ffmpeg(["-f","lavfi","-i","testsrc2=size=320x200","-frames:v","1",path.join(dir,"plain.png")]);
  await ffmpeg(["-f","lavfi","-i","testsrc=size=320x240:rate=10","-t","3","-pix_fmt","yuv420p",path.join(dir,"clip.mp4")]);
  await run("exiftool",["-q","-overwrite_original","-CreateDate=2025:01:02 03:04:05",path.join(dir,"clip.mp4")]);
  return {trip:await readFile(path.join(dir,"trip.jpg")),plain:await readFile(path.join(dir,"plain.png")),clip:await readFile(path.join(dir,"clip.mp4"))};
}

test("Photos: indexing, thumbnails, views, albums, descriptions, search, trash and isolation",{skip:!process.env.DAR_INTEGRATION_ORIGIN,timeout:240000},async()=>{
  const root=await mkdtemp("/tmp/dar-photos-acceptance-"),name="dar-photos-"+randomUUID().slice(0,8),listener=createServer();await new Promise<void>(r=>listener.listen(0,"127.0.0.1",r));const port=(listener.address() as import("node:net").AddressInfo).port;await new Promise<void>(r=>listener.close(()=>r()));const base=`http://127.0.0.1:${port}`;
  const published=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"publish",name,"--js-path",path.resolve(import.meta.dirname,"../module/dist/bundle.js"),"--server",process.env.DAR_DATABASE_URL!,"--yes","--no-config"],{env:{...process.env,DAR_ISSUER:base+"/auth/issuer"},encoding:"utf8",timeout:30000});assert.equal(published.status,0,published.stdout+published.stderr);
  const media=await fixtures(root);
  const runtime=await startServer({host:"127.0.0.1",port,origin:base,root:path.join(root,"state"),databaseUrl:process.env.DAR_DATABASE_URL!,databaseName:name,serviceTokenFile:process.env.DAR_DATABASE_TOKEN_FILE!,bootstrapTokenFile:process.env.DAR_BOOTSTRAP_TOKEN_FILE!,development:true});
  try{
    const setup=await fetch(base+"/api/v1/auth/setup",{method:"POST",headers:{origin:base,"content-type":"application/json"},body:JSON.stringify({username:"admin",displayName:"Photos fixture",password:"a long isolated test password",bootstrapToken:await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!,"utf8")})});assert.equal(setup.status,200);
    const session=await login(base),headers={...session.headers,"content-type":"application/json"};
    const raw=(route:string,init:RequestInit={},as=headers)=>fetch(base+"/api/v1/photos"+route,{...init,headers:{...as,...init.headers}});
    const call=async<T>(route:string,body?:unknown,method=body===undefined?"GET":"POST",as=headers):Promise<T>=>{const response=await raw(route,{method,...body!==undefined?{body:JSON.stringify(body)}:{}},as);assert.ok(response.ok,`${method} ${route}: ${response.status} ${await response.clone().text()}`);return await response.json() as T;};
    async function put(contents:Buffer,fileName:string,mediaType:string,parentId:string,as=headers){
      const reservation=await fetch(base+"/api/v1/files/uploads",{method:"POST",headers:as,body:JSON.stringify({parentId,name:fileName,size:contents.length,mediaType})});assert.equal(reservation.status,201,await reservation.clone().text());
      const transfer=await fetch(`${base}/api/v1/files/uploads/${(await reservation.json()).id}`,{method:"PUT",headers:{cookie:as.cookie,origin:base,"x-csrf-token":as["x-csrf-token"]},body:new Uint8Array(contents)});assert.equal(transfer.status,200);
      return (await transfer.json() as {id:string}).id;
    }
    const page=(query="")=>call<PhotoPage>(`/assets${query}`);

    // Uploads land in Photos/YYYY/MM (created once) and index immediately.
    const target=await call<{parentId:string}>("/upload-target",{year:2024,month:4});
    assert.equal((await call<{parentId:string}>("/upload-target",{year:2024,month:4})).parentId,target.parentId);
    const trip=await put(media.trip,"trip.jpg","image/jpeg",target.parentId),clip=await put(media.clip,"clip.mp4","video/mp4",target.parentId);
    const broken=await put(Buffer.from("not really a jpeg"),"broken.jpg","image/jpeg",target.parentId);
    for(const id of [trip,clip,broken])assert.deepEqual(await call(`/assets/${id}/index`,{}),{indexed:true});
    assert.deepEqual(await call(`/assets/${trip}/index`,{}),{indexed:false});

    // Files outside Photos are found by the background scan (kicked by the overview); text files are ignored.
    const plain=await put(media.plain,"plain.png","image/png","");await put(Buffer.from("hello"),"notes.txt","text/plain","");
    let library:PhotoPage={items:[],cursor:""};
    for(let i=0;i<60&&library.items.length<4;i++){await call<PhotoOverview>("/overview");library=await page();if(library.items.length<4)await new Promise(r=>setTimeout(r,500));}
    assert.equal(library.items.length,4);assert.ok(!library.items.some(i=>i.name==="notes.txt"));
    const detail=await call<PhotoDetail>(`/assets/${trip}`);
    assert.equal(detail.takenAt,Date.parse("2024-04-05T07:20:30Z"));assert.equal(detail.width,480);assert.equal(detail.height,640);
    assert.equal(detail.exif.model,"Pixel 9");assert.equal(detail.exif.fNumber,1.8);assert.equal(detail.latitude,41);assert.equal(detail.longitude,28.97);assert.equal(detail.parentId,target.parentId);
    const video=library.items.find(i=>i.id===clip)!;assert.equal(video.kind,"video");assert.equal(video.duration,3);assert.equal(video.takenAt,Date.parse("2025-01-02T03:04:05Z"));
    assert.equal(library.items.find(i=>i.id===broken)!.thumbnail,"failed");
    // Newest first; paging returns the rest without repeats.
    assert.deepEqual(library.items.map(i=>i.takenAt),[...library.items.map(i=>i.takenAt)].sort((a,b)=>b-a));
    const first=await page("?limit=2"),second=await page(`?limit=2&cursor=${encodeURIComponent(first.cursor)}`);
    assert.equal(first.items.length,2);assert.deepEqual([...first.items,...second.items].map(i=>i.id),library.items.map(i=>i.id));

    // Thumbnails: WebP with an ETag, a larger preview, and the original as a fallback for decodable images.
    const small=await raw(`/assets/${trip}/thumbnail`);assert.equal(small.status,200);assert.equal(small.headers.get("content-type"),"image/webp");
    const bytes=Buffer.from(await small.arrayBuffer());assert.equal(bytes.subarray(8,12).toString(),"WEBP");
    assert.equal((await raw(`/assets/${trip}/thumbnail`,{headers:{"if-none-match":small.headers.get("etag")!}})).status,304);
    assert.equal((await raw(`/assets/${clip}/thumbnail?size=m`)).headers.get("content-type"),"image/webp");
    const fallback=await raw(`/assets/${broken}/thumbnail`);assert.equal(fallback.status,200);assert.equal(await fallback.text(),"not really a jpeg");

    // Favorites, videos and archive views with counts.
    await call("/assets",{ids:[trip,plain],favorite:true},"PATCH");await call("/assets",{ids:[plain],archived:true},"PATCH");
    let overview=await call<PhotoOverview>("/overview");assert.deepEqual(overview.counts,{library:3,favorites:2,videos:1,archive:1,trash:0,duplicates:0,shared:0});assert.equal(overview.uploadFolder,"pics");
    assert.deepEqual((await page("?view=favorites")).items.map(i=>i.id).sort(),[trip,plain].sort());
    assert.deepEqual((await page("?view=videos")).items.map(i=>i.id),[clip]);assert.deepEqual((await page("?view=archive")).items.map(i=>i.id),[plain]);
    assert.ok(!(await page()).items.some(i=>i.id===plain));
    assert.equal((await raw("/assets",{method:"PATCH",body:JSON.stringify({ids:[trip]})})).status,400);

    // Descriptions use revision checks.
    const described=await call<{revision:number}>(`/assets/${trip}/description`,{description:"  Bosphorus at dawn ",revision:detail.revision+1},"PUT");
    assert.equal((await call<PhotoDetail>(`/assets/${trip}`)).description,"Bosphorus at dawn");
    const stale=await raw(`/assets/${trip}/description`,{method:"PUT",body:JSON.stringify({description:"x",revision:detail.revision})});assert.equal(stale.status,409);assert.match((await stale.json()).error.message,/changed elsewhere/);
    assert.ok(described.revision>detail.revision);

    // Albums.
    const album=await call<PhotoAlbum>("/albums",{name:"Istanbul",ids:[trip]});assert.equal(album.count,1);assert.equal(album.coverId,trip);
    assert.equal((await call<PhotoAlbum&{added:number}>(`/albums/${album.id}/add`,{ids:[trip,clip,plain]})).added,2);
    assert.deepEqual((await page(`?view=album&album=${album.id}`)).items.map(i=>i.id),[plain,clip,trip]);
    assert.deepEqual((await call<PhotoDetail>(`/assets/${trip}`)).albums,[{id:album.id,name:"Istanbul"}]);
    const renamed=await call<PhotoAlbum>(`/albums/${album.id}`,{name:"Istanbul 2024",revision:(await call<PhotoOverview>("/overview")).albums[0].revision},"PATCH");assert.equal(renamed.name,"Istanbul 2024");
    assert.equal((await raw(`/albums/${album.id}`,{method:"PATCH",body:JSON.stringify({name:"Old",revision:1})})).status,409);
    assert.equal((await call<PhotoAlbum&{removed:number}>(`/albums/${album.id}/remove`,{ids:[clip]})).removed,1);
    assert.equal((await call<PhotoOverview>("/overview")).albums[0].count,2);

    // Search finds photos by name and description, with a thumbnail URL.
    const found=await (await fetch(`${base}/api/v1/search?q=bosphorus`,{headers})).json() as SearchResults;
    assert.deepEqual(found.items.filter(i=>i.group==="Photos").map(i=>[i.id,i.thumbnail]),[[trip,`/api/v1/photos/assets/${trip}/thumbnail`]]);
    assert.ok((await (await fetch(`${base}/api/v1/search?q=istanbul`,{headers})).json() as SearchResults).items.some(i=>i.group==="Photos"&&i.href===`/photos?album=${album.id}`));

    // Another account sees nothing and cannot use these IDs.
    assert.equal((await fetch(base+"/api/v1/auth/users",{method:"POST",headers,body:JSON.stringify({username:"other",displayName:"Other",password:"a long isolated test password"})})).status,201);
    const otherSession=await login(base,"other"),other={...otherSession.headers,"content-type":"application/json"};
    assert.deepEqual((await call<PhotoPage>("/assets",undefined,"GET",other)).items,[]);
    for(const route of [`/assets/${trip}`,`/assets/${trip}/thumbnail`])assert.equal((await raw(route,{},other)).status,404);
    assert.equal((await raw("/assets",{method:"PATCH",body:JSON.stringify({ids:[trip],favorite:false})},other)).status,404);
    const theirs=await call<PhotoAlbum>("/albums",{name:"Mine"},"POST",other);
    assert.equal((await raw(`/albums/${theirs.id}/add`,{method:"POST",body:JSON.stringify({ids:[trip]})},other)).status,404);
    assert.equal((await raw(`/albums/${album.id}`,{method:"DELETE"},other)).status,404);
    assert.equal((await raw(`/assets/${trip}/index`,{method:"POST"},other)).status,404);

    // Trashing in Files hides the photo everywhere; its metadata survives a restore from trash.
    const db=await connect(session);
    try{
      await db.reducers.trashEntry({id:trip,expectedRevision:1});
      assert.ok(!(await page()).items.some(i=>i.id===trip));assert.ok((await call<PhotoDetail>(`/assets/${trip}`)).trashedAt);assert.deepEqual((await page("?view=trash")).items.map(i=>i.id),[trip]);assert.equal((await raw(`/assets/${trip}/description`,{method:"PUT",body:JSON.stringify({description:"x",revision:99})})).status,404);
      assert.equal((await call<PhotoOverview>("/overview")).albums[0].count,1);
    }finally{db.disconnect();}

    // Deleting an album keeps its photos.
    await call(`/albums/${album.id}`,undefined,"DELETE");assert.equal((await call<PhotoOverview>("/overview")).albums.length,0);assert.ok((await page()).items.some(i=>i.id===clip));
    assert.equal((await raw(`/assets/not-an-id`)).status,404);

    const browser=await run(process.execPath,["test/photos-browser.mjs"],{cwd:path.resolve(import.meta.dirname,".."),env:{...process.env,DAR_INTEGRATION_ORIGIN:base,DAR_TEST_ROOT:root},encoding:"utf8",timeout:120000});console.log(browser.stdout.trim());
  }finally{await runtime.close();}
});
