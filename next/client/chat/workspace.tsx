import {Fragment,useCallback,useEffect,useLayoutEffect,useMemo,useRef,useState,type FormEvent,type KeyboardEvent,type ReactNode} from "react";
import {request,formatBytes,type BrowserSession} from "../api.ts";
import {FileTile} from "../file-types.tsx";
import {DarFilePicker,type DarFileSelection} from "../file-picker.tsx";
import {WorkspaceHeader,useMobileNavigation} from "../shell.tsx";
import {ActionMenu,NewItemButton,Select,type MenuAction} from "../controls.tsx";
import {EditorPane} from "../pane.tsx";
import {Dialog} from "../dialog.tsx";
import {Icon,type IconName} from "../icons.tsx";
import {Segmented,SettingsSheet,SettingRow} from "../settings-sheet.tsx";
import {useLiveConnection,subscribe} from "../live.ts";
import {avatarHue} from "../pim/common.tsx";
import {localDate,localDateTime,localTime} from "../dates.ts";
import {qrMatrix,qrPath} from "../../shared/qr.ts";
import {chatKindLabels,chatLimits,chatPlatformNames,chatTimeKey,localName,chatQuickReactions,chatMuteChoices,type ChatInfo,type ChatAccount,type ChatFriend,type ChatLinkState,type ChatMessage,type ChatMessagePage,type ChatOverview,type ChatPlatform,type ChatServer,type ChatThread,type ChatThreadPage,type ChatView} from "../../shared/chat.ts";

const base="/api/v1/chat";
const views:{id:ChatView;label:string;icon:IconName}[]=[{id:"chats",label:"Chats",icon:"chat"},{id:"unread",label:"Unread",icon:"unread"},{id:"groups",label:"Groups",icon:"contacts"},{id:"archived",label:"Archived",icon:"archive"}];
type Pane={kind:"link";accountId?:string;platform?:ChatPlatform}|{kind:"newChat"}|{kind:"settings"}|{kind:"rename";account:ChatAccount}|{kind:"save";threadId:string;message:ChatMessage}|{kind:"addContact";thread:ChatThread};
interface Confirm {title:string;text:string;action:string;run:()=>Promise<void>}
const kindIcon:Partial<Record<string,IconName>>={image:"image",video:"media",audio:"media",voice:"media",document:"file",sticker:"image",location:"location",contact:"user",poll:"checklist",unsupported:"info"};
const stateText:Record<string,string>={
  replaced:"Another copy of Dar is using this link. Unlink it and link again here.",banned:"WhatsApp has temporarily blocked this number.",outdated:"WhatsApp no longer accepts this version of the bridge. Update Dar.",
  unlinked:"You unlinked this account. Its chats stay here, read-only.",unlinkedHere:"Dar stopped using this link. Remove Dar under Linked devices on your phone too.",
  notLinked:"This account is no longer linked.",timeout:"The code expired before it was used.",passkey:"This WhatsApp account asks for a passkey, which Dar does not support yet.",
  authenticationFailed:"Discord no longer accepts this sign-in, after a new password or a sign-out elsewhere.",gateway:"Discord refused the connection.",
};
/** A custom Discord emoji is kept as "name:id" and shown as :name:. */
const emojiLabel=(emoji:string)=>/^[\w~-]+:\d+$/.test(emoji)?`:${emoji.split(":")[0]}:`:emoji;
const quietUntil=(until:number)=>until===-1||until>Date.now();

function initials(name:string){const words=name.replace(/^\+/,"").trim().split(/\s+/).filter(Boolean);if(!words.length)return "?";if(/^[0-9]/.test(words[0]))return words[0].slice(-2);return (words[0][0]+(words.length>1?words[words.length-1][0]:"")).toUpperCase();}
const phoneOf=(jid:string)=>jid.endsWith("@s.whatsapp.net")?"+"+jid.split("@")[0]:"";
function dayLabel(time:number){
  const day=new Date(time),today=new Date();today.setHours(0,0,0,0);const start=new Date(day);start.setHours(0,0,0,0);const diff=Math.round((today.getTime()-start.getTime())/86400000);
  return diff<=0?"Today":diff===1?"Yesterday":diff<7?localDate(day,{weekday:"long"}):localDate(day,{day:"numeric",month:"long",...day.getFullYear()!==today.getFullYear()?{year:"numeric"}:{}});
}
function listTime(time:number){
  if(!time)return "";const label=dayLabel(time);
  return label==="Today"?localTime(time,{hour:"numeric",minute:"2-digit"}):label==="Yesterday"?label:Date.now()-time<6*86400000?localDate(time,{weekday:"short"}):localDate(time,{month:"short",day:"numeric"});
}
const muted=(thread:ChatThread)=>thread.mutedUntil===-1||thread.mutedUntil>Date.now();
function readView(){const p=new URLSearchParams(location.search),view=p.get("view");return {account:p.get("account")??"",view:(views.some(v=>v.id===view)?view:"chats") as ChatView,chat:p.get("chat")??"",message:p.get("message")??""};}
function writeView(account:string,view:ChatView,chat:string){
  const p=new URLSearchParams();if(account)p.set("account",account);if(view!=="chats")p.set("view",view);if(chat)p.set("chat",chat);
  const next=`/chat${p.size?"?"+p:""}`;if(next!==location.pathname+location.search)history.replaceState(null,"",next);
}

function Avatar({name,group,size=""}:{name:string;group:boolean;size?:string}){
  return <span className={`chat-avatar contact-initials ${avatarHue(name)} ${size}`} aria-hidden="true">{group?<Icon name="contacts"/>:initials(name)}</span>;
}
function Ticks({status}:{status:string}){
  if(status==="pending")return <span className="chat-tick" title="Sending"><Icon name="clock"/><span className="sr-only">Sending</span></span>;
  if(status==="failed")return <span className="chat-tick is-failed" title="Not sent"><Icon name="alert"/><span className="sr-only">Not sent</span></span>;
  if(status==="sent")return <span className="chat-tick" title="Sent"><Icon name="check"/><span className="sr-only">Sent</span></span>;
  if(status==="delivered"||status==="read")return <span className={`chat-tick${status==="read"?" is-read":""}`} title={status==="read"?"Read":"Delivered"}><Icon name="checks"/><span className="sr-only">{status==="read"?"Read":"Delivered"}</span></span>;
  return null;
}
function preview(thread:ChatThread){
  if(thread.previewKind==="deleted")return "This message was deleted";
  const label=thread.previewKind?chatKindLabels[thread.previewKind]:"";
  return thread.preview?(label&&thread.previewKind!=="text"&&thread.previewKind!=="document"?`${label} · ${thread.preview}`:thread.preview):label;
}

/** Chat (selected mock A with account tabs): one account at a time, its chat list and the open conversation. */
export function ChatWorkspace({session,onSignedOut}:{session:BrowserSession;onSignedOut:()=>void}){
  const start=useMemo(readView,[]);
  const [overview,setOverview]=useState<ChatOverview>(),[loadError,setLoadError]=useState(""),[accountId,setAccountId]=useState(start.account),[view,setView]=useState<ChatView>(start.view);
  const [page,setPage]=useState<ChatThreadPage>(),[listState,setListState]=useState<"loading"|"ready"|"error">("loading"),[listError,setListError]=useState(""),[more,setMore]=useState(false);
  const [info,setInfo]=useState(false);
  const [selected,setSelected]=useState(start.chat),[pane,setPane]=useState<Pane>(),[confirm,setConfirm]=useState<Confirm>(),[notice,setNotice]=useState(""),[epoch,setEpoch]=useState(0);
  const {open:navigation,setOpen:setNavigation,id:navigationId,ref:navigationRef}=useMobileNavigation();
  const live=useLiveConnection(session),generation=useRef(0),origin=useRef<HTMLElement|null>(null);
  const accounts=overview?.accounts??[],account=accounts.find(a=>a.id===accountId)??accounts[0],discord=account?.platform==="discord";
  const [servers,setServers]=useState<ChatServer[]>(),[collapsed,setCollapsed]=useState<ReadonlySet<string>>(new Set());
  const fail=(cause:unknown)=>setNotice((cause as Error).message||"The request could not be completed.");
  useEffect(()=>{if(!notice)return;const timer=setTimeout(()=>setNotice(""),5000);return()=>clearTimeout(timer);},[notice]);

  const loadOverview=useCallback(async()=>{try{setOverview(await request<ChatOverview>(`${base}/overview`));setLoadError("");}catch(cause){setLoadError((cause as Error).message);}},[]);
  const loadThreads=useCallback(async(quiet=false)=>{
    if(!account){setPage(undefined);setListState("ready");return;}
    const ticket=++generation.current;if(!quiet)setListState("loading");
    try{const result=await request<ChatThreadPage>(`${base}/threads?${new URLSearchParams({accountId:account.id,view})}`);if(ticket!==generation.current)return;setPage(result);setListState("ready");}
    catch(cause){if(ticket===generation.current){setListState("error");setListError((cause as Error).message);}}
  },[account?.id,view]);
  async function loadMore(){
    if(!account||!page?.cursor||more)return;setMore(true);const ticket=generation.current;
    try{const next=await request<ChatThreadPage>(`${base}/threads?${new URLSearchParams({accountId:account.id,view,cursor:page.cursor})}`);if(ticket===generation.current)setPage(p=>p&&{...p,threads:[...p.threads,...next.threads.filter(t=>!p.threads.some(o=>o.id===t.id))],cursor:next.cursor});}
    catch(cause){fail(cause);}finally{setMore(false);}
  }
  useEffect(()=>{void loadOverview();},[]);
  useEffect(()=>{void loadThreads();},[loadThreads]);
  useEffect(()=>{if(epoch){void loadOverview();void loadThreads(true);}},[epoch]);
  // Discord: servers and their channels for the sidebar.
  useEffect(()=>{
    if(!discord||!account){setServers(undefined);return;}let gone=false;
    void request<{servers:ChatServer[]}>(`${base}/servers?${new URLSearchParams({accountId:account.id})}`).then(r=>{if(!gone)setServers(r.servers);},()=>{if(!gone)setServers([]);});
    return()=>{gone=true;};
  },[account?.id,discord,epoch]);
  useEffect(()=>{writeView(account?.id??"",view,selected);},[account?.id,view,selected]);
  // While linking or importing, nothing in the database changes for the browser to hear about the bridge; poll gently.
  useEffect(()=>{const busy=accounts.some(a=>a.importing||a.state==="connecting");if(!busy)return;const timer=setInterval(()=>{void loadOverview();},5000);return()=>clearInterval(timer);},[accounts.map(a=>a.importing+a.state).join()]);
  useEffect(()=>{
    const c=live.connection;if(!c)return;let disposed=false,revision=-1;
    let timer:ReturnType<typeof setTimeout>|undefined;
    // History arrives in many small batches; one reload covers a burst of changes.
    const bump=()=>{clearTimeout(timer);timer=setTimeout(()=>{if(!disposed)setEpoch(n=>n+1);},250);};
    const changed=()=>{if(disposed)return;let next=0;for(const row of c.db.myChatChanges.iter())next=row.revision;if(revision>=0&&next!==revision)bump();revision=next;};
    const gone=()=>{if(!disposed)onSignedOut();};
    c.db.myChatChanges.onInsert(changed);c.db.myChatChanges.onUpdate(changed);c.db.me.onDelete(gone);
    // Dar contacts name chats, so a contact change reloads them too.
    const contactsChanged=()=>{if(!disposed&&revision>=0)bump();};c.db.myPimChanges.onUpdate(contactsChanged);
    const stop=subscribe(c,["SELECT * FROM me","SELECT * FROM my_chat_changes","SELECT * FROM my_pim_changes"],()=>{if(!disposed){if(revision>=0)setEpoch(n=>n+1);changed();}},()=>{});
    return()=>{disposed=true;clearTimeout(timer);c.db.myPimChanges.removeOnUpdate(contactsChanged);c.db.myChatChanges.removeOnInsert(changed);c.db.myChatChanges.removeOnUpdate(changed);c.db.me.removeOnDelete(gone);stop();};
  },[live.connection]);

  function chooseAccount(id:string){setAccountId(id);setSelected("");setView("chats");}
  function openThread(thread:ChatThread,from?:HTMLElement){origin.current=from??null;setSelected(thread.id);setNavigation(false);}
  /** Mute, pin or archive on WhatsApp too; the list and info refresh from the change. */
  async function setChat(threadId:string,change:{pinned?:boolean;archived?:boolean;mutedUntil?:number}){
    try{await request(`${base}/chatSetting`,{method:"POST",body:JSON.stringify({threadId,...change})},session.csrfToken);
      setNotice(change.archived===true?"Chat archived":change.archived===false?"Chat unarchived":change.pinned===true?"Chat pinned":change.pinned===false?"Chat unpinned":change.mutedUntil?"Chat muted":"Chat unmuted");}
    catch(cause){fail(cause);}
  }
  function closeThread(){setSelected("");requestAnimationFrame(()=>{const target=origin.current;if(target?.isConnected)target.focus();});}
  const all=page?[...page.pinned,...page.threads]:[],channel=servers?.flatMap(s=>s.channels).find(t=>t.id===selected),current=channel??all.find(t=>t.id===selected);

  const accountActions=(a:ChatAccount):MenuAction[]=>[
    {label:"Rename",icon:"pencil",run:()=>setPane({kind:"rename",account:a})},
    ...a.state==="loggedOut"||a.state==="failed"?[{label:"Link again",icon:"link" as IconName,run:()=>setPane({kind:"link",accountId:a.id,platform:a.platform})}]:[],
    ...a.state!=="loggedOut"?[{label:a.platform==="discord"?"Sign out…":"Unlink…",icon:"logout" as IconName,separator:true,run:()=>setConfirm({title:a.platform==="discord"?`Sign ${a.name} out?`:`Unlink ${a.name}?`,
      text:a.platform==="discord"?"Dar ends its Discord session and stops receiving messages. Messages stay in Dar until you delete the account.":"Dar stops receiving messages from this account and removes itself from your phone’s linked devices. Chat history stays in Dar until you delete it.",action:a.platform==="discord"?"Sign out":"Unlink",
      run:async()=>{const result=await request<{confirmed:boolean}>(`${base}/accounts/${a.id}/unlink`,{method:"POST",body:"{}"},session.csrfToken);
        setNotice(result.confirmed?`${a.name} ${a.platform==="discord"?"signed out":"unlinked"}`:a.platform==="discord"?"Stopped here. Remove the session under Devices in Discord’s settings too.":"Unlinked here. Remove Dar under Linked devices on your phone too.");await loadOverview();}})}]:[],
    {label:"Delete from Dar…",icon:"trash",danger:true,separator:a.state==="loggedOut",run:()=>setConfirm({title:`Delete ${a.name} from Dar?`,
      text:a.platform==="discord"?"This signs Dar out and deletes the account’s servers, chats and messages from Dar. Discord keeps them.":"This unlinks the account and deletes its chats and messages from Dar. Your phone keeps them.",action:"Delete",
      run:async()=>{await request(`${base}/accounts/${a.id}`,{method:"DELETE"},session.csrfToken);setSelected("");setAccountId("");await loadOverview();}})},
  ];

  const status=account&&<AccountStatus account={account} onLink={()=>setPane({kind:"link",accountId:account.id,platform:account.platform})} actions={accountActions(account)}/>;
  const toggleServer=(id:string)=>setCollapsed(old=>{const next=new Set(old);if(next.has(id))next.delete(id);else next.add(id);return next;});
  const sidebar=<>
    <NewItemButton disabled={!account||account.state==="loggedOut"||account.state==="linking"} onClick={()=>setPane({kind:"newChat"})}>{discord?"New message":"New chat"}</NewItemButton>
    <div className="chat-accounts" role="tablist" aria-label="Accounts">
      {accounts.map(a=><button key={a.id} type="button" role="tab" aria-selected={a.id===account?.id} className={`chat-account-tab${a.id===account?.id?" is-current":""}`} title={a.name} onClick={()=>chooseAccount(a.id)}>
        <span className={`chat-platform is-${a.platform}`} aria-hidden="true"><Icon name={a.platform==="discord"?"hash":"chat"}/></span><span className="chat-account-name">{a.name}</span><span className="sr-only"> ({chatPlatformNames[a.platform]})</span>{a.unread>0&&<span className="chat-account-count">{a.unread}<span className="sr-only"> unread</span></span>}</button>)}
      <button type="button" className="icon-button chat-account-add" aria-label="Link an account" title="Link an account" onClick={()=>setPane({kind:"link"})}><Icon name="plus"/></button>
    </div>
    {account&&discord&&<nav aria-label="Direct messages"><button type="button" className={`nav-item chat-view${!channel?" active":""}`} aria-current={!channel?"page":undefined} onClick={()=>{setView("chats");if(channel)setSelected("");setNavigation(false);}}>
      <Icon name="contacts"/><span className="chat-row-title">Direct messages</span>{account.counts.unread>0&&<span className="chat-count">{account.counts.unread}</span>}</button></nav>}
    {account&&discord&&<ServerTree servers={servers} selected={selected} collapsed={collapsed} onToggle={toggleServer} onOpen={(thread,el)=>openThread(thread,el)}/>}
    {account&&!discord&&<nav aria-label="Chat views">{views.map(v=><button key={v.id} type="button" className={`nav-item chat-view${v.id===view?" active":""}`} aria-current={v.id===view?"page":undefined} onClick={()=>{setView(v.id);setNavigation(false);}}>
      <Icon name={v.icon}/><span className="chat-row-title">{v.label}</span>{account.counts[v.id]>0&&<span className="chat-count">{account.counts[v.id]}</span>}</button>)}</nav>}
    {status}
    <button type="button" className="nav-item ui-app-settings-entry" onClick={()=>setPane({kind:"settings"})}><Icon name="settings"/>Chat settings</button>
  </>;
  const title=discord?"Direct messages":views.find(v=>v.id===view)!.label;
  return <div className={`app-shell pim-shell chat-shell${selected?" has-pim-details":""}${channel?" is-channel":""}${selected&&info?" has-chat-info":""}`}>
    <WorkspaceHeader app="chat" session={session} onSignedOut={onSignedOut} onNavigation={()=>setNavigation(!navigation)} navigationOpen={navigation} navigationId={navigationId} onSettings={()=>setPane({kind:"settings"})}/>
    <div className="pim-workspace chat-workspace">{navigation&&<button className="nav-scrim" aria-label="Close navigation" onClick={()=>setNavigation(false)}/>}
      <aside id={navigationId} ref={navigationRef} className={`sidebar chat-sidebar${navigation?" is-open":""}`} aria-label="Chat navigation">{sidebar}</aside>
      <main className="pim-main chat-main" tabIndex={-1} aria-label={title}>
        <div className="chat-list-heading"><h1>{title}</h1>{account&&<span className="chat-heading-count">{account.counts[view]}</span>}</div>
        {loadError&&<div className="chat-banner is-error" role="alert"><Icon name="alert"/><span><b>Chat could not be loaded</b>{loadError}</span><button className="secondary-button" onClick={()=>{void loadOverview();void loadThreads();}}>Retry</button></div>}
        {overview&&!overview.bridge.available&&overview.bridge.detail&&<div className="chat-banner" role="status"><Icon name="alert"/><span><b>Chat is unavailable</b>{overview.bridge.detail}</span></div>}
        <div className="chat-list" role="list" aria-label={`${title}`}>
          {!overview&&!loadError&&<p className="chat-empty" role="status">Loading chats…</p>}
          {overview&&!accounts.length&&<div className="chat-empty"><span className="chat-empty-icon"><Icon name="chat"/></span><h2>No accounts linked</h2><p>Link WhatsApp or Discord to bring their chats here. Each account keeps its own tab.</p>
            <button className="primary-button" disabled={!overview.bridge.available} onClick={()=>setPane({kind:"link"})}>Link an account</button></div>}
          {account&&listState==="loading"&&!page&&<p className="chat-empty" role="status">Loading chats…</p>}
          {account&&listState==="error"&&<div className="chat-empty" role="alert"><h2>Chats could not be loaded</h2><p>{listError}</p><button className="secondary-button" onClick={()=>{void loadThreads();}}>Retry</button></div>}
          {account&&listState==="ready"&&page&&!all.length&&<div className="chat-empty"><h2>{account.importing?"Importing your chats…":account.state==="linking"?`Waiting for ${chatPlatformNames[account.platform]}`:discord?"No direct messages yet":view==="unread"?"All caught up":view==="archived"?"No archived chats":view==="groups"?"No groups":"No chats yet"}</h2>
            <p>{account.importing?"They appear here as your phone sends them.":account.state==="linking"?"Finish linking to see your chats.":discord?"Your servers are in the sidebar.":view==="unread"?"There are no unread chats.":""}</p></div>}
          {all.map(thread=><ThreadRow key={thread.id} thread={thread} selected={thread.id===selected} onOpen={el=>openThread(thread,el)}/>)}
          {page?.cursor&&<div className="chat-more"><button className="text-button" disabled={more} onClick={()=>{void loadMore();}}>{more?"Loading…":"Load more"}</button></div>}
        </div>
      </main>
      {selected&&account&&<aside className="pim-details chat-details" aria-label={current?.name??"Conversation"}>
        <Conversation key={selected} threadId={selected} focusMessage={selected===start.chat?start.message:""} account={account} session={session} epoch={epoch} info={info} onClose={closeThread} onNotice={setNotice}
          onSave={message=>setPane({kind:"save",threadId:selected,message})} onInfo={()=>setInfo(!info)} onSetting={change=>setChat(selected,change)}/>
      </aside>}
      {selected&&account&&info&&<aside className="chat-info-sheet" aria-label="Chat info"><ChatInfoSheet threadId={selected} session={session} epoch={epoch} onClose={()=>setInfo(false)} onSetting={change=>setChat(selected,change)} onAddContact={thread=>setPane({kind:"addContact",thread})}/></aside>}
      {!selected&&account&&<div className="chat-placeholder" aria-hidden="true"><span className="chat-empty-icon"><Icon name="chat"/></span><p>Choose a chat</p></div>}
    </div>
    {pane?.kind==="link"&&<LinkPane session={session} accountId={pane.accountId} platform={pane.platform} onClose={()=>{setPane(undefined);void loadOverview();}} onLinked={id=>{setPane(undefined);chooseAccount(id);void loadOverview();setNotice("Linked. Your chats are on their way.");}}/>}
    {pane?.kind==="newChat"&&account&&discord&&<NewMessagePane session={session} account={account} onClose={()=>setPane(undefined)} onOpened={thread=>{setPane(undefined);setView("chats");void loadThreads(true);setSelected(thread.id);}}/>}
    {pane?.kind==="newChat"&&account&&!discord&&<NewChatPane session={session} account={account} onClose={()=>setPane(undefined)} onOpened={thread=>{setPane(undefined);setView("chats");void loadThreads(true);setSelected(thread.id);}}/>}
    {pane?.kind==="save"&&<SaveMediaPane session={session} threadId={pane.threadId} message={pane.message} platform={account?.platform??"whatsapp"} onClose={()=>setPane(undefined)} onSaved={name=>{setPane(undefined);setNotice(`Saved to Files as ${name}`);}}/>}
    {pane?.kind==="addContact"&&<AddContactPane session={session} thread={pane.thread} onClose={()=>setPane(undefined)} onSaved={name=>{setPane(undefined);setNotice(`${name} added to Contacts`);setEpoch(n=>n+1);}}/>}
    {pane?.kind==="rename"&&<RenamePane session={session} account={pane.account} onClose={()=>setPane(undefined)} onSaved={()=>{setPane(undefined);void loadOverview();}}/>}
    {pane?.kind==="settings"&&<ChatSettings accounts={accounts} onClose={()=>setPane(undefined)} onLink={()=>setPane({kind:"link"})} actions={accountActions}/>}
    {confirm&&<ConfirmAction {...confirm} onClose={()=>setConfirm(undefined)}/>}
    <div className="chat-toast-region" role="status" aria-live="polite">{notice&&<div className="settings-toast chat-toast"><span>{notice}</span><button type="button" className="text-button" onClick={()=>setNotice("")}>Dismiss</button></div>}</div>
  </div>;
}

function AccountStatus({account,onLink,actions}:{account:ChatAccount;onLink:()=>void;actions:MenuAction[]}){
  const phone=phoneOf(account.jid),detail=stateText[account.detail]??"";
  if(account.platform==="discord"){
    const [tone,icon,heading,text]:[string,IconName,string,ReactNode]=
      account.state==="connected"?["","hash","Signed in to Discord","Your own account. Dar sees what you can see."]
      :account.state==="connecting"?["","refresh","Connecting…","Loading your servers and messages."]
      :account.state==="disconnected"?["is-warning","alert","Reconnecting…","Dar keeps trying. New messages arrive once Discord answers."]
      :account.state==="linking"?["","link","Not signed in yet",<button type="button" className="text-button" onClick={onLink}>Link Discord</button>]
      :account.state==="loggedOut"?["is-warning","alert","Signed out",<>{detail||"Discord signed Dar out. Messages stay here, read-only."} <button type="button" className="text-button" onClick={onLink}>Link again</button></>]
      :["is-error","alert","Discord stopped",<>{detail||"Something went wrong with this account."} <button type="button" className="text-button" onClick={onLink}>Link again</button></>];
    return <div className={`chat-status ${tone}`}><Icon name={icon}/><div className="chat-status-text"><b>{heading}</b><span>{text}</span></div>
      <ActionMenu label={`Actions for ${account.name}`} actions={actions}><Icon name="more"/></ActionMenu></div>;
  }
  const [tone,icon,heading,text]:[string,IconName,string,ReactNode]=
    account.state==="connected"?["",account.importing?"refresh":"phone",phone?`Linked to ${phone}`:"Linked",account.importing?`Importing history${account.historyProgress?` · ${account.historyProgress}%`:"…"}`:"Messages also stay on your phone."]
    :account.state==="connecting"?["",  "refresh","Connecting…",phone]
    :account.state==="disconnected"?["is-warning","alert","Reconnecting…","Dar keeps trying. New messages arrive once WhatsApp answers."]
    :account.state==="linking"?["","link","Not linked yet",<button type="button" className="text-button" onClick={onLink}>Link WhatsApp</button>]
    :account.state==="loggedOut"?["is-warning","alert","Unlinked",<>{detail||"WhatsApp unlinked Dar. Chats stay here, read-only."} <button type="button" className="text-button" onClick={onLink}>Link again</button></>]
    :["is-error","alert","WhatsApp stopped",<>{detail||"Something went wrong with this link."} <button type="button" className="text-button" onClick={onLink}>Link again</button></>];
  return <div className={`chat-status ${tone}`}><Icon name={icon}/><div className="chat-status-text"><b>{heading}</b><span>{text}</span></div>
    <ActionMenu label={`Actions for ${account.name}`} actions={actions}><Icon name="more"/></ActionMenu></div>;
}

function ThreadRow({thread,selected,onOpen}:{thread:ChatThread;selected:boolean;onOpen:(el:HTMLElement)=>void}){
  const name=thread.name||phoneOf(thread.jid)||"Unknown",text=preview(thread),quiet=muted(thread);
  return <div role="listitem" className={`chat-row${selected?" is-selected":""}${thread.unread?" is-unread":""}`}>
    <button type="button" aria-current={selected?"true":undefined} onClick={event=>onOpen(event.currentTarget)}>
      <Avatar name={name} group={thread.kind==="group"}/>
      <span className="chat-row-name">{name}</span>
      <time className="chat-row-time" dateTime={thread.at?new Date(thread.at).toISOString():undefined}>{listTime(thread.at)}</time>
      <span className="chat-row-preview">{thread.previewFromMe&&<Ticks status={thread.previewStatus}/>}{thread.previewSender&&<b>{thread.previewSender}: </b>}
        {thread.previewKind&&kindIcon[thread.previewKind]&&<Icon name={kindIcon[thread.previewKind]!}/>}<span className="chat-row-text">{text}</span></span>
      <span className="chat-row-marks">{quiet&&<span title="Muted"><Icon name="bell"/><span className="sr-only">Muted</span></span>}{thread.pinned&&<span title="Pinned"><Icon name="pin"/><span className="sr-only">Pinned</span></span>}
        {thread.unread>0&&<span className={`chat-unread${quiet?" is-muted":""}`}>{thread.unread}<span className="sr-only"> unread</span></span>}</span>
    </button>
  </div>;
}

function Conversation({threadId,focusMessage,account,session,epoch,info,onClose,onNotice,onSave,onInfo,onSetting}:{threadId:string;focusMessage:string;account:ChatAccount;session:BrowserSession;epoch:number;info:boolean;onClose:()=>void;
  onNotice:(text:string)=>void;onSave:(message:ChatMessage)=>void;onInfo:()=>void;onSetting:(change:ChatSetting)=>Promise<void>}){
  const [page,setPage]=useState<ChatMessagePage>(),[error,setError]=useState(""),[older,setOlder]=useState<{messages:ChatMessage[];before:string|null}>({messages:[],before:null}),[loadingOlder,setLoadingOlder]=useState(false);
  const [text,setText]=useState(""),[reply,setReply]=useState<ChatMessage>(),[editing,setEditing]=useState<ChatMessage>(),[sending,setSending]=useState(false),[asked,setAsked]=useState(false),[confirm,setConfirm]=useState<Confirm>();
  const scroller=useRef<HTMLDivElement>(null),input=useRef<HTMLTextAreaElement>(null),stick=useRef(true),heading=useRef<HTMLHeadingElement>(null),first=useRef(true);
  const load=useCallback(async()=>{
    try{const result=await request<ChatMessagePage>(`${base}/messages?${new URLSearchParams({threadId})}`);setPage(result);setError("");if(first.current){first.current=false;setOlder({messages:[],before:result.before});}
      if(result.thread.unread>0)void request(`${base}/read`,{method:"POST",body:JSON.stringify({threadId})},session.csrfToken).catch(()=>{});}
    catch(cause){setError((cause as Error).message);}
  },[threadId]);
  // Earlier messages from the phone arrive as history; start the pages again so they show.
  useEffect(()=>{if(asked)first.current=true;void load();},[load,epoch]);
  useEffect(()=>{if(matchMedia("(max-width:800px)").matches)heading.current?.focus();else input.current?.focus();},[threadId]);
  async function loadOlder(){
    if(!older.before||loadingOlder)return;setLoadingOlder(true);const el=scroller.current,height=el?.scrollHeight??0;
    try{const result=await request<ChatMessagePage>(`${base}/messages?${new URLSearchParams({threadId,before:older.before})}`);setOlder(o=>({messages:[...result.messages,...o.messages],before:result.before}));
      requestAnimationFrame(()=>{if(el)el.scrollTop+=el.scrollHeight-height;});}
    catch(cause){onNotice((cause as Error).message);}finally{setLoadingOlder(false);}
  }
  const messages=useMemo(()=>{const seen=new Set<string>(),out:ChatMessage[]=[];for(const m of [...older.messages,...page?.messages??[]])if(!seen.has(m.id)){seen.add(m.id);out.push(m);}return out;},[older,page]);
  // A search result opens the chat at its message: earlier pages load until it is found, then it is shown and outlined.
  const [seek,setSeek]=useState(focusMessage),[found,setFound]=useState(""),tries=useRef(0);
  useEffect(()=>{
    if(!seek||!page)return;
    const el=scroller.current?.querySelector<HTMLElement>(`[data-message="${CSS.escape(seek)}"]`);
    if(el){stick.current=false;el.scrollIntoView({block:"center"});setFound(seek);setSeek("");const timer=setTimeout(()=>setFound(""),4000);return()=>clearTimeout(timer);}
    if(older.before&&!loadingOlder&&tries.current++<40)void loadOlder();else if(!older.before)setSeek("");
  },[seek,messages,older.before,loadingOlder]);
  useLayoutEffect(()=>{const el=scroller.current;if(el&&stick.current)el.scrollTop=el.scrollHeight;},[messages.length,page]);
  const thread=page?.thread,name=thread?.name||phoneOf(thread?.jid??"")||"Chat",group=thread?.kind==="group";
  const discord=account.platform==="discord",channel=!!thread?.serverId,service=chatPlatformNames[account.platform],shown=channel?`#${name}`:name;
  const canSend=account.state==="connected"||account.state==="connecting"||account.state==="disconnected";
  // Discord: fetch what was said while Dar was away; the change feed shows it.
  useEffect(()=>{if(discord&&account.state==="connected")void request(`${base}/catchUp`,{method:"POST",body:JSON.stringify({threadId})},session.csrfToken).catch(()=>{});},[threadId]);
  const [start,setStart]=useState(false);
  /** Discord: earlier messages come straight from Discord, then show above the ones loaded. */
  async function earlier(){
    if(loadingOlder)return;setLoadingOlder(true);const el=scroller.current,height=el?.scrollHeight??0;
    try{
      const result=await request<{count:number;more:boolean}>(`${base}/history`,{method:"POST",body:JSON.stringify({threadId})},session.csrfToken);
      if(!result.more)setStart(true);
      const oldest=messages[0];
      if(oldest){const next=await request<ChatMessagePage>(`${base}/messages?${new URLSearchParams({threadId,before:`${threadId}/${chatTimeKey(oldest.at)}/${oldest.messageId}`})}`);
        setOlder(o=>({messages:[...next.messages,...o.messages],before:next.before}));requestAnimationFrame(()=>{if(el)el.scrollTop+=el.scrollHeight-height;});}
      else await load();
    }catch(cause){onNotice((cause as Error).message);}finally{setLoadingOlder(false);}
  }
  const post=(path:string,body:object)=>request(`${base}/${path}`,{method:"POST",body:JSON.stringify({threadId,...body})},session.csrfToken);
  async function send(event?:FormEvent){
    event?.preventDefault();const body=text.trim();if(!body||sending||!canSend)return;
    if(editing){
      const target=editing;setSending(true);
      try{await post("edit",{messageId:target.messageId,text:body});setEditing(undefined);setText("");await load();}
      catch(cause){onNotice((cause as Error).message);}finally{setSending(false);input.current?.focus();}
      return;
    }
    // The composer clears at once; the text comes back if Dar could not store the message.
    const quoted=reply;setSending(true);stick.current=true;setText("");setReply(undefined);
    try{await post("send",{text:body,...quoted?{quotedId:quoted.messageId}:{}});await load();}
    catch(cause){setText(current=>current||body);setReply(current=>current??quoted);onNotice((cause as Error).message);}finally{setSending(false);input.current?.focus();}
  }
  function keys(event:KeyboardEvent<HTMLTextAreaElement>){
    if(event.key==="Enter"&&!event.shiftKey&&!event.nativeEvent.isComposing){event.preventDefault();void send();}
    if(event.key==="Escape"&&(reply||editing)){event.preventDefault();setReply(undefined);if(editing){setEditing(undefined);setText("");}}
  }
  async function act(path:string,m:ChatMessage,body:object={}){try{await post(path,{messageId:m.messageId,...body});await load();}catch(cause){onNotice((cause as Error).message);}}
  async function askPhone(){try{await post("history",{});setAsked(true);}catch(cause){onNotice((cause as Error).message);}}
  const quiet=thread?muted(thread):false;
  const chatActions:MenuAction[]=thread?[
    {label:quiet?"Unmute":"Mute",icon:"bell",...quiet?{run:()=>{void onSetting({mutedUntil:0});}}:{items:chatMuteChoices.map(([label,ms])=>({label,run:()=>{void onSetting({mutedUntil:ms<0?-1:Date.now()+ms});}}))}},
    ...discord?[]:[{label:thread.pinned?"Unpin":"Pin",icon:"pin" as IconName,disabled:thread.archived,run:()=>{void onSetting({pinned:!thread.pinned});}},
      {label:thread.archived?"Unarchive":"Archive",icon:"archive" as IconName,run:()=>{void onSetting({archived:!thread.archived});}}],
  ]:[];
  let lastDay="";
  return <section className={`chat-conversation${discord?" is-flat":""}`}>
    <header className="chat-conversation-head">
      <button type="button" className="icon-button chat-back" aria-label="Back to chats" onClick={onClose}><Icon name="back"/></button>
      {channel?<span className="chat-channel-mark" aria-hidden="true"><Icon name={thread?.kind==="voice"?"speaker":"hash"}/></span>:<Avatar name={name} group={!!group} size="is-small"/>}
      <div className="chat-who"><h2 ref={heading} tabIndex={-1}>{shown}</h2><small>{channel?thread?.topic||"Channel":discord?group?"Group":"Direct message":group?"Group":phoneOf(thread?.jid??"")}{quiet?" · Muted":""}</small></div>
      <button type="button" className="icon-button" aria-label={channel?"Channel info":"Chat info"} aria-pressed={info} title={channel?"Channel info":"Chat info"} onClick={onInfo}><Icon name="info"/></button>
      {canSend&&chatActions.length>0&&<ActionMenu label={`Actions for ${shown}`} actions={chatActions}><Icon name="more"/></ActionMenu>}
      <button type="button" className="icon-button chat-close" aria-label="Close chat" onClick={onClose}><Icon name="close"/></button>
    </header>
    {account.state==="disconnected"&&<div className="chat-banner slim" role="status"><Icon name="alert"/><span><b>Reconnecting…</b>Messages you send wait until {service} answers.</span></div>}
    {(account.state==="loggedOut"||account.state==="failed")&&<div className="chat-banner slim is-error" role="status"><Icon name="alert"/><span><b>{account.state==="loggedOut"?discord?"Discord signed Dar out":"WhatsApp unlinked Dar":`${service} stopped`}</b>Your history stays here, read-only. Link again to send and receive.</span></div>}
    <div className="chat-messages" ref={scroller} onScroll={event=>{const el=event.currentTarget;stick.current=el.scrollHeight-el.scrollTop-el.clientHeight<80;if(el.scrollTop<200)void loadOlder();}} role="log" aria-label={`Messages in ${shown}`} aria-live="polite">
      {older.before&&<div className="chat-more"><button className="text-button" disabled={loadingOlder} onClick={()=>{void loadOlder();}}>{loadingOlder?"Loading…":"Load earlier messages"}</button></div>}
      {!discord&&page&&!older.before&&messages.length>0&&canSend&&<div className="chat-more">{asked?<span className="chat-hint">Asked your phone; earlier messages appear here as they arrive.</span>
        :<button className="text-button" onClick={()=>{void askPhone();}}>Get earlier messages from your phone</button>}</div>}
      {discord&&page&&!older.before&&messages.length>0&&<div className="chat-more">{start?<span className="chat-hint">This is the start of {shown}.</span>
        :canSend&&account.state==="connected"?<button className="text-button" disabled={loadingOlder} onClick={()=>{void earlier();}}>{loadingOlder?"Loading…":"Load earlier messages from Discord"}</button>:null}</div>}
      {error&&<div className="chat-empty" role="alert"><h2>This chat could not be opened</h2><p>{error}</p><button className="secondary-button" onClick={()=>{void load();}}>Retry</button></div>}
      {!page&&!error&&<p className="chat-empty" role="status">Opening chat…</p>}
      {page&&!messages.length&&<div className="chat-empty"><h2>No messages yet</h2><p>{canSend?`Say hello. Messages go out from ${account.name}.`:"Messages from this chat appear here."}</p></div>}
      {messages.map((m,i)=>{
        const day=dayLabel(m.at),next=messages[i+1],tail=!next||next.fromMe!==m.fromMe||next.sender!==m.sender||dayLabel(next.at)!==day,prev=messages[i-1];
        // Discord rows name the sender at the start of each run, as Discord does (a new run after seven minutes).
        const showName=discord?!prev||prev.sender!==m.sender||prev.fromMe!==m.fromMe||dayLabel(prev.at)!==day||m.at-prev.at>7*60_000||!!m.quotedId
          :group&&!m.fromMe&&(!prev||prev.sender!==m.sender||prev.fromMe||dayLabel(prev.at)!==day);
        const heading=day!==lastDay?(lastDay=day,<div className="chat-day" key={`day-${m.id}`}><span>{day}</span></div>):null;
        return <Fragment key={m.id}>{heading}<Bubble threadId={threadId} message={m} found={m.messageId===found} tail={tail} showName={showName} canChange={canSend} platform={account.platform} onSave={()=>onSave(m)}
          onReply={()=>{setEditing(undefined);setReply(m);input.current?.focus();}} onCopy={()=>{void navigator.clipboard?.writeText(m.text);onNotice("Copied");}}
          onReact={(emoji,remove)=>{void act("react",m,{emoji,...remove?{remove:true}:{}});}} onEdit={()=>{setReply(undefined);setEditing(m);setText(m.text);input.current?.focus();}}
          onRevoke={()=>setConfirm({title:"Delete for everyone?",text:"This message is removed for everyone in the chat. People may already have seen it.",action:"Delete for everyone",run:()=>act("revoke",m)})}
          onDeleteForMe={()=>setConfirm({title:"Delete from Dar?",text:discord?"This removes the message from Dar only. Discord keeps it.":"This removes the message from Dar only. WhatsApp and your phone keep it.",action:"Delete",run:()=>act("deleteForMe",m)})}
          onRetry={()=>{void act("retry",m);}} onDiscard={()=>{void act("discard",m);}}/></Fragment>;
      })}
    </div>
    <form className="chat-composer" onSubmit={send}>
      {reply&&<div className="chat-reply-bar"><div><b>Replying to {reply.fromMe?"yourself":reply.senderName||name}</b><span>{reply.text||chatKindLabels[reply.kind as keyof typeof chatKindLabels]||""}</span></div>
        <button type="button" className="icon-button" aria-label="Cancel reply" onClick={()=>setReply(undefined)}><Icon name="close"/></button></div>}
      {editing&&<div className="chat-reply-bar"><div><b>Editing message</b><span>{editing.text}</span></div>
        <button type="button" className="icon-button" aria-label="Cancel editing" onClick={()=>{setEditing(undefined);setText("");}}><Icon name="close"/></button></div>}
      <div className="chat-compose-box">
        <textarea ref={input} rows={1} value={text} maxLength={discord?chatLimits.discordText:chatLimits.text} disabled={!canSend} aria-label={`Message ${shown}`} placeholder={canSend?discord?`Message ${channel?shown:"@"+name}`:"Message":"Link again to send messages"} onChange={event=>setText(event.target.value)} onKeyDown={keys}/>
        <button type="submit" className="chat-send" aria-label={editing?"Save edit":"Send"} disabled={!canSend||!text.trim()||sending}><Icon name={editing?"check":"send"}/></button>
      </div>
    </form>
    {confirm&&<ConfirmAction {...confirm} onClose={()=>setConfirm(undefined)}/>}
  </section>;
}

/** WhatsApp lets senders edit a message for a short while after sending it. */
const editWindow=15*60_000;
function Bubble({threadId,message:m,found,tail,showName,canChange,platform,onSave,onReply,onCopy,onReact,onEdit,onRevoke,onDeleteForMe,onRetry,onDiscard}:{threadId:string;message:ChatMessage;found:boolean;tail:boolean;showName:boolean;canChange:boolean;platform:ChatPlatform;
  onSave:()=>void;onReply:()=>void;onCopy:()=>void;onReact:(emoji:string,remove?:boolean)=>void;onEdit:()=>void;onRevoke:()=>void;onDeleteForMe:()=>void;onRetry:()=>void;onDiscard:()=>void}){
  const label=m.kind!=="text"?chatKindLabels[m.kind]:"",icon=kindIcon[m.kind],discord=platform==="discord";
  const file=m.media&&m.media.state!=="expired"?mediaUrl(threadId,m.messageId):"",mine=m.reactions.find(r=>r.mine)?.emoji??"",live=m.status!=="pending"&&m.status!=="failed";
  // WhatsApp keeps one reaction per person; Discord allows several, each toggled on its own.
  const mineOf=(emoji:string)=>m.reactions.some(r=>r.emoji===emoji&&r.mine);
  const reactItems:MenuAction[]=discord?chatQuickReactions.map(emoji=>({label:emoji,current:mineOf(emoji),run:()=>onReact(emoji,mineOf(emoji))}))
    :[...chatQuickReactions.map(emoji=>({label:emoji,current:emoji===mine,run:()=>onReact(emoji===mine?"":emoji)})),...mine?[{label:"Remove reaction",icon:"close" as IconName,separator:true,run:()=>onReact("")}]:[]];
  const actions:MenuAction[]=m.deleted?[{label:"Delete from Dar…",icon:"trash",danger:true,run:onDeleteForMe}]:[
    ...canChange&&live?[{label:"React",icon:"star" as IconName,items:reactItems}]:[],
    ...canChange&&live?[{label:"Reply",icon:"reply" as IconName,run:onReply}]:[],
    ...m.text?[{label:"Copy text",icon:"copy" as IconName,run:onCopy}]:[],
    ...canChange&&live&&m.fromMe&&m.kind==="text"&&(discord||Date.now()-m.at<editWindow)?[{label:"Edit",icon:"pencil" as IconName,run:onEdit}]:[],
    ...file?[{label:"Download",icon:"download" as IconName,href:`${file}&download=1`,download:true,separator:true},{label:"Save to Files…",icon:"folder" as IconName,run:onSave}]:[],
    ...canChange&&live&&m.fromMe?[{label:"Delete for everyone…",icon:"trash" as IconName,danger:true,separator:true,run:onRevoke}]:[],
    {label:"Delete from Dar…",icon:"trash",danger:true,separator:!(canChange&&live&&m.fromMe),run:onDeleteForMe},
  ];
  const photo=!m.deleted&&!!m.media&&(m.kind==="image"||m.kind==="sticker")&&m.media.state!=="expired"&&m.media.state!=="retrying";
  const who=m.fromMe&&discord?"You":m.senderName||phoneOf(m.sender)||"Unknown";
  return <div data-message={m.messageId} className={`chat-message ${m.fromMe?"is-out":"is-in"}${tail?" is-tail":""}${showName?" is-first":""}${m.status==="failed"?" is-failed":""}${found?" is-found":""}`}>
    {discord&&showName&&<span className="chat-message-avatar"><Avatar name={who} group={false} size="is-small"/></span>}
    <div className="chat-line"><div className={`chat-bubble${photo?" has-photo":""}`}>
      {showName&&<span className={`chat-sender ${avatarHue(m.sender)}`}>{who}{discord&&<time className="chat-sender-time" dateTime={new Date(m.at).toISOString()}>{localTime(m.at,{hour:"numeric",minute:"2-digit"})}</time>}</span>}
      {m.quotedId&&<span className="chat-quote"><b>{m.quotedSender||"Message"}</b><span>{m.quotedText||"Message"}</span></span>}
      {m.deleted?<span className="chat-text is-deleted"><Icon name="close"/>This message was deleted</span>:<>
        {m.media?<MediaView threadId={threadId} message={m} platform={platform}/>:label&&<span className="chat-media"><Icon name={icon??"file"}/>{m.kind==="document"&&m.fileName?m.fileName:label}</span>}
        {m.text&&<span className="chat-text">{m.text}</span>}</>}
      {discord&&m.edited&&!m.deleted&&<small className="chat-edited">(edited)</small>}
      <span className="chat-meta">{m.edited&&!m.deleted&&!discord&&"edited · "}<time dateTime={new Date(m.at).toISOString()} title={localDateTime(m.at,{dateStyle:"medium",timeStyle:"short"})}>{localTime(m.at,{hour:"numeric",minute:"2-digit"})}</time>{m.fromMe&&<Ticks status={m.status}/>}</span>
    </div>
    <ActionMenu label="Message actions" className="chat-message-menu" actions={actions}><Icon name="more"/></ActionMenu></div>
    {m.reactions.length>0&&<div className="chat-reactions">{m.reactions.map(r=>{
      const who=discord?r.mine?r.count>1?`You and ${r.count-1} other${r.count>2?"s":""}`:"You":"":`${r.mine?["You",...r.names].join(", "):r.names.join(", ")}${r.count>r.names.length+(r.mine?1:0)?" and others":""}`;
      const shown=emojiLabel(r.emoji);
      return canChange&&live?<button key={r.emoji} type="button" className={r.mine?"is-mine":""} aria-pressed={r.mine} title={who} aria-label={`${shown} ${r.count}${who?`, ${who}`:""}. ${r.mine?"Remove your reaction":"React with "+shown}`} onClick={()=>discord?onReact(r.emoji,r.mine):onReact(r.mine?"":r.emoji)}>{shown}{r.count>1&&<small>{r.count}</small>}</button>
        :<span key={r.emoji} className={r.mine?"is-mine":""} title={who}>{shown}{r.count>1&&<small>{r.count}</small>}</span>;})}</div>}
    {m.status==="failed"&&<span className="chat-failed"><Icon name="alert"/>Not sent · <button type="button" className="text-button" onClick={onRetry}>Retry</button> · <button type="button" className="text-button" onClick={onDiscard}>Delete</button></span>}
  </div>;
}

type ChatSetting={pinned?:boolean;archived?:boolean;mutedUntil?:number};
/** Chat info (mock A2): who it is, mute/pin/archive, attachments and, for groups, members. */
function ChatInfoSheet({threadId,session,epoch,onClose,onSetting,onAddContact}:{threadId:string;session:BrowserSession;epoch:number;onClose:()=>void;onSetting:(change:ChatSetting)=>Promise<void>;onAddContact:(thread:ChatThread)=>void}){
  const [info,setInfo]=useState<ChatInfo>(),[error,setError]=useState("");
  useEffect(()=>{let gone=false;void request<ChatInfo>(`${base}/info?${new URLSearchParams({threadId})}`).then(value=>{if(!gone){setInfo(value);setError("");}},cause=>{if(!gone)setError((cause as Error).message);});return()=>{gone=true;};},[threadId,epoch]);
  const thread=info?.thread,name=thread?.name||phoneOf(thread?.jid??"")||"Chat",quiet=thread?muted(thread):false,group=thread?.kind==="group";
  const pictures=info?.media.filter(m=>(m.kind==="image"||m.kind==="sticker")&&m.state==="stored").slice(0,9)??[],files=info?.media.filter(m=>m.kind==="document").slice(0,6)??[];
  const linked=info?.account.state==="connected"||info?.account.state==="connecting"||info?.account.state==="disconnected";
  const discord=info?.account.platform==="discord",channel=!!thread?.serverId;
  return <section className="chat-info" aria-label="Chat info">
    <header className="chat-conversation-head"><button type="button" className="icon-button chat-back" aria-label="Back to chat" onClick={onClose}><Icon name="back"/></button>
      <h2 className="chat-info-title">{channel?"Channel info":group?"Group info":discord?"Chat info":"Contact info"}</h2><button type="button" className="icon-button chat-close" aria-label="Close chat info" onClick={onClose}><Icon name="close"/></button></header>
    <div className="chat-info-body">
      {error&&<p className="error" role="alert">{error}</p>}
      {!info&&!error&&<p className="chat-empty" role="status">Loading…</p>}
      {info&&thread&&<>
        <div className="chat-info-hero">{channel?<span className="chat-channel-mark is-large" aria-hidden="true"><Icon name={thread.kind==="voice"?"speaker":"hash"}/></span>:<Avatar name={name} group={group} size="is-large"/>}
          <h3>{channel?`#${name}`:name}</h3><p>{channel?`${info.server?.name??"Server"}${thread.section?` · ${thread.section}`:""}`:group?`Group${info.group?` · ${info.group.members.length} members`:""}`:discord?"Direct message":phoneOf(thread.jid)} · {chatPlatformNames[info.account.platform]}</p></div>
        {linked&&<div className="chat-info-actions">
          {quiet?<button type="button" onClick={()=>{void onSetting({mutedUntil:0});}}><span><Icon name="bell"/></span>Unmute</button>
            :<ActionMenu label="Mute" className="chat-info-mute" actions={chatMuteChoices.map(([label,ms])=>({label,run:()=>{void onSetting({mutedUntil:ms<0?-1:Date.now()+ms});}}))}><span><Icon name="bell"/></span>Mute</ActionMenu>}
          {!discord&&<><button type="button" disabled={thread.archived} onClick={()=>{void onSetting({pinned:!thread.pinned});}}><span><Icon name="pin"/></span>{thread.pinned?"Unpin":"Pin"}</button>
          <button type="button" onClick={()=>{void onSetting({archived:!thread.archived});}}><span><Icon name="archive"/></span>{thread.archived?"Unarchive":"Archive"}</button></>}
        </div>}
        {(info.group?.topic||thread.topic)&&<p className="chat-info-topic">{info.group?.topic||thread.topic}</p>}
        {!group&&!discord&&<div className="chat-contact">{thread.contact?<><Avatar name={thread.contact.name} group={false} size="is-small"/><div><b>{thread.contact.name}</b><small>Dar contact{thread.whatsappName&&thread.whatsappName!==thread.contact.name&&thread.whatsappName!==phoneOf(thread.jid)?` · on WhatsApp “${thread.whatsappName}”`:""}</small></div>
          <a className="text-button" href={`/contacts?${new URLSearchParams({book:thread.contact.book,contact:thread.contact.href})}`}>Open</a></>
          :<><span className="chat-empty-icon is-small"><Icon name="contacts"/></span><div><b>Not in your contacts</b><small>Save this number in Dar Contacts.</small></div><button type="button" className="text-button" onClick={()=>onAddContact(thread)}>Add</button></>}</div>}
        <dl className="chat-info-card">
          <div><dt><Icon name="chat"/>Account</dt><dd>{info.account.name}</dd></div>
          <div><dt><Icon name="clock"/>Messages in Dar</dt><dd>{info.messages.toLocaleString()}{info.complete?"":"+"}</dd></div>
          {quiet&&<div><dt><Icon name="bell"/>Muted</dt><dd>{thread.mutedUntil===-1?"Always":`Until ${localDateTime(thread.mutedUntil,{dateStyle:"medium",timeStyle:"short"})}`}</dd></div>}
        </dl>
        {pictures.length>0&&<><h4 className="chat-info-heading">Photos</h4><div className="chat-info-grid">{pictures.map(m=><a key={m.messageId} href={mediaUrl(threadId,m.messageId)} target="_blank" rel="noopener"><img src={mediaUrl(threadId,m.messageId)} alt="" loading="lazy"/></a>)}</div></>}
        {files.length>0&&<><h4 className="chat-info-heading">Documents</h4><div className="chat-info-files">{files.map(m=>{const file=localName({kind:m.kind,mime:m.mime,fileName:m.fileName,at:m.at,platform:info.account.platform});
          return <a key={m.messageId} className="chat-file" href={`${mediaUrl(threadId,m.messageId)}&download=1`} download={file}><FileTile entry={{kind:"file",name:file,mediaType:m.mime}}/><span className="chat-file-name"><b>{file}</b><small>{formatBytes(m.size)}</small></span></a>;})}</div></>}
        {group&&!discord&&<><h4 className="chat-info-heading">Members{info.group?` · ${info.group.members.length}`:""}</h4>
          {info.groupError&&<p className="chat-hint">{info.groupError}</p>}
          {info.group&&<ul className="chat-info-members">{info.group.members.map(member=><li key={member.jid}><Avatar name={member.name||member.jid} group={false} size="is-tiny"/><span>{member.name||phoneOf(member.jid)||"Unknown"}</span>{member.admin&&<small>Admin</small>}</li>)}</ul>}</>}
      </>}
    </div>
  </section>;
}

const mediaUrl=(threadId:string,messageId:string)=>`${base}/media?${new URLSearchParams({threadId,messageId})}`;
const clock=(seconds:number)=>`${Math.floor(seconds/60)}:${String(seconds%60).padStart(2,"0")}`;
/** Display size within a box, keeping the photo's shape; sent as width/height attributes so the space is kept while it loads. */
function fit(width:number,height:number,maxWidth:number,maxHeight:number){
  if(!width||!height)return {width:maxWidth,height:Math.round(maxWidth*.75)};
  const scale=Math.min(1,maxWidth/width,maxHeight/height);return {width:Math.max(80,Math.round(width*scale)),height:Math.max(60,Math.round(height*scale))};
}

/** An attachment in a bubble. Opening it downloads it from WhatsApp first if Dar does not have it yet. */
function MediaView({threadId,message:m,platform}:{threadId:string;message:ChatMessage;platform:ChatPlatform}){
  const media=m.media!,[attempt,setAttempt]=useState(0),[failed,setFailed]=useState(false),[play,setPlay]=useState(false);
  const url=mediaUrl(threadId,m.messageId)+(attempt?`&attempt=${attempt}`:""),label=chatKindLabels[m.kind],name=localName({kind:m.kind,mime:media.mime,fileName:m.fileName,at:m.at,platform});
  const retry=()=>{setFailed(false);setAttempt(n=>n+1);};
  if(media.state==="expired")return <span className="chat-media is-missing"><Icon name="alert"/>{label} is no longer on {chatPlatformNames[platform]}</span>;
  if(media.state==="retrying")return <span className="chat-media is-missing"><span className="chat-spinner" aria-hidden="true"/>{label}: waiting for your phone to send it again</span>;
  if(failed)return <span className="chat-media is-missing"><Icon name="alert"/>{label} could not be loaded<button type="button" className="text-button" onClick={retry}>Retry</button></span>;
  if(m.kind==="image"||m.kind==="sticker"){
    const sticker=m.kind==="sticker",size=fit(media.width,media.height,sticker?160:280,sticker?160:320);
    return <a className={`chat-photo${sticker?" is-sticker":""}`} href={url} target="_blank" rel="noopener" aria-label={`Open ${label.toLowerCase()}`}>
      <img src={url} alt={m.text||label} width={size.width} height={size.height} loading="lazy" decoding="async" onError={()=>setFailed(true)}/></a>;
  }
  if(m.kind==="video"){
    if(play)return <video className="chat-video" src={url} controls autoPlay playsInline preload="metadata" onError={()=>setFailed(true)}/>;
    return <button type="button" className="chat-file" onClick={()=>setPlay(true)}><span className="chat-file-icon"><Icon name="media"/></span>
      <span className="chat-file-name"><b>Play video</b><small>{[media.seconds?clock(media.seconds):"",media.size?formatBytes(media.size):""].filter(Boolean).join(" · ")}</small></span></button>;
  }
  if(m.kind==="audio"||m.kind==="voice")return <span className="chat-audio"><audio controls preload="none" src={url} aria-label={label} onError={()=>setFailed(true)}/>{media.seconds>0&&<small>{clock(media.seconds)}</small>}</span>;
  return <a className="chat-file" href={`${url}&download=1`} download={name}><FileTile entry={{kind:"file",name,mediaType:media.mime}}/>
    <span className="chat-file-name"><b>{name}</b><small>{formatBytes(media.size)}</small></span><Icon name="download"/></a>;
}

function SaveMediaPane({session,threadId,message,platform,onClose,onSaved}:{session:BrowserSession;threadId:string;message:ChatMessage;platform:ChatPlatform;onClose:()=>void;onSaved:(name:string)=>void}){
  const [folder,setFolder]=useState<DarFileSelection>(),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const media=message.media!,name=localName({kind:message.kind,mime:media.mime,fileName:message.fileName,at:message.at,platform});
  async function save(event:FormEvent){
    event.preventDefault();if(!folder)return;setBusy(true);setError("");
    try{onSaved((await request<{name:string}>(`${base}/save`,{method:"POST",body:JSON.stringify({threadId,messageId:message.messageId,folderId:folder.id})},session.csrfToken)).name);}
    catch(cause){setError((cause as Error).message);setBusy(false);}
  }
  return <EditorPane title="Save to Files" onClose={onClose}><form className="chat-link" onSubmit={save}>
    <div className="chat-file is-static"><FileTile entry={{kind:"file",name,mediaType:media.mime}}/><span className="chat-file-name"><b>{name}</b><small>{formatBytes(media.size)}</small></span></div>
    <DarFilePicker mode="folder" allowRoot disabled={busy} onChange={setFolder}/>
    {error&&<p role="alert" className="error">{error}</p>}
    <div className="dialog-actions"><button type="button" className="text-button" onClick={onClose}>Cancel</button><button className="primary-button" disabled={busy||!folder}>{busy?"Saving…":folder?`Save to ${folder.name}`:"Save"}</button></div>
  </form></EditorPane>;
}

function LinkPane({session,accountId,platform:given,onClose,onLinked}:{session:BrowserSession;accountId?:string;platform?:ChatPlatform;onClose:()=>void;onLinked:(id:string)=>void}){
  const [platform,setPlatform]=useState<ChatPlatform>(given??"whatsapp"),discord=platform==="discord";
  const [method,setMethod]=useState<"qr"|"phone">(()=>matchMedia("(max-width:800px)").matches?"phone":"qr"),[name,setName]=useState("Personal"),[phone,setPhone]=useState("");
  const [link,setLink]=useState<ChatLinkState>(),[busy,setBusy]=useState(false),[error,setError]=useState(""),[now,setNow]=useState(Date.now()),cancelled=useRef(false);
  const id=link?.accountId;
  useEffect(()=>{
    if(!id||link?.state==="failed"||link?.state==="paired")return;
    const timer=setInterval(()=>{void request<ChatLinkState>(`${base}/accounts/${id}/link`).then(state=>{if(cancelled.current)return;setLink(state);if(state.state==="paired")setTimeout(()=>onLinked(id),1200);}).catch(()=>{});setNow(Date.now());},1500);
    return()=>clearInterval(timer);
  },[id,link?.state]);
  async function begin(event:FormEvent){
    event.preventDefault();setBusy(true);setError("");
    try{const result=await request<{accountId:string}>(`${base}/accounts/link`,{method:"POST",body:JSON.stringify({...accountId?{accountId}:{name:name.trim()||chatPlatformNames[platform],platform},...!discord&&method==="phone"?{phone}:{}})},session.csrfToken);setLink({accountId:result.accountId,state:"starting"});}
    catch(cause){setError((cause as Error).message);}finally{setBusy(false);}
  }
  async function close(){cancelled.current=true;if(id&&link?.state!=="paired")await request(`${base}/accounts/${id}/link`,{method:"DELETE"},session.csrfToken).catch(()=>{});onClose();}
  const qr=useMemo(()=>link?.qr?qrPath(qrMatrix(link.qr)):undefined,[link?.qr]);
  const left=link?.expiresAt?Math.max(0,Math.round((link.expiresAt-now)/1000)):0;
  const service=chatPlatformNames[platform];
  return <EditorPane title={accountId?`Link ${service} again`:link||given?`Link ${service}`:"Link an account"} onClose={()=>{void close();}}>
    {!link&&<form className="chat-link" onSubmit={begin}>
      {!accountId&&<Segmented label="Service" value={platform} options={[["whatsapp","WhatsApp"],["discord","Discord"]] as const} onChange={value=>{setPlatform(value);setName(value==="discord"?"Discord":"Personal");}}/>}
      {discord&&<div className="chat-banner is-warning" role="note"><Icon name="alert"/><span><b>Discord does not allow other apps on your account</b>Using Dar with your own Discord account breaks Discord’s terms; Discord can limit or close the account. Dar never sees your password.</span></div>}
      {!discord&&<Segmented label="How to link" value={method} options={[["qr","QR code"],["phone","Phone number"]] as const} onChange={setMethod}/>}
      {!discord&&method==="phone"&&<><p className="chat-hint">On a phone you can’t scan your own screen, so WhatsApp gives you a code to type instead.</p>
        <label>Phone number<input type="tel" inputMode="tel" autoComplete="tel" required value={phone} placeholder="+966 55 123 4567" onChange={e=>setPhone(e.target.value)}/></label></>}
      {!accountId&&<label>Account name<input value={name} maxLength={chatLimits.name} onChange={e=>setName(e.target.value)}/><small className="chat-hint">Names the account’s tab when you link more than one.</small></label>}
      <p className="chat-hint">{discord?"Sign in by scanning a code with the Discord app on your phone. Dar shows your servers, channels and direct messages, and keeps the messages it receives.":"Dar becomes a linked device, like WhatsApp Web. Your phone keeps working, and you can unlink Dar from either side at any time. Dar imports as much history as your phone sends."}</p>
      {error&&<p role="alert" className="error">{error}</p>}
      <div className="dialog-actions"><button type="button" className="text-button" onClick={()=>{void close();}}>Cancel</button><button className="primary-button" disabled={busy||!discord&&method==="phone"&&!phone.trim()}>{busy?"Starting…":discord||method==="qr"?"Show QR code":"Get code"}</button></div>
    </form>}
    {link&&<div className="chat-link" aria-live="polite">
      {link.state==="starting"&&<p className="chat-progress" role="status"><span className="chat-spinner" aria-hidden="true"/>Contacting {service}…</p>}
      {link.state==="qr"&&qr&&<>
        {discord&&<div className="chat-banner is-warning" role="note"><Icon name="alert"/><span><b>Discord does not allow other apps on your account</b>Discord can limit or close an account that uses Dar. Dar never sees your password.</span></div>}
        <div className="chat-qr"><svg viewBox={`0 0 ${qr.size} ${qr.size}`} role="img" aria-label={`QR code to scan with ${service}`}><path d={qr.d}/></svg></div>
        <p className="chat-progress" role="status"><span className="chat-spinner" aria-hidden="true"/>{discord?"Waiting for the Discord app":"Waiting for scan"}{left?` · ${discord?"code expires":"new code"} in ${left} s`:""}</p>
        {discord?<ol className="chat-steps"><li>Open Discord on your phone.</li><li>Go to <b>You</b> and tap <b>Scan QR code</b>.</li><li>Point it at this code and confirm <b>Log in</b>.</li></ol>
          :<ol className="chat-steps"><li>Open WhatsApp on your phone.</li><li>Go to <b>Settings → Linked devices</b> and tap <b>Link a device</b>.</li><li>Point your phone at this code.</li></ol>}</>}
      {link.state==="scanned"&&<p className="chat-progress" role="status"><span className="chat-spinner" aria-hidden="true"/>Scanned{link.name?` by ${link.name}`:""}. Confirm <b>&nbsp;Log in&nbsp;</b> in the Discord app.</p>}
      {link.state==="code"&&link.code&&<>
        <p className="chat-pairing" aria-label={`Pairing code ${link.code}`}>{[...link.code.replace("-","")].map((c,i)=><Fragment key={i}>{i===4&&<i aria-hidden="true">–</i>}<span>{c}</span></Fragment>)}</p>
        <ol className="chat-steps"><li>In WhatsApp, open <b>Linked devices → Link a device</b>.</li><li>Tap <b>Link with phone number instead</b>.</li><li>Enter this code.</li></ol>
        <p className="chat-progress" role="status"><span className="chat-spinner" aria-hidden="true"/>Waiting for WhatsApp…</p>
        <button type="button" className="secondary-button" onClick={()=>{void navigator.clipboard?.writeText(link.code!);}}><Icon name="copy"/>Copy code</button></>}
      {link.state==="paired"&&<p className="chat-progress is-done" role="status"><Icon name="check"/>{discord?"Signed in. Loading your servers…":"Linked. Importing your chats…"}</p>}
      {link.state==="failed"&&<div className="chat-banner is-error" role="alert"><Icon name="alert"/><span><b>Linking stopped</b>{link.reason??`${service} could not link this device.`}</span></div>}
      <div className="dialog-actions">{link.state==="failed"&&<button type="button" className="primary-button" onClick={()=>setLink(undefined)}>Try again</button>}<button type="button" className="text-button" onClick={()=>{void close();}}>{link.state==="paired"?"Close":"Cancel"}</button></div>
    </div>}
  </EditorPane>;
}

interface ContactChoice {name:string;phone:string;label:string;book:string;href:string}
/** New chat: pick a Dar contact with a phone number, or type a number. */
function NewChatPane({session,account,onClose,onOpened}:{session:BrowserSession;account:ChatAccount;onClose:()=>void;onOpened:(thread:ChatThread)=>void}){
  const [query,setQuery]=useState(""),[choices,setChoices]=useState<ContactChoice[]>(),[busy,setBusy]=useState(false),[error,setError]=useState("");
  useEffect(()=>{
    const timer=setTimeout(()=>{void request<{items:ContactChoice[]}>(`${base}/contacts?${new URLSearchParams({accountId:account.id,q:query})}`).then(r=>setChoices(r.items),()=>setChoices([]));},query?200:0);
    return()=>clearTimeout(timer);
  },[query,account.id]);
  const typed=query.replace(/[^\d+]/g,""),number=/^\+?\d{7,15}$/.test(typed)?typed:"";
  async function open(phone:string){setBusy(true);setError("");
    try{onOpened(await request<ChatThread>(`${base}/chats`,{method:"POST",body:JSON.stringify({accountId:account.id,phone})},session.csrfToken));}
    catch(cause){setError((cause as Error).message==="invalidInput"?"Enter the full number with its country code.":(cause as Error).message);setBusy(false);}}
  return <EditorPane title="New chat" onClose={onClose}><form className="chat-link" onSubmit={event=>{event.preventDefault();if(number)void open(number);else if(choices?.length===1)void open(choices[0].phone);}}>
    <label>Contact or phone number<input type="search" autoFocus value={query} placeholder="Name or +966 55 123 4567" onChange={e=>setQuery(e.target.value)}/>
      <small className="chat-hint">Your Dar contacts with a phone number. The chat starts from {account.name}.</small></label>
    {number&&<button type="button" className="chat-pick" disabled={busy} onClick={()=>{void open(number);}}><span className="chat-avatar contact-initials avatar-hue-0 is-small" aria-hidden="true"><Icon name="phone"/></span><span><b>Chat with {number.startsWith("+")?number:`+${number}`}</b><small>Number as typed</small></span></button>}
    {choices&&<div className="chat-picks" role="list" aria-label="Dar contacts">{choices.map(c=><div key={`${c.href}/${c.phone}`} role="listitem"><button type="button" className="chat-pick" disabled={busy} onClick={()=>{void open(c.phone);}}>
      <Avatar name={c.name} group={false} size="is-small"/><span><b>{c.name}</b><small>{c.phone}{c.label?` · ${c.label}`:""}</small></span></button></div>)}
      {!choices.length&&!number&&<p className="chat-hint">{query?"No Dar contact with a phone number matches.":"None of your Dar contacts has a phone number yet."}</p>}</div>}
    {error&&<p role="alert" className="error">{error}</p>}
    <div className="dialog-actions"><button type="button" className="text-button" onClick={onClose}>Cancel</button></div>
  </form></EditorPane>;
}

/** Saves a WhatsApp number as a Dar contact; nothing is written to WhatsApp. */
function AddContactPane({session,thread,onClose,onSaved}:{session:BrowserSession;thread:ChatThread;onClose:()=>void;onSaved:(name:string)=>void}){
  const phone=phoneOf(thread.jid),[name,setName]=useState(thread.whatsappName&&thread.whatsappName!==phone?thread.whatsappName:""),[books,setBooks]=useState<{id:string;name:string;role:string}[]>(),[book,setBook]=useState(""),[busy,setBusy]=useState(false),[error,setError]=useState("");
  useEffect(()=>{void request<{items:{id:string;name:string;role:string}[]}>("/api/v1/contacts/books").then(r=>{const writable=r.items.filter(b=>b.role!=="read");setBooks(writable);setBook(writable[0]?.id??"");},cause=>setError((cause as Error).message));},[]);
  async function save(event:FormEvent){
    event.preventDefault();if(!book||!name.trim())return;setBusy(true);setError("");
    try{await request(`/api/v1/contacts/books/${book}/items`,{method:"POST",headers:{"if-none-match":"*"},body:JSON.stringify({data:{displayName:name.trim(),phones:[{value:phone,label:"cell"}]}})},session.csrfToken);onSaved(name.trim());}
    catch(cause){setError((cause as Error).message);setBusy(false);}
  }
  return <EditorPane title="Add to Contacts" onClose={onClose}><form className="chat-link" onSubmit={save}>
    <label>Name<input autoFocus required maxLength={1024} value={name} onChange={e=>setName(e.target.value)}/></label>
    <label>Phone<input readOnly value={phone}/></label>
    {books&&books.length>1&&<label>Address book<Select value={book} onChange={e=>setBook(e.target.value)}>{books.map(b=><option key={b.id} value={b.id}>{b.name}</option>)}</Select></label>}
    {books&&!books.length&&<p className="chat-hint">You have no address book you can add to. Create one in Contacts first.</p>}
    <p className="chat-hint">The contact is saved in Dar only; WhatsApp and your phone are not changed.</p>
    {error&&<p role="alert" className="error">{error}</p>}
    <div className="dialog-actions"><button type="button" className="text-button" onClick={onClose}>Cancel</button><button className="primary-button" disabled={busy||!book||!name.trim()}>{busy?"Saving…":"Add contact"}</button></div>
  </form></EditorPane>;
}

function RenamePane({session,account,onClose,onSaved}:{session:BrowserSession;account:ChatAccount;onClose:()=>void;onSaved:()=>void}){
  const [name,setName]=useState(account.name),[busy,setBusy]=useState(false),[error,setError]=useState("");
  async function save(event:FormEvent){event.preventDefault();setBusy(true);setError("");
    try{await request(`${base}/accounts/${account.id}`,{method:"PATCH",body:JSON.stringify({name:name.trim()})},session.csrfToken);onSaved();}catch(cause){setError((cause as Error).message);setBusy(false);}}
  return <EditorPane title="Rename account" onClose={onClose}><form className="chat-link" onSubmit={save}>
    <label>Name<input autoFocus required maxLength={chatLimits.name} value={name} onChange={e=>setName(e.target.value)}/></label>
    {error&&<p role="alert" className="error">{error}</p>}
    <div className="dialog-actions"><button type="button" className="text-button" onClick={onClose}>Cancel</button><button className="primary-button" disabled={busy||!name.trim()}>{busy?"Saving…":"Save"}</button></div>
  </form></EditorPane>;
}

function ChatSettings({accounts,onClose,onLink,actions}:{accounts:ChatAccount[];onClose:()=>void;onLink:()=>void;actions:(a:ChatAccount)=>MenuAction[]}){
  const [section,setSection]=useState("Accounts");
  return <SettingsSheet title="Chat settings" icon="chat" sections={["Accounts","Keyboard shortcuts"]} section={section} onSection={setSection} onClose={onClose} footer="Each account keeps its own chats.">
    {section==="Accounts"&&<section className="settings-group" aria-label="Accounts"><h3>Linked accounts</h3>
      {accounts.map(a=><SettingRow key={a.id} label={a.name} hint={`${chatPlatformNames[a.platform]}${a.platform==="whatsapp"&&phoneOf(a.jid)?` · ${phoneOf(a.jid)}`:""} · ${a.state==="connected"?"Connected":a.state==="loggedOut"?a.platform==="discord"?"Signed out":"Unlinked":a.state==="linking"?"Not linked yet":a.state==="failed"?"Stopped":"Reconnecting"}`}>
        <ActionMenu label={`Actions for ${a.name}`} actions={actions(a)}><Icon name="more"/></ActionMenu></SettingRow>)}
      <SettingRow label="Link an account" hint={`WhatsApp or Discord, up to ${chatLimits.accounts}.`}><button type="button" className="secondary-button" onClick={onLink}><Icon name="plus"/>Link an account</button></SettingRow></section>}
    {section==="Keyboard shortcuts"&&<dl className="ui-shortcuts"><dt>Enter</dt><dd>Send the message</dd><dt>Shift Enter</dt><dd>New line</dd><dt>Escape</dt><dd>Cancel a reply</dd><dt>Ctrl / Cmd K</dt><dd>Search all apps</dd></dl>}
  </SettingsSheet>;
}

/** Discord servers in the sidebar: each opens to its channels, grouped by category (mock B). */
function ServerTree({servers,selected,collapsed,onToggle,onOpen}:{servers?:ChatServer[];selected:string;collapsed:ReadonlySet<string>;onToggle:(id:string)=>void;onOpen:(thread:ChatThread,el:HTMLElement)=>void}){
  if(!servers)return <p className="chat-hint chat-tree-hint" role="status">Loading servers…</p>;
  if(!servers.length)return <p className="chat-hint chat-tree-hint">No servers yet.</p>;
  return <nav className="chat-servers" aria-label="Servers"><h2 className="chat-tree-heading">Servers</h2>
    {servers.map(s=>{const open=!collapsed.has(s.id),quiet=quietUntil(s.mutedUntil);let section:string|undefined;
      return <div key={s.id} className="chat-server">
        <button type="button" className={`nav-item chat-server-row${s.unread&&!open?" is-unread":""}${quiet?" is-muted":""}`} aria-expanded={open} onClick={()=>onToggle(s.id)}>
          <span className={`chat-server-mark contact-initials ${avatarHue(s.serverId)}`} aria-hidden="true">{initials(s.name)}</span><span className="chat-row-title">{s.name}</span>
          {s.mentions>0&&<span className="chat-mentions">{s.mentions}<span className="sr-only"> mentions</span></span>}<Icon name="chevron" className={`chat-server-chevron${open?" is-open":""}`}/></button>
        {open&&<ul className="chat-channels" aria-label={`Channels in ${s.name}`}>{s.channels.map(c=>{
          const heading=c.section!==section?(section=c.section,c.section?<li key={`section-${c.id}`} className="chat-section" aria-hidden="true">{c.section}</li>:null):null;
          const current=c.id===selected,muted=quietUntil(c.mutedUntil);
          return <Fragment key={c.id}>{heading}<li><button type="button" className={`nav-item chat-channel${current?" active":""}${c.unread>0&&!muted?" is-unread":""}${muted?" is-muted":""}`} aria-current={current?"page":undefined}
            aria-label={`${c.kind==="voice"?"Voice channel":"Channel"} ${c.name}${c.mentions?`, ${c.mentions} mentions`:c.unread>0&&!muted?", unread":""}${muted?", muted":""}`} onClick={event=>onOpen(c,event.currentTarget)}>
            <Icon name={c.kind==="voice"?"speaker":"hash"}/><span className="chat-row-title">{c.name}</span>{c.mentions>0&&<span className="chat-mentions">{c.mentions}</span>}{muted&&<Icon name="bell" className="chat-channel-muted"/>}</button></li></Fragment>;
        })}{!s.channels.length&&<li className="chat-section">No channels you can see</li>}</ul>}
      </div>;})}
  </nav>;
}

/** Discord: start a direct message with a friend. */
function NewMessagePane({session,account,onClose,onOpened}:{session:BrowserSession;account:ChatAccount;onClose:()=>void;onOpened:(thread:ChatThread)=>void}){
  const [query,setQuery]=useState(""),[friends,setFriends]=useState<ChatFriend[]>(),[busy,setBusy]=useState(false),[error,setError]=useState("");
  useEffect(()=>{void request<{friends:ChatFriend[]}>(`${base}/friends?${new URLSearchParams({accountId:account.id})}`).then(r=>setFriends(r.friends),cause=>{setFriends([]);setError((cause as Error).message);});},[account.id]);
  const q=query.trim().toLocaleLowerCase(),shown=(friends??[]).filter(f=>!q||f.name.toLocaleLowerCase().includes(q)||f.username.toLocaleLowerCase().includes(q)).slice(0,50);
  async function open(user:string){setBusy(true);setError("");
    try{onOpened(await request<ChatThread>(`${base}/chats`,{method:"POST",body:JSON.stringify({accountId:account.id,user})},session.csrfToken));}
    catch(cause){setError((cause as Error).message);setBusy(false);}}
  return <EditorPane title="New message" onClose={onClose}><form className="chat-link" onSubmit={event=>{event.preventDefault();if(shown.length===1)void open(shown[0].id);}}>
    <label>Friend<input type="search" autoFocus value={query} placeholder="Name or username" onChange={e=>setQuery(e.target.value)}/>
      <small className="chat-hint">Your Discord friends. The message goes out from {account.name}.</small></label>
    {!friends&&!error&&<p className="chat-hint" role="status">Loading friends…</p>}
    {friends&&<div className="chat-picks" role="list" aria-label="Friends">{shown.map(f=><div key={f.id} role="listitem"><button type="button" className="chat-pick" disabled={busy} onClick={()=>{void open(f.id);}}>
      <Avatar name={f.name} group={false} size="is-small"/><span><b>{f.name}</b><small>{f.username}</small></span></button></div>)}
      {!shown.length&&<p className="chat-hint">{q?"No friend matches.":"You have no Discord friends yet."}</p>}</div>}
    {error&&<p role="alert" className="error">{error}</p>}
    <div className="dialog-actions"><button type="button" className="text-button" onClick={onClose}>Cancel</button></div>
  </form></EditorPane>;
}

function ConfirmAction({title,text,action,run,onClose}:Confirm&{onClose:()=>void}){
  const [busy,setBusy]=useState(false),[error,setError]=useState("");
  return <Dialog title={title} onClose={onClose}><p>{text}</p>{error&&<p role="alert" className="error">{error}</p>}<div className="dialog-actions"><button className="text-button" onClick={onClose}>Cancel</button><button className="danger-button" disabled={busy} onClick={()=>{setBusy(true);setError("");void run().then(onClose,cause=>{setError((cause as Error).message);setBusy(false);});}}>{busy?"Working…":action}</button></div></Dialog>;
}
