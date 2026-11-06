import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp} from "node:fs/promises";
import path from "node:path";
import {tmpdir} from "node:os";
import {spawnSync} from "node:child_process";
import {randomUUID} from "node:crypto";
import {request as upstreamRequest} from "node:http";
import * as oidc from "openid-client";
import {startServer} from "../runtime/server.ts";
import {authorize,exchange,CookieJar,PASSWORD} from "./identity-helpers.ts";
test("production OIDC checks local proxy headers, secure cookies and canonical HTTPS issuer throughout a real code exchange",{skip:!process.env.DAR_DATABASE_URL,timeout:40000},async()=>{
  const root=await mkdtemp(path.join(tmpdir(),"dar-provider-proxy-")),origin="https://identity.example.test",name=`dar-proxy-${randomUUID().slice(0,8)}`;
  const publish=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"publish",name,"--js-path",path.resolve(import.meta.dirname,"../module/dist/bundle.js"),"--server",process.env.DAR_DATABASE_URL!,"--yes","--no-config"],{env:{...process.env,DAR_ISSUER:`${origin}/auth/issuer`},encoding:"utf8"});assert.equal(publish.status,0,publish.stdout+publish.stderr);
  const runtime=await startServer({host:"127.0.0.1",port:0,origin,root,databaseUrl:process.env.DAR_DATABASE_URL!,databaseName:name,serviceTokenFile:process.env.DAR_DATABASE_TOKEN_FILE!,bootstrapTokenFile:process.env.DAR_BOOTSTRAP_TOKEN_FILE!,development:false});
  const port=(runtime.server.address() as import("node:net").AddressInfo).port,backend=`http://127.0.0.1:${port}`,cookieHeaders:string[]=[];
  // Model the TLS terminator's request boundary; this never resolves or contacts example.test.
  const proxyFetch:typeof fetch=async(input,init)=>{
    const url=new URL(input instanceof Request?input.url:String(input));assert.equal(url.origin,origin);const headers=new Headers(init?.headers);headers.set("host","identity.example.test");headers.set("x-forwarded-proto","https");
    const response=await new Promise<Response>((resolve,reject)=>{
      const request=upstreamRequest(`${backend}${url.pathname}${url.search}`,{method:init?.method??"GET",headers:Object.fromEntries(headers),signal:init?.signal??undefined},res=>{void(async()=>{const chunks:Buffer[]=[];for await(const chunk of res)chunks.push(chunk);const output=new Headers();for(let i=0;i<res.rawHeaders.length;i+=2)output.append(res.rawHeaders[i],res.rawHeaders[i+1]);resolve(new Response([204,205,304].includes(res.statusCode!)?null:Buffer.concat(chunks),{status:res.statusCode,headers:output}));})().catch(reject);});
      request.on("error",reject);if(init?.body instanceof URLSearchParams)request.end(init.body.toString());else if(typeof init?.body==="string"||init?.body instanceof Uint8Array)request.end(init.body);else if(init?.body===undefined||init.body===null)request.end();else request.destroy(new Error("Unsupported fixture request body."));
    });cookieHeaders.push(...response.headers.getSetCookie());return response;
  };
  async function rejected(headers:Record<string,string>,code:string){const result=await new Promise<{status:number;body:string}>((resolve,reject)=>{const request=upstreamRequest(`${backend}/oidc/.well-known/openid-configuration`,{headers},res=>{let body="";res.setEncoding("utf8");res.on("data",chunk=>body+=chunk);res.on("end",()=>resolve({status:res.statusCode!,body}));});request.on("error",reject);request.end();});assert.equal(result.status,400);assert.equal(JSON.parse(result.body).error.code,code);}
  try{
    await rejected({host:"identity.example.test"},"untrustedProxy");
    await rejected({host:"evil.example","x-forwarded-proto":"https"},"invalidHost");
    await rejected({host:"identity.example.test","x-forwarded-proto":"https","x-forwarded-host":"evil.example"},"untrustedProxy");
    const {readFile}=await import("node:fs/promises"),bootstrapToken=await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!,"utf8");
    const setup=await proxyFetch(`${origin}/api/v1/auth/setup`,{method:"POST",headers:{origin,"content-type":"application/json"},body:JSON.stringify({username:"admin",displayName:"Production operator",password:PASSWORD,bootstrapToken})});assert.equal(setup.status,200);const cookie=setup.headers.get("set-cookie")!.split(";")[0],{csrfToken}=await setup.json() as {csrfToken:string},headers={cookie,origin,"x-csrf-token":csrfToken,"content-type":"application/json"};
    const confirmation=await proxyFetch(`${origin}/api/v1/identity/confirmation`,{method:"POST",headers,body:JSON.stringify({password:PASSWORD})});assert.equal(confirmation.status,200);const {proof}=await confirmation.json() as {proof:string};
    const payload={name:"HTTPS relying party",type:"confidential",scopes:"openid profile",redirectUris:["https://rp.example.test/callback"],postLogoutRedirectUris:[]};
    const invalid=await proxyFetch(`${origin}/api/v1/identity/clients`,{method:"POST",headers:{...headers,"x-identity-proof":proof},body:JSON.stringify({...payload,redirectUris:["http://127.0.0.1/callback"]})});assert.equal(invalid.status,400);
    const registration=await proxyFetch(`${origin}/api/v1/identity/clients`,{method:"POST",headers:{...headers,"x-identity-proof":proof},body:JSON.stringify(payload)});assert.equal(registration.status,201);const client=await registration.json() as {id:string;secret:string};
    const config=await oidc.discovery(new URL(`${origin}/oidc`),client.id,client.secret,oidc.ClientSecretBasic(client.secret),{[oidc.customFetch]:(url,options)=>proxyFetch(url,{...options,body:options.body instanceof Uint8Array?new Uint8Array(options.body):options.body})});
    assert.equal(config.serverMetadata().issuer,`${origin}/oidc`);assert.equal(config.serverMetadata().pushed_authorization_request_endpoint,undefined);
    const relyingParty={config,redirect:payload.redirectUris[0]},session={headers},flow=await authorize(origin,session,relyingParty,{scope:"openid profile",jar:new CookieJar(cookie,proxyFetch)}),tokens=await exchange(relyingParty,flow);assert.ok(tokens.id_token);
    assert.equal(tokens.claims()!.iss,`${origin}/oidc`);const providerCookies=cookieHeaders.filter(c=>c.startsWith("dar_oidc"));assert.ok(providerCookies.length);assert.ok(providerCookies.every(c=>/; secure/i.test(c)&&/; httponly/i.test(c)&&/samesite=lax/i.test(c)));assert.match(cookie,/^__Host-dar_session=/);
  }finally{await runtime.close();}
});
