package calendar

import (
	"context"
	"fmt"
	"path/filepath"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
)

func dashboardTestApp(t *testing.T) *App {
	t.Helper()
	migrations := append([]string{}, preferences.Migrations()...)
	migrations = append(migrations, Migrations()...)
	database, err := db.Open(filepath.Join(t.TempDir(), "dashboard-calendar.sqlite"), migrations)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	return New(Config{DB: database, DataRoot: t.TempDir()})
}

func TestCalendarWidgetsUseSemanticLayouts(t *testing.T) {
	app := dashboardTestApp(t)
	app.ensureSQLDefault("alice")
	now := time.Now().UTC()
	for i := range 12 {
		start := now.Add(time.Duration(i+1) * time.Hour)
		if err := app.sqlWriteEvent("alice", Event{
			CalendarID: "default", ID: fmt.Sprintf("upcoming-%02d", i), Title: fmt.Sprintf("Upcoming %02d", i),
			Start: start.Unix(), End: start.Add(time.Hour).Unix(), Status: "CONFIRMED",
		}); err != nil {
			t.Fatal(err)
		}
	}
	for size, want := range map[apps.WidgetSize]int{
		apps.WidgetCompact:  1,
		apps.WidgetStandard: 3,
		apps.WidgetWide:     6,
	} {
		data, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: size}, "calendar.upcoming")
		if err != nil {
			t.Fatal(err)
		}
		if len(data.Items) != want {
			t.Fatalf("%s upcoming items = %d, want %d", size, len(data.Items), want)
		}
	}
	large, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: apps.WidgetLarge}, "calendar.upcoming")
	if err != nil {
		t.Fatal(err)
	}
	total := 0
	for _, section := range large.Sections {
		total += len(section.Items)
	}
	if large.Kind != "sections" || total != 10 {
		t.Fatalf("large upcoming = %#v", large)
	}
}

func TestOnThisDayWidgetUsesSemanticSectionLimits(t *testing.T) {
	app := dashboardTestApp(t)
	app.ensureSQLDefault("alice")
	now := time.Now().UTC()
	start := time.Date(now.Year(), now.Month(), now.Day(), 12, 0, 0, 0, time.UTC)
	for i := range 6 {
		today := start.Add(time.Duration(i) * time.Minute)
		if err := app.sqlWriteEvent("alice", Event{
			CalendarID: "default", ID: fmt.Sprintf("today-%02d", i), Title: fmt.Sprintf("Today %02d", i),
			Start: today.Unix(), End: today.Add(time.Hour).Unix(), Status: "CONFIRMED",
		}); err != nil {
			t.Fatal(err)
		}
		memory := start.AddDate(-(i + 1), 0, 0)
		if err := app.sqlWriteEvent("alice", Event{
			CalendarID: "default", ID: fmt.Sprintf("memory-%02d", i), Title: fmt.Sprintf("Memory %02d", i),
			Start: memory.Unix(), End: memory.Add(time.Hour).Unix(), Status: "CONFIRMED",
		}); err != nil {
			t.Fatal(err)
		}
	}
	compact, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: apps.WidgetCompact}, "calendar.on_this_day")
	if err != nil {
		t.Fatal(err)
	}
	if compact.Kind != "list" || len(compact.Items) != 1 || compact.Items[0].ID != "today-00" {
		t.Fatalf("compact on this day = %#v", compact)
	}
	for size, want := range map[apps.WidgetSize]int{
		apps.WidgetStandard: 1,
		apps.WidgetWide:     3,
		apps.WidgetLarge:    5,
	} {
		data, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: size}, "calendar.on_this_day")
		if err != nil {
			t.Fatal(err)
		}
		if len(data.Sections) != 2 || len(data.Sections[0].Items) != want || len(data.Sections[1].Items) != want {
			t.Fatalf("%s on this day = %#v", size, data.Sections)
		}
	}
}

func TestCalendarDashboardWidgetsFilterAndGroup(t *testing.T) {
	app := dashboardTestApp(t)
	app.ensureSQLDefault("alice")
	now := time.Now().UTC()
	if err := app.sqlWriteEvent("alice", Event{CalendarID: "default", ID: "upcoming", Title: "Planning", Start: now.Add(time.Hour).Unix(), End: now.Add(2 * time.Hour).Unix(), Status: "CONFIRMED"}); err != nil {
		t.Fatal(err)
	}
	if err := app.sqlWriteEvent("alice", Event{CalendarID: "default", ID: "cancelled", Title: "Cancelled", Start: now.Add(time.Hour).Unix(), End: now.Add(2 * time.Hour).Unix(), Status: "CANCELLED"}); err != nil {
		t.Fatal(err)
	}
	lastYear := now.AddDate(-1, 0, 0)
	if err := app.sqlWriteEvent("alice", Event{CalendarID: "default", ID: "memory", Title: "Last year", Start: lastYear.Unix(), End: lastYear.Add(time.Hour).Unix(), Status: "CONFIRMED"}); err != nil {
		t.Fatal(err)
	}
	request := apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: apps.WidgetStandard}
	upcoming, err := app.DashboardWidget(context.Background(), request, "calendar.upcoming")
	if err != nil {
		t.Fatal(err)
	}
	if len(upcoming.Items) != 1 || upcoming.Items[0].ID != "upcoming" {
		t.Fatalf("upcoming = %#v", upcoming.Items)
	}
	onThisDay, err := app.DashboardWidget(context.Background(), request, "calendar.on_this_day")
	if err != nil {
		t.Fatal(err)
	}
	if len(onThisDay.Sections) != 2 || len(onThisDay.Sections[1].Items) != 1 || onThisDay.Sections[1].Items[0].ID != "memory" {
		t.Fatalf("on this day = %#v", onThisDay.Sections)
	}
}
