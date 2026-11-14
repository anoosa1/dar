import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,readFile} from "node:fs/promises";
import path from "node:path";
import {randomUUID,createHash} from "node:crypto";
import http from "node:http";
import {createServer} from "node:net";
import {spawnSync,execFile} from "node:child_process";
import {promisify} from "node:util";
import {startServer} from "../runtime/server.ts";
import {login,connect} from "./integration-helpers.ts";
import {readProcedure} from "../client/api.ts";
import type {PhotoDetail,PhotoOverview,PhotoPage} from "../shared/photos.ts";

const run=promisify(execFile);
const sha1=(bytes:Buffer)=>createHash("sha1").update(bytes).digest("base64");
async function freePort(){const s=createServer();await new Promise<void>(r=>s.listen(0,"127.0.0.1",r));const port=(s.address() as import("node:net").AddressInfo).port;await new Promise<void>(r=>s.close(()=>r()));return port;}
function multipart(fields:Record<string,string>,file:{name:string;type:string;bytes:Buffer}){
  const boundary="----dar"+randomUUID(),parts:Buffer[]=[];
  for(const [name,value] of Object.entries(fields))parts.push(Buffer.from(`--${boundary}\r\nContent-Disposition: form-data; name="${name}"\r\n\r\n${value}\r\n`));
  parts.push(Buffer.from(`--${boundary}\r\nContent-Disposition: form-data; name="assetData"; filename="${file.name}"\r\nContent-Type: ${file.type}\r\n\r\n`),file.bytes,Buffer.from(`\r\n--${boundary}--\r\n`));
  return {body:Buffer.concat(parts),type:`multipart/form-data; boundary=${boundary}`};
}
interface Line {type:string;data:Record<string,unknown>;ack:string}

/** A small Immich server for the connector: the endpoints and shapes of the v3.4.1 API it uses. */
async function fakeImmich(assets:{id:string;bytes:Buffer;name:string;type:string;favorite:boolean;createdAt:string}[],albums:{id:string;albumName:string;assetIds:string[]}[]){
  const received:{checksum:string;name:string}[]=[];
  const server=http.createServer((req,res)=>{void (async()=>{
    const chunks:Buffer[]=[];for await (const c of req)chunks.push(c as Buffer);const body=Buffer.concat(chunks),url=new URL(req.url!,"http://x");
    const json=(v:unknown,s=200)=>{res.writeHead(s,{"content-type":"application/json"});res.end(JSON.stringify(v));};
    // A proxy in front that moved Immich: same host, permanent redirect.
    if(url.pathname.startsWith("/old/")){res.writeHead(308,{location:url.pathname.slice(4)+url.search});res.end();return;}
    if(url.pathname==="/api/server/ping")return json({res:"pong"});
    if(req.headers["x-api-key"]!=="good-key")return json({message:"Invalid API key",statusCode:401},401);
    if(url.pathname==="/api/users/me")return json({id:randomUUID(),name:"Remote",email:"remote@example.com"});
    if(url.pathname==="/api/search/metadata"&&req.method==="POST"){
      const p=JSON.parse(body.toString()),list=p.albumIds?assets.filter(a=>albums.find(al=>al.id===p.albumIds[0])?.assetIds.includes(a.id)):assets;
      const items=list.map(a=>({id:a.id,checksum:sha1(a.bytes),originalFileName:a.name,originalMimeType:a.type,fileCreatedAt:a.createdAt,fileModifiedAt:a.createdAt,isFavorite:a.favorite,visibility:"timeline",isTrashed:false,type:"IMAGE",exifInfo:{description:a.favorite?"from immich":null}}));
      return json({assets:{items,nextPage:null,count:items.length,total:items.length,facets:[],nextCursor:null},albums:{items:[],total:0,count:0,facets:[]}});
    }
    const original=/^\/api\/assets\/([^/]+)\/original$/.exec(url.pathname);if(original){const a=assets.find(x=>x.id===original[1]);if(!a)return json({},404);res.writeHead(200,{"content-type":a.type});res.end(a.bytes);return;}
    if(url.pathname==="/api/albums")return json(albums.map(a=>({id:a.id,albumName:a.albumName,description:"",assetCount:a.assetIds.length})));
    if(url.pathname==="/api/assets"&&req.method==="POST"){received.push({checksum:String(req.headers["x-immich-checksum"]),name:/filename="([^"]+)"/.exec(body.toString("latin1"))?.[1]??""});return json({id:randomUUID(),status:"created"},201);}
    json({},404);
  })();});
  await new Promise<void>(r=>server.listen(0,"127.0.0.1",r));
  return {url:`http://127.0.0.1:${(server.address() as import("node:net").AddressInfo).port}`,received,close:()=>new Promise<void>(r=>server.close(()=>r()))};
}

test("Immich: mobile-app API (login, backup, sync stream, albums, media) and the Immich server connector",{skip:!process.env.DAR_INTEGRATION_ORIGIN,timeout:300000},async()=>{
  const root=await mkdtemp("/tmp/dar-photos-immich-"),name="dar-photos-"+randomUUID().slice(0,8),port=await freePort(),base=`http://127.0.0.1:${port}`;
  const published=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"publish",name,"--js-path",path.resolve(import.meta.dirname,"../module/dist/bundle.js"),"--server",process.env.DAR_DATABASE_URL!,"--yes","--no-config"],{env:{...process.env,DAR_ISSUER:base+"/auth/issuer"},encoding:"utf8",timeout:30000});assert.equal(published.status,0,published.stdout+published.stderr);
  const ffmpeg=(args:string[])=>run("ffmpeg",["-nostdin","-v","error","-y",...args]);
  await ffmpeg(["-f","lavfi","-i","testsrc2=size=320x240","-frames:v","1",path.join(root,"beach.jpg")]);
  await ffmpeg(["-f","lavfi","-i","testsrc=size=320x240:rate=10","-t","2","-pix_fmt","yuv420p",path.join(root,"wave.mp4")]);
  await ffmpeg(["-f","lavfi","-i","mandelbrot=size=320x240","-frames:v","1",path.join(root,"remote.jpg")]);
  await ffmpeg(["-f","lavfi","-i","rgbtestsrc=size=320x240","-frames:v","1",path.join(root,"solo.jpg")]);
  const beach=await readFile(path.join(root,"beach.jpg")),wave=await readFile(path.join(root,"wave.mp4")),remoteBytes=await readFile(path.join(root,"remote.jpg")),solo=await readFile(path.join(root,"solo.jpg"));
  const runtime=await startServer({host:"127.0.0.1",port,origin:base,root:path.join(root,"state"),databaseUrl:process.env.DAR_DATABASE_URL!,databaseName:name,serviceTokenFile:process.env.DAR_DATABASE_TOKEN_FILE!,bootstrapTokenFile:process.env.DAR_BOOTSTRAP_TOKEN_FILE!,development:true});
  let fake:Awaited<ReturnType<typeof fakeImmich>>|undefined;
  try{
    const setup=await fetch(base+"/api/v1/auth/setup",{method:"POST",headers:{origin:base,"content-type":"application/json"},body:JSON.stringify({username:"admin",displayName:"Anas",password:"a long isolated test password",bootstrapToken:await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!,"utf8")})});assert.equal(setup.status,200);
    const session=await login(base),headers={...session.headers,"content-type":"application/json"};
    const appPassword=async(scope:string)=>{const r=await fetch(base+"/api/v1/auth/app-passwords",{method:"POST",headers,body:JSON.stringify({label:"Phone",scope})});assert.equal(r.status,201,await r.clone().text());return await r.json() as {secret:string;immichUrl?:string};};
    const photosPassword=await appPassword("v2 photos:read photos:write"),filesPassword=await appPassword("v2 files:read");
    assert.equal(photosPassword.immichUrl,base);

    // Discovery and public server information.
    assert.deepEqual(await (await fetch(base+"/.well-known/immich")).json(),{api:{endpoint:"/dav/immich/api"}});
    const api=base+"/dav/immich/api";
    assert.deepEqual(await (await fetch(api+"/server/ping")).json(),{res:"pong"});
    const version=await (await fetch(api+"/server/version")).json() as {major:number;minor:number;patch:number};assert.deepEqual([version.major,version.minor,version.patch],[3,4,1]);
    assert.equal((await (await fetch(api+"/server/features")).json() as {passwordLogin:boolean}).passwordLogin,true);
    assert.ok((await (await fetch(api+"/server/media-types")).json() as {image:string[]}).image.includes(".heic"));

    // Sign-in: the username before "@", and an app password that allows Photos.
    const signIn=(email:string,password:string)=>fetch(api+"/auth/login",{method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify({email,password})});
    assert.equal((await signIn("admin@dar","a long isolated test password")).status,401);
    const refused=await signIn("admin@dar",filesPassword.secret);assert.equal(refused.status,401);assert.match((await refused.json() as {message:string}).message,/no Photos access/);
    const signed=await signIn("Admin@example.org",photosPassword.secret);assert.equal(signed.status,201);
    const {accessToken,userEmail}=await signed.json() as {accessToken:string;userEmail:string};assert.equal(userEmail,"admin@dar");
    const auth={"x-immich-user-token":accessToken};
    const call=async<T>(route:string,init:{method?:string;body?:unknown}={})=>{const r=await fetch(api+route,{method:init.method??(init.body===undefined?"GET":"POST"),headers:{...auth,...init.body!==undefined?{"content-type":"application/json"}:{}},body:init.body===undefined?undefined:JSON.stringify(init.body)});assert.ok(r.ok,`${route}: ${r.status} ${await r.clone().text()}`);return (r.status===204?undefined:await r.json()) as T;};
    assert.equal((await fetch(api+"/users/me",{headers:{"x-immich-user-token":"x".repeat(43)}})).status,401);
    assert.equal((await call<{email:string}>("/users/me")).email,"admin@dar");
    assert.equal((await fetch(api+"/users/me",{headers:{authorization:`Bearer ${accessToken}`}})).status,200);

    // Backup: multipart upload into pics/YYYY/MM, duplicates by SHA-1.
    const upload=async(bytes:Buffer,file:string,type:string,fields:Record<string,string>={})=>{
      const form=multipart({deviceAssetId:file,deviceId:"phone",fileCreatedAt:"2024-03-05T10:00:00.000Z",fileModifiedAt:"2024-03-05T10:00:00.000Z",filename:file,...fields},{name:file,type,bytes});
      const r=await fetch(api+"/assets",{method:"POST",headers:{...auth,"content-type":form.type,"x-immich-checksum":sha1(bytes)},body:new Uint8Array(form.body)});return {http:r.status,...await r.json() as {id:string;status:string}};
    };
    const photo=await upload(beach,"beach.jpg","image/jpeg",{isFavorite:"true"});assert.deepEqual([photo.http,photo.status],[201,"created"]);
    assert.deepEqual(await upload(beach,"beach copy.jpg","image/jpeg"),{http:200,id:photo.id,status:"duplicate"});
    const video=await upload(wave,"wave.mp4","application/octet-stream");assert.equal(video.http,201);
    const check=await call<{results:{id:string;action:string;assetId?:string;reason?:string}[]}>("/assets/bulk-upload-check",{body:{assets:[{id:"a",checksum:sha1(beach)},{id:"b",checksum:sha1(Buffer.from("new"))}]}});
    assert.deepEqual(check.results,[{id:"a",action:"reject",reason:"duplicate",assetId:photo.id,isTrashed:false},{id:"b",action:"accept"}]);
    const detail=await call<{isFavorite:boolean;checksum:string;originalFileName:string;exifInfo:{exifImageWidth:number}}>(`/assets/${photo.id}`);
    assert.equal(detail.isFavorite,true);assert.equal(detail.checksum,sha1(beach));assert.equal(detail.exifInfo.exifImageWidth,320);
    const db=await connect(session);
    try{const where=readProcedure<{ancestors:{name:string}[]}>(await db.procedures.fileDetails({id:photo.id}));assert.deepEqual(where.ancestors.map(a=>a.name),["pics","2024","03"]);}finally{db.disconnect();}

    // Media.
    const thumb=await fetch(api+`/assets/${photo.id}/thumbnail?size=thumbnail`,{headers:auth});assert.equal(thumb.headers.get("content-type"),"image/webp");
    assert.deepEqual(Buffer.from(await (await fetch(api+`/assets/${photo.id}/original`,{headers:auth})).arrayBuffer()),beach);
    const play=await fetch(api+`/assets/${video.id}/video/playback`,{headers:{...auth,range:"bytes=0-9"}});assert.equal(play.status,206);assert.equal(play.headers.get("content-type"),"video/mp4");

    // Sync stream: everything first, then only changes, deletes and resets.
    const types=["AuthUsersV2","UsersV1","AssetsV2","AssetExifsV1","AlbumsV2","AlbumUsersV1","AlbumToAssetsV1","AlbumAssetsV2","StacksV1","PeopleV1","MemoriesV2"];
    const stream=async()=>{const r=await fetch(api+"/sync/stream",{method:"POST",headers:{...auth,"content-type":"application/json",accept:"application/jsonlines+json"},body:JSON.stringify({types})});assert.equal(r.status,200);
      const lines=(await r.text()).trim().split("\n").map(l=>JSON.parse(l) as Line);
      const last=new Map<string,string>();for(const l of lines)last.set(l.ack.split("|")[0],l.ack);await call("/sync/ack",{body:{acks:[...last.values()]}});return lines;};
    const kinds=(lines:Line[])=>lines.map(l=>l.type);
    let lines=await stream();
    assert.equal(kinds(lines).at(-1),"SyncCompleteV1");assert.equal(kinds(lines)[0],"AuthUserV2");
    const assets=lines.filter(l=>l.type==="AssetV2");assert.deepEqual(assets.map(l=>l.data.id).sort(),[photo.id,video.id].sort());
    const synced=assets.find(l=>l.data.id===photo.id)!.data;assert.equal(synced.checksum,sha1(beach));assert.equal(synced.isFavorite,true);assert.equal(synced.type,"IMAGE");assert.equal(synced.visibility,"timeline");
    assert.equal(assets.find(l=>l.data.id===video.id)!.data.duration,2000);
    assert.equal(lines.filter(l=>l.type==="AssetExifV1").length,2);
    assert.deepEqual(kinds(await stream()),["AuthUserV2","UserV1","SyncCompleteV1"]);
    await call(`/assets/${photo.id}`,{method:"PUT",body:{isFavorite:false}});
    lines=await stream();assert.deepEqual(kinds(lines).filter(t=>t.startsWith("Asset")),["AssetV2","AssetExifV1"]);assert.equal(lines.find(l=>l.type==="AssetV2")!.data.isFavorite,false);
    const album=await call<{id:string;albumName:string}>("/albums",{body:{albumName:"Beach",assetIds:[photo.id]}});assert.equal(album.albumName,"Beach");
    lines=await stream();assert.deepEqual(lines.find(l=>l.type==="AlbumV2")!.data.name,"Beach");
    assert.deepEqual(lines.find(l=>l.type==="AlbumUserV1")!.data,{albumId:album.id,userId:(await call<{id:string}>("/users/me")).id,role:"owner"});
    assert.deepEqual(lines.find(l=>l.type==="AlbumToAssetV1")!.data,{albumId:album.id,assetId:photo.id});
    assert.deepEqual(await call(`/albums/${album.id}/assets`,{method:"DELETE",body:{ids:[photo.id]}}),[{id:photo.id,success:true}]);
    lines=await stream();assert.deepEqual(lines.find(l=>l.type==="AlbumToAssetDeleteV1")!.data,{albumId:album.id,assetId:photo.id});
    // Trash keeps the asset with deletedAt; force deletes it; a trashed folder resets the device.
    await call("/assets",{method:"DELETE",body:{ids:[video.id]}});
    lines=await stream();assert.ok(lines.find(l=>l.type==="AssetV2"&&l.data.id===video.id)!.data.deletedAt);
    await call("/trash/restore/assets",{body:{ids:[video.id]}});await call("/assets",{method:"DELETE",body:{ids:[video.id],force:true}});
    lines=await stream();assert.ok(lines.some(l=>l.type==="AssetDeleteV1"&&l.data.assetId===video.id));
    const files=await connect(session);
    try{
      const parent=readProcedure<{ancestors:{id:string;name:string}[]}>(await files.procedures.fileDetails({id:photo.id})).ancestors[0];
      const folder=readProcedure<{entry:{revision:number}}>(await files.procedures.fileDetails({id:parent.id})).entry;
      await files.reducers.trashEntry({id:parent.id,expectedRevision:folder.revision});
      assert.deepEqual(kinds(await stream()),["SyncResetV1"]);
      lines=await stream();assert.equal(lines.filter(l=>l.type==="AssetV2").length,0);assert.equal(kinds(lines).at(-1),"SyncCompleteV1");
      await files.reducers.restoreEntry({id:parent.id,expectedRevision:folder.revision+1});
      assert.deepEqual(kinds(await stream()),["SyncResetV1"]);assert.ok((await stream()).some(l=>l.type==="AssetV2"&&l.data.id===photo.id));
    }finally{files.disconnect();}
    assert.equal((await upload(solo,"solo.jpg","image/jpeg")).http,201);
    // The web app sees the devices; signing out ends the token.
    const devices=(await (await fetch(base+"/api/v1/photos/immich",{headers})).json() as {devices:{id:string}[]}).devices;assert.equal(devices.length,1);
    await call("/auth/logout",{body:{}});assert.equal((await fetch(api+"/users/me",{headers:auth})).status,401);

    // The connector: Immich has one new photo (a favorite) and one Dar already has, in an album.
    const remoteId=randomUUID(),sameId=randomUUID();
    fake=await fakeImmich([{id:remoteId,bytes:remoteBytes,name:"remote.jpg",type:"image/jpeg",favorite:true,createdAt:"2023-08-01T12:00:00.000Z"},{id:sameId,bytes:beach,name:"beach.jpg",type:"image/jpeg",favorite:false,createdAt:"2024-03-05T10:00:00.000Z"}],[{id:randomUUID(),albumName:"Remote trip",assetIds:[remoteId,sameId]}]);
    const connect2=(apiKey:string,address=fake!.url+"/old")=>fetch(base+"/api/v1/photos/immich/connection",{method:"PUT",headers,body:JSON.stringify({url:address,apiKey,direction:"twoWay",intervalMinutes:60,enabled:true,revision:0})});
    const refusedConnection=await connect2("good-key",`http://127.0.0.1:${await freePort()}`);assert.equal(refusedConnection.status,400);assert.match((await refusedConnection.json() as {error:{message:string}}).error.message,/Nothing accepted the connection/);
    const rejected=await connect2("bad-key");assert.equal(rejected.status,400);assert.match((await rejected.json() as {error:{message:string}}).error.message,/rejected the API key/);
    assert.equal((await connect2("good-key")).status,200);
    assert.equal((await fetch(base+"/api/v1/photos/immich/connection",{method:"PUT",headers,body:JSON.stringify({url:"http://169.254.169.254",apiKey:"good-key",direction:"import",intervalMinutes:60,enabled:true,revision:1})})).status,400);
    const state=async()=>(await (await fetch(base+"/api/v1/photos/immich",{headers})).json() as {connection:{lastRunAt:number;lastError:string;summary:{imported:number;exported:number;albums:number}}}).connection;
    assert.equal((await fetch(base+"/api/v1/photos/immich/connection/sync",{method:"POST",headers})).status,202);
    let status=await state();for(let i=0;i<120&&!status.lastRunAt;i++){await new Promise(r=>setTimeout(r,500));status=await state();}
    assert.equal(status.lastError,"");assert.deepEqual([status.summary.imported,status.summary.albums],[1,1]);
    const page=(await (await fetch(base+"/api/v1/photos/assets",{headers})).json() as PhotoPage).items,imported=page.find(i=>i.name==="remote.jpg")!;
    const importedDetail=await (await fetch(base+`/api/v1/photos/assets/${imported.id}`,{headers})).json() as PhotoDetail;
    assert.equal(importedDetail.favorite,true);assert.equal(importedDetail.description,"from immich");assert.equal(new Date(importedDetail.takenAt).getUTCFullYear(),2023);
    const overview=await (await fetch(base+"/api/v1/photos/overview",{headers})).json() as PhotoOverview;
    assert.equal(overview.albums.find(a=>a.name==="Remote trip")?.count,2);
    // Two-way sends what Immich lacks (solo.jpg); a second run imports and sends nothing new.
    assert.equal(status.summary.exported,1);assert.deepEqual(fake!.received.map(r=>[r.name,r.checksum]),[["solo.jpg",sha1(solo)]]);
    await fetch(base+"/api/v1/photos/immich/connection/sync",{method:"POST",headers});const first=status.lastRunAt;
    for(let i=0;i<120&&status.lastRunAt===first;i++){await new Promise(r=>setTimeout(r,500));status=await state();}
    assert.deepEqual([status.summary.imported,status.summary.exported],[0,0]);assert.equal((await (await fetch(base+"/api/v1/photos/assets",{headers})).json() as PhotoPage).items.filter(i=>i.name.startsWith("remote")).length,1);
    assert.equal((await fetch(base+"/api/v1/photos/immich/connection",{method:"DELETE",headers})).status,200);
  }finally{await fake?.close();await runtime.close();}
});
