package main

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
)

func testSettingsServer(t *testing.T) *server.Server {
	t.Helper()
	path := "/tmp/dar_test_files_settings.sqlite"
	_ = os.Remove(path)
	t.Cleanup(func() { _ = os.Remove(path) })
	database, err := db.Open(path, append(auth.Migrations(), preferences.Migrations()...))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = database.Close() })
	return server.New(database, nil)
}

func filesSettingsRequest(method, target string, body any) (*httptest.ResponseRecorder, *http.Request) {
	var reader *bytes.Reader
	if body == nil {
		reader = bytes.NewReader(nil)
	} else {
		data, _ := json.Marshal(body)
		reader = bytes.NewReader(data)
	}
	req := httptest.NewRequest(method, target, reader)
	req.Header.Set("Content-Type", "application/json")
	req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: "alice"}))
	return httptest.NewRecorder(), req
}

func TestFilesSettingsDefaults(t *testing.T) {
	srv := testSettingsServer(t)
	rec, req := filesSettingsRequest(http.MethodGet, "/api/settings/files", nil)
	handleGetFilesSettings(srv)(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status = %d body = %s", rec.Code, rec.Body.String())
	}
	var got filesSettings
	if err := json.Unmarshal(rec.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.PersonalFolder != "/docs" || got.TrashFolder != "/.local/share/Trash" {
		t.Fatalf("defaults = %+v", got)
	}
	if !got.SortFavoritesFirst || !got.SortFoldersBeforeFiles || got.ShowHidden || !got.ShowFileExtensions || !got.WarnBeforeExtensionChange || got.WarnBeforeDelete {
		t.Fatalf("unexpected boolean defaults: %+v", got)
	}
}

func TestFilesSettingsValidation(t *testing.T) {
	cases := []filesSettings{
		{PersonalFolder: "docs", TrashFolder: "/.local/share/Trash", DefaultView: "list"},
		{PersonalFolder: "/", TrashFolder: "/.local/share/Trash", DefaultView: "list"},
		{PersonalFolder: "/docs", TrashFolder: "/", DefaultView: "list"},
		{PersonalFolder: "/docs", TrashFolder: "/docs/trash", DefaultView: "list"},
	}
	for _, tc := range cases {
		tc.SortFavoritesFirst = true
		tc.SortFoldersBeforeFiles = true
		tc.ShowFileExtensions = true
		tc.CropImagePreviews = true
		tc.ShowRecommendations = true
		tc.WarnBeforeExtensionChange = true
		if _, err := validateFilesSettings(tc); err == nil {
			t.Fatalf("validateFilesSettings(%+v) succeeded, want error", tc)
		}
	}
}

func TestFilesSettingsPostPersistsPerUser(t *testing.T) {
	srv := testSettingsServer(t)
	want := defaultFilesSettings()
	want.PersonalFolder = "/work"
	want.TrashFolder = "/bin/trash"
	want.ShowHidden = true
	want.DefaultView = "grid"

	rec, req := filesSettingsRequest(http.MethodPost, "/api/settings/files", want)
	handleSaveFilesSettings(srv)(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("post status = %d body = %s", rec.Code, rec.Body.String())
	}

	rec, req = filesSettingsRequest(http.MethodGet, "/api/settings/files", nil)
	handleGetFilesSettings(srv)(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("get status = %d body = %s", rec.Code, rec.Body.String())
	}
	var got filesSettings
	if err := json.Unmarshal(rec.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.PersonalFolder != want.PersonalFolder || got.TrashFolder != want.TrashFolder || !got.ShowHidden || got.DefaultView != "grid" {
		t.Fatalf("got %+v want %+v", got, want)
	}

	rec, req = filesSettingsRequest(http.MethodGet, "/api/settings/files", nil)
	req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: "bob"}))
	handleGetFilesSettings(srv)(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("bob get status = %d body = %s", rec.Code, rec.Body.String())
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.PersonalFolder != "/docs" || got.TrashFolder != "/.local/share/Trash" {
		t.Fatalf("bob should get defaults, got %+v", got)
	}
}

func intPtr(v int) *int { return &v }

func TestCalendarSettingsDefaultsAndPersistence(t *testing.T) {
	srv := testSettingsServer(t)
	rec, req := filesSettingsRequest(http.MethodGet, "/api/settings/calendar", nil)
	handleGetCalendarSettings(srv)(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("default status = %d body = %s", rec.Code, rec.Body.String())
	}
	var got calendarSettings
	if err := json.Unmarshal(rec.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.Timezone != "automatic" || !got.BirthdayCalendar || !got.ShowWeekends || !got.LimitMonthEvents || got.SlotMinutes != 30 || !got.SimpleEditor {
		t.Fatalf("unexpected defaults: %+v", got)
	}

	want := defaultCalendarSettings()
	want.Timezone = "America/Toronto"
	want.ShowWeekends = false
	want.ShowWeekNumbers = true
	want.SlotMinutes = 15
	want.DefaultTimedReminder = intPtr(10)
	want.SimpleEditor = false
	rec, req = filesSettingsRequest(http.MethodPost, "/api/settings/calendar", want)
	handleSaveCalendarSettings(srv)(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("save status = %d body = %s", rec.Code, rec.Body.String())
	}
	rec, req = filesSettingsRequest(http.MethodGet, "/api/settings/calendar", nil)
	handleGetCalendarSettings(srv)(rec, req)
	if err := json.Unmarshal(rec.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.Timezone != want.Timezone || got.ShowWeekends || !got.ShowWeekNumbers || got.SlotMinutes != 15 || got.DefaultTimedReminder == nil || *got.DefaultTimedReminder != 10 || got.SimpleEditor {
		t.Fatalf("persisted settings = %+v", got)
	}

	rec, req = filesSettingsRequest(http.MethodGet, "/api/settings/calendar", nil)
	req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: "bob"}))
	handleGetCalendarSettings(srv)(rec, req)
	if err := json.Unmarshal(rec.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.Timezone != "automatic" || !got.ShowWeekends || got.ShowWeekNumbers {
		t.Fatalf("bob should receive defaults: %+v", got)
	}
}

func TestCalendarSettingsValidation(t *testing.T) {
	srv := testSettingsServer(t)
	invalid := []calendarSettings{
		func() calendarSettings { v := defaultCalendarSettings(); v.Timezone = "Mars/Olympus"; return v }(),
		func() calendarSettings { v := defaultCalendarSettings(); v.SlotMinutes = 25; return v }(),
		func() calendarSettings { v := defaultCalendarSettings(); v.DefaultTimedReminder = intPtr(7); return v }(),
	}
	for _, value := range invalid {
		rec, req := filesSettingsRequest(http.MethodPost, "/api/settings/calendar", value)
		handleSaveCalendarSettings(srv)(rec, req)
		if rec.Code != http.StatusBadRequest {
			t.Fatalf("value %+v status = %d body = %s", value, rec.Code, rec.Body.String())
		}
	}
	rec, req := filesSettingsRequest(http.MethodPost, "/api/settings/calendar", nil)
	req.Body = http.NoBody
	handleSaveCalendarSettings(srv)(rec, req)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("invalid JSON status = %d", rec.Code)
	}
}
