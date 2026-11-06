import { randomUUID } from "node:crypto";
import type { Database } from "../database.ts";
import type { authService } from "../auth/service.ts";
import { AppError } from "../errors.ts";
import { applyPatch, normalize, resource, ScimError, type DirectoryRecord } from "./scim-model.ts";

export interface DirectoryActor {sessionId?:string;bearer?:string;proof?:string}
export function directoryService(db:Database,auth:Awaited<ReturnType<typeof authService>>,actor:DirectoryActor){
  const call=<T=unknown>(operation:string,p:object={})=>db.call<T>("service_identity",[actor.sessionId??"",actor.bearer??"",actor.proof??"",operation,JSON.stringify(p)]);
  const get=(kind:"Users"|"Groups",id:string)=>call<DirectoryRecord>(kind==="Users"?"user":"group",{id});
  return {
    call,get,
    list:(kind:"Users"|"Groups")=>call<DirectoryRecord[]>(kind==="Users"?"users":"groups"),
    async save(kind:"Users"|"Groups",id:string|undefined,input:unknown,match:string|undefined,patch=false,required=false){
      await call("authorizeWrite");
      const old=id?await get(kind,id):undefined;
      checkMatch(old,match,required&&!!id);
      const parsed=normalize(kind,patch?applyPatch(kind,resource(kind,old!,""),input):input);
      const verifier=parsed.password===undefined?undefined:await auth.provisionPassword(parsed.password);
      try{return await call<DirectoryRecord>(kind==="Users"?"saveUser":"saveGroup",{id:id??randomUUID(),revision:old?.revision??0,document:JSON.stringify(parsed.document),...(verifier!==undefined?{verifier}:{})});}
      catch(error){if(error instanceof AppError&&error.code==="conflict")throw new ScimError(match?412:409,match?"The resource changed. Read it and retry.":"The resource changed or userName is already reserved.",match?undefined:"uniqueness");throw error;}
    },
    async remove(kind:"Users"|"Groups",id:string,match:string|undefined,required=false){await call("authorizeWrite");const old=await get(kind,id);checkMatch(old,match,required);try{await call(kind==="Users"?"deleteUser":"deleteGroup",{id,revision:old.revision});}catch(error){if(error instanceof AppError&&error.code==="conflict")throw new ScimError(412,"The resource changed. Read it and retry.");throw error;}}
  };
}
export function checkMatch(row:{revision:number}|undefined,match:string|undefined,required=false){
  if(required&&(!match||match.split(",").some(value=>value.trim()==="*")))throw new AppError("preconditionRequired",428,"Read the current revision before changing this resource.");
  if(match&&match!=="*"&&(!row||!match.split(",").map(s=>s.trim()).includes(`W/"${row.revision}"`)))throw new ScimError(412,"The resource changed. Read it and retry.");
}
