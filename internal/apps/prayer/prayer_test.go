package prayer

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/apps/todo"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/geocode"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
	"zombiezen.com/go/sqlite"
)

func toronto(t *testing.T) *time.Location {
	t.Helper()
	loc, err := time.LoadLocation("America/Toronto")
	if err != nil {
		t.Fatal(err)
	}
	return loc
}

func TestCalculationFixturesMethodsSchoolsAndMidnight(t *testing.T) {
	loc := toronto(t)
	date := time.Date(2026, 8, 12, 12, 0, 0, 0, loc)
	base := Calculation{Latitude: 43.6532, Longitude: -79.3832, Location: loc, Method: MethodMWL, AsrSchool: AsrStandard}
	values, err := base.Calculate(date)
	if err != nil {
		t.Fatal(err)
	}
	for name, fixture := range map[string]struct {
		got  time.Time
		hour int
		min  int
	}{
		"fajr": {values.Fajr, 4, 30}, "sunrise": {values.Sunrise, 6, 18}, "dhuhr": {values.Dhuhr, 13, 20},
		"asr": {values.Asr, 17, 13}, "maghrib": {values.Maghrib, 20, 21}, "isha": {values.Isha, 22, 2},
	} {
		want := time.Date(2026, 8, 12, fixture.hour, fixture.min, 0, 0, loc)
		if delta := fixture.got.Sub(want); delta < -12*time.Minute || delta > 12*time.Minute {
			t.Errorf("%s = %s, want near %s", name, fixture.got, want)
		}
	}
	if !values.Midnight.After(values.Isha) || values.Midnight.Day() != 13 {
		t.Fatalf("midnight = %s, want next day after Isha", values.Midnight)
	}
	hanafi := base
	hanafi.AsrSchool = AsrHanafi
	hanafiValues, err := hanafi.Calculate(date)
	if err != nil || !hanafiValues.Asr.After(values.Asr.Add(45*time.Minute)) {
		t.Fatalf("hanafi Asr = %s, standard = %s, err=%v", hanafiValues.Asr, values.Asr, err)
	}
	for _, method := range []Method{MethodMWL, MethodISNA, MethodEgyptian, MethodKarachi} {
		calculation := base
		calculation.Method = method
		got, err := calculation.Calculate(date)
		if err != nil || !got.Fajr.Before(got.Sunrise) || !got.Isha.After(got.Maghrib) {
			t.Errorf("method %s produced invalid ordering: %+v err=%v", method, got, err)
		}
	}
}

func TestCalculationDSTHighLatitudeAndPolarUnavailable(t *testing.T) {
	loc := toronto(t)
	calculation := Calculation{Latitude: 43.6532, Longitude: -79.3832, Location: loc, Method: MethodMWL, AsrSchool: AsrStandard}
	before, err := calculation.Calculate(time.Date(2026, 3, 7, 12, 0, 0, 0, loc))
	if err != nil {
		t.Fatal(err)
	}
	after, err := calculation.Calculate(time.Date(2026, 3, 9, 12, 0, 0, 0, loc))
	if err != nil {
		t.Fatal(err)
	}
	_, beforeOffset := before.Dhuhr.Zone()
	_, afterOffset := after.Dhuhr.Zone()
	if beforeOffset == afterOffset {
		t.Fatal("DST offset did not change across transition")
	}
	helsinki, _ := time.LoadLocation("Europe/Helsinki")
	high := Calculation{Latitude: 60.1699, Longitude: 24.9384, Location: helsinki, Method: MethodMWL, AsrSchool: AsrStandard}
	highValues, err := high.Calculate(time.Date(2026, 6, 20, 12, 0, 0, 0, helsinki))
	if err != nil || !highValues.Fajr.Before(highValues.Sunrise) || !highValues.Isha.After(highValues.Maghrib) {
		t.Fatalf("high-latitude adjustment = %+v err=%v", highValues, err)
	}
	tromso, _ := time.LoadLocation("Europe/Oslo")
	polar := Calculation{Latitude: 69.6492, Longitude: 18.9553, Location: tromso, Method: MethodMWL, AsrSchool: AsrStandard}
	if _, err := polar.Calculate(time.Date(2026, 6, 20, 12, 0, 0, 0, tromso)); err == nil {
		t.Fatal("polar day unexpectedly produced prayer times")
	}
}

func prayerTestDB(t *testing.T) *db.DB {
	t.Helper()
	migrations := append([]string{}, auth.Migrations()...)
	migrations = append(migrations, preferences.Migrations()...)
	migrations = append(migrations, todo.Migrations()...)
	migrations = append(migrations, todo.GeneratedTaskMigrations()...)
	migrations = append(migrations, Migrations()...)
	database, err := db.Open(filepath.Join(t.TempDir(), "prayer.sqlite"), migrations)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	return database
}

func TestTodoSyncIsIdempotentAndDisableRemovesFuture(t *testing.T) {
	database := prayerTestDB(t)
	todoApp := todo.New(todo.Config{DB: database})
	loc := toronto(t)
	now := time.Date(2026, 8, 12, 0, 1, 0, 0, loc)
	app := New(Config{DB: database, Todo: todoApp, Now: func() time.Time { return now }})
	settings := Settings{Location: "Toronto", LocationLabel: "Toronto, Ontario, Canada", Latitude: 43.6532, Longitude: -79.3832, Timezone: loc.String(), Method: MethodMWL, AsrSchool: AsrStandard, TodoSync: true, TrackingStartedAt: now.Unix(), Configured: true}
	if err := app.saveSettings(context.Background(), "alice", settings); err != nil {
		t.Fatal(err)
	}
	if err := app.syncUser(context.Background(), "alice"); err != nil {
		t.Fatal(err)
	}
	states, err := todoApp.GeneratedTaskStates(context.Background(), "alice", sourceName)
	if err != nil || len(states) != 40 {
		t.Fatalf("first sync states=%d err=%v", len(states), err)
	}
	if err := app.syncUser(context.Background(), "alice"); err != nil {
		t.Fatal(err)
	}
	states, _ = todoApp.GeneratedTaskStates(context.Background(), "alice", sourceName)
	if len(states) != 40 {
		t.Fatalf("second sync duplicated tasks: %d", len(states))
	}
	occurrences, err := app.occurrences(context.Background(), "alice")
	if err != nil || len(occurrences) != 40 {
		t.Fatalf("occurrences=%d err=%v", len(occurrences), err)
	}
	if err := app.disableSync(context.Background(), "alice"); err != nil {
		t.Fatal(err)
	}
	occurrences, _ = app.occurrences(context.Background(), "alice")
	if len(occurrences) != 0 {
		t.Fatalf("future occurrences remain after disable: %d", len(occurrences))
	}
	states, _ = todoApp.GeneratedTaskStates(context.Background(), "alice", sourceName)
	if len(states) != 0 {
		t.Fatalf("generated task mappings remain after disable: %d", len(states))
	}
}

type fakeTodo struct {
	states map[string]todo.GeneratedTaskState
}

func (f *fakeTodo) ReconcileGeneratedTasks(context.Context, string, string, string, []todo.GeneratedTask) (map[string]todo.GeneratedTaskState, error) {
	return f.states, nil
}
func (f *fakeTodo) GeneratedTaskStates(context.Context, string, string) (map[string]todo.GeneratedTaskState, error) {
	return f.states, nil
}
func (f *fakeTodo) RemoveGeneratedTasks(_ context.Context, _, _ string, ids []string) ([]string, error) {
	return ids, nil
}

func TestAnalyticsClassifiesWindowsMissedPendingAndReopened(t *testing.T) {
	database := prayerTestDB(t)
	now := time.Unix(10_000, 0)
	fake := &fakeTodo{states: map[string]todo.GeneratedTaskState{
		"d:fajr":    {ExternalID: "d:fajr", Status: "COMPLETED", CompletedAt: 1500},
		"d:dhuhr":   {ExternalID: "d:dhuhr", Status: "COMPLETED", CompletedAt: 3500},
		"d:maghrib": {ExternalID: "d:maghrib", Status: "NEEDS-ACTION", CompletedAt: 7500},
	}}
	app := New(Config{DB: database, Todo: fake, Now: func() time.Time { return now }})
	_ = app.saveSettings(context.Background(), "alice", Settings{Method: MethodMWL, AsrSchool: AsrStandard, TrackingStartedAt: 1000})
	values := []occurrence{
		{LocalDate: "d", Prayer: "fajr", ExternalID: "d:fajr", Scheduled: 1000, WindowEnd: 2000},
		{LocalDate: "d", Prayer: "dhuhr", ExternalID: "d:dhuhr", Scheduled: 2000, WindowEnd: 3000},
		{LocalDate: "d", Prayer: "asr", ExternalID: "d:asr", Scheduled: 4000, WindowEnd: 5000},
		{LocalDate: "d", Prayer: "maghrib", ExternalID: "d:maghrib", Scheduled: 7000, WindowEnd: 9000},
		{LocalDate: "d", Prayer: "isha", ExternalID: "d:isha", Scheduled: 11000, WindowEnd: 12000},
	}
	if err := app.upsertOccurrences(context.Background(), "alice", values); err != nil {
		t.Fatal(err)
	}
	req := httptest.NewRequest(http.MethodGet, "/api/prayer/analytics", nil)
	req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: "alice"}))
	rec := httptest.NewRecorder()
	app.handleAnalytics(rec, req)
	var response analyticsResponse
	if err := json.Unmarshal(rec.Body.Bytes(), &response); err != nil {
		t.Fatal(err)
	}
	if response.Totals.OnTime != 1 || response.Totals.OffTime != 1 || response.Totals.Missed != 2 || response.Totals.Pending != 1 || response.CompletionRate != 0.5 {
		t.Fatalf("analytics = %+v", response)
	}
}

func TestSettingsAPISeedsProfileResolvesLocationAndSyncs(t *testing.T) {
	geoServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte(`{"results":[{"name":"Toronto","admin1":"Ontario","country":"Canada","latitude":43.6532,"longitude":-79.3832,"timezone":"America/Toronto"}]}`))
	}))
	defer geoServer.Close()
	database := prayerTestDB(t)
	fake := &fakeTodo{states: map[string]todo.GeneratedTaskState{}}
	app := New(Config{DB: database, Todo: fake, Geocoder: geocode.New(geoServer.Client(), geoServer.URL), Now: func() time.Time { return time.Date(2026, 8, 12, 0, 1, 0, 0, toronto(t)) }})
	user := &auth.User{ID: "alice", Profile: auth.UserProfile{Location: "Toronto, Canada"}}
	get := httptest.NewRequest(http.MethodGet, "/api/prayer/settings", nil)
	get = get.WithContext(context.WithValue(get.Context(), server.UserContextKey, user))
	rec := httptest.NewRecorder()
	app.handleSettings(rec, get)
	if !strings.Contains(rec.Body.String(), "Toronto, Canada") {
		t.Fatalf("seeded settings = %s", rec.Body.String())
	}
	body := strings.NewReader(`{"location":"Toronto, Canada","method":"mwl","asr_school":"standard","todo_sync":true}`)
	put := httptest.NewRequest(http.MethodPut, "/api/prayer/settings", body)
	put = put.WithContext(context.WithValue(put.Context(), server.UserContextKey, user))
	rec = httptest.NewRecorder()
	app.handleSettings(rec, put)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"configured":true`) {
		t.Fatalf("saved settings status=%d body=%s", rec.Code, rec.Body.String())
	}
}

func TestRenderedQuietTimelineAndLifecycleShutdown(t *testing.T) {
	html := prayerHTML()
	for _, marker := range []string{"next-prayer-card", "prayer-timeline", "calendar-grid", "prayer-settings-modal", "analytics-breakdown"} {
		if !strings.Contains(html, marker) {
			t.Errorf("missing %s", marker)
		}
	}
	app := New(Config{DB: prayerTestDB(t), Todo: &fakeTodo{}, SyncInterval: time.Hour})
	ctx, cancel := context.WithCancel(context.Background())
	if err := app.Start(ctx); err != nil {
		t.Fatal(err)
	}
	cancel()
	shutdownCtx, shutdownCancel := context.WithTimeout(context.Background(), time.Second)
	defer shutdownCancel()
	if err := app.Shutdown(shutdownCtx); err != nil {
		t.Fatal(err)
	}
}

func TestPrayerSidebarDoesNotDuplicateCalendar(t *testing.T) {
	request := httptest.NewRequest(http.MethodGet, "/prayer", nil)
	var rendered strings.Builder
	if err := renderPage(&rendered, request, "Alice"); err != nil {
		t.Fatal(err)
	}
	html := rendered.String()
	if strings.Contains(html, "prayer-calendar-link") || strings.Contains(html, `href="/prayer#calendar"`) {
		t.Fatal("prayer sidebar still contains the redundant Calendar entry")
	}
	for _, marker := range []string{"prayer-today", "prayer-analytics-link", "prayer-settings"} {
		if !strings.Contains(html, marker) {
			t.Errorf("prayer sidebar missing %s", marker)
		}
	}
}

func TestPrayerDashboardWidgetUsesSettingsAndClockPreference(t *testing.T) {
	database := prayerTestDB(t)
	now := time.Date(2026, 8, 12, 13, 30, 0, 0, toronto(t))
	app := New(Config{DB: database, Now: func() time.Time { return now }})
	settings := Settings{
		Location: "Milton", LocationLabel: "Milton, Ontario, Canada",
		Latitude: 43.5183, Longitude: -79.8774, Timezone: "America/Toronto",
		Method: MethodISNA, AsrSchool: AsrStandard, Configured: true,
	}
	if err := app.saveSettings(context.Background(), "alice", settings); err != nil {
		t.Fatal(err)
	}
	if err := app.withConn(context.Background(), func(conn *sqlite.Conn) error {
		return preferences.SetJSON(conn, "alice", "global", "time_format", map[string]bool{"time_format_24h": true})
	}); err != nil {
		t.Fatal(err)
	}

	definitions := app.DashboardWidgets()
	if len(definitions) != 1 || definitions[0].ID != prayerWidgetID || definitions[0].DefaultSize != apps.WidgetStandard {
		t.Fatalf("dashboard definitions = %+v", definitions)
	}
	data, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: apps.WidgetStandard}, prayerWidgetID)
	if err != nil {
		t.Fatal(err)
	}
	if len(data.Items) != 3 || data.Items[0].Title != "Next · Asr" {
		t.Fatalf("standard widget items = %+v", data.Items)
	}
	if !strings.Contains(data.Items[0].Subtitle, "17:") || !strings.Contains(data.Items[0].Subtitle, "in ") {
		t.Fatalf("next prayer subtitle = %q", data.Items[0].Subtitle)
	}
	compact, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Size: apps.WidgetCompact}, prayerWidgetID)
	if err != nil || len(compact.Items) != 1 {
		t.Fatalf("compact widget = %+v err=%v", compact, err)
	}
}

func TestPrayerDashboardWidgetSetupTomorrowAndErrors(t *testing.T) {
	database := prayerTestDB(t)
	late := time.Date(2026, 8, 12, 23, 30, 0, 0, toronto(t))
	app := New(Config{DB: database, Now: func() time.Time { return late }})

	empty, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "unconfigured", Size: apps.WidgetStandard}, prayerWidgetID)
	if err != nil || len(empty.Items) != 0 || !strings.Contains(empty.EmptyState, "location") {
		t.Fatalf("unconfigured widget = %+v err=%v", empty, err)
	}
	if _, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "unconfigured", Size: apps.WidgetStandard}, "missing"); err == nil {
		t.Fatal("unknown widget did not return an error")
	}
	settings := Settings{
		Location: "Toronto", LocationLabel: "Toronto, Ontario, Canada",
		Latitude: 43.6532, Longitude: -79.3832, Timezone: "America/Toronto",
		Method: MethodMWL, AsrSchool: AsrStandard, Configured: true,
	}
	if err := app.saveSettings(context.Background(), "alice", settings); err != nil {
		t.Fatal(err)
	}
	data, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Size: apps.WidgetCompact}, prayerWidgetID)
	if err != nil {
		t.Fatal(err)
	}
	if len(data.Items) != 1 || data.Items[0].Title != "Next · Fajr" || !strings.Contains(data.Items[0].Subtitle, "Tomorrow") || !strings.Contains(data.Items[0].URL, "2026-08-13") {
		t.Fatalf("tomorrow widget = %+v", data.Items)
	}
}
