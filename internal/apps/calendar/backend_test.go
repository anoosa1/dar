package calendar

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/outbound"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
)

var testApp = New(Config{})

func withTestUser(r *http.Request, id string) *http.Request {
	return r.WithContext(context.WithValue(r.Context(), server.UserContextKey, &auth.User{ID: id}))
}

func withTempDataRoot(t *testing.T) string {
	t.Helper()
	old := testApp.dataRoot
	dir := t.TempDir()
	testApp.dataRoot = dir
	t.Cleanup(func() { testApp.dataRoot = old })
	return dir
}

func newSQLTestApp(t *testing.T) *App {
	t.Helper()
	policy, err := outbound.ParsePolicy("127.0.0.1,localhost", "")
	if err != nil {
		t.Fatalf("configure outbound policy: %v", err)
	}
	database, err := db.Open(filepath.Join(t.TempDir(), "calendar.sqlite"), append(preferences.Migrations(), Migrations()...))
	if err != nil {
		t.Fatalf("open test database: %v", err)
	}
	t.Cleanup(func() { _ = database.Close() })
	return New(Config{DataRoot: t.TempDir(), DB: database, OutboundPolicy: policy})
}

func postJSON(t *testing.T, handler http.HandlerFunc, uid string, body string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(http.MethodPost, "/", bytes.NewBufferString(body))
	req.Header.Set("Content-Type", "application/json")
	req = withTestUser(req, uid)
	rr := httptest.NewRecorder()
	handler(rr, req)
	return rr
}

func getJSON(t *testing.T, handler http.HandlerFunc, target string, uid string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(http.MethodGet, target, nil)
	req = withTestUser(req, uid)
	rr := httptest.NewRecorder()
	handler(rr, req)
	return rr
}

func decodeBody[T any](t *testing.T, rr *httptest.ResponseRecorder) T {
	t.Helper()
	var got T
	if err := json.NewDecoder(rr.Body).Decode(&got); err != nil {
		t.Fatalf("decode response: %v\nbody: %s", err, rr.Body.String())
	}
	return got
}

func TestCreateWritesICSForRequestUser(t *testing.T) {
	dir := withTempDataRoot(t)

	rr := postJSON(t, testApp.handleCreate, "alice", `{"title":"Team Standup","start":1700000000,"end":1700003600,"description":"Daily sync"}`)
	if rr.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d; body=%s", rr.Code, http.StatusOK, rr.Body.String())
	}
	created := decodeBody[Event](t, rr)
	if created.ID == "" {
		t.Fatal("created event ID is empty")
	}

	path := filepath.Join(dir, "alice", "calendars", "default", created.ID+".ics")
	parsed, err := testApp.parseICS(path)
	if err != nil {
		t.Fatalf("parse created ICS: %v", err)
	}
	if parsed.Title != "Team Standup" || parsed.Description != "Daily sync" {
		t.Fatalf("created event = %+v", parsed)
	}
	if parsed.Start != 1700000000 || parsed.End != 1700003600 {
		t.Fatalf("timestamps: start=%d end=%d", parsed.Start, parsed.End)
	}
}

func TestListReturnsEventsForSameUserOnly(t *testing.T) {
	withTempDataRoot(t)

	postJSON(t, testApp.handleCreate, "alice", `{"title":"Alice Event","start":100,"end":200}`)
	postJSON(t, testApp.handleCreate, "bob", `{"title":"Bob Event","start":100,"end":200}`)

	rr := getJSON(t, testApp.handleList, "/list", "alice")
	if rr.Code != http.StatusOK {
		t.Fatalf("list status = %d, want %d", rr.Code, http.StatusOK)
	}
	got := decodeBody[struct {
		Events []Event `json:"events"`
	}](t, rr)
	if len(got.Events) != 1 {
		t.Fatalf("events len = %d, want 1: %+v", len(got.Events), got.Events)
	}
	if got.Events[0].Title != "Alice Event" {
		t.Fatalf("listed event = %+v, want Alice only", got.Events[0])
	}
}

func TestSQLRangeLoadSkipsNonOverlappingEvents(t *testing.T) {
	app := newSQLTestApp(t)
	app.ensureSQLDefault("alice")
	for _, event := range []Event{
		{ID: "past", CalendarID: "default", Start: 100, End: 200},
		{ID: "overlap", CalendarID: "default", Start: 1000, End: 1200},
		{ID: "series", CalendarID: "default", Start: 100, End: 200, RRule: "FREQ=DAILY"},
	} {
		if err := app.sqlWriteEvent("alice", event); err != nil {
			t.Fatalf("write %q: %v", event.ID, err)
		}
	}

	got := app.sqlEventsInRange("alice", "", 900, 1300)
	if len(got) != 2 {
		t.Fatalf("range events = %#v, want overlapping event and recurring master", got)
	}
	ids := map[string]bool{}
	for _, event := range got {
		ids[event.ID] = true
	}
	if !ids["overlap"] || !ids["series"] || ids["past"] {
		t.Fatalf("range event IDs = %#v", ids)
	}
}

func TestSQLReplaceCalendarEventsReplacesSnapshot(t *testing.T) {
	app := newSQLTestApp(t)
	app.ensureSQLDefault("alice")
	if err := app.sqlWriteEvent("alice", Event{ID: "old", CalendarID: "default", Start: 100, End: 200}); err != nil {
		t.Fatalf("write old event: %v", err)
	}
	if err := app.sqlReplaceCalendarEvents("alice", "default", []Event{
		{ID: "new-1", CalendarID: "default", Start: 300, End: 400},
		{ID: "new-2", CalendarID: "default", Start: 500, End: 600},
	}); err != nil {
		t.Fatalf("replace events: %v", err)
	}

	got := app.sqlEvents("alice", "default")
	if len(got) != 2 || got[0].ID != "new-1" || got[1].ID != "new-2" {
		t.Fatalf("replaced events = %#v", got)
	}
}

func TestSubscriptionRefreshStoresEventsAndMetadata(t *testing.T) {
	app := newSQLTestApp(t)
	feed := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/calendar")
		_, _ = w.Write([]byte("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:remote-event\r\nDTSTART:20300115T120000Z\r\nDTEND:20300115T120000Z\r\nSUMMARY:Remote planning\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"))
	}))
	defer feed.Close()

	req := httptest.NewRequest(http.MethodPost, "/api/calendar/subscriptions", strings.NewReader(`{"name":"Team feed","url":"`+feed.URL+`"}`))
	req = withTestUser(req, "alice")
	rr := httptest.NewRecorder()
	app.handleSubscriptions(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("create subscription status = %d body = %s", rr.Code, rr.Body.String())
	}
	meta := decodeBody[CalendarMeta](t, rr)
	if meta.ID == "" {
		t.Fatal("subscription ID is empty")
	}
	calendars := app.listCalendarMetas("alice")
	var stored CalendarMeta
	for _, calendar := range calendars {
		if calendar.ID == meta.ID {
			stored = calendar
			break
		}
	}
	if stored.Kind != "subscription" || !stored.ReadOnly || stored.SubscriptionURL != feed.URL || stored.LastSyncedAt == 0 {
		t.Fatalf("subscription metadata = %+v", stored)
	}
	events := app.sqlEventsInRange("alice", meta.ID, 1894700000, 1894800000)
	if len(events) != 1 || events[0].ID != "remote-event" || events[0].Title != "Remote planning" || events[0].End-events[0].Start != 60 {
		t.Fatalf("subscription events = %+v", events)
	}
}

func TestUpdateChangesExistingEvent(t *testing.T) {
	withTempDataRoot(t)

	createdRR := postJSON(t, testApp.handleCreate, "alice", `{"title":"Old","start":100,"end":200}`)
	created := decodeBody[Event](t, createdRR)

	updateBody := `{"id":"` + created.ID + `","title":"Updated","description":"New desc","start":300,"end":400}`
	rr := postJSON(t, testApp.handleUpdate, "alice", updateBody)
	if rr.Code != http.StatusOK {
		t.Fatalf("update status = %d, want %d; body=%s", rr.Code, http.StatusOK, rr.Body.String())
	}

	getRR := getJSON(t, testApp.handleGet, "/get?id="+created.ID, "alice")
	got := decodeBody[Event](t, getRR)
	if got.Title != "Updated" || got.Description != "New desc" || got.Start != 300 || got.End != 400 {
		t.Fatalf("updated event = %+v", got)
	}
}

func TestDeleteRemovesEvent(t *testing.T) {
	withTempDataRoot(t)

	createdRR := postJSON(t, testApp.handleCreate, "alice", `{"title":"Delete Me","start":100,"end":200}`)
	created := decodeBody[Event](t, createdRR)

	req := httptest.NewRequest(http.MethodPost, "/delete?id="+created.ID, nil)
	req = withTestUser(req, "alice")
	rr := httptest.NewRecorder()
	testApp.handleDelete(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("delete status = %d, want %d", rr.Code, http.StatusOK)
	}

	getRR := getJSON(t, testApp.handleGet, "/get?id="+created.ID, "alice")
	if getRR.Code != http.StatusNotFound {
		t.Fatalf("get after delete status = %d, want %d", getRR.Code, http.StatusNotFound)
	}
}

func TestListFiltersByTimeRange(t *testing.T) {
	withTempDataRoot(t)

	postJSON(t, testApp.handleCreate, "alice", `{"title":"Jan","start":1700000000,"end":1700003600}`) // Jan 2023
	postJSON(t, testApp.handleCreate, "alice", `{"title":"Mar","start":1710000000,"end":1710003600}`) // Mar 2023
	postJSON(t, testApp.handleCreate, "alice", `{"title":"May","start":1720000000,"end":1720003600}`) // May 2023

	// Filter Jan-Feb range
	rr := getJSON(t, testApp.handleList, "/list?start=1690000000&end=1715000000", "alice")
	got := decodeBody[struct {
		Events []Event `json:"events"`
	}](t, rr)
	if len(got.Events) != 2 {
		t.Fatalf("filtered events len = %d, want 2: %+v", len(got.Events), got.Events)
	}
}

func TestExportReturnsICSFile(t *testing.T) {
	withTempDataRoot(t)

	postJSON(t, testApp.handleCreate, "alice", `{"title":"Meeting 1","start":100,"end":200}`)
	postJSON(t, testApp.handleCreate, "alice", `{"title":"Meeting 2","start":300,"end":400}`)

	rr := getJSON(t, testApp.handleExport, "/export", "alice")
	if rr.Code != http.StatusOK {
		t.Fatalf("export status = %d, want %d", rr.Code, http.StatusOK)
	}
	body := rr.Body.String()
	if !strings.Contains(body, "BEGIN:VCALENDAR") || !strings.Contains(body, "END:VCALENDAR") {
		t.Fatal("export missing VCALENDAR markers")
	}
	if !strings.Contains(body, "Meeting 1") || !strings.Contains(body, "Meeting 2") {
		t.Fatal("export missing event data")
	}
}

func TestImportParsesICSFile(t *testing.T) {
	dir := withTempDataRoot(t)

	ics := "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:imp-001\r\nDTSTAMP:20260101T000000Z\r\nDTSTART:20260101T090000Z\r\nDTEND:20260101T100000Z\r\nSUMMARY:Imported Event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n" +
		"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:imp-002\r\nDTSTAMP:20260101T000000Z\r\nDTSTART:20260102T090000Z\r\nDTEND:20260102T100000Z\r\nSUMMARY:Second Import\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"

	body := strings.NewReader(
		"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"events.ics\"\r\n\r\n" +
			ics + "\r\n--boundary--\r\n",
	)
	req := httptest.NewRequest(http.MethodPost, "/import", body)
	req.Header.Set("Content-Type", "multipart/form-data; boundary=boundary")
	req = withTestUser(req, "alice")
	rr := httptest.NewRecorder()
	testApp.handleImport(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("import status = %d, want %d; body=%s", rr.Code, http.StatusOK, rr.Body.String())
	}
	got := decodeBody[struct {
		Imported int `json:"imported"`
	}](t, rr)
	if got.Imported != 2 {
		t.Fatalf("imported = %d, want 2", got.Imported)
	}

	listRR := getJSON(t, testApp.handleList, "/list", "alice")
	list := decodeBody[struct {
		Events []Event `json:"events"`
	}](t, listRR)
	if len(list.Events) != 2 {
		t.Fatalf("events after import = %d, want 2", len(list.Events))
	}
	os.RemoveAll(dir)
}

func TestAllDayEvent(t *testing.T) {
	withTempDataRoot(t)

	rr := postJSON(t, testApp.handleCreate, "alice", `{"title":"Holiday","start":1700000000,"end":1700172800,"all_day":true}`)
	if rr.Code != http.StatusOK {
		t.Fatalf("create all-day: %d %s", rr.Code, rr.Body.String())
	}

	listRR := getJSON(t, testApp.handleList, "/list", "alice")
	got := decodeBody[struct {
		Events []Event `json:"events"`
	}](t, listRR)
	if len(got.Events) != 1 {
		t.Fatalf("all-day event not listed: %d events", len(got.Events))
	}
	if !got.Events[0].AllDay {
		t.Fatalf("expected all_day=true, got %+v", got.Events[0])
	}
}

func TestCalendarsCRUD(t *testing.T) {
	withTempDataRoot(t)

	// Default Personal calendar always present
	listRR := getJSON(t, testApp.handleCalendars, "/calendars", "alice")
	if listRR.Code != http.StatusOK {
		t.Fatalf("list calendars: %d %s", listRR.Code, listRR.Body.String())
	}
	listed := decodeBody[struct {
		Calendars []CalendarMeta `json:"calendars"`
	}](t, listRR)
	if len(listed.Calendars) < 1 || listed.Calendars[0].ID != "default" {
		t.Fatalf("expected default calendar, got %+v", listed.Calendars)
	}
	if listed.Calendars[0].Color != calendarTheme.Pink {
		t.Fatalf("default calendar color = %q, want %q", listed.Calendars[0].Color, calendarTheme.Pink)
	}

	defaultColorRR := postJSON(t, testApp.handleCalendars, "alice", `{"name":"Personal projects"}`)
	if defaultColorRR.Code != http.StatusOK {
		t.Fatalf("create default-colored calendar: %d %s", defaultColorRR.Code, defaultColorRR.Body.String())
	}
	if got := decodeBody[CalendarMeta](t, defaultColorRR).Color; got != calendarTheme.Pink {
		t.Fatalf("new calendar color = %q, want %q", got, calendarTheme.Pink)
	}

	createRR := postJSON(t, testApp.handleCalendars, "alice", `{"name":"Work","color":"#ff5722"}`)
	if createRR.Code != http.StatusOK {
		t.Fatalf("create calendar: %d %s", createRR.Code, createRR.Body.String())
	}
	created := decodeBody[CalendarMeta](t, createRR)
	if created.ID != "work" || created.Color != "#ff5722" {
		t.Fatalf("created calendar = %+v", created)
	}

	// Event on work calendar
	evRR := postJSON(t, testApp.handleCreate, "alice", `{"title":"Standup","start":100,"end":200,"calendar":"work","color":"#ff5722"}`)
	if evRR.Code != http.StatusOK {
		t.Fatalf("create on work: %d %s", evRR.Code, evRR.Body.String())
	}
	ev := decodeBody[Event](t, evRR)
	if ev.CalendarID != "work" {
		t.Fatalf("event calendar = %q", ev.CalendarID)
	}

	// List all includes work event
	allRR := getJSON(t, testApp.handleList, "/list", "alice")
	all := decodeBody[struct {
		Events []Event `json:"events"`
	}](t, allRR)
	if len(all.Events) != 1 || all.Events[0].Title != "Standup" {
		t.Fatalf("list all = %+v", all.Events)
	}

	// Filter by calendar
	workRR := getJSON(t, testApp.handleList, "/list?calendar=work", "alice")
	work := decodeBody[struct {
		Events []Event `json:"events"`
	}](t, workRR)
	if len(work.Events) != 1 {
		t.Fatalf("work list len = %d", len(work.Events))
	}
	defRR := getJSON(t, testApp.handleList, "/list?calendar=default", "alice")
	def := decodeBody[struct {
		Events []Event `json:"events"`
	}](t, defRR)
	if len(def.Events) != 0 {
		t.Fatalf("default should be empty, got %+v", def.Events)
	}

	// Update visibility
	updRR := postJSON(t, testApp.handleCalendarUpdate, "alice", `{"id":"work","name":"Work","color":"#ff5722","visible":false,"order":1}`)
	if updRR.Code != http.StatusOK {
		t.Fatalf("update cal: %d %s", updRR.Code, updRR.Body.String())
	}
	upd := decodeBody[CalendarMeta](t, updRR)
	if upd.Visible {
		t.Fatal("expected visible=false")
	}

	// Cannot delete default
	req := httptest.NewRequest(http.MethodPost, "/calendars/delete?id=default", nil)
	req = withTestUser(req, "alice")
	rr := httptest.NewRecorder()
	testApp.handleCalendarDelete(rr, req)
	if rr.Code != http.StatusBadRequest {
		t.Fatalf("delete default status = %d, want 400", rr.Code)
	}
}

func TestColorAndLocationRoundTrip(t *testing.T) {
	dir := withTempDataRoot(t)

	rr := postJSON(t, testApp.handleCreate, "alice", `{"title":"Paint","start":100,"end":200,"location":"Studio","color":"#00bcd4","all_day":false}`)
	if rr.Code != http.StatusOK {
		t.Fatalf("create: %d %s", rr.Code, rr.Body.String())
	}
	created := decodeBody[Event](t, rr)

	path := filepath.Join(dir, "alice", "calendars", "default", created.ID+".ics")
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	ics := string(raw)
	if !strings.Contains(ics, "COLOR:") && !strings.Contains(ics, "COLOR;") {
		t.Fatalf("ICS missing COLOR: %s", ics)
	}
	if !strings.Contains(ics, "LOCATION:Studio") {
		t.Fatalf("ICS missing LOCATION: %s", ics)
	}

	parsed, err := testApp.parseICS(path)
	if err != nil {
		t.Fatal(err)
	}
	if parsed.Color != "#00bcd4" || parsed.Location != "Studio" {
		t.Fatalf("parsed = %+v", parsed)
	}
}

func TestStatusBusyCategoriesRoundTrip(t *testing.T) {
	withTempDataRoot(t)

	rr := postJSON(t, testApp.handleCreate, "alice", `{"title":"Tagged","start":100,"end":200,"status":"TENTATIVE","busy":false,"categories":["work","phase2"]}`)
	if rr.Code != http.StatusOK {
		t.Fatalf("create: %d %s", rr.Code, rr.Body.String())
	}
	created := decodeBody[Event](t, rr)
	if created.Status != "TENTATIVE" || created.Busy {
		t.Fatalf("create response status/busy = %q %v", created.Status, created.Busy)
	}
	if len(created.Categories) != 2 || created.Categories[0] != "work" || created.Categories[1] != "phase2" {
		t.Fatalf("create categories = %#v", created.Categories)
	}

	// Re-read via list (parseICS path)
	listRR := getJSON(t, testApp.handleList, "/list?start=0&end=999", "alice")
	var list struct {
		Events []Event `json:"events"`
	}
	if err := json.NewDecoder(listRR.Body).Decode(&list); err != nil {
		t.Fatal(err)
	}
	var found *Event
	for i := range list.Events {
		if list.Events[i].ID == created.ID {
			found = &list.Events[i]
			break
		}
	}
	if found == nil {
		t.Fatal("event not in list")
	}
	if found.Status != "TENTATIVE" || found.Busy {
		t.Fatalf("list status/busy = %q %v", found.Status, found.Busy)
	}
	if len(found.Categories) != 2 || found.Categories[0] != "work" || found.Categories[1] != "phase2" {
		t.Fatalf("list categories = %#v (escaped bug?)", found.Categories)
	}

	// Update and re-read should not double-escape
	upd := postJSON(t, testApp.handleUpdate, "alice", `{"id":"`+created.ID+`","title":"Tagged","start":100,"end":200,"status":"TENTATIVE","busy":false,"categories":["work","phase2"]}`)
	if upd.Code != http.StatusOK {
		t.Fatalf("update: %d %s", upd.Code, upd.Body.String())
	}
	listRR2 := getJSON(t, testApp.handleList, "/list?start=0&end=999", "alice")
	if err := json.NewDecoder(listRR2.Body).Decode(&list); err != nil {
		t.Fatal(err)
	}
	found = nil
	for i := range list.Events {
		if list.Events[i].ID == created.ID {
			found = &list.Events[i]
			break
		}
	}
	if found == nil || len(found.Categories) != 2 || found.Categories[0] != "work" {
		t.Fatalf("after update categories = %#v", found)
	}
}

func TestRecurrenceExpandAndExdate(t *testing.T) {
	withTempDataRoot(t)
	// Daily series starting at a fixed unix time (2024-01-01 10:00 UTC)
	start := int64(1704103200) // 2024-01-01 10:00:00 UTC
	end := start + 3600
	rr := postJSON(t, testApp.handleCreate, "alice", fmt.Sprintf(
		`{"title":"Standup","start":%d,"end":%d,"rrule":"FREQ=DAILY","reminders":[15]}`, start, end))
	if rr.Code != http.StatusOK {
		t.Fatalf("create: %d %s", rr.Code, rr.Body.String())
	}
	created := decodeBody[Event](t, rr)
	if created.RRule != "FREQ=DAILY" {
		t.Fatalf("rrule = %q", created.RRule)
	}
	if len(created.Reminders) != 1 || created.Reminders[0] != 15 {
		t.Fatalf("reminders = %#v", created.Reminders)
	}

	// List 3 days of window
	listStart := start - 3600
	listEnd := start + 3*86400
	listRR := getJSON(t, testApp.handleList, fmt.Sprintf("/list?start=%d&end=%d", listStart, listEnd), "alice")
	var list struct {
		Events []Event `json:"events"`
	}
	if err := json.NewDecoder(listRR.Body).Decode(&list); err != nil {
		t.Fatal(err)
	}
	if len(list.Events) < 3 {
		t.Fatalf("expected >=3 instances, got %d: %+v", len(list.Events), list.Events)
	}
	for _, ev := range list.Events {
		if ev.MasterID != created.ID {
			t.Fatalf("master_id = %q want %q", ev.MasterID, created.ID)
		}
		if !strings.Contains(ev.ID, "@") {
			t.Fatalf("instance id should be synthetic: %q", ev.ID)
		}
		if len(ev.Reminders) != 1 || ev.Reminders[0] != 15 {
			t.Fatalf("instance reminders = %#v", ev.Reminders)
		}
	}

	// Delete "this" second instance
	second := list.Events[1]
	delReq := httptest.NewRequest(http.MethodPost,
		"/delete?id="+second.ID+"&scope=this&recurrence_id="+fmt.Sprintf("%d", second.RecurrenceID), nil)
	delReq = withTestUser(delReq, "alice")
	delRR := httptest.NewRecorder()
	testApp.handleDelete(delRR, delReq)
	if delRR.Code != http.StatusOK {
		t.Fatalf("delete this: %d %s", delRR.Code, delRR.Body.String())
	}

	listRR2 := getJSON(t, testApp.handleList, fmt.Sprintf("/list?start=%d&end=%d", listStart, listEnd), "alice")
	if err := json.NewDecoder(listRR2.Body).Decode(&list); err != nil {
		t.Fatal(err)
	}
	for _, ev := range list.Events {
		if ev.RecurrenceID == second.RecurrenceID {
			t.Fatalf("exdated instance still present: %+v", ev)
		}
	}
}

func TestReminderVALARMRoundTrip(t *testing.T) {
	dir := withTempDataRoot(t)
	rr := postJSON(t, testApp.handleCreate, "alice", `{"title":"Call","start":100,"end":200,"reminders":[10,60]}`)
	if rr.Code != http.StatusOK {
		t.Fatalf("create: %d %s", rr.Code, rr.Body.String())
	}
	created := decodeBody[Event](t, rr)
	path := filepath.Join(dir, "alice", "calendars", "default", created.ID+".ics")
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	ics := string(raw)
	if !strings.Contains(ics, "BEGIN:VALARM") || !strings.Contains(ics, "TRIGGER") {
		t.Fatalf("ICS missing VALARM: %s", ics)
	}
	parsed, err := testApp.parseICS(path)
	if err != nil {
		t.Fatal(err)
	}
	if len(parsed.Reminders) != 2 {
		t.Fatalf("parsed reminders = %#v", parsed.Reminders)
	}
}

func TestEventMoveBetweenCalendars(t *testing.T) {
	withTempDataRoot(t)
	postJSON(t, testApp.handleCalendars, "alice", `{"name":"Side","color":"#111111"}`)

	createdRR := postJSON(t, testApp.handleCreate, "alice", `{"title":"Move me","start":100,"end":200}`)
	created := decodeBody[Event](t, createdRR)

	upd := postJSON(t, testApp.handleUpdate, "alice", `{"id":"`+created.ID+`","title":"Move me","start":100,"end":200,"calendar":"side"}`)
	if upd.Code != http.StatusOK {
		t.Fatalf("update: %d %s", upd.Code, upd.Body.String())
	}
	got := decodeBody[Event](t, upd)
	if got.CalendarID != "side" {
		t.Fatalf("calendar = %q", got.CalendarID)
	}

	// Gone from default
	defRR := getJSON(t, testApp.handleList, "/list?calendar=default", "alice")
	def := decodeBody[struct {
		Events []Event `json:"events"`
	}](t, defRR)
	if len(def.Events) != 0 {
		t.Fatalf("still on default: %+v", def.Events)
	}
}

func TestEventIDRejectsPathTraversal(t *testing.T) {
	for _, id := range []string{"../secret", `..\\secret`, ".", "..", "event\r\nX-Test: injected"} {
		if validEventID(id) {
			t.Fatalf("unsafe event id %q was accepted", id)
		}
	}
	if !validEventFilename("safe-event@123.ics") {
		t.Fatal("safe CalDAV event filename was rejected")
	}
}
