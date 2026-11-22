import assert from "node:assert/strict";
import path from "node:path";
import {readFile,writeFile} from "node:fs/promises";
import {chromium} from "@playwright/test";
const base=process.env.DAR_INTEGRATION_ORIGIN,root=process.env.DAR_TEST_ROOT,control=process.env.DAR_CHAT_CONTROL;if(!base||!root||!control)throw Error("Use the isolated Chat fixture.");
// Chrome's fake microphone (a beeping tone) stands in for a person speaking in a call.
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true,args:["--use-fake-device-for-media-stream","--use-fake-ui-for-media-stream","--autoplay-policy=no-user-gesture-required"]}),
  context=await browser.newContext({viewport:{width:1440,height:950},permissions:["microphone"]}),page=await context.newPage(),errors=[];
page.on("pageerror",e=>errors.push(e.message));page.on("console",m=>{if(m.type()==="error"&&!/Failed to load resource/.test(m.text()))errors.push(m.text());});
page.setDefaultTimeout(15000);
const overview=()=>page.evaluate(async()=>(await fetch("/api/v1/chat/overview")).json());
try{
  await page.goto(`${base}/chat`);await page.getByLabel("Username",{exact:true}).fill("admin");await page.getByLabel("Password",{exact:true}).fill("a long isolated test password");await page.getByRole("button",{name:"Sign in",exact:true}).click();
  await page.getByRole("link",{name:"Chat",exact:true}).first().waitFor();
  await page.getByRole("heading",{name:"No accounts linked"}).waitFor();
  // Link with a QR code drawn in the editing pane; the fake bridge "scans" it on request.
  await page.getByRole("button",{name:"Link an account"}).first().click();const pane=page.locator("[data-editor-pane]");
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
  const log=chat.getByRole("log",{name:"Messages in Sara Haddad"});await log.getByText("Are we still on for dinner?").waitFor();await log.locator(".chat-quote").getByText("Yes!").waitFor();
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
  // The shared search finds the message; its result opens the chat with the message outlined.
  await page.getByRole("button",{name:"Close chat",exact:true}).click();await page.keyboard.press("Control+k");
  const search=page.getByRole("combobox",{name:"Search all apps",exact:true});await search.fill("see you at 7 then");
  const results=page.getByRole("listbox",{name:"Search results",exact:true});await results.getByRole("option",{name:/Sara Haddad/}).click();
  await page.locator(".chat-message.is-found").filter({hasText:"Perfect, see you at 7 then"}).waitFor();
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
  // Discord: a second account tab, signed in by QR code with the terms notice.
  await page.evaluate(()=>{document.documentElement.dataset.theme="mocha";});await page.setViewportSize({width:1440,height:950});await page.waitForTimeout(200);
  await page.getByRole("tablist",{name:"Accounts"}).getByRole("button",{name:"Link an account"}).click();const link=page.locator("[data-editor-pane]");
  await link.getByRole("radio",{name:"Discord"}).click();await link.getByText("Discord does not allow other apps on your account").waitFor();
  assert.equal(await link.getByRole("radio",{name:"QR code"}).count(),0,"Discord has no pairing code");
  await link.getByRole("button",{name:"Show QR code"}).click();await link.getByRole("img",{name:"QR code to scan with Discord"}).waitFor();
  await link.getByText(/Waiting for the Discord app/).waitFor();await page.screenshot({path:path.join(root,"chat-discord-link.png")});
  const dc=(await overview()).accounts.find(a=>a.platform==="discord");
  await writeFile(path.join(control,`${dc.id}.scan`),"");await link.getByText(/Scanned by anas/).waitFor();
  await writeFile(path.join(control,`${dc.id}.pair`),"");await link.waitFor({state:"detached"});
  const dcTab=page.getByRole("tab",{name:/Discord/});await dcTab.waitFor();assert.equal(await dcTab.getAttribute("aria-selected"),"true");
  // Servers with their channels in the sidebar; direct messages in the list.
  const servers=page.getByRole("navigation",{name:"Servers"});await servers.getByRole("button",{name:/Homelab Club/}).waitFor();
  await servers.getByRole("button",{name:/^Voice channel Lounge/}).waitFor();await servers.getByRole("button",{name:/Channel off-topic.*muted/}).waitFor();
  await page.getByRole("list",{name:"Direct messages"}).getByRole("button",{name:/Lina K/}).waitFor();
  // A channel replaces the list sheet; messages are flat rows.
  await servers.getByRole("button",{name:/Channel general, 1 mentions/}).click();
  const channel=page.getByRole("complementary",{name:"general"}),rows=channel.getByRole("log",{name:"Messages in #general"});
  await rows.getByText("Anyone moved to the new caddy module?").waitFor();assert.equal(await page.locator(".chat-main").isVisible(),false);
  await channel.getByRole("heading",{name:"#general"}).waitFor();await channel.getByText("Say hello").waitFor();
  await page.waitForFunction(()=>document.querySelector(".chat-conversation.is-flat .chat-photo img")?.naturalWidth===4);
  // Reactions are counts; the account's own toggles off and others stay.
  const painless=rows.locator(".chat-message").filter({hasText:"Yes, painless"});
  await painless.getByRole("button",{name:/^👍 3, You and 2 others\. Remove your reaction/}).click();
  await painless.getByRole("button",{name:/^👍 2\. React with 👍/}).waitFor();await painless.getByText(":party:").waitFor();
  const dcBox=channel.getByLabel("Message #general");await dcBox.fill("hello from Dar");await dcBox.press("Enter");
  await rows.locator(".chat-message").filter({hasText:"hello from Dar"}).waitFor();assert.equal(await dcBox.inputValue(),"");
  await page.screenshot({path:path.join(root,"chat-discord-channel.png")});
  // Channel info: topic and server; Discord chats mute but do not pin or archive.
  await channel.getByRole("button",{name:"Channel info"}).click();const dcInfo=page.getByRole("region",{name:"Chat info"});
  await dcInfo.getByRole("heading",{name:"Channel info"}).waitFor();await dcInfo.getByText(/Homelab Club · Text channels · Discord/).waitFor();
  assert.equal(await dcInfo.getByRole("button",{name:"Pin"}).count(),0);await dcInfo.getByRole("button",{name:"Close chat info"}).click();
  // Servers collapse; Direct messages brings back the list.
  await servers.getByRole("button",{name:/Homelab Club/}).click();assert.equal(await servers.getByRole("button",{name:/Channel general/}).count(),0);
  await page.getByRole("navigation",{name:"Direct messages"}).getByRole("button",{name:/Direct messages/}).click();await page.locator(".chat-main").waitFor();
  await page.getByRole("list",{name:"Direct messages"}).getByRole("button",{name:/Lina K/}).click();await page.getByRole("log",{name:"Messages in Lina K"}).getByText("Hopping into Lounge later?").waitFor();
  // Voice: who is in Lounge shows under it; opening it shows the call view, and joining sends the
  // microphone as Opus and plays the others (the fake bridge echoes it back as Lina).
  await servers.getByRole("button",{name:/Homelab Club/}).click();
  await servers.getByRole("button",{name:"Voice channel Lounge, 2 in voice"}).click();
  await servers.getByRole("list",{name:"In voice"}).getByText("Lina K").waitFor();
  const stage=page.getByRole("region",{name:"Voice channel Lounge"});
  await stage.getByRole("heading",{name:"2 people are in Lounge"}).waitFor();await page.screenshot({path:path.join(root,"chat-voice-join.png")});
  await stage.getByRole("button",{name:"Join voice"}).click();
  const dock=page.locator(".chat-sidebar").getByRole("region",{name:"Voice call"});await dock.getByText("Voice connected").waitFor();
  await stage.getByRole("list",{name:"In the call"}).getByText("Anas (you)").waitFor();
  await servers.getByRole("button",{name:"Voice channel Lounge, you are in this call"}).waitFor();
  const bridgeLog=async()=>(await readFile(path.join(control,"log.jsonl"),"utf8")).split("\n").filter(Boolean).map(line=>JSON.parse(line));
  for(let i=0;i<100&&!(await bridgeLog()).some(e=>e.method==="voiceAudio");i++)await page.waitForTimeout(100);
  assert.ok((await bridgeLog()).some(e=>e.method==="voiceAudio"&&e.bytes>0),"the microphone reached the bridge as Opus");
  await stage.locator(".chat-voice-tile.is-speaking").filter({hasText:"Lina K"}).waitFor();
  await page.screenshot({path:path.join(root,"chat-voice-call.png")});
  // The call code of an encrypted call; mute and deafen; the dock stays while reading a text channel.
  await stage.getByRole("button",{name:"End-to-end encrypted"}).click();const code=page.getByRole("dialog",{name:"Verify the call is private"});
  await code.getByText("48312").waitFor();await code.getByRole("button",{name:"Close",exact:true}).click();
  await dock.getByRole("button",{name:"Mute"}).click();await page.waitForFunction(()=>document.querySelector(".chat-sidebar .chat-voice-dock [aria-label=Mute]")?.getAttribute("aria-pressed")==="true");
  assert.ok((await bridgeLog()).some(e=>e.method==="voiceSet"&&e.muted===true));
  await stage.locator(".chat-voice-tile").filter({hasText:"Anas (you)"}).getByTitle("Muted").waitFor();
  await servers.getByRole("button",{name:/Channel general/}).click();await page.getByRole("log",{name:"Messages in #general"}).waitFor();await dock.getByText("Lounge · Homelab Club").waitFor();
  await page.screenshot({path:path.join(root,"chat-voice-dock.png")});
  for(const theme of ["latte","mocha"]){await page.evaluate(value=>{document.documentElement.dataset.theme=value;},theme);
    for(const width of [1440,390]){await page.setViewportSize({width,height:844});await page.waitForTimeout(300);
      const overflow=await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth?[...document.querySelectorAll("body *")].filter(e=>e.getBoundingClientRect().right>innerWidth+1&&!e.closest(".sidebar")).slice(0,8).map(e=>e.tagName+"."+[...e.classList].join(".")):[]);
      assert.deepEqual(overflow,[],`Discord ${theme} ${width} overflow`);
      await page.screenshot({path:path.join(root,`chat-discord-${width}-${theme}.png`)});
    }}
  // On a phone the call view fills the screen and the dock sits under the list.
  await page.setViewportSize({width:390,height:844});
  await page.getByRole("button",{name:"Back to chats"}).click();const phoneDock=page.locator(".chat-main").getByRole("region",{name:"Voice call"});await phoneDock.waitFor();
  await page.screenshot({path:path.join(root,"chat-voice-phone-dock.png")});
  await phoneDock.getByRole("button",{name:/Voice connected/}).click();await stage.getByRole("button",{name:"Leave voice"}).waitFor();
  await page.screenshot({path:path.join(root,"chat-voice-phone-call.png")});
  await stage.getByRole("button",{name:"Leave voice"}).click();await stage.getByRole("button",{name:"Join voice"}).waitFor();
  assert.equal(await page.getByRole("region",{name:"Voice call"}).count(),0);
  assert.ok((await bridgeLog()).some(e=>e.method==="voiceLeave"));
  assert.deepEqual(errors,[]);
  console.log("PASS Chat browser: link by QR code, account tab and views, imported chats, conversation with quotes, send with ticks, live incoming message, reply menu, settings sheet, phone layout and both themes; Discord sign-in with the terms notice, server tree, flat channel rows, reactions as counts, channel info and DMs; voice members, call view, joining with the microphone as Opus, others' audio with speaking rings, call code, mute, the dock while reading, the phone dock and leaving.");
}finally{await browser.close();}
