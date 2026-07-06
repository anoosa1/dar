package todo

import "github.com/anoosa/dar/internal/apps"

type App struct{}

func (a App) Name() string                     { return "todo" }
func (a App) DisplayName() string              { return "Todo" }
func (a App) Icon() string                     { return `<i class="fa-solid fa-check-double"></i>` }
func (a App) Description() string              { return "Task lists and to-dos" }
func (a App) AppBarOrder() int                 { return 5 }
func (a App) DaemonConfig() *apps.DaemonConfig { return nil }
func (a App) Routes() []apps.Route {
	return []apps.Route{
		{Method: "GET", Pattern: "/todo", Handler: handlePage},
	}
}
