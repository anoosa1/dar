import {execFile,spawn} from "node:child_process";
import {mkdtemp,rm,rename,mkdir,stat,writeFile} from "node:fs/promises";
import path from "node:path";
import {nameParts,type PhotoExif,type PhotoFacts,type PhotoPlayback} from "../../shared/photos.ts";

/**
 * Media tools from the Nix package (exiftool, libvips, ffmpeg/ffprobe) run as subprocesses
 * with an argument list (no shell), a minimal environment, time and output limits.
 * Any tool may be missing: facts then fall back to the file's time and thumbnails fail.
 */
const tool=(name:string)=>process.env[`DAR_${name.toUpperCase()}`]||name;
const environment=()=>({PATH:process.env.PATH??"",HOME:"/nonexistent",LC_ALL:"C.UTF-8"});
function run(name:string,args:string[],timeout=60_000,maxBuffer=4*1024*1024){
  return new Promise<{stdout:string;ok:boolean}>(resolve=>{
    execFile(tool(name),args,{timeout,maxBuffer,env:environment(),windowsHide:true},(error,stdout)=>resolve({stdout:String(stdout??""),ok:!error}));
  });
}
function runBytes(name:string,args:string[],timeout:number,maxBuffer:number){
  return new Promise<Buffer|undefined>(resolve=>execFile(tool(name),args,{timeout,maxBuffer,encoding:"buffer",env:environment(),windowsHide:true},(error,stdout)=>resolve(error||!stdout.length?undefined:stdout)));
}

const exifTags=["DateTimeOriginal","CreateDate","MediaCreateDate","OffsetTimeOriginal","ImageWidth","ImageHeight","ExifImageWidth","ExifImageHeight","Orientation","GPSLatitude","GPSLongitude","Make","Model","LensModel","FNumber","ExposureTime","ISO","FocalLength","Duration","Rotation","ContentIdentifier","BurstUUID"];
/** EXIF dates have no zone unless OffsetTimeOriginal is present; then they are read as server-local time. */
function exifDate(value:unknown,offset:unknown,utc:boolean){
  const m=typeof value==="string"&&/^(\d{4}):(\d\d):(\d\d)[ T](\d\d):(\d\d):(\d\d)/.exec(value);if(!m||m[1]==="0000")return undefined;
  const [y,mo,d,h,mi,s]=m.slice(1).map(Number),zone=typeof offset==="string"&&/^[+-]\d\d:\d\d$/.test(offset)?offset:utc?"Z":"";
  const time=zone?Date.parse(`${m[1]}-${m[2]}-${m[3]}T${m[4]}:${m[5]}:${m[6]}${zone}`):new Date(y,mo-1,d,h,mi,s).getTime();
  return Number.isFinite(time)&&time>0?time:undefined;
}
const finite=(value:unknown)=>typeof value==="number"&&Number.isFinite(value)?value:undefined;
const short=(value:unknown)=>typeof value==="string"&&value.trim()?value.trim().slice(0,120):undefined;
const identifier=(value:unknown)=>typeof value==="string"&&/^[A-Za-z0-9._:-]{1,200}$/.test(value.trim())?value.trim():"";

/**
 * Which browsers play without help: MP4 with H.264/VP9/AV1 and AAC/MP3/Opus, or WebM with
 * VP8/VP9/AV1. Other H.264 files are remuxed into MP4; everything else is transcoded.
 */
export function playbackFor(name:string,mediaType:string,video:string,audio:string):PhotoPlayback{
  const ext=nameParts(name).ext,type=mediaType.toLowerCase();
  const container=["mp4","m4v"].includes(ext)||type==="video/mp4"?"mp4":ext==="webm"||type==="video/webm"?"webm":"other";
  const audioIn=(list:string[])=>!audio||list.includes(audio);
  if(container==="mp4"&&["h264","av1","vp9"].includes(video)&&audioIn(["aac","mp3","opus"]))return "original";
  if(container==="webm"&&["vp8","vp9","av1"].includes(video)&&audioIn(["opus","vorbis"]))return "original";
  return video==="h264"?"remux":"transcode";
}
async function probe(file:string){
  const result=await run("ffprobe",["-v","error","-show_entries","stream=codec_type,codec_name","-of","json",file],60_000,1024*1024);
  let streams:{codec_type?:string;codec_name?:string}[]=[];
  if(result.ok)try{streams=(JSON.parse(result.stdout) as {streams?:typeof streams}).streams??[];}catch{streams=[];}
  const codec=(type:string)=>short(streams.find(s=>s.codec_type===type)?.codec_name)?.toLowerCase().slice(0,30)??"";
  return {ok:result.ok,video:codec("video"),audio:codec("audio")};
}

export async function readFacts(file:string,kind:"image"|"video",fallbackTime:number,name:string,mediaType:string):Promise<PhotoFacts>{
  const facts:PhotoFacts={takenAt:fallbackTime,width:0,height:0,duration:0,exif:{},latitude:null,longitude:null,phash:"",playback:"",contentId:"",burstId:"",sha1:""};
  if(kind==="video"){
    const streams=await probe(file);
    if(streams.ok){facts.playback=playbackFor(name,mediaType,streams.video,streams.audio);facts.exif={...streams.video?{videoCodec:streams.video}:{},...streams.audio?{audioCodec:streams.audio}:{}};}
  }
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
  const exif:PhotoExif={...facts.exif,make:short(data.Make),model:short(data.Model),lens:short(data.LensModel),fNumber:finite(data.FNumber),exposure:finite(data.ExposureTime),iso:finite(data.ISO),focalLength:finite(data.FocalLength),orientation};
  facts.exif=Object.fromEntries(Object.entries(exif).filter(([,v])=>v!==undefined)) as PhotoExif;
  facts.contentId=identifier(data.ContentIdentifier);facts.burstId=identifier(data.BurstUUID);
  return facts;
}

/**
 * 64-bit difference hash of a thumbnail: shrink to 9×8 grey pixels and compare neighbours.
 * Resized or re-encoded copies of a picture share it.
 */
export async function perceptualHash(file:string){
  const bytes=await runBytes("ffmpeg",["-nostdin","-v","error","-i",file,"-vf","scale=9:8:flags=area,format=gray","-f","rawvideo","pipe:1"],30_000,4096);
  if(!bytes||bytes.length!==72)return "";
  let hash=0n;for(let y=0;y<8;y++)for(let x=0;x<8;x++)hash=hash<<1n|(bytes[y*9+x]<bytes[y*9+x+1]?1n:0n);
  return hash.toString(16).padStart(16,"0");
}

export const thumbnailSizes={s:420,m:1800} as const;
export type ThumbnailSize=keyof typeof thumbnailSizes;
function cacheFile(cache:string,blobId:string,suffix:string){
  if(!/^[A-Za-z0-9_-]{8,128}$/.test(blobId))throw new Error("Unexpected blob ID.");
  return path.join(cache,blobId.slice(0,2),`${blobId}-${suffix}`);
}
export const cachePath=(cache:string,blobId:string,size:ThumbnailSize)=>cacheFile(cache,blobId,`${size}.webp`);
/** Browser-playable MP4 made from a video that needs a remux or transcode. */
export const videoPath=(cache:string,blobId:string)=>cacheFile(cache,blobId,"v.mp4");
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
          const bytes=await runBytes("exiftool",["-b",tag,"--",source],60_000,64*1024*1024);
          if(bytes){await writeFile(preview,bytes,{mode:0o600});break;}
        }
        ok=await exists(preview)&&(await vips(preview)).ok;
      }
    }
    if(!ok||!await exists(out))return false;
    await rename(out,target);return true;
  }finally{await rm(work,{recursive:true,force:true});}
}

/**
 * Makes a browser-playable MP4 (H.264/AAC, fast start). A remux copies the H.264 stream;
 * a transcode fits the picture in 1920×1920 with ffmpeg's automatic rotation. Progress
 * comes from ffmpeg's -progress output; the time limit grows with the video's length.
 */
export async function makeVideo(cache:string,source:string,blobId:string,mode:"remux"|"transcode",audio:string,duration:number,progress:(fraction:number)=>void,signal:AbortSignal){
  const target=videoPath(cache,blobId);if(await exists(target))return true;
  await mkdir(path.dirname(target),{recursive:true,mode:0o700});
  const work=await mkdtemp(path.join(cache,".work-"));
  try{
    const out=path.join(work,"out.mp4");
    const video=mode==="remux"?["-c:v","copy","-tag:v","avc1"]:["-c:v","libx264","-preset","veryfast","-crf","23","-pix_fmt","yuv420p","-vf","scale=w='min(1920,iw)':h='min(1920,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2"];
    const args=["-nostdin","-v","error","-nostats","-i",source,"-map","0:v:0","-map","0:a:0?","-sn","-dn",...video,...audio==="aac"?["-c:a","copy"]:["-c:a","aac","-b:a","160k"],"-movflags","+faststart","-progress","pipe:1","-f","mp4","-y",out];
    const ok=await new Promise<boolean>(resolve=>{
      if(signal.aborted)return resolve(false);
      const child=spawn(tool("ffmpeg"),args,{env:environment(),stdio:["ignore","pipe","ignore"],windowsHide:true});
      const stop=()=>child.kill("SIGKILL"),limit=setTimeout(stop,Math.min(4*3600_000,Math.max(300_000,duration*8_000)));
      signal.addEventListener("abort",stop,{once:true});
      let pending="";child.stdout.setEncoding("utf8");
      child.stdout.on("data",(chunk:string)=>{pending+=chunk;const lines=pending.split("\n");pending=lines.pop()??"";for(const line of lines){const m=/^out_time_us=(\d+)/.exec(line);if(m&&duration>0)progress(Math.min(.99,Number(m[1])/1e6/duration));}});
      child.on("error",()=>{clearTimeout(limit);resolve(false);});
      child.on("close",code=>{clearTimeout(limit);signal.removeEventListener("abort",stop);resolve(code===0);});
    });
    if(!ok||!await exists(out))return false;
    await rename(out,target);return true;
  }finally{await rm(work,{recursive:true,force:true});}
}
