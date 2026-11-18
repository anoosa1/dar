import assert from "node:assert/strict";
import path from "node:path";
import {writeFile} from "node:fs/promises";
import {chromium} from "@playwright/test";
const base=process.env.DAR_INTEGRATION_ORIGIN,root=process.env.DAR_TEST_ROOT,control=process.env.DAR_CHAT_CONTROL;if(!base||!root||!control)throw Error("Use the isolated Chat fixture.");
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true}),context=await browser.newContext({viewport:{width:1440,height:950}}),page=await context.newPage(),errors=[];
page.on("pageerror",e=>errors.push(e.message));page.on("console",m=>{if(m.type()==="error"&&!/Failed to load resource/.test(m.text()))errors.push(m.text());});
page.setDefaultTimeout(15000);
const overview=()=>page.evaluate(async()=>(await fetch("/api/v1/chat/overview")).json());
try{
  await page.goto(`${base}/chat`);await page.getByLabel("Username",{exact:true}).fill("admin");await page.getByLabel("Password",{exact:true}).fill("a long isolated test password");await page.getByRole("button",{name:"Sign in",exact:true}).click();
  await page.getByRole("link",{name:"Chat",exact:true}).first().waitFor();
  await page.getByRole("heading",{name:"No accounts linked"}).waitFor();
  // Link with a QR code drawn in the editing pane; the fake bridge "scans" it on request.
  await page.getByRole("button",{name:"Link WhatsApp"}).click();const pane=page.locator("[data-editor-pane]");
  assert.equal(await pane.getByRole("radio",{name:"QR code"}).isChecked(),true);
  await pane.getByLabel("Account name").fill("Personal");await pane.getByRole("button",{name:"Show QR code"}).click();
  const qr=pane.getByRole("img",{name:"QR code to scan with WhatsApp"});await qr.waitFor();
  assert.ok((await qr.locator("path").getAttribute("d")).length>500,"the QR code has modules");
  await pane.getByText(/Waiting for scan/).waitFor();await page.screenshot({path:path.join(root,"chat-link.png")});
  const [account]=(await overview()).accounts;await writeFile(path.join(control,`${account.id}.pair`),"");
  await page.getByText("Linked. Your chats are on their way.").waitFor();await pane.waitFor({state:"detached"});
  // The account tab, its views and the imported chats.
  const tab=page.getByRole("tab",{name:/Personal/});await tab.waitFor();assert.equal(await tab.getAttribute("aria-selected"),"true");
  const list=page.getByRole("list",{name:"Chats"});await list.getByRole("button",{name:/Sara Haddad/}).waitFor();await list.getByRole("button",{name:/Family/}).waitFor();
  assert.equal(await list.getByRole("button",{name:/Old friend/}).count(),0,"archived chats are not in Chats");
  await page.getByRole("button",{name:/^Archived/}).click();await page.getByRole("list",{name:"Archived"}).getByRole("button",{name:/Old friend/}).waitFor();
  await page.getByRole("button",{name:/^Groups/}).click();await page.getByRole("list",{name:"Groups"}).getByRole("button",{name:/Family/}).waitFor();
  await page.getByRole("button",{name:/^Chats/}).click();
  // A conversation: history with a quote, then a sent message with its tick and a live incoming one.
  await list.getByRole("button",{name:/Sara Haddad/}).click();const chat=page.getByRole("complementary",{name:"Sara Haddad"});
  const log=chat.getByRole("log",{name:"Messages with Sara Haddad"});await log.getByText("Are we still on for dinner?").waitFor();await log.locator(".chat-quote").getByText("Yes!").waitFor();
  // The imported photo downloads through the bridge when shown.
  await page.waitForFunction(()=>document.querySelector(".chat-photo img")?.naturalWidth===4);
  const box=chat.getByLabel("Message Sara Haddad");await box.fill("See you at 7");await box.press("Enter");
  const sent=log.locator(".chat-message.is-out").filter({hasText:"See you at 7"});await sent.getByText("Sent").waitFor();assert.equal(await box.inputValue(),"");
  await writeFile(path.join(control,`${account.id}.incoming.json`),JSON.stringify({messages:[{id:"LIVE1",chat:"15550002222@s.whatsapp.net",sender:"15550002222@s.whatsapp.net",fromMe:false,at:Date.now(),kind:"text",text:"Perfect, see you at 7 then"}]}));
  await log.getByText("Perfect, see you at 7 then").waitFor();
  // Reply from the message menu.
  await log.locator(".chat-message.is-in").filter({hasText:"Perfect, see you at 7 then"}).hover();
  await log.locator(".chat-message.is-in").filter({hasText:"Perfect, see you at 7 then"}).getByRole("button",{name:"Message actions"}).click();await page.getByRole("menuitem",{name:"Reply"}).click();
  await chat.getByText("Replying to Sara Haddad").waitFor();await box.press("Escape");assert.equal(await chat.getByText("Replying to Sara Haddad").count(),0);
  // React from the message menu; the chip shows, and clicking it removes the reaction.
  const incoming=log.locator(".chat-message.is-in").filter({hasText:"Perfect, see you at 7 then"});await incoming.hover();
  await incoming.getByRole("button",{name:"Message actions"}).click();await page.getByRole("menuitem",{name:"React"}).click();await page.getByRole("menuitemradio",{name:"❤️"}).click();
  const chip=incoming.getByRole("button",{name:/Remove your reaction/});await chip.waitFor();
  // Chat info opens beside the conversation; mute goes through its menu.
  await chat.getByRole("button",{name:"Chat info"}).click();const infoSheet=page.getByRole("region",{name:"Chat info"});await infoSheet.getByRole("heading",{name:"Contact info"}).waitFor();
  // Add to Contacts saves the number in a Dar address book (created here through the API) and names the chat.
  const signIn=await fetch(base+"/api/v1/auth/login",{method:"POST",headers:{"content-type":"application/json",origin:base},body:JSON.stringify({username:"admin",password:"a long isolated test password"})});
  const api={cookie:signIn.headers.get("set-cookie").split(";")[0],origin:base,"x-csrf-token":(await signIn.json()).csrfToken,"content-type":"application/json"};
  assert.equal((await fetch(base+"/api/v1/contacts/books",{method:"POST",headers:api,body:JSON.stringify({name:"Chat test book",color:"#2563eb"})})).status,201);
  await infoSheet.getByText("Not in your contacts").waitFor();await infoSheet.getByRole("button",{name:"Add",exact:true}).click();
  const add=page.locator("[data-editor-pane]");await add.getByRole("heading",{name:"Add to Contacts"}).waitFor();assert.equal(await add.getByLabel("Name").inputValue(),"Sara Haddad");
  await add.getByRole("button",{name:"Add contact"}).click();await page.getByText("Sara Haddad added to Contacts").waitFor();await infoSheet.getByText("Dar contact").waitFor();
  await infoSheet.getByRole("button",{name:"Mute"}).click();await page.getByRole("menuitem",{name:"8 hours"}).click();await page.getByText("Chat muted").waitFor();await infoSheet.getByRole("button",{name:"Unmute"}).waitFor();
  await page.screenshot({path:path.join(root,"chat-desktop.png")});
  await chip.click();await chip.waitFor({state:"detached"});
  await infoSheet.getByRole("button",{name:"Close chat info"}).click();await infoSheet.waitFor({state:"detached"});
  // A document in the group: Save to Files from the message menu, into the shared folder picker.
  await page.getByRole("list",{name:"Chats"}).getByRole("button",{name:/Family/}).click();const group=page.getByRole("complementary",{name:"Family"});
  const document=group.locator(".chat-message").filter({hasText:"Eid lunch menu.pdf"});await document.waitFor();await document.hover();
  await document.getByRole("button",{name:"Message actions"}).click();await page.getByRole("menuitem",{name:"Save to Files…"}).click();
  const save=page.locator("[data-editor-pane]");await save.getByRole("heading",{name:"Save to Files"}).waitFor();await save.getByRole("button",{name:/^Save to/}).click();
  await page.getByText("Saved to Files as Eid lunch menu.pdf").waitFor();await save.waitFor({state:"detached"});
  await page.getByRole("list",{name:"Chats"}).getByRole("button",{name:/Sara Haddad/}).click();await chat.getByLabel("Message Sara Haddad").waitFor();
  // Settings sheet lists the account.
  await page.getByRole("button",{name:"Chat settings",exact:true}).first().click();const sheet=page.getByRole("dialog",{name:"Chat settings"});await sheet.getByText("Linked accounts").waitFor();await sheet.getByText("Personal",{exact:true}).waitFor();
  await sheet.getByRole("button",{name:"Close settings"}).click();await sheet.waitFor({state:"detached"});
  // Both themes, desktop and phone: no sideways scrolling; on phones the chat opens full screen with Back.
  for(const theme of ["latte","mocha"]){await page.evaluate(value=>{document.documentElement.dataset.theme=value;},theme);
    for(const width of [1440,390]){await page.setViewportSize({width,height:844});await page.waitForTimeout(300);
      const overflow=await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth?[...document.querySelectorAll("body *")].filter(e=>e.getBoundingClientRect().right>innerWidth+1&&!e.closest(".sidebar")).slice(0,8).map(e=>e.tagName+"."+[...e.classList].join(".")):[]);
      assert.deepEqual(overflow,[],`${theme} ${width} overflow`);
      if(width===390){
        const back=page.getByRole("button",{name:"Back to chats"});
        if(!await back.isVisible()){await page.getByRole("list",{name:"Chats"}).getByRole("button",{name:/Sara Haddad/}).click();}
        await back.waitFor();assert.equal(await page.locator(".chat-main").isVisible(),false);await chat.getByLabel("Message Sara Haddad").waitFor();
        await page.screenshot({path:path.join(root,`chat-phone-${theme}.png`)});await back.click();await page.locator(".chat-main").waitFor();
        await page.getByRole("tab",{name:/Personal/}).waitFor();await page.screenshot({path:path.join(root,`chat-phone-list-${theme}.png`)});
      }
    }}
  assert.deepEqual(errors,[]);
  console.log("PASS Chat browser: link by QR code, account tab and views, imported chats, conversation with quotes, send with ticks, live incoming message, reply menu, settings sheet, phone layout and both themes.");
}finally{await browser.close();}
