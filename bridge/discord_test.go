package main

import (
	"bufio"
	"context"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
)

// testPeer answers every runtime request with {} and records them.
type testPeer struct {
	*peer
	mu       sync.Mutex
	requests []frame
	events   []frame
}

func newTestPeer(t *testing.T) *testPeer {
	r, w := io.Pipe()
	tp := &testPeer{peer: newPeer(context.Background(), w)}
	go func() {
		scanner := bufio.NewScanner(r)
		scanner.Buffer(make([]byte, 1<<16), 64<<20)
		for scanner.Scan() {
			var f frame
			if json.Unmarshal(scanner.Bytes(), &f) != nil {
				continue
			}
			tp.mu.Lock()
			if f.Event != "" {
				tp.events = append(tp.events, f)
			} else {
				tp.requests = append(tp.requests, f)
			}
			tp.mu.Unlock()
			if f.Method != "" {
				if ch, ok := tp.pending.Load(f.ID); ok {
					ch.(chan frame) <- frame{ID: f.ID, Result: json.RawMessage(`{}`)}
				}
			}
		}
	}()
	t.Cleanup(func() { w.Close() })
	return tp
}

func (tp *testPeer) ingests(kind string) []map[string]any {
	tp.mu.Lock()
	defer tp.mu.Unlock()
	var out []map[string]any
	for _, f := range tp.requests {
		var params map[string]any
		if f.Method == "ingest" && json.Unmarshal(f.Params, &params) == nil && params["kind"] == kind {
			out = append(out, params)
		}
	}
	return out
}

func (tp *testPeer) event(name string) (frame, bool) {
	tp.mu.Lock()
	defer tp.mu.Unlock()
	for _, f := range tp.events {
		if f.Event == name {
			return f, true
		}
	}
	return frame{}, false
}

func waitFor(t *testing.T, what string, ok func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !ok() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

const testAccount = "00000000-0000-4000-8000-000000000001"

func TestRemoteAuthHandsOverTheToken(t *testing.T) {
	var clientKey *rsa.PublicKey
	encrypt := func(data []byte) string {
		out, err := rsa.EncryptOAEP(sha256.New(), rand.Reader, clientKey, data, nil)
		if err != nil {
			t.Fatal(err)
		}
		return base64.StdEncoding.EncodeToString(out)
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/ra", func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Origin") != "https://discord.com" {
			http.Error(w, "origin", http.StatusForbidden)
			return
		}
		conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{InsecureSkipVerify: true})
		if err != nil {
			return
		}
		defer conn.CloseNow()
		ctx := r.Context()
		send := func(v any) { raw, _ := json.Marshal(v); _ = conn.Write(ctx, websocket.MessageText, raw) }
		recv := func() map[string]string {
			for {
				_, raw, err := conn.Read(ctx)
				if err != nil {
					return nil
				}
				var m map[string]string
				_ = json.Unmarshal(raw, &m)
				if m["op"] != "heartbeat" {
					return m
				}
				send(map[string]any{"op": "heartbeat_ack"})
			}
		}
		send(map[string]any{"op": "hello", "heartbeat_interval": 41250, "timeout_ms": 120000})
		init := recv()
		der, _ := base64.StdEncoding.DecodeString(init["encoded_public_key"])
		parsed, err := x509.ParsePKIXPublicKey(der)
		if err != nil {
			t.Error("public key", err)
			return
		}
		clientKey = parsed.(*rsa.PublicKey)
		nonce := []byte("a nonce of sorts")
		send(map[string]any{"op": "nonce_proof", "encrypted_nonce": encrypt(nonce)})
		if proof := recv(); proof["nonce"] != base64.RawURLEncoding.EncodeToString(nonce) {
			t.Error("wrong proof", proof)
			return
		}
		digest := sha256.Sum256(der)
		send(map[string]any{"op": "pending_remote_init", "fingerprint": base64.RawURLEncoding.EncodeToString(digest[:])})
		send(map[string]any{"op": "pending_ticket", "encrypted_user_payload": encrypt([]byte("1234:0:0:someone"))})
		send(map[string]any{"op": "pending_login", "ticket": "the-ticket"})
		_, _, _ = conn.Read(ctx)
	})
	mux.HandleFunc("/api/users/@me/remote-auth/login", func(w http.ResponseWriter, r *http.Request) {
		var body map[string]string
		_ = json.NewDecoder(r.Body).Decode(&body)
		if body["ticket"] != "the-ticket" || r.Header.Get("Authorization") != "" {
			http.Error(w, "{}", http.StatusBadRequest)
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]string{"encrypted_token": encrypt([]byte("secret-token"))})
	})
	server := httptest.NewServer(mux)
	defer server.Close()

	var code string
	var scanned remoteAuthUser
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	token, _, err := remoteAuth(ctx, "ws"+strings.TrimPrefix(server.URL, "http")+"/ra", newDiscordAPI(server.URL+"/api", ""), remoteAuthEvents{
		code:    func(url string, _ time.Duration) { code = url },
		scanned: func(u remoteAuthUser) { scanned = u },
	})
	if err != nil {
		t.Fatal(err)
	}
	if token != "secret-token" || !strings.HasPrefix(code, "https://discord.com/ra/") || scanned.ID != "1234" || scanned.Username != "someone" {
		t.Fatalf("token %q code %q user %+v", token, code, scanned)
	}
}

func TestRemoteAuthCancelled(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{InsecureSkipVerify: true})
		if err != nil {
			return
		}
		raw, _ := json.Marshal(map[string]any{"op": "cancel"})
		_ = conn.Write(r.Context(), websocket.MessageText, raw)
		_ = conn.Close(websocket.StatusNormalClosure, "")
	}))
	defer server.Close()
	_, _, err := remoteAuth(context.Background(), "ws"+strings.TrimPrefix(server.URL, "http"), newDiscordAPI(server.URL, ""), remoteAuthEvents{code: func(string, time.Duration) {}, scanned: func(remoteAuthUser) {}})
	if coded, ok := err.(*codedError); !ok || coded.code != "cancelled" {
		t.Fatalf("expected cancelled, got %v", err)
	}
}

const readyFixture = `{
 "session_id":"s1","resume_gateway_url":"RESUME",
 "user":{"id":"100","username":"anas","global_name":"Anas"},
 "users":[{"id":"200","username":"lina","global_name":"Lina K"},{"id":"300","username":"tarek","global_name":null}],
 "guilds":[{"id":"900","name":"Homelab Club","owner_id":"999",
   "roles":[{"id":"900","name":"@everyone","permissions":"1024"},{"id":"901","name":"mods","permissions":"0"}],
   "channels":[
     {"id":"910","type":4,"name":"Text channels","position":0},
     {"id":"911","type":0,"name":"general","position":1,"parent_id":"910","last_message_id":"1300000000000000000","topic":"Say hello"},
     {"id":"912","type":0,"name":"mods-only","position":2,"parent_id":"910","permission_overwrites":[{"id":"900","type":0,"allow":"0","deny":"1024"},{"id":"901","type":0,"allow":"1024","deny":"0"}]},
     {"id":"913","type":0,"name":"secret","position":3,"parent_id":"910","permission_overwrites":[{"id":"900","type":0,"allow":"0","deny":"1024"}]},
     {"id":"920","type":4,"name":"Voice channels","position":1},
     {"id":"921","type":2,"name":"Lounge","position":0,"parent_id":"920"},
     {"id":"930","type":0,"name":"rules","position":0}
   ]}],
 "merged_members":[[{"user_id":"100","roles":["901"]}]],
 "private_channels":[{"id":"700","type":1,"recipient_ids":["200"],"last_message_id":"1300000000000000001"},{"id":"701","type":3,"name":"","recipient_ids":["200","300"]}],
 "read_state":[{"id":"911","last_message_id":"1200000000000000000","mention_count":2},{"id":"700","last_message_id":"1300000000000000001","mention_count":0}],
 "user_guild_settings":[{"guild_id":null,"muted":false,"channel_overrides":[{"channel_id":"701","muted":true,"mute_config":null}]}],
 "relationships":[{"id":"200","type":1,"user_id":"200"},{"id":"300","type":2,"user_id":"300"}]
}`

func newTestDiscord(t *testing.T) (*dcAccount, *testPeer) {
	tp := newTestPeer(t)
	b := newBridge(tp.peer, newMemory(), stderrLog{module: "test", level: 3, mu: &sync.Mutex{}})
	a := b.newDiscordAccount(testAccount)
	b.discord[testAccount] = a
	return a, tp
}

func TestReadyStoresVisibleChannelsAndDMs(t *testing.T) {
	a, tp := newTestDiscord(t)
	a.ready(context.Background(), json.RawMessage(readyFixture))
	waitFor(t, "the connected event", func() bool { _, ok := tp.event("connected"); return ok })
	servers := tp.ingests("servers")
	if len(servers) != 1 || servers[0]["complete"] != true {
		t.Fatalf("servers %+v", servers)
	}
	channels := tp.ingests("channels")
	if len(channels) != 1 {
		t.Fatalf("channels %+v", channels)
	}
	var chats []dcChatOut
	raw, _ := json.Marshal(channels[0]["chats"])
	_ = json.Unmarshal(raw, &chats)
	names := map[string]dcChatOut{}
	for _, c := range chats {
		names[c.Name] = c
	}
	if _, ok := names["secret"]; ok {
		t.Fatal("a hidden channel was stored")
	}
	if _, ok := names["mods-only"]; !ok {
		t.Fatal("a channel the account's role can see was left out")
	}
	general, lounge, rules := names["general"], names["Lounge"], names["rules"]
	if general.Section != "Text channels" || general.Topic != "Say hello" || *general.Unread != 2 || *general.Mentions != 2 || general.Kind != "channel" {
		t.Fatalf("general %+v", general)
	}
	if lounge.Kind != "voice" || !(rules.Order < general.Order && general.Order < lounge.Order) {
		t.Fatalf("order: rules %s general %s lounge %s", rules.Order, general.Order, lounge.Order)
	}
	dms := tp.ingests("chats")
	if len(dms) != 1 {
		t.Fatalf("dms %+v", dms)
	}
	raw, _ = json.Marshal(dms[0]["chats"])
	chats = nil
	_ = json.Unmarshal(raw, &chats)
	for _, c := range chats {
		switch c.ID {
		case "700":
			if c.Kind != "direct" || c.Name != "Lina K" || *c.Unread != 0 {
				t.Fatalf("dm %+v", c)
			}
		case "701":
			if c.Kind != "group" || c.Name != "Lina K, tarek" || *c.MutedUntil != -1 {
				t.Fatalf("group %+v", c)
			}
		}
	}
	if !a.friends["200"] || a.friends["300"] {
		t.Fatalf("friends %+v", a.friends)
	}
}

func TestMessagesRenderMentionsRepliesAndAttachments(t *testing.T) {
	a, tp := newTestDiscord(t)
	a.ready(context.Background(), json.RawMessage(readyFixture))
	create := `{"id":"1300000000000000005","channel_id":"911","guild_id":"900","author":{"id":"200","username":"lina","global_name":"Lina K"},
	  "member":{"nick":"lina.k","roles":[]},"content":"hi <@100> see <#930> and <:party:55>","timestamp":"2026-11-18T10:00:00.000000+00:00","type":19,
	  "mentions":[{"id":"100","username":"anas","global_name":"Anas"}],
	  "referenced_message":{"id":"1300000000000000004","channel_id":"911","author":{"id":"100","username":"anas"},"content":"question?","timestamp":"2026-11-18T09:00:00+00:00","type":0},
	  "attachments":[{"id":"1","filename":"photo.png","content_type":"image/png","size":2048,"url":"https://cdn.discordapp.com/attachments/1/photo.png?ex=1","width":10,"height":20},{"id":"2","filename":"notes.pdf","size":10,"url":"https://cdn.discordapp.com/attachments/2/notes.pdf"}],
	  "reactions":[{"emoji":{"id":null,"name":"👍"},"count":3,"me":true}]}`
	a.dispatch(context.Background(), "MESSAGE_CREATE", json.RawMessage(create))
	waitFor(t, "messages", func() bool { return len(tp.ingests("messages")) == 1 })
	var msgs []dcMessageOut
	raw, _ := json.Marshal(tp.ingests("messages")[0]["messages"])
	_ = json.Unmarshal(raw, &msgs)
	if len(msgs) != 1 {
		t.Fatalf("%+v", msgs)
	}
	m := msgs[0]
	if m.Text != "hi @Anas see #rules and :party:\nAlso attached: notes.pdf" || m.SenderName != "lina.k" || !m.Mention || m.Kind != "image" ||
		m.QuotedSender != "You" || m.QuotedText != "question?" || m.Media.Mime != "image/png" || m.At != time.Date(2026, 11, 18, 10, 0, 0, 0, time.UTC).UnixMilli() ||
		len(m.Reactions) != 1 || !m.Reactions[0].Me {
		t.Fatalf("%+v", m)
	}
	stored, ok, err := getJSON[dcStoredMedia](context.Background(), a.kv, mediaName("911", m.ID))
	if err != nil || !ok || !strings.Contains(stored.URL, "photo.png") {
		t.Fatalf("media url %+v %v %v", stored, ok, err)
	}
	// A channel the account cannot see is ignored.
	a.dispatch(context.Background(), "MESSAGE_CREATE", json.RawMessage(strings.Replace(create, `"channel_id":"911"`, `"channel_id":"913"`, 1)))
	time.Sleep(50 * time.Millisecond)
	if len(tp.ingests("messages")) != 1 {
		t.Fatal("a hidden channel's message was stored")
	}
}

func TestGatewayIdentifiesThenResumes(t *testing.T) {
	var mu sync.Mutex
	var ops []int
	sessions := 0
	var server *httptest.Server
	server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{InsecureSkipVerify: true})
		if err != nil {
			return
		}
		defer conn.CloseNow()
		ctx := r.Context()
		send := func(v any) { raw, _ := json.Marshal(v); _ = conn.Write(ctx, websocket.MessageText, raw) }
		send(map[string]any{"op": 10, "d": map[string]any{"heartbeat_interval": 30000}})
		_, raw, err := conn.Read(ctx)
		if err != nil {
			return
		}
		var f struct {
			Op int `json:"op"`
			D  struct {
				Token string `json:"token"`
				Seq   int64  `json:"seq"`
			} `json:"d"`
		}
		_ = json.Unmarshal(raw, &f)
		mu.Lock()
		ops = append(ops, f.Op)
		sessions++
		first := sessions == 1
		mu.Unlock()
		if f.D.Token != "tok" {
			_ = conn.Close(4004, "auth")
			return
		}
		if first {
			send(map[string]any{"op": 0, "s": 1, "t": "READY", "d": map[string]any{"session_id": "abc", "resume_gateway_url": "ws" + strings.TrimPrefix(server.URL, "http")}})
			send(map[string]any{"op": 0, "s": 2, "t": "MESSAGE_ACK", "d": map[string]any{}})
			_ = conn.Close(4000, "drop")
			return
		}
		if f.D.Seq != 2 {
			t.Errorf("resumed at %d", f.D.Seq)
		}
		send(map[string]any{"op": 0, "s": 3, "t": "RESUMED", "d": map[string]any{}})
		_, _, _ = conn.Read(ctx)
	}))
	defer server.Close()
	var states []string
	var events []string
	g := &gateway{url: "ws" + strings.TrimPrefix(server.URL, "http"), token: "tok",
		dispatch: func(_ context.Context, event string, _ json.RawMessage) {
			mu.Lock()
			events = append(events, event)
			mu.Unlock()
		},
		state: func(state, _ string) { mu.Lock(); states = append(states, state); mu.Unlock() }}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() { g.run(ctx); close(done) }()
	waitFor(t, "resume", func() bool {
		mu.Lock()
		defer mu.Unlock()
		return len(states) >= 2 && states[len(states)-1] == "connected"
	})
	cancel()
	<-done
	mu.Lock()
	defer mu.Unlock()
	if len(ops) != 2 || ops[0] != 2 || ops[1] != 6 || events[0] != "READY" || states[0] != "disconnected" {
		t.Fatalf("ops %v events %v states %v", ops, events, states)
	}

	// A rejected token stops the gateway with loggedOut.
	states = nil
	mu.Unlock()
	bad := &gateway{url: g.url, token: "wrong", dispatch: func(context.Context, string, json.RawMessage) {}, state: func(state, _ string) { mu.Lock(); states = append(states, state); mu.Unlock() }}
	bad.run(context.Background())
	mu.Lock()
	if len(states) != 1 || states[0] != "loggedOut" {
		t.Fatalf("states %v", states)
	}
}

func TestPermissionsFollowDiscordOrder(t *testing.T) {
	g := &guildState{id: "1", ownerID: "9", roles: map[string]dcRole{"1": {ID: "1", Permissions: permView}, "2": {ID: "2"}}, myRoles: []string{"2"}}
	member := &dcChannel{Overwrites: []dcOverwrite{{ID: "1", Type: 0, Deny: permView}, {ID: "2", Type: 0, Allow: permView}, {ID: "5", Type: 1, Deny: permView}}}
	if g.permissions(member, "5")&permView != 0 {
		t.Fatal("a member overwrite should deny last")
	}
	if g.permissions(member, "6")&permView == 0 {
		t.Fatal("a role overwrite should allow over @everyone")
	}
	if g.permissions(member, "9") != -1 {
		t.Fatal("the owner sees everything")
	}
}
