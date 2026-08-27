package apps

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"testing"

	"github.com/anoosa/dar/internal/server"
)

func TestInvokeAssistantHTTPInjectsUserAndMapsErrors(t *testing.T) {
	handler := func(w http.ResponseWriter, r *http.Request) {
		if user := server.GetUser(r); user == nil || user.ID != "alice" {
			http.Error(w, "wrong user", http.StatusForbidden)
			return
		}
		if r.URL.Query().Get("q") != "hello" {
			http.Error(w, "missing query", http.StatusBadRequest)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"ok":true}`))
	}
	result, err := InvokeAssistantHTTP(context.Background(), "alice", handler, http.MethodGet, "/items", json.RawMessage(`{"q":"hello"}`))
	if err != nil || len(result.Content) != 1 || result.Content[0].Text != `{"ok":true}` {
		t.Fatalf("result=%#v err=%v", result, err)
	}
	_, err = InvokeAssistantHTTP(context.Background(), "alice", func(w http.ResponseWriter, _ *http.Request) { http.Error(w, "gone", http.StatusNotFound) }, http.MethodGet, "/items", nil)
	var assistantErr *AssistantError
	if !errors.As(err, &assistantErr) || assistantErr.Category != AssistantNotFound {
		t.Fatalf("error = %#v", err)
	}
}
