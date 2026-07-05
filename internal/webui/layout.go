// Package webui provides HTML layout rendering and reusable components for Dar.
package webui

import (
	"embed"
	"html/template"
	"io"
)

//go:embed templates/*
var templateFS embed.FS

var templates = template.Must(template.New("").ParseFS(templateFS, "templates/*.html"))

// PageData is passed to the layout template for every page.
type PageData struct {
	Title     string
	AppBar    template.HTML
	UserMenu  template.HTML
	Sidebar   template.HTML
	Content   template.HTML
	HeadExtra template.HTML
}

// Render renders a page using the base layout template.
func Render(w io.Writer, data PageData) error {
	return templates.ExecuteTemplate(w, "layout.html", data)
}

// RenderLogin renders the login page.
func RenderLogin(w io.Writer, providers []LoginProvider) error {
	return templates.ExecuteTemplate(w, "login.html", map[string]any{
		"Title":     "Login — Dar",
		"Providers": providers,
	})
}

// RenderHome renders the home/app launcher page.
func RenderHome(w io.Writer, userName string, appCards, appBar template.HTML) error {
	return Render(w, PageData{
		Title:    "Home — Dar",
		AppBar:   appBar,
		UserMenu: buildUserMenu(userName),
		Content:  appCards,
	})
}

// LoginProvider represents an OIDC provider shown on the login page.
type LoginProvider struct {
	Name string
	Icon string
	URL  string
}

func buildUserMenu(displayName string) template.HTML {
	initial := "U"
	if len(displayName) > 0 {
		initial = displayName[:1]
	}
	return template.HTML(`<div class="relative">
		<button id="user-menu-button" type="button" onclick="toggleUserMenu()"
			class="w-10 h-10 rounded-full bg-primary-container text-primary inline-flex items-center justify-center font-medium border-0 cursor-pointer"
			aria-label="User menu">
			<span aria-hidden="true">` + initial + `</span>
		</button>
		<div id="user-menu" class="hidden absolute right-0 mt-2 w-48 bg-surface-container rounded-md shadow-2 border border-outline-variant py-2 z-30">
			<div class="px-4 py-2 text-sm font-medium text-on-surface">` + displayName + `</div>
			<a href="/settings" class="block px-4 py-2 text-sm text-on-surface no-underline hover:bg-surface-container-highest">Settings</a>
			<a href="/logout" class="block px-4 py-2 text-sm text-error no-underline hover:bg-error-container">Logout</a>
		</div>
	</div>
	<script>
	function toggleUserMenu(){let m=document.getElementById('user-menu');if(m)m.classList.toggle('hidden')}
	document.addEventListener('click',function(e){let b=document.getElementById('user-menu-button');let m=document.getElementById('user-menu');if(b&&m&&!b.contains(e.target)&&!m.contains(e.target))m.classList.add('hidden')});
	</script>`)
}
