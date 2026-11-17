import assert from "node:assert/strict";
import {test} from "node:test";
import http from "node:http";
import type {AddressInfo} from "node:net";
import {watchBody} from "../runtime/upload-pace.ts";

/** Request body pacing with short limits: moving bodies finish, stalled or crawling ones are dropped. */
const pace={idleMs:400,windowMs:600,minimumBytesPerWindow:600,capMs:5_000,checkMs:50};
async function server(){
  const instance=http.createServer((req,res)=>{
    watchBody(req,res,pace);
    let size=0;req.on("data",chunk=>{size+=chunk.length;});
    req.on("end",()=>{if(!res.headersSent){res.writeHead(200);res.end(String(size));}});
  });
  instance.requestTimeout=0;
  await new Promise<void>(resolve=>instance.listen(0,"127.0.0.1",resolve));
  return {instance,port:(instance.address() as AddressInfo).port};
}
/** Sends `chunks` 100-byte pieces `every` ms apart, then waits up to `stallMs` before ending. */
function send(port:number,chunks:number,every:number,stallMs=0){
  return new Promise<{status:number|"closed";elapsed:number}>(resolve=>{
    const started=Date.now(),request=http.request({port,host:"127.0.0.1",method:"PUT",path:"/",headers:{"content-length":chunks*100+(stallMs?100:0)}},res=>{res.resume();res.on("end",()=>resolve({status:res.statusCode!,elapsed:Date.now()-started}));});
    request.on("error",()=>resolve({status:"closed",elapsed:Date.now()-started}));
    let sent=0;const tick=setInterval(()=>{
      if(sent<chunks){request.write(Buffer.alloc(100,1));sent++;return;}
      clearInterval(tick);if(!stallMs)request.end();else setTimeout(()=>{if(!request.destroyed)request.end(Buffer.alloc(100,1));},stallMs);
    },every);
  });
}

test("upload pace: slow but steady bodies finish past every window",async()=>{
  const {instance,port}=await server();
  try{
    // 2.4 s at 2,000 bytes a second, twice the minimum so a late timer under load cannot
    // starve a window: several windows and six times the idle limit.
    const steady=await send(port,48,50);
    assert.equal(steady.status,200);assert.ok(steady.elapsed>2000);
  }finally{instance.close();}
});
test("upload pace: a stalled body is dropped after the idle limit",async()=>{
  const {instance,port}=await server();
  try{
    const stalled=await send(port,3,20,3000);
    assert.ok(stalled.status===408||stalled.status==="closed",String(stalled.status));
    assert.ok(stalled.elapsed>=400&&stalled.elapsed<2000,`${stalled.elapsed} ms`);
  }finally{instance.close();}
});
test("upload pace: a body that crawls below the minimum rate is dropped",async()=>{
  const {instance,port}=await server();
  try{
    // 100 bytes every 300 ms never idles, but moves about 200 bytes per 600 ms window.
    const crawl=await send(port,20,300);
    assert.ok(crawl.status===408||crawl.status==="closed",String(crawl.status));
    assert.ok(crawl.elapsed<3000,`${crawl.elapsed} ms`);
  }finally{instance.close();}
});
test("upload pace: requests without a body are not watched",async()=>{
  const {instance,port}=await server();
  try{
    const response=await fetch(`http://127.0.0.1:${port}/`);assert.equal(response.status,200);assert.equal(await response.text(),"0");
  }finally{instance.close();}
});
