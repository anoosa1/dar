import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,readFile} from "node:fs/promises";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {createServer} from "node:net";
import {spawnSync,execFile} from "node:child_process";
import {promisify} from "node:util";
import {startServer} from "../runtime/server.ts";
import {login} from "./integration-helpers.ts";
import type {PhotoAlbum,PhotoAlbumSharing,PhotoDetail,PhotoLink,PhotoOverview,PhotoPage,PhotoPublicShare,PhotoPublicDetail} from "../shared/photos.ts";
import type {SearchResults} from "../shared/search.ts";

const run=promisify(execFile);
async function still(dir:string,source:string,name:string){await run("ffmpeg",["-nostdin","-v","error","-y","-f","lavfi","-i",`${source}=size=320x240`,"-frames:v","1",path.join(dir,name)]);return readFile(path.join(dir,name));}

test("Photos sharing: albums with people (view/add), shared links with password, expiry, download, metadata and uploads",{skip:!process.env.DAR_INTEGRATION_ORIGIN,timeout:240000},async()=>{
  const root=await mkdtemp("/tmp/dar-photos-sharing-"),name="dar-photos-"+randomUUID().slice(0,8),listener=createServer();await new Promise<void>(r=>listener.listen(0,"127.0.0.1",r));const port=(listener.address() as import("node:net").AddressInfo).port;await new Promise<void>(r=>listener.close(()=>r()));const base=`http://127.0.0.1:${port}`;
  const published=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"publish",name,"--js-path",path.resolve(import.meta.dirname,"../module/dist/bundle.js"),"--server",process.env.DAR_DATABASE_URL!,"--yes","--no-config"],{env:{...process.env,DAR_ISSUER:base+"/auth/issuer"},encoding:"utf8",timeout:30000});assert.equal(published.status,0,published.stdout+published.stderr);
  const media={a:await still(root,"testsrc2","a.jpg"),b:await still(root,"smptebars","b.jpg"),m:await still(root,"mandelbrot","m.jpg"),y:await still(root,"rgbtestsrc","y.jpg"),v:await still(root,"testsrc","visitor.jpg")};
  const runtime=await startServer({host:"127.0.0.1",port,origin:base,root:path.join(root,"state"),databaseUrl:process.env.DAR_DATABASE_URL!,databaseName:name,serviceTokenFile:process.env.DAR_DATABASE_TOKEN_FILE!,bootstrapTokenFile:process.env.DAR_BOOTSTRAP_TOKEN_FILE!,development:true});
  try{
    const setup=await fetch(base+"/api/v1/auth/setup",{method:"POST",headers:{origin:base,"content-type":"application/json"},body:JSON.stringify({username:"admin",displayName:"Anas",password:"a long isolated test password",bootstrapToken:await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!,"utf8")})});assert.equal(setup.status,200);
    const admin={...(await login(base)).headers,"content-type":"application/json"};
    for(const [username,displayName] of [["maryam","Maryam"],["yusuf","Yusuf"]])assert.equal((await fetch(base+"/api/v1/auth/users",{method:"POST",headers:admin,body:JSON.stringify({username,displayName,password:"a long isolated test password"})})).status,201);
    const maryam={...(await login(base,"maryam")).headers,"content-type":"application/json"},yusuf={...(await login(base,"yusuf")).headers,"content-type":"application/json"};
    const raw=(route:string,as:Record<string,string>,init:RequestInit={})=>fetch(base+"/api/v1/photos"+route,{...init,headers:{...as,...init.headers}});
    const call=async<T>(route:string,as:Record<string,string>,body?:unknown,method=body===undefined?"GET":"POST"):Promise<T>=>{const response=await raw(route,as,{method,...body!==undefined?{body:JSON.stringify(body)}:{}});assert.ok(response.ok,`${method} ${route}: ${response.status} ${await response.clone().text()}`);return await response.json() as T;};
    async function put(as:Record<string,string>,contents:Buffer,fileName:string){
      const {parentId}=await call<{parentId:string}>("/upload-target",as,{year:2025,month:6});
      const reservation=await fetch(base+"/api/v1/files/uploads",{method:"POST",headers:as,body:JSON.stringify({parentId,name:fileName,size:contents.length,mediaType:"image/jpeg"})});assert.equal(reservation.status,201);
      const transfer=await fetch(`${base}/api/v1/files/uploads/${(await reservation.json()).id}`,{method:"PUT",headers:{cookie:as.cookie,origin:base,"x-csrf-token":as["x-csrf-token"]},body:new Uint8Array(contents)});assert.equal(transfer.status,200);
      const id=(await transfer.json() as {id:string}).id;await call(`/assets/${id}/index`,as,{});return id;
    }
    const albumItems=async(as:Record<string,string>,albumId:string)=>(await call<PhotoPage>(`/assets?view=album&album=${albumId}`,as)).items;
    const a=await put(admin,media.a,"a.jpg"),b=await put(admin,media.b,"b.jpg"),m=await put(maryam,media.m,"m.jpg"),y=await put(yusuf,media.y,"y.jpg");
    const album=await call<PhotoAlbum>("/albums",admin,{name:"Trip",ids:[a,b]});assert.equal(album.role,"owner");assert.equal(album.shared,false);

    // Sharing with people: an editor adds their own photos, a viewer only looks.
    assert.equal((await raw(`/albums/${album.id}/members`,admin,{method:"POST",body:JSON.stringify({username:"nobody",role:"viewer"})})).status,404);
    assert.equal((await raw(`/albums/${album.id}/members`,maryam,{method:"POST",body:JSON.stringify({username:"yusuf",role:"viewer"})})).status,404);
    await call(`/albums/${album.id}/members`,admin,{username:"maryam",role:"editor"});await call(`/albums/${album.id}/members`,admin,{username:"@Yusuf",role:"viewer"});
    const sharing=await call<PhotoAlbumSharing>(`/albums/${album.id}/sharing`,admin);assert.deepEqual(sharing.members.map(x=>[x.username,x.role]),[["maryam","editor"],["yusuf","viewer"]]);
    const seen=await call<PhotoOverview>("/overview",maryam);assert.equal(seen.counts.shared,1);assert.deepEqual(seen.sharedAlbums.map(x=>[x.name,x.role,x.owner?.name]),[["Trip","editor","Anas"]]);
    assert.equal((await call<PhotoOverview>("/overview",admin)).albums[0].shared,true);
    let items=await albumItems(maryam,album.id);assert.deepEqual(items.map(i=>i.id).sort(),[a,b].sort());assert.ok(items.every(i=>i.owner==="Anas"&&!i.favorite));
    assert.equal((await raw(`/assets/${a}/thumbnail`,maryam)).status,200);assert.equal((await raw(`/assets/${a}/original`,maryam)).status,200);
    const theirs=await call<PhotoDetail>(`/assets/${a}`,maryam);assert.equal(theirs.parentId,"");assert.equal(theirs.owner,"Anas");assert.deepEqual(theirs.albums.map(x=>x.id),[album.id]);
    assert.equal((await raw("/assets",maryam,{method:"PATCH",body:JSON.stringify({ids:[a],favorite:true})})).status,404);
    assert.equal((await raw(`/assets/${a}/description`,maryam,{method:"PUT",body:JSON.stringify({description:"x",revision:theirs.revision})})).status,404);
    assert.equal((await raw(`/albums/${album.id}`,maryam,{method:"PATCH",body:JSON.stringify({name:"Mine",revision:1})})).status,403);
    await call(`/albums/${album.id}/add`,maryam,{ids:[m]});
    assert.equal((await raw(`/albums/${album.id}/add`,maryam,{method:"POST",body:JSON.stringify({ids:[y]})})).status,404);
    assert.equal((await raw(`/albums/${album.id}/add`,yusuf,{method:"POST",body:JSON.stringify({ids:[y]})})).status,403);
    items=await albumItems(admin,album.id);assert.equal(items.find(i=>i.id===m)?.owner,"Maryam");assert.equal((await raw(`/assets/${m}/thumbnail`,admin)).status,200);
    assert.equal((await albumItems(yusuf,album.id)).length,3);
    assert.equal((await raw(`/albums/${album.id}/remove`,maryam,{method:"POST",body:JSON.stringify({ids:[a]})})).status,403);
    assert.equal((await raw(`/albums/${album.id}/remove`,yusuf,{method:"POST",body:JSON.stringify({ids:[m]})})).status,403);
    // Search finds the shared album; a trashed photo leaves the shared view.
    const found=await (await fetch(`${base}/api/v1/search?q=trip`,{headers:yusuf})).json() as SearchResults;assert.ok(found.items.some(i=>i.href===`/photos?album=${album.id}`));
    await call("/assets/trash",admin,{ids:[b]});assert.ok(!(await albumItems(maryam,album.id)).some(i=>i.id===b));assert.equal((await raw(`/assets/${b}/thumbnail`,maryam)).status,404);
    await call("/assets/restore",admin,{ids:[b]});
    // Removing a person, and leaving.
    await call(`/albums/${album.id}/members/${(await call<PhotoAlbumSharing>(`/albums/${album.id}/sharing`,admin)).members.find(x=>x.username==="yusuf")!.userId}`,admin,undefined,"DELETE");
    assert.equal((await raw(`/assets/${a}/thumbnail`,yusuf)).status,404);assert.equal((await raw(`/assets?view=album&album=${album.id}`,yusuf)).status,404);
    const maryamId=(await call<PhotoAlbumSharing>(`/albums/${album.id}/sharing`,admin)).members[0].userId;

    // Shared links: the token is in the URL only; the owner can copy it again.
    const link=await call<PhotoLink>("/links",admin,{albumId:album.id,expiresAt:0,allowDownload:true,showMetadata:true});
    const token=/\/s\/([A-Za-z0-9_-]{43})$/.exec(link.url)![1];assert.equal(link.url,`${base}/s/${token}`);
    assert.equal((await call<{links:PhotoLink[]}>("/links",admin)).links[0].url,link.url);
    assert.equal((await fetch(`${base}/s/${token}`)).status,200);
    const visitor=(route="",init:RequestInit={})=>fetch(`${base}/public/photos/${token}${route}`,{...init,headers:{origin:base,"content-type":"application/json",...init.headers}});
    let info=await (await visitor()).json() as PhotoPublicShare;assert.equal(info.needsPassword,false);
    if(!info.needsPassword){assert.equal(info.title,"Trip");assert.equal(info.owner,"Anas");assert.equal(info.count,3);assert.equal(info.allowUpload,false);}
    const publicItems=(await (await visitor("/assets")).json() as PhotoPage).items;assert.equal(publicItems.length,3);assert.ok(publicItems.every(i=>!i.owner));
    assert.equal((await visitor(`/assets/${a}/thumbnail`)).status,200);assert.equal((await visitor(`/assets/${y}/thumbnail`)).status,404);
    const original=await visitor(`/assets/${m}/original`);assert.equal(original.status,200);assert.equal(Buffer.from(await original.arrayBuffer()).length,media.m.length);
    assert.ok((await (await visitor(`/assets/${a}`)).json() as PhotoPublicDetail).takenAt);
    let edited=await call<PhotoLink>(`/links/${link.id}`,admin,{revision:link.revision,allowDownload:false,showMetadata:false},"PATCH");
    assert.equal((await visitor(`/assets/${a}/original`)).status,403);assert.equal((await (await visitor(`/assets/${a}`)).json() as PhotoPublicDetail).takenAt,undefined);
    assert.equal((await raw(`/links/${link.id}`,admin,{method:"PATCH",body:JSON.stringify({revision:link.revision,allowDownload:true})})).status,409);
    assert.equal((await raw(`/links/${link.id}`,admin,{method:"PATCH",body:JSON.stringify({revision:edited.revision,expiresAt:Date.now()-1000})})).status,400);
    assert.equal((await raw(`/links/${link.id}`,admin,{method:"PATCH",body:JSON.stringify({revision:edited.revision,password:"abc"})})).status,400);

    // A password: visitors see nothing until they enter it, then a path-scoped pass cookie opens the link.
    edited=await call<PhotoLink>(`/links/${link.id}`,admin,{revision:edited.revision,password:"open sesame"},"PATCH");assert.equal(edited.hasPassword,true);
    assert.deepEqual(await (await visitor()).json(),{needsPassword:true});assert.equal((await visitor("/assets")).status,403);assert.equal((await visitor(`/assets/${a}/thumbnail`)).status,403);
    assert.equal((await visitor("/password",{method:"POST",body:JSON.stringify({password:"wrong"})})).status,403);
    assert.equal((await fetch(`${base}/public/photos/${token}/password`,{method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify({password:"open sesame"})})).status,403);
    const opened=await visitor("/password",{method:"POST",body:JSON.stringify({password:"open sesame"})});assert.equal(opened.status,200);
    const pass=opened.headers.get("set-cookie")!;assert.match(pass,new RegExp(`^dar_link_pass=[A-Za-z0-9_-]{43}; Path=/public/photos/${token}; HttpOnly; SameSite=Strict; Max-Age=43200$`));
    const withPass={cookie:pass.split(";")[0]};
    info=await (await visitor("",{headers:withPass})).json() as PhotoPublicShare;assert.equal(info.needsPassword,false);assert.equal((await visitor("/assets",{headers:withPass})).status,200);
    // Changing the password signs visitors out.
    edited=await call<PhotoLink>(`/links/${link.id}`,admin,{revision:edited.revision,password:"another secret"},"PATCH");assert.equal((await visitor("/assets",{headers:withPass})).status,403);
    edited=await call<PhotoLink>(`/links/${link.id}`,admin,{revision:edited.revision,password:null,allowUpload:true},"PATCH");assert.equal(edited.hasPassword,false);

    // Visitor uploads land in the owner's Photos folder and the album, with a free name.
    for(const attempt of [1,2]){
      const begin=await visitor("/uploads",{method:"POST",body:JSON.stringify({name:"visitor.jpg",size:media.v.length,mediaType:"image/jpeg",year:2025,month:7})});assert.equal(begin.status,201,await begin.clone().text());
      const sent=await visitor(`/uploads/${(await begin.json() as {id:string}).id}`,{method:"PUT",headers:{"content-type":"application/octet-stream"},body:new Uint8Array(media.v)});assert.equal(sent.status,200,await sent.clone().text());void attempt;
    }
    items=await albumItems(admin,album.id);const uploaded=items.filter(i=>i.name.startsWith("visitor"));assert.deepEqual(uploaded.map(i=>i.name).sort(),["visitor (2).jpg","visitor.jpg"]);
    assert.ok(uploaded.every(i=>!i.owner));assert.equal((await call<PhotoDetail>(`/assets/${uploaded[0].id}`,admin)).width,320);
    assert.equal((await visitor("/uploads",{method:"POST",body:JSON.stringify({name:"notes.txt",size:3,mediaType:"text/plain",year:2025,month:7})})).status,400);

    // Links to chosen photos; no uploads there; removing a link ends it.
    const chosen=await call<PhotoLink>("/links",admin,{ids:[b]});assert.equal(chosen.count,1);assert.equal(chosen.albumName,"");
    assert.equal((await raw("/links",admin,{method:"POST",body:JSON.stringify({ids:[b],allowUpload:true})})).status,400);
    assert.equal((await raw("/links",maryam,{method:"POST",body:JSON.stringify({ids:[a]})})).status,404);
    const chosenToken=/\/s\/(.+)$/.exec(chosen.url)![1];
    assert.deepEqual(((await (await fetch(`${base}/public/photos/${chosenToken}/assets`)).json()) as PhotoPage).items.map(i=>i.id),[b]);
    await call(`/links/${chosen.id}`,admin,undefined,"DELETE");assert.equal((await fetch(`${base}/public/photos/${chosenToken}`)).status,404);
    assert.equal((await fetch(`${base}/public/photos/${"x".repeat(43)}`)).status,404);

    // Leaving; deleting the album ends its links.
    await call(`/albums/${album.id}/members/${maryamId}`,maryam,undefined,"DELETE");assert.equal((await call<PhotoOverview>("/overview",maryam)).sharedAlbums.length,0);
    await call(`/albums/${album.id}`,admin,undefined,"DELETE");assert.equal((await visitor()).status,404);
  }finally{await runtime.close();}
});
