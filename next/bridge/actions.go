package main

import (
	"context"
	"encoding/json"
	"strings"
	"time"

	"go.mau.fi/whatsmeow/appstate"
	"go.mau.fi/whatsmeow/proto/waCommon"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/types"
	"google.golang.org/protobuf/proto"
)

// targetParams names a message: its chat, ID and who sent it (the account's own JID for its messages).
type targetParams struct {
	Account string `json:"account"`
	Chat    string `json:"chat"`
	ID      string `json:"id"`
	Sender  string `json:"sender"`
	Text    string `json:"text"`
	Emoji   string `json:"emoji"`
}

func (b *bridge) target(raw json.RawMessage) (*account, targetParams, types.JID, types.JID, error) {
	p, err := decode[targetParams](raw)
	if err != nil {
		return nil, p, types.EmptyJID, types.EmptyJID, err
	}
	a, err := b.running(p.Account)
	if err != nil {
		return nil, p, types.EmptyJID, types.EmptyJID, err
	}
	chat, err := types.ParseJID(p.Chat)
	if err != nil || p.ID == "" {
		return nil, p, types.EmptyJID, types.EmptyJID, fail("invalidInput", "Unknown message.")
	}
	sender := types.EmptyJID
	if p.Sender != "" {
		if sender, err = types.ParseJID(p.Sender); err != nil {
			return nil, p, types.EmptyJID, types.EmptyJID, fail("invalidInput", "Unknown sender.")
		}
	}
	return a, p, chat, sender, nil
}

func (a *account) sendNow(ctx context.Context, chat types.JID, msg *waE2E.Message) (map[string]any, error) {
	resp, err := a.client.SendMessage(ctx, chat, msg)
	if err != nil {
		return nil, fail("sendFailed", "WhatsApp did not accept the change.")
	}
	return map[string]any{"id": resp.ID, "at": resp.Timestamp.UnixMilli()}, nil
}

// react sets (or with an empty emoji, removes) the account's reaction to a message.
func (b *bridge) react(ctx context.Context, raw json.RawMessage) (any, error) {
	a, p, chat, sender, err := b.target(raw)
	if err != nil {
		return nil, err
	}
	if len(p.Emoji) > 32 {
		return nil, fail("invalidInput", "Choose one emoji.")
	}
	return a.sendNow(ctx, chat, a.client.BuildReaction(chat, sender, p.ID, p.Emoji))
}

// edit replaces the text of one of the account's own messages.
func (b *bridge) edit(ctx context.Context, raw json.RawMessage) (any, error) {
	a, p, chat, _, err := b.target(raw)
	if err != nil {
		return nil, err
	}
	if strings.TrimSpace(p.Text) == "" || len(p.Text) > 65536 {
		return nil, fail("invalidInput", "Write the new text first.")
	}
	return a.sendNow(ctx, chat, a.client.BuildEdit(chat, p.ID, &waE2E.Message{Conversation: proto.String(p.Text)}))
}

// revoke deletes a message for everyone.
func (b *bridge) revoke(ctx context.Context, raw json.RawMessage) (any, error) {
	a, p, chat, sender, err := b.target(raw)
	if err != nil {
		return nil, err
	}
	return a.sendNow(ctx, chat, a.client.BuildRevoke(chat, sender, p.ID))
}

type chatSettingParams struct {
	Account    string `json:"account"`
	Chat       string `json:"chat"`
	Pinned     *bool  `json:"pinned"`
	Archived   *bool  `json:"archived"`
	MutedUntil *int64 `json:"mutedUntil"` // Unix ms; -1 forever, 0 unmute
	LastAt     int64  `json:"lastAt"`
	LastID     string `json:"lastId"`
	LastSender string `json:"lastSender"`
	LastFromMe bool   `json:"lastFromMe"`
}

// chatSetting mutes, pins or archives a chat through WhatsApp's app state, so the phone follows.
func (b *bridge) chatSetting(ctx context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[chatSettingParams](raw)
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
	var patch appstate.PatchInfo
	switch {
	case p.Pinned != nil:
		patch = appstate.BuildPin(chat, *p.Pinned)
	case p.MutedUntil != nil:
		if *p.MutedUntil == 0 {
			patch = appstate.BuildMute(chat, false, 0)
		} else if *p.MutedUntil < 0 {
			patch = appstate.BuildMute(chat, true, 0)
		} else {
			patch = appstate.BuildMuteAbs(chat, true, p.MutedUntil)
		}
	case p.Archived != nil:
		var key *waCommon.MessageKey
		if p.LastID != "" {
			sender := types.EmptyJID
			if p.LastSender != "" {
				sender, _ = types.ParseJID(p.LastSender)
			}
			if p.LastFromMe {
				sender = a.client.Store.GetJID()
			}
			key = a.client.BuildMessageKey(chat, sender, p.LastID)
		}
		patch = appstate.BuildArchive(chat, *p.Archived, time.UnixMilli(p.LastAt), key)
	default:
		return nil, fail("invalidInput", "Nothing to change.")
	}
	if err = a.client.SendAppState(ctx, patch); err != nil {
		return nil, fail("settingFailed", "WhatsApp did not accept the change.")
	}
	return map[string]any{}, nil
}

type person struct {
	JID   string `json:"jid"`
	Name  string `json:"name"`
	Admin bool   `json:"admin,omitempty"`
}

// groupInfo reads a group's subject, description and members.
func (b *bridge) groupInfo(ctx context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[targetParams](raw)
	if err != nil {
		return nil, err
	}
	a, err := b.running(p.Account)
	if err != nil {
		return nil, err
	}
	chat, err := types.ParseJID(p.Chat)
	if err != nil || chat.Server != types.GroupServer {
		return nil, fail("invalidInput", "Unknown group.")
	}
	info, err := a.client.GetGroupInfo(ctx, chat)
	if err != nil {
		return nil, fail("infoFailed", "WhatsApp did not send the group's details.")
	}
	members := make([]person, 0, len(info.Participants))
	for _, member := range info.Participants {
		jid := a.normalize(ctx, member.JID, member.PhoneNumber)
		name := a.personName(ctx, jid, member.DisplayName)
		members = append(members, person{JID: jid.String(), Name: name, Admin: member.IsAdmin || member.IsSuperAdmin})
	}
	return map[string]any{"name": info.Name, "topic": info.Topic, "members": members, "created": info.GroupCreated.UnixMilli()}, nil
}

type historyParams struct {
	Account string `json:"account"`
	Chat    string `json:"chat"`
	ID      string `json:"id"`
	FromMe  bool   `json:"fromMe"`
	At      int64  `json:"at"`
}

// requestHistory asks the phone for up to 50 messages before the oldest one Dar has; they arrive as history.
func (b *bridge) requestHistory(ctx context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[historyParams](raw)
	if err != nil {
		return nil, err
	}
	a, err := b.running(p.Account)
	if err != nil {
		return nil, err
	}
	chat, err := types.ParseJID(p.Chat)
	if err != nil || p.ID == "" {
		return nil, fail("invalidInput", "Unknown chat.")
	}
	info := &types.MessageInfo{MessageSource: types.MessageSource{Chat: chat, IsFromMe: p.FromMe}, ID: p.ID, Timestamp: time.UnixMilli(p.At)}
	if _, err = a.client.SendPeerMessage(ctx, a.client.BuildHistorySyncRequest(info, 50)); err != nil {
		return nil, fail("historyFailed", "Your phone could not be asked for earlier messages.")
	}
	return map[string]any{}, nil
}
