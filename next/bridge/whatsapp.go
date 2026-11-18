package main

import (
	"context"
	"encoding/json"
	"errors"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/proto/waHistorySync"
	"go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/types/events"
	waLog "go.mau.fi/whatsmeow/util/log"
	"google.golang.org/protobuf/proto"
)

// bridge owns every account's WhatsApp connection.
type bridge struct {
	peer    *peer
	backend backend
	log     waLog.Logger

	mu       sync.Mutex
	accounts map[string]*account
}

type account struct {
	id        string
	bridge    *bridge
	kv        *kv
	container *container
	client    *whatsmeow.Client
	cancel    context.CancelFunc
	linking   atomic.Bool
	stopped   atomic.Bool
}

// Ask the phone for as much history as it will send, and say what Dar is in Linked devices.
func init() {
	store.SetOSInfo("Dar", [3]uint32{0, 1, 0})
	store.DeviceProps.RequireFullSync = proto.Bool(true)
	store.DeviceProps.HistorySyncConfig.StorageQuotaMb = proto.Uint32(102400)
	store.DeviceProps.HistorySyncConfig.FullSyncDaysLimit = nil
	store.DeviceProps.HistorySyncConfig.FullSyncSizeMbLimit = nil
	store.DeviceProps.HistorySyncConfig.InitialSyncMaxMessagesPerChat = nil
}

func newBridge(p *peer, b backend, log waLog.Logger) *bridge {
	return &bridge{peer: p, backend: b, log: log, accounts: map[string]*account{}}
}

type accountParams struct {
	Account string `json:"account"`
}

func decode[T any](raw json.RawMessage) (T, error) {
	var value T
	if err := json.Unmarshal(raw, &value); err != nil {
		return value, fail("invalidInput", "The request could not be read.")
	}
	return value, nil
}

func validAccount(id string) bool {
	if len(id) < 8 || len(id) > 64 {
		return false
	}
	for _, r := range id {
		if !(r >= 'a' && r <= 'z' || r >= '0' && r <= '9' || r == '-') {
			return false
		}
	}
	return true
}

// stop disconnects an account and forgets it.
func (b *bridge) stop(id string) {
	b.mu.Lock()
	a := b.accounts[id]
	delete(b.accounts, id)
	b.mu.Unlock()
	if a != nil {
		a.stopped.Store(true)
		if a.cancel != nil {
			a.cancel()
		}
		if a.client != nil {
			a.client.Disconnect()
		}
	}
}

func (b *bridge) newAccount(id string) *account {
	s := newKV(id, b.backend)
	log := b.log.Sub(id[:8])
	return &account{id: id, bridge: b, kv: s, container: newContainer(s, log)}
}

func (a *account) newClient(device *store.Device) {
	client := whatsmeow.NewClient(device, a.bridge.log.Sub(a.id[:8]).Sub("Client"))
	client.EmitAppStateEventsOnFullSync = true
	client.AutomaticMessageRerequestFromPhone = true
	client.AddEventHandlerWithSuccessStatus(a.handle)
	a.client = client
}

func (a *account) emit(event string, data any) { a.bridge.peer.emit(a.id, event, data) }

// ingest hands chats and messages to the runtime and waits until they are stored.
func (a *account) ingest(ctx context.Context, data map[string]any) bool {
	data["account"] = a.id
	ctx, cancel := context.WithTimeout(ctx, 2*time.Minute)
	defer cancel()
	if err := a.bridge.peer.call(ctx, "ingest", data, nil); err != nil {
		a.bridge.log.Warnf("Storing %s for %s failed: %v", data["kind"], a.id[:8], err)
		return false
	}
	return true
}

type linkParams struct {
	Account string `json:"account"`
	Phone   string `json:"phone"`
}

// link starts pairing a new device. Codes and the result arrive as events.
func (b *bridge) link(ctx context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[linkParams](raw)
	if err != nil {
		return nil, err
	}
	if !validAccount(p.Account) {
		return nil, fail("invalidInput", "Unknown account.")
	}
	b.stop(p.Account)
	a := b.newAccount(p.Account)
	a.linking.Store(true)
	a.newClient(a.container.newDevice())
	linkCtx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	a.cancel = cancel
	codes, err := a.client.GetQRChannel(linkCtx)
	if err != nil {
		cancel()
		return nil, err
	}
	b.mu.Lock()
	b.accounts[p.Account] = a
	b.mu.Unlock()
	if err = a.client.Connect(); err != nil {
		b.stop(p.Account)
		return nil, fail("connectFailed", "Dar could not reach WhatsApp. Check the server's internet connection.")
	}
	go a.watchLink(linkCtx, codes, strings.TrimSpace(p.Phone))
	return map[string]any{}, nil
}

func (a *account) watchLink(ctx context.Context, codes <-chan whatsmeow.QRChannelItem, phone string) {
	paired := false
	for item := range codes {
		switch item.Event {
		case whatsmeow.QRChannelEventCode:
			if phone == "" {
				a.emit("linkCode", map[string]any{"code": item.Code, "expiresIn": int(item.Timeout.Seconds())})
				continue
			}
			if paired {
				continue
			}
			paired = true
			code, err := a.client.PairPhone(ctx, phone, true, whatsmeow.PairClientChrome, "Chrome (Linux)")
			if err != nil {
				reason := "pairingFailed"
				if errors.Is(err, whatsmeow.ErrPhoneNumberTooShort) || errors.Is(err, whatsmeow.ErrPhoneNumberIsNotInternational) {
					reason = "invalidPhone"
				}
				a.emit("linkFailed", map[string]any{"reason": reason})
				a.bridge.stop(a.id)
				return
			}
			a.emit("pairingCode", map[string]any{"code": code})
		case "success":
			// PairSuccess has already been handled.
		case whatsmeow.QRChannelEventPasskeyRequest:
			a.emit("linkFailed", map[string]any{"reason": "passkey"})
			a.bridge.stop(a.id)
			return
		default:
			reason := "timeout"
			if item.Event == whatsmeow.QRChannelEventError {
				reason = "pairingFailed"
			} else if item == whatsmeow.QRChannelClientOutdated {
				reason = "outdated"
			} else if item == whatsmeow.QRChannelScannedWithoutMultidevice {
				reason = "multidevice"
				continue
			}
			if !a.stopped.Load() {
				a.emit("linkFailed", map[string]any{"reason": reason})
				a.bridge.stop(a.id)
			}
			return
		}
	}
}

// connect opens a linked account's stored device and connects it.
func (b *bridge) connect(ctx context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[accountParams](raw)
	if err != nil {
		return nil, err
	}
	if !validAccount(p.Account) {
		return nil, fail("invalidInput", "Unknown account.")
	}
	b.stop(p.Account)
	a := b.newAccount(p.Account)
	if err = a.kv.open(ctx); err != nil {
		return nil, err
	}
	device, err := a.container.loadDevice(ctx)
	if err != nil {
		return nil, err
	} else if device == nil {
		return nil, fail("notLinked", "This account is not linked.")
	}
	a.newClient(device)
	b.mu.Lock()
	b.accounts[p.Account] = a
	b.mu.Unlock()
	if err = a.client.Connect(); err != nil {
		// whatsmeow keeps retrying on its own once connected; a first failure is reported and retried by the runtime.
		b.stop(p.Account)
		return nil, fail("connectFailed", "Dar could not reach WhatsApp.")
	}
	return map[string]any{"jid": device.ID.String()}, nil
}

func (b *bridge) disconnect(_ context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[accountParams](raw)
	if err != nil {
		return nil, err
	}
	b.stop(p.Account)
	return map[string]any{}, nil
}

func (b *bridge) running(id string) (*account, error) {
	b.mu.Lock()
	a := b.accounts[id]
	b.mu.Unlock()
	if a == nil || a.client == nil || a.linking.Load() {
		return nil, fail("notConnected", "WhatsApp is not connected.")
	}
	return a, nil
}

// logout unlinks Dar on WhatsApp's side; the runtime then deletes the account's keys.
func (b *bridge) logout(ctx context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[accountParams](raw)
	if err != nil {
		return nil, err
	}
	a, err := b.running(p.Account)
	if err != nil {
		return nil, err
	}
	if !a.client.IsLoggedIn() {
		deadline := time.Now().Add(15 * time.Second)
		for !a.client.IsLoggedIn() && time.Now().Before(deadline) {
			time.Sleep(200 * time.Millisecond)
		}
	}
	err = a.client.Logout(ctx)
	b.stop(p.Account)
	if err != nil {
		return nil, fail("logoutFailed", "WhatsApp did not confirm the unlink. Remove Dar under Linked devices on your phone.")
	}
	return map[string]any{}, nil
}

type sendParams struct {
	Account      string `json:"account"`
	Chat         string `json:"chat"`
	Text         string `json:"text"`
	ID           string `json:"id"`
	QuotedID     string `json:"quotedId"`
	QuotedSender string `json:"quotedSender"`
	QuotedText   string `json:"quotedText"`
}

func (b *bridge) send(ctx context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[sendParams](raw)
	if err != nil {
		return nil, err
	}
	a, err := b.running(p.Account)
	if err != nil {
		return nil, err
	}
	chat, err := types.ParseJID(p.Chat)
	if err != nil || strings.TrimSpace(p.Text) == "" || len(p.Text) > 65536 {
		return nil, fail("invalidInput", "The message could not be sent.")
	}
	msg := &waE2E.Message{Conversation: proto.String(p.Text)}
	if p.QuotedID != "" {
		ci := &waE2E.ContextInfo{StanzaID: proto.String(p.QuotedID), QuotedMessage: &waE2E.Message{Conversation: proto.String(p.QuotedText)}}
		if p.QuotedSender != "" {
			ci.Participant = proto.String(p.QuotedSender)
		}
		msg = &waE2E.Message{ExtendedTextMessage: &waE2E.ExtendedTextMessage{Text: proto.String(p.Text), ContextInfo: ci}}
	}
	extra := whatsmeow.SendRequestExtra{}
	if p.ID != "" {
		extra.ID = p.ID
	}
	resp, err := a.client.SendMessage(ctx, chat, msg, extra)
	if err != nil {
		return nil, fail("sendFailed", "WhatsApp did not accept the message.")
	}
	return map[string]any{"id": resp.ID, "at": resp.Timestamp.UnixMilli()}, nil
}

type readParams struct {
	Account string `json:"account"`
	Chat    string `json:"chat"`
	Items   []struct {
		ID     string `json:"id"`
		Sender string `json:"sender"`
	} `json:"items"`
}

// markRead sends read receipts, grouped by sender as WhatsApp expects.
func (b *bridge) markRead(ctx context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[readParams](raw)
	if err != nil {
		return nil, err
	}
	a, err := b.running(p.Account)
	if err != nil {
		return nil, err
	}
	chat, err := types.ParseJID(p.Chat)
	if err != nil {
		return nil, fail("invalidInput", "Unknown chat.")
	}
	bySender := map[string][]types.MessageID{}
	for _, item := range p.Items {
		bySender[item.Sender] = append(bySender[item.Sender], item.ID)
	}
	for senderID, ids := range bySender {
		sender := types.EmptyJID
		if chat.Server == types.GroupServer {
			if sender, err = types.ParseJID(senderID); err != nil {
				continue
			}
		}
		if err = a.client.MarkRead(ctx, ids, time.Now(), chat, sender); err != nil {
			return nil, fail("readFailed", "Read receipts could not be sent.")
		}
	}
	return map[string]any{}, nil
}

// handle turns whatsmeow events into runtime events. Returning false for a
// message leaves it unacknowledged, so WhatsApp delivers it again later.
func (a *account) handle(raw any) bool {
	ctx := context.Background()
	switch evt := raw.(type) {
	case *events.PairSuccess:
		// The link context stays open: cancelling it before whatsmeow closes its QR channel would disconnect the new device.
		a.linking.Store(false)
		a.emit("paired", map[string]any{"jid": evt.ID.ToNonAD().String(), "platform": evt.Platform, "businessName": evt.BusinessName})
	case *events.Connected:
		a.emit("connected", map[string]any{"pushName": a.client.Store.PushName})
		go a.syncGroupNames()
	case *events.Disconnected:
		a.emit("disconnected", map[string]any{})
	case *events.KeepAliveTimeout:
		a.emit("disconnected", map[string]any{"reason": "keepalive"})
	case *events.LoggedOut:
		a.emit("loggedOut", map[string]any{"reason": evt.Reason.String()})
		go a.bridge.stop(a.id)
	case *events.StreamReplaced:
		a.emit("replaced", map[string]any{})
		go a.bridge.stop(a.id)
	case *events.TemporaryBan:
		a.emit("banned", map[string]any{"reason": evt.String(), "expiresIn": int(evt.Expire.Seconds())})
	case *events.ClientOutdated:
		a.emit("outdated", map[string]any{})
	case *events.ConnectFailure:
		a.emit("connectFailure", map[string]any{"reason": evt.Reason.String()})
	case *events.Message:
		msg, ok := a.convert(ctx, evt, "")
		if !ok {
			return true
		}
		chat := chatInfo{ID: msg.Chat, Kind: kindOfChat(evt.Info.Chat)}
		if chatJID, err := types.ParseJID(msg.Chat); err == nil && chat.Kind == "direct" && !msg.FromMe {
			chat.Name = a.personName(ctx, chatJID, evt.Info.PushName)
		}
		return a.ingest(ctx, map[string]any{"kind": "messages", "chats": []chatInfo{chat}, "messages": []chatMessage{msg}})
	case *events.HistorySync:
		return a.history(ctx, evt.Data)
	case *events.Receipt:
		status := ""
		switch evt.Type {
		case types.ReceiptTypeDelivered:
			status = "delivered"
		case types.ReceiptTypeRead, types.ReceiptTypePlayed:
			status = "read"
		case types.ReceiptTypeReadSelf:
			status = "readSelf"
		}
		if status == "" || skipChat(evt.Chat) {
			return true
		}
		chat := a.normalize(ctx, evt.Chat, types.EmptyJID)
		return a.ingest(ctx, map[string]any{"kind": "receipts", "chat": chat.String(), "ids": evt.MessageIDs, "status": status, "fromMe": evt.IsFromMe})
	case *events.Archive:
		archived := evt.Action.GetArchived()
		return a.chatState(ctx, evt.JID, chatInfo{Archived: &archived})
	case *events.Pin:
		pinned := evt.Action.GetPinned()
		return a.chatState(ctx, evt.JID, chatInfo{Pinned: &pinned})
	case *events.Mute:
		var until int64
		if evt.Action.GetMuted() {
			if until = evt.Action.GetMuteEndTimestamp(); until <= 0 {
				until = -1
			}
		}
		return a.chatState(ctx, evt.JID, chatInfo{MutedUntil: &until})
	case *events.MarkChatAsRead:
		if evt.Action.GetRead() {
			zero := 0
			return a.chatState(ctx, evt.JID, chatInfo{Unread: &zero})
		}
	case *events.Contact:
		if evt.Action != nil && evt.JID.Server == types.DefaultUserServer {
			return a.chatState(ctx, evt.JID, chatInfo{Name: a.personName(ctx, evt.JID, "")})
		}
	case *events.PushName:
		return a.chatState(ctx, evt.JID, chatInfo{Name: a.personName(ctx, a.normalize(ctx, evt.JID, evt.JIDAlt), evt.NewPushName)})
	case *events.GroupInfo:
		if evt.Name != nil {
			return a.chatState(ctx, evt.JID, chatInfo{Name: evt.Name.Name})
		}
	}
	return true
}

func kindOfChat(jid types.JID) string {
	if jid.Server == types.GroupServer {
		return "group"
	}
	return "direct"
}

// chatState updates a chat the runtime already knows; unknown chats are ignored by the runtime.
func (a *account) chatState(ctx context.Context, jid types.JID, change chatInfo) bool {
	if skipChat(jid) {
		return true
	}
	jid = a.normalize(ctx, jid, types.EmptyJID)
	change.ID, change.Kind = jid.String(), kindOfChat(jid)
	return a.ingest(ctx, map[string]any{"kind": "chatState", "chats": []chatInfo{change}})
}

// syncGroupNames reads the names of every joined group once connected.
func (a *account) syncGroupNames() {
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	groups, err := a.client.GetJoinedGroups(ctx)
	if err != nil {
		return
	}
	var chats []chatInfo
	for _, group := range groups {
		chats = append(chats, chatInfo{ID: group.JID.String(), Kind: "group", Name: group.Name})
	}
	if len(chats) > 0 {
		a.ingest(ctx, map[string]any{"kind": "chatState", "chats": chats})
	}
}

const historyBatch = 400

// history stores one history sync blob in batches, oldest messages first.
func (a *account) history(ctx context.Context, data *waHistorySync.HistorySync) bool {
	var chats []chatInfo
	var messages []chatMessage
	flush := func(final bool) bool {
		if len(chats) == 0 && len(messages) == 0 && !final {
			return true
		}
		payload := map[string]any{"kind": "history", "chats": chats, "messages": messages, "syncType": data.GetSyncType().String(), "final": final}
		if final {
			payload["progress"] = data.GetProgress()
		}
		ok := a.ingest(ctx, payload)
		chats, messages = nil, nil
		return ok
	}
	for _, conv := range data.GetConversations() {
		jid, err := types.ParseJID(conv.GetID())
		if err != nil || skipChat(jid) {
			continue
		}
		var alt types.JID
		if pn, err := types.ParseJID(conv.GetPnJID()); err == nil {
			alt = pn
		}
		jid = a.normalize(ctx, jid, alt)
		unread := int(conv.GetUnreadCount())
		if conv.GetMarkedAsUnread() && unread == 0 {
			unread = 1
		}
		archived, pinned := conv.GetArchived(), conv.GetPinned() > 0
		muted := int64(conv.GetMuteEndTime()) * 1000
		chat := chatInfo{ID: jid.String(), Kind: kindOfChat(jid), Name: conv.GetName(), Unread: &unread, Archived: &archived, Pinned: &pinned, MutedUntil: &muted, At: int64(conv.GetConversationTimestamp()) * 1000}
		if chat.Kind == "direct" && chat.Name == "" {
			chat.Name = a.personName(ctx, jid, "")
		}
		chats = append(chats, chat)
		original, _ := types.ParseJID(conv.GetID())
		for _, item := range conv.GetMessages() {
			web := item.GetMessage()
			evt, err := a.client.ParseWebMessage(original, web)
			if err != nil {
				continue
			}
			status := ""
			if evt.Info.IsFromMe {
				status = historyStatus(web.GetStatus())
			}
			if msg, ok := a.convert(ctx, evt, status); ok {
				messages = append(messages, msg)
			}
			if len(messages) >= historyBatch && !flush(false) {
				return false
			}
		}
	}
	return flush(true)
}
