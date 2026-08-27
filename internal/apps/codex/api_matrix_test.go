package codex

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"net/textproto"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/testutil"
)

func TestCodexAPISurfaceAndAttachmentMatrix(t *testing.T) {
	service, alice, bob := testCodexService(t)
	unauthorized := httptest.NewRecorder()
	service.handleAPI(unauthorized, httptest.NewRequest(http.MethodGet, "/api/codex/status", nil))
	if unauthorized.Code != http.StatusUnauthorized {
		t.Fatalf("unauthorized status=%d", unauthorized.Code)
	}
	for _, tc := range []struct {
		method, path, body string
		allowed            []int
	}{
		{http.MethodGet, "/status", "", []int{http.StatusOK}},
		{http.MethodGet, "/models", "", []int{http.StatusOK}},
		{http.MethodPost, "/login/device", `{}`, []int{http.StatusOK}},
		{http.MethodPost, "/login/api-key", `{}`, []int{http.StatusBadRequest}},
		{http.MethodPost, "/login/api-key", `{"api_key":"test-key"}`, []int{http.StatusOK}},
		{http.MethodGet, "/preferences", "", []int{http.StatusOK}},
		{http.MethodPut, "/preferences", `{"approval_mode":"confirm","web_search_mode":"live"}`, []int{http.StatusOK}},
		{http.MethodPatch, "/preferences", `{}`, []int{http.StatusMethodNotAllowed}},
		{http.MethodGet, "/sessions", "", []int{http.StatusOK}},
		{http.MethodPost, "/sessions", `{}`, []int{http.StatusBadRequest}},
		{http.MethodPost, "/approvals/missing", `{"decision":"invalid"}`, []int{http.StatusBadRequest, http.StatusNotFound}},
		{http.MethodPost, "/attachments/select", `{"paths":["/Documents/one.txt"," ","/Photos/two.png"]}`, []int{http.StatusCreated}},
		{http.MethodPost, "/attachments/select", `{"paths":[` + strings.Repeat(`"x",`, 21) + `"x"]}`, []int{http.StatusBadRequest}},
		{http.MethodGet, "/missing", "", []int{http.StatusNotFound}},
	} {
		response := codexRequest(t, service, alice, tc.method, tc.path, tc.body)
		if !containsStatus(tc.allowed, response.Code) {
			t.Errorf("%s %s status=%d want=%v body=%s", tc.method, tc.path, response.Code, tc.allowed, response.Body.String())
		}
	}

	createdResponse := codexRequest(t, service, alice, http.MethodPost, "/sessions", `{"text":"matrix chat"}`)
	if createdResponse.Code != http.StatusCreated {
		t.Fatalf("create status=%d body=%s", createdResponse.Code, createdResponse.Body.String())
	}
	var created struct {
		Session Session `json:"session"`
	}
	if err := json.Unmarshal(createdResponse.Body.Bytes(), &created); err != nil {
		t.Fatal(err)
	}
	id := created.Session.ID
	for _, tc := range []struct {
		method, suffix, body string
		want                 int
	}{
		{http.MethodGet, "", "", http.StatusOK},
		{http.MethodPatch, "", `{"title":"Renamed","model":"fake-model","reasoningEffort":"low"}`, http.StatusOK},
		{http.MethodPost, "/turns", `{}`, http.StatusBadRequest},
		{http.MethodPost, "/turns", `{"text":"second"}`, http.StatusAccepted},
		{http.MethodPost, "/interrupt", `{}`, http.StatusConflict},
		{http.MethodPost, "/archive", `{"archived":true}`, http.StatusOK},
		{http.MethodPost, "/archive", `{"archived":false}`, http.StatusOK},
	} {
		response := codexRequest(t, service, alice, tc.method, "/sessions/"+id+tc.suffix, tc.body)
		if response.Code != tc.want {
			t.Errorf("%s %s status=%d want=%d body=%s", tc.method, tc.suffix, response.Code, tc.want, response.Body.String())
		}
	}
	if response := codexRequest(t, service, bob, http.MethodGet, "/sessions/"+id, ""); response.Code != http.StatusNotFound {
		t.Fatalf("cross-user session status=%d", response.Code)
	}

	ctx, cancel := context.WithCancel(context.Background())
	eventRequest := httptest.NewRequest(http.MethodGet, "/api/codex/sessions/"+id+"/events", nil).WithContext(ctx)
	eventRequest = eventRequest.WithContext(context.WithValue(eventRequest.Context(), server.UserContextKey, &auth.User{ID: alice}))
	eventResponse := httptest.NewRecorder()
	eventDone := make(chan struct{})
	go func() {
		service.handleAPI(eventResponse, eventRequest)
		close(eventDone)
	}()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		hub := service.hub(alice, id)
		hub.mu.Lock()
		subscribed := len(hub.subscribers) > 0
		hub.mu.Unlock()
		if subscribed {
			break
		}
		time.Sleep(time.Millisecond)
	}
	cancel()
	<-eventDone
	if eventResponse.Code != http.StatusOK || !strings.Contains(eventResponse.Body.String(), "event:") {
		t.Fatalf("events status=%d body=%s", eventResponse.Code, eventResponse.Body.String())
	}

	var upload bytes.Buffer
	writer := multipart.NewWriter(&upload)
	header := textproto.MIMEHeader{}
	header.Set("Content-Disposition", `form-data; name="file"; filename="pixel.png"`)
	header.Set("Content-Type", "image/png")
	part, err := writer.CreatePart(header)
	if err != nil {
		t.Fatal(err)
	}
	_, _ = part.Write([]byte("\x89PNG\r\n\x1a\n"))
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	uploadRequest := httptest.NewRequest(http.MethodPost, "/api/codex/attachments/upload", &upload)
	uploadRequest.Header.Set("Content-Type", writer.FormDataContentType())
	uploadRequest = uploadRequest.WithContext(context.WithValue(uploadRequest.Context(), server.UserContextKey, &auth.User{ID: alice}))
	uploadResponse := httptest.NewRecorder()
	service.handleAPI(uploadResponse, uploadRequest)
	if uploadResponse.Code != http.StatusCreated {
		t.Fatalf("upload status=%d body=%s", uploadResponse.Code, uploadResponse.Body.String())
	}
	if response := codexRequest(t, service, alice, http.MethodPost, "/logout", `{}`); response.Code != http.StatusOK {
		t.Fatalf("logout status=%d body=%s", response.Code, response.Body.String())
	}
	if firstError(nil, context.Canceled) != context.Canceled || firstError(nil, nil) != nil {
		t.Fatal("firstError regression")
	}
}

func TestCodexSessionPatchDeleteAndValidationEdges(t *testing.T) {
	service, alice, _ := testCodexService(t)
	createdResponse := codexRequest(t, service, alice, http.MethodPost, "/sessions", `{"text":"deletable chat"}`)
	if createdResponse.Code != http.StatusCreated {
		t.Fatalf("create status=%d body=%s", createdResponse.Code, createdResponse.Body.String())
	}
	var created struct {
		Session Session `json:"session"`
	}
	if err := json.Unmarshal(createdResponse.Body.Bytes(), &created); err != nil {
		t.Fatal(err)
	}
	id := created.Session.ID
	for _, tc := range []struct {
		method string
		body   string
		status int
	}{
		{http.MethodPatch, `{`, http.StatusBadRequest},
		{http.MethodPatch, `{"title":"` + strings.Repeat("x", 201) + `"}`, http.StatusBadRequest},
		{http.MethodPut, `{}`, http.StatusMethodNotAllowed},
	} {
		response := codexRequest(t, service, alice, tc.method, "/sessions/"+id, tc.body)
		if response.Code != tc.status {
			t.Errorf("%s status=%d want=%d body=%s", tc.method, response.Code, tc.status, response.Body.String())
		}
	}
	response := codexRequest(t, service, alice, http.MethodDelete, "/sessions/"+id, "")
	if response.Code != http.StatusNoContent {
		t.Fatalf("delete status=%d body=%s", response.Code, response.Body.String())
	}
	if response = codexRequest(t, service, alice, http.MethodGet, "/sessions/"+id, ""); response.Code != http.StatusNotFound {
		t.Fatalf("deleted get status=%d body=%s", response.Code, response.Body.String())
	}
	if response = codexRequest(t, service, alice, http.MethodDelete, "/sessions/missing", ""); response.Code != http.StatusNotFound {
		t.Fatalf("missing delete status=%d body=%s", response.Code, response.Body.String())
	}
}

func TestCodexInjectedRPCFailureResponseMatrix(t *testing.T) {
	for _, tc := range []struct {
		name, failMethod, method, target, body string
		createFirst                            bool
		want                                   int
	}{
		{"models", "model/list", http.MethodGet, "/models", "", false, http.StatusBadGateway},
		{"device login", "account/login/start", http.MethodPost, "/login/device", `{}`, false, http.StatusBadGateway},
		{"API key login", "account/login/start", http.MethodPost, "/login/api-key", `{"api_key":"key"}`, false, http.StatusBadGateway},
		{"logout", "account/logout", http.MethodPost, "/logout", `{}`, false, http.StatusBadGateway},
		{"thread start", "thread/start", http.MethodPost, "/sessions", `{"text":"cannot start"}`, false, http.StatusBadGateway},
		{"thread read", "thread/read", http.MethodGet, "/sessions/{id}", "", true, http.StatusBadGateway},
		{"thread rename", "thread/name/set", http.MethodPatch, "/sessions/{id}", `{"title":"cannot rename"}`, true, http.StatusBadGateway},
		{"thread archive", "thread/archive", http.MethodPost, "/sessions/{id}/archive", `{"archived":true}`, true, http.StatusBadGateway},
		{"thread unarchive", "thread/unarchive", http.MethodPost, "/sessions/{id}/archive", `{"archived":false}`, true, http.StatusBadGateway},
		{"thread delete", "thread/delete", http.MethodDelete, "/sessions/{id}", "", true, http.StatusBadGateway},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Setenv("DAR_TEST_CODEX_FAIL_METHOD", tc.failMethod)
			service, alice, _ := testCodexService(t)
			target := tc.target
			if tc.createFirst {
				created := codexRequest(t, service, alice, http.MethodPost, "/sessions", `{"text":"created before failure"}`)
				if created.Code != http.StatusCreated {
					t.Fatalf("setup create status=%d body=%s", created.Code, created.Body.String())
				}
				var payload struct {
					Session Session `json:"session"`
				}
				if err := json.Unmarshal(created.Body.Bytes(), &payload); err != nil {
					t.Fatal(err)
				}
				target = strings.ReplaceAll(target, "{id}", payload.Session.ID)
			}
			response := codexRequest(t, service, alice, tc.method, target, tc.body)
			if response.Code != tc.want {
				t.Fatalf("%s %s status=%d want=%d body=%s", tc.method, target, response.Code, tc.want, response.Body.String())
			}
		})
	}

	t.Run("turn start after creation", func(t *testing.T) {
		t.Setenv("DAR_TEST_CODEX_FAIL_TURN_AFTER", "1")
		service, alice, _ := testCodexService(t)
		created := codexRequest(t, service, alice, http.MethodPost, "/sessions", `{"text":"first turn succeeds"}`)
		if created.Code != http.StatusCreated {
			t.Fatalf("setup create status=%d body=%s", created.Code, created.Body.String())
		}
		var payload struct {
			Session Session `json:"session"`
		}
		if err := json.Unmarshal(created.Body.Bytes(), &payload); err != nil {
			t.Fatal(err)
		}
		failed := codexRequest(t, service, alice, http.MethodPost, "/sessions/"+payload.Session.ID+"/turns", `{"text":"second turn fails"}`)
		if failed.Code != http.StatusBadGateway {
			t.Fatalf("failed turn status=%d body=%s", failed.Code, failed.Body.String())
		}
	})
}

func TestCodexUnavailableClientHandlerAndCapacityMatrix(t *testing.T) {
	service, alice, bob := testCodexService(t)
	created := codexRequest(t, service, alice, http.MethodPost, "/sessions", `{"text":"exists before outage"}`)
	if created.Code != http.StatusCreated {
		t.Fatalf("setup create status=%d body=%s", created.Code, created.Body.String())
	}
	var payload struct {
		Session Session `json:"session"`
	}
	if err := json.Unmarshal(created.Body.Bytes(), &payload); err != nil {
		t.Fatal(err)
	}
	original := service.manager
	service.manager = newProcessManager(managerConfig{Path: filepath.Join(t.TempDir(), "missing-codex"), HomeRoot: t.TempDir()})
	for _, tc := range []struct {
		method, target, body string
		want                 int
	}{
		{http.MethodPost, "/login/device", `{}`, http.StatusServiceUnavailable},
		{http.MethodPost, "/login/api-key", `{"api_key":"key"}`, http.StatusServiceUnavailable},
		{http.MethodPost, "/logout", `{}`, http.StatusBadGateway},
		{http.MethodPost, "/sessions", `{"text":"new during outage"}`, http.StatusServiceUnavailable},
		{http.MethodGet, "/sessions/" + payload.Session.ID, "", http.StatusServiceUnavailable},
		{http.MethodPatch, "/sessions/" + payload.Session.ID, `{"title":"outage"}`, http.StatusServiceUnavailable},
		{http.MethodDelete, "/sessions/" + payload.Session.ID, "", http.StatusServiceUnavailable},
		{http.MethodPost, "/sessions/" + payload.Session.ID + "/archive", `{"archived":true}`, http.StatusServiceUnavailable},
		{http.MethodPost, "/sessions/" + payload.Session.ID + "/turns", `{"text":"during outage"}`, http.StatusServiceUnavailable},
	} {
		response := codexRequest(t, service, alice, tc.method, tc.target, tc.body)
		if response.Code != tc.want {
			t.Errorf("%s %s status=%d want=%d body=%s", tc.method, tc.target, response.Code, tc.want, response.Body.String())
		}
	}
	service.manager = original
	service.manager.config.MaxProcesses = 1
	if _, err := service.client(context.Background(), alice); err != nil {
		t.Fatal(err)
	}
	service.manager.attach(alice)
	defer service.manager.detach(alice)
	capacity := httptest.NewRecorder()
	service.handleStatus(capacity, httptest.NewRequest(http.MethodGet, "/status", nil), bob)
	if capacity.Code != http.StatusServiceUnavailable || capacity.Header().Get("Retry-After") != "5" {
		t.Fatalf("capacity status=%d headers=%v body=%s", capacity.Code, capacity.Header(), capacity.Body.String())
	}
}

func TestCodexAttachmentUploadFilesystemFailureMatrix(t *testing.T) {
	service, alice, _ := testCodexService(t)
	upload := func(target *Service, filename string) *httptest.ResponseRecorder {
		t.Helper()
		var body bytes.Buffer
		writer := multipart.NewWriter(&body)
		header := textproto.MIMEHeader{}
		header.Set("Content-Disposition", `form-data; name="file"; filename="`+filename+`"`)
		header.Set("Content-Type", "image/png")
		part, err := writer.CreatePart(header)
		if err != nil {
			t.Fatal(err)
		}
		_, _ = part.Write([]byte("\x89PNG\r\n\x1a\n"))
		_ = writer.Close()
		req := httptest.NewRequest(http.MethodPost, "/attachments/upload", &body)
		req.Header.Set("Content-Type", writer.FormDataContentType())
		rec := httptest.NewRecorder()
		target.handleAttachmentUpload(rec, req, alice)
		return rec
	}
	var noFile bytes.Buffer
	writer := multipart.NewWriter(&noFile)
	_ = writer.WriteField("description", "no image")
	_ = writer.Close()
	req := httptest.NewRequest(http.MethodPost, "/attachments/upload", &noFile)
	req.Header.Set("Content-Type", writer.FormDataContentType())
	rec := httptest.NewRecorder()
	service.handleAttachmentUpload(rec, req, alice)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("missing upload file status=%d body=%s", rec.Code, rec.Body.String())
	}
	if rec = upload(service, "image.verylongextension"); rec.Code != http.StatusCreated {
		t.Fatalf("long extension upload status=%d body=%s", rec.Code, rec.Body.String())
	}
	blocker := filepath.Join(t.TempDir(), "home-blocker")
	if err := os.WriteFile(blocker, []byte("blocked"), 0o600); err != nil {
		t.Fatal(err)
	}
	originalManager := service.manager
	service.manager = newProcessManager(managerConfig{Path: originalManager.config.Path, HomeRoot: blocker})
	if rec = upload(service, "image.png"); rec.Code != http.StatusInternalServerError {
		t.Fatalf("workspace failure status=%d body=%s", rec.Code, rec.Body.String())
	}
	service.manager = originalManager
	_, workspace, err := service.manager.userDirs(alice)
	if err != nil {
		t.Fatal(err)
	}
	attachmentsPath := filepath.Join(workspace, "attachments")
	if err := os.RemoveAll(attachmentsPath); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(attachmentsPath, []byte("blocked"), 0o600); err != nil {
		t.Fatal(err)
	}
	if rec = upload(service, "image.png"); rec.Code != http.StatusInternalServerError {
		t.Fatalf("attachment directory failure status=%d body=%s", rec.Code, rec.Body.String())
	}
	malformed := codexRequest(t, service, alice, http.MethodPost, "/attachments/select", `{`)
	if malformed.Code != http.StatusBadRequest {
		t.Fatalf("malformed selection status=%d body=%s", malformed.Code, malformed.Body.String())
	}
}

type codexSearchSink struct {
	documents []apps.SearchDocument
}

func (s *codexSearchSink) AddSearchDocument(_ context.Context, document apps.SearchDocument) error {
	s.documents = append(s.documents, document)
	return nil
}

func TestCodexSearchIndexAndValueExtraction(t *testing.T) {
	service, alice, _ := testCodexService(t)
	response := codexRequest(t, service, alice, http.MethodPost, "/sessions", `{"text":"searchable matrix text"}`)
	if response.Code != http.StatusCreated {
		t.Fatalf("create status=%d body=%s", response.Code, response.Body.String())
	}
	sink := &codexSearchSink{}
	if err := service.indexSearch(context.Background(), alice, sink); err != nil {
		t.Fatal(err)
	}
	if len(sink.documents) == 0 {
		t.Fatal("Codex session produced no search documents")
	}
	values := []map[string]any{
		map[string]any{"text": "direct"},
		map[string]any{"content": []any{map[string]any{"text": "nested"}}},
	}
	for _, value := range values {
		if firstCodexText(value) == "" {
			t.Errorf("no text extracted from %#v", value)
		}
	}
	if len(collectCodexMessages([]any{map[string]any{"type": "userMessage", "text": "array"}})) != 1 {
		t.Fatal("Codex message collection regression")
	}
	if firstString(map[string]any{"one": "", "two": "value"}, "one", "two") != "value" || stringValue(3) != "" {
		t.Fatal("Codex value extraction regression")
	}
	app := &App{service: service}
	wrapperSink := &codexSearchSink{}
	if err := app.IndexSearch(context.Background(), alice, wrapperSink); err != nil || len(wrapperSink.documents) == 0 {
		t.Fatalf("app search wrapper: docs=%d err=%v", len(wrapperSink.documents), err)
	}
}

func TestCodexApprovalInterruptCrashAndStreamingFailures(t *testing.T) {
	service, alice, _ := testCodexService(t)
	response := codexRequest(t, service, alice, http.MethodPost, "/sessions", `{"text":"control flow"}`)
	if response.Code != http.StatusCreated {
		t.Fatalf("create: %d %s", response.Code, response.Body.String())
	}
	var created struct {
		Session Session `json:"session"`
	}
	if err := json.Unmarshal(response.Body.Bytes(), &created); err != nil {
		t.Fatal(err)
	}
	client, err := service.client(context.Background(), alice)
	if err != nil {
		t.Fatal(err)
	}

	service.mu.Lock()
	service.pending["deny-call"] = &pendingApproval{
		UserID: alice, SessionID: created.Session.ID, CallID: "deny-call",
		Client: client, RequestID: json.RawMessage(`991`),
	}
	service.mu.Unlock()
	response = codexRequest(t, service, alice, http.MethodPost, "/approvals/deny-call", `{"decision":"deny"}`)
	if response.Code != http.StatusOK {
		t.Fatalf("deny approval: %d %s", response.Code, response.Body.String())
	}
	service.mu.Lock()
	if service.pending["deny-call"] != nil {
		service.mu.Unlock()
		t.Fatal("denied approval remained pending")
	}
	service.active[hubKey(alice, created.Session.ID)] = "manual-turn"
	service.mu.Unlock()
	response = codexRequest(t, service, alice, http.MethodPost, "/sessions/"+created.Session.ID+"/interrupt", `{}`)
	if response.Code != http.StatusOK {
		t.Fatalf("interrupt: %d %s", response.Code, response.Body.String())
	}

	temporary := filepath.Join(t.TempDir(), "turn-file")
	if err := os.WriteFile(temporary, []byte("temporary"), 0600); err != nil {
		t.Fatal(err)
	}
	service.mu.Lock()
	service.active[hubKey(alice, created.Session.ID)] = "crashed-turn"
	service.turnFiles[hubKey(alice, created.Session.ID)] = []string{temporary}
	service.pending["crash-call"] = &pendingApproval{UserID: alice, SessionID: created.Session.ID}
	service.mu.Unlock()
	service.onProcessCrash(alice)
	if _, err := os.Stat(temporary); !os.IsNotExist(err) {
		t.Fatalf("crash cleanup left temporary file: %v", err)
	}

	writer := &nonFlushingWriter{header: make(http.Header)}
	request := httptest.NewRequest(http.MethodGet, "/api/codex/sessions/"+created.Session.ID+"/events", nil)
	service.handleEvents(writer, request, alice, created.Session.ID)
	if writer.status != http.StatusInternalServerError {
		t.Fatalf("non-streaming writer status = %d body=%s", writer.status, writer.body.String())
	}

	originalManager := service.manager
	service.manager = newProcessManager(managerConfig{Path: filepath.Join(t.TempDir(), "missing-codex"), HomeRoot: t.TempDir()})
	statusRec := httptest.NewRecorder()
	service.handleStatus(statusRec, httptest.NewRequest(http.MethodGet, "/status", nil), alice)
	if statusRec.Code != http.StatusOK || !strings.Contains(statusRec.Body.String(), `"available":false`) {
		t.Fatalf("unavailable status: %d %s", statusRec.Code, statusRec.Body.String())
	}
	modelsRec := httptest.NewRecorder()
	service.handleModels(modelsRec, httptest.NewRequest(http.MethodGet, "/models", nil), alice)
	if modelsRec.Code != http.StatusServiceUnavailable {
		t.Fatalf("unavailable models = %d %s", modelsRec.Code, modelsRec.Body.String())
	}
	service.manager = originalManager
}

type nonFlushingWriter struct {
	header http.Header
	body   bytes.Buffer
	status int
}

func (w *nonFlushingWriter) Header() http.Header { return w.header }
func (w *nonFlushingWriter) Write(data []byte) (int, error) {
	if w.status == 0 {
		w.status = http.StatusOK
	}
	return w.body.Write(data)
}
func (w *nonFlushingWriter) WriteHeader(status int) { w.status = status }

func containsStatus(values []int, value int) bool {
	for _, candidate := range values {
		if candidate == value {
			return true
		}
	}
	return false
}

type matrixAssistantProvider struct{}

func (matrixAssistantProvider) AssistantTools() []apps.AssistantTool {
	return []apps.AssistantTool{{Name: "read", Effect: apps.AssistantRead}}
}

type dynamicToolProvider struct{}

func (dynamicToolProvider) AssistantTools() []apps.AssistantTool {
	return []apps.AssistantTool{
		{Name: "valid", Description: "Valid tool", InputSchema: json.RawMessage(`{"type":"object"}`), Effect: apps.AssistantRead},
		{Name: "invalid", Description: "Invalid tool", InputSchema: json.RawMessage(`{`), Effect: apps.AssistantRead},
	}
}
func (dynamicToolProvider) CallAssistantTool(context.Context, string, string, json.RawMessage) (apps.AssistantResult, error) {
	return apps.AssistantResult{}, nil
}

func TestCodexDynamicToolsActivityAndClosedDatabase(t *testing.T) {
	service, alice, _ := testCodexService(t)
	service.config.Registry.RegisterAssistantProvider("zeta", dynamicToolProvider{})
	service.config.Registry.RegisterAssistantProvider("empty", matrixAssistantProvider{})
	tools := service.dynamicTools()
	if len(tools) != 1 || tools[0]["name"] != "zeta" {
		t.Fatalf("dynamic tools=%+v", tools)
	}

	sink := &testutil.ActivitySink{}
	service.config.Registry = testutil.RegistryWithActivity(sink, &App{service: service})
	service.recordActivity(context.Background(), alice, "chat_created", "Created chat", "s1", "Chat")
	service.recordActivity(context.Background(), "", "ignored", "Ignored", "s2", "Chat")
	sink.Err = errors.New("recording failed")
	service.recordActivity(context.Background(), alice, "chat_updated", "Updated chat", "s1", "Chat")
	if len(sink.Events) != 2 {
		t.Fatalf("activity events=%+v", sink.Events)
	}

	if err := service.config.DB.Close(); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		method string
		path   string
		body   string
	}{
		{http.MethodGet, "/preferences", ""},
		{http.MethodPut, "/preferences", `{"approval_mode":"confirm"}`},
		{http.MethodGet, "/sessions", ""},
		{http.MethodPost, "/sessions", `{"text":"closed"}`},
		{http.MethodGet, "/sessions/missing", ""},
		{http.MethodPatch, "/sessions/missing", `{"title":"closed"}`},
		{http.MethodDelete, "/sessions/missing", ""},
		{http.MethodPost, "/sessions/missing/archive", `{"archived":true}`},
		{http.MethodPost, "/sessions/missing/turns", `{"text":"closed"}`},
		{http.MethodPost, "/sessions/missing/interrupt", `{}`},
	} {
		rec := codexRequest(t, service, alice, tc.method, tc.path, tc.body)
		if rec.Code < 400 {
			t.Errorf("%s %s unexpectedly returned %d: %s", tc.method, tc.path, rec.Code, rec.Body.String())
		}
	}
}
func (matrixAssistantProvider) CallAssistantTool(context.Context, string, string, json.RawMessage) (apps.AssistantResult, error) {
	return apps.AssistantResult{}, nil
}

type attachmentAssistantProvider struct {
	result apps.AssistantResult
	err    error
}

func (p attachmentAssistantProvider) AssistantTools() []apps.AssistantTool {
	return []apps.AssistantTool{{Name: "read", Effect: apps.AssistantRead}}
}

func (p attachmentAssistantProvider) CallAssistantTool(context.Context, string, string, json.RawMessage) (apps.AssistantResult, error) {
	return p.result, p.err
}

func TestCodexHelperAndDataImageBoundaries(t *testing.T) {
	anonymous := httptest.NewRecorder()
	(&Service{}).handlePage(anonymous, httptest.NewRequest(http.MethodGet, "/codex", nil))
	if anonymous.Code != http.StatusSeeOther {
		t.Fatalf("anonymous page status=%d", anonymous.Code)
	}
	service, alice, _ := testCodexService(t)
	path, err := service.materializeDataImage(alice, "data:image/png;base64,iVBORw0KGgo=")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatal(err)
	}
	for _, raw := range []string{"", "data:text/plain;base64,YQ==", "data:image/png,plain", "data:image/png;base64,!"} {
		if _, err := service.materializeDataImage(alice, raw); err == nil {
			t.Errorf("invalid data image accepted: %q", raw)
		}
	}
	for input, want := range map[string]string{
		"turn/started":            "turn.started",
		"turn/completed":          "turn.completed",
		"item/agentMessage/delta": "message.delta",
		"item/started":            "item.started",
		"item/completed":          "item.completed",
		"error":                   "error",
		"thread/name/updated":     "session.renamed",
		"unknown":                 "",
	} {
		if got := mapEventType(input); got != want {
			t.Errorf("mapEventType(%q)=%q want=%q", input, got, want)
		}
	}
	provider := matrixAssistantProvider{}
	if _, ok := findTool(provider, "read"); !ok {
		t.Fatal("declared tool not found")
	}
	if _, ok := findTool(provider, "missing"); ok {
		t.Fatal("missing tool found")
	}
	if _, ok := findTool(nil, "read"); ok {
		t.Fatal("tool found on nil provider")
	}
	if firstNonEmpty("", "value", "later") != "value" || firstNonEmpty("", "") != "" {
		t.Fatal("firstNonEmpty regression")
	}
	if errorString(nil) != "" || errorString(errors.New("boom")) != "boom" {
		t.Fatal("errorString regression")
	}
	if statusFor(errNotFound) != http.StatusNotFound || statusFor(errors.New("boom")) != http.StatusInternalServerError {
		t.Fatal("status mapping regression")
	}
	env := withEnv([]string{"A=1", "B=2", "A=old"}, "A", "new")
	if strings.Join(env, ",") != "B=2,A=new" {
		t.Fatalf("withEnv = %v", env)
	}
	var stream bytes.Buffer
	writeSSE(&stream, streamEvent{ID: 7, Type: "matrix", Data: json.RawMessage(`{"ok":true}`)})
	if !strings.Contains(stream.String(), "id: 7") {
		t.Fatalf("SSE = %q", stream.String())
	}
	service.config.CSRFToken = func(*http.Request) string { return "csrf" }
	if service.csrf(httptest.NewRequest(http.MethodGet, "/", nil)) != "csrf" {
		t.Fatal("CSRF callback not used")
	}
	service.config.CSRFToken = nil
	if service.csrf(httptest.NewRequest(http.MethodGet, "/", nil)) != "" {
		t.Fatal("nil CSRF callback did not return empty")
	}

	input, paths := service.attachmentInput(context.Background(), alice, attachment{Kind: "image", Path: "/tmp/pixel.png"})
	if len(input) != 1 || input[0]["type"] != "localImage" || len(paths) != 1 {
		t.Fatalf("image attachment input=%v paths=%v", input, paths)
	}
	input, paths = service.attachmentInput(context.Background(), alice, attachment{Name: "/Documents/missing.txt"})
	if len(input) != 1 || !strings.Contains(input[0]["text"].(string), "Selected Dar item") || len(paths) != 0 {
		t.Fatalf("missing provider input=%v paths=%v", input, paths)
	}
	service.config.Registry.RegisterAssistantProvider("files", attachmentAssistantProvider{err: errors.New("unavailable")})
	input, _ = service.attachmentInput(context.Background(), alice, attachment{Name: "/Documents/unavailable.txt"})
	if len(input) != 1 || !strings.Contains(input[0]["text"].(string), "content unavailable") {
		t.Fatalf("provider error input=%v", input)
	}

	registry := apps.NewRegistry()
	registry.RegisterAssistantProvider("files", attachmentAssistantProvider{result: apps.AssistantResult{Content: []apps.AssistantContent{
		{Type: "text", Text: "hello"},
		{Type: "text", Text: string([]byte{0xff})},
		{Type: "image", ImageURL: "data:image/png;base64,iVBORw0KGgo="},
		{Type: "image", ImageURL: "invalid"},
	}}})
	service.config.Registry = registry
	input, paths = service.attachmentInput(context.Background(), alice, attachment{Name: "/Documents/content.bin"})
	if len(input) != 2 || len(paths) != 1 || input[0]["type"] != "text" || input[1]["type"] != "localImage" {
		t.Fatalf("provider content input=%v paths=%v", input, paths)
	}
	registry = apps.NewRegistry()
	registry.RegisterAssistantProvider("files", matrixAssistantProvider{})
	service.config.Registry = registry
	input, paths = service.attachmentInput(context.Background(), alice, attachment{Name: "/Documents/binary.bin"})
	if len(input) != 1 || !strings.Contains(input[0]["text"].(string), "binary item") || len(paths) != 0 {
		t.Fatalf("empty provider input=%v paths=%v", input, paths)
	}

	if _, _, _, err := service.prepareTurnInput(context.Background(), alice, turnRequest{Attachments: []string{"missing"}}); err == nil {
		t.Fatal("turn with no valid input accepted")
	}
	service.mu.Lock()
	service.attachments["alice-item"] = attachment{UserID: alice, Name: "/Documents/a.txt", Kind: "dar"}
	service.attachments["bob-item"] = attachment{UserID: "bob", Name: "/Documents/b.txt", Kind: "dar"}
	service.mu.Unlock()
	service.config.Registry = apps.NewRegistry()
	prepared, used, _, err := service.prepareTurnInput(context.Background(), alice, turnRequest{
		Text: "  prompt  ", Attachments: []string{"alice-item", "bob-item"},
	})
	if err != nil || len(prepared) != 2 || len(used) != 1 || used[0] != "alice-item" {
		t.Fatalf("prepared input=%v used=%v err=%v", prepared, used, err)
	}
	temp := filepath.Join(t.TempDir(), "completed-turn")
	if err := os.WriteFile(temp, []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	key := hubKey(alice, "completed-session")
	service.mu.Lock()
	service.finished[key] = "turn-finished"
	service.mu.Unlock()
	service.commitTurn(alice, "completed-session", []string{"alice-item"}, []string{temp}, "turn-finished")
	if _, err := os.Stat(temp); !os.IsNotExist(err) {
		t.Fatalf("completed turn file remained: %v", err)
	}
	service.commitTurn(alice, "active-session", nil, nil, "turn-active")
	service.mu.Lock()
	active := service.active[hubKey(alice, "active-session")]
	service.mu.Unlock()
	if active != "turn-active" {
		t.Fatalf("active turn=%q", active)
	}
	if isConfirmedEmptyThreadError(errors.New("plain")) {
		t.Fatal("plain error classified as empty thread")
	}
	for _, message := range []string{"not materialized yet", "no rollout found", "thread not found", "no thread found", "missing thread", "thread does not exist"} {
		if !isConfirmedEmptyThreadError(&rpcCallError{Message: message}) {
			t.Errorf("empty-thread message not recognized: %q", message)
		}
	}
	capacity := httptest.NewRecorder()
	apiError(capacity, errProcessCapacity, http.StatusInternalServerError)
	if capacity.Code != http.StatusServiceUnavailable || capacity.Header().Get("Retry-After") != "5" {
		t.Fatalf("capacity response=%d headers=%v body=%s", capacity.Code, capacity.Header(), capacity.Body.String())
	}

	badUpload := httptest.NewRecorder()
	service.handleAttachmentUpload(badUpload, httptest.NewRequest(http.MethodPost, "/upload", strings.NewReader("bad")), alice)
	if badUpload.Code != http.StatusBadRequest {
		t.Fatalf("bad upload status=%d", badUpload.Code)
	}
	var textUpload bytes.Buffer
	textWriter := multipart.NewWriter(&textUpload)
	textHeader := textproto.MIMEHeader{}
	textHeader.Set("Content-Disposition", `form-data; name="file"; filename="note.txt"`)
	textHeader.Set("Content-Type", "text/plain")
	textPart, err := textWriter.CreatePart(textHeader)
	if err != nil {
		t.Fatal(err)
	}
	_, _ = textPart.Write([]byte("not an image"))
	_ = textWriter.Close()
	textRequest := httptest.NewRequest(http.MethodPost, "/upload", &textUpload)
	textRequest.Header.Set("Content-Type", textWriter.FormDataContentType())
	textResponse := httptest.NewRecorder()
	service.handleAttachmentUpload(textResponse, textRequest, alice)
	if textResponse.Code != http.StatusBadRequest {
		t.Fatalf("text upload status=%d body=%s", textResponse.Code, textResponse.Body.String())
	}
}
