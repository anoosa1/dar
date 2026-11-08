import {lstat,readFile,realpath} from "node:fs/promises";
import {createHash} from "node:crypto";
import path from "node:path";
import {appPermissions,validAppId,type AppManifest,type InstalledApp} from "../../shared/installed-apps.ts";
export interface PackageInput {path:string;digest?:string;enabled?:boolean}
export interface AppPackage {root:string;manifest:AppManifest;descriptor:InstalledApp;assets:Map<string,Buffer>}
const safeFile=(s:unknown):s is string=>typeof s==="string"&&s.length<=180&&/^[a-zA-Z0-9_-]+(?:[./][a-zA-Z0-9_-]+)*\.(js|css|svg|png|webp|json)$/.test(s)&&!s.split("/").some(p=>p==="."||p==="..");
/** Read only regular, declared package files; retained bytes make asset digests immutable. */
export async function readAppPackage(input:PackageInput):Promise<AppPackage>{
  if(!path.isAbsolute(input.path))throw Error("Package paths must be absolute.");
  const root=await realpath(input.path),assets=new Map<string,Buffer>();
  async function regular(name:string,max:number){
    let current=root;for(const part of name.split("/")){current=path.join(current,part);const stat=await lstat(current);if(stat.isSymbolicLink())throw Error("Package symlinks are not allowed.");}
    const stat=await lstat(current);if(!stat.isFile()||stat.size>max)throw Error("Invalid package file.");return readFile(current);
  }
  const raw=await regular("dar-app.json",32768),m=JSON.parse(raw.toString()) as AppManifest;
  if(!validAppId(m.id)||typeof m.name!=="string"||!m.name.trim()||m.name.length>60||typeof m.version!=="string"||!/^\d+\.\d+\.\d+$/.test(m.version)||m.apiVersion!==1||typeof m.license!=="string"||m.license.length>100||!Array.isArray(m.files)||m.files.length>200||new Set(m.files).size!==m.files.length||!m.files.every(safeFile)||!m.files.includes(m.client)||!m.client.endsWith(".js")||!Array.isArray(m.styles)||!m.styles.every(s=>m.files.includes(s)&&s.endsWith(".css"))||!Array.isArray(m.permissions)||!m.permissions.every(p=>appPermissions.includes(p))||new Set(m.permissions).size!==m.permissions.length||m.server!==undefined&&(!m.files.includes(m.server)||!m.server.endsWith(".js")))throw Error("Invalid app manifest or unsupported API.");
  const hash=createHash("sha256").update(raw);let size=0;
  for(const name of [...m.files].sort()){const bytes=await regular(name,8*1024*1024);size+=bytes.length;if(size>32*1024*1024)throw Error("Package exceeds 32 MiB.");assets.set(name,bytes);hash.update(JSON.stringify(name)).update(bytes);}
  const digest=hash.digest("hex");if(input.digest&&input.digest!==digest)throw Error("Package digest mismatch.");
  const base=`/appAssets/${m.id}/${digest}/`;
  return {root,manifest:m,assets,descriptor:{id:m.id,name:m.name,version:m.version,digest,clientUrl:base+m.client,styles:m.styles.map(s=>base+s),permissions:m.permissions}};
}
export async function loadApps(filename?:string){
  const apps=new Map<string,AppPackage>(),inventory:{id:string;name:string;version:string;status:string;permissions:readonly string[]}[]=[];
  if(!filename||process.env.DAR_DISABLE_APPS==="1")return {apps,inventory};
  const inputs=JSON.parse(await readFile(filename,"utf8")) as PackageInput[];
  if(!Array.isArray(inputs)||inputs.length>100)throw Error("App registry must contain up to 100 package entries.");
  const seen=new Set<string>();
  for(const input of inputs){
    try{const app=await readAppPackage(input);if(seen.has(app.manifest.id))throw Error("Duplicate app ID.");seen.add(app.manifest.id);
      inventory.push({id:app.manifest.id,name:app.manifest.name,version:app.manifest.version,status:input.enabled===false?"Disabled":"Ready",permissions:app.manifest.permissions});
      if(input.enabled!==false)apps.set(app.manifest.id,app);
    }catch{inventory.push({id:`invalid-${inventory.length+1}`,name:"Unavailable package",version:"",status:"Invalid package, duplicate ID or unsupported API. Check host configuration.",permissions:[]});}
  }
  return {apps,inventory};
}
