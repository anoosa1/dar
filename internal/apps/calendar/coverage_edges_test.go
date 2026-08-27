package calendar

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/apps"
	"github.com/emersion/go-ical"
)

type failingCalendarSearch struct {
	dirty int
}

func (*failingCalendarSearch) ReplaceSearchObject(context.Context, string, string, []apps.SearchDocument) error {
	return errors.New("replace failed")
}

func (*failingCalendarSearch) DeleteSearchObject(context.Context, string, string) error {
	return errors.New("delete failed")
}

func (s *failingCalendarSearch) MarkSearchDirty(string) {
	s.dirty++
}

type failingCalendarSink struct{}

func (failingCalendarSink) AddSearchDocument(context.Context, apps.SearchDocument) error {
	return errors.New("sink failed")
}

func TestCalendarSearchFailureRecovery(t *testing.T) {
	a := newSQLTestApp(t)
	a.ensureSQLDefault("alice")
	event := Event{CalendarID: "default", ID: "search-boundary", Title: "", Description: "indexed"}
	if err := a.sqlWriteEvent("alice", event); err != nil {
		t.Fatal(err)
	}
	writer := &failingCalendarSearch{}
	a.search = writer
	a.replaceEventSearch("alice", event)
	a.deleteEventSearch("alice", "default", event.ID)
	if writer.dirty != 2 {
		t.Fatalf("dirty marks=%d", writer.dirty)
	}
	if err := a.IndexSearch(context.Background(), "alice", failingCalendarSink{}); err == nil {
		t.Fatal("failing search sink succeeded")
	}
	doc := a.eventSearchDocument("alice", event)
	if doc.Title != "Untitled event" || doc.ModifiedAt == 0 {
		t.Fatalf("search document=%#v", doc)
	}
}

func TestCalendarEdgeHelpersHealthAndBirthdays(t *testing.T) {
	anonymous := httptest.NewRecorder()
	handlePage(anonymous, httptest.NewRequest(http.MethodGet, "/calendar", nil))
	if anonymous.Code != http.StatusSeeOther {
		t.Fatalf("anonymous page status=%d", anonymous.Code)
	}
	a := newSQLTestApp(t)
	if path := a.defaultCalendarDir("alice"); filepath.Base(path) != "default" {
		t.Fatalf("default calendar dir=%q", path)
	}
	if got, err := a.parseRRuleStripUntil("FREQ=DAILY;COUNT=4;UNTIL=20300101T000000Z;INTERVAL=2"); err != nil || got != "FREQ=DAILY;INTERVAL=2" {
		t.Fatalf("stripped rule=%q err=%v", got, err)
	}
	if got, err := a.parseRRuleStripUntil(""); err != nil || got != "" {
		t.Fatalf("empty stripped rule=%q err=%v", got, err)
	}

	health := httptest.NewRecorder()
	a.handleHealth(health, withTestUser(httptest.NewRequest(http.MethodGet, "/health", nil), "alice"))
	if health.Code != http.StatusOK || !strings.Contains(health.Body.String(), `"status":"ok"`) {
		t.Fatalf("health status=%d body=%s", health.Code, health.Body.String())
	}

	events := sortedEvents([]Event{{ID: "late", Start: 20}, {ID: "early", Start: 10}})
	if events[0].ID != "early" {
		t.Fatalf("sorted events=%+v", events)
	}
	if _, ok := birthdayDate("not-a-date"); ok {
		t.Fatal("invalid birthday accepted")
	}
	if when, ok := birthdayDate("1815-12-10"); !ok || when.Year() != 1815 {
		t.Fatalf("birthday=%v ok=%v", when, ok)
	}

	book := filepath.Join(a.dataRoot, "alice", "addressbooks", "personal")
	if err := os.MkdirAll(book, 0o700); err != nil {
		t.Fatal(err)
	}
	valid := "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ada Lovelace\r\nUID:ada\r\nBDAY:1815-12-10\r\nEND:VCARD\r\n"
	fallback := "BEGIN:VCARD\r\nVERSION:3.0\r\nBDAY:1912-06-23\r\nEND:VCARD\r\n"
	for name, data := range map[string]string{"ada.vcf": valid, "turing.vcf": fallback, "bad.vcf": "broken"} {
		if err := os.WriteFile(filepath.Join(book, name), []byte(data), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Mkdir(filepath.Join(book, "directory.vcf"), 0o700); err != nil {
		t.Fatal(err)
	}
	got := a.birthdayEvents("alice")
	if len(got) != 2 || got[0].CalendarID != "birthdays" || !got[0].AllDay {
		t.Fatalf("birthday events=%+v", got)
	}
}

func TestCalendarSQLiteImportAndInvalidImportRequests(t *testing.T) {
	a := newSQLTestApp(t)
	const ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:sql-import\r\nDTSTART:20260729T090000Z\r\nDTEND:20260729T100000Z\r\nSUMMARY:SQL Import\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
	var body bytes.Buffer
	writer := multipart.NewWriter(&body)
	part, err := writer.CreateFormFile("file", "events.ics")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := part.Write([]byte(ics)); err != nil {
		t.Fatal(err)
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	req := withTestUser(httptest.NewRequest(http.MethodPost, "/api/calendar/import", &body), "alice")
	req.Header.Set("Content-Type", writer.FormDataContentType())
	rec := httptest.NewRecorder()
	a.handleImport(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"imported":1`) {
		t.Fatalf("import status=%d body=%s", rec.Code, rec.Body.String())
	}
	events := a.listAllEvents("alice")
	if len(events) != 1 || events[0].Title != "SQL Import" {
		t.Fatalf("imported events=%+v", events)
	}

	for _, tc := range []struct {
		method      string
		contentType string
		body        string
		status      int
	}{
		{http.MethodGet, "", "", http.StatusMethodNotAllowed},
		{http.MethodPost, "text/plain", "not multipart", http.StatusBadRequest},
	} {
		req = withTestUser(httptest.NewRequest(tc.method, "/api/calendar/import", strings.NewReader(tc.body)), "alice")
		req.Header.Set("Content-Type", tc.contentType)
		rec = httptest.NewRecorder()
		a.handleImport(rec, req)
		if rec.Code != tc.status {
			t.Fatalf("%s import status=%d want=%d", tc.method, rec.Code, tc.status)
		}
	}

	a.markSearchDirty()
	if err := a.Shutdown(context.Background()); err != nil {
		t.Fatalf("shutdown without worker: %v", err)
	}
}

func TestCalendarRecurringUpdateScopesAndFailures(t *testing.T) {
	a := newSQLTestApp(t)
	base := Event{
		ID: "series", CalendarID: "default", Title: "Series", Start: 1_800_000_000,
		End: 1_800_003_600, RRule: "FREQ=DAILY;COUNT=10", Busy: true,
		Location: "Room", Reminders: []int{10}, Status: "CONFIRMED",
	}
	if err := a.writeEvent("alice", base); err != nil {
		t.Fatal(err)
	}
	call := func(method string, value any) *httptest.ResponseRecorder {
		var raw []byte
		switch body := value.(type) {
		case []byte:
			raw = body
		default:
			raw, _ = json.Marshal(body)
		}
		req := withTestUser(httptest.NewRequest(method, "/api/calendar/update", bytes.NewReader(raw)), "alice")
		rec := httptest.NewRecorder()
		a.handleUpdate(rec, req)
		return rec
	}
	for _, tc := range []struct {
		method string
		body   any
		status int
	}{
		{http.MethodGet, Event{}, http.StatusMethodNotAllowed},
		{http.MethodPost, []byte("{"), http.StatusBadRequest},
		{http.MethodPost, Event{Title: "missing"}, http.StatusBadRequest},
		{http.MethodPost, Event{ID: "missing", Title: "missing"}, http.StatusNotFound},
	} {
		if rec := call(tc.method, tc.body); rec.Code != tc.status {
			t.Fatalf("%s update status=%d want=%d body=%s", tc.method, rec.Code, tc.status, rec.Body.String())
		}
	}

	recurrence := base.Start + 86400
	this := base
	this.ID = "series@" + fmt.Sprint(recurrence)
	this.MasterID = "series"
	this.RecurrenceID = recurrence
	this.Scope = "this"
	this.Start, this.End, this.Title = recurrence, recurrence+7200, "Changed occurrence"
	if rec := call(http.MethodPost, this); rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Changed occurrence") {
		t.Fatalf("this update status=%d body=%s", rec.Code, rec.Body.String())
	}
	master, _, err := a.findEvent("alice", "series")
	if err != nil || !a.hasExDate(master.ExDates, recurrence, false) {
		t.Fatalf("master after this=%+v err=%v", master, err)
	}

	futureBase := base
	futureBase.ID = "future-series"
	futureBase.RRule = "FREQ=DAILY;COUNT=10;UNTIL=20300101T000000Z"
	if err := a.writeEvent("alice", futureBase); err != nil {
		t.Fatal(err)
	}
	future := futureBase
	future.MasterID = futureBase.ID
	future.ID = futureBase.ID + "@" + fmt.Sprint(recurrence)
	future.RecurrenceID = recurrence
	future.Scope = "future"
	future.Start, future.End, future.Title = recurrence, recurrence+3600, "Future split"
	future.RRule = ""
	if rec := call(http.MethodPost, future); rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Future split") {
		t.Fatalf("future update status=%d body=%s", rec.Code, rec.Body.String())
	}

	firstBase := base
	firstBase.ID = "first-series"
	if err := a.writeEvent("alice", firstBase); err != nil {
		t.Fatal(err)
	}
	first := firstBase
	first.ID = firstBase.ID + "@" + fmt.Sprint(firstBase.Start)
	first.MasterID = firstBase.ID
	first.RecurrenceID = firstBase.Start
	first.Scope = "future"
	first.Title = "Whole series"
	if rec := call(http.MethodPost, first); rec.Code != http.StatusOK {
		t.Fatalf("first future status=%d body=%s", rec.Code, rec.Body.String())
	}

	birthday := Event{ID: "generated", CalendarID: "birthdays", Title: "Birthday", Start: base.Start, End: base.End}
	if err := a.sqlSaveCalendar("alice", CalendarMeta{ID: "birthdays", Name: "Birthdays", Visible: true, ReadOnly: true}); err != nil {
		t.Fatal(err)
	}
	if err := a.writeEvent("alice", birthday); err != nil {
		t.Fatal(err)
	}
	birthday.Title = "Blocked"
	if rec := call(http.MethodPost, birthday); rec.Code != http.StatusForbidden {
		t.Fatalf("birthday update status=%d body=%s", rec.Code, rec.Body.String())
	}
}

func TestCalendarHandlerMethodValidationAndClosedDatabase(t *testing.T) {
	app := newSQLTestApp(t)
	for _, tc := range []struct {
		handler http.HandlerFunc
		method  string
		target  string
		body    string
	}{
		{app.handleCalendars, http.MethodPatch, "/api/calendar/calendars", ""},
		{app.handleCalendarUpdate, http.MethodGet, "/api/calendar/calendars/update", ""},
		{app.handleCalendarDelete, http.MethodGet, "/api/calendar/calendars/delete", ""},
		{app.handleList, http.MethodPost, "/api/calendar/list", ""},
		{app.handleGet, http.MethodPost, "/api/calendar/get", ""},
		{app.handleCreate, http.MethodGet, "/api/calendar/create", ""},
		{app.handleUpdate, http.MethodGet, "/api/calendar/update", ""},
		{app.handleDelete, http.MethodGet, "/api/calendar/delete", ""},
		{app.handleSearch, http.MethodPost, "/api/calendar/search", ""},
		{app.handleImport, http.MethodGet, "/api/calendar/import", ""},
		{app.handleExport, http.MethodPost, "/api/calendar/export", ""},
		{app.handleCalendars, http.MethodPost, "/api/calendar/calendars", `{`},
		{app.handleCalendarUpdate, http.MethodPost, "/api/calendar/calendars/update", `{`},
		{app.handleCalendarDelete, http.MethodPost, "/api/calendar/calendars/delete", `{`},
		{app.handleCreate, http.MethodPost, "/api/calendar/create", `{`},
		{app.handleUpdate, http.MethodPost, "/api/calendar/update", `{`},
		{app.handleDelete, http.MethodPost, "/api/calendar/delete", `{`},
	} {
		req := withTestUser(httptest.NewRequest(tc.method, tc.target, strings.NewReader(tc.body)), "alice")
		rec := httptest.NewRecorder()
		tc.handler(rec, req)
		if rec.Code < 200 || rec.Code >= 600 {
			t.Errorf("%s %s returned invalid status %d: %s", tc.method, tc.target, rec.Code, rec.Body.String())
		}
	}

	if err := app.db.Close(); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		handler http.HandlerFunc
		method  string
		target  string
		body    string
	}{
		{app.handleCalendars, http.MethodGet, "/api/calendar/calendars", ""},
		{app.handleCalendars, http.MethodPost, "/api/calendar/calendars", `{"name":"Closed"}`},
		{app.handleCalendarUpdate, http.MethodPost, "/api/calendar/calendars/update", `{"id":"default","name":"Closed"}`},
		{app.handleCalendarDelete, http.MethodPost, "/api/calendar/calendars/delete", `{"id":"default"}`},
		{app.handleSubscriptions, http.MethodGet, "/api/calendar/subscriptions", ""},
		{app.handleSubscriptions, http.MethodPost, "/api/calendar/subscriptions", `{"name":"Closed","url":"https://example.test/calendar.ics"}`},
		{app.handleSubscriptionRefresh, http.MethodPost, "/api/calendar/subscriptions/refresh?id=1", ""},
	} {
		req := withTestUser(httptest.NewRequest(tc.method, tc.target, strings.NewReader(tc.body)), "alice")
		rec := httptest.NewRecorder()
		tc.handler(rec, req)
		if rec.Code < 200 || rec.Code >= 600 {
			t.Errorf("closed DB %s %s returned invalid status %d: %s", tc.method, tc.target, rec.Code, rec.Body.String())
		}
	}
	health := httptest.NewRecorder()
	app.handleHealth(health, withTestUser(httptest.NewRequest(http.MethodGet, "/health", nil), "alice"))
	if health.Code != http.StatusOK {
		t.Fatalf("closed DB health=%d %s", health.Code, health.Body.String())
	}
}

func TestCalendarAlarmAndICSParserEdgeMatrix(t *testing.T) {
	app := newSQLTestApp(t)
	for _, tc := range []struct {
		value string
		want  int
		ok    bool
	}{
		{"", 0, false},
		{"20260729T120000Z", 0, false},
		{"-P1W", 10080, true},
		{"-P2DT3H", 3060, true},
		{"-PT1H30M15S", 90, true},
		{"PT15M", 0, true},
		{"-PT1X", 0, false},
		{"-PT", 0, false},
	} {
		got, ok := app.parseAlarmTriggerMinutes(tc.value)
		if got != tc.want || ok != tc.ok {
			t.Errorf("parseAlarmTriggerMinutes(%q)=%d,%v want %d,%v", tc.value, got, ok, tc.want, tc.ok)
		}
	}
	for input, want := range map[int]string{0: "-PT0M", -1: "-PT0M", 15: "-PT15M", 120: "-PT2H", 2880: "-P2D"} {
		if got := app.formatAlarmTrigger(input); got != want {
			t.Errorf("formatAlarmTrigger(%d)=%q want %q", input, got, want)
		}
	}
	for _, value := range []string{"20260729", "20260729T120000", "20260729T120000Z"} {
		if app.parseICSDate(value) == 0 {
			t.Errorf("valid ICS date %q rejected", value)
		}
	}
	for _, value := range []string{"bad", "20261399", "20260729T999999Z"} {
		if app.parseICSDate(value) != 0 {
			t.Errorf("invalid ICS date %q accepted", value)
		}
	}
	for _, data := range []string{
		"not calendar data",
		"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n",
		"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:no-start\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
	} {
		if _, err := app.parseICSBytes([]byte(data), "event", "default"); err == nil {
			t.Errorf("invalid ICS accepted: %q", data)
		}
	}
}

func TestCalendarRESTValidationDefaultsAndMissingResourceMatrix(t *testing.T) {
	app := newSQLTestApp(t)
	call := func(handler http.HandlerFunc, method, target, body string) *httptest.ResponseRecorder {
		t.Helper()
		recorder := httptest.NewRecorder()
		handler(recorder, withTestUser(httptest.NewRequest(method, target, strings.NewReader(body)), "alice"))
		return recorder
	}
	for _, tc := range []struct {
		name    string
		handler http.HandlerFunc
		method  string
		target  string
		body    string
		want    int
	}{
		{"calendar malformed", app.handleCalendars, http.MethodPost, "/api/calendar/calendars", `{`, http.StatusBadRequest},
		{"calendar invalid name", app.handleCalendars, http.MethodPost, "/api/calendar/calendars", `{"name":"..."}`, http.StatusBadRequest},
		{"calendar method", app.handleCalendars, http.MethodPatch, "/api/calendar/calendars", ``, http.StatusMethodNotAllowed},
		{"calendar update method", app.handleCalendarUpdate, http.MethodGet, "/api/calendar/calendars/update", ``, http.StatusMethodNotAllowed},
		{"calendar update malformed", app.handleCalendarUpdate, http.MethodPost, "/api/calendar/calendars/update", `{`, http.StatusBadRequest},
		{"calendar update missing id", app.handleCalendarUpdate, http.MethodPost, "/api/calendar/calendars/update", `{}`, http.StatusBadRequest},
		{"calendar update birthdays", app.handleCalendarUpdate, http.MethodPost, "/api/calendar/calendars/update", `{"id":"birthdays"}`, http.StatusForbidden},
		{"calendar update missing", app.handleCalendarUpdate, http.MethodPost, "/api/calendar/calendars/update", `{"id":"missing","name":"Missing"}`, http.StatusNotFound},
		{"calendar delete method", app.handleCalendarDelete, http.MethodGet, "/api/calendar/calendars/delete", ``, http.StatusMethodNotAllowed},
		{"calendar delete empty", app.handleCalendarDelete, http.MethodPost, "/api/calendar/calendars/delete", ``, http.StatusBadRequest},
		{"calendar delete default", app.handleCalendarDelete, http.MethodPost, "/api/calendar/calendars/delete?id=default", ``, http.StatusBadRequest},
		{"calendar delete birthdays", app.handleCalendarDelete, http.MethodPost, "/api/calendar/calendars/delete?id=birthdays", ``, http.StatusBadRequest},
		{"event get missing", app.handleGet, http.MethodGet, "/api/calendar/get?id=missing", ``, http.StatusNotFound},
		{"event get explicit missing", app.handleGet, http.MethodGet, "/api/calendar/get?id=missing&calendar=default", ``, http.StatusNotFound},
		{"event create method", app.handleCreate, http.MethodGet, "/api/calendar/create", ``, http.StatusMethodNotAllowed},
		{"event create malformed", app.handleCreate, http.MethodPost, "/api/calendar/create", `{`, http.StatusBadRequest},
		{"event create birthday", app.handleCreate, http.MethodPost, "/api/calendar/create", `{"calendar":"birthdays","title":"No"}`, http.StatusForbidden},
		{"event update method", app.handleUpdate, http.MethodGet, "/api/calendar/update", ``, http.StatusMethodNotAllowed},
		{"event update malformed", app.handleUpdate, http.MethodPost, "/api/calendar/update", `{`, http.StatusBadRequest},
		{"event update missing id", app.handleUpdate, http.MethodPost, "/api/calendar/update", `{}`, http.StatusBadRequest},
		{"event update missing", app.handleUpdate, http.MethodPost, "/api/calendar/update", `{"id":"missing","title":"Missing"}`, http.StatusNotFound},
		{"event delete method", app.handleDelete, http.MethodGet, "/api/calendar/delete", ``, http.StatusMethodNotAllowed},
		{"event delete missing", app.handleDelete, http.MethodPost, "/api/calendar/delete?id=missing", ``, http.StatusOK},
		{"event delete explicit", app.handleDelete, http.MethodPost, "/api/calendar/delete?id=missing&calendar=default", ``, http.StatusOK},
		{"import method", app.handleImport, http.MethodGet, "/api/calendar/import", ``, http.StatusMethodNotAllowed},
		{"export missing calendar", app.handleExport, http.MethodGet, "/api/calendar/export?calendar=missing", ``, http.StatusOK},
		{"subscription method", app.handleSubscriptions, http.MethodPatch, "/api/calendar/subscriptions", ``, http.StatusMethodNotAllowed},
		{"subscription malformed", app.handleSubscriptions, http.MethodPost, "/api/calendar/subscriptions", `{`, http.StatusBadRequest},
		{"subscription empty url", app.handleSubscriptions, http.MethodPost, "/api/calendar/subscriptions", `{"name":"Feed"}`, http.StatusBadRequest},
		{"subscription scheme", app.handleSubscriptions, http.MethodPost, "/api/calendar/subscriptions", `{"name":"Feed","url":"ftp://example.test/feed.ics"}`, http.StatusBadRequest},
		{"subscription blocked target", app.handleSubscriptions, http.MethodPost, "/api/calendar/subscriptions", `{"name":"Feed","url":"http://[::1"}`, http.StatusBadRequest},
		{"subscription refresh missing id", app.handleSubscriptionRefresh, http.MethodPost, "/api/calendar/subscriptions/refresh", ``, http.StatusBadRequest},
		{"subscription refresh missing", app.handleSubscriptionRefresh, http.MethodPost, "/api/calendar/subscriptions/refresh?id=missing", ``, http.StatusBadGateway},
	} {
		t.Run(tc.name, func(t *testing.T) {
			recorder := call(tc.handler, tc.method, tc.target, tc.body)
			if recorder.Code != tc.want {
				t.Fatalf("status=%d want=%d body=%s", recorder.Code, tc.want, recorder.Body.String())
			}
		})
	}

	created := call(app.handleCalendars, http.MethodPost, "/api/calendar/calendars", `{"name":"Work"}`)
	if created.Code != http.StatusOK {
		t.Fatalf("create calendar status=%d body=%s", created.Code, created.Body.String())
	}
	duplicate := call(app.handleCalendars, http.MethodPost, "/api/calendar/calendars", `{"name":"Work"}`)
	if duplicate.Code != http.StatusConflict {
		t.Fatalf("duplicate calendar status=%d body=%s", duplicate.Code, duplicate.Body.String())
	}
	updated := call(app.handleCalendarUpdate, http.MethodPost, "/api/calendar/calendars/update", `{"id":"work","name":"Renamed","color":"#123456","visible":true,"order":2}`)
	if updated.Code != http.StatusOK || !strings.Contains(updated.Body.String(), "Renamed") {
		t.Fatalf("update calendar status=%d body=%s", updated.Code, updated.Body.String())
	}
	deleted := call(app.handleCalendarDelete, http.MethodPost, "/api/calendar/calendars/delete?id=work", ``)
	if deleted.Code != http.StatusOK {
		t.Fatalf("delete calendar status=%d body=%s", deleted.Code, deleted.Body.String())
	}

	event := call(app.handleCreate, http.MethodPost, "/api/calendar/create", `{"title":"Defaults","start":100}`)
	if event.Code != http.StatusOK {
		t.Fatalf("create default event status=%d body=%s", event.Code, event.Body.String())
	}
	var decoded Event
	if err := json.Unmarshal(event.Body.Bytes(), &decoded); err != nil {
		t.Fatal(err)
	}
	if decoded.End != 3700 || !decoded.Busy || decoded.Status != "CONFIRMED" || decoded.CalendarID != "default" {
		t.Fatalf("event defaults=%+v", decoded)
	}
	search := call(app.handleSearch, http.MethodGet, "/api/calendar/search?q=defaults", ``)
	if search.Code != http.StatusOK || !strings.Contains(search.Body.String(), decoded.ID) {
		t.Fatalf("search status=%d body=%s", search.Code, search.Body.String())
	}
	list := call(app.handleList, http.MethodGet, "/api/calendar/list?calendar=default&start=1&end=5000", ``)
	if list.Code != http.StatusOK || !strings.Contains(list.Body.String(), decoded.ID) {
		t.Fatalf("range list status=%d body=%s", list.Code, list.Body.String())
	}

	noDB := New(Config{DataRoot: t.TempDir()})
	recorder := call(noDB.handleSubscriptions, http.MethodGet, "/api/calendar/subscriptions", ``)
	if recorder.Code != http.StatusServiceUnavailable {
		t.Fatalf("database-free subscriptions status=%d", recorder.Code)
	}
	noDB.refreshAllSubscriptions(context.Background())
}

func TestCalendarFilesystemImportExportCalDAVAndRecurringDeleteMatrix(t *testing.T) {
	app := New(Config{DataRoot: t.TempDir()})
	callDAV := func(method, target, body string) *httptest.ResponseRecorder {
		req := withTestUser(httptest.NewRequest(method, target, strings.NewReader(body)), "alice")
		rec := httptest.NewRecorder()
		app.handleCalDAV(rec, req)
		return rec
	}
	const eventICS = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:filesystem-event\r\nDTSTART:20310101T090000Z\r\nDTEND:20310101T100000Z\r\nSUMMARY:Filesystem event\r\nDESCRIPTION:Protocol matrix\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
	for _, tc := range []struct {
		method string
		target string
		body   string
		status int
	}{
		{"MKCALENDAR", "/caldav/alice/filesystem", "", http.StatusCreated},
		{http.MethodPut, "/caldav/alice/filesystem/filesystem-event.ics", eventICS, http.StatusCreated},
		{http.MethodGet, "/caldav/alice/filesystem/filesystem-event.ics", "", http.StatusOK},
		{"PROPFIND", "/caldav/alice", "", http.StatusMultiStatus},
		{"PROPFIND", "/caldav/alice/filesystem", "", http.StatusMultiStatus},
		{"REPORT", "/caldav/alice/filesystem", `<cal:calendar-query><cal:time-range start="20300101T000000Z" end="20320101T000000Z"/></cal:calendar-query>`, http.StatusMultiStatus},
		{"REPORT", "/caldav/alice/filesystem", `<cal:calendar-query><cal:time-range start="bad" end="bad"/></cal:calendar-query>`, http.StatusMultiStatus},
		{http.MethodGet, "/caldav/alice/filesystem/missing.ics", "", http.StatusNotFound},
		{http.MethodDelete, "/caldav/alice/filesystem/filesystem-event.ics", "", http.StatusNoContent},
		{http.MethodDelete, "/caldav/alice/filesystem", "", http.StatusBadRequest},
		{http.MethodDelete, "/caldav/alice/filesystem/../escape.ics", "", http.StatusBadRequest},
		{"MKCALENDAR", "/caldav/alice", "", http.StatusBadRequest},
	} {
		rec := callDAV(tc.method, tc.target, tc.body)
		if rec.Code != tc.status {
			t.Errorf("%s %s status=%d want=%d body=%s", tc.method, tc.target, rec.Code, tc.status, rec.Body.String())
		}
	}

	var multipartBody bytes.Buffer
	writer := multipart.NewWriter(&multipartBody)
	part, err := writer.CreateFormFile("file", "calendar.ics")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := part.Write([]byte(eventICS)); err != nil {
		t.Fatal(err)
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	req := withTestUser(httptest.NewRequest(http.MethodPost, "/api/calendar/import?calendar=filesystem", &multipartBody), "alice")
	req.Header.Set("Content-Type", writer.FormDataContentType())
	rec := httptest.NewRecorder()
	app.handleImport(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"imported":1`) {
		t.Fatalf("filesystem import status=%d body=%s", rec.Code, rec.Body.String())
	}
	req = withTestUser(httptest.NewRequest(http.MethodGet, "/api/calendar/export?calendar=filesystem", nil), "alice")
	rec = httptest.NewRecorder()
	app.handleExport(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Filesystem event") {
		t.Fatalf("filesystem export status=%d body=%s", rec.Code, rec.Body.String())
	}
	events := app.listEventsInCalendar("alice", "filesystem")
	if len(events) != 1 {
		t.Fatalf("filesystem events=%+v", events)
	}
	req = withTestUser(httptest.NewRequest(http.MethodGet, "/api/calendar/get?id="+events[0].ID+"&calendar=filesystem", nil), "alice")
	rec = httptest.NewRecorder()
	app.handleGet(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Filesystem event") {
		t.Fatalf("explicit get status=%d body=%s", rec.Code, rec.Body.String())
	}

	invalidBodies := []string{
		"not a calendar",
		"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n",
	}
	for _, raw := range invalidBodies {
		multipartBody.Reset()
		writer = multipart.NewWriter(&multipartBody)
		part, err = writer.CreateFormFile("file", "bad.ics")
		if err != nil {
			t.Fatal(err)
		}
		_, _ = part.Write([]byte(raw))
		_ = writer.Close()
		req = withTestUser(httptest.NewRequest(http.MethodPost, "/api/calendar/import", &multipartBody), "alice")
		req.Header.Set("Content-Type", writer.FormDataContentType())
		rec = httptest.NewRecorder()
		app.handleImport(rec, req)
		if rec.Code != http.StatusBadRequest {
			t.Errorf("invalid filesystem import status=%d body=%s", rec.Code, rec.Body.String())
		}
	}

	series := Event{
		ID: "delete-series", CalendarID: "default", Title: "Delete series",
		Start: 1_900_000_000, End: 1_900_003_600, RRule: "FREQ=DAILY;COUNT=10", Busy: true,
	}
	if err := app.writeEvent("alice", series); err != nil {
		t.Fatal(err)
	}
	recurrence := series.Start + 86400
	for _, scope := range []string{"this", "future"} {
		req = withTestUser(httptest.NewRequest(http.MethodPost,
			"/api/calendar/delete?id=delete-series@"+fmt.Sprint(recurrence)+"&scope="+scope, nil), "alice")
		rec = httptest.NewRecorder()
		app.handleDelete(rec, req)
		if rec.Code != http.StatusOK {
			t.Fatalf("delete %s status=%d body=%s", scope, rec.Code, rec.Body.String())
		}
	}
	req = withTestUser(httptest.NewRequest(http.MethodPost, "/api/calendar/delete?id=missing", nil), "alice")
	rec = httptest.NewRecorder()
	app.handleDelete(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("idempotent missing delete status=%d", rec.Code)
	}
}

func TestCalendarRecurrenceBoundsAndSubscriptionFieldMatrix(t *testing.T) {
	app := New(Config{DataRoot: t.TempDir()})
	if got := app.expandInRange(nil, 20, 10); len(got) != 0 {
		t.Fatalf("invalid range expanded=%+v", got)
	}
	if got := app.expandInRange([]Event{{ID: "outside", Start: 1, End: 2}}, 10, 20); len(got) != 0 {
		t.Fatalf("outside event expanded=%+v", got)
	}
	start := int64(1_900_000_000)
	invalid := Event{ID: "invalid", Start: start, End: start + 3600, RRule: "not-a-rule"}
	if got := app.expandRecurring(invalid, start-1, start+7200); len(got) != 1 {
		t.Fatalf("overlapping invalid recurrence=%+v", got)
	}
	if got := app.expandRecurring(invalid, start+7200, start+9000); len(got) != 0 {
		t.Fatalf("non-overlapping invalid recurrence=%+v", got)
	}
	timed := Event{ID: "timed", Start: start, End: start, RRule: "FREQ=DAILY;COUNT=3"}
	if got := app.expandRecurring(timed, start-1, start+4*86400); len(got) != 3 || got[0].End-got[0].Start != 3600 {
		t.Fatalf("zero-duration timed recurrence=%+v", got)
	}
	allDay := Event{ID: "day", Start: start, End: start, AllDay: true, RRule: "FREQ=DAILY;COUNT=3", ExDates: []int64{start + 86400}}
	got := app.expandRecurring(allDay, start-86400, start+5*86400)
	if len(got) == 0 || got[0].End-got[0].Start != 86400 {
		t.Fatalf("all-day recurrence=%+v", got)
	}
	if normalized := app.normalizeExDate(start+1234, true); normalized == start+1234 {
		t.Fatal("all-day exception date was not normalized")
	}
	if list := app.appendExDate([]int64{start}, start, false); len(list) != 1 {
		t.Fatalf("duplicate exception appended=%v", list)
	}
	if value := app.rruleWithUntil("invalid", start, false); value != "invalid" {
		t.Fatalf("invalid until rule=%q", value)
	}
	if value := app.rruleWithUntil("FREQ=DAILY;COUNT=5", start+86400, true); !strings.Contains(value, "UNTIL=") || strings.Contains(value, "COUNT=") {
		t.Fatalf("all-day until rule=%q", value)
	}
	if app.masterIDFrom("plain@not-a-time") != "plain@not-a-time" || app.masterIDFrom("master@123") != "master" {
		t.Fatal("master ID parsing regression")
	}
	large := make([]Event, maxRecurrenceInstances+10)
	for i := range large {
		large[i] = Event{ID: fmt.Sprint(i), Start: start, End: start + 1}
	}
	if expanded := app.expandInRange(large, start-1, start+2); len(expanded) != maxRecurrenceInstances {
		t.Fatalf("recurrence cap=%d", len(expanded))
	}
	if expanded := app.expandInRange([]Event{timed}, start, start+maxRecurrenceRange+86400); len(expanded) == 0 {
		t.Fatal("clamped recurrence range produced no instances")
	}

	component := ical.NewEvent()
	component.Props.SetText(ical.PropUID, "rich-remote")
	component.Props.SetText(ical.PropSummary, "Remote title")
	component.Props.SetText(ical.PropDescription, "Remote details")
	component.Props.SetText(ical.PropLocation, "Remote room")
	component.Props.SetText(ical.PropStatus, "TENTATIVE")
	component.Props.SetText(ical.PropTransparency, "TRANSPARENT")
	component.Props.SetText(ical.PropRecurrenceRule, "FREQ=DAILY")
	component.Props.SetText(ical.PropDateTimeStart, "20300101T120000Z")
	component.Props.SetText(ical.PropDateTimeEnd, "20300101T120000Z")
	categories := ical.NewProp(ical.PropCategories)
	categories.SetTextList([]string{"one", "two"})
	component.Props.Set(categories)
	exdates := ical.NewProp(ical.PropExceptionDates)
	exdates.Value = "20300102T120000Z,invalid"
	component.Props.Set(exdates)
	alarm := ical.NewComponent(ical.CompAlarm)
	alarm.Props.SetText(ical.PropTrigger, "-PT15M")
	component.Children = append(component.Children, alarm)
	event, ok := app.subscriptionEvent(*component, "remote", 3)
	if !ok || event.ID != "rich-remote" || event.Title != "Remote title" || event.Description != "Remote details" ||
		event.Location != "Remote room" || event.Status != "TENTATIVE" || event.Busy || event.End-event.Start != 60 ||
		len(event.Categories) != 2 || len(event.ExDates) != 1 || len(event.Reminders) != 1 {
		t.Fatalf("rich subscription event=%+v ok=%v", event, ok)
	}
	noStart := ical.NewEvent()
	if _, ok := app.subscriptionEvent(*noStart, "remote", 4); ok {
		t.Fatal("subscription event without DTSTART accepted")
	}
	day := ical.NewEvent()
	day.Props.SetText(ical.PropDateTimeStart, "20300101")
	dayEvent, ok := app.subscriptionEvent(*day, "remote", 5)
	if !ok || !dayEvent.AllDay || dayEvent.End-dayEvent.Start != 86400 || dayEvent.ID != "remote-5" {
		t.Fatalf("all-day subscription=%+v ok=%v", dayEvent, ok)
	}
}

func TestCalendarSQLiteCalDAVValidationAndCollectionMatrix(t *testing.T) {
	app := newSQLTestApp(t)
	call := func(method, target, body string) *httptest.ResponseRecorder {
		t.Helper()
		req := withTestUser(httptest.NewRequest(method, target, strings.NewReader(body)), "alice")
		rec := httptest.NewRecorder()
		app.handleCalDAV(rec, req)
		return rec
	}
	const eventICS = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:sqlite-dav\r\nDTSTART:20320101T090000Z\r\nDTEND:20320101T100000Z\r\nSUMMARY:SQLite DAV\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
	for _, tc := range []struct {
		method, target, body string
		status               int
	}{
		{"OPTIONS", "/caldav/alice", "", http.StatusOK},
		{"PATCH", "/caldav/alice", "", http.StatusMethodNotAllowed},
		{"PROPFIND", "/caldav/alice", "", http.StatusMultiStatus},
		{"MKCALENDAR", "/caldav/alice/work", "", http.StatusCreated},
		{http.MethodPut, "/caldav/alice/work", "", http.StatusBadRequest},
		{http.MethodPut, "/caldav/alice/work/../escape.ics", eventICS, http.StatusBadRequest},
		{http.MethodPut, "/caldav/alice/work/event.txt", eventICS, http.StatusBadRequest},
		{http.MethodPut, "/caldav/alice/work/empty.ics", "", http.StatusBadRequest},
		{http.MethodPut, "/caldav/alice/work/invalid.ics", "not an icalendar", http.StatusBadRequest},
		{http.MethodPut, "/caldav/alice/work/sqlite-dav.ics", eventICS, http.StatusCreated},
		{http.MethodGet, "/caldav/alice/work/sqlite-dav.ics", "", http.StatusOK},
		{http.MethodGet, "/caldav/alice/work/missing.ics", "", http.StatusNotFound},
		{http.MethodGet, "/caldav/alice/work", "", http.StatusBadRequest},
		{http.MethodGet, "/caldav/alice/work/../escape.ics", "", http.StatusBadRequest},
		{"PROPFIND", "/caldav/alice/work", "", http.StatusMultiStatus},
		{"REPORT", "/caldav/alice/work", `<cal:calendar-query><cal:time-range start="20310101T000000Z" end="20330101T000000Z"/></cal:calendar-query>`, http.StatusMultiStatus},
		{http.MethodDelete, "/caldav/alice/work/sqlite-dav.ics", "", http.StatusNoContent},
		{http.MethodDelete, "/caldav/alice/work", "", http.StatusBadRequest},
		{http.MethodDelete, "/caldav/alice/work/../escape.ics", "", http.StatusBadRequest},
		{"PROPFIND", "/caldav/bob", "", http.StatusNotFound},
	} {
		rec := call(tc.method, tc.target, tc.body)
		if rec.Code != tc.status {
			t.Errorf("%s %s status=%d want=%d body=%s", tc.method, tc.target, rec.Code, tc.status, rec.Body.String())
		}
	}
}
