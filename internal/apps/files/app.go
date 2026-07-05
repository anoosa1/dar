package files

import "github.com/anoosa/dar/internal/apps"

type App struct{}

func (a App) Name() string                         { return "files" }
func (a App) DisplayName() string                  { return "Files" }
func (a App) Icon() string                         { return `<i class="fa-solid fa-folder"></i>` }
func (a App) Description() string                  { return "File browser — upload, download, manage" }
func (a App) AppBarOrder() int                     { return 2 }
func (a App) DaemonConfig() *apps.DaemonConfig     { return nil }
func (a App) Routes() []apps.Route {
	return []apps.Route{
		{Method: "GET", Pattern: "/files", Handler: handlePage},
	}
}
