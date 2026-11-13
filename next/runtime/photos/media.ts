import {execFile} from "node:child_process";
import {mkdtemp,rm,rename,mkdir,stat,writeFile} from "node:fs/promises";
import path from "node:path";
import type {PhotoExif,PhotoFacts} from "../../shared/photos.ts";

/**
 * Media tools from the Nix package (exiftool, libvips, ffmpeg) run as subprocesses
 * with an argument list (no shell), a minimal environment, time and output limits.
 * Any tool may be missing: facts then fall back to the file's time and thumbnails fail.
 */
const tool=(name:string)=>process.env[`DAR_${name.toUpperCase()}`]||name;
function run(name:string,args:string[],timeout=60_000,maxBuffer=4*1024*1024){
  return new Promise<{stdout:string;ok:boolean}>(resolve=>{
    execFile(tool(name),args,{timeout,maxBuffer,env:{PATH:process.env.PATH??"",HOME:"/nonexistent",LC_ALL:"C.UTF-8"},windowsHide:true},(error,stdout)=>resolve({stdout:String(stdout??""),ok:!error}));
  });
}

const exifTags=["DateTimeOriginal","CreateDate","MediaCreateDate","OffsetTimeOriginal","ImageWidth","ImageHeight","ExifImageWidth","ExifImageHeight","Orientation","GPSLatitude","GPSLongitude","Make","Model","LensModel","FNumber","ExposureTime","ISO","FocalLength","Duration","Rotation"];
/** EXIF dates have no zone unless OffsetTimeOriginal is present; then they are read as server-local time. */
function exifDate(value:unknown,offset:unknown,utc:boolean){
  const m=typeof value==="string"&&/^(\d{4}):(\d\d):(\d\d)[ T](\d\d):(\d\d):(\d\d)/.exec(value);if(!m||m[1]==="0000")return undefined;
  const [y,mo,d,h,mi,s]=m.slice(1).map(Number),zone=typeof offset==="string"&&/^[+-]\d\d:\d\d$/.test(offset)?offset:utc?"Z":"";
  const time=zone?Date.parse(`${m[1]}-${m[2]}-${m[3]}T${m[4]}:${m[5]}:${m[6]}${zone}`):new Date(y,mo-1,d,h,mi,s).getTime();
  return Number.isFinite(time)&&time>0?time:undefined;
}
const finite=(value:unknown)=>typeof value==="number"&&Number.isFinite(value)?value:undefined;
const short=(value:unknown)=>typeof value==="string"&&value.trim()?value.trim().slice(0,120):undefined;

export async function readFacts(file:string,kind:"image"|"video",fallbackTime:number):Promise<PhotoFacts>{
  const facts:PhotoFacts={takenAt:fallbackTime,width:0,height:0,duration:0,exif:{},latitude:null,longitude:null};
  const result=await run("exiftool",["-j","-n","-fast",...exifTags.map(t=>"-"+t),"--",file]);
  if(!result.ok)return facts;
  let data:Record<string,unknown>={};try{data=(JSON.parse(result.stdout) as Record<string,unknown>[])[0]??{};}catch{return facts;}
  // QuickTime CreateDate is UTC by specification; photo EXIF dates are local.
  // Empty dates ("0000:00:00 …") are skipped so the next candidate applies.
  facts.takenAt=exifDate(data.DateTimeOriginal,data.OffsetTimeOriginal,false)??(kind==="video"?exifDate(data.MediaCreateDate,undefined,true):undefined)??exifDate(data.CreateDate,data.OffsetTimeOriginal,kind==="video")??fallbackTime;
  let width=finite(data.ExifImageWidth)??finite(data.ImageWidth)??0,height=finite(data.ExifImageHeight)??finite(data.ImageHeight)??0;
  const orientation=finite(data.Orientation),rotation=finite(data.Rotation);
  if(orientation&&orientation>=5&&orientation<=8||rotation===90||rotation===270)[width,height]=[height,width];
  facts.width=Math.max(0,Math.min(Math.round(width),0xffffffff));facts.height=Math.max(0,Math.min(Math.round(height),0xffffffff));
  facts.duration=kind==="video"?Math.max(0,finite(data.Duration)??0):0;
  const lat=finite(data.GPSLatitude),lon=finite(data.GPSLongitude);
  if(lat!==undefined&&lon!==undefined&&Math.abs(lat)<=90&&Math.abs(lon)<=180&&!(lat===0&&lon===0)){facts.latitude=lat;facts.longitude=lon;}
  const exif:PhotoExif={make:short(data.Make),model:short(data.Model),lens:short(data.LensModel),fNumber:finite(data.FNumber),exposure:finite(data.ExposureTime),iso:finite(data.ISO),focalLength:finite(data.FocalLength),orientation};
  facts.exif=Object.fromEntries(Object.entries(exif).filter(([,v])=>v!==undefined)) as PhotoExif;
  return facts;
}

export const thumbnailSizes={s:420,m:1800} as const;
export type ThumbnailSize=keyof typeof thumbnailSizes;
export const cachePath=(cache:string,blobId:string,size:ThumbnailSize)=>{
  if(!/^[A-Za-z0-9_-]{8,128}$/.test(blobId))throw new Error("Unexpected blob ID.");
  return path.join(cache,blobId.slice(0,2),`${blobId}-${size}.webp`);
};
async function exists(file:string){try{return (await stat(file)).size>0;}catch{return false;}}

/** Writes an auto-rotated WebP thumbnail; video posters come from a frame ffmpeg extracts. */
export async function makeThumbnail(cache:string,source:string,blobId:string,kind:"image"|"video",size:ThumbnailSize){
  const target=cachePath(cache,blobId,size);if(await exists(target))return true;
  await mkdir(path.dirname(target),{recursive:true,mode:0o700});
  const work=await mkdtemp(path.join(cache,".work-"));
  try{
    const out=path.join(work,"out.webp"),edge=String(thumbnailSizes[size]),vips=(input:string)=>run("vipsthumbnail",[input,"--size",`${edge}x${edge}>`,"-o",`${out}[Q=${size==="s"?72:82},strip]`],120_000);
    let ok=false;
    if(kind==="video"){
      const frame=path.join(work,"frame.png");
      for(const at of ["1","0"]){if((await run("ffmpeg",["-nostdin","-v","error","-ss",at,"-i",source,"-frames:v","1","-y",frame],120_000)).ok&&await exists(frame))break;}
      ok=await exists(frame)&&(await vips(frame)).ok;
    }else{
      ok=(await vips(source)).ok;
      if(!ok){// RAW files: use the embedded JPEG preview.
        const preview=path.join(work,"preview.jpg");
        for(const tag of ["-PreviewImage","-JpgFromRaw"]){
          const bytes=await new Promise<Buffer|undefined>(resolve=>execFile(tool("exiftool"),["-b",tag,"--",source],{timeout:60_000,maxBuffer:64*1024*1024,encoding:"buffer",env:{PATH:process.env.PATH??""}},(e,stdout)=>resolve(e||!stdout.length?undefined:stdout)));
          if(bytes){await writeFile(preview,bytes,{mode:0o600});break;}
        }
        ok=await exists(preview)&&(await vips(preview)).ok;
      }
    }
    if(!ok||!await exists(out))return false;
    await rename(out,target);return true;
  }finally{await rm(work,{recursive:true,force:true});}
}
