package calendar

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
)

type calendarSearchSink struct {
	docs []apps.SearchDocument
	err  error
}

func (s *calendarSearchSink) AddSearchDocument(_ context.Context, doc apps.SearchDocument) error {
	if s.err != nil {
		return s.err
	}
	s.docs = append(s.docs, doc)
	return nil
}

func TestCalendarCalDAVStoreSearchAndLifecycle(t *testing.T) {
	app := newSQLTestApp(t)
	collection, err := app.CreateCollection("alice", "Engineering")
	if err != nil {
		t.Fatal(err)
	}
	if collection.ID != "engineering" || collection.Name != "Engineering" {
		t.Fatalf("collection = %#v", collection)
	}
	if _, err = app.CreateCollection("alice", "../"); err == nil {
		t.Fatal("invalid collection name accepted")
	}

	ics := "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:release\r\nDTSTART:20270101T150000Z\r\nDTEND:20270101T160000Z\r\nSUMMARY:Release review\r\nDESCRIPTION:Searchable launch plan\r\nLOCATION:Room 3\r\nCATEGORIES:quality,release\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
	object, err := app.PutObject("alice", collection.ID, "release.ics", ics)
	if err != nil {
		t.Fatal(err)
	}
	if object.Href != "release.ics" || object.ETag == "" {
		t.Fatalf("put object = %#v", object)
	}
	if _, err = app.PutObject("alice", collection.ID, "bad.ics", "not an event"); err == nil {
		t.Fatal("malformed calendar object accepted")
	}

	collections, err := app.Collections("alice")
	if err != nil || len(collections) < 2 {
		t.Fatalf("collections = %#v, %v", collections, err)
	}
	objects, err := app.Objects("alice", collection.ID)
	if err != nil || len(objects) != 1 {
		t.Fatalf("objects = %#v, %v", objects, err)
	}
	read, err := app.Object("alice", collection.ID, "release.ics")
	if err != nil || !strings.Contains(read.Data, "Release review") {
		t.Fatalf("object = %#v, %v", read, err)
	}
	if _, err = app.Object("bob", collection.ID, "release.ics"); err == nil {
		t.Fatal("cross-user calendar object was readable")
	}

	sink := &calendarSearchSink{}
	if err = app.IndexSearch(context.Background(), "alice", sink); err != nil {
		t.Fatal(err)
	}
	if len(sink.docs) != 1 || sink.docs[0].ObjectID != collection.ID+":release" ||
		!strings.Contains(sink.docs[0].Keywords, "Room 3") {
		t.Fatalf("search documents = %#v", sink.docs)
	}
	sink.err = context.Canceled
	if err = app.IndexSearch(context.Background(), "alice", sink); !errors.Is(err, context.Canceled) {
		t.Fatalf("search sink error = %v", err)
	}

	if got := firstCalendarTitle(" "); got != "Untitled event" {
		t.Fatalf("empty title = %q", got)
	}
	if got := firstCalendarTitle("Planning"); got != "Planning" {
		t.Fatalf("title = %q", got)
	}

	ctx, cancel := context.WithCancel(context.Background())
	if err = app.Start(ctx); err != nil {
		t.Fatal(err)
	}
	cancel()
	shutdownCtx, shutdownCancel := context.WithTimeout(context.Background(), time.Second)
	defer shutdownCancel()
	if err = app.Shutdown(shutdownCtx); err != nil {
		t.Fatal(err)
	}
	if err = New(Config{}).Start(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err = New(Config{}).Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}

	if err = app.DeleteObject("alice", collection.ID, "release.ics"); err != nil {
		t.Fatal(err)
	}
	if _, err = app.Object("alice", collection.ID, "release.ics"); err == nil {
		t.Fatal("deleted object remained readable")
	}
}

func TestLegacyCalDAVCompatibilityMatrix(t *testing.T) {
	app := newSQLTestApp(t)
	ics := "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:matrix\r\nDTSTART:20270101T150000Z\r\nDTEND:20270101T160000Z\r\nSUMMARY:Matrix event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
	call := func(method, target, body string) *httptest.ResponseRecorder {
		t.Helper()
		req := httptest.NewRequest(method, target, bytes.NewBufferString(body))
		req = withTestUser(req, "alice")
		rec := httptest.NewRecorder()
		app.handleCalDAV(rec, req)
		return rec
	}

	if rec := call("MKCALENDAR", "/caldav/alice/matrix", ""); rec.Code != http.StatusCreated {
		t.Fatalf("MKCALENDAR: %d %s", rec.Code, rec.Body.String())
	}
	if rec := call(http.MethodPut, "/caldav/alice/matrix/matrix.ics", ics); rec.Code != http.StatusCreated {
		t.Fatalf("PUT: %d %s", rec.Code, rec.Body.String())
	}
	if rec := call(http.MethodGet, "/caldav/alice/matrix/matrix.ics", ""); rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Matrix event") {
		t.Fatalf("GET: %d %s", rec.Code, rec.Body.String())
	}
	if rec := call("PROPFIND", "/caldav/alice", ""); rec.Code != http.StatusMultiStatus || !strings.Contains(rec.Body.String(), "matrix") {
		t.Fatalf("root PROPFIND: %d %s", rec.Code, rec.Body.String())
	}
	if rec := call("PROPFIND", "/caldav/alice/matrix", ""); rec.Code != http.StatusMultiStatus || !strings.Contains(rec.Body.String(), "matrix.ics") {
		t.Fatalf("calendar PROPFIND: %d %s", rec.Code, rec.Body.String())
	}
	reportBody := `<cal:calendar-query xmlns:cal="urn:ietf:params:xml:ns:caldav"><cal:filter><cal:time-range start="20260101T000000Z" end="20280101T000000Z"/></cal:filter></cal:calendar-query>`
	if rec := call("REPORT", "/caldav/alice/matrix", reportBody); rec.Code != http.StatusMultiStatus || !strings.Contains(rec.Body.String(), "Matrix event") {
		t.Fatalf("REPORT: %d %s", rec.Code, rec.Body.String())
	}
	if rec := call(http.MethodOptions, "/caldav/alice/matrix", ""); rec.Code != http.StatusOK || !strings.Contains(rec.Header().Get("DAV"), "calendar-access") {
		t.Fatalf("OPTIONS: %d headers=%v", rec.Code, rec.Header())
	}

	for _, tc := range []struct {
		method string
		target string
		body   string
		status int
	}{
		{http.MethodGet, "/caldav/bob/matrix/matrix.ics", "", 404},
		{http.MethodPut, "/caldav/alice/matrix", ics, 400},
		{http.MethodPut, "/caldav/alice/matrix/../escape.ics", ics, 400},
		{http.MethodPut, "/caldav/alice/matrix/matrix.txt", ics, 400},
		{http.MethodPut, "/caldav/alice/matrix/empty.ics", "", 400},
		{http.MethodPut, "/caldav/alice/matrix/bad.ics", "invalid", 400},
		{http.MethodGet, "/caldav/alice/matrix/missing.ics", "", 404},
		{http.MethodPatch, "/caldav/alice/matrix", "", 405},
	} {
		if rec := call(tc.method, tc.target, tc.body); rec.Code != tc.status {
			t.Fatalf("%s %s: %d, want %d; body=%s", tc.method, tc.target, rec.Code, tc.status, rec.Body.String())
		}
	}
	if rec := call(http.MethodDelete, "/caldav/alice/matrix/matrix.ics", ""); rec.Code != http.StatusNoContent {
		t.Fatalf("DELETE: %d %s", rec.Code, rec.Body.String())
	}
	if rec := call(http.MethodGet, "/caldav/alice/matrix/matrix.ics", ""); rec.Code != http.StatusNotFound {
		t.Fatalf("GET deleted: %d %s", rec.Code, rec.Body.String())
	}
}

func TestSubscriptionHTTPWorkflowAndFailureState(t *testing.T) {
	fixture := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/bad" {
			http.Error(w, "broken", http.StatusBadGateway)
			return
		}
		_, _ = w.Write([]byte("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:remote\r\nDTSTART:20280101T120000Z\r\nDTEND:20280101T130000Z\r\nSUMMARY:Remote event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"))
	}))
	defer fixture.Close()
	app := newSQLTestApp(t)
	call := func(handler http.HandlerFunc, method, target, body string) *httptest.ResponseRecorder {
		req := withTestUser(httptest.NewRequest(method, target, bytes.NewBufferString(body)), "alice")
		rec := httptest.NewRecorder()
		handler(rec, req)
		return rec
	}
	rec := call(app.handleSubscriptions, http.MethodPost, "/api/calendar/subscriptions", `{"name":"Remote","url":"`+fixture.URL+`"}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("create subscription: %d %s", rec.Code, rec.Body.String())
	}
	var meta CalendarMeta
	if err := json.Unmarshal(rec.Body.Bytes(), &meta); err != nil {
		t.Fatal(err)
	}
	if meta.ID == "" || !meta.ReadOnly {
		t.Fatalf("subscription = %#v", meta)
	}
	rec = call(app.handleSubscriptions, http.MethodGet, "/api/calendar/subscriptions", "")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Remote") {
		t.Fatalf("list subscriptions: %d %s", rec.Code, rec.Body.String())
	}
	rec = call(app.handleSubscriptionRefresh, http.MethodPost, "/api/calendar/subscriptions/refresh?id="+meta.ID, "")
	if rec.Code != http.StatusOK {
		t.Fatalf("refresh subscription: %d %s", rec.Code, rec.Body.String())
	}
	app.refreshAllSubscriptions(context.Background())
	events := app.listEventsInCalendar("alice", meta.ID)
	if len(events) != 1 || events[0].ID != "remote" {
		t.Fatalf("subscription events = %#v", events)
	}

	rec = call(app.handleSubscriptions, http.MethodPost, "/api/calendar/subscriptions", `{"name":"Broken","url":"`+fixture.URL+`/bad"}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("create broken subscription: %d %s", rec.Code, rec.Body.String())
	}
	var broken CalendarMeta
	if err := json.Unmarshal(rec.Body.Bytes(), &broken); err != nil {
		t.Fatal(err)
	}
	if err := app.refreshSubscription(context.Background(), "alice", broken.ID); err == nil {
		t.Fatal("broken subscription refresh succeeded")
	}
	broken = app.loadMeta("alice", broken.ID)
	if broken.LastSyncError == "" {
		t.Fatal("subscription failure was not persisted")
	}
	rec = call(app.handleSubscriptionRefresh, http.MethodPost, "/api/calendar/subscriptions/refresh?id=missing", "")
	if rec.Code != http.StatusBadGateway {
		t.Fatalf("refresh missing: %d %s", rec.Code, rec.Body.String())
	}
	rec = call(app.handleSubscriptions, http.MethodDelete, "/api/calendar/subscriptions?id="+meta.ID, "")
	if rec.Code != http.StatusOK {
		t.Fatalf("delete subscription: %d %s", rec.Code, rec.Body.String())
	}
}
