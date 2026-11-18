package main

import (
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"os"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/coder/websocket"
	"github.com/disgoorg/godave"
	davesession "github.com/thomas-vilte/dave-go/session"
	"golang.org/x/crypto/chacha20poly1305"
)

// Discord voice in server voice channels. The main gateway asks to join (op 4) and answers
// with the voice session and server; the bridge then opens the voice gateway, finds its
// address over UDP and sends and receives Opus in RTP, transport-encrypted and, with
// DAVE, end-to-end encrypted by dave-go. Opus is encoded and decoded in the browser.

const (
	permConnect      = 1 << 20
	permSpeak        = 1 << 21
	permMoveMembers  = 1 << 24
	rtpHeaderSize    = 12
	rtpPayloadOpus   = 0x78
	opusFrameSamples = 960 // 20 ms at 48 kHz
	voiceAttempts    = 5
)

var opusSilence = []byte{0xF8, 0xFF, 0xFE}

// ---- voice states: who is in which voice channel ----

type dcVoiceState struct {
	UserID     string    `json:"user_id"`
	GuildID    string    `json:"guild_id"`
	ChannelID  *string   `json:"channel_id"`
	SessionID  string    `json:"session_id"`
	SelfMute   bool      `json:"self_mute"`
	SelfDeaf   bool      `json:"self_deaf"`
	Mute       bool      `json:"mute"`
	Deaf       bool      `json:"deaf"`
	SelfVideo  bool      `json:"self_video"`
	SelfStream bool      `json:"self_stream"`
	Suppress   bool      `json:"suppress"`
	Member     *dcMember `json:"member"`
}

type dcVoiceMemberOut struct {
	ID        string `json:"id"`
	Name      string `json:"name"`
	Muted     bool   `json:"muted,omitempty"`
	Deafened  bool   `json:"deafened,omitempty"`
	Video     bool   `json:"video,omitempty"`
	Streaming bool   `json:"streaming,omitempty"`
}

// setVoiceState records one state; a.mu is held.
func (a *dcAccount) setVoiceState(g *guildState, vs dcVoiceState) {
	if vs.Member != nil {
		if vs.Member.User != nil {
			a.users[vs.Member.User.ID] = *vs.Member.User
		}
		if vs.Member.Nick != nil && *vs.Member.Nick != "" {
			g.nicks[vs.UserID] = *vs.Member.Nick
		} else {
			delete(g.nicks, vs.UserID)
		}
	}
	if vs.ChannelID == nil || *vs.ChannelID == "" {
		delete(g.voice, vs.UserID)
		return
	}
	g.voice[vs.UserID] = vs
}

// voiceOut lists who is in each voice channel of a server that Dar shows; a.mu is held.
func (a *dcAccount) voiceOut(g *guildState) map[string][]dcVoiceMemberOut {
	out := map[string][]dcVoiceMemberOut{}
	for user, vs := range g.voice {
		c := g.channels[*vs.ChannelID]
		if c == nil || c.Type != channelVoice || g.permissions(c, a.me.ID)&permView == 0 {
			continue
		}
		name := g.nicks[user]
		if name == "" {
			if u, ok := a.users[user]; ok {
				name = u.name()
			} else {
				name = "Someone"
			}
		}
		out[c.ID] = append(out[c.ID], dcVoiceMemberOut{ID: user, Name: name, Muted: vs.SelfMute || vs.Mute || vs.Suppress,
			Deafened: vs.SelfDeaf || vs.Deaf, Video: vs.SelfVideo, Streaming: vs.SelfStream})
	}
	for _, list := range out {
		sort.Slice(list, func(i, j int) bool { return strings.ToLower(list[i].Name) < strings.ToLower(list[j].Name) })
	}
	return out
}

// emitVoice reports the voice channels of the given servers; complete replaces every server.
func (a *dcAccount) emitVoice(complete bool, ids ...string) {
	a.mu.Lock()
	servers := map[string]any{}
	if complete {
		for id, g := range a.guilds {
			servers[id] = a.voiceOut(g)
		}
	} else {
		for _, id := range ids {
			if g := a.guilds[id]; g != nil {
				servers[id] = a.voiceOut(g)
			} else {
				servers[id] = map[string]any{}
			}
		}
	}
	a.mu.Unlock()
	a.emit("voiceStates", map[string]any{"servers": servers, "complete": complete})
}

func (a *dcAccount) voiceStateUpdate(raw json.RawMessage) {
	var vs dcVoiceState
	if json.Unmarshal(raw, &vs) != nil || vs.GuildID == "" || vs.UserID == "" {
		return // calls in direct messages are not supported
	}
	a.mu.Lock()
	g := a.guilds[vs.GuildID]
	if g != nil {
		a.setVoiceState(g, vs)
	}
	me := vs.UserID == a.me.ID
	call := a.call
	a.mu.Unlock()
	if me && call != nil && call.guildID == vs.GuildID {
		call.onState(vs)
	}
	if g != nil {
		a.emitVoice(false, vs.GuildID)
	}
}

func (a *dcAccount) voiceServerUpdate(raw json.RawMessage) {
	var s struct {
		GuildID  string  `json:"guild_id"`
		Token    string  `json:"token"`
		Endpoint *string `json:"endpoint"`
	}
	if json.Unmarshal(raw, &s) != nil {
		return
	}
	a.mu.Lock()
	call := a.call
	a.mu.Unlock()
	if call != nil && call.guildID == s.GuildID {
		endpoint := ""
		if s.Endpoint != nil {
			endpoint = *s.Endpoint
		}
		call.onServer(s.Token, endpoint)
	}
}

// ---- the call ----

type dcCall struct {
	a       *dcAccount
	guildID string
	ctx     context.Context
	cancel  context.CancelFunc

	mu         sync.Mutex
	channelID  string
	selfMute   bool
	selfDeaf   bool
	canSpeak   bool
	sessionID  string
	token      string
	endpoint   string
	state      string // connecting, connected, reconnecting
	conn       *voiceConn
	connCancel context.CancelFunc
	attempts   int
	ended      bool
}

func (c *dcCall) snapshot() map[string]any {
	c.mu.Lock()
	defer c.mu.Unlock()
	e2ee, version := false, 0
	if c.conn != nil {
		e2ee, version = c.conn.e2ee.Load(), int(c.conn.version.Load())
	}
	return map[string]any{"state": c.state, "serverId": c.guildID, "channelId": c.channelID, "selfMute": c.selfMute,
		"selfDeaf": c.selfDeaf, "canSpeak": c.canSpeak, "e2ee": e2ee, "protocol": version}
}

func (c *dcCall) report() {
	if c.ctx.Err() == nil {
		c.a.emit("voiceCall", c.snapshot())
	}
}

func (c *dcCall) voiceUpdate(ctx context.Context, channel *string) error {
	c.mu.Lock()
	mute, deaf := c.selfMute, c.selfDeaf
	c.mu.Unlock()
	gw := c.a.gw.Load()
	if gw == nil {
		return fail("notConnected", "Discord is not connected.")
	}
	return gw.send(ctx, 4, map[string]any{"guild_id": c.guildID, "channel_id": channel, "self_mute": mute, "self_deaf": deaf, "self_video": false})
}

// voiceJoin joins a voice channel, leaving any call this account is in first.
func (a *dcAccount) voiceJoin(ctx context.Context, channel string, mute, deaf bool) error {
	a.mu.Lock()
	var g *guildState
	var c *dcChannel
	for _, guild := range a.guilds {
		if ch := guild.channels[channel]; ch != nil {
			g, c = guild, ch
		}
	}
	if g == nil || c.Type != channelVoice {
		a.mu.Unlock()
		return fail("notFound", "This voice channel no longer exists.")
	}
	perms := g.permissions(c, a.me.ID)
	if perms&permView == 0 || perms&permConnect == 0 {
		a.mu.Unlock()
		return fail("forbidden", "You do not have permission to join this voice channel.")
	}
	if c.UserLimit > 0 && perms&permMoveMembers == 0 {
		count := 0
		for user, vs := range g.voice {
			if *vs.ChannelID == channel && user != a.me.ID {
				count++
			}
		}
		if count >= c.UserLimit {
			a.mu.Unlock()
			return fail("conflict", "This voice channel is full.")
		}
	}
	old := a.call
	a.mu.Unlock()
	if old != nil {
		old.end("moved", old.guildID != g.id)
	}
	callCtx, cancel := context.WithCancel(context.Background())
	call := &dcCall{a: a, guildID: g.id, ctx: callCtx, cancel: cancel, channelID: channel, selfMute: mute || perms&permSpeak == 0,
		selfDeaf: deaf, canSpeak: perms&permSpeak != 0, state: "connecting"}
	a.mu.Lock()
	a.call = call
	a.mu.Unlock()
	if err := call.voiceUpdate(ctx, &channel); err != nil {
		call.end("connectFailed", false)
		return err
	}
	call.report()
	go func() {
		select {
		case <-time.After(20 * time.Second):
			if call.snapshot()["state"] == "connecting" {
				call.end("timeout", true)
			}
		case <-callCtx.Done():
		}
	}()
	return nil
}

// end leaves the call (telling Discord when leave is set) and reports why it ended.
func (c *dcCall) end(reason string, leave bool) {
	c.mu.Lock()
	if c.ended {
		c.mu.Unlock()
		return
	}
	c.ended = true
	conn, cancelConn := c.conn, c.connCancel
	c.conn = nil
	c.mu.Unlock()
	if cancelConn != nil {
		cancelConn()
	}
	if conn != nil {
		conn.close()
	}
	if leave {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		_ = c.voiceUpdate(ctx, nil)
		cancel()
	}
	c.a.mu.Lock()
	if c.a.call == c {
		c.a.call = nil
	}
	c.a.mu.Unlock()
	c.cancel()
	c.a.emit("voiceCall", map[string]any{"state": "ended", "serverId": c.guildID, "channelId": c.channelID, "reason": reason})
}

func (c *dcCall) set(ctx context.Context, mute, deaf bool) error {
	c.mu.Lock()
	c.selfMute, c.selfDeaf = mute || !c.canSpeak, deaf
	channel := c.channelID
	c.mu.Unlock()
	c.report()
	return c.voiceUpdate(ctx, &channel)
}

func (c *dcCall) onState(vs dcVoiceState) {
	if vs.ChannelID == nil || *vs.ChannelID == "" {
		c.mu.Lock()
		leaving := c.ended
		c.mu.Unlock()
		if !leaving {
			c.end("disconnected", false)
		}
		return
	}
	c.mu.Lock()
	moved := c.channelID != *vs.ChannelID
	c.channelID, c.selfMute, c.selfDeaf = *vs.ChannelID, vs.SelfMute, vs.SelfDeaf
	sessionChanged := c.sessionID != vs.SessionID
	c.sessionID = vs.SessionID
	c.mu.Unlock()
	if sessionChanged {
		c.restart()
	}
	if moved || !sessionChanged {
		c.report()
	}
}

func (c *dcCall) onServer(token, endpoint string) {
	c.mu.Lock()
	c.token, c.endpoint = token, endpoint
	c.mu.Unlock()
	if endpoint != "" {
		c.restart()
	}
}

// rejoin asks the main gateway for a fresh voice session, as after a gateway re-identify.
func (c *dcCall) rejoin() {
	c.mu.Lock()
	c.sessionID, c.token, c.endpoint = "", "", ""
	channel := c.channelID
	c.mu.Unlock()
	ctx, cancel := context.WithTimeout(c.ctx, 10*time.Second)
	defer cancel()
	if err := c.voiceUpdate(ctx, &channel); err != nil {
		c.end("connectionLost", false)
	}
}

// restart opens a new voice connection once the session and the server are both known.
func (c *dcCall) restart() {
	c.a.mu.Lock()
	me := c.a.me.ID
	c.a.mu.Unlock()
	c.mu.Lock()
	if c.ended || c.sessionID == "" || c.token == "" || c.endpoint == "" {
		c.mu.Unlock()
		return
	}
	old, cancelOld := c.conn, c.connCancel
	ctx, cancel := context.WithCancel(c.ctx)
	conn := &voiceConn{call: c, userID: me, guildID: c.guildID, channelID: c.channelID, sessionID: c.sessionID,
		token: c.token, endpoint: c.endpoint, scheme: c.a.bridge.discordURLs.voiceScheme, ssrcUsers: map[uint32]string{}}
	conn.dave = davesession.New(godave.UserID(me), conn, davesession.WithLogger(daveLog))
	if id, err := strconv.ParseUint(c.channelID, 10, 64); err == nil {
		conn.dave.SetChannelID(godave.ChannelID(id))
	}
	c.conn, c.connCancel = conn, cancel
	if c.state == "connected" {
		c.state = "reconnecting"
	}
	c.mu.Unlock()
	if cancelOld != nil {
		cancelOld()
	}
	if old != nil {
		old.close()
	}
	go c.watch(ctx, conn)
}

// watch runs one voice connection and decides what follows when it ends.
func (c *dcCall) watch(ctx context.Context, conn *voiceConn) {
	code, err := conn.run(ctx)
	conn.close()
	if ctx.Err() != nil {
		return
	}
	c.mu.Lock()
	current := c.conn == conn
	c.mu.Unlock()
	if !current {
		return
	}
	switch code {
	case 4004, 4011, 4016, 4017, 4020:
		c.a.bridge.log.Warnf("Discord voice for %s closed with %d: %v", c.a.id[:8], code, err)
		c.end("failed", true)
		return
	case 4014:
		// Kicked or moved: a moved call gets a new server shortly; otherwise it is over.
		select {
		case <-time.After(3 * time.Second):
			c.mu.Lock()
			still := c.conn == conn
			c.mu.Unlock()
			if still {
				c.end("disconnected", false)
			}
		case <-ctx.Done():
		}
		return
	case 4021, 4022:
		c.end("disconnected", false)
		return
	}
	c.mu.Lock()
	c.attempts++
	attempts := c.attempts
	c.state = "reconnecting"
	c.mu.Unlock()
	c.report()
	if attempts > voiceAttempts {
		c.end("connectionLost", true)
		return
	}
	select {
	case <-time.After(time.Duration(attempts) * time.Second):
	case <-ctx.Done():
		return
	}
	if code == 4006 || code == 4009 {
		c.rejoin()
		return
	}
	c.restart()
}

// ---- one voice gateway connection with its UDP socket ----

type voiceConn struct {
	call                                                       *dcCall
	userID, guildID, channelID, sessionID, token, endpoint, scheme string

	ws      atomic.Pointer[websocket.Conn]
	writeMu sync.Mutex
	seq     atomic.Int64
	acked   atomic.Bool
	closed  atomic.Bool

	udp     *net.UDPConn
	aead    cipher.AEAD
	ssrc    uint32
	nonce   atomic.Uint32
	dave    *davesession.Session
	e2ee    atomic.Bool
	version atomic.Int32

	usersMu   sync.Mutex
	ssrcUsers map[uint32]string

	sendMu   sync.Mutex
	rtpSeq   uint16
	rtpTS    uint32
	lastSend time.Time
	speaking bool
}

type voiceFrame struct {
	Op  int             `json:"op"`
	D   json.RawMessage `json:"d"`
	S   *int64          `json:"s"`
	Seq *int64          `json:"seq"`
}

func (v *voiceConn) close() {
	if v.closed.Swap(true) {
		return
	}
	if ws := v.ws.Load(); ws != nil {
		_ = ws.Close(websocket.StatusNormalClosure, "")
	}
	v.sendMu.Lock()
	udp := v.udp
	v.sendMu.Unlock()
	if udp != nil {
		_ = udp.Close()
	}
	_ = v.dave.Close()
}

func (v *voiceConn) sendJSON(op int, data any) error {
	raw, err := json.Marshal(map[string]any{"op": op, "d": data})
	if err != nil {
		return err
	}
	return v.write(websocket.MessageText, raw)
}

func (v *voiceConn) sendBinary(op byte, payload []byte) error {
	return v.write(websocket.MessageBinary, append([]byte{op}, payload...))
}

func (v *voiceConn) write(kind websocket.MessageType, raw []byte) error {
	ws := v.ws.Load()
	if ws == nil || v.closed.Load() {
		return errors.New("voice gateway is not connected")
	}
	v.writeMu.Lock()
	defer v.writeMu.Unlock()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	return ws.Write(ctx, kind, raw)
}

// godave.Callbacks: DAVE's messages to the voice gateway.
func (v *voiceConn) SendMLSKeyPackage(p []byte) error    { return v.sendBinary(26, p) }
func (v *voiceConn) SendMLSCommitWelcome(p []byte) error { return v.sendBinary(28, p) }
func (v *voiceConn) SendReadyForTransition(id uint16) error {
	return v.sendJSON(23, map[string]any{"transition_id": id})
}
func (v *voiceConn) SendInvalidCommitWelcome(id uint16) error {
	return v.sendJSON(31, map[string]any{"transition_id": id})
}

var daveLog = slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelWarn}))

// run connects, identifies and serves the connection; it returns the close code.
func (v *voiceConn) run(ctx context.Context) (int, error) {
	scheme := v.scheme
	if scheme == "" {
		scheme = "wss"
	}
	host := strings.TrimPrefix(strings.TrimPrefix(v.endpoint, "wss://"), "ws://")
	dialCtx, cancel := context.WithTimeout(ctx, 15*time.Second)
	ws, _, err := websocket.Dial(dialCtx, scheme+"://"+strings.TrimRight(host, "/")+"/?v=8", &websocket.DialOptions{
		HTTPHeader: http.Header{"Origin": {"https://discord.com"}, "User-Agent": {discordUserAgent}}})
	cancel()
	if err != nil {
		return 0, err
	}
	ws.SetReadLimit(1 << 20)
	v.ws.Store(ws)
	runCtx, stop := context.WithCancel(ctx)
	defer stop()
	go func() {
		<-runCtx.Done()
		v.close()
	}()
	for {
		kind, data, err := ws.Read(runCtx)
		if err != nil {
			return int(websocket.CloseStatus(err)), err
		}
		if kind == websocket.MessageBinary {
			v.binary(data)
			continue
		}
		var f voiceFrame
		if json.Unmarshal(data, &f) != nil {
			continue
		}
		if f.Seq != nil {
			v.seq.Store(*f.Seq)
		} else if f.S != nil {
			v.seq.Store(*f.S)
		}
		if err = v.handle(runCtx, f); err != nil {
			return 0, err
		}
	}
}

func (v *voiceConn) handle(ctx context.Context, f voiceFrame) error {
	switch f.Op {
	case 8: // hello
		var d struct {
			Interval float64 `json:"heartbeat_interval"`
		}
		_ = json.Unmarshal(f.D, &d)
		go v.heartbeat(ctx, time.Duration(d.Interval*float64(time.Millisecond)))
		return v.sendJSON(0, map[string]any{"server_id": v.guildID, "user_id": v.userID, "session_id": v.sessionID,
			"token": v.token, "max_dave_protocol_version": v.dave.MaxSupportedProtocolVersion(), "video": false})
	case 2: // ready
		var d struct {
			SSRC  uint32   `json:"ssrc"`
			IP    string   `json:"ip"`
			Port  int      `json:"port"`
			Modes []string `json:"modes"`
		}
		if err := json.Unmarshal(f.D, &d); err != nil {
			return err
		}
		mode := ""
		for _, preferred := range []string{"aead_aes256_gcm_rtpsize", "aead_xchacha20_poly1305_rtpsize"} {
			for _, m := range d.Modes {
				if m == preferred && mode == "" {
					mode = m
				}
			}
		}
		if mode == "" {
			return fmt.Errorf("no supported voice encryption mode in %v", d.Modes)
		}
		ip, port, err := v.discover(ctx, d.IP, d.Port, d.SSRC)
		if err != nil {
			return err
		}
		v.ssrc = d.SSRC
		v.dave.AssignSsrcToCodec(d.SSRC, godave.CodecOpus)
		return v.sendJSON(1, map[string]any{"protocol": "udp", "data": map[string]any{"address": ip, "port": port, "mode": mode}})
	case 4: // session description
		var d struct {
			Mode    string `json:"mode"`
			Key     []int  `json:"secret_key"`
			Version int    `json:"dave_protocol_version"`
		}
		if err := json.Unmarshal(f.D, &d); err != nil {
			return err
		}
		key := make([]byte, len(d.Key))
		for i, b := range d.Key {
			key[i] = byte(b)
		}
		var aead cipher.AEAD
		var err error
		if d.Mode == "aead_xchacha20_poly1305_rtpsize" {
			aead, err = chacha20poly1305.NewX(key)
		} else {
			var block cipher.Block
			if block, err = aes.NewCipher(key); err == nil {
				aead, err = cipher.NewGCM(block)
			}
		}
		if err != nil {
			return err
		}
		v.sendMu.Lock()
		v.aead = aead
		v.sendMu.Unlock()
		v.version.Store(int32(d.Version))
		v.dave.OnSelectProtocolAck(uint16(d.Version))
		go v.receive(ctx)
		go v.tick(ctx)
		c := v.call
		c.mu.Lock()
		if c.conn == v {
			c.state, c.attempts = "connected", 0
		}
		c.mu.Unlock()
		c.report()
	case 5: // speaking: which user sends on an SSRC
		var d struct {
			UserID string `json:"user_id"`
			SSRC   uint32 `json:"ssrc"`
		}
		if json.Unmarshal(f.D, &d) == nil && d.UserID != "" {
			v.usersMu.Lock()
			v.ssrcUsers[d.SSRC] = d.UserID
			v.usersMu.Unlock()
		}
	case 6:
		v.acked.Store(true)
	case 11:
		var d struct {
			UserIDs []string `json:"user_ids"`
		}
		if json.Unmarshal(f.D, &d) == nil {
			for _, id := range d.UserIDs {
				v.dave.AddUser(godave.UserID(id))
			}
		}
	case 13:
		var d struct {
			UserID string `json:"user_id"`
		}
		if json.Unmarshal(f.D, &d) == nil {
			v.dave.RemoveUser(godave.UserID(d.UserID))
			v.usersMu.Lock()
			for ssrc, user := range v.ssrcUsers {
				if user == d.UserID {
					delete(v.ssrcUsers, ssrc)
				}
			}
			v.usersMu.Unlock()
		}
	case 21:
		var d struct {
			Version    uint16 `json:"protocol_version"`
			Transition uint16 `json:"transition_id"`
		}
		if json.Unmarshal(f.D, &d) == nil {
			v.version.Store(int32(d.Version))
			v.dave.OnDavePrepareTransition(d.Transition, d.Version)
		}
	case 22:
		var d struct {
			Transition uint16 `json:"transition_id"`
		}
		if json.Unmarshal(f.D, &d) == nil {
			v.dave.OnDaveExecuteTransition(d.Transition)
		}
	case 24:
		var d struct {
			Version uint16 `json:"protocol_version"`
			Epoch   int    `json:"epoch"`
		}
		if json.Unmarshal(f.D, &d) == nil {
			v.version.Store(int32(d.Version))
			v.dave.OnDavePrepareEpoch(d.Epoch, d.Version)
		}
	}
	return nil
}

// binary handles DAVE's binary messages: sequence (2 bytes), opcode, payload.
func (v *voiceConn) binary(data []byte) {
	if len(data) < 3 {
		return
	}
	v.seq.Store(int64(binary.BigEndian.Uint16(data)))
	op, payload := data[2], data[3:]
	switch op {
	case 25:
		v.dave.OnDaveMLSExternalSenderPackage(payload)
	case 27:
		v.dave.OnDaveMLSProposals(payload)
	case 29, 30:
		if len(payload) < 2 {
			return
		}
		transition := binary.BigEndian.Uint16(payload)
		if op == 29 {
			v.dave.OnDaveMLSPrepareCommitTransition(transition, payload[2:])
		} else {
			v.dave.OnDaveMLSWelcome(transition, payload[2:])
		}
	}
}

func (v *voiceConn) heartbeat(ctx context.Context, interval time.Duration) {
	if interval < time.Second {
		interval = 13750 * time.Millisecond
	}
	v.acked.Store(true)
	t := time.NewTicker(interval)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
			if !v.acked.Swap(false) {
				if ws := v.ws.Load(); ws != nil {
					_ = ws.Close(4000, "heartbeat not acknowledged")
				}
				return
			}
			_ = v.sendJSON(3, map[string]any{"t": time.Now().UnixMilli(), "seq_ack": v.seq.Load()})
		}
	}
}

// discover sends Discord's IP discovery packet and reads the address it saw.
func (v *voiceConn) discover(ctx context.Context, ip string, port int, ssrc uint32) (string, int, error) {
	addr, err := net.ResolveUDPAddr("udp", net.JoinHostPort(ip, strconv.Itoa(port)))
	if err != nil {
		return "", 0, err
	}
	udp, err := net.DialUDP("udp", nil, addr)
	if err != nil {
		return "", 0, err
	}
	v.sendMu.Lock()
	v.udp = udp
	v.sendMu.Unlock()
	if v.closed.Load() {
		_ = udp.Close()
		return "", 0, net.ErrClosed
	}
	packet := make([]byte, 74)
	binary.BigEndian.PutUint16(packet, 1)
	binary.BigEndian.PutUint16(packet[2:], 70)
	binary.BigEndian.PutUint32(packet[4:], ssrc)
	answer := make([]byte, 74)
	for attempt := 0; attempt < 3; attempt++ {
		if _, err = udp.Write(packet); err != nil {
			return "", 0, err
		}
		_ = udp.SetReadDeadline(time.Now().Add(2 * time.Second))
		n, err := udp.Read(answer)
		if err != nil || n < 74 || binary.BigEndian.Uint16(answer) != 2 || binary.BigEndian.Uint32(answer[4:]) != ssrc {
			if ctx.Err() != nil {
				return "", 0, ctx.Err()
			}
			continue
		}
		_ = udp.SetReadDeadline(time.Time{})
		return string(bytes.TrimRight(answer[8:72], "\x00")), int(binary.BigEndian.Uint16(answer[72:])), nil
	}
	return "", 0, errors.New("voice server did not answer IP discovery")
}

// receive reads RTP, removes both layers of encryption and hands Opus to the runtime.
func (v *voiceConn) receive(ctx context.Context) {
	buf := make([]byte, 1500)
	plain := make([]byte, 0, 1500)
	nonce := make([]byte, v.aead.NonceSize())
	for ctx.Err() == nil {
		n, err := v.udp.Read(buf)
		if err != nil {
			if ctx.Err() == nil && !v.closed.Load() {
				var timeout net.Error
				if errors.As(err, &timeout) && timeout.Timeout() {
					continue
				}
			}
			return
		}
		packet := buf[:n]
		if n < rtpHeaderSize+4+v.aead.Overhead() || packet[0]>>6 != 2 || packet[1]&0x7f != rtpPayloadOpus {
			continue
		}
		header := rtpHeaderSize + 4*int(packet[0]&0x0f)
		extended := packet[0]&0x10 != 0
		extWords := 0
		if extended {
			if n < header+4 {
				continue
			}
			extWords = int(binary.BigEndian.Uint16(packet[header+2:]))
			header += 4
		}
		if n < header+4+v.aead.Overhead() {
			continue
		}
		clear(nonce)
		copy(nonce, packet[n-4:])
		plain, err = v.aead.Open(plain[:0], nonce, packet[header:n-4], packet[:header])
		if err != nil {
			continue
		}
		payload := plain
		if extended {
			if len(payload) < extWords*4 {
				continue
			}
			payload = payload[extWords*4:]
		}
		if packet[0]&0x20 != 0 && len(payload) > 0 {
			pad := int(payload[len(payload)-1])
			if pad > len(payload) {
				continue
			}
			payload = payload[:len(payload)-pad]
		}
		if len(payload) == 0 || bytes.Equal(payload, opusSilence) {
			continue
		}
		ssrc := binary.BigEndian.Uint32(packet[8:])
		v.usersMu.Lock()
		user := v.ssrcUsers[ssrc]
		v.usersMu.Unlock()
		c := v.call
		c.mu.Lock()
		deaf := c.selfDeaf
		c.mu.Unlock()
		if user == "" || deaf {
			continue
		}
		out := make([]byte, v.dave.MaxDecryptedFrameSize(godave.UserID(user), len(payload)))
		m, err := v.dave.Decrypt(godave.UserID(user), payload, out)
		if err != nil || m == 0 {
			continue
		}
		c.a.emit("voiceAudio", map[string]any{"user": user, "seq": binary.BigEndian.Uint16(packet[2:]), "data": out[:m]})
	}
}

// tick ends a burst of speech with silence, keeps the UDP mapping alive and reports E2EE.
func (v *voiceConn) tick(ctx context.Context) {
	t := time.NewTicker(100 * time.Millisecond)
	defer t.Stop()
	keepalive := time.Now()
	var counter uint32
	for {
		select {
		case <-ctx.Done():
			return
		case now := <-t.C:
			v.sendMu.Lock()
			if v.speaking && now.Sub(v.lastSend) > 250*time.Millisecond {
				for range 5 {
					v.packet(opusSilence)
				}
				v.speaking = false
				_ = v.sendJSON(5, map[string]any{"speaking": 0, "delay": 0, "ssrc": v.ssrc})
			}
			v.sendMu.Unlock()
			if now.Sub(keepalive) > 5*time.Second {
				keepalive = now
				counter++
				ka := make([]byte, 8)
				binary.LittleEndian.PutUint32(ka, counter)
				_, _ = v.udp.Write(ka)
			}
			state := v.dave.State()
			if e2ee := state.Ready && state.ProtocolVersion > 0; v.e2ee.Swap(e2ee) != e2ee {
				v.call.report()
			}
		}
	}
}

// send transmits one Opus frame from the browser's microphone.
func (v *voiceConn) send(frame []byte) {
	if v.closed.Load() || v.dave.ShouldHoldFrames() {
		return
	}
	v.sendMu.Lock()
	defer v.sendMu.Unlock()
	if v.aead == nil {
		return
	}
	now := time.Now()
	if !v.speaking {
		v.speaking = true
		go v.sendJSON(5, map[string]any{"speaking": 1, "delay": 0, "ssrc": v.ssrc})
	}
	if !v.lastSend.IsZero() && now.Sub(v.lastSend) > 60*time.Millisecond {
		// After a pause the timestamp follows the clock, in whole frames.
		v.rtpTS += uint32(now.Sub(v.lastSend)/(20*time.Millisecond)) * opusFrameSamples
	}
	v.lastSend = now
	v.packet(frame)
}

// packet writes one RTP packet; sendMu is held.
func (v *voiceConn) packet(frame []byte) {
	header := make([]byte, rtpHeaderSize, rtpHeaderSize+len(frame)+64)
	header[0], header[1] = 0x80, rtpPayloadOpus
	binary.BigEndian.PutUint16(header[2:], v.rtpSeq)
	binary.BigEndian.PutUint32(header[4:], v.rtpTS)
	binary.BigEndian.PutUint32(header[8:], v.ssrc)
	v.rtpSeq++
	v.rtpTS += opusFrameSamples
	encrypted := make([]byte, v.dave.MaxEncryptedFrameSize(len(frame)))
	n, err := v.dave.Encrypt(v.ssrc, frame, encrypted)
	if err != nil {
		return
	}
	nonce := make([]byte, v.aead.NonceSize())
	binary.BigEndian.PutUint32(nonce, v.nonce.Add(1))
	out := v.aead.Seal(header, nonce, encrypted[:n], header[:rtpHeaderSize])
	_, _ = v.udp.Write(append(out, nonce[:4]...))
}

// sendAudio passes microphone frames to the call, unless it is muted.
func (a *dcAccount) sendAudio(frames [][]byte) {
	a.mu.Lock()
	call := a.call
	a.mu.Unlock()
	if call == nil {
		return
	}
	call.mu.Lock()
	conn, muted := call.conn, call.selfMute
	call.mu.Unlock()
	if conn == nil || muted {
		return
	}
	for _, f := range frames {
		if len(f) > 0 && len(f) <= 1275 {
			conn.send(f)
		}
	}
}

// callCode is the 30-digit code everyone in an end-to-end encrypted call sees.
func (a *dcAccount) callCode(ctx context.Context) (string, error) {
	a.mu.Lock()
	call := a.call
	a.mu.Unlock()
	if call == nil {
		return "", fail("conflict", "You are not in a call.")
	}
	call.mu.Lock()
	conn := call.conn
	call.mu.Unlock()
	if conn == nil || conn.dave == nil || !conn.e2ee.Load() {
		return "", fail("conflict", "This call is not end-to-end encrypted.")
	}
	return conn.dave.EpochAuthenticatorCode(ctx)
}
