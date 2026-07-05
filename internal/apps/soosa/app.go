// Package soosa provides the Soosa AI chat web UI for Dar.
package soosa

import (
	"github.com/anoosa/dar/internal/apps"
)

// App implements the apps.App interface for Soosa.
type App struct{}

func (a App) Name() string             { return "soosa" }
func (a App) DisplayName() string      { return "Soosa" }
func (a App) Icon() string             { return "🧠" }
func (a App) Description() string      { return "AI assistant — chat with your data" }
func (a App) AppBarOrder() int         { return 1 }
func (a App) DaemonConfig() *apps.DaemonConfig { return nil } // daemon started separately
func (a App) Routes() []apps.Route {
	return []apps.Route{
		{Method: "GET", Pattern: "/soosa", Handler: handlePage},
		{Method: "GET", Pattern: "/soosa/health", Handler: handleHealth},
	}
}
