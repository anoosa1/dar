package email

import (
	"html/template"
	"io"
	"net/http"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
)

type App struct{}

func (a App) Name() string                     { return "email" }
func (a App) DisplayName() string              { return "Email" }
func (a App) Icon() string                     { return `<i class="fa-solid fa-envelope"></i>` }
func (a App) Description() string              { return "IMAP/SMTP email client" }
func (a App) AppBarOrder() int                 { return 8 }
func (a App) DaemonConfig() *apps.DaemonConfig { return nil }
func (a App) Routes() []apps.Route {
	return []apps.Route{{Method: "GET", Pattern: "/email", Handler: handlePage}}
}

func handlePage(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil { http.Redirect(w, r, "/login", http.StatusSeeOther); return }
	renderPage(w, r, user.DisplayName)
}

func renderPage(w io.Writer, r *http.Request, displayName string) {
	webui.Render(w, webui.PageData{
		Title: "Email — Dar", AppBar: server.AppBarHTML(r), UserMenu: webui.UserMenuHTML(displayName),
		Sidebar: webui.BuildSidebar([]webui.SidebarItem{
			{Label: "Inbox", Icon: "fa-solid fa-inbox", Href: "/email", Active: true},
			{Label: "Sent", Icon: "fa-solid fa-paper-plane", Href: "/email?folder=Sent"},
			{Label: "Drafts", Icon: "fa-solid fa-file-pen", Href: "/email?folder=Drafts"},
			{Label: "Trash", Icon: "fa-solid fa-trash", Href: "/email?folder=Trash"},
		}, "/email?view=accounts"),
		Content: template.HTML(emailHTML()),
	})
}

func emailHTML() string {
	return `
<div id="email-app" class="flex h-[calc(100vh-120px)] -m-4 md:-m-6 gap-0">
	<!-- Folder sidebar -->
	<div class="w-52 shrink-0 border-r border-outline-variant overflow-y-auto p-2 hidden md:block" id="folder-sidebar">
		<div class="mb-3">
			<select id="account-select" onchange="selectAccount()" class="w-full h-9 px-3 rounded-full bg-surface-container text-sm border-0 outline-none"></select>
		</div>
		<button onclick="showCompose()" class="w-full h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90 mb-3 inline-flex items-center gap-2 justify-center">
			<i class="fa-solid fa-pen"></i> Compose</button>
		<div id="folder-list" class="space-y-0.5"></div>
		<div class="mt-3 pt-3 border-t border-outline-variant">
			<button onclick="syncAccount()" class="w-full h-9 px-3 rounded-full text-sm text-on-surface-variant border-0 bg-transparent cursor-pointer hover:bg-surface-container-highest text-left inline-flex items-center gap-2">
				<i class="fa-solid fa-rotate"></i> Sync</button>
			<button onclick="showAccounts()" class="w-full h-9 px-3 rounded-full text-sm text-on-surface-variant border-0 bg-transparent cursor-pointer hover:bg-surface-container-highest text-left inline-flex items-center gap-2">
				<i class="fa-solid fa-gear"></i> Accounts</button>
		</div>
	</div>

	<!-- Message list -->
	<div class="w-72 shrink-0 border-r border-outline-variant overflow-y-auto hidden md:block" id="msg-list-panel">
		<div id="msg-list" class="divide-y divide-outline-variant"></div>
	</div>

	<!-- Message view -->
	<div class="flex-1 overflow-y-auto" id="msg-view-panel">
		<div id="msg-view" class="p-4 text-center text-on-surface-variant pt-12">
			<i class="fa-solid fa-envelope text-4xl mb-2 block"></i>Select a message
		</div>
	</div>
</div>

<!-- Mobile: show panels sequentially -->
<div class="md:hidden" id="mobile-view">
	<div id="mobile-list"></div>
</div>

<!-- Compose modal -->
<div id="compose-modal" class="hidden fixed inset-0 bg-black/30 z-50 flex items-center justify-center p-4" onclick="if(event.target===this)closeCompose()">
	<div class="bg-surface rounded-2xl p-6 w-full max-w-lg shadow-2">
		<h2 class="text-lg font-semibold mb-4">New Message</h2>
		<input id="compose-to" placeholder="To" class="w-full h-10 px-4 mb-2 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
		<input id="compose-cc" placeholder="Cc" class="w-full h-10 px-4 mb-2 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
		<input id="compose-subject" placeholder="Subject" class="w-full h-10 px-4 mb-2 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
		<textarea id="compose-body" placeholder="Write your message..." rows="8" class="w-full px-4 py-3 rounded-xl bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30 resize-none"></textarea>
		<div class="flex gap-2 justify-end mt-4">
			<button onclick="closeCompose()" class="h-10 px-4 rounded-full text-sm font-medium text-on-surface-variant border-0 bg-transparent cursor-pointer hover:bg-surface-container-highest">Cancel</button>
			<button onclick="sendEmail()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90">Send</button>
		</div>
	</div>
</div>

<!-- Accounts modal -->
<div id="accounts-modal" class="hidden fixed inset-0 bg-black/30 z-50 flex items-center justify-center p-4" onclick="if(event.target===this)document.getElementById('accounts-modal').classList.add('hidden')">
	<div class="bg-surface rounded-2xl p-6 w-full max-w-md shadow-2">
		<h2 class="text-lg font-semibold mb-4">Email Accounts</h2>
		<div id="accounts-list" class="space-y-2 mb-4"></div>
		<button onclick="showAddAccount()" class="w-full h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90">Add Account</button>
		<div id="add-account-form" class="hidden mt-4 space-y-2">
			<input id="acct-email" placeholder="Email" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<input id="acct-name" placeholder="Your name" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<input id="acct-imap" placeholder="IMAP host" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<input id="acct-user" placeholder="Username" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<input id="acct-smtp" placeholder="SMTP host" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<div class="flex gap-2 justify-end">
				<button onclick="document.getElementById('add-account-form').classList.add('hidden')" class="h-10 px-4 rounded-full text-sm font-medium text-on-surface-variant border-0 bg-transparent cursor-pointer">Cancel</button>
				<button onclick="addAccount()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90">Save</button>
			</div>
		</div>
	</div>
</div>

<script>
const apiBase="/api/email";
let accounts=[],currentAccount=null,currentFolder="",currentPage=1,messages=[],currentMsg=null;

async function api(m,p,b){let o={method:m};if(b){o.headers={"Content-Type":"application/json"};o.body=JSON.stringify(b)};let r=await fetch(apiBase+p,o);return r.json()}

function init(){loadAccounts()}
function loadAccounts(){api("GET","/accounts").then(d=>{accounts=d.accounts||[];renderAccountSelect();if(accounts.length>0){currentAccount=accounts[0].id;selectAccount()}})}
function renderAccountSelect(){let s=document.getElementById("account-select");s.innerHTML=accounts.map(a=>'<option value="'+a.id+'">'+esc(a.email)+'</option>').join("")}
function selectAccount(){currentAccount=document.getElementById("account-select").value;currentFolder="";loadFolders();loadMessages()}

function loadFolders(){if(!currentAccount)return;api("GET","/folders?account="+currentAccount).then(d=>renderFolders(d.folders||[]))}
function renderFolders(folders){let d=document.getElementById("folder-list");
	let items=[{name:"Inbox",icon:"fa-inbox"},{name:"Sent",icon:"fa-paper-plane"},{name:"Drafts",icon:"fa-file-pen"},{name:"Trash",icon:"fa-trash"}];
	d.innerHTML=items.map(f=>{let fd=folders.find(x=>x.name===f.name||(f.name==="Inbox"&&x.name===""));
		let count=fd?fd.new:0;let badge=count>0?'<span class="ml-auto bg-primary text-white text-xs rounded-full px-2 py-0.5">'+count+'</span>':'';
		let active=currentFolder===f.name?' bg-primary-container text-primary font-medium':'';
		return'<a href="#" onclick="navigateFolder(\''+f.name+'\')" class="flex items-center gap-2 rounded-full px-3 h-9 text-sm no-underline hover:bg-surface-container-highest'+active+'"><i class="fa-solid '+f.icon+' w-4 text-center"></i><span>'+f.name+'</span>'+badge+'</a>'}).join("")}

function navigateFolder(f){currentFolder=f;currentPage=1;loadMessages()}
function loadMessages(){if(!currentAccount)return;api("GET","/messages?account="+currentAccount+"&folder="+currentFolder+"&page="+currentPage).then(d=>{messages=d.messages||[];renderMessages()})}
function renderMessages(){let d=document.getElementById("msg-list");if(messages.length===0){d.innerHTML='<div class="text-center text-on-surface-variant py-8 text-sm">No messages</div>';return}
	d.innerHTML=messages.map(m=>'<div class="flex items-start gap-2 px-3 py-2.5 cursor-pointer hover:bg-surface-container-high transition-colors'+(m.seen?'':' font-semibold')+(currentMsg&&currentMsg.id===m.id?' bg-primary-container/30':'')+'" onclick="viewMessage(\''+m.id+'\')">'+
		'<div class="flex-1 min-w-0"><div class="text-xs text-on-surface truncate">'+esc(m.from||"")+'</div>'+
		'<div class="text-xs text-on-surface-variant truncate">'+esc(m.subject||"(no subject)")+'</div>'+
		'<div class="text-[10px] text-on-surface-variant mt-0.5">'+fmtDate(m.date)+'</div></div>'+
		(m.flagged?'<i class="fa-solid fa-star text-yellow-500 text-[10px] mt-1"></i>':'')+
		'</div>').join("")}

function viewMessage(id){currentMsg=messages.find(m=>m.id===id);if(!currentMsg)return;
	api("GET","/message?id="+id+"&account="+currentAccount+"&folder="+currentFolder).then(m=>{
		let dv=document.getElementById("msg-view");
		dv.innerHTML='<div class="p-4"><div class="flex items-center gap-2 mb-3">'+
			'<button onclick="showCompose()" class="h-8 px-3 rounded-full bg-surface-container-high text-xs font-medium border-0 cursor-pointer hover:bg-surface-container-highest inline-flex items-center gap-1"><i class="fa-solid fa-reply"></i> Reply</button>'+
			'<button onclick="deleteMsg()" class="h-8 px-3 rounded-full bg-error-container text-error text-xs font-medium border-0 cursor-pointer hover:opacity-90 inline-flex items-center gap-1"><i class="fa-solid fa-trash"></i> Delete</button>'+
			'</div>'+
			'<h2 class="text-base font-semibold mb-1">'+esc(m.subject||"(no subject)")+'</h2>'+
			'<div class="text-xs text-on-surface-variant mb-1"><span class="font-medium">'+esc(m.from)+'</span></div>'+
			'<div class="text-xs text-on-surface-variant mb-4">To: '+esc(m.to||"")+' &middot; '+esc(m.date||"")+'</div>'+
			'<div class="text-sm text-on-surface whitespace-pre-wrap border-t border-outline-variant pt-3">'+esc(m.body||"")+'</div></div>';
		renderMessages() // update seen state
	})}

function showCompose(){["compose-to","compose-cc","compose-subject","compose-body"].forEach(id=>document.getElementById(id).value="");document.getElementById("compose-modal").classList.remove("hidden")}
function closeCompose(){document.getElementById("compose-modal").classList.add("hidden")}
function sendEmail(){if(!currentAccount)return;let to=document.getElementById("compose-to").value,cc=document.getElementById("compose-cc").value,subject=document.getElementById("compose-subject").value,body=document.getElementById("compose-body").value;if(!to)return;
	api("POST","/send",{account_id:currentAccount,to:to,cc:cc,subject:subject,body:body}).then(r=>{closeCompose();if(r.status==="sent"){navigateFolder("Sent")}})}

function syncAccount(){if(!currentAccount)return;api("POST","/sync?account="+currentAccount).then(r=>{setTimeout(()=>{loadFolders();loadMessages()},2000)})}
function deleteMsg(){if(!currentMsg||!currentAccount)return;if(!confirm("Delete?"))return;
	api("POST","/message?id="+currentMsg.id+"&account="+currentAccount+"&folder="+currentFolder+"&action=move&to=Trash").then(()=>loadMessages())}

function showAccounts(){document.getElementById("accounts-modal").classList.remove("hidden");renderAccountsList()}
function renderAccountsList(){let d=document.getElementById("accounts-list");d.innerHTML=accounts.map(a=>'<div class="flex items-center gap-2 p-2 rounded-lg bg-surface-container"><span class="text-sm flex-1">'+esc(a.email)+'</span><button onclick="deleteAccount(\''+a.id+'\')" class="text-xs text-error cursor-pointer border-0 bg-transparent"><i class="fa-solid fa-trash"></i></button></div>').join("")||'<p class="text-sm text-on-surface-variant">No accounts</p>'}
function showAddAccount(){document.getElementById("add-account-form").classList.remove("hidden")}
function addAccount(){let a={email:document.getElementById("acct-email").value,name:document.getElementById("acct-name").value,imap_host:document.getElementById("acct-imap").value,username:document.getElementById("acct-user").value,smtp_host:document.getElementById("acct-smtp").value,imap_tls:true,smtp_tls:true};
	api("POST","/accounts",a).then(r=>{document.getElementById("add-account-form").classList.add("hidden");loadAccounts();showAccounts()})}
function deleteAccount(id){if(!confirm("Delete account?"))return;api("DELETE","/accounts?id="+id).then(()=>{loadAccounts();document.getElementById("accounts-modal").classList.add("hidden")})}

function fmtDate(d){if(!d)return"";try{return new Date(d).toLocaleDateString()}catch(e){return d}}
function esc(s){let d=document.createElement("div");d.textContent=s;return d.innerHTML}
init();
</script>`
}
