import assert from "node:assert/strict";
import {mkdir, writeFile} from "node:fs/promises";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {chromium} from "@playwright/test";
import {login, upload, connect} from "./integration-helpers.ts";
import {calendarDate, fileDetails, noteDetails} from "./ui-helpers.mjs";

// Run only against the temporary acceptance fixture. Screenshots accompany actual
// contrast/overflow checks and saved account appearance, rather than pixel goldens.
const base=process.env.DAR_INTEGRATION_ORIGIN, root=process.env.DAR_TEST_ROOT;
if(!base||!root)throw Error("Use an isolated acceptance fixture.");
const output=path.join(root,"twilight"),manifest=[],failures=[];
await mkdir(output,{recursive:true});
const admin=await login(base),marker="Twilight "+randomUUID().slice(0,6);
async function post(url,data){const response=await fetch(base+url,{method:"POST",headers:{...admin.headers,"content-type":"application/json"},body:JSON.stringify(data)});assert.equal(response.status,201,await response.clone().text());return response.json();}
await upload(base,admin,marker+".md","# A quieter week\n\nKeep room for the important things.\n");
const connection=await connect(admin);
try{for(let i=0;i<24;i++)await connection.reducers.createFolder({id:randomUUID(),parentId:"",name:`${marker} folder ${String(i+1).padStart(2,"0")}`});}finally{connection.disconnect();}
const notebook=await post("/api/v1/notes/notebooks",{name:marker,color:"#73396f"});
const note=await post("/api/v1/notes/items",{collectionId:notebook.id,title:marker,content:"# A quieter week\n\n- Plant herbs\n- Finish the chapter\n- Take a long walk\n"});
const longNotebookName="N".repeat(223)+randomUUID().replaceAll("-",""),longTaskListName="T".repeat(223)+randomUUID().replaceAll("-","");
await post("/api/v1/notes/notebooks",{name:longNotebookName,color:"#73396f"});
await post("/api/v1/tasks/lists",{name:longTaskListName,color:"#73396f"});
const tasklist=await post("/api/v1/tasks/lists",{name:marker,color:"#73396f"});
const task=await post("/api/v1/tasks/items",{collectionId:tasklist.id,title:marker,description:"Make room for the important things.",priority:5});
const calendar=await post("/api/v1/calendar/calendars",{name:marker,color:"#73396f"});
await post(`/api/v1/calendar/calendars/${calendar.id}/items`,{data:{title:marker,start:"2026-11-08T10:00",end:"2026-11-08T11:00",timezone:"UTC",allDay:false}});
const addressbook=await post("/api/v1/contacts/books",{name:marker,color:"#73396f"});
const contact=await post(`/api/v1/contacts/books/${addressbook.id}/items`,{data:{firstName:"Amira",lastName:marker,emails:[{label:"work",value:"amira@example.test"}],organization:"Design team"}});
const prayerUrl=base+"/api/v1/apps/org.dar.prayer-times/userData/settings";
const prayer=await(await fetch(prayerUrl,{headers:admin.headers})).json();
if(!prayer){const response=await fetch(prayerUrl,{method:"PUT",headers:{...admin.headers,"content-type":"application/json","if-match":'"0"'},body:JSON.stringify({schemaVersion:1,value:{schemaVersion:1,location:{label:"Toronto",latitude:43.6532,longitude:-79.3832,timezone:"America/Toronto"},calculation:{method:"ISNA",asrRule:"standard",highLatitudeRule:"angle",offsetMinutes:{fajr:0,sunrise:0,dhuhr:0,asr:0,maghrib:0,isha:0}}}})});assert.equal(response.status,200,await response.clone().text());}
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true});
let page,theme,width;
async function capture(name){
  // Inspect the composed background, including transparent nested controls.
  const issues=await page.evaluate(()=>{
    const canvas=document.createElement("canvas");canvas.width=canvas.height=1;const ctx=canvas.getContext("2d",{willReadFrequently:true});
    function rgba(value){ctx.clearRect(0,0,1,1);ctx.fillStyle=value;ctx.fillRect(0,0,1,1);return [...ctx.getImageData(0,0,1,1).data];}
    function over(top,bottom){const alpha=top[3]/255;return [...top.slice(0,3).map((v,i)=>v*alpha+bottom[i]*(1-alpha)),255];}
    function background(el){const c=rgba(getComputedStyle(el).backgroundColor);return c[3]===255?c:over(c,el.parentElement?background(el.parentElement):[255,255,255,255]);}
    function luminance(c){return c.slice(0,3).map(v=>{v/=255;return v<=.04045?v/12.92:((v+.055)/1.055)**2.4;}).reduce((s,v,i)=>s+v*[.2126,.7152,.0722][i],0);}
    const issues=[];
    for(const el of document.querySelectorAll("button,a,label,input,textarea,h1,h2,h3,p,td,th,strong,small,span")){
      if(!el.checkVisibility({checkOpacity:true,checkVisibilityCSS:true})||el.closest(":disabled,[aria-disabled=true],.sr-only")||getComputedStyle(el).opacity!=="1")continue;
      if(![...el.childNodes].some(n=>n.nodeType===3&&n.textContent.trim())&&!el.matches("input:not([type=checkbox],[type=radio],[type=color]),textarea"))continue;
      const rect=el.getBoundingClientRect();if(rect.bottom<=0||rect.top>=innerHeight||rect.right<=0||rect.left>=innerWidth)continue;
      const style=getComputedStyle(el),bg=background(el),fg=over(rgba(style.color),bg),a=luminance(fg),b=luminance(bg),ratio=(Math.max(a,b)+.05)/(Math.min(a,b)+.05);
      const large=parseFloat(style.fontSize)>=24||(parseFloat(style.fontSize)>=18.66&&parseFloat(style.fontWeight)>=700),min=large?3:4.5;
      if(ratio+.02<min)issues.push({text:(el.textContent||el.getAttribute("aria-label")||el.tagName).trim().slice(0,70),class:el.className,ratio:+ratio.toFixed(2),min,color:style.color,bg});
    }
    return {contrast:issues,overflow:document.documentElement.scrollWidth>innerWidth,verticalOverflow:document.documentElement.scrollHeight>innerHeight+1};
  });
  const filename=`${theme}-${width}-${name}.png`;
  await page.screenshot({path:path.join(output,filename),animations:"disabled",fullPage:true});
  manifest.push({filename,...issues});
  if(issues.overflow||issues.verticalOverflow||issues.contrast.length)failures.push({filename,...issues});
}
async function navigate(url,ready){await page.goto(base+url);await ready().waitFor();await page.waitForLoadState("networkidle");}
async function openSettings(app){await page.getByRole("button",{name:app+" settings",exact:true}).click();await page.getByRole("dialog",{name:app+" settings",exact:true}).waitFor();}
async function closeDialog(){await page.getByRole("button",{name:"Close dialog",exact:true}).click();}
try{
  const context=await browser.newContext({viewport:{width:1440,height:950}});page=await context.newPage();page.setDefaultTimeout(10000);
  const errors=[];page.on("pageerror",e=>errors.push(e.message));
  for(theme of ["latte","mocha"]){
    await page.emulateMedia({colorScheme:theme==="latte"?"light":"dark",reducedMotion:"reduce"});
    for(width of [1440,390]){
      await page.setViewportSize({width,height:width===390?844:950});
      await context.clearCookies();await navigate("/",()=>page.getByLabel("Username",{exact:true}));await capture("signin");
      await page.getByLabel("Username",{exact:true}).fill("admin");await page.getByLabel("Password",{exact:true}).fill("a long isolated test password");await page.getByRole("button",{name:"Sign in",exact:true}).click();await page.getByRole("navigation",{name:"Applications",exact:true}).waitFor();
      await navigate("/settings?section=appearance",()=>page.getByRole("radio",{name:"Follow system",exact:true}));
      await page.getByRole("radio",{name:theme==="latte"?"Light — Twilight Afterglow":"Dark — Twilight Nightfall",exact:true}).check();await page.getByRole("button",{name:"Save appearance",exact:true}).click();await page.getByText("Appearance saved",{exact:true}).waitFor();await page.reload();await page.waitForFunction(t=>document.documentElement.dataset.theme===t,theme);await page.getByRole("radio",{name:"Follow system",exact:true}).waitFor();await capture("appearance");
      await navigate("/",()=>page.getByRole("button",{name:"Actions for "+marker+".md",exact:true}));await capture("files");
      const fileScroll=await page.locator(".files-main").evaluate(el=>{el.scrollTop=0;const before=el.scrollTop;el.scrollTop=el.scrollHeight;return {before,after:el.scrollTop,height:el.clientHeight,total:el.scrollHeight};});
      assert.ok(fileScroll.total>fileScroll.height&&fileScroll.after>fileScroll.before,JSON.stringify(fileScroll));
      assert.equal(await page.locator(".file-table tbody tr").last().evaluate(el=>{const row=el.getBoundingClientRect(),pane=el.closest(".files-main").getBoundingClientRect();return row.top>=pane.top&&row.bottom<=pane.bottom+1;}),true);
      assert.equal(await page.locator(".topbar").evaluate(el=>el.getBoundingClientRect().top),width>800?0:8);
      if(width>800)assert.equal(await page.locator(".sidebar").evaluate(el=>el.getBoundingClientRect().top),56);
      assert.equal(await page.evaluate(()=>document.documentElement.scrollHeight>innerHeight+1),false);
      await page.locator(".files-main").evaluate(el=>{el.scrollTop=0;});
      if(width<801){await page.getByRole("button",{name:"More apps",exact:true}).click();await capture("launcher");await page.keyboard.press("Escape");}else await capture("launcher");
      await page.getByRole("button",{name:"Open account menu",exact:true}).click();await capture("account-menu");await page.keyboard.press("Escape");
      await page.getByRole("button",{name:"Actions for "+marker+".md",exact:true}).click();await capture("file-actions");await page.getByRole("menuitem",{name:"Rename",exact:true}).click();await page.getByLabel("Name",{exact:true}).waitFor();await capture("file-rename");await page.getByRole("button",{name:"Cancel",exact:true}).click();
      await fileDetails(page,marker+".md");
      const details=page.getByRole("complementary",{name:"File details",exact:true});
      await details.waitFor();await details.getByRole("heading",{name:marker+".md",exact:true}).waitFor();
      const bounds=await details.boundingBox();assert.ok(bounds.x>=0&&bounds.y>=0&&bounds.x+bounds.width<=width+1&&bounds.y+bounds.height<=page.viewportSize().height+1,JSON.stringify(bounds));
      if(width>900){
        const list=page.locator(".files-main");await list.evaluate(el=>{el.scrollTop=0;});
        const frame=await details.boundingBox(),before=await list.evaluate(el=>el.scrollTop);
        await list.evaluate(el=>{el.scrollTop=el.scrollHeight;});assert.ok(await list.evaluate(el=>el.scrollTop)>before);
        assert.deepEqual(await details.boundingBox(),frame);
      }
      await capture("file-details");await page.getByRole("button",{name:"Close details",exact:true}).click();
      await openSettings("Files");await capture("files-settings");await closeDialog();
      await page.getByRole("button",{name:"New",exact:true}).click();await capture("files-new-menu");await page.getByRole("menuitem",{name:"Folder",exact:true}).click();await page.getByLabel("Name",{exact:true}).waitFor();await capture("new-folder");await page.getByRole("button",{name:"Cancel",exact:true}).click();
      await navigate("/contacts",()=>page.getByRole("heading",{name:"All contacts",exact:true}));await capture("contacts");await navigate(`/contacts?book=${addressbook.id}&contact=${encodeURIComponent(contact.href)}`,()=>page.getByRole("button",{name:"Edit contact",exact:true}));await capture("contact-details");await page.getByRole("button",{name:"Edit contact",exact:true}).click();await page.getByLabel("First name",{exact:true}).waitFor();await capture("contact-editor");await page.getByRole("button",{name:"Cancel",exact:true}).click();await page.getByRole("button",{name:"Delete contact",exact:true}).click();await capture("contact-delete");await page.getByRole("button",{name:"Cancel",exact:true}).click();await page.getByRole("button",{name:"Close contact details",exact:true}).click();await openSettings("Contacts");await capture("contacts-settings");await closeDialog();
      await page.getByRole("button",{name:"New address book",exact:true}).click();await page.getByLabel("Name",{exact:true}).waitFor();await capture("new-address-book");await page.getByRole("button",{name:"Cancel",exact:true}).click();
      await navigate("/calendar",()=>page.getByRole("button",{name:"Choose date",exact:true}));await calendarDate(page,"2026-11-08");await capture("calendar");await page.getByRole("button",{name:"Choose date",exact:true}).click();await capture("calendar-date");await page.keyboard.press("Escape");await openSettings("Calendar");await capture("calendar-settings");await closeDialog();
      await page.getByRole("button",{name:"New event",exact:true}).click();await page.getByLabel("Title",{exact:true}).waitFor();await capture("new-event");await page.getByRole("button",{name:"Cancel",exact:true}).click();
      await navigate(`/notes?item=${note.id}`,()=>page.getByLabel("Note title",{exact:true}));await capture("notes-write");await noteDetails(page);await capture("notes-details");await closeDialog();await page.getByRole("button",{name:"Preview",exact:true}).click();await capture("notes-preview");
      await navigate("/notes",()=>page.getByRole("heading",{name:"All notes",exact:true}));await openSettings("Notes");await capture("notes-settings");await closeDialog();
      await navigate(`/tasks?item=${task.id}`,()=>page.getByRole("heading",{name:marker,exact:true}));await capture("task-details");
      await navigate("/tasks",()=>page.getByRole("heading",{name:"Today",exact:true}));await openSettings("Tasks");await capture("tasks-settings");await closeDialog();
      await page.getByRole("button",{name:"New task",exact:true}).click();await page.getByLabel("Title",{exact:true}).waitFor();await capture("new-task");await page.getByRole("button",{name:"Cancel",exact:true}).click();
      for(const [url,noun,name,menu,label] of [["notes","notebook",longNotebookName,"Delete notebook","Delete notebook"],["tasks","task list",longTaskListName,"Delete task list","Delete task list"]]){
        await navigate("/"+url,()=>page.getByRole("navigation",{name:"Applications",exact:true}));
        
        // Sidebar sizing determines collection pagination after its first layout.
        await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
        const targetAction=page.getByRole("button",{name:`Actions for ${noun} ${name}`,exact:true});
        for(let i=0;!await targetAction.isVisible()&&i<100;i++){const nextPage=page.getByRole("button",{name:`Next ${noun}s`,exact:true});assert.ok(await nextPage.isEnabled());await nextPage.click();}
        await targetAction.click();
        await page.getByRole("menuitem",{name:menu,exact:true}).click();
        const dialog=page.getByRole("dialog",{name:label+"?",exact:true});await dialog.waitFor();
        assert.ok((await dialog.textContent()).includes(name));assert.equal(await dialog.getByRole("button",{name:label,exact:true}).count(),1);
        assert.equal(await dialog.evaluate(el=>el.scrollWidth<=el.clientWidth+1),true);
        const action=await dialog.getByRole("button",{name:label,exact:true}).boundingBox(),box=await dialog.boundingBox();
        assert.ok(action.x>=box.x&&action.x+action.width<=box.x+box.width+1&&action.y+action.height<=page.viewportSize().height+1);
        await capture("delete-"+noun.replaceAll(" ","-")+"-long-name");await dialog.getByRole("button",{name:"Cancel",exact:true}).click();
      }
      await navigate("/apps/org.dar.prayer-times",()=>page.getByRole("heading",{name:"Prayer times",exact:true}));await page.locator(".prayer-day").first().waitFor();await capture("prayer-times");await page.locator(".prayer-day").first().click();await page.getByLabel("Prayer timetable",{exact:true}).waitFor();await capture("prayer-timetable");await page.getByRole("button",{name:"Back to week",exact:true}).click();await openSettings("Prayer Times");await capture("prayer-settings");await closeDialog();
      for(const section of ["profile","security","regional","connected","instanceAppearance","installedApps","applications","directory","scim","keys"]){await navigate("/settings?section="+section,()=>page.locator(".identity-main:not([aria-busy=true])"));await capture("settings-"+section);}
      await page.getByRole("button",{name:"Rotate signing key",exact:true}).click();await capture("admin-confirmation");await page.getByRole("button",{name:"Cancel",exact:true}).click();
    }
  }
  assert.deepEqual(errors,[]);
}finally{
  await writeFile(path.join(output,"manifest.json"),JSON.stringify({screenshots:manifest,failures},null,2));
  await browser.close();
}
assert.deepEqual(failures,[],"Visible text contrast and viewport overflow; see twilight/manifest.json");
console.info(`PASS Twilight: ${manifest.length} desktop/phone screenshots, visible text contrast, menus/forms/settings and appearance persistence. ${output}`);
