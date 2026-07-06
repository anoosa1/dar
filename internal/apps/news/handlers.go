package news

import (
	"html/template"
	"io"
	"net/http"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
)

type App struct{}

func (a App) Name() string                     { return "news" }
func (a App) DisplayName() string              { return "News" }
func (a App) Icon() string                     { return `<i class="fa-solid fa-newspaper"></i>` }
func (a App) Description() string              { return "RSS feed reader" }
func (a App) AppBarOrder() int                 { return 7 }
func (a App) DaemonConfig() *apps.DaemonConfig { return nil }
func (a App) Routes() []apps.Route {
	return []apps.Route{{Method: "GET", Pattern: "/news", Handler: handlePage}}
}

func handlePage(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil { http.Redirect(w, r, "/login", http.StatusSeeOther); return }
	renderPage(w, r, user.DisplayName)
}

func renderPage(w io.Writer, r *http.Request, displayName string) {
	webui.Render(w, webui.PageData{
		Title: "News — Dar", AppBar: server.AppBarHTML(r), UserMenu: webui.UserMenuHTML(displayName),
		Sidebar: webui.BuildSidebar([]webui.SidebarItem{
			{Label: "All articles", Icon: "fa-solid fa-newspaper", Href: "/news", Active: true},
			{Label: "Unread", Icon: "fa-solid fa-envelope", Href: "/news?unread=true"},
			{Label: "Starred", Icon: "fa-solid fa-star", Href: "/news?starred=true"},
		}, "/settings"),
		Content: template.HTML(newsHTML()),
	})
}

func newsHTML() string {
	return `
<div id="news-app" class="max-w-3xl mx-auto">
	<div class="flex items-center gap-3 mb-4">
		<button onclick="showAddFeed()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90 inline-flex items-center gap-2">
			<i class="fa-solid fa-plus"></i> Add feed</button>
		<button onclick="refreshFeeds()" class="h-10 px-4 rounded-full bg-surface-container-high text-on-surface text-sm font-medium border-0 cursor-pointer hover:bg-surface-container-highest inline-flex items-center gap-2">
			<i class="fa-solid fa-rotate"></i> Refresh</button>
		<div class="flex-1"></div>
		<select id="feed-filter" onchange="loadArticles()" class="h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none">
			<option value="">All feeds</option></select>
	</div>
	<div id="article-list" class="space-y-1"></div>
	<div id="feed-modal" class="hidden fixed inset-0 bg-black/30 z-40 flex items-center justify-center p-4" onclick="if(event.target===this)document.getElementById('feed-modal').classList.add('hidden')">
		<div class="bg-surface rounded-2xl p-6 w-full max-w-sm shadow-2">
			<h2 class="text-lg font-semibold mb-4">Add RSS Feed</h2>
			<input id="feed-url" placeholder="https://example.com/rss" class="w-full h-10 px-4 mb-3 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<div class="flex gap-2 justify-end">
				<button onclick="document.getElementById('feed-modal').classList.add('hidden')" class="h-10 px-4 rounded-full text-sm font-medium text-on-surface-variant border-0 bg-transparent cursor-pointer hover:bg-surface-container-highest">Cancel</button>
				<button onclick="addFeed()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90">Add</button>
			</div>
		</div>
	</div>
</div>
<script>
const apiBase="/api/news";
async function api(m,p,b){let o={method:m};if(b){o.headers={"Content-Type":"application/json"};o.body=JSON.stringify(b)};let r=await fetch(apiBase+p,o);return r.json()}
function init(){loadFeeds();loadArticles()}
function loadFeeds(){api("GET","/feeds").then(d=>{let s=document.getElementById("feed-filter");s.innerHTML='<option value="">All feeds</option>'+(d.feeds||[]).map(f=>'<option value="'+f.id+'">'+esc(f.title)+'</option>').join("")})}
function loadArticles(){let feed=document.getElementById("feed-filter").value;let q=new URLSearchParams(window.location.search);let unread=q.get("unread")||"";let params=feed?"?feed_id="+feed:(unread?"?unread="+unread:"");api("GET","/articles"+params).then(d=>renderArticles(d.articles||[]))}
function renderArticles(articles){let d=document.getElementById("article-list");if(articles.length===0){d.innerHTML='<div class="text-center text-on-surface-variant py-12"><i class="fa-solid fa-newspaper text-4xl mb-2 block"></i>No articles</div>';return}
	d.innerHTML=articles.map(a=>'<div class="flex items-start gap-3 px-4 py-3 hover:bg-surface-container-high rounded-xl cursor-pointer transition-colors'+(a.read?' opacity-60':'')+'" onclick="window.open(\''+a.link+'\',\'_blank\');markRead(\''+a.id+'\')">'+
		'<div class="flex-1"><div class="text-sm font-medium text-on-surface">'+esc(a.title)+'</div><div class="text-xs text-on-surface-variant mt-1 line-clamp-2">'+esc(a.summary||"")+'</div><div class="text-xs text-on-surface-variant mt-1">'+esc(a.date||"")+'</div></div>'+
		'<button class="w-6 h-6 rounded-full bg-transparent border-0 cursor-pointer shrink-0 mt-1 '+ (a.starred?'text-yellow-500':'text-on-surface-variant')+'" onclick="event.stopPropagation();toggleStar(\''+a.id+'\')"><i class="fa-solid fa-star text-xs"></i></button>'+
		'</div>').join("")}
function showAddFeed(){document.getElementById("feed-url").value="";document.getElementById("feed-modal").classList.remove("hidden")}
function addFeed(){let u=document.getElementById("feed-url").value.trim();if(!u)return;api("POST","/feed",{url:u}).then(()=>{document.getElementById("feed-modal").classList.add("hidden");loadFeeds();loadArticles()})}
function refreshFeeds(){api("POST","/refresh").then(()=>setTimeout(loadArticles,1000))}
function markRead(id){api("GET","/articles?star="+id)}
function toggleStar(id){api("GET","/articles?star="+id).then(loadArticles)}
function esc(s){let d=document.createElement("div");d.textContent=s;return d.innerHTML}
init();
</script>`
}
