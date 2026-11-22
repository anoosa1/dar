// Drives the React client (reference, :4421) and the Rust client (:4422) against the same
// isolated Dar instance and screenshots matching states. Usage: node compare.mjs <out dir>
import {createRequire} from "node:module";
import {mkdirSync,readFileSync,writeFileSync} from "node:fs";
import {execSync} from "node:child_process";
const require=createRequire(new URL("../../../next/package.json",import.meta.url));
const {chromium}=require("@playwright/test");
const out=process.argv[2];mkdirSync(out,{recursive:true});
const react=process.env.DAR_REACT_ORIGIN,rust=process.env.DAR_RUST_ORIGIN;if(!react||!rust)throw Error("Set DAR_REACT_ORIGIN and DAR_RUST_ORIGIN (an isolated instance; see run.sh).");

const password="a long isolated test password";
const browser=await chromium.launch({executablePath:execSync("which chromium").toString().trim(),headless:true});
const log=[];
async function open(base,viewport={width:1440,height:950},scheme="light"){
  const context=await browser.newContext({viewport,colorScheme:scheme,locale:"en-US",timezoneId:"UTC"});const page=await context.newPage();
  page.on("pageerror",e=>log.push(`${base} pageerror: ${e.message}`));page.on("console",m=>{if(m.type()==="error")log.push(`${base} console: ${m.text()}`);});
  return {context,page};
}
async function shot(page,name){await page.waitForTimeout(400);await page.screenshot({path:`${out}/${name}.png`});}
// Each origin may be its own instance (React on the TypeScript stack, the Rust client on the
// Rust stack) or one shared instance. Setup and content go through that origin's own client.
const roots={[react]:process.env.DAR_REACT_ROOT??process.env.DAR_ROOT,[rust]:process.env.DAR_RUST_ROOT??process.env.DAR_ROOT};
const initialized=async base=>(await (await fetch(base+"/api/v1/auth/status")).json()).initialized;
const fresh={[react]:!await initialized(react),[rust]:!await initialized(rust)};
for(const [base,name] of [[react,"react"],[rust,"rust"]])if(fresh[base]){const {context,page}=await open(base);await page.goto(base+"/");await page.getByRole("heading",{name:"Set up Dar"}).waitFor();await shot(page,`setup-${name}`);await context.close();}
async function prepare(base){
  const {context,page}=await open(base);await page.goto(base+"/");
  await page.getByRole("heading",{name:/^(Sign in|Set up Dar)$/}).or(page.locator(".app-rail")).first().waitFor({timeout:30000});
  if(await page.getByRole("heading",{name:"Set up Dar"}).isVisible()){
    await page.getByLabel("Display name").fill("Amina Rahman");await page.getByLabel("Username").fill("amina");await page.getByLabel("Password",{exact:true}).fill(password);
    await page.getByLabel("Instance setup key").fill(readFileSync(roots[base]+"/secrets/bootstrap-token","utf8").trim());
    await page.getByRole("button",{name:"Create your account"}).click();
  }else if(await page.getByLabel("Username").isVisible()){await page.getByLabel("Username").fill("amina");await page.getByLabel("Password",{exact:true}).fill(password);await page.getByRole("button",{name:"Sign in",exact:true}).click();}
  await page.locator(".files-upload:not([disabled])").waitFor({timeout:30000});await page.waitForTimeout(500);
  if(!await page.getByRole("button",{name:"Projects",exact:true}).isVisible()){
    await page.locator(".files-new-menu .ui-menu-trigger").click();await page.getByRole("menuitem",{name:"Folder"}).click();
    await page.getByLabel("Name",{exact:true}).fill("Projects");await page.getByRole("button",{name:"Create folder"}).click();
    await page.getByRole("button",{name:"Projects",exact:true}).waitFor();
    writeFileSync(out+"/notes.md","# Plans\n\nSome **bold** text and a [link](https://example.com).\n\n- [ ] first\n- [x] done\n");
    writeFileSync(out+"/budget.csv","item,cost\nrent,1200\n");
    await page.setInputFiles('input[aria-label="Choose files to upload"]',[out+"/notes.md",out+"/budget.csv"]);
    await page.getByRole("button",{name:"budget.csv",exact:true}).waitFor({timeout:20000});await page.getByRole("button",{name:"notes.md",exact:true}).waitFor();
  }
  if(!await page.getByRole("button",{name:"mock.png",exact:true}).isVisible()){
    const {copyFileSync}=await import("node:fs");copyFileSync(new URL("../../../docs/spacetimedb/ux-mock-A.png",import.meta.url),out+"/mock.png");
    await page.setInputFiles('input[aria-label="Choose files to upload"]',[out+"/mock.png"]);await page.getByRole("button",{name:"mock.png",exact:true}).waitFor({timeout:20000});
  }
  await context.close();
}
// Both instances get their content together, early in a minute, so "Today, 3:04 PM" matches.
if(new Date().getSeconds()>15)await new Promise(resolve=>setTimeout(resolve,(61-new Date().getSeconds())*1000));
await Promise.all([...new Set([react,rust])].map(prepare));
async function signIn(base,page){await page.goto(base+"/");await page.getByLabel("Username").fill("amina");await page.getByLabel("Password",{exact:true}).fill(password);await shot(page,`signin-${base===rust?"rust":"react"}`);await page.getByRole("button",{name:"Sign in",exact:true}).click();await page.getByRole("button",{name:"Projects",exact:true}).waitFor({timeout:30000});await page.locator(".files-upload:not([disabled])").waitFor();}
for(const scheme of ["light","dark"])for(const [base,name] of [[react,"react"],[rust,"rust"]]){
  const {context,page}=await open(base,{width:1440,height:950},scheme);await signIn(base,page);
  const tag=`${scheme}-${name}`;
  await page.mouse.move(0,0);await shot(page,`files-${tag}`);
  if(scheme==="dark")await context.close();
  if(scheme==="dark")continue;
  await page.getByRole("button",{name:"Actions for budget.csv"}).click();await shot(page,`menu-${tag}`);await page.keyboard.press("Escape");
  await page.getByRole("button",{name:"Actions for budget.csv"}).click();await page.getByRole("menuitem",{name:"Details"}).click();await page.locator(".details-panel h2").waitFor();await shot(page,`details-${tag}`);
  await page.getByRole("tab",{name:"Sharing"}).click();await shot(page,`sharing-${tag}`);
  await page.getByRole("button",{name:"Close details"}).click();
  await page.getByRole("button",{name:"Grid view"}).click();await shot(page,`grid-${tag}`);await page.getByRole("button",{name:"List view"}).click();
  await page.locator(".files-new-menu .ui-menu-trigger").click();await shot(page,`new-${tag}`);await page.keyboard.press("Escape");
  await page.getByRole("checkbox",{name:"Select notes.md"}).check();await shot(page,`selected-${tag}`);await page.getByRole("button",{name:"Clear selection"}).click();
  await page.getByRole("button",{name:"Open search"}).click();await page.getByRole("combobox",{name:"Search all apps"}).fill("notes");await page.waitForTimeout(1200);await shot(page,`search-${tag}`);await page.keyboard.press("Escape");await page.keyboard.press("Escape");
  await page.getByRole("button",{name:"notes.md",exact:true}).click();await page.locator(".file-text-editor").waitFor();await page.waitForTimeout(800);await shot(page,`editor-${tag}`);
  await page.getByRole("button",{name:"Preview",exact:true}).click();await shot(page,`editor-preview-${tag}`);await page.getByRole("button",{name:"Close editor"}).click();
  await page.getByRole("button",{name:"mock.png",exact:true}).click();await page.locator(".file-preview").waitFor();await shot(page,`preview-${tag}`);await page.getByRole("button",{name:"Close preview"}).click();
  await page.getByRole("button",{name:"Files settings"}).first().click();await page.locator(".settings-sheet").waitFor();await shot(page,`settings-${tag}`);await page.getByRole("button",{name:"Close settings"}).click();
  await page.getByRole("button",{name:"Deleted files"}).click();await page.waitForTimeout(600);await shot(page,`trash-${tag}`);
  await page.getByRole("button",{name:"Locked folder"}).click();await page.waitForTimeout(600);await shot(page,`locked-${tag}`);
  await page.getByRole("button",{name:"All files"}).first().click();await page.getByRole("button",{name:"Projects",exact:true}).click();await page.waitForTimeout(800);await shot(page,`folder-${tag}`);
  await page.setViewportSize({width:390,height:844});await page.goto(base+"/");await page.getByRole("button",{name:"Projects",exact:true}).waitFor();await shot(page,`phone-${tag}`);
  await context.close();
}
await browser.close();
writeFileSync(out+"/log.txt",log.join("\n"));console.log(log.join("\n")||"no page errors");
