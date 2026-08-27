package prayer

import (
	"context"
	"fmt"
	"net/url"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/preferences"
	"zombiezen.com/go/sqlite"
)

const prayerWidgetID = "prayer.times"

func (a *App) DashboardWidgets() []apps.WidgetDefinition {
	return []apps.WidgetDefinition{{
		ID: prayerWidgetID, Title: "Prayer times", Icon: "fa-solid fa-mosque", Kind: "list",
		DefaultSize: apps.WidgetStandard, SupportedSizes: apps.AllDashboardWidgetSizes(),
		EmptyState: "Set a Prayer Times location to see today's prayers",
		Actions:    []apps.WidgetAction{{ID: "open", Label: "Open Prayer Times", URL: "/prayer", Icon: "fa-solid fa-arrow-up-right-from-square"}},
	}}
}

func (a *App) DashboardWidget(ctx context.Context, request apps.DashboardWidgetRequest, id string) (apps.WidgetData, error) {
	if id != prayerWidgetID {
		return apps.WidgetData{}, fmt.Errorf("unknown prayer widget")
	}
	definition := a.DashboardWidgets()[0]
	settings, err := a.loadSettings(ctx, request.UserID)
	if err != nil {
		return apps.WidgetData{}, err
	}
	if !settings.Configured {
		return apps.WidgetData{Kind: definition.Kind, Actions: definition.Actions, EmptyState: definition.EmptyState}, nil
	}
	loc, err := time.LoadLocation(settings.Timezone)
	if err != nil {
		return apps.WidgetData{}, fmt.Errorf("invalid prayer timezone")
	}
	now := a.now().In(loc)
	calculation := Calculation{
		Latitude: settings.Latitude, Longitude: settings.Longitude, Location: loc,
		Method: settings.Method, AsrSchool: settings.AsrSchool,
	}
	day := time.Date(now.Year(), now.Month(), now.Day(), 12, 0, 0, 0, loc)
	times, err := calculation.Calculate(day)
	if err != nil {
		return apps.WidgetData{}, fmt.Errorf("calculate prayer widget: %w", err)
	}
	prayers := trackedPrayers(times)
	date := day.Format("2006-01-02")
	start := 0
	for start < len(prayers) && prayers[start].Scheduled.Before(now) {
		start++
	}
	if start == len(prayers) {
		day = day.AddDate(0, 0, 1)
		times, err = calculation.Calculate(day)
		if err != nil {
			return apps.WidgetData{}, fmt.Errorf("calculate next prayer widget: %w", err)
		}
		prayers = trackedPrayers(times)
		date = day.Format("2006-01-02")
		start = 0
	}
	prayers = prayers[start:]
	limit := map[apps.WidgetSize]int{
		apps.WidgetCompact:  1,
		apps.WidgetStandard: 3,
		apps.WidgetWide:     5,
		apps.WidgetLarge:    5,
	}[request.Size]
	if limit == 0 {
		return apps.WidgetData{}, fmt.Errorf("unsupported prayer widget size")
	}
	if len(prayers) > limit {
		prayers = prayers[:limit]
	}
	format := "3:04 PM"
	var clockPreference struct {
		TimeFormat24h bool `json:"time_format_24h"`
	}
	if err := a.withConn(ctx, func(conn *sqlite.Conn) error {
		_, err := preferences.GetJSON(conn, request.UserID, "global", "time_format", &clockPreference)
		return err
	}); err != nil {
		return apps.WidgetData{}, err
	}
	if clockPreference.TimeFormat24h {
		format = "15:04"
	}
	items := make([]apps.WidgetItem, 0, len(prayers))
	for i, prayer := range prayers {
		title := prayer.Title
		subtitle := prayer.Scheduled.Format(format)
		if prayer.Scheduled.YearDay() != now.YearDay() || prayer.Scheduled.Year() != now.Year() {
			subtitle = "Tomorrow · " + subtitle
		}
		if i == 0 {
			title = "Next · " + title
			subtitle += " · " + prayerCountdown(now, prayer.Scheduled)
		}
		items = append(items, apps.WidgetItem{
			ID: date + ":" + prayer.Name, Title: title, Subtitle: subtitle,
			URL: "/prayer?date=" + url.QueryEscape(date), Icon: prayerWidgetIcon(prayer.Name),
		})
	}
	return apps.WidgetData{Kind: definition.Kind, Items: items, Actions: definition.Actions, EmptyState: definition.EmptyState}, nil
}

func prayerCountdown(now, scheduled time.Time) string {
	remaining := scheduled.Sub(now)
	if remaining <= 0 {
		return "Now"
	}
	minutes := int(remaining.Round(time.Minute) / time.Minute)
	if minutes < 60 {
		return fmt.Sprintf("in %dm", minutes)
	}
	return fmt.Sprintf("in %dh %dm", minutes/60, minutes%60)
}

func prayerWidgetIcon(name string) string {
	switch name {
	case "fajr", "isha":
		return "fa-solid fa-moon"
	case "dhuhr", "maghrib":
		return "fa-solid fa-sun"
	default:
		return "fa-solid fa-clock"
	}
}
