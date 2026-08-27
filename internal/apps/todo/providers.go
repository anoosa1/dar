package todo

import (
	"context"
	"fmt"
	"net/url"
	"sort"
	"time"

	"github.com/anoosa/dar/internal/apps"
)

func (a *App) DashboardWidgets() []apps.WidgetDefinition {
	return []apps.WidgetDefinition{{
		ID: "todo.upcoming", Title: "Upcoming tasks", Icon: "fa-solid fa-list-check", Kind: "list",
		DefaultSize: apps.WidgetStandard, SupportedSizes: apps.AllDashboardWidgetSizes(),
		EmptyState: "No tasks due in the next 14 days",
		Actions:    []apps.WidgetAction{{ID: "open", Label: "Open Todo", URL: "/todo?collection=week", Icon: "fa-solid fa-arrow-up-right-from-square"}},
	}}
}

func (a *App) DashboardWidget(_ context.Context, request apps.DashboardWidgetRequest, id string) (apps.WidgetData, error) {
	if id != "todo.upcoming" {
		return apps.WidgetData{}, fmt.Errorf("unknown todo widget")
	}
	loc, err := time.LoadLocation(request.Timezone)
	if err != nil {
		return apps.WidgetData{}, fmt.Errorf("invalid timezone")
	}
	tasks, err := a.tasks(request.UserID, "", "all", false)
	if err != nil {
		return apps.WidgetData{}, err
	}
	lists, err := a.lists(request.UserID, false)
	if err != nil {
		return apps.WidgetData{}, err
	}
	names := make(map[string]string, len(lists))
	for _, list := range lists {
		names[list.ID] = list.Name
	}
	now := time.Now()
	localNow := now.In(loc)
	start := time.Date(localNow.Year(), localNow.Month(), localNow.Day(), 0, 0, 0, 0, loc)
	end := start.AddDate(0, 0, 14)
	filtered := tasks[:0]
	for _, task := range tasks {
		if task.DueAt == 0 || task.DueAt >= end.Unix() || task.Status == "COMPLETED" || task.Status == "CANCELLED" {
			continue
		}
		filtered = append(filtered, task)
	}
	sort.SliceStable(filtered, func(i, j int) bool {
		leftOverdue, rightOverdue := filtered[i].DueAt < now.Unix(), filtered[j].DueAt < now.Unix()
		if leftOverdue != rightOverdue {
			return leftOverdue
		}
		if filtered[i].DueAt == filtered[j].DueAt {
			return filtered[i].Priority > filtered[j].Priority
		}
		return filtered[i].DueAt < filtered[j].DueAt
	})
	limit := map[apps.WidgetSize]int{
		apps.WidgetCompact: 1, apps.WidgetStandard: 3, apps.WidgetWide: 6, apps.WidgetLarge: 10,
	}[request.Size]
	if len(filtered) > limit {
		filtered = filtered[:limit]
	}
	items := make([]apps.WidgetItem, 0, len(filtered))
	for _, task := range filtered {
		due := time.Unix(task.DueAt, 0).In(loc)
		label := due.Format("Mon, Jan 2 · 3:04 PM")
		if task.DueAt < now.Unix() {
			label = "Overdue · " + label
		}
		if name := names[task.ListID]; name != "" {
			label += " · " + name
		}
		items = append(items, apps.WidgetItem{
			ID: task.ID, Title: task.Title, Subtitle: label,
			URL:  "/todo?list_id=" + url.QueryEscape(task.ListID) + "#task-" + url.PathEscape(task.ID),
			Icon: "fa-regular fa-circle-check", Favorite: task.Starred,
		})
	}
	def := a.DashboardWidgets()[0]
	if request.Size == apps.WidgetLarge {
		overdue := make([]apps.WidgetItem, 0, len(items))
		upcoming := make([]apps.WidgetItem, 0, len(items))
		for i, task := range filtered {
			if task.DueAt < now.Unix() {
				overdue = append(overdue, items[i])
			} else {
				upcoming = append(upcoming, items[i])
			}
		}
		return apps.WidgetData{
			Kind: "sections",
			Sections: []apps.WidgetSection{
				{Title: "Overdue", Items: overdue},
				{Title: "Upcoming", Items: upcoming},
			},
			Actions: def.Actions, EmptyState: def.EmptyState,
		}, nil
	}
	return apps.WidgetData{Kind: def.Kind, Items: items, Actions: def.Actions, EmptyState: def.EmptyState}, nil
}
