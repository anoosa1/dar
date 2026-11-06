import { AsyncLocalStorage } from "node:async_hooks";
import { createHash } from "node:crypto";
import { errors, type AdapterPayload, type Adapter } from "oidc-provider";
import type { Database } from "../database.ts";
import { keyringBox } from "../auth/keyring.ts";

export const providerContext=new AsyncLocalStorage<{nativeSessionId?:string;authenticatedSessionId?:string}>();
export const modelKey=(model:string,id:string)=>`${model}/${createHash("sha256").update(id).digest("hex")}`;
interface Stored {id:string;encrypted:string;nativeSessionId:string;consumed:number;accountId:string}
export async function providerStore(db:Database,directory:string){
  const box=await keyringBox(directory,"provider-model"),clientBox=await keyringBox(directory,"provider-client");
  const call=<T>(operation:string,p:object={})=>db.call<T>("service_provider",[operation,JSON.stringify(p)]);
  class PersistentAdapter implements Adapter {
    readonly name:string;
    constructor(name:string){this.name=name;}
    async upsert(id:string,payload:AdapterPayload,expiresIn?:number){
      if(this.name==="Client")throw new Error("Clients must be registered by a native administrator.");
      const key=modelKey(this.name,id),old=await call<Stored|null>("find",{id:key});
      const context=providerContext.getStore();
      let nativeSessionId=(this.name==="Session"&&old?.nativeSessionId?old.nativeSessionId:context?.authenticatedSessionId??context?.nativeSessionId)??old?.nativeSessionId??"";
      const uid=payload.sessionUid??payload.session?.uid;
      if(!nativeSessionId&&uid){const session=await call<Stored|null>("findUid",{id:modelKey("uid",uid)});nativeSessionId=session?.nativeSessionId??"";}
      const accountId=(this.name==="Interaction"?payload.result?.login?.accountId:undefined)??payload.accountId??payload.session?.accountId??"";
      if(accountId&&!nativeSessionId)throw new errors.InvalidGrant("The native sign-in session is unavailable.");
      const expiresAt=Date.now()+Math.min(expiresIn??86400,30*86400)*1000;
      // Interactions can reference a grant that the library retires while switching accounts.
      // Permission authority stays with Grant and token records, not the pending interaction.
      await call("upsert",{id:key,model:this.name,encrypted:box.encrypt(payload),accountId,clientId:payload.clientId??String(payload.params?.client_id??""),grantKey:payload.grantId&&this.name!=="Interaction"?modelKey("Grant",payload.grantId):"",sessionKey:uid?modelKey("uid",uid):"",uidKey:payload.uid?modelKey("uid",payload.uid):"",userCodeKey:payload.userCode?modelKey("userCode",payload.userCode):"",nativeSessionId,expiresAt,expiryKey:String(expiresAt).padStart(16,"0"),consumed:old?.consumed??0});
    }
    async find(id:string){
      if(this.name==="Client"){const client=await call<{enabled:boolean;encrypted:string}|null>("client",{id});return client?.enabled?clientBox.decrypt<AdapterPayload>(client.encrypted):undefined;}
      return this.decode(await call<Stored|null>("find",{id:modelKey(this.name,id)}));
    }
    async findByUid(uid:string){return this.decode(await call<Stored|null>("findUid",{id:modelKey("uid",uid)}));}
    async findByUserCode(code:string){return this.decode(await call<Stored|null>("findUserCode",{id:modelKey("userCode",code)}));}
    decode(row:Stored|null){if(!row)return undefined;const context=providerContext.getStore();if(context&&!context.authenticatedSessionId&&row.nativeSessionId)context.nativeSessionId=row.nativeSessionId;const payload=box.decrypt<AdapterPayload>(row.encrypted);if(row.consumed)payload.consumed=row.consumed;return payload;}
    async consume(id:string){try{await call("consume",{id:modelKey(this.name,id)});}catch{throw new errors.InvalidGrant("The credential was already consumed or expired.");}}
    async destroy(id:string){await call("destroy",{id:modelKey(this.name,id)});}
    async revokeByGrantId(id:string){await call("revokeGrant",{id:modelKey("Grant",id)});}
  }
  return {Adapter:PersistentAdapter,call,clientBox};
}
