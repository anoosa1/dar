import assert from "node:assert/strict";
import path from "node:path";
import {chromium} from "@playwright/test";
const base=process.env.DAR_INTEGRATION_ORIGIN,root=process.env.DAR_TEST_ROOT,site=process.env.DAR_NEWS_SITE;if(!base||!root||!site)throw Error("Use the isolated News fixture.");
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true}),context=await browser.newContext({viewport:{width:1440,height:950}}),page=await context.newPage(),errors=[];
page.on("pageerror",e=>errors.push(e.message));page.on("console",m=>{if(m.type()==="error"&&!/Failed to load resource/.test(m.text()))errors.push(m.text());});
page.setDefaultTimeout(10000);
try{
  await page.goto(`${base}/news`);await page.getByLabel("Username",{exact:true}).fill("admin");await page.getByLabel("Password",{exact:true}).fill("a long isolated test password");await page.getByRole("button",{name:"Sign in",exact:true}).click();
  await page.getByRole("link",{name:"News",exact:true}).first().waitFor();
  await page.getByRole("heading",{name:"Unread",exact:true}).waitFor();await page.getByRole("heading",{name:"All caught up",exact:true}).waitFor();
  // Settings sheet: the API set "On scroll"; switch back and see it saved.
  await page.getByRole("button",{name:"News settings",exact:true}).first().click();const sheet=page.getByRole("dialog",{name:"News settings"});
  assert.equal(await sheet.getByRole("radio",{name:"On scroll"}).isChecked(),true);await sheet.getByRole("radio",{name:"On open"}).check();await sheet.getByText("Saved").first().waitFor();
  await sheet.getByRole("tab",{name:"Keyboard shortcuts"}).click();await sheet.getByText("Next unread article").waitFor();
  await sheet.getByRole("button",{name:"Close settings"}).click();await sheet.waitFor({state:"detached"});
  // All articles, grouped by day; opening one shows the sanitized body with proxied images.
  await page.getByRole("button",{name:"All articles"}).click();const list=page.getByRole("list",{name:"All articles articles"});
  await list.getByRole("button",{name:/Welcome to the fixture/}).click();
  const reader=page.getByRole("complementary",{name:"Article"});await reader.getByRole("heading",{name:"Welcome to the fixture"}).waitFor();await reader.getByText("Hello reader").waitFor();
  assert.match(await reader.locator(".news-body img").getAttribute("src"),/^\/api\/v1\/news\/image\?url=/);assert.equal(await reader.locator(".news-body script").count(),0);
  const star=reader.getByRole("button",{name:"Star",exact:true}),wasStarred=await star.getAttribute("aria-pressed");
  await page.locator("body").press("s");await page.waitForFunction(([before])=>document.querySelector(".news-reader-tools [aria-label=Star]")?.getAttribute("aria-pressed")!==before,[wasStarred]);
  await page.locator("body").press("k");await reader.getByRole("heading",{name:"Episode one"}).waitFor();await reader.locator("audio[src='https://media.example/ep1.mp3']").waitFor({state:"attached"});
  // Subscribe through discovery, then unsubscribe through the feed menu with confirmation.
  await page.getByRole("button",{name:"Subscribe",exact:true}).first().click();const pane=page.locator("[data-editor-pane]");
  await pane.getByLabel("Site or feed address").fill(site+"/atom.xml");await pane.getByRole("button",{name:"Find feeds"}).first().click();
  await pane.getByText("Found 1 feed").waitFor();await pane.getByText("Long read").waitFor();await pane.getByRole("button",{name:"Subscribe",exact:true}).click();
  await page.getByText("Subscribed to Atom Blog").waitFor();await page.getByRole("heading",{name:"Atom Blog",exact:true}).waitFor();await page.getByRole("list",{name:"Atom Blog articles"}).getByRole("button",{name:/Long read/}).waitFor();
  await page.getByRole("button",{name:"Actions for Atom Blog"}).first().click();await page.getByRole("menuitem",{name:"Unsubscribe…"}).click();
  const confirm=page.getByRole("dialog",{name:"Unsubscribe from Atom Blog?"});await confirm.getByRole("button",{name:"Unsubscribe"}).click();await confirm.waitFor({state:"detached"});
  await page.getByRole("heading",{name:"Unread",exact:true}).waitFor();assert.equal(await page.getByRole("button",{name:"Actions for Atom Blog"}).count(),0);
  // Folder with a failing imported feed shows its banner.
  await page.getByRole("button",{name:/^Blogs/}).first().click();await page.getByRole("heading",{name:"Blogs",exact:true}).waitFor();await page.getByText(/Elsewhere: last \d+ updates? failed/).waitFor();
  await page.screenshot({path:path.join(root,"news-desktop.png")});
  // Phones: chips, the list, a full-screen article with Back.
  for(const theme of ["latte","mocha"]){await page.evaluate(value=>{document.documentElement.dataset.theme=value;},theme);
    for(const width of [1440,390]){await page.setViewportSize({width,height:844});await page.getByRole("button",{name:"All articles"}).click();
      await page.waitForTimeout(300);const overflow=await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth?[...document.querySelectorAll("body *")].filter(e=>e.getBoundingClientRect().right>innerWidth+1&&!e.closest(".sidebar")).slice(0,8).map(e=>e.tagName+"."+[...e.classList].join(".")+" "+Math.round(e.getBoundingClientRect().right)):[]);
      assert.deepEqual(overflow,[],`${theme} ${width} overflow`);
      if(width===390){await page.getByRole("list",{name:"All articles articles"}).getByRole("button",{name:/Fresh arrival/}).click();const back=page.getByRole("button",{name:"Back to articles"});await back.waitFor();await page.getByRole("heading",{name:"Fresh arrival"}).waitFor();
        assert.equal(await page.locator(".news-main").isVisible(),false);await page.screenshot({path:path.join(root,`news-phone-${theme}.png`)});await back.click();await page.locator(".news-main").waitFor();}
    }}
  assert.deepEqual(errors,[]);
  console.log("PASS News browser: views, settings sheet, sanitized reader with proxied images, keyboard star/navigation, podcast enclosure, subscribe by discovery, confirmed unsubscribe, folders, phone reader and both themes.");
}finally{await browser.close();}
