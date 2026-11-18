import {createHash} from "node:crypto";
import type {IncomingMessage} from "node:http";
import type {Duplex} from "node:stream";

/** One browser's live socket: JSON text both ways, Opus audio as binary. */
export interface LiveSocket {
  sendText(value:unknown):void;
  /** Sends audio unless the browser is behind; late audio is worse than lost audio. */
  sendAudio(data:Uint8Array):void;
  close(code?:number,reason?:string):void;
  onText:(value:Record<string,unknown>)=>void;
  onBinary:(data:Buffer)=>void;
  onClose:()=>void;
}

const guid="258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const maxMessage=64*1024,behind=256*1024;

/** Refuses an upgrade with a plain HTTP status. */
export function refuseUpgrade(socket:Duplex,status:number,text:string){
  if(socket.writable)socket.end(`HTTP/1.1 ${status} ${text}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`);
  socket.destroy();
}

/**
 * A minimal RFC 6455 server side for Chat's live socket: one connection, masked client
 * frames, fragments joined up to 64 KiB, pings answered, and a ping every 30 s so dead
 * connections close. Nothing here knows about Chat.
 */
export function acceptWebSocket(req:IncomingMessage,socket:Duplex,head:Buffer):LiveSocket|undefined{
  const key=req.headers["sec-websocket-key"];
  if(String(req.headers.upgrade).toLowerCase()!=="websocket"||req.headers["sec-websocket-version"]!=="13"||typeof key!=="string"||!/^[A-Za-z0-9+/]{22}==$/.test(key)){
    refuseUpgrade(socket,400,"Bad Request");return undefined;
  }
  const accept=createHash("sha1").update(key+guid).digest("base64");
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
  let buffer:Buffer=head.length?Buffer.from(head):Buffer.alloc(0),fragments:Buffer[]=[],fragmentType=0,fragmentSize=0,closed=false,seen=Date.now();
  const frame=(op:number,payload:Uint8Array)=>{
    const length=payload.length,header=length<126?Buffer.from([0x80|op,length]):length<65536?Buffer.alloc(4):Buffer.alloc(10);
    if(length>=126&&length<65536){header[0]=0x80|op;header[1]=126;header.writeUInt16BE(length,2);}
    else if(length>=65536){header[0]=0x80|op;header[1]=127;header.writeBigUInt64BE(BigInt(length),2);}
    socket.write(Buffer.concat([header,payload]));
  };
  const live:LiveSocket={
    sendText(value){if(!closed)frame(1,Buffer.from(JSON.stringify(value)));},
    sendAudio(data){if(!closed&&socket.writableLength<behind)frame(2,data);},
    close(code=1000,reason=""){
      if(closed)return;closed=true;clearInterval(timer);
      const body=Buffer.alloc(2+Buffer.byteLength(reason));body.writeUInt16BE(code,0);body.write(reason,2);
      try{frame(8,body);}catch{}
      socket.end();setTimeout(()=>socket.destroy(),1000).unref();
      live.onClose();
    },
    onText:()=>{},onBinary:()=>{},onClose:()=>{},
  };
  const timer=setInterval(()=>{
    if(Date.now()-seen>75_000){live.close(1001,"timeout");return;}
    if(!closed)frame(9,Buffer.alloc(0));
  },30_000);
  timer.unref();
  const fail=(code:number)=>{live.close(code);};
  socket.on("data",(chunk:Buffer)=>{
    seen=Date.now();
    buffer=buffer.length?Buffer.concat([buffer,chunk]):chunk;
    while(!closed&&buffer.length>=2){
      const fin=(buffer[0]&0x80)!==0,op=buffer[0]&0x0f,masked=(buffer[1]&0x80)!==0;
      let length=buffer[1]&0x7f,offset=2;
      if(buffer[0]&0x70)return fail(1002);
      if(!masked)return fail(1002);
      if(length===126){if(buffer.length<4)return;length=buffer.readUInt16BE(2);offset=4;}
      else if(length===127){if(buffer.length<10)return;const big=buffer.readBigUInt64BE(2);if(big>BigInt(maxMessage))return fail(1009);length=Number(big);offset=10;}
      if(length>maxMessage)return fail(1009);
      if(buffer.length<offset+4+length)return;
      const mask=buffer.subarray(offset,offset+4),payload=Buffer.from(buffer.subarray(offset+4,offset+4+length));
      for(let i=0;i<payload.length;i++)payload[i]^=mask[i&3];
      buffer=buffer.subarray(offset+4+length);
      if(op>=8){
        if(!fin||length>125)return fail(1002);
        if(op===8){live.close(1000);return;}
        if(op===9)frame(10,payload);
        continue;
      }
      if(op===0){if(!fragmentType)return fail(1002);}
      else if(op===1||op===2){if(fragmentType)return fail(1002);fragmentType=op;}
      else return fail(1003);
      fragments.push(payload);fragmentSize+=payload.length;
      if(fragmentSize>maxMessage)return fail(1009);
      if(!fin)continue;
      const message=fragments.length===1?fragments[0]:Buffer.concat(fragments),type=fragmentType;
      fragments=[];fragmentType=0;fragmentSize=0;
      if(type===2){try{live.onBinary(message);}catch{}continue;}
      let value:unknown;
      try{value=JSON.parse(message.toString("utf8"));}catch{return fail(1007);}
      if(!value||typeof value!=="object"||Array.isArray(value))return fail(1003);
      try{live.onText(value as Record<string,unknown>);}catch{}
    }
  });
  const gone=()=>{if(!closed){closed=true;clearInterval(timer);live.onClose();}};
  socket.on("close",gone);socket.on("end",gone);socket.on("error",()=>{socket.destroy();gone();});
  return live;
}
