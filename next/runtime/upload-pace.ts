import type {IncomingMessage,ServerResponse} from "node:http";

/**
 * Request bodies may take as long as they keep moving. Node's total request timeout would cut off
 * a large Files upload or `git push` on a slow link, so the server turns it off and this watches
 * each body instead: it is dropped after a minute without new bytes, when it moves slower than
 * the minimum rate over a minute, or after the overall cap. Headers keep their own short timeout.
 * Progress is read from the socket's byte count, so the body stream itself is left untouched.
 */
export const uploadPace={idleMs:60_000,windowMs:60_000,minimumBytesPerWindow:16*1024,capMs:6*3600_000,checkMs:5_000};

export function watchBody(req:IncomingMessage,res:ServerResponse,pace=uploadPace){
  const length=Number(req.headers["content-length"]??0);
  if(!(length>0||req.headers["transfer-encoding"]))return;
  const socket=req.socket,started=Date.now();
  let lastBytes=socket.bytesRead,lastMoved=started,windowStart=started,windowBytes=0;
  const timer=setInterval(()=>{
    if(req.complete||socket.destroyed){stop();return;}
    const now=Date.now(),read=socket.bytesRead,moved=read-lastBytes;
    if(moved>0){lastBytes=read;lastMoved=now;windowBytes+=moved;}
    const slow=now-windowStart>=pace.windowMs&&windowBytes<pace.minimumBytesPerWindow;
    if(now-windowStart>=pace.windowMs){windowStart=now;windowBytes=0;}
    if(now-lastMoved>=pace.idleMs||slow||now-started>=pace.capMs){
      stop();
      if(!res.headersSent){res.writeHead(408,{"content-type":"application/json","connection":"close"});res.end(JSON.stringify({error:{code:"requestTimeout",message:"The upload stopped or slowed down too much."}}));}
      // The rest of the body is not wanted; the connection cannot be reused.
      setTimeout(()=>req.destroy(),1000).unref();
    }
  },pace.checkMs);
  timer.unref();
  function stop(){clearInterval(timer);}
  req.once("end",stop);res.once("close",stop);res.once("finish",stop);
}
