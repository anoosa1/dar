package photos

import (
	"html/template"
	"io"
	"net/http"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
)

type App struct{}

func (a App) Name() string                     { return "photos" }
func (a App) DisplayName() string              { return "Photos" }
func (a App) Icon() string                     { return `<i class="fa-solid fa-image"></i>` }
func (a App) Description() string              { return "Photo gallery" }
func (a App) AppBarOrder() int                 { return 9 }
func (a App) DaemonConfig() *apps.DaemonConfig { return nil }
func (a App) Routes() []apps.Route {
	return []apps.Route{{Method: "GET", Pattern: "/photos", Handler: handlePage}}
}

func handlePage(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil { http.Redirect(w, r, "/login", http.StatusSeeOther); return }
	renderPage(w, r, user.DisplayName)
}

func renderPage(w io.Writer, r *http.Request, displayName string) {
	webui.Render(w, webui.PageData{
		Title: "Photos — Dar", AppBar: server.AppBarHTML(r), UserMenu: webui.UserMenuHTML(displayName),
		Sidebar: webui.BuildSidebar([]webui.SidebarItem{
			{Label: "All photos", Icon: "fa-solid fa-image", Href: "/photos", Active: true},
			{Label: "Albums", Icon: "fa-solid fa-folder", Href: "/photos?view=albums"},
			{Label: "Favorites", Icon: "fa-solid fa-heart", Href: "/photos?filter=favorites"},
		}, "/settings"),
		Content: template.HTML(photosHTML()),
	})
}

func photosHTML() string {
	return `
<div id="photos-app" class="max-w-5xl mx-auto">
	<div class="flex items-center gap-3 mb-4">
		<label class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium cursor-pointer hover:opacity-90 inline-flex items-center gap-2">
			<i class="fa-solid fa-upload"></i> Upload
			<input type="file" id="upload-input" onchange="uploadPhotos()" class="hidden" multiple accept="image/*">
		</label>
		<button onclick="showNewAlbum()" class="h-10 px-4 rounded-full bg-surface-container-high text-on-surface text-sm font-medium border-0 cursor-pointer hover:bg-surface-container-highest inline-flex items-center gap-2">
			<i class="fa-solid fa-folder-plus"></i> Album</button>
	</div>
	<div id="gallery" class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 gap-2"></div>

	<!-- Lightbox -->
	<div id="lightbox" class="hidden fixed inset-0 bg-black/90 z-50 flex items-center justify-center p-4" onclick="closeLightbox()">
		<button onclick="closeLightbox()" class="absolute top-4 right-4 text-white text-2xl cursor-pointer bg-transparent border-0">&times;</button>
		<div class="max-w-full max-h-full flex flex-col items-center" onclick="event.stopPropagation()">
			<img id="lightbox-img" class="max-w-full max-h-[80vh] rounded-xl object-contain" src="">
			<div class="flex gap-2 mt-3">
				<button onclick="toggleFav()" id="fav-btn" class="h-10 px-4 rounded-full bg-white/20 text-white text-sm font-medium border-0 cursor-pointer hover:bg-white/30"><i class="fa-solid fa-heart"></i></button>
				<button onclick="deletePhoto()" class="h-10 px-4 rounded-full bg-red-500/50 text-white text-sm font-medium border-0 cursor-pointer hover:bg-red-500/70"><i class="fa-solid fa-trash"></i> Delete</button>
			</div>
		</div>
	</div>
</div>

<script>
const apiBase="/api/photos";
let photos=[],currentPhoto=null;

async function api(m,p,b){let o={method:m};if(b){o.headers={"Content-Type":"application/json"};o.body=JSON.stringify(b)};let r=await fetch(apiBase+p,o);return r.json()}

function init(){loadPhotos()}
function loadPhotos(){api("GET","/photos").then(d=>{photos=d.photos||[];renderGallery()})}
function renderGallery(){let d=document.getElementById("gallery");if(photos.length===0){d.innerHTML='<div class="col-span-full text-center text-on-surface-variant py-12"><i class="fa-solid fa-image text-4xl mb-2 block"></i>No photos yet</div>';return}
	d.innerHTML=photos.map(p=>'<div class="aspect-square bg-surface-container rounded-xl overflow-hidden cursor-pointer hover:opacity-90 transition-opacity relative group" onclick="openLightbox(\''+p.id+'\')">'+
		'<img src="/api/photos/thumb/'+getUserId()+'/'+p.id+'" alt="'+esc(p.name)+'" class="w-full h-full object-cover" loading="lazy">'+
		(p.favorited?'<div class="absolute top-2 right-2 text-red-400"><i class="fa-solid fa-heart text-sm"></i></div>':'')+
		'</div>').join("")}
function getUserId(){return "default"}

function openLightbox(id){currentPhoto=photos.find(p=>p.id===id);if(!currentPhoto)return;
	document.getElementById("lightbox-img").src="/api/photos/thumb/"+getUserId()+"/"+currentPhoto.id;
	document.getElementById("fav-btn").innerHTML=currentPhoto.favorited?'<i class="fa-solid fa-heart text-red-400"></i>':'<i class="fa-regular fa-heart"></i>';
	document.getElementById("lightbox").classList.remove("hidden")}
function closeLightbox(){document.getElementById("lightbox").classList.add("hidden")}
function toggleFav(){if(!currentPhoto)return;currentPhoto.favorited=!currentPhoto.favorited;api("PUT","/photo?id="+currentPhoto.id,{favorited:currentPhoto.favorited}).then(loadPhotos)}
function deletePhoto(){if(!currentPhoto||!confirm("Delete?"))return;api("DELETE","/photo?id="+currentPhoto.id).then(()=>{closeLightbox();loadPhotos()})}
function uploadPhotos(){let input=document.getElementById("upload-input");if(!input.files.length)return;let form=new FormData();for(let f of input.files)form.append("file",f);fetch(apiBase+"/upload",{method:"POST",body:form}).then(()=>{input.value="";loadPhotos()})}
function showNewAlbum(){let name=prompt("Album name:");if(!name)return;api("POST","/album",{name:name}).then(loadPhotos)}
function esc(s){let d=document.createElement("div");d.textContent=s;return d.innerHTML}
init();
</script>`
}
