package calendar

import (
	"context"
	"fmt"
	"net/url"
	"sort"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/preferences"
)

type dashboardCalendarSettings struct {
	Timezone         string `json:"timezone"`
	BirthdayCalendar bool   `json:"birthday_calendar"`
}

func (a *App) DashboardWidgets() []apps.WidgetDefinition {
	return []apps.WidgetDefinition{
		{ID: "calendar.upcoming", Title: "Upcoming events", Icon: "fa-solid fa-calendar-days", Kind: "list", DefaultSize: apps.WidgetStandard, SupportedSizes: apps.AllDashboardWidgetSizes(), EmptyState: "No events in the next 14 days", Actions: []apps.WidgetAction{{ID: "open", Label: "Open Calendar", URL: "/calendar?view=list", Icon: "fa-solid fa-arrow-up-right-from-square"}}},
		{ID: "calendar.on_this_day", Title: "On this day", Icon: "fa-solid fa-clock-rotate-left", Kind: "sections", DefaultSize: apps.WidgetStandard, SupportedSizes: apps.AllDashboardWidgetSizes(), EmptyState: "Nothing on this day", Actions: []apps.WidgetAction{{ID: "open", Label: "Open Calendar", URL: "/calendar", Icon: "fa-solid fa-arrow-up-right-from-square"}}},
	}
}

func (a *App) DashboardWidget(ctx context.Context, request apps.DashboardWidgetRequest, id string) (apps.WidgetData, error) {
	settings := dashboardCalendarSettings{Timezone: "automatic", BirthdayCalendar: true}
	if a.db != nil {
		conn, release, err := a.db.Conn(ctx)
		if err != nil {
			return apps.WidgetData{}, err
		}
		_, err = preferences.GetJSON(conn, request.UserID, "calendar", "settings", &settings)
		release()
		if err != nil {
			return apps.WidgetData{}, err
		}
	}
	timezone := request.Timezone
	if settings.Timezone != "" && settings.Timezone != "automatic" {
		timezone = settings.Timezone
	}
	loc, err := time.LoadLocation(timezone)
	if err != nil {
		return apps.WidgetData{}, fmt.Errorf("invalid timezone")
	}
	visible, names := a.dashboardCalendars(request.UserID, settings.BirthdayCalendar)
	switch id {
	case "calendar.upcoming":
		return a.upcomingWidget(request, loc, visible, names), nil
	case "calendar.on_this_day":
		return a.onThisDayWidget(request, loc, visible, names), nil
	default:
		return apps.WidgetData{}, fmt.Errorf("unknown calendar widget")
	}
}

func (a *App) dashboardCalendars(uid string, birthdays bool) (map[string]bool, map[string]string) {
	visible := map[string]bool{}
	names := map[string]string{"birthdays": "Birthdays"}
	for _, calendar := range a.listCalendarMetas(uid) {
		visible[calendar.ID] = calendar.Visible && (calendar.ID != "birthdays" || birthdays)
		names[calendar.ID] = calendar.Name
	}
	if birthdays {
		visible["birthdays"] = true
	}
	return visible, names
}

func (a *App) upcomingWidget(request apps.DashboardWidgetRequest, loc *time.Location, visible map[string]bool, names map[string]string) apps.WidgetData {
	now := time.Now()
	end := now.Add(14 * 24 * time.Hour)
	events := a.expandInRange(a.listAllEventsInRange(request.UserID, now.Unix(), end.Unix()), now.Unix(), end.Unix())
	events = filterDashboardEvents(events, visible)
	sort.SliceStable(events, func(i, j int) bool {
		if events[i].Start == events[j].Start {
			return strings.ToLower(events[i].Title) < strings.ToLower(events[j].Title)
		}
		return events[i].Start < events[j].Start
	})
	limit := map[apps.WidgetSize]int{
		apps.WidgetCompact: 1, apps.WidgetStandard: 3, apps.WidgetWide: 6, apps.WidgetLarge: 10,
	}[request.Size]
	if len(events) > limit {
		events = events[:limit]
	}
	def := a.DashboardWidgets()[0]
	items := dashboardEventItems(events, loc, names, false)
	if request.Size == apps.WidgetLarge {
		sections := make([]apps.WidgetSection, 0, len(events))
		for i, event := range events {
			title := dashboardDayLabel(time.Unix(event.Start, 0).In(loc))
			if len(sections) == 0 || sections[len(sections)-1].Title != title {
				sections = append(sections, apps.WidgetSection{Title: title})
			}
			sections[len(sections)-1].Items = append(sections[len(sections)-1].Items, items[i])
		}
		return apps.WidgetData{Kind: "sections", Sections: sections, Actions: def.Actions, EmptyState: def.EmptyState}
	}
	return apps.WidgetData{Kind: def.Kind, Items: items, Actions: def.Actions, EmptyState: def.EmptyState}
}

func dashboardDayLabel(day time.Time) string {
	now := time.Now().In(day.Location())
	today := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, day.Location())
	value := time.Date(day.Year(), day.Month(), day.Day(), 0, 0, 0, 0, day.Location())
	switch {
	case value.Equal(today):
		return "Today"
	case value.Equal(today.AddDate(0, 0, 1)):
		return "Tomorrow"
	default:
		return value.Format("Monday, Jan 2")
	}
}

func (a *App) onThisDayWidget(request apps.DashboardWidgetRequest, loc *time.Location, visible map[string]bool, names map[string]string) apps.WidgetData {
	now := time.Now().In(loc)
	start := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, loc)
	end := start.AddDate(0, 0, 1)
	today := a.expandInRange(a.listAllEventsInRange(request.UserID, start.Unix(), end.Unix()), start.Unix(), end.Unix())
	today = filterDashboardEvents(today, visible)
	sort.SliceStable(today, func(i, j int) bool { return today[i].Start < today[j].Start })

	var memories []Event
	for _, event := range a.listAllEvents(request.UserID) {
		if event.RRule != "" || !visible[event.CalendarID] || strings.EqualFold(event.Status, "CANCELLED") {
			continue
		}
		when := time.Unix(event.Start, 0).In(loc)
		if when.Year() < now.Year() && when.Month() == now.Month() && when.Day() == now.Day() {
			memories = append(memories, event)
		}
	}
	sort.SliceStable(memories, func(i, j int) bool {
		left, right := time.Unix(memories[i].Start, 0).In(loc), time.Unix(memories[j].Start, 0).In(loc)
		if left.Year() == right.Year() {
			return memories[i].Start > memories[j].Start
		}
		return left.Year() > right.Year()
	})
	def := a.DashboardWidgets()[1]
	if request.Size == apps.WidgetCompact {
		var items []apps.WidgetItem
		if len(today) > 0 {
			items = dashboardEventItems(today[:1], loc, names, false)
		} else if len(memories) > 0 {
			items = dashboardEventItems(memories[:1], loc, names, true)
		}
		return apps.WidgetData{Kind: "list", Items: items, Actions: def.Actions, EmptyState: def.EmptyState}
	}
	limit := map[apps.WidgetSize]int{
		apps.WidgetStandard: 1, apps.WidgetWide: 3, apps.WidgetLarge: 5,
	}[request.Size]
	if len(today) > limit {
		today = today[:limit]
	}
	if len(memories) > limit {
		memories = memories[:limit]
	}
	return apps.WidgetData{
		Kind: def.Kind,
		Sections: []apps.WidgetSection{
			{Title: "Today", Items: dashboardEventItems(today, loc, names, false)},
			{Title: "Previous years", Items: dashboardEventItems(memories, loc, names, true)},
		},
		Actions: def.Actions, EmptyState: def.EmptyState,
	}
}

func filterDashboardEvents(events []Event, visible map[string]bool) []Event {
	out := events[:0]
	for _, event := range events {
		if visible[event.CalendarID] && !strings.EqualFold(event.Status, "CANCELLED") {
			out = append(out, event)
		}
	}
	return out
}

func dashboardEventItems(events []Event, loc *time.Location, names map[string]string, includeYear bool) []apps.WidgetItem {
	items := make([]apps.WidgetItem, 0, len(events))
	for _, event := range events {
		start := time.Unix(event.Start, 0).In(loc)
		date := start.Format("Mon, Jan 2")
		if includeYear {
			date = start.Format("2006")
		} else if !event.AllDay {
			date += " · " + start.Format("3:04 PM")
		}
		if calendarName := names[event.CalendarID]; calendarName != "" {
			date += " · " + calendarName
		}
		itemURL := "/calendar?date=" + url.QueryEscape(start.Format("2006-01-02")) + "#event-" + url.PathEscape(event.ID)
		items = append(items, apps.WidgetItem{ID: event.ID, Title: event.Title, Subtitle: date, URL: itemURL, Icon: "fa-regular fa-calendar"})
	}
	return items
}
