// Package apps defines the App interface and registry for Dar apps.
package apps

import (
	"html/template"
	"net/http"
	"sort"
)

// DaemonConfig describes a daemon process to spawn and monitor.
type DaemonConfig struct {
	Command          string
	Args             []string
	Port             int
	HealthPath       string
	StartupTimeoutMs int
}

// Route is an HTTP route registered by an app on the main server.
type Route struct {
	Method  string
	Pattern string
	Handler http.HandlerFunc
}

// App is the interface that every Dar app must implement.
type App interface {
	Name() string
	DisplayName() string
	Icon() string
	Description() string
	AppBarOrder() int
	DaemonConfig() *DaemonConfig // nil if no daemon
	Routes() []Route
}

// Registry collects and orders apps for the app bar and home screen.
type Registry struct {
	apps []App
}

// NewRegistry creates a new app registry.
func NewRegistry() *Registry {
	return &Registry{}
}

// Register adds an app, inserting it in AppBarOrder-sorted position.
func (r *Registry) Register(app App) {
	idx := sort.Search(len(r.apps), func(i int) bool {
		return r.apps[i].AppBarOrder() >= app.AppBarOrder()
	})
	r.apps = append(r.apps, nil)
	copy(r.apps[idx+1:], r.apps[idx:])
	r.apps[idx] = app
}

// Apps returns all registered apps in AppBarOrder.
func (r *Registry) Apps() []App {
	return r.apps
}

// AppBarHTML returns the HTML for the top app bar navigation icons.
func (r *Registry) AppBarHTML() template.HTML {
	html := `<nav id="dar-app-bar" class="flex items-center gap-1">`
	for _, app := range r.apps {
		html += `<a href="/` + app.Name() + `" aria-label="` + app.DisplayName() + `" title="` + app.DisplayName() + `"
			class="w-10 h-10 rounded-full inline-flex items-center justify-center text-[20px] text-on-surface-variant hover:bg-surface-container-highest no-underline transition-colors">
			<span aria-hidden="true">` + app.Icon() + `</span></a>`
	}
	html += `</nav>`
	return template.HTML(html)
}

// HomeCardsHTML returns the HTML for the home screen app launcher grid.
func (r *Registry) HomeCardsHTML() template.HTML {
	if len(r.apps) == 0 {
		return template.HTML(`<p class="text-on-surface-variant">No apps installed yet.</p>`)
	}
	html := `<div class="grid grid-cols-2 md:grid-cols-4 gap-4 max-w-2xl mx-auto">`
	for _, app := range r.apps {
		html += `<a href="/` + app.Name() + `"
			class="bg-surface-container rounded-md shadow-1 hover:shadow-2 transition-shadow p-6 text-center no-underline text-on-surface">
			<div class="text-[40px] mb-3">` + app.Icon() + `</div>
			<div class="font-medium text-base">` + app.DisplayName() + `</div>
			<div class="text-on-surface-variant text-sm mt-1">` + app.Description() + `</div>
		</a>`
	}
	html += `</div>`
	return template.HTML(html)
}
