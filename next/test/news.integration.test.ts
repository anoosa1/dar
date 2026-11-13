import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,readFile} from "node:fs/promises";
import path from "node:path";
import http from "node:http";
import {randomUUID} from "node:crypto";
import {createServer} from "node:net";
import {spawnSync,execFile} from "node:child_process";
import {promisify} from "node:util";
import {startServer} from "../runtime/server.ts";
import {login} from "./integration-helpers.ts";
import type {NewsOverview,NewsFeed,NewsItem,NewsItemDetail,NewsDiscovered,NewsFolder} from "../shared/news.ts";

/** Local feed fixture. Feeds change on demand; ETags let refreshes answer 304. */
function feedServer(){
  const state={extra:false,failing:false,conditional:0,requests:0};
  const rss=(base:string)=>`<?xml version="1.0"?><rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/"><channel><title>Fixture News</title><link>${base}/</link>
    <item><title>Welcome to the fixture</title><link>${base}/posts/1</link><guid>post-1</guid><pubDate>Mon, 09 Nov 2026 08:00:00 GMT</pubDate><description>Short summary</description><content:encoded><![CDATA[<p>Hello <b>reader</b><script>alert(1)</script><img src="/pixel.png" onerror="x()"></p>]]></content:encoded></item>
    <item><title>Episode one</title><link>${base}/posts/2</link><guid>post-2</guid><pubDate>Tue, 10 Nov 2026 08:00:00 GMT</pubDate><description>Listen</description><enclosure url="https://media.example/ep1.mp3" type="audio/mpeg" length="1"/></item>
    ${state.extra?`<item><title>Fresh arrival</title><link>${base}/posts/3</link><guid>post-3</guid><pubDate>Wed, 11 Nov 2026 08:00:00 GMT</pubDate><description>New</description></item>`:""}</channel></rss>`;
  const atom=(base:string)=>`<feed xmlns="http://www.w3.org/2005/Atom"><title>Atom Blog</title><link href="${base}/blog"/><entry><id>tag:blog,1</id><title>Long read</title><link href="${base}/article"/><updated>2026-11-08T09:00:00Z</updated><summary>Only a teaser</summary></entry></feed>`;
  const server=http.createServer((req,res)=>{
    state.requests++;const base=`http://${req.headers.host}`;
    if(req.url==="/feed.xml"){
      if(state.failing){res.writeHead(500);res.end();return;}
      const etag=`"v${state.extra?2:1}"`;if(req.headers["if-none-match"]===etag){state.conditional++;res.writeHead(304,{etag});res.end();return;}
      res.writeHead(200,{"content-type":"application/rss+xml",etag});res.end(rss(base));return;
    }
    if(req.url==="/atom.xml"){res.writeHead(200,{"content-type":"application/atom+xml"});res.end(atom(base));return;}
    if(req.url==="/"){res.writeHead(200,{"content-type":"text/html"});res.end(`<html><head><link rel="alternate" type="application/rss+xml" title="Fixture News" href="/feed.xml"></head><body>Home</body></html>`);return;}
    if(req.url==="/article"){res.writeHead(200,{"content-type":"text/html; charset=utf-8"});res.end(`<html><body><nav>${"Menu item. ".repeat(40)}</nav><article><h1>Long read</h1>${Array.from({length:6},(_,i)=>`<p>Full article paragraph ${i} with enough words to be counted as real content by the extractor.</p>`).join("")}</article></body></html>`);return;}
    if(req.url==="/pixel.png"){res.writeHead(200,{"content-type":"image/png"});res.end(Buffer.from("89504e470d0a1a0a0000000d49484452","hex"));return;}
    if(req.url==="/vector.svg"){res.writeHead(200,{"content-type":"image/svg+xml"});res.end("<svg onload='x()'/>");return;}
    res.writeHead(404);res.end();
  });
  return {server,state};
}

test("News: subscribe, refresh, state, folders, OPML, proxy, isolation and the Nextcloud News API",{skip:!process.env.DAR_INTEGRATION_ORIGIN,timeout:180000},async()=>{
  const root=await mkdtemp("/tmp/dar-news-acceptance-"),name="dar-news-"+randomUUID().slice(0,8),listener=createServer();await new Promise<void>(r=>listener.listen(0,"127.0.0.1",r));const port=(listener.address() as import("node:net").AddressInfo).port;await new Promise<void>(r=>listener.close(()=>r()));const base=`http://127.0.0.1:${port}`;
  const published=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"publish",name,"--js-path",path.resolve(import.meta.dirname,"../module/dist/bundle.js"),"--server",process.env.DAR_DATABASE_URL!,"--yes","--no-config"],{env:{...process.env,DAR_ISSUER:base+"/auth/issuer"},encoding:"utf8",timeout:30000});assert.equal(published.status,0,published.stdout+published.stderr);
  const fixture=feedServer();await new Promise<void>(r=>fixture.server.listen(0,"127.0.0.1",r));const site=`http://127.0.0.1:${(fixture.server.address() as import("node:net").AddressInfo).port}`;
  const config={host:"127.0.0.1",port,origin:base,root:path.join(root,"state"),databaseUrl:process.env.DAR_DATABASE_URL!,databaseName:name,serviceTokenFile:process.env.DAR_DATABASE_TOKEN_FILE!,bootstrapTokenFile:process.env.DAR_BOOTSTRAP_TOKEN_FILE!,development:true,newsAllowPrivateNetworks:true};
  const runtime=await startServer(config);
  try{
    const setup=await fetch(base+"/api/v1/auth/setup",{method:"POST",headers:{origin:base,"content-type":"application/json"},body:JSON.stringify({username:"admin",displayName:"News fixture",password:"a long isolated test password",bootstrapToken:await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!,"utf8")})});assert.equal(setup.status,200);
    const session=await login(base),headers={...session.headers,"content-type":"application/json"};
    const raw=(route:string,init:RequestInit={})=>fetch(base+"/api/v1/news"+route,{...init,headers:{...headers,...init.headers}});
    const call=async<T>(route:string,body?:unknown,method=body===undefined?"GET":"POST"):Promise<T>=>{const response=await raw(route,{method,...body!==undefined?{body:JSON.stringify(body)}:{}});assert.ok(response.ok,`${method} ${route}: ${response.status} ${await response.clone().text()}`);return response.json() as Promise<T>;};
    const overview=()=>call<NewsOverview>("/overview");
    assert.deepEqual(await overview(),{folders:[],feeds:[],starredCount:0,unreadCount:0,newestItemId:0});
    // Discovery from a site's alternate link and directly from a feed.
    const found=await call<{items:NewsDiscovered[]}>("/discover",{url:site+"/"});assert.deepEqual(found.items.map(f=>[f.url,f.title,f.kind]),[[site+"/feed.xml","Fixture News","RSS"]]);assert.equal(found.items[0].items[0].title,"Episode one");
    assert.equal((await raw("/discover",{method:"POST",body:JSON.stringify({url:site+"/missing"})})).status,502);
    const feed=await call<NewsFeed>("/feeds",{url:site+"/feed.xml",folderId:0});assert.equal(feed.title,"Fixture News");assert.equal(feed.unreadCount,2);
    const duplicate=await raw("/feeds",{method:"POST",body:JSON.stringify({url:site+"/feed.xml",folderId:0})});assert.equal(duplicate.status,409);assert.match((await duplicate.json()).error.message,/already subscribed/);
    let page=await call<{items:NewsItem[]}>("/items?type=unread");assert.deepEqual(page.items.map(i=>i.title),["Episode one","Welcome to the fixture"]);
    const [episode,welcome]=page.items;assert.equal(episode.enclosureLink,"https://media.example/ep1.mp3");
    const detail=await call<NewsItemDetail>(`/items/${welcome.id}`);assert.equal(detail.body,`<p>Hello <b>reader</b><img src="${site}/pixel.png"></p>`);assert.equal(detail.url,site+"/posts/1");
    // Read and star state, counts and mark-all-read with the newest loaded ID.
    await call("/items/state",{ids:[welcome.id],unread:false,starred:true});
    let o=await overview();assert.equal(o.unreadCount,1);assert.equal(o.starredCount,1);assert.equal(o.feeds[0].unreadCount,1);
    assert.deepEqual((await call<{items:NewsItem[]}>("/items?type=starred")).items.map(i=>i.id),[welcome.id]);
    const marked=await call<{changed:number;ids:number[]}>("/read",{type:"all",newestItemId:o.newestItemId});assert.deepEqual(marked.ids,[episode.id]);assert.equal((await overview()).unreadCount,0);
    await call("/items/state",{ids:marked.ids,unread:true});assert.equal((await overview()).unreadCount,1);
    await call(`/items/${episode.id}/position`,{position:93});assert.equal((await call<NewsItemDetail>(`/items/${episode.id}`)).position,93);
    // Refresh: a 304 keeps everything; new entries arrive unread; failures are recorded and back off.
    await call(`/feeds/${feed.id}`,{updateNow:true},"PATCH");assert.equal(fixture.state.conditional,1);
    fixture.state.extra=true;let info=await call<NewsFeed>(`/feeds/${feed.id}`,{updateNow:true},"PATCH");assert.equal(info.unreadCount,2);
    assert.equal((await call<{items:NewsItem[]}>("/items?type=unread")).items[0].title,"Fresh arrival");assert.equal((await call<{items:NewsItem[]}>(`/items?type=all`)).items.length,3);
    fixture.state.failing=true;info=await call<NewsFeed>(`/feeds/${feed.id}`,{updateNow:true},"PATCH");assert.equal(info.updateErrorCount,1);assert.match(info.lastUpdateError,/500/);assert.ok(info.nextUpdate>Date.now());
    fixture.state.failing=false;info=await call<NewsFeed>(`/feeds/${feed.id}`,{updateNow:true},"PATCH");assert.equal(info.updateErrorCount,0);
    // Folders, moving, renaming and duplicate names.
    const folder=await call<NewsFolder>("/folders",{name:"Technology"});assert.equal((await raw("/folders",{method:"POST",body:JSON.stringify({name:"technology"})})).status,409);
    await call(`/feeds/${feed.id}`,{folderId:folder.id,title:"Renamed fixture"},"PATCH");o=await overview();assert.equal(o.feeds[0].folderId,folder.id);assert.equal(o.feeds[0].title,"Renamed fixture");
    assert.equal((await call<{items:NewsItem[]}>(`/items?type=folder&id=${folder.id}&getRead=1`)).items.length,3);
    await call(`/folders/${folder.id}`,{name:"Tech",opened:false},"PATCH");assert.deepEqual((await overview()).folders,[{id:folder.id,name:"Tech",opened:false}]);
    // Full text replaces a teaser with the article page's main content.
    const atom=await call<NewsFeed>("/feeds",{url:site+"/atom.xml",folderId:folder.id,fullText:true});
    const long=(await call<{items:NewsItem[]}>(`/items?type=feed&id=${atom.id}`)).items[0],longDetail=await call<NewsItemDetail>(`/items/${long.id}`);
    assert.match(longDetail.body,/^<h1>Long read<\/h1><p>Full article paragraph 0/);assert.ok(!longDetail.body.includes("Menu item"));
    // OPML round trip.
    const opml=await raw("/opml");assert.equal(opml.headers.get("content-disposition"),'attachment; filename="subscriptions.opml"');const exported=await opml.text();assert.match(exported,/<outline text="Tech" title="Tech"><outline type="rss" text="Renamed fixture"/);
    const imported=await call<{added:number;skipped:number;folders:number}>("/opml",{text:`<opml version="2.0"><body><outline text="Blogs"><outline text="Atom again" xmlUrl="${site}/atom.xml"/><outline text="Elsewhere" xmlUrl="${site}/elsewhere.xml"/></outline></body></opml>`});
    assert.deepEqual(imported,{added:1,skipped:1,folders:1});
    // The image proxy serves only raster images, inert.
    const image=await raw(`/image?url=${encodeURIComponent(site+"/pixel.png")}`);assert.equal(image.status,200);assert.equal(image.headers.get("content-type"),"image/png");assert.equal(image.headers.get("content-security-policy"),"default-src 'none'; sandbox");
    assert.equal((await raw(`/image?url=${encodeURIComponent(site+"/vector.svg")}`)).status,404);
    // Preferences are validated; search finds articles.
    const prefs=await (await fetch(base+"/api/v1/pim/preferences",{headers})).json() as {revision:number;newsRefresh?:number};assert.equal(prefs.newsRefresh??30,30);
    assert.equal((await fetch(base+"/api/v1/pim/preferences",{method:"PUT",headers:{...headers,"if-match":`"preferences:${prefs.revision}"`},body:JSON.stringify({newsRefresh:7})})).status,400);
    assert.equal((await fetch(base+"/api/v1/pim/preferences",{method:"PUT",headers:{...headers,"if-match":`"preferences:${prefs.revision}"`},body:JSON.stringify({newsRefresh:60,newsMarkRead:"scroll"})})).status,200);
    const search=await (await fetch(base+"/api/v1/search?q=fresh",{headers})).json() as {items:{group:string;title:string;href:string}[]};assert.deepEqual(search.items.filter(i=>i.group==="News").map(i=>i.title),["Fresh arrival"]);

    // Nextcloud News API with app passwords.
    const password=async(scope:string)=>(await (await fetch(base+"/api/v1/auth/app-passwords",{method:"POST",headers,body:JSON.stringify({label:"Phone",scope})})).json()) as {secret:string;newsUrl?:string};
    const writer=await password("v2 news:read news:write"),reader=await password("v2 news:read"),filesOnly=await password("v2 files:read");
    assert.equal(writer.newsUrl,`${base}/dav/news`);
    const nc=(secret:string|undefined,route:string,init:RequestInit={})=>fetch(`${base}/dav/news${route}`,{...init,headers:{...secret?{authorization:"Basic "+Buffer.from("admin:"+secret).toString("base64")}:{},...init.body?{"content-type":"application/json"}:{},...init.headers}});
    const api="/index.php/apps/news/api/v1-3";
    assert.equal((await (await nc(undefined,"/status.php")).json()).productname,"Nextcloud");
    const unauthenticated=await nc(undefined,api+"/feeds");assert.equal(unauthenticated.status,401);assert.match(unauthenticated.headers.get("www-authenticate")??"",/Basic/);
    assert.equal((await nc(filesOnly.secret,api+"/feeds")).status,403);
    assert.deepEqual(await (await nc(reader.secret,api+"/version")).json(),{version:"24.0.0"});
    const user=await (await nc(reader.secret,"/ocs/v2.php/cloud/user?format=json")).json() as {ocs:{data:{id:string}}};assert.equal(user.ocs.data.id,"admin");
    const ncFeeds=await (await nc(reader.secret,api+"/feeds")).json() as {feeds:{id:number;title:string;folderId:number|null;unreadCount:number;added:number}[];starredCount:number;newestItemId:number};
    assert.equal(ncFeeds.feeds.length,3);assert.equal(ncFeeds.starredCount,1);assert.ok(ncFeeds.newestItemId>0);assert.ok(ncFeeds.feeds.every(f=>f.added<1e11));
    assert.ok(ncFeeds.feeds.every(f=>typeof f.folderId==="number"&&f.folderId>0));
    const ncFolders=await (await nc(reader.secret,api+"/folders")).json() as {folders:{id:number;name:string}[]};assert.deepEqual(ncFolders.folders.map(f=>f.name).sort(),["Blogs","Tech"]);
    const ncItems=await (await nc(reader.secret,api+"/items?type=3&batchSize=2&getRead=true")).json() as {items:{id:number;title:string;body:string;guidHash:string;feedId:number;unread:boolean;starred:boolean;lastModified:number;pubDate:number}[]};
    assert.equal(ncItems.items.length,2);assert.ok(ncItems.items[0].id>ncItems.items[1].id);assert.ok(ncItems.items.every(i=>i.pubDate<1e11&&i.lastModified<1e11));
    const older=await (await nc(reader.secret,`${api}/items?type=3&batchSize=10&offset=${ncItems.items[1].id}`)).json() as {items:{id:number}[]};assert.ok(older.items.length>=1&&older.items.every(i=>i.id<ncItems.items[1].id));
    assert.equal((await nc(reader.secret,`${api}/items/${ncItems.items[0].id}/read`,{method:"POST"})).status,403);
    const since=Math.floor(Date.now()/1000)-1;
    assert.equal((await nc(writer.secret,`${api}/items/read/multiple`,{method:"POST",body:JSON.stringify({itemIds:ncItems.items.map(i=>i.id)})})).status,200);
    const target=ncItems.items[0];assert.equal((await nc(writer.secret,`/index.php/apps/news/api/v1-2/items/${target.feedId}/${target.guidHash}/star`,{method:"PUT"})).status,200);
    const updated=await (await nc(reader.secret,`${api}/items/updated?type=3&lastModified=${since}`)).json() as {items:{id:number;unread:boolean;starred:boolean}[]};
    assert.ok(updated.items.some(i=>i.id===target.id&&!i.unread&&i.starred));
    assert.equal((await nc(writer.secret,api+"/folders",{method:"POST",body:JSON.stringify({name:"Tech"})})).status,409);
    const created=await (await nc(writer.secret,api+"/folders",{method:"POST",body:JSON.stringify({name:"Podcasts"})})).json() as {folders:{id:number}[]};
    assert.equal((await nc(writer.secret,`${api}/feeds/${feed.id}/move`,{method:"POST",body:JSON.stringify({folderId:created.folders[0].id})})).status,200);
    assert.equal((await nc(writer.secret,`${api}/feeds/${feed.id}/rename`,{method:"POST",body:JSON.stringify({feedTitle:"Via app"})})).status,200);
    o=await overview();assert.equal(o.feeds.find(f=>f.id===feed.id)!.title,"Via app");assert.equal(o.feeds.find(f=>f.id===feed.id)!.folderId,created.folders[0].id);
    assert.equal((await nc(writer.secret,api+"/feeds",{method:"POST",body:JSON.stringify({url:site+"/feed.xml",folderId:0})})).status,409);
    assert.equal((await nc(writer.secret,api+"/items/read",{method:"POST",body:JSON.stringify({newestItemId:o.newestItemId})})).status,200);assert.equal((await overview()).unreadCount,0);

    // Another account cannot reach these feeds; disabling News hides both APIs.
    assert.equal((await fetch(base+"/api/v1/auth/users",{method:"POST",headers,body:JSON.stringify({username:"other",displayName:"Other",password:"a long isolated test password"})})).status,201);
    const other=await login(base,"other"),otherHeaders={...other.headers,"content-type":"application/json"};
    assert.equal((await fetch(`${base}/api/v1/news/feeds/${feed.id}`,{headers:otherHeaders})).status,404);
    assert.equal((await fetch(`${base}/api/v1/news/items/${welcome.id}`,{headers:otherHeaders})).status,404);
    assert.equal((await fetch(`${base}/api/v1/news/items/state`,{method:"POST",headers:otherHeaders,body:JSON.stringify({ids:[welcome.id],unread:true})}).then(r=>r.json())).items.length,0);
    const apps=await (await fetch(base+"/api/v1/appSettings",{headers})).json() as {revision:number};
    assert.equal((await fetch(base+"/api/v1/admin/installedApps",{method:"PUT",headers:{...headers,"if-match":`"apps:${apps.revision}"`},body:JSON.stringify({id:"news",enabled:false,revision:apps.revision})})).status,200);
    assert.equal((await raw("/overview")).status,404);assert.equal((await nc(reader.secret,api+"/feeds")).status,404);
    const enabled=await (await fetch(base+"/api/v1/appSettings",{headers})).json() as {revision:number};
    assert.equal((await fetch(base+"/api/v1/admin/installedApps",{method:"PUT",headers:{...headers,"if-match":`"apps:${enabled.revision}"`},body:JSON.stringify({id:"news",enabled:true,revision:enabled.revision})})).status,200);
    // Unsubscribing removes the feed's articles.
    await call(`/feeds/${atom.id}`,undefined,"DELETE");assert.equal((await raw(`/items/${long.id}`)).status,404);

    const browser=await promisify(execFile)(process.execPath,["test/news-browser.mjs"],{cwd:path.resolve(import.meta.dirname,".."),env:{...process.env,DAR_INTEGRATION_ORIGIN:base,DAR_TEST_ROOT:root,DAR_NEWS_SITE:site},encoding:"utf8",timeout:90000});console.log(browser.stdout.trim());
  }finally{await runtime.close();fixture.server.close();}
});
