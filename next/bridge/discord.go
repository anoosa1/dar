package main

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// A Discord account: a user session (not a bot) linked by QR sign-in. The bridge keeps the
// servers, channels, roles and friends it needs in memory and hands chats and messages to
// the runtime like WhatsApp's.

type discordURLs struct{ api, gateway, remoteAuth string }

var defaultDiscordURLs = discordURLs{api: discordAPIBase, gateway: discordGatewayURL, remoteAuth: remoteAuthURL}

const (
	keyDiscordToken = "discord/token"
	keyDiscordUser  = "discord/user"
	permAdmin       = 1 << 3
	permView        = 1 << 10
)

type guildState struct {
	id, name, ownerID string
	order             int
	roles             map[string]dcRole
	myRoles           []string
	channels          map[string]*dcChannel
	mutedUntil        int64
}

type readState struct {
	last     string
	mentions int
}

type dcAccount struct {
	id      string
	bridge  *bridge
	kv      *kv
	api     *discordAPI
	cancel  context.CancelFunc
	linking atomic.Bool
	stopped atomic.Bool

	mu          sync.Mutex
	me          dcUser
	users       map[string]dcUser
	guilds      map[string]*guildState
	privates    map[string]*dcChannel
	friends     map[string]bool
	reads       map[string]readState
	channelMute map[string]int64
}

func (b *bridge) newDiscordAccount(id string) *dcAccount {
	return &dcAccount{id: id, bridge: b, kv: newKV(id, b.backend), users: map[string]dcUser{}, guilds: map[string]*guildState{},
		privates: map[string]*dcChannel{}, friends: map[string]bool{}, reads: map[string]readState{}, channelMute: map[string]int64{}}
}

func (a *dcAccount) emit(event string, data any) { a.bridge.peer.emit(a.id, event, data) }

func (a *dcAccount) ingest(ctx context.Context, data map[string]any) bool {
	data["account"] = a.id
	ctx, cancel := context.WithTimeout(ctx, 2*time.Minute)
	defer cancel()
	if err := a.bridge.peer.call(ctx, "ingest", data, nil); err != nil {
		a.bridge.log.Warnf("Storing Discord %s for %s failed: %v", data["kind"], a.id[:8], err)
		return false
	}
	return true
}

func (a *dcAccount) stop() {
	a.stopped.Store(true)
	if a.cancel != nil {
		a.cancel()
	}
}

// start opens the gateway with a stored token.
func (a *dcAccount) start(token string) {
	ctx, cancel := context.WithCancel(context.Background())
	a.cancel = cancel
	a.api = newDiscordAPI(a.bridge.discordURLs.api, token)
	g := &gateway{url: a.bridge.discordURLs.gateway, token: token, dispatch: a.dispatch, state: func(state, detail string) {
		switch state {
		case "connected":
			a.emit("connected", map[string]any{})
		case "disconnected":
			a.emit("disconnected", map[string]any{"reason": detail})
		case "loggedOut":
			a.emit("loggedOut", map[string]any{"reason": detail})
			go a.bridge.stop(a.id)
		case "failed":
			a.emit("outdated", map[string]any{})
			go a.bridge.stop(a.id)
		}
	}}
	go g.run(ctx)
}

// ---- READY and state ----

func muteUntil(muted bool, config *struct {
	EndTime *string `json:"end_time"`
}) int64 {
	if !muted {
		return 0
	}
	if config == nil || config.EndTime == nil || *config.EndTime == "" {
		return -1
	}
	end, err := time.Parse(time.RFC3339Nano, *config.EndTime)
	if err != nil {
		return -1
	}
	if end.Before(time.Now()) {
		return 0
	}
	return end.UnixMilli()
}

type guildSettings struct {
	GuildID    *string `json:"guild_id"`
	Muted      bool    `json:"muted"`
	MuteConfig *struct {
		EndTime *string `json:"end_time"`
	} `json:"mute_config"`
	ChannelOverrides []struct {
		ChannelID  string `json:"channel_id"`
		Muted      bool   `json:"muted"`
		MuteConfig *struct {
			EndTime *string `json:"end_time"`
		} `json:"mute_config"`
	} `json:"channel_overrides"`
}

// entries reads a list Discord sends either plainly or as {"entries": [...]}.
func entries[T any](raw json.RawMessage) []T {
	var list []T
	if json.Unmarshal(raw, &list) == nil {
		return list
	}
	var versioned struct {
		Entries []T `json:"entries"`
	}
	_ = json.Unmarshal(raw, &versioned)
	return versioned.Entries
}

func (a *dcAccount) applySettings(list []guildSettings) {
	for _, s := range list {
		muted := muteUntil(s.Muted, s.MuteConfig)
		if s.GuildID != nil && *s.GuildID != "" {
			if g := a.guilds[*s.GuildID]; g != nil {
				g.mutedUntil = muted
			}
		}
		for _, o := range s.ChannelOverrides {
			a.channelMute[o.ChannelID] = muteUntil(o.Muted, o.MuteConfig)
		}
	}
}

func newGuildState(g dcGuild, order int) *guildState {
	s := &guildState{id: g.ID, name: g.Name, ownerID: g.OwnerID, order: order, roles: map[string]dcRole{}, channels: map[string]*dcChannel{}}
	if g.Properties != nil {
		if g.Properties.Name != "" {
			s.name = g.Properties.Name
		}
		if g.Properties.OwnerID != "" {
			s.ownerID = g.Properties.OwnerID
		}
	}
	for _, r := range g.Roles {
		s.roles[r.ID] = r
	}
	for i := range g.Channels {
		c := g.Channels[i]
		c.GuildID = g.ID
		s.channels[c.ID] = &c
	}
	return s
}

func (a *dcAccount) ready(ctx context.Context, raw json.RawMessage) {
	var r struct {
		User              dcUser            `json:"user"`
		Users             []dcUser          `json:"users"`
		Guilds            []dcGuild         `json:"guilds"`
		MergedMembers     [][]dcMember      `json:"merged_members"`
		PrivateChannels   []dcChannel       `json:"private_channels"`
		ReadState         json.RawMessage   `json:"read_state"`
		UserGuildSettings json.RawMessage   `json:"user_guild_settings"`
		Relationships     []json.RawMessage `json:"relationships"`
	}
	if err := json.Unmarshal(raw, &r); err != nil {
		a.bridge.log.Warnf("Discord READY could not be read: %v", err)
		return
	}
	a.mu.Lock()
	a.me = r.User
	a.users = map[string]dcUser{r.User.ID: r.User}
	for _, u := range r.Users {
		a.users[u.ID] = u
	}
	a.guilds = map[string]*guildState{}
	for i, g := range r.Guilds {
		if g.Unavailable {
			continue
		}
		s := newGuildState(g, i)
		members := g.Members
		if i < len(r.MergedMembers) {
			members = append(members, r.MergedMembers[i]...)
		}
		for _, m := range members {
			if m.id() == r.User.ID {
				s.myRoles = m.Roles
			}
		}
		a.guilds[g.ID] = s
	}
	a.privates = map[string]*dcChannel{}
	for i := range r.PrivateChannels {
		c := r.PrivateChannels[i]
		a.privates[c.ID] = &c
	}
	a.reads = map[string]readState{}
	for _, s := range entries[struct {
		ID            string `json:"id"`
		LastMessageID flexID `json:"last_message_id"`
		MentionCount  int    `json:"mention_count"`
		ReadStateType int    `json:"read_state_type"`
	}](r.ReadState) {
		if s.ReadStateType == 0 {
			a.reads[s.ID] = readState{last: string(s.LastMessageID), mentions: s.MentionCount}
		}
	}
	a.channelMute = map[string]int64{}
	a.applySettings(entries[guildSettings](r.UserGuildSettings))
	a.friends = map[string]bool{}
	for _, raw := range r.Relationships {
		a.addRelationship(raw)
	}
	a.mu.Unlock()
	a.storeUser(ctx)

	// Servers first (removing ones Dar no longer sees), then each server's channels, then DMs.
	a.ingest(ctx, map[string]any{"kind": "servers", "servers": a.serversOut(), "complete": true})
	a.mu.Lock()
	ids := make([]string, 0, len(a.guilds))
	for id := range a.guilds {
		ids = append(ids, id)
	}
	a.mu.Unlock()
	for _, id := range ids {
		a.syncGuild(ctx, id)
	}
	a.syncPrivates(ctx, nil)
	a.emit("connected", map[string]any{"name": r.User.name()})
}

func (a *dcAccount) storeUser(ctx context.Context) {
	a.mu.Lock()
	me := a.me
	a.mu.Unlock()
	if me.ID != "" {
		_ = putJSON(ctx, a.kv, keyDiscordUser, me, 0)
	}
}

func (a *dcAccount) addRelationship(raw json.RawMessage) {
	var rel struct {
		ID     string  `json:"id"`
		Type   int     `json:"type"`
		UserID string  `json:"user_id"`
		User   *dcUser `json:"user"`
	}
	if json.Unmarshal(raw, &rel) != nil {
		return
	}
	id := rel.UserID
	if id == "" {
		id = rel.ID
	}
	if rel.User != nil {
		a.users[rel.User.ID] = *rel.User
		id = rel.User.ID
	}
	if rel.Type == 1 {
		a.friends[id] = true
	} else {
		delete(a.friends, id)
	}
}

// serversOut lists the servers in the order READY gave them. Call without a.mu held.
func (a *dcAccount) serversOut() []dcServerOut {
	a.mu.Lock()
	defer a.mu.Unlock()
	out := make([]dcServerOut, 0, len(a.guilds))
	for _, g := range a.guilds {
		out = append(out, dcServerOut{ID: g.id, Name: g.name, Order: fmt.Sprintf("%05d", g.order), MutedUntil: g.mutedUntil})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Order < out[j].Order })
	return out
}

// permissions computes what the account may do in a guild channel.
func (g *guildState) permissions(c *dcChannel, me string) int64 {
	if g.ownerID == me {
		return -1
	}
	base := int64(g.roles[g.id].Permissions)
	for _, id := range g.myRoles {
		base |= int64(g.roles[id].Permissions)
	}
	if base&permAdmin != 0 {
		return -1
	}
	var allow, deny int64
	for _, o := range c.Overwrites {
		if o.Type == 0 && o.ID == g.id {
			base = base&^int64(o.Deny) | int64(o.Allow)
		}
	}
	for _, o := range c.Overwrites {
		if o.Type == 0 && o.ID != g.id && contains(g.myRoles, o.ID) {
			allow |= int64(o.Allow)
			deny |= int64(o.Deny)
		}
	}
	base = base&^deny | allow
	for _, o := range c.Overwrites {
		if o.Type == 1 && o.ID == me {
			base = base&^int64(o.Deny) | int64(o.Allow)
		}
	}
	return base
}

func contains(list []string, value string) bool {
	for _, v := range list {
		if v == value {
			return true
		}
	}
	return false
}

func (a *dcAccount) unreadOf(c *dcChannel) (int, int) {
	last := string(c.LastMessageID)
	read, known := a.reads[c.ID]
	unread := 0
	if last != "" && (!known || snowflakeLess(read.last, last)) {
		unread = 1
	}
	if read.mentions > unread {
		unread = read.mentions
	}
	return unread, read.mentions
}

// guildChats lists a guild's text and voice channels the account can see, in Discord's order:
// channels without a category first, then each category; text before voice within one.
func (a *dcAccount) guildChats(g *guildState) []dcChatOut {
	out := []dcChatOut{}
	for _, c := range g.channels {
		if c.Type != channelText && c.Type != channelNews && c.Type != channelVoice {
			continue
		}
		if g.permissions(c, a.me.ID)&permView == 0 {
			continue
		}
		section, sectionPos := "", -1
		if c.ParentID != nil {
			if parent := g.channels[*c.ParentID]; parent != nil {
				section, sectionPos = parent.Name, parent.Position
			}
		}
		kind, voice := "channel", 0
		if c.Type == channelVoice {
			kind, voice = "voice", 1
		}
		unread, mentions := a.unreadOf(c)
		muted := a.channelMute[c.ID]
		topic := ""
		if c.Topic != nil {
			topic = *c.Topic
		}
		out = append(out, dcChatOut{ID: c.ID, Kind: kind, Name: c.Name, ServerID: g.id, Section: section, Topic: topic,
			Order: fmt.Sprintf("%06d/%d/%06d/%s", sectionPos+1, voice, c.Position, c.ID), Unread: &unread, Mentions: &mentions, MutedUntil: &muted, At: snowflakeTime(string(c.LastMessageID))})
	}
	return out
}

// syncGuild stores a guild's visible channels and removes ones it no longer has.
func (a *dcAccount) syncGuild(ctx context.Context, id string) {
	a.mu.Lock()
	g := a.guilds[id]
	var chats []dcChatOut
	if g != nil {
		chats = a.guildChats(g)
	}
	a.mu.Unlock()
	if g == nil {
		return
	}
	a.ingest(ctx, map[string]any{"kind": "channels", "serverId": id, "chats": chats, "complete": true})
}

func (a *dcAccount) privateChat(c *dcChannel) dcChatOut {
	kind, name := "direct", c.Name
	var people []string
	for _, id := range c.RecipientIDs {
		if u, ok := a.users[id]; ok {
			people = append(people, u.name())
		}
	}
	for _, u := range c.Recipients {
		a.users[u.ID] = u
		people = append(people, u.name())
	}
	if c.Type == channelGroupDM {
		kind = "group"
	}
	if name == "" {
		name = strings.Join(people, ", ")
	}
	unread, mentions := a.unreadOf(c)
	if mentions > unread {
		unread = mentions
	}
	muted := a.channelMute[c.ID]
	return dcChatOut{ID: c.ID, Kind: kind, Name: name, Unread: &unread, Mentions: &mentions, MutedUntil: &muted, At: snowflakeTime(string(c.LastMessageID))}
}

// syncPrivates stores direct messages and group DMs (all of them when ids is nil).
func (a *dcAccount) syncPrivates(ctx context.Context, ids []string) {
	a.mu.Lock()
	var chats []dcChatOut
	for id, c := range a.privates {
		if ids != nil && !contains(ids, id) {
			continue
		}
		if c.Type == channelDM || c.Type == channelGroupDM {
			chats = append(chats, a.privateChat(c))
		}
	}
	a.mu.Unlock()
	for start := 0; start < len(chats); start += 1000 {
		a.ingest(ctx, map[string]any{"kind": "chats", "chats": chats[start:min(start+1000, len(chats))]})
	}
}

// ---- Messages ----

func (a *dcAccount) channelName(id string) string {
	for _, g := range a.guilds {
		if c := g.channels[id]; c != nil {
			return c.Name
		}
	}
	return ""
}

func (a *dcAccount) roleName(guild, id string) string {
	if g := a.guilds[guild]; g != nil {
		return g.roles[id].Name
	}
	return ""
}

// render turns Discord's markup for mentions, channels, roles and custom emoji into plain text.
func (a *dcAccount) render(m *dcMessage) string {
	text := m.Content
	if !strings.Contains(text, "<") {
		return text
	}
	mentioned := map[string]string{}
	for _, u := range m.Mentions {
		mentioned[u.ID] = u.name()
	}
	var out strings.Builder
	for {
		start := strings.IndexByte(text, '<')
		if start < 0 {
			out.WriteString(text)
			break
		}
		end := strings.IndexByte(text[start:], '>')
		if end < 0 {
			out.WriteString(text)
			break
		}
		out.WriteString(text[:start])
		token, replaced := text[start+1:start+end], ""
		switch {
		case strings.HasPrefix(token, "@&"):
			if name := a.roleName(m.GuildID, token[2:]); name != "" {
				replaced = "@" + name
			}
		case strings.HasPrefix(token, "@"):
			id := strings.TrimPrefix(token[1:], "!")
			if name, ok := mentioned[id]; ok {
				replaced = "@" + name
			} else if u, ok := a.users[id]; ok {
				replaced = "@" + u.name()
			}
		case strings.HasPrefix(token, "#"):
			if name := a.channelName(token[1:]); name != "" {
				replaced = "#" + name
			}
		case strings.HasPrefix(token, ":") || strings.HasPrefix(token, "a:"):
			parts := strings.Split(token, ":")
			if len(parts) == 3 {
				replaced = ":" + parts[1] + ":"
			}
		}
		if replaced == "" {
			replaced = "<" + token + ">"
		}
		out.WriteString(replaced)
		text = text[start+end+1:]
	}
	return out.String()
}

func (a *dcAccount) senderName(m *dcMessage) string {
	if m.Member != nil && m.Member.Nick != nil && *m.Member.Nick != "" {
		return *m.Member.Nick
	}
	return m.Author.name()
}

func attachmentKind(at dcAttachment, flags int) (string, string) {
	mime := strings.ToLower(strings.TrimSpace(strings.SplitN(at.ContentType, ";", 2)[0]))
	if mime == "" {
		mime = mimeByName(at.Filename)
	}
	switch {
	case flags&(1<<13) != 0:
		return "voice", mime
	case strings.HasPrefix(mime, "image/"):
		return "image", mime
	case strings.HasPrefix(mime, "video/"):
		return "video", mime
	case strings.HasPrefix(mime, "audio/"):
		return "audio", mime
	}
	return "document", mime
}

// mediaURL is where an attachment can be fetched; kept encrypted in the key store.
type dcStoredMedia struct {
	URL string `json:"url"`
}

// convertMessage maps a Discord message to what Dar stores, with the attachment's URL to remember.
func (a *dcAccount) convertMessage(m *dcMessage) (dcMessageOut, string, bool) {
	var out dcMessageOut
	at := snowflakeTime(m.ID)
	if t, err := time.Parse(time.RFC3339Nano, m.Timestamp); err == nil {
		at = t.UnixMilli()
	}
	out.ID, out.Chat, out.Sender, out.SenderName, out.At = m.ID, m.ChannelID, m.Author.ID, a.senderName(m), at
	out.FromMe = m.Author.ID == a.me.ID
	out.Kind, out.Text = "text", a.render(m)
	switch m.Type {
	case 0, 19, 20, 23:
	case 7:
		out.Text = "joined the server."
	case 6:
		out.Text = "pinned a message."
	case 8, 9, 10, 11:
		out.Text = "boosted the server."
	default:
		return out, "", false
	}
	url := ""
	if len(m.Attachments) > 0 {
		first := m.Attachments[0]
		kind, mime := attachmentKind(first, m.Flags)
		out.Kind, out.FileName, url = kind, first.Filename, first.URL
		out.Media = &mediaInfo{Mime: mime, Size: first.Size, Width: first.Width, Height: first.Height, Seconds: uint32(first.Duration + 0.5)}
		if len(m.Attachments) > 1 {
			names := make([]string, 0, len(m.Attachments)-1)
			for _, more := range m.Attachments[1:] {
				names = append(names, more.Filename)
			}
			extra := "Also attached: " + strings.Join(names, ", ")
			out.Text = strings.TrimSpace(out.Text + "\n" + extra)
		}
	} else if len(m.Stickers) > 0 {
		s := m.Stickers[0]
		out.Kind, out.FileName = "sticker", s.Name
		switch s.FormatType {
		case 1, 2:
			url = "https://media.discordapp.net/stickers/" + s.ID + ".png?size=320"
			out.Media = &mediaInfo{Mime: "image/png"}
		case 4:
			url = "https://media.discordapp.net/stickers/" + s.ID + ".gif?size=320"
			out.Media = &mediaInfo{Mime: "image/gif"}
		}
		if out.Text == "" {
			out.Text = s.Name
		}
	}
	if out.Text == "" && out.Media == nil && len(m.Embeds) > 0 {
		e := m.Embeds[0]
		out.Text = strings.TrimSpace(strings.TrimSpace(e.Title+"\n"+e.Description) + "\n" + e.URL)
	}
	if out.Text == "" && out.Media == nil {
		return out, "", false
	}
	if len(out.Text) > 65536 {
		out.Text = out.Text[:65536]
	}
	if r := m.ReferencedMessage; r != nil {
		out.QuotedID, out.QuotedSender, out.QuotedText = r.ID, a.senderName(r), a.render(r)
		if r.Author.ID == a.me.ID {
			out.QuotedSender = "You"
		}
		if out.QuotedText == "" && len(r.Attachments) > 0 {
			out.QuotedText = r.Attachments[0].Filename
		}
		if len(out.QuotedText) > 2000 {
			out.QuotedText = out.QuotedText[:2000]
		}
	} else if m.MessageReference != nil && m.Type == 19 {
		out.QuotedID = m.MessageReference.MessageID
	}
	if out.FromMe {
		out.Status = "sent"
		out.Nonce = string(m.Nonce)
	}
	for _, r := range m.Reactions {
		out.Reactions = append(out.Reactions, dcReactionOut{Emoji: r.Emoji.key(), Count: r.Count, Me: r.Me})
	}
	out.Mention = !out.FromMe && a.mentions(m)
	return out, url, true
}

// mentions is whether a message calls the account: always in DMs, otherwise by name, role or @everyone.
func (a *dcAccount) mentions(m *dcMessage) bool {
	if m.GuildID == "" {
		_, private := a.privates[m.ChannelID]
		if private {
			return true
		}
	}
	for _, u := range m.Mentions {
		if u.ID == a.me.ID {
			return true
		}
	}
	if m.MentionEveryone {
		return true
	}
	if g := a.guilds[m.GuildID]; g != nil {
		for _, id := range m.MentionRoles {
			if contains(g.myRoles, id) {
				return true
			}
		}
	}
	return false
}

// storeMessages remembers attachment URLs and hands the messages over in one ingest.
func (a *dcAccount) storeMessages(ctx context.Context, kind string, list []dcMessage, chats []dcChatOut) bool {
	a.mu.Lock()
	out := make([]dcMessageOut, 0, len(list))
	urls := map[string]string{}
	for i := range list {
		msg, url, ok := a.convertMessage(&list[i])
		if !ok {
			continue
		}
		out = append(out, msg)
		if url != "" {
			urls[mediaName(msg.Chat, msg.ID)] = url
		}
	}
	a.mu.Unlock()
	if len(out) == 0 && len(chats) == 0 {
		return true
	}
	err := a.kv.transaction(ctx, func(ctx context.Context) error {
		for name, url := range urls {
			if err := putJSON(ctx, a.kv, name, dcStoredMedia{URL: url}, 0); err != nil {
				return err
			}
		}
		return nil
	})
	if err != nil {
		return false
	}
	payload := map[string]any{"kind": kind, "messages": out}
	if len(chats) > 0 {
		payload["chats"] = chats
	}
	return a.ingest(ctx, payload)
}

// knownChannel reports whether a channel is one Dar shows, and the chat to create for a new DM.
func (a *dcAccount) knownChannel(m *dcMessage) (bool, []dcChatOut) {
	a.mu.Lock()
	defer a.mu.Unlock()
	if m.GuildID != "" {
		g := a.guilds[m.GuildID]
		if g == nil {
			return false, nil
		}
		c := g.channels[m.ChannelID]
		if c == nil || g.permissions(c, a.me.ID)&permView == 0 {
			return false, nil
		}
		c.LastMessageID = flexID(m.ID)
		return true, nil
	}
	c := a.privates[m.ChannelID]
	if c == nil {
		c = &dcChannel{ID: m.ChannelID, Type: channelDM, Recipients: []dcUser{m.Author}}
		a.privates[c.ID] = c
	}
	c.LastMessageID = flexID(m.ID)
	chat := a.privateChat(c)
	chat.Unread, chat.Mentions, chat.MutedUntil = nil, nil, nil
	return true, []dcChatOut{chat}
}

// ---- Gateway events ----

func (a *dcAccount) dispatch(ctx context.Context, event string, raw json.RawMessage) {
	if a.stopped.Load() {
		return
	}
	switch event {
	case "READY":
		a.ready(ctx, raw)
	case "MESSAGE_CREATE":
		var m dcMessage
		if json.Unmarshal(raw, &m) != nil {
			return
		}
		if ok, chats := a.knownChannel(&m); ok {
			a.storeMessages(ctx, "messages", []dcMessage{m}, chats)
		}
	case "MESSAGE_UPDATE":
		var m dcMessage
		if json.Unmarshal(raw, &m) != nil || m.EditedTimestamp == nil {
			return
		}
		if _, ok := rawField(raw, "content"); !ok {
			return
		}
		a.mu.Lock()
		text := a.render(&m)
		a.mu.Unlock()
		edit := chatMessage{ID: m.ID + "-edit", Chat: m.ChannelID, Sender: m.Author.ID, FromMe: m.Author.ID == a.me.ID, At: time.Now().UnixMilli(), Kind: "edit", Text: text, Target: m.ID}
		if edit.Text != "" {
			a.ingest(ctx, map[string]any{"kind": "messages", "messages": []chatMessage{edit}})
		}
	case "MESSAGE_DELETE", "MESSAGE_DELETE_BULK":
		var d struct {
			ID        string   `json:"id"`
			IDs       []string `json:"ids"`
			ChannelID string   `json:"channel_id"`
		}
		if json.Unmarshal(raw, &d) != nil {
			return
		}
		ids := d.IDs
		if d.ID != "" {
			ids = append(ids, d.ID)
		}
		var out []chatMessage
		for _, id := range ids {
			out = append(out, chatMessage{ID: id + "-delete", Chat: d.ChannelID, At: time.Now().UnixMilli(), Kind: "revoke", Target: id})
		}
		if len(out) > 0 {
			a.ingest(ctx, map[string]any{"kind": "messages", "messages": out})
		}
	case "MESSAGE_REACTION_ADD", "MESSAGE_REACTION_REMOVE", "MESSAGE_REACTION_REMOVE_ALL", "MESSAGE_REACTION_REMOVE_EMOJI":
		var d struct {
			UserID    string  `json:"user_id"`
			ChannelID string  `json:"channel_id"`
			MessageID string  `json:"message_id"`
			Emoji     dcEmoji `json:"emoji"`
		}
		if json.Unmarshal(raw, &d) != nil {
			return
		}
		change := map[string]any{"chat": d.ChannelID, "id": d.MessageID, "emoji": d.Emoji.key(), "me": d.UserID == a.me.ID}
		switch event {
		case "MESSAGE_REACTION_ADD":
			change["delta"] = 1
		case "MESSAGE_REACTION_REMOVE":
			change["delta"] = -1
		case "MESSAGE_REACTION_REMOVE_ALL":
			change["clear"], change["emoji"] = true, ""
		default:
			change["clear"] = true
		}
		a.ingest(ctx, map[string]any{"kind": "reactions", "items": []any{change}})
	case "MESSAGE_ACK":
		var d struct {
			ChannelID string `json:"channel_id"`
			MessageID string `json:"message_id"`
		}
		if json.Unmarshal(raw, &d) == nil && d.ChannelID != "" {
			a.mu.Lock()
			a.reads[d.ChannelID] = readState{last: d.MessageID}
			a.mu.Unlock()
			a.ingest(ctx, map[string]any{"kind": "read", "chat": d.ChannelID})
		}
	case "CHANNEL_CREATE", "CHANNEL_UPDATE", "CHANNEL_DELETE":
		var c dcChannel
		if json.Unmarshal(raw, &c) != nil {
			return
		}
		a.mu.Lock()
		g := a.guilds[c.GuildID]
		switch {
		case g != nil && event == "CHANNEL_DELETE":
			delete(g.channels, c.ID)
		case g != nil:
			g.channels[c.ID] = &c
		case event != "CHANNEL_DELETE":
			a.privates[c.ID] = &c
		}
		a.mu.Unlock()
		if g != nil {
			a.syncGuild(ctx, g.id)
		} else if event != "CHANNEL_DELETE" {
			a.syncPrivates(ctx, []string{c.ID})
		}
	case "GUILD_CREATE", "GUILD_UPDATE":
		var g dcGuild
		if json.Unmarshal(raw, &g) != nil || g.Unavailable {
			return
		}
		a.mu.Lock()
		old := a.guilds[g.ID]
		s := newGuildState(g, len(a.guilds))
		if old != nil {
			s.order, s.myRoles, s.mutedUntil = old.order, old.myRoles, old.mutedUntil
			if event == "GUILD_UPDATE" {
				s.channels = old.channels
			}
		}
		for _, m := range g.Members {
			if m.id() == a.me.ID {
				s.myRoles = m.Roles
			}
		}
		a.guilds[g.ID] = s
		a.mu.Unlock()
		a.ingest(ctx, map[string]any{"kind": "servers", "servers": a.serversOut()})
		a.syncGuild(ctx, g.ID)
	case "GUILD_DELETE":
		var g dcGuild
		if json.Unmarshal(raw, &g) != nil || g.Unavailable {
			return
		}
		a.mu.Lock()
		delete(a.guilds, g.ID)
		a.mu.Unlock()
		a.ingest(ctx, map[string]any{"kind": "servers", "servers": a.serversOut(), "complete": true})
	case "GUILD_ROLE_CREATE", "GUILD_ROLE_UPDATE", "GUILD_ROLE_DELETE":
		var d struct {
			GuildID string `json:"guild_id"`
			Role    dcRole `json:"role"`
			RoleID  string `json:"role_id"`
		}
		if json.Unmarshal(raw, &d) != nil {
			return
		}
		a.mu.Lock()
		g := a.guilds[d.GuildID]
		if g != nil {
			if event == "GUILD_ROLE_DELETE" {
				delete(g.roles, d.RoleID)
			} else {
				g.roles[d.Role.ID] = d.Role
			}
		}
		a.mu.Unlock()
		if g != nil {
			a.syncGuild(ctx, d.GuildID)
		}
	case "GUILD_MEMBER_UPDATE":
		var d struct {
			GuildID string   `json:"guild_id"`
			User    dcUser   `json:"user"`
			Roles   []string `json:"roles"`
		}
		if json.Unmarshal(raw, &d) != nil || d.User.ID != a.me.ID {
			return
		}
		a.mu.Lock()
		g := a.guilds[d.GuildID]
		if g != nil {
			g.myRoles = d.Roles
		}
		a.mu.Unlock()
		if g != nil {
			a.syncGuild(ctx, d.GuildID)
		}
	case "USER_GUILD_SETTINGS_UPDATE":
		var s guildSettings
		if json.Unmarshal(raw, &s) != nil {
			return
		}
		a.mu.Lock()
		a.applySettings([]guildSettings{s})
		a.mu.Unlock()
		if s.GuildID != nil && *s.GuildID != "" {
			a.ingest(ctx, map[string]any{"kind": "servers", "servers": a.serversOut()})
			a.syncGuild(ctx, *s.GuildID)
		} else {
			a.syncPrivates(ctx, nil)
		}
	case "RELATIONSHIP_ADD", "RELATIONSHIP_REMOVE":
		a.mu.Lock()
		if event == "RELATIONSHIP_ADD" {
			a.addRelationship(raw)
		} else {
			var rel struct {
				ID string `json:"id"`
			}
			if json.Unmarshal(raw, &rel) == nil {
				delete(a.friends, rel.ID)
			}
		}
		a.mu.Unlock()
	}
}
