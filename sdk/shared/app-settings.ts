export const essentialApps = ["files", "contacts", "calendar"] as const;
export interface AppSettings {revision:number;disabled:string[]}
export interface ManagedApp {id:string;name:string;version:string;status:string;permissions:readonly string[];enabled:boolean;canToggle:boolean}
