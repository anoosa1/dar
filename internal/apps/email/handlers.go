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
func (a App) Description() string              { return "Email client" }
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
			{Label: "Sent", Icon: "fa-solid fa-paper-plane", Href: "/email?folder=sent"},
			{Label: "Drafts", Icon: "fa-solid fa-file-pen", Href: "/email?folder=drafts"},
			{Label: "Trash", Icon: "fa-solid fa-trash", Href: "/email?folder=trash"},
		}, "/settings"),
		Content: template.HTML(emailHTML()),
	})
}

func emailHTML() string {
	return `
<div id="email-app" class="max-w-3xl mx-auto">
	<div class="flex items-center gap-3 mb-4">
		<button onclick="showCompose()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90 inline-flex items-center gap-2">
			<i class="fa-solid fa-pen"></i> Compose</button>
	</div>
	<div id="message-list" class="space-y-1"></div>

	<!-- Compose modal -->
	<div id="compose-modal" class="hidden fixed inset-0 bg-black/30 z-40 flex items-center justify-center p-4" onclick="if(event.target===this)document.getElementById('compose-modal').classList.add('hidden')">
		<div class="bg-surface rounded-2xl p-6 w-full max-w-lg shadow-2">
			<h2 class="text-lg font-semibold mb-4">New Message</h2>
			<input id="compose-to" placeholder="To" class="w-full h-10 px-4 mb-2 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<input id="compose-subject" placeholder="Subject" class="w-full h-10 px-4 mb-2 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<textarea id="compose-body" placeholder="Write your message..." rows="6" class="w-full px-4 py-3 rounded-xl bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30 resize-none"></textarea>
			<div class="flex gap-2 justify-end mt-4">
				<button onclick="document.getElementById('compose-modal').classList.add('hidden')" class="h-10 px-4 rounded-full text-sm font-medium text-on-surface-variant border-0 bg-transparent cursor-pointer hover:bg-surface-container-highest">Cancel</button>
				<button onclick="sendEmail()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90">Send</button>
			</div>
		</div>
	</div>
</div>
<script>
const apiBase="/api/email";
let currentFolder="inbox", currentMsg=null;

async function api(m,p,b){let o={method:m};if(b){o.headers={"Content-Type":"application/json"};o.body=JSON.stringify(b)};let r=await fetch(apiBase+p,o);return r.json()}

function init(){let q=new URLSearchParams(window.location.search);currentFolder=q.get("folder")||"inbox";loadMessages()}
function loadMessages(){api("GET","/messages?folder="+currentFolder).then(d=>renderMessages(d.messages||[]))}
function renderMessages(msgs){let d=document.getElementById("message-list")
	if(msgs.length===0){d.innerHTML='<div class="text-center text-on-surface-variant py-12"><i class="fa-solid fa-envelope text-4xl mb-2 block"></i>No messages</div>';return}
	d.innerHTML=msgs.map(m=>'<div class="flex items-start gap-3 px-4 py-3 hover:bg-surface-container-high rounded-xl cursor-pointer transition-colors'+(m.read?'':' font-bold')+'" onclick="viewMessage(\''+m.id+'\')">'+
		'<div class="flex-1 min-w-0"><div class="flex items-center gap-2"><span class="text-sm text-on-surface truncate">'+esc(m.from||m.to||"")+'</span>'+
		'<span class="text-sm font-medium text-on-surface truncate">'+esc(m.subject)+'</span></div>'+
		'<div class="text-xs text-on-surface-variant truncate">'+esc(m.body||"").substring(0,100)+'</div></div>'+
		'<span class="text-xs text-on-surface-variant shrink-0">'+new Date(m.date*1000).toLocaleDateString()+'</span>'+
		'<button class="w-6 h-6 rounded-full bg-transparent border-0 cursor-pointer shrink-0 text-on-surface-variant hover:text-error" onclick="event.stopPropagation();deleteMsg(\''+m.id+'\')"><i class="fa-solid fa-trash text-xs"></i></button>'+
		'</div>').join("")}
function viewMessage(id){api("GET","/message?id="+id).then(m=>alert(m.from+"\n"+m.subject+"\n\n"+m.body));loadMessages()}
function showCompose(){["compose-to","compose-subject","compose-body"].forEach(id=>document.getElementById(id).value="");document.getElementById("compose-modal").classList.remove("hidden")}
function sendEmail(){let to=document.getElementById("compose-to").value,subject=document.getElementById("compose-subject").value,body=document.getElementById("compose-body").value;if(!to)return
	api("POST","/message",{to:to,subject:subject,body:body,folder:"sent"}).then(()=>{document.getElementById("compose-modal").classList.add("hidden");loadMessages()})}
function deleteMsg(id){api("DELETE","/message?id="+id).then(loadMessages)}
function esc(s){let d=document.createElement("div");d.textContent=s;return d.innerHTML}
init();
</script>`
}
