import assert from "node:assert/strict";
import {choose,switchApp} from "./ui-helpers.mjs";
import { chromium } from "@playwright/test";
import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
// Initiate native dragging before scrolling the destination into view.
async function drag(source,target){
  const mouse=source.page().mouse;await source.scrollIntoViewIfNeeded();const start=await source.boundingBox();assert.ok(start);
  await mouse.move(start.x+12,start.y+12);await mouse.down();
  try{await mouse.move(start.x+24,start.y+24,{steps:4});await target.scrollIntoViewIfNeeded();const end=await target.boundingBox();assert.ok(end);await mouse.move(end.x+end.width/2,end.y+end.height/2,{steps:10});}finally{await mouse.up();}
}
const origin=process.env.DAR_INTEGRATION_ORIGIN;
if(!origin)throw new Error("An isolated test instance is required.");
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true}),failures=[];
try{
  const context=await browser.newContext({viewport:{width:1440,height:950},acceptDownloads:true}),page=await context.newPage();
  page.on("pageerror",error=>failures.push(error.message));page.on("console",message=>{if(message.type()==="error"&&!/401.*Unauthorized/.test(message.text()))failures.push(message.text());});
  await page.goto(origin+"/contacts");await page.getByLabel("Username",{exact:true}).fill("admin");await page.getByLabel("Password",{exact:true}).fill("a long isolated test password");await page.getByRole("button",{name:"Sign in",exact:true}).click();
  await page.getByRole("heading",{name:"All contacts",exact:true}).waitFor();
  await page.getByRole("button",{name:"New address book",exact:true}).click();
  const book="Browser people "+randomUUID().slice(0,6);
  await page.locator("dialog,[data-editor-pane]").getByLabel("Name",{exact:true}).fill(book);await page.locator("dialog,[data-editor-pane]").getByRole("button",{name:"Save",exact:true}).click();
  await page.getByRole("heading",{name:book,exact:true}).waitFor();await page.getByRole("button",{name:book,exact:true}).waitFor();
  await page.getByRole("button",{name:"New contact",exact:true}).click();
  const editor=page.locator("dialog,[data-editor-pane]");await editor.getByLabel("First name",{exact:true}).fill("Amira");await editor.getByLabel("Last name",{exact:true}).fill("Browser");
  await editor.getByRole("button",{name:"Add email",exact:true}).click();await editor.getByLabel("Email 1",{exact:true}).fill("amira@example.com");
  await editor.getByLabel("Groups (one per line)",{exact:true}).fill("Design team");await editor.getByLabel("Notes",{exact:true}).fill("A real contact, saved in SpacetimeDB.");await editor.getByRole("button",{name:"Save contact",exact:true}).click();
  await page.getByRole("heading",{name:"Amira Browser",exact:true}).waitFor();
  const deep=page.url();await page.reload();await page.getByRole("heading",{name:"Amira Browser",exact:true}).waitFor();assert.equal(page.url(),deep);
  await page.getByRole("button",{name:"Edit contact",exact:true}).click();await page.locator("dialog,[data-editor-pane]").getByLabel("Job title",{exact:true}).fill("Product designer");await page.locator("dialog,[data-editor-pane]").getByLabel("Favorite",{exact:true}).check();await page.locator("dialog,[data-editor-pane]").getByRole("button",{name:"Save contact",exact:true}).click();
  await page.getByText("Product designer",{exact:true}).waitFor();await page.screenshot({path:"/tmp/dar-contacts-desktop.png",fullPage:true});
  await page.waitForFunction(()=>document.activeElement?.textContent?.trim()==="Edit contact");
  const exported=page.waitForEvent("download");await page.getByRole("link",{name:"Export contact",exact:true}).click();const download=await exported;assert.match(await readFile(await download.path(),"utf8"),/FN:Amira Browser/);
  await page.getByRole("button",{name:"Close contact details",exact:true}).click();await page.getByRole("button",{name:"Favorites",exact:true}).click();await page.getByRole("button",{name:/Amira Browser/}).waitFor();
  await page.getByRole("button",{name:/Amira Browser/}).click();await page.getByRole("button",{name:"Close contact details",exact:true}).click();await page.getByLabel("Search contacts",{exact:true}).fill("no contact has this name");await page.getByRole("heading",{name:"No matching contacts",exact:true}).waitFor();await page.getByRole("button",{name:"Clear search",exact:true}).click();
  await switchApp(page,"calendar");await page.getByRole("heading",{name:/2026/}).waitFor();
  await page.getByRole("button",{name:"New calendar",exact:true}).click();const calendar="Browser time "+randomUUID().slice(0,6);await page.locator("dialog,[data-editor-pane]").getByLabel("Name",{exact:true}).fill(calendar);await page.locator("dialog,[data-editor-pane]").getByRole("button",{name:"Save",exact:true}).click();await page.getByRole("button",{name:calendar,exact:true}).waitFor();
  await page.getByLabel("Go to date",{exact:true}).fill("2026-11-09");await page.getByRole("button",{name:"New event",exact:true}).click();
  const eventTitle="Browser design review "+randomUUID().slice(0,6),event=page.locator("dialog,[data-editor-pane]");await event.getByLabel("Title",{exact:true}).fill(eventTitle);await event.getByLabel("Starts",{exact:true}).fill("2026-11-09T09:00");await event.getByLabel("Ends",{exact:true}).fill("2026-11-09T10:00");await event.getByLabel("Timezone",{exact:true}).fill("America/Toronto");await choose(event.getByLabel("Repeats",{exact:true}),"weekly");await event.getByLabel("Recurrence rule",{exact:true}).fill("FREQ=WEEKLY;COUNT=3");await event.getByRole("button",{name:"Save event",exact:true}).click();
  await page.getByRole("button",{name:new RegExp(eventTitle)}).first().click();await page.getByRole("heading",{name:eventTitle,exact:true}).waitFor();await page.getByRole("button",{name:"Edit event",exact:true}).click();await page.getByRole("button",{name:"This event only",exact:true}).click();await page.locator("dialog,[data-editor-pane]").getByLabel("Title",{exact:true}).fill(eventTitle+" moved");await page.locator("dialog,[data-editor-pane]").getByRole("button",{name:"Save event",exact:true}).click();await page.getByRole("button",{name:new RegExp(eventTitle+" moved")}).first().waitFor();
  await page.screenshot({path:"/tmp/dar-calendar-desktop.png",fullPage:true});
  await drag(page.getByRole("button",{name:new RegExp(eventTitle+" moved")}).first(),page.getByRole("button",{name:"New event 2026-11-09 15:00",exact:true}).locator(".."));
  await page.locator("dialog,[data-editor-pane]").getByLabel("Starts",{exact:true}).waitFor();assert.equal(await page.locator("dialog,[data-editor-pane]").getByLabel("Starts",{exact:true}).inputValue(),"2026-11-09T15:00");await page.locator("dialog,[data-editor-pane]").getByRole("button",{name:"Save event",exact:true}).click();
  await page.locator("dialog,[data-editor-pane]").waitFor({state:"hidden"});
  await page.getByRole("button",{name:new RegExp("15:00.*"+eventTitle+" moved")}).waitFor();
  await page.getByLabel("Calendar time grid",{exact:true}).evaluate(element=>{element.scrollTop=12*64;});
  await drag(page.getByRole("button",{name:`Resize ${eventTitle} moved`,exact:true}),page.getByRole("button",{name:"New event 2026-11-09 18:00",exact:true}).locator(".."));
  await page.locator("dialog,[data-editor-pane]").getByLabel("Ends",{exact:true}).waitFor();assert.equal(await page.locator("dialog,[data-editor-pane]").getByLabel("Ends",{exact:true}).inputValue(),"2026-11-09T18:00");await page.locator("dialog,[data-editor-pane]").getByRole("button",{name:"Save event",exact:true}).click();
  await page.locator("dialog,[data-editor-pane]").waitFor({state:"hidden"});
  for(const view of ["day","month","agenda","week"]){await choose(page.getByLabel("Calendar view",{exact:true}),view);await page.getByRole("button",{name:new RegExp(eventTitle+" moved")}).first().waitFor();}
  await choose(page.getByLabel("Calendar view",{exact:true}),"agenda");await page.getByRole("button",{name:new RegExp(eventTitle+" moved")}).first().click();await page.getByRole("button",{name:"Delete event",exact:true}).click();await page.getByRole("button",{name:"This event only",exact:true}).click();await page.getByRole("button",{name:new RegExp(eventTitle+" moved")}).waitFor({state:"hidden"});
  await page.getByLabel("Show cancelled events",{exact:true}).check();
  await page.getByRole("button",{name:new RegExp(eventTitle+" moved")}).first().click();
  await page.getByRole("button",{name:"Edit event",exact:true}).click();await page.getByRole("button",{name:"This event only",exact:true}).click();await choose(page.locator("dialog,[data-editor-pane]").getByLabel("Status",{exact:true}),"CONFIRMED");await page.locator("dialog,[data-editor-pane]").getByRole("button",{name:"Save event",exact:true}).click();await page.locator("dialog,[data-editor-pane]").waitFor({state:"hidden"});
  await page.getByLabel("Show cancelled events",{exact:true}).uncheck();await page.getByRole("button",{name:new RegExp(eventTitle+" moved")}).first().waitFor();
  const phone=await context.newPage();await phone.setViewportSize({width:390,height:844});phone.on("pageerror",error=>failures.push(error.message));await phone.goto(origin+"/contacts");await phone.getByRole("button",{name:/Amira Browser/}).click();await phone.getByRole("heading",{name:"Amira Browser",exact:true}).waitFor();assert.ok(await phone.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));await phone.screenshot({path:"/tmp/dar-contacts-mobile.png",fullPage:true});
  await phone.goto(origin+"/calendar?date=2026-11-16&view=week");await phone.getByRole("button",{name:new RegExp(eventTitle)}).first().waitFor();assert.ok(await phone.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));await phone.screenshot({path:"/tmp/dar-calendar-mobile.png",fullPage:true});await phone.getByRole("button",{name:new RegExp(eventTitle)}).first().click();await phone.getByRole("heading",{name:eventTitle,exact:true}).waitFor();assert.ok(await phone.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  assert.deepEqual(failures,[]);console.info("PASS Contacts and Calendar desktop/phone CRUD, deep links, export, filters, recurring occurrence edit/delete, views, navigation and console.");await context.close();
}catch(error){for(const context of browser.contexts())for(const page of context.pages()){console.error((await page.locator("body").innerText()).slice(0,6000));await page.screenshot({path:"/tmp/dar-pim-browser-failure.png",fullPage:true}).catch(()=>{});}throw error;}finally{await browser.close();}
