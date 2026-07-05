package contacts

import "github.com/anoosa/dar/internal/apps"

type App struct{}

func (a App) Name() string                     { return "contacts" }
func (a App) DisplayName() string              { return "Contacts" }
func (a App) Icon() string                     { return `<i class="fa-solid fa-address-book"></i>` }
func (a App) Description() string              { return "Manage your contacts" }
func (a App) AppBarOrder() int                 { return 3 }
func (a App) DaemonConfig() *apps.DaemonConfig { return nil }
func (a App) Routes() []apps.Route {
	return []apps.Route{
		{Method: "GET", Pattern: "/contacts", Handler: handlePage},
	}
}
