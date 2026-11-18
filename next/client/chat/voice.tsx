import {useEffect,useMemo,useState,useSyncExternalStore} from "react";
import {request,type BrowserSession} from "../api.ts";
import {Icon} from "../icons.tsx";
import {Dialog} from "../dialog.tsx";
import {avatarHue} from "../pim/common.tsx";
import type {ChatAccount,ChatLiveMessage,ChatThread,ChatVoiceCall,ChatVoiceMember,ChatVoiceServers} from "../../shared/chat.ts";

const base="/api/v1/chat";

/** One Discord account's voice: who is where, its call, and whether this tab carries the call's audio. */
export interface AccountVoice {servers:ChatVoiceServers;call:ChatVoiceCall|null;here:boolean;elsewhere:boolean;online:boolean}
export type MicState="idle"|"on"|"blocked"|"unsupported";
export interface VoiceSnapshot {accounts:Record<string,AccountVoice>;speaking:ReadonlySet<string>;mic:MicState;ended?:{accountId:string;message:string;at:number}}
const empty:AccountVoice={servers:{},call:null,here:false,elsewhere:false,online:false};
export const voiceSupported=()=>typeof AudioEncoder!=="undefined"&&typeof AudioDecoder!=="undefined"&&typeof AudioWorkletNode!=="undefined"&&!!navigator.mediaDevices;

/**
 * The browser's audio for a call: the microphone in 20 ms frames, sent as Opus while the
 * person speaks, and everyone else's Opus decoded and played with a short jitter buffer.
 */
class VoiceAudio {
  private ctx?:AudioContext;private master?:GainNode;private stream?:MediaStream;private encoder?:AudioEncoder;
  private decoders=new Map<string,{decoder:AudioDecoder;next:number}>();private heard=new Map<string,number>();
  private muted=true;private deaf=false;private talking=false;private hang=0;private previous?:Float32Array<ArrayBuffer>;private time=0;private timer?:ReturnType<typeof setInterval>;private shown="";
  send?:(frame:Uint8Array<ArrayBuffer>)=>void;
  private me:string;private onSpeaking:(ids:Set<string>)=>void;
  constructor(me:string,onSpeaking:(ids:Set<string>)=>void){this.me=me;this.onSpeaking=onSpeaking;}
  /** Starts audio from a click (browsers need one) and asks for the microphone. */
  async open():Promise<MicState>{
    if(this.ctx){await this.ctx.resume();return this.encoder?"on":"blocked";}
    const ctx=this.ctx=new AudioContext({sampleRate:48000,latencyHint:"interactive"});
    this.master=ctx.createGain();this.master.connect(ctx.destination);
    await ctx.resume().catch(()=>{});
    this.timer=setInterval(()=>this.tick(),100);
    try{
      await ctx.audioWorklet.addModule(__DAR_MIC_WORKLET__);
      this.stream=await navigator.mediaDevices.getUserMedia({audio:{echoCancellation:true,noiseSuppression:true,autoGainControl:true,channelCount:1}});
      const source=ctx.createMediaStreamSource(this.stream),node=new AudioWorkletNode(ctx,"dar-mic",{numberOfInputs:1,numberOfOutputs:1}),silent=ctx.createGain();
      silent.gain.value=0;source.connect(node);node.connect(silent);silent.connect(ctx.destination);
      node.port.onmessage=event=>this.frame(event.data as Float32Array<ArrayBuffer>);
      this.encoder=new AudioEncoder({output:chunk=>{const frame=new Uint8Array(1+chunk.byteLength);frame[0]=2;chunk.copyTo(frame.subarray(1));this.send?.(frame);},error:()=>{}});
      this.encoder.configure({codec:"opus",sampleRate:48000,numberOfChannels:1,bitrate:64000,opus:{frameDuration:20000}} as AudioEncoderConfig);
      return "on";
    }catch{this.stream?.getTracks().forEach(t=>t.stop());this.stream=undefined;return "blocked";}
  }
  set(muted:boolean,deaf:boolean){this.muted=muted||deaf;this.deaf=deaf;if(this.master)this.master.gain.value=deaf?0:1;if(this.muted)this.talking=false;}
  /** Voice activity: frames go out while the level is above a speaking threshold, plus 300 ms. */
  private frame(samples:Float32Array<ArrayBuffer>){
    if(!this.encoder||this.encoder.state!=="configured")return;
    let sum=0;for(const s of samples)sum+=s*s;
    const loud=Math.sqrt(sum/samples.length)>0.012;
    if(this.muted||!this.send){this.talking=false;this.previous=samples;return;}
    this.hang=loud?15:Math.max(0,this.hang-1);
    const sending=this.hang>0;
    if(sending&&!this.talking&&this.previous)this.encode(this.previous);
    if(sending)this.encode(samples);
    this.talking=sending;this.previous=samples;
  }
  private encode(samples:Float32Array<ArrayBuffer>){
    const data=new AudioData({format:"f32",sampleRate:48000,numberOfFrames:samples.length,numberOfChannels:1,timestamp:this.time,data:samples});
    this.time+=20_000;this.encoder!.encode(data);data.close();
  }
  play(user:string,opus:Uint8Array<ArrayBuffer>){
    if(!this.ctx||this.deaf)return;
    this.heard.set(user,performance.now());
    let d=this.decoders.get(user);
    if(!d){
      const entry={next:0,decoder:new AudioDecoder({output:data=>this.schedule(entry,data),error:()=>{this.decoders.delete(user);}})};
      entry.decoder.configure({codec:"opus",sampleRate:48000,numberOfChannels:2});this.decoders.set(user,entry);d=entry;
    }
    if(d.decoder.state==="configured")d.decoder.decode(new EncodedAudioChunk({type:"key",timestamp:Math.round(performance.now()*1000),data:opus}));
  }
  private schedule(entry:{next:number},data:AudioData){
    const ctx=this.ctx;
    if(!ctx){data.close();return;}
    const buffer=ctx.createBuffer(data.numberOfChannels,data.numberOfFrames,data.sampleRate);
    for(let c=0;c<data.numberOfChannels;c++)data.copyTo(buffer.getChannelData(c),{planeIndex:c,format:"f32-planar"});
    data.close();
    const now=ctx.currentTime;
    // 60 ms of buffer absorbs network jitter; a late or drifting stream starts again from there.
    if(entry.next<now+0.01||entry.next>now+0.4)entry.next=now+0.06;
    const source=ctx.createBufferSource();source.buffer=buffer;source.connect(this.master!);source.start(entry.next);entry.next+=buffer.duration;
  }
  private tick(){
    const now=performance.now(),ids=new Set<string>();
    for(const [user,at] of this.heard)if(now-at<300)ids.add(user);else this.heard.delete(user);
    if(this.talking)ids.add(this.me);
    const key=[...ids].sort().join();
    if(key!==this.shown){this.shown=key;this.onSpeaking(ids);}
  }
  close(){
    clearInterval(this.timer);this.stream?.getTracks().forEach(t=>t.stop());
    try{this.encoder?.close();}catch{}
    for(const d of this.decoders.values())try{d.decoder.close();}catch{}
    this.decoders.clear();void this.ctx?.close().catch(()=>{});this.ctx=undefined;this.onSpeaking(new Set());
  }
}

interface Link {socket?:WebSocket;closed:boolean;retry:number;timer?:ReturnType<typeof setTimeout>;wantHere:boolean;me:string}

/** Live sockets for the person's Discord accounts and the one call this tab carries. */
class VoiceHub {
  private links=new Map<string,Link>();private listeners=new Set<()=>void>();private audio?:VoiceAudio;private audioAccount="";
  snapshot:VoiceSnapshot={accounts:{},speaking:new Set(),mic:voiceSupported()?"idle":"unsupported"};
  private session:BrowserSession;
  constructor(session:BrowserSession){this.session=session;}
  subscribe=(listener:()=>void)=>{this.listeners.add(listener);return()=>{this.listeners.delete(listener);};};
  get=()=>this.snapshot;
  private set(change:Partial<VoiceSnapshot>){this.snapshot={...this.snapshot,...change};for(const l of this.listeners)l();}
  private update(id:string,change:Partial<AccountVoice>){
    this.set({accounts:{...this.snapshot.accounts,[id]:{...this.snapshot.accounts[id]??empty,...change}}});this.follow();
  }
  /** Keeps a socket open for each account, closing those no longer listed. */
  sync(accounts:{id:string;me:string}[]){
    for(const [id,link] of this.links)if(!accounts.some(a=>a.id===id)){link.closed=true;clearTimeout(link.timer);link.socket?.close();this.links.delete(id);}
    for(const a of accounts)if(!this.links.has(a.id)){const link:Link={closed:false,retry:0,wantHere:false,me:a.me};this.links.set(a.id,link);this.open(a.id,link);}
  }
  private open(id:string,link:Link){
    if(link.closed)return;
    const socket=new WebSocket(`${location.protocol==="https:"?"wss":"ws"}://${location.host}${base}/live?${new URLSearchParams({accountId:id})}`);
    socket.binaryType="arraybuffer";link.socket=socket;
    socket.onopen=()=>{link.retry=0;};
    socket.onmessage=event=>{
      if(event.data instanceof ArrayBuffer){
        const bytes=new Uint8Array(event.data);
        if(bytes[0]===1&&bytes.length>9&&this.audioAccount===id)this.audio?.play(new DataView(event.data).getBigUint64(1).toString(),bytes.subarray(9));
        return;
      }
      let message:ChatLiveMessage;try{message=JSON.parse(String(event.data));}catch{return;}
      this.receive(id,link,message);
    };
    socket.onclose=()=>{
      if(link.socket!==socket)return;link.socket=undefined;
      this.update(id,{online:false,here:false,elsewhere:false});
      if(link.closed)return;
      link.timer=setTimeout(()=>this.open(id,link),Math.min(15_000,1000*2**link.retry++));
    };
  }
  private receive(id:string,link:Link,m:ChatLiveMessage){
    if(m.type==="hello"){
      this.update(id,{servers:m.servers,call:m.call,online:true});
      if(link.wantHere&&m.call)this.claim(id);
    }else if(m.type==="voice"){
      const servers=m.complete?{}:{...this.snapshot.accounts[id]?.servers};
      for(const [server,channels] of Object.entries(m.servers)){if(Object.keys(channels).length)servers[server]=channels;else delete servers[server];}
      this.update(id,{servers});
    }else if(m.type==="call"){
      if(!m.call){link.wantHere=false;if(m.ended?.message)this.set({ended:{accountId:id,message:m.ended.message,at:Date.now()}});}
      this.update(id,{call:m.call});
    }else if(m.type==="owner"){
      if(!m.here&&m.elsewhere)link.wantHere=false;
      this.update(id,{here:m.here,elsewhere:m.elsewhere});
    }
  }
  private claim(id:string){this.links.get(id)?.socket?.send(JSON.stringify({type:"claim"}));}
  /** Runs audio for the call this tab carries, and stops it when there is none. */
  private follow(){
    const entry=Object.entries(this.snapshot.accounts).find(([,v])=>v.here&&v.call);
    if(!entry){if(this.audio){this.audio.close();this.audio=undefined;this.audioAccount="";}return;}
    const [id,v]=entry,link=this.links.get(id);
    if(!this.audio||this.audioAccount!==id){this.audio?.close();this.audio=new VoiceAudio(link?.me??"",ids=>this.set({speaking:ids}));this.audioAccount=id;void this.audio.open().then(mic=>this.set({mic}));}
    this.audio.send=frame=>{const socket=this.links.get(id)?.socket;if(socket?.readyState===WebSocket.OPEN&&socket.bufferedAmount<64_000)socket.send(frame);};
    this.audio.set(v.call!.selfMute,v.call!.selfDeaf);
  }
  /** Prepares audio in the click that joins, so the browser allows sound and asks for the microphone. */
  private async prepare(id:string){
    const link=this.links.get(id);
    if(!this.audio||this.audioAccount!==id){this.audio?.close();this.audio=new VoiceAudio(link?.me??"",ids=>this.set({speaking:ids}));this.audioAccount=id;}
    const mic=await this.audio.open();this.set({mic});return mic;
  }
  async join(accountId:string,thread:ChatThread,muted:boolean){
    if(!voiceSupported())throw new Error("This browser cannot take part in voice calls.");
    for(const [id,v] of Object.entries(this.snapshot.accounts))if(id!==accountId&&v.here&&v.call)await this.leave(id);
    const mic=await this.prepare(accountId),link=this.links.get(accountId);
    if(link)link.wantHere=true;
    try{await request(`${base}/voice/join`,{method:"POST",body:JSON.stringify({threadId:thread.id,muted:muted||mic!=="on"})},this.session.csrfToken);}
    catch(error){if(link)link.wantHere=false;this.follow();throw error;}
    this.claim(accountId);
  }
  async leave(accountId:string){
    const link=this.links.get(accountId);if(link)link.wantHere=false;
    await request(`${base}/voice/leave`,{method:"POST",body:JSON.stringify({accountId})},this.session.csrfToken);
  }
  async setSelf(accountId:string,muted:boolean,deafened:boolean){
    if(!muted&&this.snapshot.mic==="blocked"){const mic=await this.prepare(accountId);if(mic!=="on")throw new Error("Allow the microphone for this site in the browser, then try again.");}
    await request(`${base}/voice/set`,{method:"POST",body:JSON.stringify({accountId,muted,deafened})},this.session.csrfToken);
  }
  async useHere(accountId:string){
    const link=this.links.get(accountId);if(!link)return;
    await this.prepare(accountId);link.wantHere=true;this.claim(accountId);
  }
  code=(accountId:string)=>request<{code:string}>(`${base}/voice/code?${new URLSearchParams({accountId})}`);
  dispose(){for(const link of this.links.values()){link.closed=true;clearTimeout(link.timer);link.socket?.close();}this.links.clear();this.audio?.close();this.audio=undefined;}
}

export type Voice=VoiceHub;
/** Voice for the given Discord accounts; sockets live as long as the Chat page. */
export function useVoice(session:BrowserSession,accounts:ChatAccount[]){
  const hub=useMemo(()=>new VoiceHub(session),[]);
  const snapshot=useSyncExternalStore(hub.subscribe,hub.get);
  const list=accounts.filter(a=>a.platform==="discord"&&a.state!=="loggedOut"&&a.state!=="linking"&&a.state!=="failed").map(a=>({id:a.id,me:a.jid}));
  const key=list.map(a=>a.id).join();
  useEffect(()=>{hub.sync(list);},[key]);
  useEffect(()=>()=>hub.dispose(),[]);
  return {hub,snapshot};
}

const initials=(name:string)=>{const words=name.trim().split(/\s+/).filter(Boolean);return words.length?(words[0][0]+(words.length>1?words[words.length-1][0]:"")).toUpperCase():"?";};

/** People in a voice channel, under it in the server tree. */
export function VoiceMembers({members,speaking,me}:{members:ChatVoiceMember[];speaking:ReadonlySet<string>;me:string}){
  return <ul className="chat-voice-members" aria-label="In voice">{members.map(m=>{const talking=speaking.has(m.id);
    return <li key={m.id} className={`chat-voice-member${talking?" is-speaking":""}`}>
      <span className={`chat-voice-avatar contact-initials ${avatarHue(m.id)}`} aria-hidden="true">{initials(m.name)}</span>
      <span className="chat-row-title">{m.name}{m.id===me?" (you)":""}{talking&&<span className="sr-only">, speaking</span>}</span>
      <Marks member={m}/>
    </li>;})}</ul>;
}

function Marks({member:m}:{member:ChatVoiceMember}){
  if(!m.muted&&!m.deafened)return null;
  return <span className="chat-voice-marks">{m.muted&&<span title="Muted"><Icon name="micOff"/><span className="sr-only">, muted</span></span>}{m.deafened&&<span title="Deafened"><Icon name="headphonesOff"/><span className="sr-only">, deafened</span></span>}</span>;
}

const stateLabel=(call:ChatVoiceCall)=>call.state==="connected"?"Voice connected":call.state==="reconnecting"?"Reconnecting…":"Connecting…";

function Controls({call,mic,big,onSet,onLeave}:{call:ChatVoiceCall;mic:MicState;big?:boolean;onSet:(muted:boolean,deafened:boolean)=>void;onLeave:()=>void}){
  // Unmuting also undeafens, as in Discord; deafening keeps the microphone setting for afterwards.
  const muted=call.selfMute||call.selfDeaf||mic!=="on";
  return <div className={`chat-voice-controls${big?" is-big":""}`}>
    <button type="button" className={`chat-voice-button${muted?" is-off":""}`} aria-pressed={muted} disabled={!call.canSpeak} title={!call.canSpeak?"You cannot speak in this channel":muted?"Unmute":"Mute"} aria-label="Mute" onClick={()=>onSet(!muted,muted?false:call.selfDeaf)}>
      <Icon name={muted?"micOff":"mic"}/></button>
    <button type="button" className={`chat-voice-button${call.selfDeaf?" is-off":""}`} aria-pressed={call.selfDeaf} aria-label="Deafen" title={call.selfDeaf?"Undeafen":"Deafen"} onClick={()=>onSet(call.selfMute,!call.selfDeaf)}>
      <Icon name={call.selfDeaf?"headphonesOff":"headphones"}/></button>
    <button type="button" className={`chat-voice-button is-leave${big?" is-wide":""}`} aria-label="Leave voice" title="Leave voice" onClick={onLeave}><Icon name="phoneOff"/>{big&&<span>Leave</span>}</button>
  </div>;
}

/** The call at the bottom of the sidebar while you keep reading and writing. */
export function VoiceDock({call,channel,server,here,elsewhere,mic,className="",onSet,onLeave,onUseHere,onOpen,onCode}:{call:ChatVoiceCall;channel:string;server:string;here:boolean;elsewhere:boolean;mic:MicState;className?:string;
  onSet:(muted:boolean,deafened:boolean)=>void;onLeave:()=>void;onUseHere:()=>void;onOpen:()=>void;onCode:()=>void}){
  return <section className={`chat-voice-dock${className}`} aria-label="Voice call">
    <div className="chat-voice-dock-head">
      <button type="button" className="chat-voice-dock-open" onClick={onOpen}><span className={`chat-voice-live is-${call.state}`} aria-hidden="true"><Icon name="signal"/></span>
        <span className="chat-voice-dock-text"><b>{stateLabel(call)}</b><small>{channel} · {server}</small></span></button>
      {call.e2ee&&<button type="button" className="icon-button chat-voice-lock" aria-label="End-to-end encrypted: verify the call code" title="End-to-end encrypted" onClick={onCode}><Icon name="lock"/></button>}
    </div>
    {!here&&<div className="chat-voice-away"><span>{elsewhere?"The call’s audio is in another tab.":"The call’s audio is not playing here."}</span><button type="button" className="secondary-button" onClick={onUseHere}>Use this tab</button></div>}
    {here&&mic==="blocked"&&<p className="chat-voice-note"><Icon name="micOff"/>Microphone blocked. You can still listen.</p>}
    <Controls call={call} mic={here?mic:"on"} onSet={onSet} onLeave={onLeave}/>
  </section>;
}

/** A voice channel opened from the tree: who is in it, and joining or the call's controls. */
export function VoiceStage({thread,server,members,call,inOther,speaking,me,mic,here,onJoin,onSet,onLeave,onCode,onUseHere,onClose}:{thread:ChatThread;server:string;members:ChatVoiceMember[];call:ChatVoiceCall|null;inOther:string;
  speaking:ReadonlySet<string>;me:string;mic:MicState;here:boolean;onJoin:(muted:boolean)=>Promise<void>;onSet:(muted:boolean,deafened:boolean)=>void;onLeave:()=>void;onCode:()=>void;onUseHere:()=>void;onClose:()=>void}){
  const [joinMuted,setJoinMuted]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const inThis=!!call&&call.channelId===thread.jid,supported=voiceSupported();
  const count=members.length;
  async function join(){setBusy(true);setError("");try{await onJoin(joinMuted);}catch(cause){setError((cause as Error).message);}finally{setBusy(false);}}
  return <section className="chat-voice-stage" aria-label={`Voice channel ${thread.name}`}>
    <header className="chat-conversation-head">
      <button type="button" className="icon-button chat-back" aria-label="Back to chats" onClick={onClose}><Icon name="back"/></button>
      <span className={`chat-channel-mark${inThis?" is-live":""}`} aria-hidden="true"><Icon name="speaker"/></span>
      <div className="chat-who"><h2 tabIndex={-1}>{thread.name}</h2><small>{server} · {count?`${count} in voice`:"nobody in voice"}</small></div>
      {inThis&&call.e2ee&&<button type="button" className="chat-voice-e2ee" onClick={onCode}><Icon name="lock"/><span>End-to-end encrypted</span></button>}
      <button type="button" className="icon-button chat-close" aria-label="Close voice channel" onClick={onClose}><Icon name="close"/></button>
    </header>
    {inThis?<>
      <div className="chat-voice-grid" role="list" aria-label="In the call">{members.map(m=>{const talking=speaking.has(m.id);
        return <div key={m.id} role="listitem" className={`chat-voice-tile${talking?" is-speaking":""}`}>
          <span className={`chat-voice-tile-avatar contact-initials ${avatarHue(m.id)}`} aria-hidden="true">{initials(m.name)}</span>
          <span className="chat-voice-tile-name">{m.name}{m.id===me?" (you)":""}<Marks member={m}/>{talking&&<span className="sr-only">, speaking</span>}</span>
        </div>;})}
        {!members.length&&<p className="chat-hint">Waiting for Discord…</p>}</div>
      {!here&&<div className="chat-banner" role="status"><Icon name="info"/><span><b>The call’s audio is not playing here</b>It is in another tab, or this page was reloaded.</span><button className="secondary-button" onClick={onUseHere}>Use this tab</button></div>}
      {here&&mic==="blocked"&&<div className="chat-banner is-error" role="alert"><Icon name="micOff"/><span><b>Microphone blocked</b>Allow the microphone for this site in the browser, then unmute. You can still listen.</span></div>}
      <div className="chat-voice-bar"><span className={`chat-voice-status is-${call.state}`}><Icon name="signal"/><b>{stateLabel(call)}</b></span>
        <Controls call={call} mic={here?mic:"on"} big onSet={onSet} onLeave={onLeave}/></div>
    </>:<div className="chat-voice-join">
      <span className="chat-empty-icon"><Icon name="speaker"/></span>
      <h3>{count?`${count===1?"1 person is":`${count} people are`} in ${thread.name}`:`${thread.name} is empty`}</h3>
      {count>0&&<ul className="chat-voice-preview" aria-label="In voice">{members.slice(0,8).map(m=><li key={m.id} title={m.name}><span className={`chat-voice-avatar contact-initials ${avatarHue(m.id)}`} aria-hidden="true">{initials(m.name)}</span><span className="sr-only">{m.name}</span></li>)}</ul>}
      {supported?<>
        <p>{inOther?`Joining leaves ${inOther}.`:"Join to talk. Your browser asks for the microphone the first time."}</p>
        <label className="chat-voice-option"><input type="checkbox" checked={joinMuted} onChange={event=>setJoinMuted(event.target.checked)}/>Join muted</label>
        <button type="button" className="primary-button chat-voice-join-button" disabled={busy} onClick={()=>{void join();}}><Icon name="headphones"/>{busy?"Joining…":"Join voice"}</button>
      </>:<p>This browser cannot take part in voice calls. Use a current version of Chrome, Edge, Firefox or Safari.</p>}
      {error&&<p className="error" role="alert">{error}</p>}
    </div>}
  </section>;
}

/** The 30-digit code everyone in an end-to-end encrypted call sees. */
export function CallCodeDialog({load,onClose}:{load:()=>Promise<{code:string}>;onClose:()=>void}){
  const [code,setCode]=useState(""),[error,setError]=useState("");
  useEffect(()=>{void load().then(r=>setCode(r.code),cause=>setError((cause as Error).message));},[]);
  const groups=code.replace(/\s+/g,"").match(/.{1,5}/g)??[];
  return <Dialog title="Verify the call is private" onClose={onClose}>
    <p>Everyone in the call sees the same code. Compare it with someone over another channel; if it matches, nobody else can listen.</p>
    {code?<p className="chat-call-code" aria-label={`Call code ${groups.join(" ")}`}>{groups.map((g,i)=><span key={i}>{g}</span>)}</p>:error?<p className="error" role="alert">{error}</p>:<p role="status">Loading the code…</p>}
    <div className="dialog-actions"><button type="button" className="secondary-button" onClick={onClose}>Close</button></div>
  </Dialog>;
}
