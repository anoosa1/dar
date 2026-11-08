import type {AppRecord} from "./index.js";
export interface ServerAppContext {signal:AbortSignal;effect:(start:()=>()=>void|Promise<void>)=>void}
export interface AppRequest {
  method:string;path:string;query:Record<string,string>;body:Record<string,unknown>;
  user:{id:string;username:string;displayName:string};signal:AbortSignal;
  data:{get:(key:string)=>Promise<AppRecord|null>;put:(key:string,value:unknown,revision:number,schemaVersion?:number)=>Promise<AppRecord>;delete:(key:string,revision:number)=>Promise<unknown>};
}
export interface ServerApp {handle:(request:AppRequest)=>Promise<unknown>}
export type Activate=(context:ServerAppContext)=>ServerApp|Promise<ServerApp>;
