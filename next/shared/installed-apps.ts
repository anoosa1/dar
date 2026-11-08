export const appPermissions = ["ownUserData:read", "ownUserData:write", "regional:read"] as const;
export type AppPermission = typeof appPermissions[number];
export interface AppManifest {
  id:string; name:string; version:string; apiVersion:1; license:string;
  client:string; styles:string[]; files:string[]; permissions:AppPermission[]; server?:string;
}
export interface InstalledApp {id:string;name:string;version:string;digest:string;clientUrl:string;styles:string[];permissions:AppPermission[]}
export interface AppRecord<T = unknown> {key:string;revision:number;schemaVersion:number;value:T;modifiedAt:number}
export const validAppId = (id:string) => /^[a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*){2,7}$/.test(id) && id.length<=120;
export const validAppKey = (key:string) => /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$/.test(key);
