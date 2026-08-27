package codex

import (
	"context"
	"errors"
	"os/exec"
	"runtime"
	"strings"
	"testing"
	"time"
)

func TestProtocolMultiplexesAndIgnoresMalformedEvents(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("test helper requires POSIX shell")
	}
	command := exec.Command("sh", "-c", `while IFS= read -r line; do case "$line" in *\"method\":\"first\"*) echo 'not-json'; echo '{"id":1,"result":{"value":"one"}}' ;; *\"method\":\"second\"*) echo '{"id":2,"result":{"value":"two"}}' ;; esac; done`)
	client, err := startProtocolClient(command, nil, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer client.stop()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	var first, second struct {
		Value string `json:"value"`
	}
	if err := client.call(ctx, "first", map[string]any{}, &first); err != nil {
		t.Fatal(err)
	}
	if err := client.call(ctx, "second", map[string]any{}, &second); err != nil {
		t.Fatal(err)
	}
	if first.Value != "one" || second.Value != "two" {
		t.Fatalf("responses = %#v %#v", first, second)
	}
}

func TestProtocolPendingCleanupAndStderrRedaction(t *testing.T) {
	client := &protocolClient{pending: map[uint64]chan rpcResult{1: make(chan rpcResult, 1)}}
	client.removePending(1)
	if len(client.pending) != 0 {
		t.Fatal("pending RPC was not removed")
	}
	client.stderrLoop(strings.NewReader("Bearer abcdefghijkl\nsk-abcdefghijk\n" + strings.Repeat("x", 1200) + "\n"))
}

func TestProtocolRetainsRPCErrorDetails(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("test helper requires POSIX shell")
	}
	command := exec.Command("sh", "-c", `while IFS= read -r line; do echo '{"id":1,"error":{"code":-32600,"message":"thread is not materialized yet","data":{"kind":"empty"}}}'; done`)
	client, err := startProtocolClient(command, nil, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer client.stop()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	err = client.call(ctx, "thread/read", map[string]any{}, nil)
	var rpcErr *rpcCallError
	if !errors.As(err, &rpcErr) {
		t.Fatalf("error type = %T, want *rpcCallError", err)
	}
	if rpcErr.Code != -32600 || rpcErr.Message != "thread is not materialized yet" || string(rpcErr.Data) != `{"kind":"empty"}` {
		t.Fatalf("RPC error = %#v", rpcErr)
	}
	if !isConfirmedEmptyThreadError(err) {
		t.Fatalf("real pre-first-message error was not classified as empty: %v", err)
	}
}

func TestRedactCredentials(t *testing.T) {
	got := redactCredentials("api_key=sk-secretVALUE123 Bearer token.value-123")
	if got != "api_key=[REDACTED] [REDACTED]" {
		t.Fatalf("redacted = %q", got)
	}
}
