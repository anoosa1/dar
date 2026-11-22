package main

import (
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"encoding/binary"
	"encoding/json"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
)

// The READY fixture with @everyone allowed to view, connect and speak, and Lina in Lounge.
var voiceReady = strings.Replace(strings.Replace(readyFixture, `"permissions":"1024"`, `"permissions":"3146752"`, 1),
	`"name":"Homelab Club","owner_id":"999",`, `"name":"Homelab Club","owner_id":"999","voice_states":[{"user_id":"200","channel_id":"921","session_id":"x","self_mute":true}],`, 1)

func (tp *testPeer) all(name string) []map[string]any {
	tp.mu.Lock()
	defer tp.mu.Unlock()
	var out []map[string]any
	for _, f := range tp.events {
		if f.Event == name {
			raw, _ := json.Marshal(f.Data)
			var data map[string]any
			_ = json.Unmarshal(raw, &data)
			out = append(out, data)
		}
	}
	return out
}

func lounge(event map[string]any) []any {
	servers, _ := event["servers"].(map[string]any)
	channels, _ := servers["900"].(map[string]any)
	list, _ := channels["921"].([]any)
	return list
}

func TestVoiceStatesFollowWhoIsInEachChannel(t *testing.T) {
	a, tp := newTestDiscord(t)
	a.ready(context.Background(), json.RawMessage(voiceReady))
	waitFor(t, "voice states", func() bool { return len(tp.all("voiceStates")) == 1 })
	first := tp.all("voiceStates")[0]
	if first["complete"] != true || len(lounge(first)) != 1 {
		t.Fatalf("%+v", first)
	}
	lina := lounge(first)[0].(map[string]any)
	if lina["id"] != "200" || lina["name"] != "Lina K" || lina["muted"] != true {
		t.Fatalf("%+v", lina)
	}
	a.dispatch(context.Background(), "VOICE_STATE_UPDATE", json.RawMessage(`{"guild_id":"900","channel_id":"921","user_id":"300","session_id":"y","member":{"user":{"id":"300","username":"tarek"},"nick":"T","roles":[]}}`))
	waitFor(t, "a second person", func() bool { all := tp.all("voiceStates"); return len(lounge(all[len(all)-1])) == 2 })
	all := tp.all("voiceStates")
	if names := lounge(all[len(all)-1]); names[1].(map[string]any)["name"] != "T" {
		t.Fatalf("%+v", names)
	}
	a.dispatch(context.Background(), "VOICE_STATE_UPDATE", json.RawMessage(`{"guild_id":"900","channel_id":null,"user_id":"200","session_id":"x"}`))
	waitFor(t, "Lina leaving", func() bool { all := tp.all("voiceStates"); return len(lounge(all[len(all)-1])) == 1 })
}

type fakeVoice struct {
	t       *testing.T
	key     []byte
	version int
	udp     *net.UDPConn
	mu      sync.Mutex
	ops     []int
	ws      *websocket.Conn
	client  *net.UDPAddr
	packets chan []byte
	joins   chan map[string]any
}

// fakeDiscord is a main gateway that answers a voice join with a voice server, and that
// voice server with its UDP socket.
func newFakeDiscord(t *testing.T, version int) (*fakeVoice, string, string) {
	f := &fakeVoice{t: t, key: bytes.Repeat([]byte{7}, 32), version: version, packets: make(chan []byte, 64), joins: make(chan map[string]any, 8)}
	var err error
	f.udp, err = net.ListenUDP("udp", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { f.udp.Close() })
	go func() {
		buf := make([]byte, 1500)
		for {
			n, from, err := f.udp.ReadFromUDP(buf)
			if err != nil {
				return
			}
			if n == 74 && binary.BigEndian.Uint16(buf) == 1 {
				answer := make([]byte, 74)
				binary.BigEndian.PutUint16(answer, 2)
				binary.BigEndian.PutUint16(answer[2:], 70)
				copy(answer[4:8], buf[4:8])
				copy(answer[8:], "203.0.113.9")
				binary.BigEndian.PutUint16(answer[72:], 50000)
				f.mu.Lock()
				f.client = from
				f.mu.Unlock()
				_, _ = f.udp.WriteToUDP(answer, from)
				continue
			}
			if n > rtpHeaderSize {
				f.packets <- append([]byte(nil), buf[:n]...)
			}
		}
	}()
	voice := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{InsecureSkipVerify: true})
		if err != nil {
			return
		}
		defer conn.CloseNow()
		f.mu.Lock()
		f.ws = conn
		f.mu.Unlock()
		ctx := r.Context()
		send := func(op int, d any) { raw, _ := json.Marshal(map[string]any{"op": op, "d": d}); _ = conn.Write(ctx, websocket.MessageText, raw) }
		send(8, map[string]any{"heartbeat_interval": 30000})
		for {
			_, raw, err := conn.Read(ctx)
			if err != nil {
				return
			}
			var m struct {
				Op int             `json:"op"`
				D  json.RawMessage `json:"d"`
			}
			if json.Unmarshal(raw, &m) != nil {
				continue
			}
			f.mu.Lock()
			f.ops = append(f.ops, m.Op)
			f.mu.Unlock()
			switch m.Op {
			case 0:
				var id struct {
					Token   string `json:"token"`
					Session string `json:"session_id"`
					Max     int    `json:"max_dave_protocol_version"`
				}
				_ = json.Unmarshal(m.D, &id)
				if id.Token != "voice-token" || id.Session != "voice-session" || id.Max < 1 {
					t.Errorf("identify %s", m.D)
				}
				port := f.udp.LocalAddr().(*net.UDPAddr).Port
				send(2, map[string]any{"ssrc": 7, "ip": "127.0.0.1", "port": port, "modes": []string{"aead_xchacha20_poly1305_rtpsize", "aead_aes256_gcm_rtpsize"}})
			case 1:
				var sel struct {
					Data struct {
						Address string `json:"address"`
						Port    int    `json:"port"`
						Mode    string `json:"mode"`
					} `json:"data"`
				}
				_ = json.Unmarshal(m.D, &sel)
				if sel.Data.Address != "203.0.113.9" || sel.Data.Port != 50000 || sel.Data.Mode != "aead_aes256_gcm_rtpsize" {
					t.Errorf("select protocol %s", m.D)
				}
				key := make([]int, len(f.key))
				for i, b := range f.key {
					key[i] = int(b)
				}
				send(4, map[string]any{"mode": sel.Data.Mode, "secret_key": key, "dave_protocol_version": f.version})
			}
		}
	}))
	t.Cleanup(voice.Close)
	main := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{InsecureSkipVerify: true})
		if err != nil {
			return
		}
		defer conn.CloseNow()
		ctx := r.Context()
		seq := 0
		send := func(op int, event string, d any) {
			seq++
			raw, _ := json.Marshal(map[string]any{"op": op, "t": event, "s": seq, "d": d})
			_ = conn.Write(ctx, websocket.MessageText, raw)
		}
		send(10, "", map[string]any{"heartbeat_interval": 30000})
		for {
			_, raw, err := conn.Read(ctx)
			if err != nil {
				return
			}
			var m struct {
				Op int            `json:"op"`
				D  map[string]any `json:"d"`
			}
			_ = json.Unmarshal(raw, &m)
			switch m.Op {
			case 2:
				var ready map[string]any
				_ = json.Unmarshal([]byte(voiceReady), &ready)
				send(0, "READY", ready)
			case 4:
				f.joins <- m.D
				channel := m.D["channel_id"]
				send(0, "VOICE_STATE_UPDATE", map[string]any{"guild_id": "900", "channel_id": channel, "user_id": "100", "session_id": "voice-session",
					"self_mute": m.D["self_mute"], "self_deaf": m.D["self_deaf"]})
				if channel != nil {
					send(0, "VOICE_SERVER_UPDATE", map[string]any{"guild_id": "900", "token": "voice-token", "endpoint": strings.TrimPrefix(voice.URL, "http://")})
				}
			}
		}
	}))
	t.Cleanup(main.Close)
	return f, "ws" + strings.TrimPrefix(main.URL, "http"), voice.URL
}

func (f *fakeVoice) hasOp(op int) bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	for _, o := range f.ops {
		if o == op {
			return true
		}
	}
	return false
}

func (f *fakeVoice) gcm() cipher.AEAD {
	block, _ := aes.NewCipher(f.key)
	aead, _ := cipher.NewGCM(block)
	return aead
}

func lastCall(tp *testPeer) map[string]any {
	all := tp.all("voiceCall")
	if len(all) == 0 {
		return map[string]any{}
	}
	return all[len(all)-1]
}

func startVoiceAccount(t *testing.T, version int) (*dcAccount, *testPeer, *fakeVoice) {
	a, tp := newTestDiscord(t)
	f, gatewayURL, _ := newFakeDiscord(t, version)
	a.bridge.discordURLs = discordURLs{api: "http://127.0.0.1:1", gateway: gatewayURL, voiceScheme: "ws"}
	a.start("token")
	t.Cleanup(a.stop)
	waitFor(t, "the account to connect", func() bool { _, ok := tp.event("connected"); return ok })
	if err := a.voiceJoin(context.Background(), "921", false, false); err != nil {
		t.Fatal(err)
	}
	join := <-f.joins
	if join["guild_id"] != "900" || join["channel_id"] != "921" || join["self_mute"] != false {
		t.Fatalf("join %+v", join)
	}
	waitFor(t, "the call to connect", func() bool { return lastCall(tp)["state"] == "connected" })
	return a, tp, f
}

func TestVoiceCallCarriesAudioBothWays(t *testing.T) {
	a, tp, f := startVoiceAccount(t, 0)
	if call := lastCall(tp); call["channelId"] != "921" || call["e2ee"] != false || call["canSpeak"] != true {
		t.Fatalf("%+v", call)
	}
	// The microphone: one Opus frame goes out as RTP, encrypted for transport.
	frame := []byte{0xfc, 1, 2, 3, 4}
	a.sendAudio([][]byte{frame})
	var packet []byte
	select {
	case packet = <-f.packets:
	case <-time.After(5 * time.Second):
		t.Fatal("no RTP packet")
	}
	for len(packet) == 8 { // keepalives
		packet = <-f.packets
	}
	if packet[1] != rtpPayloadOpus || binary.BigEndian.Uint32(packet[8:]) != 7 {
		t.Fatalf("header % x", packet[:12])
	}
	aead := f.gcm()
	nonce := make([]byte, 12)
	copy(nonce, packet[len(packet)-4:])
	plain, err := aead.Open(nil, nonce, packet[12:len(packet)-4], packet[:12])
	if err != nil || !bytes.Equal(plain, frame) {
		t.Fatalf("payload % x %v", plain, err)
	}
	waitFor(t, "speaking", func() bool { return f.hasOp(5) })

	// Someone else: Discord says which SSRC is theirs, then their audio arrives with an extension.
	f.mu.Lock()
	ws, client := f.ws, f.client
	f.mu.Unlock()
	raw, _ := json.Marshal(map[string]any{"op": 5, "d": map[string]any{"user_id": "200", "ssrc": 99, "speaking": 1}})
	_ = ws.Write(context.Background(), websocket.MessageText, raw)
	time.Sleep(50 * time.Millisecond)
	header := []byte{0x90, rtpPayloadOpus, 0, 1, 0, 0, 3, 0xc0, 0, 0, 0, 99, 0xbe, 0xde, 0, 1}
	opus := []byte{0x78, 9, 9, 9}
	binary.BigEndian.PutUint32(nonce, 41)
	sealed := aead.Seal(append([]byte(nil), header...), nonce, append([]byte{1, 2, 3, 4}, opus...), header)
	_, _ = f.udp.WriteToUDP(append(sealed, nonce[:4]...), client)
	waitFor(t, "their audio", func() bool { return len(tp.all("voiceAudio")) == 1 })
	got := tp.all("voiceAudio")[0]
	if got["user"] != "200" || got["data"] != "eAkJCQ==" {
		t.Fatalf("%+v", got)
	}

	// Muting stops the microphone; leaving tells Discord and ends the call.
	if err := a.call.set(context.Background(), true, false); err != nil {
		t.Fatal(err)
	}
	if join := <-f.joins; join["self_mute"] != true {
		t.Fatalf("mute %+v", join)
	}
	waitFor(t, "muted", func() bool { return lastCall(tp)["selfMute"] == true })
	for len(f.packets) > 0 {
		<-f.packets
	}
	a.sendAudio([][]byte{frame})
	select {
	case p := <-f.packets:
		if len(p) != 8 {
			t.Fatal("a muted microphone was sent")
		}
	case <-time.After(200 * time.Millisecond):
	}
	a.call.end("left", true)
	if join := <-f.joins; join["channel_id"] != nil {
		t.Fatalf("leave %+v", join)
	}
	if call := lastCall(tp); call["state"] != "ended" || call["reason"] != "left" || a.call != nil {
		t.Fatalf("%+v", call)
	}
}

func TestEncryptedCallHoldsAudioUntilKeysAreAgreed(t *testing.T) {
	a, tp, f := startVoiceAccount(t, 1)
	if call := lastCall(tp); call["protocol"] != float64(1) || call["e2ee"] != false {
		t.Fatalf("%+v", call)
	}
	a.sendAudio([][]byte{{0xfc, 1, 2}})
	select {
	case p := <-f.packets:
		if len(p) != 8 {
			t.Fatal("audio went out before DAVE was ready")
		}
	case <-time.After(300 * time.Millisecond):
	}
	if _, err := a.callCode(context.Background()); err == nil {
		t.Fatal("a call code before encryption")
	}
}

func TestVoiceJoinNeedsPermission(t *testing.T) {
	a, _ := newTestDiscord(t)
	a.ready(context.Background(), json.RawMessage(readyFixture))
	err := a.voiceJoin(context.Background(), "921", false, false)
	if coded, ok := err.(*codedError); !ok || coded.code != "forbidden" {
		t.Fatalf("%v", err)
	}
	if err = a.voiceJoin(context.Background(), "911", false, false); err == nil {
		t.Fatal("joined a text channel")
	}
}
