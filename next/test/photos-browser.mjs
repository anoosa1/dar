import assert from "node:assert/strict";
import path from "node:path";
import {chromium} from "@playwright/test";
import {openSearch} from "./ui-helpers.mjs";
const base=process.env.DAR_INTEGRATION_ORIGIN,root=process.env.DAR_TEST_ROOT;if(!base||!root)throw Error("Use the isolated Photos fixture.");
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true}),context=await browser.newContext({viewport:{width:1440,height:950}}),page=await context.newPage(),errors=[];
page.on("pageerror",e=>errors.push(e.message));page.on("console",m=>{if(m.type()==="error"&&!/Failed to load resource/.test(m.text()))errors.push(m.text());});
page.setDefaultTimeout(15000);
try{
  await page.goto(`${base}/photos`);await page.getByLabel("Username",{exact:true}).fill("admin");await page.getByLabel("Password",{exact:true}).fill("a long isolated test password");await page.getByRole("button",{name:"Sign in",exact:true}).click();
  await page.getByRole("heading",{name:"Library",exact:true}).waitFor();
  const grid=page.getByRole("list",{name:"Library photos"});await grid.getByRole("listitem").nth(1).waitFor();
  assert.equal(await grid.getByRole("listitem").count(),2);
  assert.equal(await page.getByRole("link",{name:"Photos",exact:true}).first().getAttribute("aria-current"),"page");

  // Upload from the create button: the photo lands in Files › Photos and appears at the top.
  await page.getByLabel("Upload photos and videos",{exact:true}).setInputFiles(path.join(root,"plain.png"));
  await page.getByRole("region",{name:"Uploads"}).getByText("Done",{exact:true}).waitFor();
  await grid.getByRole("listitem").nth(2).waitFor();
  const newest=grid.getByRole("button",{name:/^Photo plain\.png/});await newest.waitFor();
  await page.waitForFunction(()=>[...document.querySelectorAll(".photos-open img")].some(img=>img.complete&&img.naturalWidth>0));
  await page.screenshot({path:path.join(root,"photos-library.png")});

  // Viewer in the middle sheet with details on the right; favorite, keyboard navigation and focus return.
  await newest.click();const viewer=page.getByRole("region",{name:"Viewer for plain.png"});await viewer.waitFor();
  const details=page.getByRole("complementary",{name:"Photo details"});await details.getByText("320 × 200",{exact:false}).waitFor();
  await viewer.getByRole("button",{name:"Add to Favorites"}).click();await viewer.getByRole("button",{name:"Remove from Favorites"}).waitFor();
  await details.getByPlaceholder("Add a description…").fill("Test card");await details.getByPlaceholder("Add a description…").blur();await details.getByText("Saved",{exact:true}).waitFor();
  await page.screenshot({path:path.join(root,"photos-viewer.png")});
  await page.keyboard.press("ArrowRight");await page.getByRole("region",{name:/^Viewer for (clip\.mp4|broken\.jpg)$/}).waitFor();
  await page.keyboard.press("Escape");await grid.waitFor();
  await page.waitForFunction(()=>document.activeElement?.classList.contains("photos-open"),undefined,{timeout:5000});
  await page.getByRole("button",{name:/^Favorites/}).click();await page.getByRole("list",{name:"Favorites photos"}).getByRole("button",{name:/^Photo plain\.png/}).first().waitFor();
  await page.getByRole("button",{name:/^Library/}).click();await grid.waitFor();

  // Select, then create an album from the selection.
  await page.getByLabel("Select clip.mp4",{exact:true}).check();const selection=page.getByRole("toolbar",{name:"Selection"});await selection.getByText("1 selected").waitFor();
  await selection.getByRole("button",{name:"Add to album"}).click();await page.getByRole("menuitem",{name:"New album…"}).click();
  const dialog=page.getByRole("dialog",{name:"New album"});await dialog.getByLabel("Name").fill("Road trip");await dialog.getByRole("button",{name:"Create album"}).click();
  await page.getByRole("navigation",{name:"Albums"}).getByRole("button",{name:/^Road trip/}).click();
  await page.getByRole("heading",{name:"Road trip",exact:true}).waitFor();await page.getByRole("list",{name:"Road trip photos"}).getByRole("button",{name:/^Video clip\.mp4/}).waitFor();assert.equal(await page.getByRole("list",{name:"Road trip photos"}).getByRole("listitem").count(),1);

  // Settings sheet and the spotlight search opening a photo.
  await page.getByRole("button",{name:"Photos settings",exact:true}).first().click();const sheet=page.getByRole("dialog",{name:"Photos settings"});await sheet.getByText("Where photos come from").waitFor();
  await sheet.getByRole("tab",{name:"Immich"}).click();await sheet.getByRole("heading",{name:"Immich mobile app"}).waitFor();
  assert.equal(await sheet.getByLabel("Server URL for the Immich app").inputValue(),base);await sheet.getByText("admin@dar").waitFor();await sheet.getByText("No devices yet.").waitFor();
  await sheet.getByLabel("Server address").fill("http://169.254.169.254");await sheet.getByLabel("API key").fill("anything");await sheet.getByRole("button",{name:"Connect"}).click();
  await sheet.getByText(/reserved network/).waitFor();await sheet.getByLabel("Server address").scrollIntoViewIfNeeded();await page.screenshot({path:path.join(root,"photos-immich-settings.png")});
  await sheet.getByRole("button",{name:"Close settings"}).click();await sheet.waitFor({state:"detached"});
  await (await openSearch(page)).fill("test card");
  const result=page.getByRole("listbox",{name:"Search results",exact:true}).getByRole("option",{name:/plain\.png/});await result.waitFor();
  assert.equal(await result.locator("img").count()>0,true);await page.screenshot({path:path.join(root,"photos-search.png")});
  await result.click();await page.getByRole("region",{name:"Viewer for plain.png"}).waitFor();

  // Phones: chips for views, the viewer above its details, no horizontal overflow.
  await page.setViewportSize({width:390,height:844});await page.getByRole("region",{name:"Viewer for plain.png"}).waitFor();
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);assert.equal(await page.getByRole("complementary",{name:"Photo details"}).isVisible(),false);await page.screenshot({path:path.join(root,"photos-phone-viewer.png")});
  await page.getByRole("button",{name:"Photo info"}).click();const back=page.getByRole("button",{name:"Back to photo"});await back.waitFor();assert.equal(await back.evaluate(el=>el===document.activeElement),true);await page.screenshot({path:path.join(root,"photos-phone-info.png")});
  await back.click();await page.getByRole("button",{name:"Photo info"}).waitFor();assert.equal(await page.getByRole("button",{name:"Photo info"}).evaluate(el=>el===document.activeElement),true);
  await page.getByRole("button",{name:"Back to photos"}).click();await page.getByRole("list",{name:/photos$/}).waitFor();
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);await page.screenshot({path:path.join(root,"photos-phone.png")});

  // Trash: the photo trashed in Files is in Photos › Trash and restores; moving to the trash offers Undo.
  await page.setViewportSize({width:1440,height:950});await page.goto(`${base}/photos?view=trash`);
  const trashGrid=page.getByRole("list",{name:"Trash photos"});await trashGrid.getByRole("button",{name:/^Photo trip\.jpg/}).waitFor();
  await page.getByLabel("Select trip.jpg",{exact:true}).check();await page.getByRole("toolbar",{name:"Selection"}).getByRole("button",{name:"Restore"}).click();
  await page.getByText("1 item restored").waitFor();await page.getByText("Trash is empty").waitFor();
  await page.getByRole("button",{name:/^Library/}).click();await grid.getByRole("button",{name:/^Photo trip\.jpg/}).waitFor();
  await page.getByLabel("Select trip.jpg",{exact:true}).check();await page.getByRole("toolbar",{name:"Selection"}).getByRole("button",{name:"Move to trash"}).click();
  await page.getByText("1 item moved to the trash").waitFor();assert.equal(await grid.getByRole("button",{name:/^Photo trip\.jpg/}).count(),0);
  await page.getByRole("button",{name:"Undo",exact:true}).click();await grid.getByRole("button",{name:/^Photo trip\.jpg/}).waitFor();

  // Duplicates: plain.png was uploaded twice; keep one and trash the other.
  await page.getByRole("button",{name:/^Duplicates/}).click();await page.getByRole("heading",{name:"Duplicates",exact:true}).waitFor();
  const group=page.getByRole("region",{name:/^2 copies of plain\.png/});await group.waitFor();await group.getByText("Identical files").waitFor();
  await page.screenshot({path:path.join(root,"photos-duplicates.png")});
  await group.getByRole("button",{name:"Keep 1, trash 1"}).click();await page.getByText("Kept 1, moved 1 to the trash").waitFor();await page.getByText("No duplicates").waitFor();

  // Locked folder: moving asks for the password, then the photo leaves the library; the video plays there.
  await page.getByRole("button",{name:/^Library/}).click();await grid.getByRole("button",{name:/^Video clip\.mp4/}).waitFor();
  await page.getByLabel("Select clip.mp4",{exact:true}).check();await page.getByRole("toolbar",{name:"Selection"}).getByRole("button",{name:"Move to Locked folder"}).click();
  const unlock=page.getByRole("dialog",{name:"Unlock the Locked folder"});await unlock.getByLabel("Password").fill("wrong password here");await unlock.getByRole("button",{name:"Unlock"}).click();
  await unlock.getByText("That password is not correct.").waitFor();
  await unlock.getByLabel("Password").fill("a long isolated test password");await unlock.getByRole("button",{name:"Unlock"}).click();
  await page.getByText("1 item moved to the Locked folder").waitFor();assert.equal(await grid.getByRole("button",{name:/^Video clip\.mp4/}).count(),0);
  await page.getByRole("button",{name:/^Locked folder/}).click();const lockedGrid=page.getByRole("list",{name:"Locked folder photos"});
  await lockedGrid.getByRole("button",{name:/^Video clip\.mp4/}).click();const video=page.getByRole("region",{name:"Viewer for clip.mp4"}).locator("video");
  await page.waitForFunction(()=>(document.querySelector(".photos-stage video")?.readyState??0)>=1);
  assert.match(await video.getAttribute("src"),/\/api\/v1\/photos\/assets\/[a-f0-9-]+\/video$/);
  await page.getByRole("complementary",{name:"Photo details"}).getByText(/In the Locked folder/).waitFor();await page.screenshot({path:path.join(root,"photos-locked.png")});
  await page.keyboard.press("Escape");await page.getByRole("button",{name:"Lock",exact:true}).click();await page.getByLabel("Password").waitFor();assert.equal(await lockedGrid.count(),0);

  // Files: the same Locked folder, outside All files, opens with the password and deletes permanently.
  await page.goto(`${base}/?section=locked`);await page.getByRole("heading",{name:"Locked folder",exact:true}).waitFor();
  await page.getByLabel("Password").fill("a long isolated test password");await page.getByRole("button",{name:"Unlock"}).click();
  await page.getByRole("button",{name:"clip.mp4",exact:true}).waitFor();await page.screenshot({path:path.join(root,"files-locked.png")});
  await page.getByRole("button",{name:"Actions for clip.mp4"}).click();await page.getByRole("menuitem",{name:"Delete permanently"}).waitFor();assert.equal(await page.getByRole("menuitem",{name:"Move to deleted files"}).count(),0);await page.keyboard.press("Escape");
  await page.getByRole("button",{name:"Lock",exact:true}).click();await page.getByLabel("Password").waitFor();assert.equal(await page.getByRole("button",{name:"clip.mp4",exact:true}).count(),0);
  await page.getByRole("button",{name:"All files"}).click();await page.getByRole("heading",{name:"All files",exact:true}).waitFor();assert.equal(await page.getByRole("button",{name:"Locked folder",exact:true}).count(),1);

  // Sharing: an album shared with another account and through a password-protected link.
  await page.goto(`${base}/photos`);await grid.getByRole("button",{name:/^Photo trip\.jpg/}).waitFor();
  await page.getByLabel("Select trip.jpg",{exact:true}).check();await page.getByRole("toolbar",{name:"Selection"}).getByRole("button",{name:"Add to album"}).click();
  await page.getByRole("menuitem",{name:"New album…"}).click();const named=page.getByRole("dialog",{name:"New album"});await named.getByLabel("Name").fill("Shared trip");await named.getByRole("button",{name:"Create album"}).click();
  await page.getByRole("navigation",{name:"Albums"}).getByRole("button",{name:/^Shared trip/}).click();await page.getByRole("heading",{name:"Shared trip",exact:true}).waitFor();
  await page.getByRole("button",{name:"Share",exact:true}).click();const pane=page.getByRole("complementary",{name:"Share Shared trip"});
  await pane.getByLabel("Username").fill("other");await pane.getByRole("button",{name:"Share",exact:true}).click();await pane.getByText("@other").waitFor();
  await pane.getByRole("button",{name:"Create link"}).click();const address=pane.getByLabel("Link address");await address.waitFor();
  const shareUrl=await address.inputValue();assert.match(shareUrl,/\/s\/[A-Za-z0-9_-]{43}$/);
  await pane.getByRole("button",{name:"Add password"}).click();await pane.getByLabel("Link password").fill("open sesame");await pane.getByRole("button",{name:"Set password"}).click();
  await pane.getByText(/Never expires · Password/).waitFor();await page.screenshot({path:path.join(root,"photos-share-pane.png")});
  await pane.getByRole("button",{name:"Done"}).click();await pane.waitFor({state:"detached"});
  assert.equal(await page.getByRole("navigation",{name:"Albums"}).getByRole("button",{name:/^Shared trip.*shared/}).count(),1);

  // A visitor without an account: password, grid, viewer with download.
  const guestContext=await browser.newContext({viewport:{width:1280,height:900}}),guest=await guestContext.newPage();
  guest.on("pageerror",e=>errors.push(e.message));guest.on("console",m=>{if(m.type()==="error"&&!/Failed to load resource/.test(m.text()))errors.push(m.text());});guest.setDefaultTimeout(15000);
  await guest.goto(shareUrl);await guest.getByRole("heading",{name:"Password required"}).waitFor();
  await guest.getByLabel("Password").fill("wrong one");await guest.getByRole("button",{name:"Open"}).click();await guest.getByText("That password is not correct.").waitFor();
  await guest.getByLabel("Password").fill("open sesame");await guest.getByRole("button",{name:"Open"}).click();
  await guest.getByRole("heading",{name:"Shared trip",exact:true}).waitFor();await guest.getByText(/Shared by Photos fixture · 1 item/).waitFor();
  await guest.waitForFunction(()=>[...document.querySelectorAll(".photos-open img")].some(img=>img.complete&&img.naturalWidth>0));await guest.screenshot({path:path.join(root,"photos-public.png")});
  await guest.getByRole("button",{name:/^Photo trip\.jpg/}).click();await guest.getByRole("region",{name:"Viewer for trip.jpg"}).waitFor();
  assert.match(await guest.getByRole("link",{name:"Download original"}).getAttribute("href"),/\/public\/photos\/.+\/original$/);
  await guest.keyboard.press("Escape");await guest.getByRole("list",{name:"Shared trip photos"}).waitFor();
  await guest.setViewportSize({width:390,height:844});assert.equal(await guest.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);await guest.screenshot({path:path.join(root,"photos-public-phone.png")});
  await guestContext.close();

  // The other account finds it under Shared with me, view only.
  const otherContext=await browser.newContext({viewport:{width:1440,height:950}}),other=await otherContext.newPage();other.setDefaultTimeout(15000);
  other.on("pageerror",e=>errors.push(e.message));
  await other.goto(`${base}/photos`);await other.getByLabel("Username",{exact:true}).fill("other");await other.getByLabel("Password",{exact:true}).fill("a long isolated test password");await other.getByRole("button",{name:"Sign in",exact:true}).click();
  await other.getByRole("button",{name:/^Shared with me/}).click();await other.getByRole("list",{name:"Albums shared with you"}).getByRole("button",{name:/^Shared trip/}).click();
  await other.getByText(/Shared by Photos fixture · view only/).waitFor();await other.getByRole("button",{name:/^Photo trip\.jpg/}).click();
  const theirViewer=other.getByRole("region",{name:"Viewer for trip.jpg"});await theirViewer.waitFor();assert.equal(await theirViewer.getByRole("button",{name:"Add to Favorites"}).count(),0);
  await other.waitForFunction(()=>(document.querySelector(".photos-stage img")?.naturalWidth??0)>0);
  await other.getByRole("complementary",{name:"Photo details"}).getByText(/Shared by Photos fixture/).waitFor();await other.screenshot({path:path.join(root,"photos-shared-with-me.png")});
  await otherContext.close();
  assert.deepEqual(errors,[]);
  console.log("PASS Photos browser: library grid with thumbnails, upload into Files, viewer with details, favorite, description, keyboard navigation and focus return, selection to a new album, settings sheet, search with thumbnails and phone layout; trash with restore and Undo, duplicates, the Locked folder with password unlock and video playback, and the Files Locked folder; album sharing with a person and a password-protected link, the public page on desktop and phone, Shared with me, and the Immich settings tab.");
}finally{await browser.close();}
