// Command dar-chat-bridge connects Dar's Chat app to WhatsApp through whatsmeow.
// The Node runtime starts it and talks to it over stdin/stdout; see rpc.go.
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"strings"
	"sync"
	"syscall"
	"time"

	waLog "go.mau.fi/whatsmeow/util/log"
)

const protocolVersion = 1

// stderrLog writes whatsmeow's logs to stderr; stdout carries the protocol only.
type stderrLog struct {
	module string
	level  int
	mu     *sync.Mutex
}

var levels = map[string]int{"debug": 0, "info": 1, "warn": 2, "error": 3}

func (l stderrLog) write(level int, name, msg string, args []any) {
	if level < l.level {
		return
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	fmt.Fprintf(os.Stderr, "%s [%s %s] %s\n", time.Now().UTC().Format("15:04:05.000"), l.module, name, fmt.Sprintf(msg, args...))
}

func (l stderrLog) Debugf(msg string, args ...any) { l.write(0, "DEBUG", msg, args) }
func (l stderrLog) Infof(msg string, args ...any)  { l.write(1, "INFO", msg, args) }
func (l stderrLog) Warnf(msg string, args ...any)  { l.write(2, "WARN", msg, args) }
func (l stderrLog) Errorf(msg string, args ...any) { l.write(3, "ERROR", msg, args) }
func (l stderrLog) Sub(module string) waLog.Logger {
	return stderrLog{module: l.module + "/" + module, level: l.level, mu: l.mu}
}

// runtimeBackend keeps keys through the runtime, which encrypts them into SpacetimeDB.
type runtimeBackend struct{ peer *peer }

func (r runtimeBackend) load(ctx context.Context, account, cursor string) ([]entry, string, error) {
	var result struct {
		Entries []entry `json:"entries"`
		Next    string  `json:"next"`
	}
	err := r.peer.call(ctx, "kv.load", map[string]any{"account": account, "cursor": cursor}, &result)
	return result.Entries, result.Next, err
}

func (r runtimeBackend) get(ctx context.Context, account string, keys []string) (map[string][]byte, error) {
	var result struct {
		Values map[string][]byte `json:"values"`
	}
	err := r.peer.call(ctx, "kv.get", map[string]any{"account": account, "keys": keys}, &result)
	return result.Values, err
}

func (r runtimeBackend) commit(ctx context.Context, account string, puts []entry, deletes []string) error {
	if puts == nil {
		puts = []entry{}
	}
	if deletes == nil {
		deletes = []string{}
	}
	return r.peer.call(ctx, "kv.commit", map[string]any{"account": account, "puts": puts, "deletes": deletes}, nil)
}

func main() {
	level, ok := levels[strings.ToLower(os.Getenv("DAR_CHAT_BRIDGE_LOG"))]
	if !ok {
		level = levels["warn"]
	}
	log := stderrLog{module: "bridge", level: level, mu: &sync.Mutex{}}
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer stop()
	p := newPeer(ctx, os.Stdout)
	b := newBridge(p, runtimeBackend{p}, log)
	p.handle("hello", func(context.Context, json.RawMessage) (any, error) {
		return map[string]any{"protocol": protocolVersion, "platforms": []string{"whatsapp"}}, nil
	})
	p.handle("link", b.link)
	p.handle("connect", b.connect)
	p.handle("disconnect", b.disconnect)
	p.handle("logout", b.logout)
	p.handle("send", b.send)
	p.handle("markRead", b.markRead)
	done := make(chan error, 1)
	go func() { done <- p.serve(os.Stdin) }()
	select {
	case <-ctx.Done():
	case <-done:
	}
	b.mu.Lock()
	ids := make([]string, 0, len(b.accounts))
	for id := range b.accounts {
		ids = append(ids, id)
	}
	b.mu.Unlock()
	for _, id := range ids {
		b.stop(id)
	}
}
