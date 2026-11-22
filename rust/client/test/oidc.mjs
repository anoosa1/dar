// Signs in to a relying party through the Rust runtime's OIDC provider with openid-client
// (an independent implementation): discovery, PKCE authorization, Dar's sign-in interaction
// API, the code exchange (ID token signature, issuer, audience and nonce checked) and UserInfo.
// Usage: DAR_RUST_ORIGIN=… DAR_OIDC_CLIENT_ID=… DAR_OIDC_CLIENT_SECRET=… node oidc.mjs
import {createRequire} from "node:module";
import assert from "node:assert/strict";
const require=createRequire(new URL("../../../next/package.json",import.meta.url));
const client=require("openid-client");
const origin=process.env.DAR_RUST_ORIGIN,redirect="http://127.0.0.1:4599/callback";
const step=async(name,fn)=>{process.stdout.write(name+" … ");const value=await fn();console.log("ok");return value;};

// A path-aware cookie jar standing in for the browser.
const jar=[];
function remember(response,url){for(const line of response.headers.getSetCookie()){const [pair,...attrs]=line.split(";").map(s=>s.trim());const [name,value]=pair.split("=");const path=attrs.find(a=>/^path=/i.test(a))?.slice(5)??"/";const i=jar.findIndex(c=>c.name===name&&c.path===path);if(i>=0)jar.splice(i,1);if(value&&!/max-age=0/i.test(line))jar.push({name,value,path});}}
const cookies=url=>jar.filter(c=>new URL(url).pathname.startsWith(c.path)).sort((a,b)=>b.path.length-a.path.length).map(c=>`${c.name}=${c.value}`).join("; ");
async function browse(url,init={}){const response=await fetch(url,{...init,redirect:"manual",headers:{...init.headers,cookie:cookies(url)}});remember(response,url);return response;}

const config=await step("discovery",()=>client.discovery(new URL(origin+"/oidc"),process.env.DAR_OIDC_CLIENT_ID,undefined,client.ClientSecretBasic(process.env.DAR_OIDC_CLIENT_SECRET),{execute:[client.allowInsecureRequests]}));
assert.equal(config.serverMetadata().issuer,origin+"/oidc");
const verifier=client.randomPKCECodeVerifier(),state=client.randomState(),nonce=client.randomNonce();
const authorize=client.buildAuthorizationUrl(config,{redirect_uri:redirect,scope:"openid profile",code_challenge:await client.calculatePKCECodeChallenge(verifier),code_challenge_method:"S256",state,nonce});

await step("sign in to Dar",async()=>{const r=await browse(origin+"/api/v1/auth/login",{method:"POST",headers:{origin,"content-type":"application/json"},body:JSON.stringify({username:"amina",password:"a long isolated test password"})});assert.equal(r.status,200);});
const csrf=(await (await browse(origin+"/api/v1/auth/session")).json()).csrfToken;
let next=authorize.href,callback,decisions=[];
await step("authorize and decide on Dar's sign-in page",async()=>{
  for(let hop=0;hop<10&&!callback;hop++){
    const r=await browse(next);assert.equal(r.status,303,`${next} answered ${r.status}`);
    const location=new URL(r.headers.get("location"),next);
    if(location.href.startsWith(redirect)){callback=location;break;}
    const uid=/^\/identity\/interaction\/([A-Za-z0-9_-]+)$/.exec(location.pathname)?.[1];assert.ok(uid,location.href);
    assert.equal((await browse(location.href,{headers:{accept:"text/html"}})).status,200);
    const api=`${origin}/api/v1/identity/interactions/${uid}`,details=await (await browse(api)).json();
    assert.equal(details.client.id,process.env.DAR_OIDC_CLIENT_ID,JSON.stringify(details));
    const action=details.prompt==="login"?"login":"approve";decisions.push(action+(details.requiresPassword?" (password confirmed)":""));
    const decided=await browse(api,{method:"POST",headers:{origin,"x-csrf-token":csrf,"content-type":"application/json"},body:JSON.stringify(details.requiresPassword?{action,password:"a long isolated test password"}:{action})});
    assert.equal(decided.status,200,await decided.clone().text());next=(await decided.json()).redirect;
  }
  assert.ok(callback,"the provider redirected back with a code");
});
console.log("  interactions:",decisions.join(", "));
const tokens=await step("code exchange (ID token checked)",()=>client.authorizationCodeGrant(config,callback,{pkceCodeVerifier:verifier,expectedState:state,expectedNonce:nonce},{redirect_uri:redirect}));
const claims=tokens.claims();assert.ok(claims.sub&&claims.at_hash&&claims.auth_time,JSON.stringify(claims));
const info=await step("UserInfo",()=>client.fetchUserInfo(config,tokens.access_token,claims.sub));
assert.equal(info.sub,claims.sub);assert.equal(info.preferred_username,"amina",JSON.stringify(info));console.log("  subject:",claims.sub,"profile:",JSON.stringify(info));
await step("the code cannot be used twice",async()=>{await assert.rejects(client.authorizationCodeGrant(config,callback,{pkceCodeVerifier:verifier,expectedState:state,expectedNonce:nonce},{redirect_uri:redirect}));});
await step("signing out of Dar ends the grant's tokens",async()=>{
  await browse(origin+"/api/v1/auth/logout",{method:"POST",headers:{origin,"x-csrf-token":csrf}});
  await assert.rejects(client.fetchUserInfo(config,tokens.access_token,claims.sub));
});
