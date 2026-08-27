package codex

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
)

type approvalProvider struct{ calls int }

func (p *approvalProvider) AssistantTools() []apps.AssistantTool {
	schema := json.RawMessage(`{"type":"object","properties":{},"additionalProperties":false}`)
	return []apps.AssistantTool{{Name: "read", InputSchema: schema, Effect: apps.AssistantRead}, {Name: "write", InputSchema: schema, Effect: apps.AssistantSafeWrite}, {Name: "delete", InputSchema: schema, Effect: apps.AssistantDestructive}}
}
func (p *approvalProvider) CallAssistantTool(context.Context, string, string, json.RawMessage) (apps.AssistantResult, error) {
	p.calls++
	return apps.AssistantResult{Content: []apps.AssistantContent{{Type: "text", Text: `{"ok":true}`}}}, nil
}

type bufferWriteCloser struct{ bytes.Buffer }

func (b *bufferWriteCloser) Close() error { return nil }

func approvalClient() (*protocolClient, *bufferWriteCloser) {
	output := &bufferWriteCloser{}
	return &protocolClient{stdin: output, pending: map[uint64]chan rpcResult{}, closed: make(chan struct{}), exited: make(chan struct{}), writeMu: sync.Mutex{}}, output
}

type signalWriteCloser struct{ writes chan []byte }

func (w *signalWriteCloser) Write(data []byte) (int, error) {
	w.writes <- append([]byte(nil), data...)
	return len(data), nil
}
func (w *signalWriteCloser) Close() error { return nil }

func signalApprovalClient() (*protocolClient, *signalWriteCloser) {
	output := &signalWriteCloser{writes: make(chan []byte, 1)}
	return &protocolClient{stdin: output, pending: map[uint64]chan rpcResult{}, closed: make(chan struct{}), exited: make(chan struct{}), writeMu: sync.Mutex{}}, output
}

func TestApprovalPolicyAutomaticOnlyBypassesSafeWrites(t *testing.T) {
	storage, alice, _, closeStore := testStore(t)
	defer closeStore()
	session, err := storage.createSession(context.Background(), alice, "thread-1", "", "")
	if err != nil {
		t.Fatal(err)
	}
	registry := apps.NewRegistry()
	provider := &approvalProvider{}
	registry.RegisterAssistantProvider("test", provider)
	service := &Service{config: Config{Registry: registry}, store: storage, hubs: map[string]*eventHub{}, active: map[string]string{}, pending: map[string]*pendingApproval{}, attachments: map[string]attachment{}, turnFiles: map[string][]string{}}
	if err := storage.savePreferences(context.Background(), alice, Preferences{ApprovalMode: "automatic", WebSearch: "disabled"}); err != nil {
		t.Fatal(err)
	}

	client, output := approvalClient()
	service.onServerCall(alice, client, json.RawMessage(`1`), "item/tool/call", json.RawMessage(`{"callId":"safe-1","threadId":"thread-1","namespace":"test","tool":"write","arguments":{}}`))
	if provider.calls != 1 || output.Len() == 0 {
		t.Fatalf("safe write was not automatic: calls=%d output=%q", provider.calls, output.String())
	}

	client, output = approvalClient()
	service.onServerCall(alice, client, json.RawMessage(`2`), "item/tool/call", json.RawMessage(`{"callId":"delete-1","threadId":"thread-1","namespace":"test","tool":"delete","arguments":{}}`))
	if provider.calls != 1 || output.Len() != 0 || service.pending["delete-1"] == nil {
		t.Fatalf("destructive action bypassed approval: calls=%d output=%q pending=%v", provider.calls, output.String(), service.pending["delete-1"] != nil)
	}
	if service.pending["delete-1"].SessionID != session.ID {
		t.Fatal("approval associated with wrong session")
	}
}

func TestApprovalPolicyConfirmRequiresAllWritesButNotReads(t *testing.T) {
	storage, alice, _, closeStore := testStore(t)
	defer closeStore()
	if _, err := storage.createSession(context.Background(), alice, "thread-2", "", ""); err != nil {
		t.Fatal(err)
	}
	registry := apps.NewRegistry()
	provider := &approvalProvider{}
	registry.RegisterAssistantProvider("test", provider)
	service := &Service{config: Config{Registry: registry}, store: storage, hubs: map[string]*eventHub{}, active: map[string]string{}, pending: map[string]*pendingApproval{}, attachments: map[string]attachment{}, turnFiles: map[string][]string{}}
	client, _ := approvalClient()
	service.onServerCall(alice, client, json.RawMessage(`1`), "item/tool/call", json.RawMessage(`{"callId":"write-1","threadId":"thread-2","namespace":"test","tool":"write","arguments":{}}`))
	if provider.calls != 0 || service.pending["write-1"] == nil {
		t.Fatal("confirm mode did not hold safe write")
	}
	client, output := approvalClient()
	service.onServerCall(alice, client, json.RawMessage(`2`), "item/tool/call", json.RawMessage(`{"callId":"read-1","threadId":"thread-2","namespace":"test","tool":"read","arguments":{}}`))
	if provider.calls != 1 || output.Len() == 0 {
		t.Fatal("read unexpectedly required approval")
	}
}

func TestApprovalEndpointApproveDenyAndServerCallFailures(t *testing.T) {
	storage, alice, bob, closeStore := testStore(t)
	defer closeStore()
	session, err := storage.createSession(context.Background(), alice, "thread-endpoint", "", "")
	if err != nil {
		t.Fatal(err)
	}
	registry := apps.NewRegistry()
	provider := &approvalProvider{}
	registry.RegisterAssistantProvider("test", provider)
	service := &Service{
		config: Config{Registry: registry}, store: storage, hubs: map[string]*eventHub{},
		active: map[string]string{}, pending: map[string]*pendingApproval{},
		attachments: map[string]attachment{}, turnFiles: map[string][]string{},
	}

	client, output := approvalClient()
	service.pending["deny"] = &pendingApproval{
		UserID: alice, SessionID: session.ID, CallID: "deny", Client: client,
		RequestID: json.RawMessage(`11`), Provider: provider, Tool: "delete",
	}
	rec := httptest.NewRecorder()
	service.handleApproval(rec, httptest.NewRequest(http.MethodPost, "/", bytes.NewBufferString(`{"decision":"deny"}`)), alice, "deny")
	if rec.Code != http.StatusOK || output.Len() == 0 || service.pending["deny"] != nil {
		t.Fatalf("deny: status=%d output=%q pending=%v", rec.Code, output.String(), service.pending["deny"] != nil)
	}

	client, signalOutput := signalApprovalClient()
	service.pending["approve"] = &pendingApproval{
		UserID: alice, SessionID: session.ID, CallID: "approve", Client: client,
		RequestID: json.RawMessage(`12`), Provider: provider, Tool: "write",
	}
	rec = httptest.NewRecorder()
	service.handleApproval(rec, httptest.NewRequest(http.MethodPost, "/", bytes.NewBufferString(`{"decision":"approve"}`)), alice, "approve")
	if rec.Code != http.StatusOK {
		t.Fatalf("approve: %d %s", rec.Code, rec.Body.String())
	}
	select {
	case data := <-signalOutput.writes:
		if len(data) == 0 {
			t.Fatal("approved tool produced an empty response")
		}
	case <-time.After(time.Second):
		t.Fatal("approved tool produced no response")
	}

	service.pending["owned"] = &pendingApproval{UserID: alice}
	rec = httptest.NewRecorder()
	service.handleApproval(rec, httptest.NewRequest(http.MethodPost, "/", bytes.NewBufferString(`{"decision":"deny"}`)), bob, "owned")
	if rec.Code != http.StatusNotFound || service.pending["owned"] == nil {
		t.Fatalf("cross-user approval: status=%d pending=%v", rec.Code, service.pending["owned"] != nil)
	}
	for _, decision := range []string{`{`, `{"decision":"later"}`} {
		rec = httptest.NewRecorder()
		service.handleApproval(rec, httptest.NewRequest(http.MethodPost, "/", bytes.NewBufferString(decision)), alice, "missing")
		if rec.Code != http.StatusBadRequest {
			t.Fatalf("invalid decision %q: %d", decision, rec.Code)
		}
	}

	client, output = approvalClient()
	service.onServerCall(alice, client, json.RawMessage(`20`), "unknown/method", nil)
	service.onServerCall(alice, client, json.RawMessage(`21`), "item/tool/call", json.RawMessage(`{`))
	service.onServerCall(alice, client, json.RawMessage(`22`), "item/tool/call", json.RawMessage(`{"callId":"missing","threadId":"other","namespace":"test","tool":"read","arguments":{}}`))
	service.onServerCall(alice, client, json.RawMessage(`23`), "item/tool/call", json.RawMessage(`{"callId":"unknown","threadId":"thread-endpoint","namespace":"test","tool":"missing","arguments":{}}`))
	if output.Len() == 0 {
		t.Fatal("server call failures produced no protocol responses")
	}
	service.onServerCall(alice, nil, json.RawMessage(`24`), "item/tool/call", nil)
}

var _ io.WriteCloser = (*bufferWriteCloser)(nil)
