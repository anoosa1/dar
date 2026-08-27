package notes

import (
	"context"
	"fmt"
	"time"

	"github.com/anoosa/dar/internal/apps"
)

func (a *App) DashboardWidgets() []apps.WidgetDefinition {
	return []apps.WidgetDefinition{{ID: "notes.list", Title: "Notes", Icon: "fa-solid fa-note-sticky", Kind: "list", DefaultSize: apps.WidgetStandard, SupportedSizes: apps.AllDashboardWidgetSizes(), EmptyState: "No notes yet", Actions: []apps.WidgetAction{{ID: "new", Label: "New note", URL: "/notes/new", Icon: "fa-solid fa-plus"}}}}
}
func (a *App) DashboardWidget(ctx context.Context, request apps.DashboardWidgetRequest, id string) (apps.WidgetData, error) {
	if id != "notes.list" {
		return apps.WidgetData{}, fmt.Errorf("unknown widget")
	}
	notes, err := a.indexedNotes(ctx, request.UserID, false)
	if err != nil {
		return apps.WidgetData{}, err
	}
	items := []apps.WidgetItem{}
	limit := map[apps.WidgetSize]int{
		apps.WidgetCompact: 1, apps.WidgetStandard: 3, apps.WidgetWide: 6, apps.WidgetLarge: 10,
	}[request.Size]
	for _, n := range notes {
		items = append(items, apps.WidgetItem{ID: fmt.Sprint(n.ID), Title: n.Title, Subtitle: relativeTime(n.Modified), URL: fmt.Sprintf("/notes/note/%d", n.ID), Icon: "fa-regular fa-note-sticky", Favorite: n.Favorite})
		if len(items) == limit {
			break
		}
	}
	def := a.DashboardWidgets()[0]
	return apps.WidgetData{Kind: "list", Items: items, Actions: def.Actions, EmptyState: def.EmptyState}, nil
}
func relativeTime(unix int64) string {
	d := time.Since(time.Unix(unix, 0))
	switch {
	case d < time.Minute:
		return "Just now"
	case d < time.Hour:
		return fmt.Sprintf("%dm ago", int(d.Minutes()))
	case d < 24*time.Hour:
		return fmt.Sprintf("%dh ago", int(d.Hours()))
	default:
		return time.Unix(unix, 0).Format("Jan 2")
	}
}
