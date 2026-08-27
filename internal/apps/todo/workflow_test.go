package todo

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/server"
	"zombiezen.com/go/sqlite"
)

type todoSearchSink struct {
	docs []apps.SearchDocument
	err  error
}

func (s *todoSearchSink) AddSearchDocument(_ context.Context, doc apps.SearchDocument) error {
	if s.err != nil {
		return s.err
	}
	s.docs = append(s.docs, doc)
	return nil
}

func todoRequest(t *testing.T, method, target string, body any, user *auth.User) *http.Request {
	t.Helper()
	var data []byte
	if body != nil {
		var err error
		data, err = json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
	}
	req := httptest.NewRequest(method, target, bytes.NewReader(data))
	req.Header.Set("Content-Type", "application/json")
	return req.WithContext(context.WithValue(req.Context(), server.UserContextKey, user))
}

func todoCall(t *testing.T, handler http.HandlerFunc, method, target string, body any, user *auth.User) *httptest.ResponseRecorder {
	t.Helper()
	rec := httptest.NewRecorder()
	handler(rec, todoRequest(t, method, target, body, user))
	return rec
}

func todoUser(t *testing.T, app *App, username string) *auth.User {
	t.Helper()
	var user *auth.User
	err := app.withConn(func(conn *sqlite.Conn) error {
		var err error
		user, _, err = auth.FindOrCreateUser(context.Background(), conn, username, username+"@example.test", username, "", "https://issuer.example.test", username)
		return err
	})
	if err != nil {
		t.Fatal(err)
	}
	return user
}

func TestTodoListShareTaskLifecycleAndSearch(t *testing.T) {
	app := testApp(t)
	alice := todoUser(t, app, "alice")
	bob := todoUser(t, app, "bob")

	rec := todoCall(t, app.handleLists, http.MethodPost, "/api/todo/lists", TaskList{Name: "Projects", Color: "#123456"}, alice)
	if rec.Code != http.StatusOK {
		t.Fatalf("create list: %d %s", rec.Code, rec.Body.String())
	}
	var list TaskList
	if err := json.Unmarshal(rec.Body.Bytes(), &list); err != nil {
		t.Fatal(err)
	}
	if list.ID == "" || list.Permission != "owner" {
		t.Fatalf("created list = %#v", list)
	}

	rec = todoCall(t, app.handleList, http.MethodPatch, "/api/todo/lists/"+list.ID, TaskList{Name: "Roadmap", Color: "#654321"}, alice)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("update list: %d %s", rec.Code, rec.Body.String())
	}
	rec = todoCall(t, app.handleList, http.MethodPost, "/api/todo/lists/"+list.ID+"/share", map[string]any{"email": bob.Email, "permission": "write"}, alice)
	if rec.Code != http.StatusOK {
		t.Fatalf("share list: %d %s", rec.Code, rec.Body.String())
	}
	rec = todoCall(t, app.handleList, http.MethodGet, "/api/todo/lists/"+list.ID+"/share", nil, alice)
	if rec.Code != http.StatusOK {
		t.Fatalf("list shares: %d %s", rec.Code, rec.Body.String())
	}

	now := time.Now().Unix()
	input := Task{
		ListID: list.ID, Title: "Ship regression suite", Notes: "searchable task body",
		Status: "NEEDS-ACTION", Priority: 8, Starred: true, StartAt: now - 60,
		DueAt: now + 60, RRule: "FREQ=DAILY", IsHabit: true, Tags: []string{"quality", "release"},
	}
	rec = todoCall(t, app.handleTasks, http.MethodPost, "/api/todo/tasks", input, bob)
	if rec.Code != http.StatusOK {
		t.Fatalf("create shared task: %d %s", rec.Code, rec.Body.String())
	}
	var task Task
	if err := json.Unmarshal(rec.Body.Bytes(), &task); err != nil {
		t.Fatal(err)
	}
	if task.ID == "" || task.ListID != list.ID {
		t.Fatalf("created task = %#v", task)
	}

	patch := task
	patch.Title = "Ship complete regression suite"
	patch.PercentComplete = 50
	patch.Status = "IN-PROCESS"
	rec = todoCall(t, app.handleTask, http.MethodPatch, "/api/todo/tasks/"+task.ID, patch, alice)
	if rec.Code != http.StatusOK {
		t.Fatalf("update task: %d %s", rec.Code, rec.Body.String())
	}
	rec = todoCall(t, app.handleTask, http.MethodPost, "/api/todo/tasks/"+task.ID+"?action=complete", nil, alice)
	if rec.Code != http.StatusOK {
		t.Fatalf("complete recurring task: %d %s", rec.Code, rec.Body.String())
	}
	var completed Task
	if err := json.Unmarshal(rec.Body.Bytes(), &completed); err != nil {
		t.Fatal(err)
	}
	if completed.Status != "NEEDS-ACTION" || completed.DueAt <= task.DueAt || completed.Streak != 1 {
		t.Fatalf("recurring completion = %#v", completed)
	}

	for _, collection := range []string{"important", "today", "week", "current", "all", "completed"} {
		rec = todoCall(t, app.handleTasks, http.MethodGet, "/api/todo/tasks?collection="+collection, nil, alice)
		if rec.Code != http.StatusOK {
			t.Fatalf("list %s: %d %s", collection, rec.Code, rec.Body.String())
		}
	}

	sink := &todoSearchSink{}
	if err := app.IndexSearch(context.Background(), alice.ID, sink); err != nil {
		t.Fatal(err)
	}
	if len(sink.docs) != 1 || sink.docs[0].ObjectID != task.ID || sink.docs[0].Subtitle != "Roadmap" {
		t.Fatalf("search docs = %#v", sink.docs)
	}
	sink.err = context.Canceled
	if err := app.IndexSearch(context.Background(), alice.ID, sink); err != context.Canceled {
		t.Fatalf("search sink error = %v", err)
	}

	rec = todoCall(t, app.handleTask, http.MethodDelete, "/api/todo/tasks/"+task.ID, nil, bob)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("trash task: %d %s", rec.Code, rec.Body.String())
	}
	rec = todoCall(t, app.handleTasks, http.MethodGet, "/api/todo/tasks?trash=1", nil, alice)
	if rec.Code != http.StatusOK {
		t.Fatalf("list task trash: %d %s", rec.Code, rec.Body.String())
	}
	rec = todoCall(t, app.handleTask, http.MethodPost, "/api/todo/tasks/"+task.ID+"?action=restore", nil, alice)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("restore task: %d %s", rec.Code, rec.Body.String())
	}

	rec = todoCall(t, app.handleList, http.MethodDelete, "/api/todo/lists/"+list.ID+"/share?user_id="+bob.ID, nil, alice)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("unshare list: %d %s", rec.Code, rec.Body.String())
	}
	rec = todoCall(t, app.handleTask, http.MethodGet, "/api/todo/tasks/"+task.ID, nil, bob)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("unshared user read status = %d", rec.Code)
	}

	prefs := defaultPreferences()
	prefs.DefaultListID = list.ID
	prefs.SortMode = "priority"
	rec = todoCall(t, app.handlePreferences, http.MethodPut, "/api/todo/preferences", prefs, alice)
	if rec.Code != http.StatusOK {
		t.Fatalf("save preferences: %d %s", rec.Code, rec.Body.String())
	}

	rec = todoCall(t, app.handleList, http.MethodDelete, "/api/todo/lists/"+list.ID, nil, alice)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("trash list: %d %s", rec.Code, rec.Body.String())
	}
	rec = todoCall(t, app.handleLists, http.MethodGet, "/api/todo/lists?trash=1", nil, alice)
	if rec.Code != http.StatusOK {
		t.Fatalf("list trash: %d %s", rec.Code, rec.Body.String())
	}
	rec = todoCall(t, app.handleList, http.MethodPost, "/api/todo/lists/"+list.ID+"/restore", nil, alice)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("restore list: %d %s", rec.Code, rec.Body.String())
	}
}

func TestTodoHandlerValidationAndNonRecurringCompletion(t *testing.T) {
	app := testApp(t)
	alice := todoUser(t, app, "alice-validation")

	for _, tc := range []struct {
		name    string
		handler http.HandlerFunc
		method  string
		target  string
		body    any
		status  int
	}{
		{"bad list JSON", app.handleLists, http.MethodPost, "/api/todo/lists", json.RawMessage(`{`), 400},
		{"empty list name", app.handleLists, http.MethodPost, "/api/todo/lists", TaskList{}, 400},
		{"bad task title", app.handleTasks, http.MethodPost, "/api/todo/tasks", Task{}, 400},
		{"unsupported lists method", app.handleLists, http.MethodPatch, "/api/todo/lists", nil, 405},
		{"missing list", app.handleList, http.MethodGet, "/api/todo/lists/missing", nil, 404},
		{"missing task", app.handleTask, http.MethodGet, "/api/todo/tasks/missing", nil, 404},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var rec *httptest.ResponseRecorder
			if raw, ok := tc.body.(json.RawMessage); ok {
				req := httptest.NewRequest(tc.method, tc.target, bytes.NewReader(raw))
				req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, alice))
				rec = httptest.NewRecorder()
				tc.handler(rec, req)
			} else {
				rec = todoCall(t, tc.handler, tc.method, tc.target, tc.body, alice)
			}
			if rec.Code != tc.status {
				t.Fatalf("status = %d, want %d; body=%s", rec.Code, tc.status, rec.Body.String())
			}
		})
	}

	rec := todoCall(t, app.handleTasks, http.MethodPost, "/api/todo/tasks", Task{Title: "One shot"}, alice)
	if rec.Code != http.StatusOK {
		t.Fatalf("create one-shot task: %d %s", rec.Code, rec.Body.String())
	}
	var task Task
	if err := json.Unmarshal(rec.Body.Bytes(), &task); err != nil {
		t.Fatal(err)
	}
	rec = todoCall(t, app.handleTask, http.MethodPost, "/api/todo/tasks/"+task.ID+"?action=complete", nil, alice)
	if rec.Code != http.StatusOK {
		t.Fatalf("complete one-shot task: %d %s", rec.Code, rec.Body.String())
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &task); err != nil {
		t.Fatal(err)
	}
	if task.Status != "COMPLETED" || task.PercentComplete != 100 || task.CompletedAt == 0 {
		t.Fatalf("one-shot completion = %#v", task)
	}

	if got := taskID(); len(got) != 33 || got[0] != 't' {
		t.Fatalf("task ID = %q", got)
	}
	if max(1, 2) != 2 || max(3, 2) != 3 {
		t.Fatal("max boundary failed")
	}
	if _, err := nextDue("FREQ=DAILY;INTERVAL=0", time.Now().Unix()); err == nil {
		t.Fatal("invalid recurrence interval accepted")
	}
	if _, err := nextDue("FREQ=MONTHLY", time.Now().Unix()); err != nil {
		t.Fatal(err)
	}
	if _, err := nextDue("FREQ=YEARLY", time.Now().Unix()); err != nil {
		t.Fatal(err)
	}
	if _, err := app.CallAssistantTool(context.Background(), alice.ID, "unknown", nil); err == nil {
		t.Fatal("unknown assistant tool accepted")
	}
	if got := fmt.Sprint(todoQueryArgs(json.RawMessage(`{"trash":true}`))); got == "" {
		t.Fatal("empty todo query arguments")
	}
}

func TestTodoPermissionAndActionFailureMatrix(t *testing.T) {
	app := testApp(t)
	alice := todoUser(t, app, "alice-edges")
	bob := todoUser(t, app, "bob-edges")
	rec := todoCall(t, app.handleLists, http.MethodPost, "/api/todo/lists", TaskList{Name: "Edges"}, alice)
	var list TaskList
	if err := json.Unmarshal(rec.Body.Bytes(), &list); err != nil {
		t.Fatal(err)
	}
	rec = todoCall(t, app.handleTasks, http.MethodPost, "/api/todo/tasks", Task{ListID: list.ID, Title: "Edge task"}, alice)
	var task Task
	if err := json.Unmarshal(rec.Body.Bytes(), &task); err != nil {
		t.Fatal(err)
	}

	for _, tc := range []struct {
		handler http.HandlerFunc
		method  string
		target  string
		body    any
		status  int
	}{
		{app.handleList, http.MethodPost, "/api/todo/lists/" + list.ID + "/share", map[string]any{"email": bob.Email, "permission": "admin"}, 400},
		{app.handleList, http.MethodPost, "/api/todo/lists/" + list.ID + "/share", map[string]any{"email": "missing@example.test", "permission": "read"}, 400},
		{app.handleList, http.MethodDelete, "/api/todo/lists/" + list.ID + "/share", nil, 400},
		{app.handleList, http.MethodPatch, "/api/todo/lists/" + list.ID + "/share", nil, 405},
		{app.handleList, http.MethodPost, "/api/todo/lists/" + list.ID, nil, 405},
		{app.handleList, http.MethodPost, "/api/todo/lists/missing/restore", nil, 404},
		{app.handleTask, http.MethodPost, "/api/todo/tasks/" + task.ID + "?action=unknown", nil, 400},
		{app.handleTask, http.MethodPut, "/api/todo/tasks/" + task.ID, nil, 405},
		{app.handleTask, http.MethodPatch, "/api/todo/tasks/" + task.ID, Task{Title: "invalid", PercentComplete: 101}, 400},
		{app.handleTasks, http.MethodPut, "/api/todo/tasks", nil, 405},
	} {
		rec = todoCall(t, tc.handler, tc.method, tc.target, tc.body, alice)
		if rec.Code != tc.status {
			t.Errorf("%s %s status=%d want=%d body=%s", tc.method, tc.target, rec.Code, tc.status, rec.Body.String())
		}
	}

	rec = todoCall(t, app.handleList, http.MethodPost, "/api/todo/lists/"+list.ID+"/share", map[string]any{"email": bob.Email, "permission": "read"}, alice)
	if rec.Code != http.StatusOK {
		t.Fatalf("read share status=%d body=%s", rec.Code, rec.Body.String())
	}
	for _, tc := range []struct {
		handler http.HandlerFunc
		method  string
		target  string
		body    any
	}{
		{app.handleList, http.MethodPatch, "/api/todo/lists/" + list.ID, TaskList{Name: "blocked"}},
		{app.handleList, http.MethodDelete, "/api/todo/lists/" + list.ID, nil},
		{app.handleList, http.MethodGet, "/api/todo/lists/" + list.ID + "/share", nil},
		{app.handleTasks, http.MethodPost, "/api/todo/tasks", Task{ListID: list.ID, Title: "blocked"}},
		{app.handleTask, http.MethodPatch, "/api/todo/tasks/" + task.ID, task},
		{app.handleTask, http.MethodPost, "/api/todo/tasks/" + task.ID + "?action=complete", nil},
		{app.handleTask, http.MethodDelete, "/api/todo/tasks/" + task.ID, nil},
	} {
		rec = todoCall(t, tc.handler, tc.method, tc.target, tc.body, bob)
		if rec.Code != http.StatusForbidden {
			t.Errorf("read-only %s %s status=%d body=%s", tc.method, tc.target, rec.Code, rec.Body.String())
		}
	}
}

func TestTodoClosedDatabaseHandlerMatrix(t *testing.T) {
	app := testApp(t)
	alice := todoUser(t, app, "closed")
	if err := app.db.Close(); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		handler http.HandlerFunc
		method  string
		target  string
		body    any
	}{
		{app.handleLists, http.MethodGet, "/api/todo/lists", nil},
		{app.handleLists, http.MethodPost, "/api/todo/lists", TaskList{Name: "Closed"}},
		{app.handleList, http.MethodGet, "/api/todo/lists/missing", nil},
		{app.handleList, http.MethodPatch, "/api/todo/lists/missing", TaskList{Name: "Closed"}},
		{app.handleList, http.MethodDelete, "/api/todo/lists/missing", nil},
		{app.handleList, http.MethodGet, "/api/todo/lists/missing/share", nil},
		{app.handleList, http.MethodPost, "/api/todo/lists/missing/share", map[string]any{"email": "nobody@example.test", "permission": "read"}},
		{app.handleTasks, http.MethodGet, "/api/todo/tasks", nil},
		{app.handleTasks, http.MethodPost, "/api/todo/tasks", Task{ListID: "missing", Title: "Closed"}},
		{app.handleTask, http.MethodGet, "/api/todo/tasks/missing", nil},
		{app.handleTask, http.MethodPatch, "/api/todo/tasks/missing", Task{Title: "Closed"}},
		{app.handleTask, http.MethodPost, "/api/todo/tasks/missing?action=complete", nil},
		{app.handleTask, http.MethodDelete, "/api/todo/tasks/missing", nil},
		{app.handlePreferences, http.MethodGet, "/api/todo/preferences", nil},
		{app.handlePreferences, http.MethodPut, "/api/todo/preferences", Preferences{}},
	} {
		rec := todoCall(t, tc.handler, tc.method, tc.target, tc.body, alice)
		if rec.Code < 400 {
			t.Errorf("closed DB %s %s returned %d: %s", tc.method, tc.target, rec.Code, rec.Body.String())
		}
	}
}

func TestTodoCalDAVStoreLifecycle(t *testing.T) {
	app := testApp(t)
	collection, err := app.CreateCollection("alice", "DAV Projects")
	if err != nil {
		t.Fatal(err)
	}
	collections, err := app.Collections("alice")
	if err != nil || len(collections) != 1 {
		t.Fatalf("collections = %#v, %v", collections, err)
	}
	ics := "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:matrix\r\nSUMMARY:DAV matrix\r\nDESCRIPTION:Task body\r\nDUE:20260712T120000Z\r\nPRIORITY:7\r\nEND:VTODO\r\nEND:VCALENDAR\r\n"
	object, err := app.PutObject("alice", collection.ID, "matrix.ics", ics)
	if err != nil || object.ETag == "" {
		t.Fatalf("put object = %#v, %v", object, err)
	}
	objects, err := app.Objects("alice", collection.ID)
	if err != nil || len(objects) != 1 {
		t.Fatalf("objects = %#v, %v", objects, err)
	}
	if _, err = app.Object("bob", collection.ID, "matrix.ics"); err == nil {
		t.Fatal("cross-user task object was readable")
	}
	updatedICS := strings.Replace(ics, "DAV matrix", "DAV matrix updated", 1)
	if _, err = app.PutObject("alice", collection.ID, "matrix.ics", updatedICS); err != nil {
		t.Fatal(err)
	}
	object, err = app.Object("alice", collection.ID, "matrix.ics")
	if err != nil || !strings.Contains(object.Data, "DAV matrix updated") {
		t.Fatalf("updated object = %#v, %v", object, err)
	}
	if _, err = app.PutObject("bob", collection.ID, "blocked.ics", ics); err == nil {
		t.Fatal("cross-user DAV write succeeded")
	}
	if _, err = app.PutObject("alice", collection.ID, "bad.ics", "not a VTODO"); err == nil {
		t.Fatal("malformed VTODO accepted")
	}
	if err = app.DeleteObject("alice", collection.ID, "matrix.ics"); err != nil {
		t.Fatal(err)
	}
	if _, err = app.Object("alice", collection.ID, "matrix.ics"); err == nil {
		t.Fatal("deleted task object remained readable")
	}
}
