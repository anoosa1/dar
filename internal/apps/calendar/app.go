package calendar

import "github.com/anoosa/dar/internal/apps"

type App struct{}

func (a App) Name() string                     { return "calendar" }
func (a App) DisplayName() string              { return "Calendar" }
func (a App) Icon() string                     { return `<i class="fa-solid fa-calendar"></i>` }
func (a App) Description() string              { return "Manage your schedule" }
func (a App) AppBarOrder() int                 { return 4 }
func (a App) DaemonConfig() *apps.DaemonConfig { return nil }
func (a App) Routes() []apps.Route {
	return []apps.Route{
		{Method: "GET", Pattern: "/calendar", Handler: handlePage},
	}
}
