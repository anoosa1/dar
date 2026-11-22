package main

import (
	"context"
	"encoding/json"
	"errors"
	"math/rand/v2"
	"net/http"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/coder/websocket"
)

// Discord's gateway: one WebSocket per account carrying events. The session resumes after a
// drop so missed events are replayed; when it cannot, Dar identifies again and READY
// brings the full state.

// Capabilities: lazy user notes and users listed once in READY instead of repeated in place.
const gatewayCapabilities = 1<<0 | 1<<4

type gatewayFrame struct {
	Op int             `json:"op"`
	D  json.RawMessage `json:"d,omitempty"`
	S  *int64          `json:"s,omitempty"`
	T  string          `json:"t,omitempty"`
}

type gateway struct {
	url      string
	token    string
	dispatch func(ctx context.Context, event string, data json.RawMessage)
	state    func(state, detail string)

	sessionID string
	resumeURL string
	seq       atomic.Int64
	writeMu   sync.Mutex
	conn      atomic.Pointer[websocket.Conn]
}

// run keeps the gateway connected until ctx ends or Discord rejects the token.
func (g *gateway) run(ctx context.Context) {
	backoff := time.Second
	for ctx.Err() == nil {
		started := time.Now()
		code := g.session(ctx)
		if ctx.Err() != nil {
			return
		}
		switch code {
		case 4004:
			g.state("loggedOut", "authenticationFailed")
			return
		case 4010, 4011, 4012, 4013, 4014:
			g.state("failed", "gateway")
			return
		case 4007, 4009, 1000, 1001:
			g.sessionID = ""
		}
		g.state("disconnected", "")
		if time.Since(started) > time.Minute {
			backoff = time.Second
		}
		wait := backoff + time.Duration(rand.Int64N(int64(backoff)))
		backoff = min(backoff*2, time.Minute)
		select {
		case <-time.After(wait):
		case <-ctx.Done():
			return
		}
	}
}

func (g *gateway) send(ctx context.Context, op int, data any) error {
	conn := g.conn.Load()
	if conn == nil {
		return fail("notConnected", "Discord is not connected.")
	}
	raw, err := json.Marshal(map[string]any{"op": op, "d": data})
	if err != nil {
		return err
	}
	g.writeMu.Lock()
	defer g.writeMu.Unlock()
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	return conn.Write(ctx, websocket.MessageText, raw)
}

// session runs one connection and returns its close code (0 when it simply dropped).
func (g *gateway) session(ctx context.Context) int {
	url := g.url
	resuming := g.sessionID != "" && g.resumeURL != ""
	if resuming {
		url = strings.TrimRight(g.resumeURL, "/") + "/?v=9&encoding=json"
	}
	conn, _, err := websocket.Dial(ctx, url, &websocket.DialOptions{HTTPHeader: http.Header{"Origin": {"https://discord.com"}, "User-Agent": {discordUserAgent}}})
	if err != nil {
		return 0
	}
	defer conn.CloseNow()
	// READY for an account in many servers is large, and Dar asks for it uncompressed.
	conn.SetReadLimit(256 << 20)
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	g.conn.Store(conn)
	defer g.conn.CompareAndSwap(conn, nil)

	read := func() (gatewayFrame, error) {
		var f gatewayFrame
		_, raw, err := conn.Read(ctx)
		if err != nil {
			return f, err
		}
		if err = json.Unmarshal(raw, &f); err != nil {
			return f, err
		}
		if f.S != nil {
			g.seq.Store(*f.S)
		}
		return f, nil
	}
	hello, err := read()
	if err != nil || hello.Op != 10 {
		return closeCode(err)
	}
	var h struct {
		HeartbeatInterval int `json:"heartbeat_interval"`
	}
	_ = json.Unmarshal(hello.D, &h)
	interval := time.Duration(max(h.HeartbeatInterval, 1000)) * time.Millisecond

	if resuming {
		err = g.send(ctx, 6, map[string]any{"token": g.token, "session_id": g.sessionID, "seq": g.seq.Load()})
	} else {
		g.seq.Store(0)
		err = g.send(ctx, 2, map[string]any{
			"token": g.token, "capabilities": gatewayCapabilities, "properties": discordProperties(), "compress": false,
			"presence":     map[string]any{"status": "unknown", "since": 0, "activities": []any{}, "afk": false},
			"client_state": map[string]any{"guild_versions": map[string]any{}},
		})
	}
	if err != nil {
		return 0
	}

	var acked atomic.Bool
	acked.Store(true)
	beat := func() error {
		var seq any
		if s := g.seq.Load(); s > 0 {
			seq = s
		}
		return g.send(ctx, 1, seq)
	}
	go func() {
		timer := time.NewTimer(time.Duration(rand.Int64N(int64(interval))))
		defer timer.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-timer.C:
			}
			if !acked.Swap(false) {
				// No acknowledgement since the last heartbeat: a dead connection.
				_ = conn.Close(4000, "heartbeat")
				return
			}
			if beat() != nil {
				return
			}
			timer.Reset(interval)
		}
	}()

	for {
		f, err := read()
		if err != nil {
			return closeCode(err)
		}
		switch f.Op {
		case 0:
			switch f.T {
			case "READY":
				var ready struct {
					SessionID        string `json:"session_id"`
					ResumeGatewayURL string `json:"resume_gateway_url"`
				}
				_ = json.Unmarshal(f.D, &ready)
				g.sessionID, g.resumeURL = ready.SessionID, ready.ResumeGatewayURL
			case "RESUMED":
				g.state("connected", "")
			}
			g.dispatch(ctx, f.T, f.D)
		case 1:
			_ = beat()
		case 7:
			_ = conn.Close(4000, "reconnect")
			return 0
		case 9:
			var resumable bool
			_ = json.Unmarshal(f.D, &resumable)
			if !resumable {
				g.sessionID = ""
			}
			select {
			case <-time.After(time.Duration(1000+rand.IntN(4000)) * time.Millisecond):
			case <-ctx.Done():
			}
			_ = conn.Close(4000, "invalid session")
			return 0
		case 11:
			acked.Store(true)
		}
	}
}

func closeCode(err error) int {
	if err == nil {
		return 0
	}
	if code := websocket.CloseStatus(err); code != -1 {
		return int(code)
	}
	var closeErr websocket.CloseError
	if errors.As(err, &closeErr) {
		return int(closeErr.Code)
	}
	return 0
}
