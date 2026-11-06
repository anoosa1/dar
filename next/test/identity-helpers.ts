import assert from "node:assert/strict";
import * as oidc from "openid-client";
import {login} from "./integration-helpers.ts";
export const PASSWORD="a long isolated test password";
export class CookieJar{
  readonly cookies=new Map<string,string>();
  readonly fetcher:typeof fetch;
  constructor(cookie?:string,fetcher:typeof fetch=fetch){this.fetcher=fetcher;if(cookie){const [key,...rest]=cookie.split("=");this.cookies.set(key,rest.join("="));}}
  get header(){return [...this.cookies].map(([k,v])=>`${k}=${v}`).join("; ");}
  async fetch(url:string|URL,init:RequestInit={}){const response=await this.fetcher(url,{...init,redirect:"manual",headers:{...init.headers,cookie:this.header}});for(const cookie of response.headers.getSetCookie()){const [pair]=cookie.split(";"),[key,...value]=pair.split("=");if(/max-age=0|expires=Thu, 01 Jan 1970/i.test(cookie))this.cookies.delete(key);else this.cookies.set(key,value.join("="));}return response;}
}
export async function identityAdmin(base:string){
  const session=await login(base);
  async function api(path:string,method="GET",payload?:unknown,extra:Record<string,string>={}){const r=await fetch(`${base}/api/v1/identity${path}`,{method,headers:{...session.headers,"content-type":"application/json",...extra},body:payload===undefined?undefined:JSON.stringify(payload)});return r;}
  const proofResponse=await api("/confirmation","POST",{password:PASSWORD});assert.equal(proofResponse.status,200,await proofResponse.clone().text());const {proof}=await proofResponse.json() as {proof:string};
  const write=(path:string,method:string,payload?:unknown,match?:string)=>api(path,method,payload,{"x-identity-proof":proof,...(match?{"if-match":match}:{})});
  async function credential(scope="write"){const r=await write("/credentials","POST",{label:"Acceptance SCIM",scope,days:1});assert.equal(r.status,201,await r.clone().text());return r.json() as Promise<{id:string;secret:string}>;}
  async function client(type="confidential",scopes="openid profile email groups offline_access",redirect="http://127.0.0.1:49999/callback"){
    const payload={name:"Acceptance relying party",type,redirectUris:[redirect],postLogoutRedirectUris:["http://127.0.0.1:49999/signed-out"],scopes,enabled:true};
    const r=await write("/clients","POST",payload);assert.equal(r.status,201,await r.clone().text());const value=await r.json() as {id:string;secret?:string};
    const config=await oidc.discovery(new URL(`${base}/oidc`),value.id,value.secret,value.secret?oidc.ClientSecretBasic(value.secret):oidc.None(),{execute:[oidc.allowInsecureRequests]});
    return {...value,payload,config,redirect};
  }
  return {session,api,write,credential,client};
}
export async function authorize(base:string,session:Pick<Awaited<ReturnType<typeof login>>,"headers">,client:Pick<Awaited<ReturnType<Awaited<ReturnType<typeof identityAdmin>>["client"]>>,"config"|"redirect">,options:{deny?:boolean;scope?:string;prompt?:string;params?:Record<string,string>;jar?:CookieJar;onInteraction?:(detail:{prompt:string;requiresPassword:boolean})=>void}={}){
  const verifier=oidc.randomPKCECodeVerifier(),challenge=await oidc.calculatePKCECodeChallenge(verifier),state=oidc.randomState(),nonce=oidc.randomNonce();
  const url=oidc.buildAuthorizationUrl(client.config,{redirect_uri:client.redirect,response_type:"code",scope:options.scope??"openid profile email groups offline_access",code_challenge:challenge,code_challenge_method:"S256",state,nonce,prompt:options.prompt??"consent",...options.params});
  const jar=options.jar??new CookieJar(session.headers.cookie);let next=url.href;
  for(let i=0;i<12;i++){
    const target=new URL(next);if(target.origin!==base)return {url:target,verifier,state,nonce,jar};
    const match=/^\/identity\/interaction\/([^/]+)$/.exec(target.pathname);
    if(match){
      const endpoint=`${base}/api/v1/identity/interactions/${match[1]}`,headers={origin:base,"x-csrf-token":session.headers["x-csrf-token"],"content-type":"application/json"};
      const detailResponse=await jar.fetch(endpoint,{headers});assert.equal(detailResponse.status,200,await detailResponse.clone().text());const detail=await detailResponse.json() as {prompt:string;requiresPassword:boolean};options.onInteraction?.(detail);
      const response=await jar.fetch(endpoint,{method:"POST",headers,body:JSON.stringify({action:options.deny?"deny":detail.prompt==="login"?"login":"approve",password:PASSWORD})});assert.equal(response.status,200,await response.clone().text());next=(await response.json() as {redirect:string}).redirect;
    }else{
      const response=await jar.fetch(next);assert.ok([302,303].includes(response.status),`${target.pathname}: ${response.status} ${await response.text()}`);next=new URL(response.headers.get("location")!,base).href;
    }
  }
  throw new Error("Authorization failed to reach the registered callback.");
}
export async function exchange(client:Pick<Awaited<ReturnType<Awaited<ReturnType<typeof identityAdmin>>["client"]>>,"config">,flow:Awaited<ReturnType<typeof authorize>>){return oidc.authorizationCodeGrant(client.config,flow.url,{pkceCodeVerifier:flow.verifier,expectedState:flow.state,expectedNonce:flow.nonce});}
