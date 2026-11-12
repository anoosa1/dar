import assert from "node:assert/strict";
import {chromium} from "@playwright/test";
import path from "node:path";
import {login} from "./integration-helpers.ts";
const base=process.env.DAR_INTEGRATION_ORIGIN;if(!base)throw Error("Use an isolated fixture.");
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true});let page,expectedConflict=false;const errors=[];
try{
 const bootstrap=await login(base),settingsUrl=base+"/api/v1/apps/org.dar.prayer-times/userData/settings",old=await(await fetch(settingsUrl,{headers:bootstrap.headers})).json();if(old)assert.equal((await fetch(settingsUrl,{method:"DELETE",headers:{...bootstrap.headers,"if-match":`"${old.revision}"`}})).status,200);
 const context=await browser.newContext({viewport:{width:1440,height:950}});page=await context.newPage();page.on("pageerror",e=>errors.push(e.message));page.on("console",m=>{if(m.type()==="error"&&!/401.*Unauthorized/.test(m.text())&&!(expectedConflict&&/409/.test(m.text())))errors.push(m.text());});
 await page.clock.install({time:new Date("2026-11-08T18:00:00Z")});
 await page.goto(base);await page.getByLabel("Username",{exact:true}).fill("admin");await page.getByLabel("Password",{exact:true}).fill("a long isolated test password");await page.getByRole("button",{name:"Sign in",exact:true}).click();await page.getByRole("navigation",{name:"Applications",exact:true}).getByRole("link",{name:"Prayer Times",exact:true}).click();
 // Setup uses the settings sheet: nothing saves until the location and method are complete.
 const session=await(await page.request.get(base+"/api/v1/auth/session")).json(),endpoint=settingsUrl;
 await page.getByRole("button",{name:"Set up Prayer Times",exact:true}).click();let editor=page.getByRole("dialog",{name:"Prayer Times settings",exact:true});
 await editor.getByLabel("Location name",{exact:true}).fill("Toronto");await editor.getByLabel("Latitude",{exact:true}).fill("43.6532");await editor.getByLabel("Longitude",{exact:true}).fill("-79.3832");await editor.getByLabel("Timezone",{exact:true}).fill("America/Toronto");
 await editor.getByRole("button",{name:"Use my location",exact:true}).click();await editor.getByText("Location was not available. Enter coordinates manually.",{exact:true}).waitFor();assert.equal(await editor.getByLabel("Latitude",{exact:true}).inputValue(),"43.6532");
 assert.equal(await(await page.request.get(endpoint)).json(),null);
 await editor.getByRole("tab",{name:"Calculation",exact:true}).click();await editor.getByRole("radio",{name:/^ISNA\b/}).check();await editor.getByText("Method set to ISNA",{exact:true}).waitFor();
 const persisted=await(await page.request.get(endpoint)).json();assert.equal(persisted.value.location.latitude,43.6532);assert.equal(persisted.revision,1);
 // Today band and week table appear behind the sheet as soon as the first save lands.
 await page.getByRole("heading",{level:1,name:/ at /}).waitFor();assert.equal(await page.locator(".prayer-week tbody tr").count(),7);
 // Each valid change saves, marks the affected times and can be undone.
 await editor.getByRole("radiogroup",{name:"Asr convention",exact:true}).getByRole("radio",{name:"Hanafi",exact:true}).check();await editor.getByText("Asr set to Hanafi",{exact:true}).waitFor();assert.equal(await page.locator(".prayer-week td.is-changed").count(),7);assert.equal((await(await page.request.get(endpoint)).json()).value.calculation.asrRule,"hanafi");
 await editor.getByRole("button",{name:"Undo",exact:true}).click();await editor.getByText("Change undone",{exact:true}).waitFor();assert.equal(await page.locator(".prayer-week td.is-changed").count(),0);assert.equal((await(await page.request.get(endpoint)).json()).value.calculation.asrRule,"standard");
 // Invalid text keeps its error and is never saved.
 await editor.getByRole("tab",{name:"Location",exact:true}).click();const longitude=editor.getByLabel("Longitude",{exact:true});await longitude.fill("-279.3832");await longitude.press("Enter");await editor.getByText("Between −180 and 180. Not saved.",{exact:true}).waitFor();assert.equal(await longitude.getAttribute("aria-invalid"),"true");assert.equal((await(await page.request.get(endpoint)).json()).value.location.longitude,-79.3832);
 await page.keyboard.press("Escape");await editor.waitFor({state:"hidden"});
 await page.getByRole("button",{name:"Today",exact:true}).first().click();
 const details=page.getByRole("complementary",{name:"Day details",exact:true});await details.waitFor();await details.getByLabel("Prayer timetable",{exact:true}).waitFor();assert.equal(await details.locator(".prayer-time-row").count(),6);assert.match(await details.textContent(),/Asr/);
 await page.reload();await page.getByRole("button",{name:"Today",exact:true}).last().click();await details.waitFor();assert.equal(await details.locator(".prayer-time-row").count(),6);
 for(const viewport of [{width:1440,height:950},{width:900,height:700},{width:780,height:800},{width:390,height:568}]){
  await page.setViewportSize(viewport);assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);assert.equal(await page.evaluate(()=>document.documentElement.scrollHeight>innerHeight),false);
  await details.getByRole("button",{name:"Edit prayer settings",exact:true}).scrollIntoViewIfNeeded();assert.equal(await page.locator(".topbar").evaluate(el=>el.getBoundingClientRect().top),viewport.width>800?0:8);
  await page.screenshot({path:path.join(process.env.DAR_TEST_ROOT,`prayer-times-${viewport.width}.png`),fullPage:true});
 }
 // Phones show settings as a bottom sheet below the search bar. Like other phone panes, it keeps the 8px page scrollbar gutter.
 await details.getByRole("button",{name:"Edit prayer settings",exact:true}).click();editor=page.getByRole("dialog",{name:"Prayer Times settings",exact:true});await editor.waitFor();await editor.evaluate(el=>Promise.all(el.getAnimations().map(a=>a.finished)));const sheet=await editor.boundingBox(),usable=await page.evaluate(()=>document.documentElement.clientWidth);assert.ok(sheet.x<=1&&sheet.x+sheet.width>=usable-9&&sheet.y>=90&&sheet.y+sheet.height<=569,JSON.stringify({sheet,usable}));assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);await page.screenshot({path:path.join(process.env.DAR_TEST_ROOT,"prayer-times-settings-390.png")});await editor.getByRole("button",{name:"Close settings",exact:true}).click();await editor.waitFor({state:"hidden"});
 await details.getByRole("button",{name:"Back to week",exact:true}).click();await page.locator(".prayer-day").first().focus();await page.keyboard.press("Enter");await details.waitFor();assert.equal(await details.getByRole("button",{name:"Back to week",exact:true}).evaluate(el=>el===document.activeElement),true);await details.getByRole("button",{name:"Back to week",exact:true}).click();assert.equal(await page.locator(".prayer-day").first().evaluate(el=>el===document.activeElement),true);
 // A change saved elsewhere is a conflict with Reload; the sheet then shows the saved version.
 await page.locator(".prayer-day").first().click();await details.getByRole("button",{name:"Edit prayer settings",exact:true}).click();editor=page.getByRole("dialog",{name:"Prayer Times settings",exact:true});const label=editor.getByLabel("Location name",{exact:true});await label.fill("Unsaved change");
 const current=await(await page.request.get(endpoint)).json(),changed=structuredClone(current.value);changed.location.label="Saved elsewhere";assert.equal((await page.request.put(endpoint,{headers:{origin:base,"x-csrf-token":session.csrfToken,"if-match":`"${current.revision}"`},data:{schemaVersion:1,value:changed}})).status(),200);
 expectedConflict=true;await label.press("Enter");await editor.getByRole("button",{name:"Reload saved settings",exact:true}).click();await page.waitForFunction(()=>document.querySelector("[data-settings-sheet] input")?.value==="Saved elsewhere");assert.equal(await label.inputValue(),"Saved elsewhere");expectedConflict=false;
 await editor.getByRole("button",{name:"Close settings",exact:true}).click();await page.clock.setFixedTime(new Date("2026-11-09T04:30:00Z"));await page.clock.runFor(1100);await details.getByText(/Next: Fajr/).waitFor();
 await page.setViewportSize({width:1440,height:950});await page.goto(base+"/settings?section=installedApps");await page.getByRole("heading",{name:"Apps",exact:true}).waitFor();await page.getByRole("heading",{name:"Prayer Times",exact:true}).waitFor();
 assert.deepEqual(errors,[]);console.info("PASS separately installed Prayer Times: shared React SDK, settings sheet setup, save-as-you-go with Undo, invalid input never saved, location denial, desktop/tablet/phone, bottom sheet, focus, conflicts, next-day Fajr and admin inventory.");
}catch(error){if(page){console.error("Prayer Times state",await page.locator("h1,h2,[role=alert]").allTextContents());await page.screenshot({path:path.join(process.env.DAR_TEST_ROOT,"prayer-times-failure.png"),fullPage:true});}throw error;}finally{await browser.close();}
