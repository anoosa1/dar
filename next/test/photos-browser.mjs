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
  assert.equal(await page.evaluate(()=>document.activeElement?.classList.contains("photos-open")),true);
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
  assert.deepEqual(errors,[]);
  console.log("PASS Photos browser: library grid with thumbnails, upload into Files, viewer with details, favorite, description, keyboard navigation and focus return, selection to a new album, settings sheet, search with thumbnails and phone layout.");
}finally{await browser.close();}
