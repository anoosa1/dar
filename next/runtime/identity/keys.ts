import { readFile, lstat } from "node:fs/promises";
import { randomBytes, randomUUID } from "node:crypto";
import { generateKeyPair, exportJWK, type JWK } from "jose";
import path from "node:path";
import { keyringBox } from "../auth/keyring.ts";
import { createDurableFile, replaceDurableFile } from "../atomic.ts";
interface Keys { active:JWK;retired:{jwk:JWK;retireAt:number}[];cookieKey:string;rotatedAt:number }
async function fresh(){const pair=await generateKeyPair("RS256",{extractable:true,modulusLength:2048});return {...await exportJWK(pair.privateKey),kid:`dar-oidc-${randomUUID()}`,alg:"RS256",use:"sig"};}
export async function providerKeys(directory:string){
  const filename=path.join(directory,"oidc-keyring.json"),box=await keyringBox(directory,"provider-keyring");let state:Keys;
  try{const stat=await lstat(filename);if(!stat.isFile()||(stat.mode&0o077))throw new Error("Unsafe provider keyring.");state=box.decrypt<Keys>(await readFile(filename,"utf8"));}
  catch(error){if((error as NodeJS.ErrnoException).code!=="ENOENT")throw error;state={active:await fresh(),retired:[],cookieKey:randomBytes(48).toString("base64url"),rotatedAt:Date.now()};await createDurableFile(filename,box.encrypt(state));}
  let queue=Promise.resolve();
  return {
    current:()=>state,
    rotate(){const work=queue.then(async()=>{const next={active:await fresh(),retired:[{jwk:state.active,retireAt:Date.now()+600000},...state.retired].filter(k=>k.retireAt>Date.now()),cookieKey:state.cookieKey,rotatedAt:Date.now()};await replaceDurableFile(filename,box.encrypt(next));state=next;});queue=work.catch(()=>{});return work;},
    prune(){const work=queue.then(async()=>{const retired=state.retired.filter(k=>k.retireAt>Date.now());if(retired.length===state.retired.length)return;const next={...state,retired};await replaceDurableFile(filename,box.encrypt(next));state=next;});queue=work.catch(()=>{});return work;},
    status:()=>({activeKid:state.active.kid,rotatedAt:new Date(state.rotatedAt).toISOString(),retained:state.retired.map(k=>({kid:k.jwk.kid,retireAt:new Date(k.retireAt).toISOString()}))})
  };
}
