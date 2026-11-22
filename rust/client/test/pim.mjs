// Contacts and Calendar in React on the TypeScript stack and in the Rust
// client on the Rust stack: the same content made through each client, the same states
// screenshotted, and the Rust side's operations checked. Usage: node pim.mjs <out dir>
// (DAR_REACT_ORIGIN, DAR_REACT_ROOT, DAR_RUST_ORIGIN, DAR_RUST_ROOT; see run.sh).
import {createRequire} from "node:module";import {execSync} from "node:child_process";import {mkdirSync,readFileSync,writeFileSync} from "node:fs";
import assert from "node:assert/strict";
const require=createRequire(new URL("../../../next/package.json",import.meta.url));const {chromium}=require("@playwright/test");
const out=process.argv[2];mkdirSync(out,{recursive:true});
const react=process.env.DAR_REACT_ORIGIN,rust=process.env.DAR_RUST_ORIGIN,roots={[react]:process.env.DAR_REACT_ROOT,[rust]:process.env.DAR_RUST_ROOT};
const password="a long isolated test password",errors=[];
const browser=await chromium.launch({executablePath:execSync("which chromium").toString().trim(),headless:true});
async function open(base,viewport={width:1440,height:950},scheme="light"){
  const context=await browser.newContext({viewport,colorScheme:scheme,locale:"en-US",timezoneId:"UTC"});const page=await context.newPage();
  page.on("pageerror",e=>errors.push(`${base}: ${e.message}`));page.on("console",m=>{if(m.type()==="error"&&m.text().includes("panicked"))errors.push(`${base}: ${m.text().split("\n")[0]}`);});
  return {context,page};
}
async function shot(page,name){await page.mouse.move(0,0);await page.waitForTimeout(500);await page.screenshot({path:`${out}/${name}.png`});}
async function signIn(base,page,path){
  await page.goto(base+path);await page.getByRole("heading",{name:/^(Sign in|Set up Dar)$/}).or(page.locator(".app-rail")).first().waitFor({timeout:30000});
  if(await page.getByRole("heading",{name:"Set up Dar"}).isVisible()){
    await page.getByLabel("Display name").fill("Amina Rahman");await page.getByLabel("Username").fill("amina");await page.getByLabel("Password",{exact:true}).fill(password);
    await page.getByLabel("Instance setup key").fill(readFileSync(roots[base]+"/secrets/bootstrap-token","utf8").trim());await page.getByRole("button",{name:"Create your account"}).click();
    await page.locator(".app-rail").waitFor({timeout:30000});if(new URL(page.url()).pathname!==path)await page.goto(base+path);
  }else if(await page.getByLabel("Username").isVisible()){await page.getByLabel("Username").fill("amina");await page.getByLabel("Password",{exact:true}).fill(password);await page.getByRole("button",{name:"Sign in",exact:true}).click();}
  await page.locator(".pim-main").waitFor({timeout:30000});await page.locator(".connection-state").waitFor({state:"detached",timeout:30000});
}
const card=["BEGIN:VCARD","VERSION:3.0","UID:imported-1","FN:Zainab Idris","N:Idris;Zainab;;;","EMAIL;TYPE=work:zainab@example.com","CATEGORIES:Work","END:VCARD","BEGIN:VCARD","VERSION:3.0","UID:imported-2","FN:Bilal Haddad","N:Haddad;Bilal;;;","TEL;TYPE=mobile:+1 555 0100","X-DAR-FAVORITE:1","END:VCARD",""].join("\r\n");
async function contactsContent(base){
  const {context,page}=await open(base);await signIn(base,page,"/contacts");
  if(!await page.getByRole("button",{name:/Personal/}).first().isVisible()){
    await page.getByRole("button",{name:"New address book"}).click();await page.getByLabel("Name",{exact:true}).fill("Personal");await page.getByRole("button",{name:"Save"}).click();
    await page.getByRole("button",{name:/Personal/}).first().waitFor();
    await page.getByRole("button",{name:"New contact"}).click();await page.getByLabel("First name").fill("Amina");await page.getByLabel("Last name").fill("Rahman");
    await page.getByLabel("Organization").fill("Dar");await page.getByLabel("Job title").fill("Engineer");
    await page.getByRole("button",{name:"Add email"}).click();await page.getByLabel("Email 1",{exact:true}).fill("amina@example.com");
    await page.getByRole("button",{name:"Add phone"}).click();await page.getByLabel("Phone 1",{exact:true}).fill("+44 20 7946 0000");
    await page.getByRole("button",{name:"Add address"}).click();await page.getByLabel("Street").fill("1 High Street");await page.getByLabel("City").fill("London");await page.getByLabel("Country").fill("United Kingdom");
    await page.getByLabel("Birthday").fill("1990-04-02");await page.getByLabel("Groups (one per line)").fill("Family");await page.getByLabel("Notes").fill("Met at the conference.");
    await page.getByRole("button",{name:"Save contact"}).click();await page.locator(".pim-hero h2",{hasText:"Amina Rahman"}).waitFor();
    // Import two more through the settings sheet.
    await page.getByRole("button",{name:"Contacts settings"}).first().click();await page.getByRole("tab",{name:"Import and export"}).click();
    await page.getByLabel("Or paste content").fill(card);await page.getByRole("button",{name:"Import vCard"}).click();await page.getByText("2 imported; 0 failed.").waitFor({timeout:20000});
    await page.getByRole("button",{name:"Close settings"}).click();
  }
  await context.close();
}
async function contactsStates(base,name){
  for(const scheme of ["light","dark"]){
    const {context,page}=await open(base,{width:1440,height:950},scheme);await signIn(base,page,"/contacts");
    await page.locator(".contact-row").nth(2).waitFor();
    await shot(page,`contacts-${scheme}-${name}`);if(scheme==="dark"){await context.close();continue;}
    await page.getByRole("button",{name:/Amina Rahman/}).click();await page.locator(".pim-hero h2").waitFor();await shot(page,`contact-details-${name}`);
    await page.getByRole("button",{name:"Edit contact"}).click();await page.locator(".contact-editor").waitFor();await shot(page,`contact-editor-${name}`);await page.getByRole("button",{name:"Close editor"}).click();
    await page.getByRole("button",{name:"Actions for address book Personal"}).click();await shot(page,`contacts-book-menu-${name}`);await page.getByRole("menuitem",{name:"Share address book"}).click();await page.getByRole("dialog").waitFor();await page.waitForTimeout(800);await shot(page,`contacts-share-${name}`);await page.keyboard.press("Escape");
    await page.getByRole("button",{name:"Favorites"}).click();await page.locator(".contact-row").first().waitFor();await shot(page,`contacts-favorites-${name}`);
    await page.getByRole("button",{name:"Family"}).click();await page.waitForTimeout(600);await shot(page,`contacts-group-${name}`);
    await page.getByRole("button",{name:"Contacts settings"}).first().click();await page.locator(".settings-sheet").waitFor();await shot(page,`contacts-settings-${name}`);
    await page.getByRole("tab",{name:"Import and export"}).click();await page.waitForTimeout(600);await shot(page,`contacts-transfers-${name}`);await page.getByRole("button",{name:"Close settings"}).click();
    await page.setViewportSize({width:390,height:844});await page.goto(base+"/contacts");await page.locator(".contact-row").first().waitFor();await shot(page,`contacts-phone-${name}`);
    await context.close();
  }
}
await Promise.all([react,rust].map(contactsContent));
for(const [base,name] of [[react,"react"],[rust,"rust"]])await contactsStates(base,name);

// Rust side behaviour: edit, export, delete.
{
  const {context,page}=await open(rust);await signIn(rust,page,"/contacts");
  const step=async(name,fn)=>{process.stdout.write(name+" … ");await fn();console.log("ok");};
  await step("edit a contact",async()=>{await page.getByRole("button",{name:/Bilal Haddad/}).click();await page.getByRole("button",{name:"Edit contact"}).click();await page.getByLabel("Job title").fill("Pilot");await page.getByRole("button",{name:"Save contact"}).click();await page.locator(".pim-hero .muted",{hasText:"Pilot"}).waitFor();});
  await step("export keeps the vCard",async()=>{const href=await page.getByRole("link",{name:"Export contact"}).getAttribute("href");const text=await (await page.request.get(rust+href)).text();assert.match(text,/TITLE:Pilot/);assert.match(text,/TEL;TYPE=mobile:\+1 555 0100/);});
  await step("restore the contact",async()=>{await page.getByRole("button",{name:"Edit contact"}).click();await page.getByLabel("Job title").fill("");await page.getByRole("button",{name:"Save contact"}).click();await page.locator(".pim-hero .muted",{hasText:"Pilot"}).waitFor({state:"detached"});});
  await step("live update from another tab",async()=>{const other=await context.newPage();await other.goto(rust+"/contacts");await other.locator(".contact-row").first().waitFor({timeout:30000});
    await other.getByRole("button",{name:"New contact"}).click();await other.getByLabel("First name").fill("Live");await other.getByLabel("Last name").fill("Update");await other.getByRole("button",{name:"Save contact"}).click();await other.locator(".pim-hero h2",{hasText:"Live Update"}).waitFor();
    await page.getByRole("button",{name:/Live Update/}).waitFor({timeout:10000});await other.close();});
  await step("delete a contact",async()=>{await page.getByRole("button",{name:/Live Update/}).click();await page.getByRole("button",{name:"Delete contact"}).click();await page.getByRole("dialog").getByRole("button",{name:"Delete permanently"}).click();await page.getByRole("button",{name:/Live Update/}).waitFor({state:"detached"});});
  await context.close();
}

// Calendar: a calendar and an event made in the editor, more imported as ICS (a weekly series,
// an overlapping event and a multi-day all-day event), the same views screenshotted.
const ics=["BEGIN:VCALENDAR","VERSION:2.0","PRODID:-//Dar test//EN",
  "BEGIN:VEVENT","UID:standup-1","DTSTAMP:20261101T000000Z","DTSTART:20261102T093000Z","DTEND:20261102T094500Z","RRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=10","SUMMARY:Standup","END:VEVENT",
  "BEGIN:VEVENT","UID:lunch-1","DTSTAMP:20261101T000000Z","DTSTART:20261118T143000Z","DTEND:20261118T150000Z","SUMMARY:Lunch and learn","LOCATION:Library","END:VEVENT",
  "BEGIN:VEVENT","UID:conference-1","DTSTAMP:20261101T000000Z","DTSTART;VALUE=DATE:20261120","DTEND;VALUE=DATE:20261124","SUMMARY:Conference","END:VEVENT",
  "END:VCALENDAR",""].join("\r\n");
const at="/calendar?date=2026-11-18&view=";
async function calendarContent(base){
  const {context,page}=await open(base);await signIn(base,page,at+"month");
  await page.locator(".calendar-month, .pim-empty").first().waitFor({timeout:30000});
  // Each step is skipped when a previous run already made its content.
  if(await page.getByRole("button",{name:"Create calendar"}).isVisible()){
    await page.getByRole("button",{name:"Create calendar"}).click();await page.getByLabel("Name",{exact:true}).fill("Work");await page.getByRole("button",{name:"Save"}).click();
  }
  await page.locator(".calendar-nav-row",{hasText:"Work"}).waitFor();await page.locator(".calendar-month").waitFor();
  if(!await page.locator(".calendar-event",{hasText:"Design review"}).count()){
    await page.getByRole("button",{name:"New event"}).click();await page.getByPlaceholder("Add title").fill("Design review");
    await page.getByLabel("Starts").fill("2026-11-18");await page.getByLabel("Start time").fill("14:00");await page.getByLabel("End time").fill("15:30");
    await page.getByLabel("Location").fill("Room 4");await page.getByLabel("Description").fill("Walk through the new layouts.");
    await page.getByRole("button",{name:"Save event"}).click();await page.locator(".calendar-event",{hasText:"Design review"}).first().waitFor();
  }
  if(!await page.locator(".calendar-event",{hasText:"Conference"}).count()){
    await page.getByRole("button",{name:"Calendar settings"}).first().click();await page.getByRole("tab",{name:"Import and export"}).click();
    await page.getByLabel("Or paste content").fill(ics);await page.getByRole("button",{name:"Import ICS"}).click();await page.getByText("3 imported; 0 failed.").waitFor({timeout:20000});
    await page.getByRole("button",{name:"Close settings"}).click();
  }
  await context.close();
}
// React measures the month rows only when the view changes after its live connection is
// ready (a bug in main Dar); switching views makes both clients measure the same grid.
const remeasure=async page=>{await page.getByRole("radio",{name:"Week"}).click();await page.locator(".calendar-time-grid").waitFor();await page.getByRole("radio",{name:"Month"}).click();await page.locator(".calendar-month").waitFor();};
async function calendarStates(base,name){
  for(const scheme of ["light","dark"]){
    const {context,page}=await open(base,{width:1440,height:950},scheme);await signIn(base,page,at+"month");
    await page.locator(".calendar-event",{hasText:"Conference"}).first().waitFor();await remeasure(page);
    await shot(page,`calendar-month-${scheme}-${name}`);if(scheme==="dark"){await context.close();continue;}
    for(const view of ["week","day","agenda"]){await page.goto(base+at+view);await page.locator(".calendar-event",{hasText:"Lunch and learn"}).first().waitFor();await shot(page,`calendar-${view}-${name}`);}
    await page.goto(base+at+"week");await page.locator(".calendar-event",{hasText:"Design review"}).first().click();await page.locator(".event-title h2").waitFor();await page.locator(".pim-detail-footer").waitFor();await shot(page,`calendar-details-${name}`);
    await page.getByRole("button",{name:"Edit event"}).click();await page.locator(".event-editor").waitFor();await shot(page,`calendar-editor-${name}`);await page.getByRole("button",{name:"Close editor"}).click();
    await page.locator(".calendar-event",{hasText:"Standup"}).first().click();await page.locator(".event-title h2",{hasText:"Standup"}).waitFor();await page.getByRole("button",{name:"Edit event"}).click();await page.getByRole("dialog").waitFor();await shot(page,`calendar-scope-${name}`);await page.getByRole("dialog").getByRole("button",{name:"Cancel"}).click();
    await page.getByRole("button",{name:"Add calendar"}).click();await page.getByRole("menuitem").first().waitFor();await shot(page,`calendar-add-menu-${name}`);await page.keyboard.press("Escape");
    await page.getByRole("button",{name:"Choose date"}).click();await page.locator(".date-popover").waitFor();await page.waitForTimeout(300);await shot(page,`calendar-date-popover-${name}`);await page.keyboard.press("Escape");
    await page.getByRole("button",{name:"Calendar settings"}).first().click();await page.locator(".settings-sheet").waitFor();await shot(page,`calendar-settings-${name}`);await page.getByRole("button",{name:"Close settings"}).click();
    await page.getByRole("button",{name:/^Reminders/}).click();await page.getByRole("dialog").waitFor();await shot(page,`calendar-reminders-${name}`);await page.keyboard.press("Escape");
    await page.setViewportSize({width:390,height:844});await page.goto(base+at+"month");await page.locator(".calendar-month").waitFor();await remeasure(page);await shot(page,`calendar-phone-${name}`);
    await context.close();
  }
}
await Promise.all([react,rust].map(calendarContent));
for(const [base,name] of [[react,"react"],[rust,"rust"]])await calendarStates(base,name);

// Rust side behaviour on a fresh daily series (away from the screenshot weeks, so reruns
// start clean): lanes, move an occurrence, cancel one, export, live update, delete.
{
  const {context,page}=await open(rust);await signIn(rust,page,at+"week");
  const step=async(name,fn)=>{process.stdout.write(name+" … ");await fn();console.log("ok");};
  const event=title=>page.locator(".calendar-event",{hasText:title});
  const title=`Series ${Date.now().toString(36)}`,week="/calendar?date=2027-01-04&view=week";
  await step("overlapping events share lanes",async()=>{await event("Lunch and learn").waitFor();assert.equal(await page.locator(".calendar-event-wrap.ev-lanes-2").count(),2);});
  await step("create a daily series",async()=>{await page.goto(rust+week);await page.locator(".calendar-time-grid").waitFor();
    await page.getByRole("button",{name:"New event 2027-01-04 08:00"}).click();await page.getByPlaceholder("Add title").fill(title);
    await page.getByRole("combobox",{name:"Repeats"}).click();await page.getByRole("option",{name:"Daily"}).click();await page.getByRole("button",{name:"Save event"}).click();
    await page.waitForFunction(t=>[...document.querySelectorAll(".calendar-event")].filter(e=>e.textContent.includes(t)).length===7,title);});
  await step("move one occurrence",async()=>{await event(title).nth(2).click();await page.getByRole("button",{name:"Edit event"}).click();await page.getByRole("button",{name:"This event only"}).click();
    await page.getByLabel("Start time").fill("11:00");await page.getByRole("button",{name:"Save event"}).click();await event(title).filter({hasText:"11:00"}).waitFor();assert.equal(await event(title).count(),7);});
  await step("cancel one occurrence",async()=>{await event(title).first().click();await page.getByRole("button",{name:"Delete event"}).click();await page.getByRole("button",{name:"This event only"}).click();
    await page.waitForFunction(t=>[...document.querySelectorAll(".calendar-event")].filter(e=>e.textContent.includes(t)).length===6,title);});
  await step("export keeps the exceptions",async()=>{await event(title).first().click();const href=await page.getByRole("link",{name:"Export event"}).getAttribute("href");const text=await (await page.request.get(rust+href)).text();
    assert.match(text,/RRULE:FREQ=DAILY/);assert.match(text,/STATUS:CANCELLED/);assert.match(text,/RECURRENCE-ID[^:\r]*:20270106T080000Z?\r/);assert.match(text,/DTSTART[^:\r]*:20270106T110000Z?\r/);});
  await step("live update from another tab",async()=>{const other=await context.newPage();await other.goto(rust+week);await other.locator(".calendar-time-grid").waitFor({timeout:30000});
    await other.getByRole("button",{name:"New event 2027-01-07 16:00"}).click();await other.getByPlaceholder("Add title").fill("Live event");await other.getByRole("button",{name:"Save event"}).click();await other.locator(".calendar-event",{hasText:"Live event"}).waitFor();
    await event("Live event").waitFor({timeout:10000});await other.close();});
  await step("delete an event and the series",async()=>{for(const name of ["Live event",title]){await event(name).first().click();await page.getByRole("button",{name:"Delete event"}).click();
    if(name===title)await page.getByRole("button",{name:"Whole series"}).click();await page.getByRole("dialog").getByRole("button",{name:"Delete permanently"}).click();await event(name).first().waitFor({state:"detached"});}});
  await context.close();
}
await browser.close();
writeFileSync(out+"/pim-log.txt",errors.join("\n"));console.log(errors.length?"page errors:\n"+errors.join("\n"):"no page errors");if(errors.length)process.exitCode=1;
