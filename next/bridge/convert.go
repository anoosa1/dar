package main

import (
	"context"
	"strings"

	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/proto/waWeb"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/types/events"
)

// chatMessage is what the runtime stores for one WhatsApp message.
type chatMessage struct {
	ID         string `json:"id"`
	Chat       string `json:"chat"`
	Sender     string `json:"sender"`
	SenderName string `json:"senderName,omitempty"`
	FromMe     bool   `json:"fromMe"`
	At         int64  `json:"at"`
	// Kind is text, image, video, audio, voice, document, sticker, location,
	// contact, poll, unsupported, or edit/revoke changing Target.
	Kind         string `json:"kind"`
	Text         string `json:"text,omitempty"`
	FileName     string `json:"fileName,omitempty"`
	Target       string `json:"target,omitempty"`
	QuotedID     string `json:"quotedId,omitempty"`
	QuotedSender string `json:"quotedSender,omitempty"`
	QuotedText   string `json:"quotedText,omitempty"`
	Status       string `json:"status,omitempty"`
	Media        *mediaInfo `json:"media,omitempty"`
}

// chatInfo is what the runtime stores about a chat.
type chatInfo struct {
	ID         string `json:"id"`
	Kind       string `json:"kind"` // direct or group
	Name       string `json:"name,omitempty"`
	Unread     *int   `json:"unread,omitempty"`
	Archived   *bool  `json:"archived,omitempty"`
	Pinned     *bool  `json:"pinned,omitempty"`
	MutedUntil *int64 `json:"mutedUntil,omitempty"` // Unix ms; -1 forever, 0 not muted
	At         int64  `json:"at,omitempty"`
}

// skipChat leaves out status updates, channels and broadcast lists.
func skipChat(jid types.JID) bool {
	return jid.IsEmpty() || jid.Server == types.BroadcastServer || jid.Server == types.NewsletterServer
}

// normalize gives one chat or person a single ID: the phone number form when known.
func (a *account) normalize(ctx context.Context, jid, alt types.JID) types.JID {
	jid = jid.ToNonAD()
	if jid.Server != types.HiddenUserServer {
		return jid
	}
	if alt = alt.ToNonAD(); alt.Server == types.DefaultUserServer {
		return alt
	}
	if pn, _ := a.container.store.GetPNForLID(ctx, jid); !pn.IsEmpty() {
		return pn.ToNonAD()
	}
	return jid
}

// personName picks the best known name for a user.
func (a *account) personName(ctx context.Context, jid types.JID, pushName string) string {
	contact, _ := a.container.store.GetContact(ctx, jid)
	for _, name := range []string{contact.FullName, contact.FirstName, pushName, contact.PushName, contact.BusinessName} {
		if strings.TrimSpace(name) != "" {
			return name
		}
	}
	if jid.Server == types.DefaultUserServer {
		return "+" + jid.User
	}
	return contact.RedactedPhone
}

func textOf(msg *waE2E.Message) string {
	switch {
	case msg.GetConversation() != "":
		return msg.GetConversation()
	case msg.GetExtendedTextMessage() != nil:
		return msg.GetExtendedTextMessage().GetText()
	case msg.GetImageMessage() != nil:
		return msg.GetImageMessage().GetCaption()
	case msg.GetVideoMessage() != nil:
		return msg.GetVideoMessage().GetCaption()
	case msg.GetDocumentMessage() != nil:
		return msg.GetDocumentMessage().GetCaption()
	}
	return ""
}

func contextOf(msg *waE2E.Message) *waE2E.ContextInfo {
	for _, ci := range []*waE2E.ContextInfo{
		msg.GetExtendedTextMessage().GetContextInfo(), msg.GetImageMessage().GetContextInfo(), msg.GetVideoMessage().GetContextInfo(),
		msg.GetAudioMessage().GetContextInfo(), msg.GetDocumentMessage().GetContextInfo(), msg.GetStickerMessage().GetContextInfo(),
	} {
		if ci != nil {
			return ci
		}
	}
	return nil
}

// kindOf names the content; ok is false for messages that are not shown (keys, receipts, reactions for now).
func kindOf(msg *waE2E.Message) (kind, text, fileName string, ok bool) {
	text = textOf(msg)
	switch {
	case msg.GetConversation() != "" || msg.GetExtendedTextMessage() != nil:
		return "text", text, "", true
	case msg.GetImageMessage() != nil:
		return "image", text, "", true
	case msg.GetVideoMessage() != nil:
		return "video", text, "", true
	case msg.GetAudioMessage() != nil:
		if msg.GetAudioMessage().GetPTT() {
			return "voice", "", "", true
		}
		return "audio", "", "", true
	case msg.GetDocumentMessage() != nil:
		return "document", text, msg.GetDocumentMessage().GetFileName(), true
	case msg.GetStickerMessage() != nil:
		return "sticker", "", "", true
	case msg.GetLocationMessage() != nil || msg.GetLiveLocationMessage() != nil:
		return "location", msg.GetLocationMessage().GetName(), "", true
	case msg.GetContactMessage() != nil:
		return "contact", msg.GetContactMessage().GetDisplayName(), "", true
	case msg.GetContactsArrayMessage() != nil:
		return "contact", msg.GetContactsArrayMessage().GetDisplayName(), "", true
	case msg.GetPollCreationMessage() != nil || msg.GetPollCreationMessageV2() != nil || msg.GetPollCreationMessageV3() != nil:
		poll := msg.GetPollCreationMessage()
		if poll == nil {
			poll = msg.GetPollCreationMessageV2()
		}
		if poll == nil {
			poll = msg.GetPollCreationMessageV3()
		}
		return "poll", poll.GetName(), "", true
	case msg.GetEventMessage() != nil || msg.GetGroupInviteMessage() != nil || msg.GetOrderMessage() != nil ||
		msg.GetTemplateMessage() != nil || msg.GetButtonsMessage() != nil || msg.GetListMessage() != nil || msg.GetInteractiveMessage() != nil:
		return "unsupported", "", "", true
	}
	return "", "", "", false
}

// convert turns a decrypted message into what the runtime stores. ok is false for messages that are not shown.
func (a *account) convert(ctx context.Context, evt *events.Message, status string) (chatMessage, bool) {
	info := evt.Info
	if skipChat(info.Chat) || evt.Message == nil {
		return chatMessage{}, false
	}
	chatAlt := info.RecipientAlt
	if !info.IsFromMe {
		chatAlt = info.SenderAlt
	}
	chat := a.normalize(ctx, info.Chat, chatAlt)
	sender := a.normalize(ctx, info.Sender, info.SenderAlt)
	out := chatMessage{ID: info.ID, Chat: chat.String(), Sender: sender.String(), FromMe: info.IsFromMe, At: info.Timestamp.UnixMilli(), Status: status}
	if info.IsGroup && !info.IsFromMe {
		out.SenderName = a.personName(ctx, sender, info.PushName)
	}
	msg := evt.Message
	if protocol := msg.GetProtocolMessage(); protocol != nil {
		switch protocol.GetType() {
		case waE2E.ProtocolMessage_REVOKE:
			out.Kind, out.Target = "revoke", protocol.GetKey().GetID()
			return out, out.Target != ""
		case waE2E.ProtocolMessage_MESSAGE_EDIT:
			out.Kind, out.Target, out.Text = "edit", protocol.GetKey().GetID(), textOf(protocol.GetEditedMessage())
			return out, out.Target != ""
		}
		return out, false
	}
	kind, text, fileName, ok := kindOf(msg)
	if !ok {
		return out, false
	}
	out.Kind, out.Text, out.FileName = kind, text, fileName
	if ci := contextOf(msg); ci != nil && ci.GetStanzaID() != "" {
		out.QuotedID = ci.GetStanzaID()
		if participant, err := types.ParseJID(ci.GetParticipant()); err == nil {
			quoted := a.normalize(ctx, participant, types.EmptyJID)
			out.QuotedSender = a.personName(ctx, quoted, "")
		}
		if quoted := ci.GetQuotedMessage(); quoted != nil {
			_, out.QuotedText, _, _ = kindOf(quoted)
		}
	}
	return out, true
}

func historyStatus(status waWeb.WebMessageInfo_Status) string {
	switch status {
	case waWeb.WebMessageInfo_READ, waWeb.WebMessageInfo_PLAYED:
		return "read"
	case waWeb.WebMessageInfo_DELIVERY_ACK:
		return "delivered"
	case waWeb.WebMessageInfo_SERVER_ACK:
		return "sent"
	case waWeb.WebMessageInfo_ERROR:
		return "failed"
	}
	return "sent"
}
