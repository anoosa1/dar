package todo

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/server"
)

func testApp(t *testing.T) *App {
	t.Helper()
	database, err := db.Open(filepath.Join(t.TempDir(), "todo.sqlite"), append(auth.Migrations(), Migrations()...))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	return New(Config{DB: database})
}

func preferenceRequest(method string, value any) (*httptest.ResponseRecorder, *http.Request) {
	var body bytes.Buffer
	if value != nil {
		_ = json.NewEncoder(&body).Encode(value)
	}
	req := httptest.NewRequest(method, "/api/todo/preferences", &body)
	req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: "alice"}))
	return httptest.NewRecorder(), req
}

func TestPreferencesModesCountsAndValidation(t *testing.T) {
	app := testApp(t)
	list, err := app.ensureDefaultList("alice")
	if err != nil {
		t.Fatal(err)
	}
	if list.Color != todoTheme.Pink {
		t.Fatalf("default list color = %q, want %q", list.Color, todoTheme.Pink)
	}
	now := time.Now().Unix()
	if err := app.exec(`INSERT INTO todo_tasks(id,list_id,title,status,starred,created_at,updated_at) VALUES(?,?,?,?,?,?,?)`, "important", list.ID, "Important", "NEEDS-ACTION", 1, now, now); err != nil {
		t.Fatal(err)
	}
	prefs := defaultPreferences()
	prefs.DefaultListID = list.ID
	prefs.SortMode = "priority"
	prefs.SmartCollections["completed"] = "hidden"
	rec, req := preferenceRequest(http.MethodPut, prefs)
	app.handlePreferences(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("save status = %d body = %s", rec.Code, rec.Body.String())
	}
	rec, req = preferenceRequest(http.MethodGet, nil)
	app.handlePreferences(rec, req)
	var got Preferences
	if err := json.Unmarshal(rec.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.DefaultListID != list.ID || got.SortMode != "priority" || got.SmartCollections["completed"] != "hidden" || got.CollectionCounts["important"] != 1 || got.CollectionCounts["completed"] != 0 {
		t.Fatalf("preferences = %+v", got)
	}

	bad := defaultPreferences()
	bad.SmartCollections["today"] = "sometimes"
	rec, req = preferenceRequest(http.MethodPut, bad)
	app.handlePreferences(rec, req)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("invalid mode status = %d", rec.Code)
	}
	bad = defaultPreferences()
	bad.DefaultListID = "not-accessible"
	rec, req = preferenceRequest(http.MethodPut, bad)
	app.handlePreferences(rec, req)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("invalid default list status = %d", rec.Code)
	}
}

func TestLegacyBooleanPreferencesFallBackToDefaults(t *testing.T) {
	app := testApp(t)
	if err := app.exec(`INSERT INTO todo_preferences(user_id,default_list_id,smart_collections_json,sort_mode) VALUES(?,?,?,?)`, "alice", "", `{"today":true}`, "due"); err != nil {
		t.Fatal(err)
	}
	prefs, err := app.preferences("alice")
	if err != nil {
		t.Fatal(err)
	}
	if prefs.SmartCollections["today"] != "automatic" {
		t.Fatalf("legacy preferences = %+v", prefs)
	}
}

func TestTaskStorageIsUserScoped(t *testing.T) {
	app := testApp(t)
	alice, err := app.ensureDefaultList("alice")
	if err != nil {
		t.Fatal(err)
	}
	now := int64(100)
	if err := app.exec(`INSERT INTO todo_tasks(id,list_id,title,status,created_at,updated_at) VALUES(?,?,?,?,?,?)`, "a", alice.ID, "Alice task", "NEEDS-ACTION", now, now); err != nil {
		t.Fatal(err)
	}
	got, err := app.tasks("alice", "", "all", false)
	if err != nil || len(got) != 1 || got[0].ID != "a" {
		t.Fatalf("alice tasks = %#v, %v", got, err)
	}
	got, err = app.tasks("bob", "", "all", false)
	if err != nil || len(got) != 0 {
		t.Fatalf("bob saw alice tasks: %#v, %v", got, err)
	}
}

func TestNextDue(t *testing.T) {
	base := int64(1704067200)
	got, err := nextDue("FREQ=WEEKLY;INTERVAL=2", base)
	if err != nil || got != base+14*86400 {
		t.Fatalf("next weekly = %d, %v", got, err)
	}
	if _, err := nextDue("FREQ=SECONDLY", base); err == nil {
		t.Fatal("invalid recurrence accepted")
	}
}

func TestTodoNormalizationAndRichICSBranchMatrix(t *testing.T) {
	anonymous := httptest.NewRecorder()
	handlePage(anonymous, httptest.NewRequest(http.MethodGet, "/todo", nil))
	if anonymous.Code != http.StatusSeeOther {
		t.Fatalf("anonymous page status=%d", anonymous.Code)
	}
	for _, task := range []Task{
		{Title: strings.Repeat("x", 513)},
		{Title: "bad status", Status: "UNKNOWN"},
		{Title: "bad percent", PercentComplete: -1},
		{Title: "bad percent", PercentComplete: 101},
		{Title: "bad priority", Priority: -1},
		{Title: "bad priority", Priority: 10},
		{Title: "bad recurrence", RRule: "FREQ=HOURLY"},
	} {
		copy := task
		if err := normalizeTask(&copy); err == nil {
			t.Errorf("invalid task accepted: %+v", task)
		}
	}
	valid := Task{
		ID: "rich", Title: "Rich, task; title", Notes: "line one\nline two",
		Status: "COMPLETED", PercentComplete: 100, Priority: 7,
		StartAt: 1_800_000_000, DueAt: 1_800_003_600, CompletedAt: 1_800_003_000,
		RRule: "FREQ=WEEKLY;INTERVAL=2", Tags: []string{"one", "two"}, IsHabit: true,
		UpdatedAt: 1_799_999_000,
	}
	raw := taskICS(valid)
	for _, want := range []string{"DESCRIPTION:", "DTSTART:", "DUE:", "COMPLETED:", "RRULE:", "CATEGORIES:", "X-DAR-HABIT:TRUE"} {
		if !strings.Contains(raw, want) {
			t.Errorf("rich ICS missing %s: %s", want, raw)
		}
	}
	parsed, err := taskFromICS(raw)
	if err != nil {
		t.Fatal(err)
	}
	if parsed.Title != valid.Title || parsed.Notes != valid.Notes || !parsed.IsHabit ||
		parsed.Priority != valid.Priority || len(parsed.Tags) != 2 {
		t.Fatalf("rich ICS round trip=%+v", parsed)
	}
	completed, err := taskFromICS("BEGIN:VTODO\r\nSUMMARY:Done\r\nSTATUS:COMPLETED\r\nEND:VTODO\r\n")
	if err != nil || completed.CompletedAt == 0 {
		t.Fatalf("completed default timestamp=%+v err=%v", completed, err)
	}
	if _, err := taskFromICS("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n"); err == nil {
		t.Fatal("ICS without VTODO accepted")
	}
	if _, err := taskFromICS("BEGIN:VTODO\r\nSUMMARY:\r\nEND:VTODO\r\n"); err == nil {
		t.Fatal("blank VTODO accepted")
	}
	if parseICSTime("20300102T030405") == 0 || parseICSTime("20300102") == 0 || parseICSTime("invalid") != 0 {
		t.Fatal("ICS time layout matrix failed")
	}
	for _, rule := range []string{"FREQ=DAILY", "FREQ=WEEKLY", "FREQ=MONTHLY", "FREQ=YEARLY"} {
		if _, err := nextDue(rule, 1_800_000_000); err != nil {
			t.Errorf("%s: %v", rule, err)
		}
	}
	for _, rule := range []string{"FREQ=DAILY;INTERVAL=0", "FREQ=DAILY;INTERVAL=x", "FREQ=HOURLY"} {
		if _, err := nextDue(rule, 1_800_000_000); err == nil {
			t.Errorf("invalid rule accepted: %s", rule)
		}
	}
}
