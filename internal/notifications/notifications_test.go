package notifications

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
	"github.com/go-chi/chi/v5"
)

func openTestService(t *testing.T) (*Service, *auth.User, *auth.User) {
	t.Helper()
	migrations := append([]string{}, auth.Migrations()...)
	migrations = append(migrations, preferences.Migrations()...)
	migrations = append(migrations, Migrations()...)
	database, err := db.Open(filepath.Join(t.TempDir(), "notifications.sqlite"), migrations)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	alice, _, err := auth.FindOrCreateUser(context.Background(), conn, "alice", "alice@example.test", "Alice", "", "https://id.test", "alice")
	if err != nil {
		release()
		t.Fatal(err)
	}
	bob, _, err := auth.FindOrCreateUser(context.Background(), conn, "bob", "bob@example.test", "Bob", "", "https://id.test", "bob")
	release()
	if err != nil {
		t.Fatal(err)
	}
	return New(database), alice, bob
}

func asUser(request *http.Request, user *auth.User) *http.Request {
	return request.WithContext(context.WithValue(request.Context(), server.UserContextKey, user))
}

type lockedResponse struct {
	mu     sync.Mutex
	header http.Header
	body   bytes.Buffer
	status int
}

func newLockedResponse() *lockedResponse {
	return &lockedResponse{header: make(http.Header)}
}

func (w *lockedResponse) Header() http.Header { return w.header }
func (w *lockedResponse) WriteHeader(status int) {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.status = status
}
func (w *lockedResponse) Write(data []byte) (int, error) {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.body.Write(data)
}
func (w *lockedResponse) Flush() {}
func (w *lockedResponse) String() string {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.body.String()
}

func TestEnqueueDeduplicatesAndScopesDismissal(t *testing.T) {
	service, alice, bob := openTestService(t)
	notification := apps.Notification{
		RecipientID: alice.ID, Source: "email", Kind: "new_mail", Subject: "New email",
		Message: "Quarterly report", ObjectURL: "/email?thread=one", DedupeKey: "account/hash",
	}
	if err := service.EnqueueNotification(context.Background(), notification); err != nil {
		t.Fatal(err)
	}
	if err := service.EnqueueNotification(context.Background(), notification); err != nil {
		t.Fatal(err)
	}
	if err := service.EnqueueNotification(context.Background(), apps.Notification{
		RecipientID: bob.ID, Source: "email", Kind: "new_mail", Subject: "Bob mail",
	}); err != nil {
		t.Fatal(err)
	}

	aliceItems, aliceCount, err := service.list(context.Background(), alice.ID, 50)
	if err != nil {
		t.Fatal(err)
	}
	if aliceCount != 1 || len(aliceItems) != 1 || aliceItems[0].Subject != "New email" {
		t.Fatalf("alice notifications=%#v count=%d", aliceItems, aliceCount)
	}
	if changed, err := service.dismiss(context.Background(), bob.ID, aliceItems[0].ID); err != nil || changed {
		t.Fatalf("cross-user dismiss changed=%v err=%v", changed, err)
	}
	if changed, err := service.dismiss(context.Background(), alice.ID, aliceItems[0].ID); err != nil || !changed {
		t.Fatalf("owner dismiss changed=%v err=%v", changed, err)
	}
	_, aliceCount, _ = service.list(context.Background(), alice.ID, 50)
	_, bobCount, _ := service.list(context.Background(), bob.ID, 50)
	if aliceCount != 0 || bobCount != 1 {
		t.Fatalf("counts after dismiss alice=%d bob=%d", aliceCount, bobCount)
	}
}

func TestHandlersPreferencesAndDismissAll(t *testing.T) {
	service, alice, _ := openTestService(t)
	_ = service.EnqueueNotification(context.Background(), apps.Notification{
		RecipientID: alice.ID, Source: "email", Kind: "new_mail", Subject: "One",
	})

	list := httptest.NewRecorder()
	service.HandleList(list, asUser(httptest.NewRequest(http.MethodGet, "/api/notifications", nil), alice))
	if list.Code != http.StatusOK || !strings.Contains(list.Body.String(), `"count":1`) || !strings.Contains(list.Body.String(), `"desktop_enabled":false`) {
		t.Fatalf("list status=%d body=%s", list.Code, list.Body.String())
	}

	bad := httptest.NewRecorder()
	service.HandlePreferences(bad, asUser(httptest.NewRequest(http.MethodPut, "/api/notifications/preferences", strings.NewReader(`{"desktop_enabled":true,"extra":1}`)), alice))
	if bad.Code != http.StatusBadRequest {
		t.Fatalf("unknown preference status=%d body=%s", bad.Code, bad.Body.String())
	}

	save := httptest.NewRecorder()
	service.HandlePreferences(save, asUser(httptest.NewRequest(http.MethodPut, "/api/notifications/preferences", strings.NewReader(`{"desktop_enabled":true}`)), alice))
	if save.Code != http.StatusOK || !strings.Contains(save.Body.String(), `"desktop_enabled":true`) {
		t.Fatalf("save status=%d body=%s", save.Code, save.Body.String())
	}

	all := httptest.NewRecorder()
	service.HandleDismissAll(all, asUser(httptest.NewRequest(http.MethodDelete, "/api/notifications", nil), alice))
	if all.Code != http.StatusNoContent {
		t.Fatalf("dismiss all status=%d body=%s", all.Code, all.Body.String())
	}
}

func TestStreamDeliversCreatedNotification(t *testing.T) {
	service, alice, _ := openTestService(t)
	ctx, cancel := context.WithCancel(context.Background())
	request := asUser(httptest.NewRequest(http.MethodGet, "/api/notifications/stream", nil).WithContext(ctx), alice)
	response := newLockedResponse()
	done := make(chan struct{})
	go func() {
		service.HandleStream(response, request)
		close(done)
	}()

	deadline := time.Now().Add(time.Second)
	for {
		service.mu.Lock()
		subscribed := len(service.subscribers[alice.ID]) == 1
		service.mu.Unlock()
		if subscribed {
			break
		}
		if time.Now().After(deadline) {
			cancel()
			t.Fatal("stream did not subscribe")
		}
		time.Sleep(time.Millisecond)
	}
	if err := service.EnqueueNotification(context.Background(), apps.Notification{
		RecipientID: alice.ID, Source: "email", Kind: "new_mail", Subject: "Live mail",
	}); err != nil {
		cancel()
		t.Fatal(err)
	}
	deadline = time.Now().Add(time.Second)
	for !strings.Contains(response.String(), "event: notification.created") {
		if time.Now().After(deadline) {
			cancel()
			t.Fatalf("stream output=%s", response.String())
		}
		time.Sleep(time.Millisecond)
	}
	service.Close()
	select {
	case <-done:
	case <-time.After(time.Second):
		cancel()
		t.Fatal("stream did not stop after service close")
	}
	cancel()

	var created storedNotification
	output := response.String()
	parts := strings.Split(output, "\n")
	for i, part := range parts {
		if part == "event: notification.created" && i+1 < len(parts) {
			_ = json.Unmarshal([]byte(strings.TrimPrefix(parts[i+1], "data: ")), &created)
		}
	}
	if created.Subject != "Live mail" {
		t.Fatalf("created event=%#v output=%s", created, output)
	}
}

type nonFlushingResponse struct {
	header http.Header
	body   bytes.Buffer
	status int
}

func (w *nonFlushingResponse) Header() http.Header { return w.header }
func (w *nonFlushingResponse) WriteHeader(status int) {
	w.status = status
}
func (w *nonFlushingResponse) Write(data []byte) (int, error) { return w.body.Write(data) }

func TestNotificationHTTPFailureAndValidationMatrix(t *testing.T) {
	service, alice, _ := openTestService(t)
	created := apps.Notification{RecipientID: alice.ID, Source: "email", Kind: "new_mail", Subject: "One"}
	if err := service.EnqueueNotification(context.Background(), created); err != nil {
		t.Fatal(err)
	}
	items, _, err := service.list(context.Background(), alice.ID, 10)
	if err != nil || len(items) != 1 {
		t.Fatalf("items=%+v err=%v", items, err)
	}
	dismissRequest := func(id string) *http.Request {
		request := asUser(httptest.NewRequest(http.MethodDelete, "/api/notifications/"+id, nil), alice)
		route := chi.NewRouteContext()
		route.URLParams.Add("id", id)
		return request.WithContext(context.WithValue(request.Context(), chi.RouteCtxKey, route))
	}
	for _, tc := range []struct {
		handler func(http.ResponseWriter, *http.Request)
		request *http.Request
		want    int
	}{
		{service.HandleList, httptest.NewRequest(http.MethodGet, "/api/notifications", nil), http.StatusUnauthorized},
		{service.HandleList, asUser(httptest.NewRequest(http.MethodGet, "/api/notifications?limit=0", nil), alice), http.StatusBadRequest},
		{service.HandleList, asUser(httptest.NewRequest(http.MethodGet, "/api/notifications?limit=101", nil), alice), http.StatusBadRequest},
		{service.HandleList, asUser(httptest.NewRequest(http.MethodGet, "/api/notifications?limit=bad", nil), alice), http.StatusBadRequest},
		{service.HandleDismiss, httptest.NewRequest(http.MethodDelete, "/", nil), http.StatusUnauthorized},
		{service.HandleDismiss, dismissRequest(""), http.StatusBadRequest},
		{service.HandleDismiss, dismissRequest("missing"), http.StatusNotFound},
		{service.HandleDismiss, dismissRequest(items[0].ID), http.StatusNoContent},
		{service.HandleDismissAll, httptest.NewRequest(http.MethodDelete, "/", nil), http.StatusUnauthorized},
		{service.HandlePreferences, httptest.NewRequest(http.MethodGet, "/", nil), http.StatusUnauthorized},
		{service.HandlePreferences, asUser(httptest.NewRequest(http.MethodGet, "/", nil), alice), http.StatusOK},
		{service.HandlePreferences, asUser(httptest.NewRequest(http.MethodPut, "/", strings.NewReader(`{"desktop_enabled":true} {}`)), alice), http.StatusBadRequest},
		{service.HandlePreferences, asUser(httptest.NewRequest(http.MethodPost, "/", nil), alice), http.StatusMethodNotAllowed},
		{service.HandleStream, httptest.NewRequest(http.MethodGet, "/", nil), http.StatusUnauthorized},
	} {
		recorder := httptest.NewRecorder()
		tc.handler(recorder, tc.request)
		if recorder.Code != tc.want {
			t.Errorf("%s status=%d want=%d body=%s", tc.request.URL, recorder.Code, tc.want, recorder.Body.String())
		}
	}
	noFlush := &nonFlushingResponse{header: make(http.Header)}
	service.HandleStream(noFlush, asUser(httptest.NewRequest(http.MethodGet, "/", nil), alice))
	if noFlush.status != http.StatusInternalServerError {
		t.Fatalf("non-flushing stream status=%d", noFlush.status)
	}
	if safeEventName("bad\nname") != "notification.resync" || safeEventName("notification.created") != "notification.created" {
		t.Fatal("SSE event-name validation regression")
	}
	if !strings.Contains(string(encodeEventData(map[string]string{"line": "a\nb"})), `\n`) {
		t.Fatal("SSE data was not encoded as one JSON line")
	}
	if string(encodeEventData(make(chan int))) != `{}` {
		t.Fatal("unencodable SSE data did not fall back safely")
	}
}

func TestNotificationClosedDatabaseFailureMatrix(t *testing.T) {
	service, alice, _ := openTestService(t)
	if err := service.db.Close(); err != nil {
		t.Fatal(err)
	}
	if err := service.EnqueueNotification(context.Background(), apps.Notification{
		RecipientID: alice.ID, Source: "email", Kind: "new_mail", Subject: "Closed",
	}); err == nil {
		t.Fatal("enqueue with closed database succeeded")
	}
	for _, tc := range []struct {
		handler func(http.ResponseWriter, *http.Request)
		method  string
		body    string
	}{
		{service.HandleList, http.MethodGet, ""},
		{service.HandleDismissAll, http.MethodDelete, ""},
		{service.HandlePreferences, http.MethodGet, ""},
		{service.HandlePreferences, http.MethodPut, `{"desktop_enabled":true}`},
	} {
		rec := httptest.NewRecorder()
		tc.handler(rec, asUser(httptest.NewRequest(tc.method, "/", strings.NewReader(tc.body)), alice))
		if rec.Code != http.StatusInternalServerError {
			t.Errorf("closed DB handler=%d body=%s", rec.Code, rec.Body.String())
		}
	}
}
