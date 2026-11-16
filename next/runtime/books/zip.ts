import {open,type FileHandle} from "node:fs/promises";
import {constants} from "node:fs";
import {inflateRaw} from "node:zlib";
import {promisify} from "node:util";

const inflate=promisify(inflateRaw);
export interface ZipEntry {name:string;method:number;compressed:number;size:number;offset:number;encrypted:boolean}
export class ZipError extends Error {}

/**
 * A read-only ZIP archive (EPUB, CBZ) over an immutable blob. Only stored and deflated
 * entries are read, with bounded entry counts and sizes; names are kept as written and
 * looked up exactly, so no entry can name a path outside the archive.
 */
export async function openZip(file:string,limits={entries:20000}){
  const handle=await open(file,constants.O_RDONLY|constants.O_NOFOLLOW);
  try{
    const entries=await centralDirectory(handle,limits.entries),byName=new Map(entries.map(entry=>[entry.name,entry]));
    return {
      entries,
      get:(name:string)=>byName.get(name),
      /** The entry's bytes, refused when larger than `max` once inflated. */
      async read(entry:ZipEntry,max:number){
        if(entry.encrypted)throw new ZipError("Encrypted entry.");
        if(entry.size>max||entry.compressed>max*2+1024)throw new ZipError("Entry too large.");
        const local=Buffer.alloc(30);await readAt(handle,local,entry.offset);
        if(local.readUInt32LE(0)!==0x04034b50)throw new ZipError("Damaged archive.");
        const start=entry.offset+30+local.readUInt16LE(26)+local.readUInt16LE(28),data=Buffer.alloc(entry.compressed);
        await readAt(handle,data,start);
        if(entry.method===0){if(data.length!==entry.size)throw new ZipError("Damaged entry.");return data;}
        if(entry.method!==8)throw new ZipError("Unsupported compression.");
        const out=await inflate(data,{maxOutputLength:Math.max(1,entry.size)}).catch(()=>{throw new ZipError("Damaged entry.");});
        if(out.length!==entry.size)throw new ZipError("Damaged entry.");
        return out;
      },
      close:()=>handle.close(),
    };
  }catch(error){await handle.close();throw error;}
}
export type Zip=Awaited<ReturnType<typeof openZip>>;

async function readAt(handle:FileHandle,buffer:Buffer,position:number){
  const {bytesRead}=await handle.read(buffer,0,buffer.length,position);
  if(bytesRead!==buffer.length)throw new ZipError("Damaged archive.");
}
async function centralDirectory(handle:FileHandle,maxEntries:number):Promise<ZipEntry[]>{
  const {size}=await handle.stat(),tail=Buffer.alloc(Math.min(size,65557));
  if(size<22)throw new ZipError("Not a ZIP archive.");
  await readAt(handle,tail,size-tail.length);
  let end=-1;for(let i=tail.length-22;i>=0;i--)if(tail.readUInt32LE(i)===0x06054b50){end=i;break;}
  if(end<0)throw new ZipError("Not a ZIP archive.");
  let count=tail.readUInt16LE(end+10),length=tail.readUInt32LE(end+12),offset=tail.readUInt32LE(end+16);
  // ZIP64: the locator just before the end record points at the 64-bit end record.
  if(count===0xffff||length===0xffffffff||offset===0xffffffff){
    if(end<20||tail.readUInt32LE(end-20)!==0x07064b50)throw new ZipError("Damaged archive.");
    const record=Buffer.alloc(56);await readAt(handle,record,Number(tail.readBigUInt64LE(end-12)));
    if(record.readUInt32LE(0)!==0x06064b50)throw new ZipError("Damaged archive.");
    count=Number(record.readBigUInt64LE(32));length=Number(record.readBigUInt64LE(40));offset=Number(record.readBigUInt64LE(48));
  }
  if(count>maxEntries)throw new ZipError("Too many entries.");
  if(offset+length>size||length>64*1024*1024)throw new ZipError("Damaged archive.");
  const directory=Buffer.alloc(length);await readAt(handle,directory,offset);
  const entries:ZipEntry[]=[];
  for(let at=0;entries.length<count;){
    if(at+46>directory.length||directory.readUInt32LE(at)!==0x02014b50)throw new ZipError("Damaged archive.");
    const flags=directory.readUInt16LE(at+8),nameLength=directory.readUInt16LE(at+28),extraLength=directory.readUInt16LE(at+30),commentLength=directory.readUInt16LE(at+32);
    let compressed=directory.readUInt32LE(at+20),uncompressed=directory.readUInt32LE(at+24),local=directory.readUInt32LE(at+42);
    const nameBytes=directory.subarray(at+46,at+46+nameLength),extra=directory.subarray(at+46+nameLength,at+46+nameLength+extraLength);
    // ZIP64 extra field: only the fields marked 0xffffffff are present, in this order.
    for(let x=0;x+4<=extra.length;){
      const id=extra.readUInt16LE(x),len=extra.readUInt16LE(x+2);
      if(id===1){let y=x+4;const next=()=>{const value=Number(extra.readBigUInt64LE(y));y+=8;return value;};
        if(uncompressed===0xffffffff)uncompressed=next();if(compressed===0xffffffff)compressed=next();if(local===0xffffffff)local=next();}
      x+=4+len;
    }
    const name=flags&0x800?nameBytes.toString("utf8"):nameBytes.toString("latin1");
    if(local+30>size||compressed>size)throw new ZipError("Damaged archive.");
    entries.push({name,method:directory.readUInt16LE(at+10),compressed,size:uncompressed,offset:local,encrypted:!!(flags&1)});
    at+=46+nameLength+extraLength+commentLength;
  }
  return entries;
}
