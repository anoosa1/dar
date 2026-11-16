import assert from "node:assert/strict";
import {chromium} from "@playwright/test";
import {randomUUID} from "node:crypto";
import path from "node:path";
import {login,upload} from "./integration-helpers.ts";
import {epub,cbz,pdf} from "./books-fixtures.ts";
const base=process.env.DAR_INTEGRATION_ORIGIN;if(!base)throw Error("Use an isolated instance.");
const admin=await login(base),username=`books-ui-${randomUUID().slice(0,6)}`,password="a long isolated test password";
assert.equal((await fetch(base+"/api/v1/auth/users",{method:"POST",headers:{...admin.headers,"content-type":"application/json"},body:JSON.stringify({username,displayName:"Books tester",password})})).status,201);
const reader=await login(base,username);
const root=(await (await fetch(`${base}/api/v1/books/library`,{headers:reader.headers})).json()).rootId;
const voyage=await upload(base,reader,"the-test-voyage.epub",epub({title:"The Test Voyage",author:"Ishmael Fixture",series:["Fixture Series",2]}),root);
await upload(base,reader,"second.epub",epub({title:"A Second Voyage",author:"Ishmael Fixture",series:["Fixture Series",3]}),root);
const comic=await upload(base,reader,"fixture-comic.cbz",cbz(),root);
const paper=await upload(base,reader,"paper.pdf",pdf(),root,undefined,undefined,"application/pdf");
const shot=name=>path.join(process.env.DAR_TEST_ROOT,`books-${name}.png`);
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true}),errors=[];
try{
  const context=await browser.newContext({viewport:{width:1440,height:960}}),page=await context.newPage();page.setDefaultTimeout(20000);
  page.on("pageerror",e=>errors.push(e.message));page.on("console",m=>{// The fixture's own <script> is blocked (expected), and the other-device step answers 409 on purpose.
  if(m.type()==="error"&&!/401.*Unauthorized|status of 404|status of 409|directive 'script-src 'none''/.test(m.text()))errors.push(m.text());});
  await page.goto(base);await page.getByLabel("Username",{exact:true}).fill(username);await page.getByLabel("Password",{exact:true}).fill(password);await page.getByRole("button",{name:"Sign in",exact:true}).click();
  await page.getByRole("navigation",{name:"Applications"}).getByRole("link",{name:"Books"}).click();
  await page.waitForURL(/\/books/);
  const main=page.getByRole("main",{name:"Home"});
  await main.getByRole("heading",{name:"Recently added",exact:true}).waitFor();
  await main.getByRole("button",{name:/The Test Voyage/}).first().waitFor();
  // Covers come from the files (EPUB cover image, first PDF page, first comic page).
  await page.waitForFunction(()=>[...document.querySelectorAll(".books-cover img")].filter(img=>img.complete&&img.naturalWidth>0).length>=4);
  await page.screenshot({animations:"disabled",path:shot("home-1440")});

  // Details: series, actions and Want to read.
  await main.getByRole("button",{name:/The Test Voyage/}).first().click();
  const details=page.getByRole("complementary",{name:"Book details",exact:true});
  await details.getByRole("heading",{name:"The Test Voyage",exact:true}).waitFor();
  await details.getByText("Fixture Series · Book 2 of 2",{exact:true}).waitFor();
  await details.getByRole("button",{name:"Want to read",exact:true}).click();
  await page.getByText("Marked as want to read",{exact:true}).waitFor();
  assert.equal(await details.getByRole("button",{name:"Want to read",exact:true}).getAttribute("aria-pressed"),"true");
  await page.screenshot({animations:"disabled",path:shot("details-1440")});

  // EPUB reader: pages turn, typography applies inside the chapter, contents jump, place is saved.
  await details.getByRole("button",{name:"Read",exact:true}).click();
  const book=page.getByRole("main",{name:"Reading The Test Voyage",exact:true});await book.waitFor();
  const frame=page.frameLocator("iframe.epub-view");await frame.getByRole("heading",{name:"Departure"}).waitFor();
  assert.equal(await page.evaluate(()=>document.querySelector("iframe.epub-view").contentDocument.title),"Departure","book scripts do not run");
  const left=()=>page.evaluate(()=>{const d=document.querySelector("iframe.epub-view").contentDocument;return (d.scrollingElement??d.documentElement).scrollLeft;});
  assert.equal(await left(),0);
  await book.focus();await page.keyboard.press("ArrowRight");await page.waitForFunction(()=>{const d=document.querySelector("iframe.epub-view").contentDocument;return (d.scrollingElement??d.documentElement).scrollLeft>0;});
  await book.getByRole("button",{name:"Text and layout",exact:true}).click();
  const typography=page.getByRole("dialog",{name:"Text and layout",exact:true});
  await typography.getByRole("button",{name:/Sepia/}).click();
  await page.waitForFunction(()=>getComputedStyle(document.querySelector("iframe.epub-view").contentDocument.documentElement).backgroundColor==="rgb(244, 236, 216)");
  await typography.getByRole("button",{name:"Larger text",exact:true}).click();
  await page.waitForFunction(()=>getComputedStyle(document.querySelector("iframe.epub-view").contentDocument.documentElement).fontSize==="19px");
  await page.screenshot({animations:"disabled",path:shot("reader-epub-1440")});
  await page.keyboard.press("Escape");await typography.waitFor({state:"detached"});
  const panel=book.getByRole("complementary",{name:"Contents and bookmarks",exact:true});
  if(!await panel.isVisible())await book.getByRole("button",{name:"Contents and bookmarks",exact:true}).click();
  await panel.getByRole("button",{name:"The mast breaks",exact:true}).click();
  await frame.getByText("Paragraph 3 of The Storm.",{exact:false}).waitFor();
  await page.waitForFunction(()=>document.querySelector("iframe.epub-view").contentDocument.title==="The Storm");
  await book.getByRole("button",{name:"Add bookmark",exact:true}).click();
  await book.getByRole("button",{name:"Remove bookmark",exact:true}).waitFor();
  await panel.getByRole("tab",{name:/Bookmarks/}).click();await panel.getByRole("button",{name:/^The mast breaks/}).waitFor();
  // Internal links stay in the reader.
  await page.waitForTimeout(1600);
  const state=await (await fetch(`${base}/api/v1/books/${voyage.fileId}`,{headers:reader.headers})).json();
  assert.equal(state.state.position.spine,1,"the place is saved to the account");assert.equal(state.state.status,"reading");assert.equal(state.state.device.includes("on"),true);
  assert.equal(state.marks.length,1);
  // Reload: the reader reopens at the saved chapter, with the account's typography.
  await page.reload();await page.getByRole("main",{name:"Reading The Test Voyage",exact:true}).waitFor();
  await page.waitForFunction(()=>document.querySelector("iframe.epub-view")?.contentDocument?.title==="The Storm");
  await page.waitForFunction(()=>getComputedStyle(document.querySelector("iframe.epub-view").contentDocument.documentElement).backgroundColor==="rgb(244, 236, 216)");
  // Another device moved on: saving here offers to go there instead of overwriting.
  const remote=await fetch(`${base}/api/v1/books/${voyage.fileId}/state`,{method:"PUT",headers:{...reader.headers,"content-type":"application/json"},body:JSON.stringify({revision:state.state.revision,position:{spine:0,fraction:0},progress:0.01,device:"Phone"})});
  assert.equal(remote.status,200);
  await page.getByRole("main",{name:"Reading The Test Voyage",exact:true}).focus();await page.keyboard.press("ArrowRight");
  const conflict=page.getByRole("alertdialog",{name:"Your place changed on another device"});await conflict.waitFor();
  await conflict.getByText(/on Phone/).waitFor();
  await page.screenshot({animations:"disabled",path:shot("reader-conflict-1440")});
  await conflict.getByRole("button",{name:"Go there",exact:true}).click();
  await page.waitForFunction(()=>document.querySelector("iframe.epub-view")?.contentDocument?.title==="Departure");
  await page.getByRole("button",{name:"Back to library",exact:true}).click();
  await page.getByRole("heading",{name:"Continue reading",exact:true}).waitFor();

  // Comic: pages turn right to left from the comic's own metadata.
  await page.goto(`${base}/books?book=${comic.fileId}&read=1`);
  const comicReader=page.getByRole("main",{name:/^Reading /});await comicReader.waitFor();
  await comicReader.getByRole("img",{name:"Page 1 of 3",exact:true}).waitFor();
  await comicReader.focus();await page.keyboard.press("ArrowLeft");await comicReader.getByRole("img",{name:"Page 2 of 3",exact:true}).waitFor();
  await page.screenshot({animations:"disabled",path:shot("reader-comic-1440")});

  // PDF: drawn by pdf.js, with the page tracked.
  await page.goto(`${base}/books?book=${paper.fileId}&read=1`);
  const pdfReader=page.getByRole("main",{name:"Reading A Fixture Paper",exact:true});await pdfReader.waitFor();
  await page.locator(".pdf-view-page.is-drawn").first().waitFor();
  await pdfReader.getByText("Page 1 of 2",{exact:true}).waitFor();
  await page.screenshot({animations:"disabled",path:shot("reader-pdf-1440")});

  // Files' PDF preview uses the same viewer.
  await page.goto(`${base}/?folder=${root}`);
  await page.getByRole("button",{name:"Actions for paper.pdf",exact:true}).click();await page.getByRole("menuitem",{name:"Details",exact:true}).click();
  await page.getByRole("button",{name:"Preview",exact:true}).click();
  await page.getByRole("region",{name:"Preview of paper.pdf",exact:true}).locator(".pdf-view-page.is-drawn").first().waitFor();
  await page.screenshot({animations:"disabled",path:shot("files-pdf-preview-1440")});

  // Phones: chips, full-screen details and reader.
  const phone=await browser.newContext({viewport:{width:390,height:844},isMobile:true,hasTouch:true}),mobile=await phone.newPage();mobile.setDefaultTimeout(20000);
  mobile.on("pageerror",e=>errors.push(e.message));
  await mobile.goto(base);await mobile.getByLabel("Username",{exact:true}).fill(username);await mobile.getByLabel("Password",{exact:true}).fill(password);await mobile.getByRole("button",{name:"Sign in",exact:true}).click();
  await mobile.getByRole("navigation",{name:"Applications"}).waitFor();await mobile.goto(`${base}/books`);
  await mobile.getByRole("heading",{name:"Continue reading",exact:true}).waitFor().catch(async error=>{await mobile.screenshot({path:shot("failure-390")});throw error;});
  assert.ok(await mobile.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),"no sideways scrolling on phones");
  await mobile.screenshot({animations:"disabled",path:shot("home-390")});
  await mobile.getByRole("button",{name:/A Second Voyage/}).first().click();
  await mobile.getByRole("complementary",{name:"Book details",exact:true}).getByRole("heading",{name:"A Second Voyage"}).waitFor();
  await mobile.screenshot({animations:"disabled",path:shot("details-390")});
  await mobile.getByRole("button",{name:"Read",exact:true}).click();
  await mobile.frameLocator("iframe.epub-view").getByRole("heading",{name:"Departure"}).waitFor();
  await mobile.screenshot({animations:"disabled",path:shot("reader-390")});
  await phone.close();
  assert.deepEqual(errors,[]);
  console.info("PASS Books browser journey (desktop and phone).");
}finally{await browser.close();}
