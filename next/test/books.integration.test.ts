import assert from "node:assert/strict";
import {test} from "node:test";
import {randomUUID} from "node:crypto";
import {spawnSync} from "node:child_process";
import {mkdtemp,mkdir,writeFile,readFile,rm} from "node:fs/promises";
import {tmpdir} from "node:os";
import path from "node:path";
import {connect,login,upload} from "./integration-helpers.ts";
import {epub,cbz,pdf,png} from "./books-fixtures.ts";
import type {BookDetail,BookLibrary,BookMark,BookState} from "../shared/books.ts";
const origin=process.env.DAR_INTEGRATION_ORIGIN;

test("books: library, metadata, covers, readers' content, reading state, bookmarks, access and app toggle",{skip:!origin,timeout:120_000},async()=>{
  const base=origin!,session=await login(base),headers={...session.headers,"content-type":"application/json"};
  const get=(route:string,who=session)=>fetch(`${base}/api/v1/books${route}`,{headers:who.headers});
  const json=async<T,>(route:string,who=session)=>{const response=await get(route,who);assert.equal(response.status,200,`${route}: ${await response.clone().text()}`);return await response.json() as T;};
  const library=async()=>{for(let i=0;i<40;i++){const value=await json<BookLibrary>("/library");if(!value.indexing)return value;await new Promise(r=>setTimeout(r,500));}throw new Error("Books never finished reading the library.");};
  const db=await connect(session);
  try{
    const first=await json<BookLibrary>("/library");
    assert.equal(first.path,"docs/books","the default folder is docs/books");
    const rootId=first.rootId;
    const comicsId=randomUUID();await db.reducers.createFolder({id:comicsId,parentId:rootId,name:"Comics"});
    const voyage=await upload(base,session,"the-test-voyage.epub",epub({title:"The Test Voyage",author:"Ishmael Fixture",series:["Fixture Series",2]}),rootId);
    const locked=await upload(base,session,"locked.epub",epub({title:"Locked Away",author:"Store",drm:true}),rootId);
    const comic=await upload(base,session,"fixture-comic.cbz",cbz(),comicsId);
    const paper=await upload(base,session,"paper.pdf",pdf(),rootId);
    await upload(base,session,"old-format.mobi","not a book we open",rootId);
    // A tar comic goes through bsdtar, like CBR and CB7.
    const work=await mkdtemp(path.join(tmpdir(),"dar-books-"));
    try{
      await mkdir(path.join(work,"pages"));for(const [n,c] of [[1,[10,10,10]],[2,[90,90,90]]] as const)await writeFile(path.join(work,"pages",`${n}.png`),png(30,45,c as [number,number,number]));
      const made=spawnSync("bsdtar",["-cf",path.join(work,"comic.cbt"),"-C",work,"pages"]);assert.equal(made.status,0,String(made.stderr));
      await upload(base,session,"tar-comic.cbt",await readFile(path.join(work,"comic.cbt")),comicsId);
    }finally{await rm(work,{recursive:true,force:true});}

    const value=await library(),find=(name:string)=>value.books.find(b=>b.name===name)!;
    assert.deepEqual(value.books.map(b=>b.name).sort(),["fixture-comic.cbz","locked.epub","paper.pdf","tar-comic.cbt","the-test-voyage.epub"],"only book formats are listed");
    assert.deepEqual(value.libraries.map(l=>[l.name,l.count]),[["Comics",2]],"top folders are libraries");
    const book=find("the-test-voyage.epub");
    assert.equal(book.title,"The Test Voyage");assert.deepEqual(book.authors,["Ishmael Fixture"]);assert.equal(book.series,"Fixture Series");assert.equal(book.seriesIndex,2);
    assert.equal(book.language,"en");assert.equal(book.publisher,"Fixture Press");assert.equal(book.description,"A test voyage.");assert.deepEqual(book.subjects,["Sea stories"]);assert.equal(book.cover,true);assert.equal(book.error,"");
    assert.equal(find("locked.epub").error,"drm","an encrypted EPUB is reported as DRM");
    const comicBook=find("fixture-comic.cbz");assert.equal(comicBook.pages,3);assert.equal(comicBook.rightToLeft,true);assert.deepEqual(comicBook.authors,["Ada Panel"]);assert.equal(comicBook.series,"Fixture Comics");assert.equal(comicBook.library,comicsId);
    assert.equal(find("tar-comic.cbt").pages,2,"bsdtar unpacks tar comics");
    const paperBook=find("paper.pdf");assert.equal(paperBook.pages,2);assert.equal(paperBook.title,"A Fixture Paper");assert.deepEqual(paperBook.authors,["Grace Columns"]);

    for(const id of [voyage.fileId,comic.fileId,paper.fileId]){const cover=await get(`/${id}/cover`);assert.equal(cover.status,200,`cover for ${id}`);assert.equal(cover.headers.get("content-type"),"image/webp");assert.ok((await cover.arrayBuffer()).byteLength>20);}

    // EPUB: manifest, chapters with a strict policy, no way out of the archive.
    const detail=await json<BookDetail>(`/${voyage.fileId}`);
    assert.equal(detail.manifest.kind,"epub");if(detail.manifest.kind!=="epub")return;
    assert.deepEqual(detail.manifest.spine.map(s=>s.href),["OEBPS/chapter1.xhtml","OEBPS/chapter2.xhtml"]);
    assert.deepEqual(detail.manifest.toc.map(t=>[t.label,t.spine,t.depth]),[["One: Departure",0,0],["Two: The Storm",1,0],["The mast breaks",1,1]]);
    assert.equal(detail.manifest.toc[2].href,"OEBPS/chapter2.xhtml#p3");
    const chapter=await get(`/${voyage.fileId}/epub/r1/OEBPS/chapter1.xhtml`);assert.equal(chapter.status,200);
    assert.equal(chapter.headers.get("content-type"),"application/xhtml+xml");
    const policy=chapter.headers.get("content-security-policy")??"";assert.match(policy,/script-src 'none'/);assert.match(policy,/form-action 'none'/);assert.match(policy,/frame-ancestors 'self'/);
    assert.match(await chapter.text(),/Paragraph 1 of Departure/);
    assert.equal((await get(`/${voyage.fileId}/epub/r1/OEBPS/chapter1.xhtml?as=html`)).headers.get("content-type"),"text/html; charset=utf-8");
    for(const bad of ["OEBPS/../../../etc/passwd","OEBPS%2F..%2F..%2Fsecret","..%2FMETA-INF%2Fcontainer.xml","OEBPS/missing.xhtml"])assert.equal((await get(`/${voyage.fileId}/epub/r1/${bad}`)).status,404,bad);
    assert.equal((await get(`/${locked.fileId}/epub/r1/OEBPS/chapter1.xhtml`)).status,200,"DRM is reported, not enforced by refusing bytes the owner already has");
    // Comics and PDF.
    const page=await get(`/${comic.fileId}/pages/0`);assert.equal(page.status,200);assert.equal(page.headers.get("content-type"),"image/png");
    assert.deepEqual([...new Uint8Array(await page.arrayBuffer()).slice(0,4)],[137,80,78,71]);
    assert.equal((await get(`/${comic.fileId}/pages/3`)).status,404);assert.equal((await get(`/${paper.fileId}/pages/0`)).status,404);
    assert.equal((await json<BookDetail>(`/${comic.fileId}`)).manifest.kind,"comic");assert.equal((await json<BookDetail>(`/${paper.fileId}`)).manifest.kind,"pdf");

    // Reading state: revisions, conflicts carry the newer state, lists.
    const put=(id:string,body:object)=>fetch(`${base}/api/v1/books/${id}/state`,{method:"PUT",headers,body:JSON.stringify(body)});
    let response=await put(voyage.fileId,{revision:0,status:"reading",position:{spine:1,fraction:0.5},progress:0.6,device:"Test browser"});assert.equal(response.status,200,await response.clone().text());
    const saved=await response.json() as BookState;assert.equal(saved.revision,1);assert.deepEqual(saved.position,{spine:1,fraction:0.5});
    response=await put(voyage.fileId,{revision:0,position:{spine:0,fraction:0.1},progress:0.05});assert.equal(response.status,409,"a stale place never overwrites a newer one");
    assert.equal(((await response.json()) as {state:BookState}).state.progress,0.6,"the conflict returns the newer place");
    for(const bad of [{revision:1,status:"maybe"},{revision:1,position:{spine:1,page:2,fraction:0}},{revision:1,progress:2},{revision:1,extra:true}])assert.equal((await put(voyage.fileId,bad)).status,400,JSON.stringify(bad));
    assert.equal((await put(paper.fileId,{revision:0,status:"want"})).status,200);
    assert.equal((await put(randomUUID(),{revision:0,status:"want"})).status,404);
    const listed=await library();assert.equal(listed.books.find(b=>b.id===voyage.fileId)!.state!.status,"reading");assert.equal(listed.books.find(b=>b.id===paper.fileId)!.state!.status,"want");

    // Bookmarks.
    const markId=randomUUID(),mark=await fetch(`${base}/api/v1/books/${voyage.fileId}/marks`,{method:"POST",headers,body:JSON.stringify({id:markId,position:{spine:1,fraction:0.25},label:"The storm"})});
    assert.equal(mark.status,201,await mark.clone().text());assert.equal((await mark.json() as BookMark).label,"The storm");
    assert.equal((await json<BookDetail>(`/${voyage.fileId}`)).marks.length,1);
    assert.equal((await fetch(`${base}/api/v1/books/${voyage.fileId}/marks/${markId}`,{method:"DELETE",headers:session.headers})).status,200);
    assert.equal((await json<BookDetail>(`/${voyage.fileId}`)).marks.length,0);

    // Another account sees none of it; the owner's state and books stay private.
    const username=`reader-${randomUUID().slice(0,6)}`;
    assert.equal((await fetch(`${base}/api/v1/auth/users`,{method:"POST",headers,body:JSON.stringify({username,displayName:"Other reader",password:"a long isolated test password"})})).status,201);
    const other=await login(base,username);
    for(const route of [`/${voyage.fileId}`,`/${voyage.fileId}/cover`,`/${voyage.fileId}/epub/r1/OEBPS/chapter1.xhtml`,`/${comic.fileId}/pages/0`])assert.ok([403,404].includes((await get(route,other)).status),route);
    assert.ok([403,404].includes((await fetch(`${base}/api/v1/books/${voyage.fileId}/state`,{method:"PUT",headers:{...other.headers,"content-type":"application/json"},body:JSON.stringify({revision:0,status:"want"})})).status));
    assert.equal((await json<BookLibrary>("/library",other)).books.length,0);

    // A changed Books folder is followed; preferences validate it.
    const prefs=async()=>(await fetch(`${base}/api/v1/pim/preferences`,{headers:session.headers})).json() as Promise<{revision:number;booksFolder?:string}>;
    let current=await prefs();assert.equal(current.booksFolder??"docs/books","docs/books");
    const putPrefs=(value:object,revision:number)=>fetch(`${base}/api/v1/pim/preferences`,{method:"PUT",headers:{...headers,"if-match":`"preferences:${revision}"`},body:JSON.stringify({revision,...value})});
    assert.equal((await putPrefs({booksFolder:"../escape"},current.revision)).status,400);
    assert.equal((await putPrefs({booksTheme:"neon"},current.revision)).status,400);
    assert.equal((await putPrefs({booksFolder:"Reading/Shelf",booksTheme:"sepia",booksFontSize:20},current.revision)).status,200);
    assert.equal((await json<BookLibrary>("/library")).path,"Reading/Shelf");
    current=await prefs();assert.equal((await putPrefs({booksFolder:"docs/books"},current.revision)).status,200);

    // Administrators can turn Books off; the data stays.
    const apps=async()=>(await fetch(`${base}/api/v1/admin/installedApps`,{headers:session.headers})).json() as Promise<{revision:number;items:{id:string;enabled:boolean;canToggle:boolean}[]}>;
    let state=await apps();assert.deepEqual(state.items.find(i=>i.id==="books"),{...state.items.find(i=>i.id==="books"),enabled:true,canToggle:true});
    const toggle=(enabled:boolean,revision:number)=>fetch(`${base}/api/v1/admin/installedApps`,{method:"PUT",headers:{...headers,"if-match":`"apps:${revision}"`},body:JSON.stringify({id:"books",enabled,revision})});
    assert.equal((await toggle(false,state.revision)).status,200);
    assert.equal((await get("/library")).status,404);assert.equal((await get(`/${voyage.fileId}`)).status,404);
    state=await apps();assert.equal((await toggle(true,state.revision)).status,200);
    assert.equal((await json<BookLibrary>("/library")).books.find(b=>b.id===voyage.fileId)!.state!.progress,0.6);

    // Backups carry reading state.
    const pin=randomUUID(),snapshot=await (await import("../runtime/database.ts")).database(process.env.DAR_DATABASE_URL!,process.env.DAR_DATABASE_NAME!,process.env.DAR_DATABASE_TOKEN_FILE!).then(d=>d.call<{format:string;books:{states:{fileId:string}[]}}>("service_backup",[pin]).finally(()=>d.call("service_release_backup",[pin])));
    assert.equal(snapshot.format,"dar-backup-v11");assert.ok(snapshot.books.states.some(s=>s.fileId===voyage.fileId));
  }finally{db.disconnect();}
});
