package codex

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/server"
)

func TestClientDiagnostic(t *testing.T) {
	tests := []struct {
		name string
		err  error
		want string
	}{
		{
			name: "executable startup failure",
			err:  &os.PathError{Op: "fork/exec", Path: "/secret/codex", Err: os.ErrPermission},
			want: "The configured Codex executable could not be started.",
		},
		{
			name: "initialization timeout",
			err:  fmt.Errorf("initialize Codex app-server: %w", context.DeadlineExceeded),
			want: "Codex app-server initialization timed out.",
		},
		{
			name: "unexpected home",
			err:  fmt.Errorf("initialize Codex app-server: %w", errUnexpectedCodexHome),
			want: "Codex app-server did not use Dar's configured per-user home.",
		},
		{
			name: "generic initialization failure",
			err:  errors.New("RPC contained sk-secretvalue and /private/path"),
			want: "Codex app-server initialization failed. Check the Dar server logs.",
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			got := clientDiagnostic(test.err)
			if got != test.want {
				t.Fatalf("clientDiagnostic = %q, want %q", got, test.want)
			}
			if strings.Contains(got, "secret") || strings.Contains(got, "/private") {
				t.Fatalf("clientDiagnostic leaked internal detail: %q", got)
			}
		})
	}
}

func TestAvailabilityDiagnostic(t *testing.T) {
	if got := availabilityDiagnostic(&exec.Error{Name: "/private/codex", Err: exec.ErrNotFound}, ""); !strings.Contains(got, "DAR_CODEX_PATH") || strings.Contains(got, "/private") {
		t.Fatalf("executable diagnostic = %q", got)
	}
	if got := availabilityDiagnostic(errors.New("old"), "0.143.9"); got != "Codex CLI 0.144.1 or newer is required (found 0.143.9)." {
		t.Fatalf("old version diagnostic = %q", got)
	}
	if got := availabilityDiagnostic(context.DeadlineExceeded, ""); got != "The Codex CLI version check timed out." {
		t.Fatalf("timeout diagnostic = %q", got)
	}
}

func testCodexService(t *testing.T) (*Service, string, string) {
	t.Helper()
	database, err := db.Open(filepath.Join(t.TempDir(), "codex-service.sqlite"), append(auth.Migrations(), Migrations()...))
	if err != nil {
		t.Fatal(err)
	}
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	userIDs := []string{}
	for _, name := range []string{"alice", "bob"} {
		user, _, err := auth.FindOrCreateUser(context.Background(), conn, name, name+"@example.test", name, "", "https://issuer.example", name)
		if err != nil {
			t.Fatal(err)
		}
		userIDs = append(userIDs, user.ID)
	}
	release()
	path, err := filepath.Abs(filepath.Join("testdata", "fake-codex"))
	if err != nil {
		t.Fatal(err)
	}
	service := newService(Config{
		DB:          database,
		Registry:    apps.NewRegistry(),
		CodexPath:   path,
		HomeRoot:    filepath.Join(t.TempDir(), "homes"),
		IdleTimeout: time.Hour,
	})
	if err := service.Start(context.Background()); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		_ = service.Shutdown(ctx)
		_ = database.Close()
	})
	return service, userIDs[0], userIDs[1]
}

func codexRequest(t *testing.T, service *Service, userID, method, path, body string) *httptest.ResponseRecorder {
	t.Helper()
	request := httptest.NewRequest(method, "/api/codex"+path, bytes.NewBufferString(body))
	request = request.WithContext(context.WithValue(request.Context(), server.UserContextKey, &auth.User{ID: userID}))
	response := httptest.NewRecorder()
	service.handleAPI(response, request)
	return response
}

func TestFirstMessageCreatesMaterializedSessionAndAvoidsRedundantResume(t *testing.T) {
	logPath := filepath.Join(t.TempDir(), "protocol.log")
	t.Setenv("DAR_TEST_CODEX_LOG", logPath)
	service, alice, _ := testCodexService(t)

	response := codexRequest(t, service, alice, http.MethodPost, "/sessions", `{"text":"hello","model":"fake-model","reasoningEffort":"medium"}`)
	if response.Code != http.StatusCreated {
		t.Fatalf("create status=%d body=%s", response.Code, response.Body.String())
	}
	var created struct {
		Session Session `json:"session"`
		TurnID  string  `json:"turn_id"`
	}
	if err := json.Unmarshal(response.Body.Bytes(), &created); err != nil {
		t.Fatal(err)
	}
	if created.Session.ID == "" || created.TurnID == "" {
		t.Fatalf("create response=%#v", created)
	}

	hub := service.hub(alice, created.Session.ID)
	ch, replay, _, unsubscribe := hub.subscribe(0)
	defer unsubscribe()
	if len(replay) == 0 {
		select {
		case <-ch:
		case <-time.After(2 * time.Second):
			t.Fatal("first-turn events were not buffered")
		}
	}

	response = codexRequest(t, service, alice, http.MethodPost, "/sessions/"+created.Session.ID+"/turns", `{"text":"again"}`)
	if response.Code != http.StatusAccepted {
		t.Fatalf("second turn status=%d body=%s", response.Code, response.Body.String())
	}
	response = codexRequest(t, service, alice, http.MethodGet, "/sessions/"+created.Session.ID, "")
	if response.Code != http.StatusOK {
		t.Fatalf("open loaded session status=%d body=%s", response.Code, response.Body.String())
	}
	logged, err := os.ReadFile(logPath)
	if err != nil {
		t.Fatal(err)
	}
	logText := string(logged)
	if strings.Count(logText, `"method":"thread/start"`) != 1 || strings.Count(logText, `"method":"turn/start"`) != 2 {
		t.Fatalf("unexpected lifecycle calls:\n%s", logText)
	}
	if strings.Contains(logText, `"method":"thread/resume"`) {
		t.Fatalf("loaded session was redundantly resumed:\n%s", logText)
	}
	if !strings.Contains(logText, `"method":"thread/read"`) {
		t.Fatalf("loaded history did not use thread/read:\n%s", logText)
	}

	service.manager.stopUser(alice)
	response = codexRequest(t, service, alice, http.MethodGet, "/sessions/"+created.Session.ID, "")
	if response.Code != http.StatusOK {
		t.Fatalf("reopen after restart status=%d body=%s", response.Code, response.Body.String())
	}
	logged, _ = os.ReadFile(logPath)
	if strings.Count(string(logged), `"method":"thread/resume"`) != 1 {
		t.Fatalf("restart did not resume exactly once:\n%s", logged)
	}
}

func TestFirstMessageRollbackRetainsAttachments(t *testing.T) {
	t.Setenv("DAR_TEST_CODEX_FAIL_TURN", "1")
	service, alice, _ := testCodexService(t)
	service.mu.Lock()
	service.attachments["attachment-1"] = attachment{UserID: alice, Name: "notes.txt", Kind: "dar"}
	service.mu.Unlock()

	response := codexRequest(t, service, alice, http.MethodPost, "/sessions", `{"text":"hello","attachments":["attachment-1"]}`)
	if response.Code != http.StatusBadGateway || !strings.Contains(response.Body.String(), "kept") {
		t.Fatalf("rollback status=%d body=%s", response.Code, response.Body.String())
	}
	sessions, err := service.store.listSessions(context.Background(), alice, false)
	if err != nil || len(sessions) != 0 {
		t.Fatalf("rolled-back sessions=%#v err=%v", sessions, err)
	}
	service.mu.Lock()
	_, retained := service.attachments["attachment-1"]
	service.mu.Unlock()
	if !retained {
		t.Fatal("attachment was consumed after first-turn failure")
	}
}

func TestLegacyEmptySessionCleanupIsConservative(t *testing.T) {
	t.Run("confirmed empty is removed on open", func(t *testing.T) {
		t.Setenv("DAR_TEST_CODEX_RESUME_ERROR", "empty")
		service, alice, _ := testCodexService(t)
		item, err := service.store.createSession(context.Background(), alice, "legacy-empty", "", "")
		if err != nil {
			t.Fatal(err)
		}
		response := codexRequest(t, service, alice, http.MethodGet, "/sessions/"+item.ID, "")
		if response.Code != http.StatusGone {
			t.Fatalf("open status=%d body=%s", response.Code, response.Body.String())
		}
		if _, err := service.store.getSession(context.Background(), alice, item.ID); !errors.Is(err, errNotFound) {
			t.Fatalf("confirmed empty session remains: %v", err)
		}
	})
	t.Run("generic failure preserves data", func(t *testing.T) {
		t.Setenv("DAR_TEST_CODEX_RESUME_ERROR", "generic")
		service, alice, _ := testCodexService(t)
		item, err := service.store.createSession(context.Background(), alice, "recoverable", "", "")
		if err != nil {
			t.Fatal(err)
		}
		response := codexRequest(t, service, alice, http.MethodGet, "/sessions/"+item.ID, "")
		if response.Code != http.StatusBadGateway {
			t.Fatalf("open status=%d body=%s", response.Code, response.Body.String())
		}
		if _, err := service.store.getSession(context.Background(), alice, item.ID); err != nil {
			t.Fatalf("generic failure deleted data: %v", err)
		}
	})
}

func TestArchiveCleansConfirmedEmptySessionAndEnforcesOwnership(t *testing.T) {
	t.Setenv("DAR_TEST_CODEX_ARCHIVE_ERROR", "empty")
	service, alice, bob := testCodexService(t)
	item, err := service.store.createSession(context.Background(), alice, "legacy-empty", "", "")
	if err != nil {
		t.Fatal(err)
	}
	response := codexRequest(t, service, bob, http.MethodPost, "/sessions/"+item.ID+"/archive", `{"archived":true}`)
	if response.Code != http.StatusNotFound {
		t.Fatalf("cross-user archive status=%d body=%s", response.Code, response.Body.String())
	}
	if _, err := service.store.getSession(context.Background(), alice, item.ID); err != nil {
		t.Fatalf("cross-user archive changed session: %v", err)
	}
	response = codexRequest(t, service, alice, http.MethodPost, "/sessions/"+item.ID+"/archive", `{"archived":true}`)
	if response.Code != http.StatusOK {
		t.Fatalf("empty archive status=%d body=%s", response.Code, response.Body.String())
	}
	if _, err := service.store.getSession(context.Background(), alice, item.ID); !errors.Is(err, errNotFound) {
		t.Fatalf("empty session was not locally removed: %v", err)
	}
}
