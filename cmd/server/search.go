package main

import (
	"net/http"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
)

func handleSearchPage(_ *server.Server, registry *apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.Redirect(w, r, "/login", http.StatusSeeOther)
			return
		}
		if err := webui.RenderRequest(w, r, webui.PageData{
			Title:           "Search — Dar",
			CurrentApp:      "search",
			AppBar:          registry.AppBarHTML(),
			UserMenu:        webui.UserMenuHTML(user.DisplayName),
			ContentTemplate: "search.html",
		}); err != nil {
			http.Error(w, "Internal server error", http.StatusInternalServerError)
		}
	}
}
