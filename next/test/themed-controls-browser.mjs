import assert from "node:assert/strict";
import {readFile,mkdtemp} from "node:fs/promises";
import {tmpdir} from "node:os";
import path from "node:path";
import {chromium} from "@playwright/test";

// Exercise the built shared CSS without a service or any account data.
const css=await readFile(new URL("../dist/app.css",import.meta.url),"utf8");
const root=process.env.DAR_TEST_ROOT??await mkdtemp(path.join(tmpdir(),"dar-controls-"));
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true,ignoreDefaultArgs:["--hide-scrollbars"]});
try{
 const page=await browser.newPage();
 await page.setContent(`<style>${css}
 .samples{padding:20px;max-width:600px}.samples form{display:grid;gap:16px}.samples label{display:flex;flex-direction:row;align-items:center;margin:0}
 .scroll-sample{height:120px;overflow:scroll;border:1px solid var(--border);border-radius:12px;padding:12px}.scroll-sample p{height:240px;margin:0}
 </style><main class="samples"><h1>Quiet Twilight</h1><form>
 <label><input type="checkbox" name="selected" value="yes">Select file</label>
 <label><input type="checkbox" id="mixed">Some files selected</label>
 <label><input type="checkbox" disabled checked>Unavailable</label>
 <label><input type="radio" name="sort" value="name" checked>Name</label>
 <label><input type="radio" name="sort" value="date">Modified</label>
 <label class="ui-switch">Hidden files<input type="checkbox" role="switch"></label>
 <input type="file" aria-label="Import file"><input type="date" aria-label="Date" value="2026-11-08">
 <div class="scroll-sample" tabindex="0" aria-label="Scrollable files"><p>Notes.md<br>Planning.txt<br>Archive<br>Photos</p></div>
 </form></main>`);
 await page.locator("#mixed").evaluate(el=>{el.indeterminate=true;});
 const selected=page.getByRole("checkbox",{name:"Select file",exact:true});
 await selected.focus();await page.keyboard.press("Space");assert.equal(await selected.isChecked(),true);
 await page.getByRole("radio",{name:"Name",exact:true}).focus();await page.keyboard.press("ArrowRight");
 assert.equal(await page.getByRole("radio",{name:"Modified",exact:true}).isChecked(),true);
 assert.deepEqual(await page.locator("form").evaluate(el=>Object.fromEntries(new FormData(el))),{selected:"yes",sort:"date"});
 assert.equal(await page.getByRole("checkbox",{name:"Unavailable",exact:true}).isDisabled(),true);
 assert.equal(await page.locator("#mixed").evaluate(el=>el.indeterminate),true);
 await page.getByRole("switch",{name:"Hidden files",exact:true}).check();
 for(const theme of ["latte","mocha"]){
  await page.evaluate(theme=>{document.documentElement.dataset.theme=theme;},theme);
  for(const width of [1100,390]){
   await page.setViewportSize({width,height:800});
   assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
   const scroll=page.getByLabel("Scrollable files");await scroll.focus();await page.keyboard.press("ArrowDown");
   await page.waitForFunction(()=>document.querySelector(".scroll-sample").scrollTop>0);
   await page.screenshot({path:path.join(root,`controls-${theme}-${width}.png`)});
  }
 }
 await page.emulateMedia({forcedColors:"active"});
 assert.equal(await selected.evaluate(el=>getComputedStyle(el).appearance),"auto");
 await selected.focus();await page.keyboard.press("Space");assert.equal(await selected.isChecked(),false);
 await page.screenshot({path:path.join(root,"controls-forced-colors.png")});
 console.info(`PASS themed controls: keyboard selection, mixed/disabled states, form values, switches, scrolling, both themes, phone layout and forced colors. Artifacts: ${root}`);
}finally{await browser.close();}
