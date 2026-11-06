import assert from "node:assert/strict";
import http from "node:http";
import path from "node:path";
import {chromium} from "@playwright/test";
import * as oidc from "openid-client";
import {randomUUID} from "node:crypto";

const base=process.env.DAR_INTEGRATION_ORIGIN;if(!base||!process.env.DAR_TEST_ROOT)throw new Error("Use an isolated acceptance instance.");
let config,redirect,lastTokens,lastClaims;const flows=new Map();
const rp=http.createServer((req,res)=>{void(async()=>{
  const url=new URL(req.url,"http://localhost");
  if(url.pathname==="/start"){
    const verifier=oidc.randomPKCECodeVerifier(),state=oidc.randomState(),nonce=oidc.randomNonce();flows.set(state,{verifier,nonce});
    const destination=oidc.buildAuthorizationUrl(config,{redirect_uri:redirect,response_type:"code",scope:"openid profile email groups offline_access",code_challenge:await oidc.calculatePKCECodeChallenge(verifier),code_challenge_method:"S256",state,nonce,prompt:"consent",...(url.searchParams.get("mode")==="form_post"?{response_mode:"form_post"}:{})});
    res.writeHead(302,{location:destination.href});res.end();return;
  }
  if(url.pathname==="/callback"){
    let callback=new URL(req.url,redirect);if(req.method==="POST"){const parts=[];for await(const part of req)parts.push(part);for(const [key,value] of new URLSearchParams(Buffer.concat(parts).toString()))callback.searchParams.set(key,value);}
    if(callback.searchParams.get("error")){res.writeHead(200,{"content-type":"text/plain"});res.end("Access denied");return;}
    const state=callback.searchParams.get("state"),flow=flows.get(state);assert.ok(flow);flows.delete(state);
    lastTokens=await oidc.authorizationCodeGrant(config,callback,{pkceCodeVerifier:flow.verifier,expectedState:state,expectedNonce:flow.nonce});lastClaims=lastTokens.claims();
    res.writeHead(200,{"content-type":"text/plain"});res.end(`Signed in as ${lastClaims.sub}`);return;
  }
  if(url.pathname==="/signed-out"){res.end("Signed out of RP");return;}
  if(url.pathname==="/favicon.ico"){res.writeHead(204);res.end();return;}
  res.writeHead(404);res.end();
})().catch(()=>{res.writeHead(500);res.end("Relying-party verification failed.");});});
await new Promise(resolve=>rp.listen(0,"127.0.0.1",resolve));
const rpOrigin=`http://localhost:${rp.address().port}`;redirect=`${rpOrigin}/callback`;
const browser=await chromium.launch({executablePath:process.env.DAR_CHROMIUM,headless:true}),failures=[],network=[];
try{
  const context=await browser.newContext({viewport:{width:1440,height:950}}),page=await context.newPage();
  page.on("pageerror",error=>failures.push(error.message));page.on("console",message=>{if(message.type()==="error"&&!/401.*Unauthorized|favicon.*404/.test(message.text()))failures.push(message.text());});
  page.on("request",r=>{network.push(`${r.method()} ${new URL(r.url()).pathname}`);if(network.length>35)network.shift();});page.on("requestfailed",r=>network.push(`FAILED ${r.method()} ${new URL(r.url()).pathname} ${r.failure()?.errorText}`));
  await page.goto(`${base}/settings`);await page.getByLabel("Username",{exact:true}).fill("admin");await page.getByLabel("Password",{exact:true}).fill("a long isolated test password");await page.getByRole("button",{name:"Sign in",exact:true}).click();await page.getByRole("heading",{name:"Registered applications",exact:true}).waitFor();
  const name=`Browser identity ${randomUUID().slice(0,6)}`;
  await page.getByRole("button",{name:"Register application",exact:true}).click();let dialog=page.getByRole("dialog").last();
  await dialog.getByLabel("Application name",{exact:true}).fill(name);await dialog.getByLabel("Redirect URLs",{exact:true}).fill(redirect);await dialog.getByLabel("Post-logout redirect URLs",{exact:true}).fill(`${rpOrigin}/signed-out`);await dialog.getByLabel("Allowed permissions",{exact:true}).fill("openid profile email groups offline_access");await dialog.getByRole("button",{name:"Save",exact:true}).click();
  dialog=page.getByRole("dialog",{name:"Confirm your identity",exact:true});await dialog.getByLabel("Current password",{exact:true}).fill("a long isolated test password");await dialog.getByRole("button",{name:"Confirm identity",exact:true}).click();
  dialog=page.getByRole("dialog",{name:"Save your credential",exact:true});await dialog.waitFor();const clientId=await dialog.getByLabel("Identifier",{exact:true}).inputValue(),secret=await dialog.getByLabel("Secret",{exact:true}).inputValue();assert.ok(secret);await dialog.getByRole("button",{name:"I saved it",exact:true}).click();
  await page.waitForFunction(()=>document.activeElement?.textContent==="Register application");
  config=await oidc.discovery(new URL(`${base}/oidc`),clientId,secret,oidc.ClientSecretBasic(secret),{execute:[oidc.allowInsecureRequests]});
  await page.getByLabel("Search settings",{exact:true}).fill(name);assert.equal(await page.locator(".identity-card").count(),1);await page.screenshot({path:path.join(process.env.DAR_TEST_ROOT,"identity-settings-desktop.png"),fullPage:true});
  await page.getByRole("button",{name:"Directory",exact:true}).click();await page.getByRole("button",{name:"Edit profile",exact:true}).click();
  dialog=page.getByRole("dialog").last();assert.equal(await dialog.getByLabel("Username",{exact:true}).getAttribute("readonly"),"");assert.equal(await dialog.getByLabel("Replacement password (optional)",{exact:true}).isDisabled(),true);await dialog.getByLabel("Email addresses",{exact:true}).fill("operator@example.test");await dialog.getByRole("button",{name:"Save",exact:true}).click();await dialog.waitFor({state:"hidden"});
  await page.getByRole("button",{name:"Create user",exact:true}).click();
  const username=`browser-id-${randomUUID().slice(0,6)}`;dialog=page.getByRole("dialog").last();await dialog.getByLabel("Username",{exact:true}).fill(username);await dialog.getByLabel("Display name",{exact:true}).fill("Browser directory person");await dialog.getByLabel("Email addresses",{exact:true}).fill("browser@example.com");await dialog.getByLabel("Local password (optional)",{exact:true}).fill("a long isolated test password");await dialog.getByRole("button",{name:"Save",exact:true}).click();await page.getByRole("button",{name:`Edit ${username}`,exact:true}).waitFor();
  await page.getByRole("button",{name:"Create group",exact:true}).click();dialog=page.getByRole("dialog").last();await dialog.getByLabel("Group name",{exact:true}).fill("Browser identity team");await dialog.getByLabel(`Browser directory person (@${username})`,{exact:true}).check();await dialog.getByRole("button",{name:"Save",exact:true}).click();await page.getByRole("button",{name:"Edit Browser identity team",exact:true}).waitFor();
  const users=await (await page.request.get(`${base}/api/v1/identity/users`)).json(),person=users.items.find(u=>u.userName===username);const groups=await (await page.request.get(`${base}/api/v1/identity/groups`)).json(),group=groups.items.find(g=>g.displayName==="Browser identity team");assert.equal(person.groups[0].value,group.id);
  await page.getByRole("button",{name:"SCIM",exact:true}).click();await page.getByRole("button",{name:"Create SCIM credential",exact:true}).click();dialog=page.getByRole("dialog").last();await dialog.getByLabel("Credential name",{exact:true}).fill("Browser provisioner");await dialog.getByRole("button",{name:"Save",exact:true}).click();dialog=page.getByRole("dialog",{name:"Save your credential",exact:true});const scimSecret=await dialog.getByLabel("Secret",{exact:true}).inputValue();await dialog.getByRole("button",{name:"I saved it",exact:true}).click();assert.equal((await fetch(`${base}/scim/v2/Users`,{headers:{authorization:`Bearer ${scimSecret}`}})).status,200);
  await page.setViewportSize({width:390,height:844});await page.getByRole("button",{name:"Applications",exact:true}).click();await page.getByLabel("Search settings",{exact:true}).fill(name);await page.screenshot({path:path.join(process.env.DAR_TEST_ROOT,"identity-settings-phone.png"),fullPage:true});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
  await page.setViewportSize({width:1440,height:950});
  // localhost RP -> 127.0.0.1 OP is cross-site. The native cookie remains SameSite=Strict.
  await page.goto(`${rpOrigin}/start`);await page.getByRole("button",{name:"Continue",exact:true}).waitFor();const password=page.getByLabel("Confirm your password",{exact:true});if(await password.count())await password.fill("a long isolated test password");await page.getByRole("button",{name:"Continue",exact:true}).click();await page.getByRole("heading",{name:"Review requested access",exact:true}).waitFor();await page.screenshot({path:path.join(process.env.DAR_TEST_ROOT,"identity-consent-desktop.png"),fullPage:true});await page.getByRole("button",{name:"Allow access",exact:true}).click();await page.getByText(/^Signed in as /).waitFor();assert.ok(lastTokens.id_token);assert.equal(lastClaims.preferred_username,undefined); // Profile is delivered through UserInfo by OIDC defaults.
  const adminClaims=lastClaims,adminTokens=lastTokens;const nativeCookies=await context.cookies(base);assert.equal(nativeCookies.find(c=>c.name==="dar_session").sameSite,"Strict");assert.equal(nativeCookies.find(c=>c.name==="dar_oidc").sameSite,"Lax");
  const parallelPage=await context.newPage();parallelPage.on("pageerror",error=>failures.push(error.message));
  await page.goto(`${rpOrigin}/start`);await page.getByRole("heading",{name:"Review requested access",exact:true}).waitFor();
  await parallelPage.goto(`${rpOrigin}/start`);await parallelPage.getByRole("heading",{name:"Review requested access",exact:true}).waitFor();
  await page.getByRole("button",{name:"Allow access",exact:true}).click();await page.getByText(/^Signed in as /).waitFor();
  await parallelPage.getByRole("button",{name:"Allow access",exact:true}).click();await parallelPage.getByText(/^Signed in as /).waitFor();await parallelPage.close();
  await page.goto(`${rpOrigin}/start`);await page.getByRole("heading",{name:"Review requested access",exact:true}).waitFor();await page.getByRole("button",{name:"Deny",exact:true}).click();await page.getByText("Access denied",{exact:true}).waitFor();
  await page.goto(`${rpOrigin}/start`);await page.getByRole("button",{name:"Switch account",exact:true}).click();await page.getByLabel("Username",{exact:true}).fill(username);await page.getByLabel("Password",{exact:true}).fill("a long isolated test password");await page.getByRole("button",{name:"Sign in",exact:true}).click();await page.getByRole("heading",{name:"Review requested access",exact:true}).waitFor();await page.setViewportSize({width:390,height:844});await page.screenshot({path:path.join(process.env.DAR_TEST_ROOT,"identity-consent-phone.png"),fullPage:true});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);await page.getByRole("button",{name:"Allow access",exact:true}).click();await page.getByText(/^Signed in as /).waitFor();assert.equal(lastClaims.sub,person.id);assert.notEqual(lastClaims.sub,adminClaims.sub);const info=await oidc.fetchUserInfo(config,lastTokens.access_token,person.id);assert.equal(info.email,"browser@example.com");assert.equal(info.email_verified,false);assert.deepEqual(info.groups,[group.id]);
  await page.goto(`${base}/settings`);await page.getByRole("heading",{name:"Connected applications",exact:true}).waitFor();assert.equal(await page.getByRole("button",{name:"Directory",exact:true}).count(),0);await page.getByRole("button",{name:"Revoke",exact:true}).click();await page.getByRole("dialog").getByRole("button",{name:"Continue",exact:true}).click();await page.getByText("No connected applications yet.",{exact:true}).waitFor();await assert.rejects(oidc.fetchUserInfo(config,lastTokens.access_token,person.id));
  // form_post uses a self-hosted script and a registered form destination, without inline code.
  await page.goto(`${rpOrigin}/start?mode=form_post`);await page.getByRole("heading",{name:"Review requested access",exact:true}).waitFor();await page.getByRole("button",{name:"Allow access",exact:true}).click();await page.getByText(/^Signed in as /).waitFor();assert.equal(lastClaims.sub,person.id);
  const logout=oidc.buildEndSessionUrl(config,{id_token_hint:lastTokens.id_token,post_logout_redirect_uri:`${rpOrigin}/signed-out`,state:"browser-logout"});
  await page.goto(logout.href);await page.getByRole("link",{name:"Cancel",exact:true}).click();await page.getByRole("heading",{name:"Connected applications",exact:true}).waitFor();assert.equal((await oidc.fetchUserInfo(config,lastTokens.access_token,person.id)).sub,person.id);
  await page.goto(logout.href);await page.getByRole("button",{name:"Sign out",exact:true}).click();await page.getByText("Signed out of RP",{exact:true}).waitFor();await assert.rejects(oidc.fetchUserInfo(config,lastTokens.access_token,person.id));assert.equal((await page.request.get(`${base}/api/v1/auth/session`)).status(),200);assert.ok(adminTokens.id_token);
  assert.deepEqual(failures,[]);console.info("PASS identity settings, reauthentication, one-time credentials, directory/groups, cross-site consent, account switching, scope claims, revoke, form_post and RP logout; desktop and phone.");
  await context.close();
}catch(error){
  console.error("Browser errors:",failures,"Recent requests:",network);
  for(const context of browser.contexts())for(const page of context.pages()){
    console.error("Identity browser failure:",new URL(page.url()).pathname,await page.locator("h1,h2,[role=alert],[role=status]").allTextContents());
    await page.screenshot({path:path.join(process.env.DAR_TEST_ROOT,"identity-browser-failure.png"),fullPage:true}).catch(()=>{});
  }throw error;
}finally{await browser.close();await new Promise(resolve=>rp.close(resolve));}
