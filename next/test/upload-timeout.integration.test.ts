import assert from "node:assert/strict";
import {test} from "node:test";
import http from "node:http";
import net from "node:net";
import {login} from "./integration-helpers.ts";
const origin=process.env.DAR_INTEGRATION_ORIGIN;

/**
 * Large uploads on slow links: a body that keeps moving may take longer than the minute that used
 * to cap every request, while slow request headers are still cut off quickly.
 */
test("uploads finish past a minute on a slow link; slow headers are still dropped",{skip:!origin,timeout:180_000},async()=>{
  const base=new URL(origin!),session=await login(origin!),chunk=1024,seconds=75;
  const reservation=await (await fetch(`${origin}/api/v1/files/uploads`,{method:"POST",headers:{...session.headers,"content-type":"application/json"},
    body:JSON.stringify({name:`slow-${Date.now()}.bin`,size:chunk*seconds,mediaType:"application/octet-stream",parentId:""})})).json() as {id:string};
  const slow=new Promise<{status:number;elapsed:number}>((resolve,reject)=>{
    const started=Date.now(),request=http.request({host:base.hostname,port:base.port,method:"PUT",path:`/api/v1/files/uploads/${reservation.id}`,headers:{...session.headers,"content-length":chunk*seconds}},
      res=>{res.resume();res.on("end",()=>resolve({status:res.statusCode!,elapsed:Date.now()-started}));});
    request.on("error",reject);
    let sent=0;const tick=setInterval(()=>{request.write(Buffer.alloc(chunk,sent%251));if(++sent===seconds){clearInterval(tick);request.end();}},1000);
  });
  const headers=new Promise<number>(resolve=>{
    // A request line and then nothing: the header timeout (15 s) still closes the connection.
    const started=Date.now(),socket=net.connect(Number(base.port),base.hostname,()=>socket.write("GET / HTTP/1.1\r\nHost: dar\r\n"));
    socket.on("close",()=>resolve(Date.now()-started));socket.on("error",()=>{});socket.resume();
  });
  const [upload,closedAfter]=await Promise.all([slow,headers]);
  assert.equal(upload.status,200,"a 75-second upload completes");assert.ok(upload.elapsed>=seconds*1000-2000);
  assert.ok(closedAfter<40_000,`slow headers were dropped after ${closedAfter} ms`);
});
