package main

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/proto/waMmsRetry"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"
)

// mediaInfo is what the runtime stores about an attachment; the keys to fetch it stay here.
type mediaInfo struct {
	Mime    string `json:"mime"`
	Size    uint64 `json:"size"`
	Seconds uint32 `json:"seconds,omitempty"`
	Width   uint32 `json:"width,omitempty"`
	Height  uint32 `json:"height,omitempty"`
}

// storedMedia is an attachment's encrypted-download description, kept in the key store
// (lazy, encrypted by the runtime) under "media/<chat>/<message>".
type storedMedia struct {
	Kind    string `json:"kind"`
	Proto   []byte `json:"proto"`
	Chat    string `json:"chat"` // as WhatsApp addressed the message, for retry receipts
	Sender  string `json:"sender"`
	FromMe  bool   `json:"fromMe"`
	IsGroup bool   `json:"isGroup"`
}

func mediaName(chat, id string) string { return "media/" + chat + "/" + id }

// attachment returns the downloadable part of a message and what to show about it.
func attachment(msg *waE2E.Message) (kind string, part proto.Message, info mediaInfo, ok bool) {
	switch {
	case msg.GetImageMessage() != nil:
		m := msg.GetImageMessage()
		return "image", m, mediaInfo{Mime: cleanMime(m.GetMimetype()), Size: m.GetFileLength(), Width: m.GetWidth(), Height: m.GetHeight()}, true
	case msg.GetStickerMessage() != nil:
		m := msg.GetStickerMessage()
		return "sticker", m, mediaInfo{Mime: cleanMime(m.GetMimetype()), Size: m.GetFileLength(), Width: m.GetWidth(), Height: m.GetHeight()}, true
	case msg.GetVideoMessage() != nil:
		m := msg.GetVideoMessage()
		return "video", m, mediaInfo{Mime: cleanMime(m.GetMimetype()), Size: m.GetFileLength(), Seconds: m.GetSeconds(), Width: m.GetWidth(), Height: m.GetHeight()}, true
	case msg.GetAudioMessage() != nil:
		m := msg.GetAudioMessage()
		return "audio", m, mediaInfo{Mime: cleanMime(m.GetMimetype()), Size: m.GetFileLength(), Seconds: m.GetSeconds()}, true
	case msg.GetDocumentMessage() != nil:
		m := msg.GetDocumentMessage()
		return "document", m, mediaInfo{Mime: cleanMime(m.GetMimetype()), Size: m.GetFileLength()}, true
	}
	return "", nil, mediaInfo{}, false
}

// rememberMedia keeps the download description for a converted message; view-once media is not kept.
func (a *account) rememberMedia(ctx context.Context, evt *events.Message, out *chatMessage) error {
	if evt.IsViewOnce {
		return nil
	}
	kind, part, info, ok := attachment(evt.Message)
	if !ok || info.Size == 0 {
		return nil
	}
	raw, err := proto.Marshal(part)
	if err != nil {
		return err
	}
	out.Media = &info
	return putJSON(ctx, a.kv, mediaName(out.Chat, out.ID), storedMedia{
		Kind: kind, Proto: raw, Chat: evt.Info.Chat.String(), Sender: evt.Info.Sender.String(), FromMe: evt.Info.IsFromMe, IsGroup: evt.Info.IsGroup,
	}, 0)
}

func (m storedMedia) downloadable() (whatsmeow.DownloadableMessage, error) {
	var part interface {
		proto.Message
		whatsmeow.DownloadableMessage
	}
	switch m.Kind {
	case "image":
		part = &waE2E.ImageMessage{}
	case "sticker":
		part = &waE2E.StickerMessage{}
	case "video":
		part = &waE2E.VideoMessage{}
	case "audio":
		part = &waE2E.AudioMessage{}
	case "document":
		part = &waE2E.DocumentMessage{}
	default:
		return nil, errors.New("unknown media kind")
	}
	return part, proto.Unmarshal(m.Proto, part)
}

type downloadParams struct {
	Account string `json:"account"`
	Chat    string `json:"chat"`
	ID      string `json:"id"`
	Path    string `json:"path"`
}

var downloadName = regexp.MustCompile(`^[0-9a-f-]{36}\.part$`)

// download decrypts an attachment into a new file the runtime named in its download directory.
// Media WhatsApp no longer serves is requested from the phone again ("retrying"); a later
// mediaRetried event says whether to try once more.
func (b *bridge) download(ctx context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[downloadParams](raw)
	if err != nil {
		return nil, err
	}
	dir := os.Getenv("DAR_CHAT_DOWNLOADS")
	if dir == "" || filepath.Dir(p.Path) != filepath.Clean(dir) || !downloadName.MatchString(filepath.Base(p.Path)) {
		return nil, fail("invalidInput", "Downloads go to the runtime's download directory.")
	}
	a, err := b.running(p.Account)
	if err != nil {
		return nil, err
	}
	stored, ok, err := getJSON[storedMedia](ctx, a.kv, mediaName(p.Chat, p.ID))
	if err != nil {
		return nil, err
	} else if !ok {
		return nil, fail("noMedia", "This attachment cannot be downloaded.")
	}
	msg, err := stored.downloadable()
	if err != nil {
		return nil, fail("noMedia", "This attachment cannot be downloaded.")
	}
	file, err := os.OpenFile(p.Path, os.O_RDWR|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return nil, err
	}
	err = a.client.DownloadToFile(ctx, msg, file)
	closeErr := file.Close()
	if err == nil {
		err = closeErr
	}
	if err != nil {
		_ = os.Remove(p.Path)
		if errors.Is(err, whatsmeow.ErrMediaDownloadFailedWith404) || errors.Is(err, whatsmeow.ErrMediaDownloadFailedWith410) ||
			errors.Is(err, whatsmeow.ErrMediaDownloadFailedWith403) || errors.Is(err, whatsmeow.ErrNoURLPresent) {
			if a.requestRetry(ctx, p.Chat, p.ID, stored, msg) {
				return nil, fail("retrying", "WhatsApp no longer has this file; your phone was asked to send it again.")
			}
			return nil, fail("expired", "WhatsApp no longer has this file.")
		}
		return nil, fail("downloadFailed", "The attachment could not be downloaded.")
	}
	info, _ := os.Stat(p.Path)
	return map[string]any{"size": info.Size()}, nil
}

// requestRetry asks the phone to upload expired media again.
func (a *account) requestRetry(ctx context.Context, chat, id string, stored storedMedia, msg whatsmeow.DownloadableMessage) bool {
	rawChat, err1 := types.ParseJID(stored.Chat)
	sender, err2 := types.ParseJID(stored.Sender)
	key := msg.GetMediaKey()
	if err1 != nil || err2 != nil || len(key) == 0 {
		return false
	}
	info := &types.MessageInfo{ID: id, MessageSource: types.MessageSource{Chat: rawChat, Sender: sender, IsFromMe: stored.FromMe, IsGroup: stored.IsGroup}}
	return a.client.SendMediaRetryReceipt(ctx, info, key) == nil
}

// mediaRetried stores the new location the phone sent and tells the runtime to download again.
func (a *account) mediaRetried(ctx context.Context, evt *events.MediaRetry) bool {
	chat := a.normalize(ctx, evt.ChatID, types.EmptyJID).String()
	name := mediaName(chat, evt.MessageID)
	stored, ok, err := getJSON[storedMedia](ctx, a.kv, name)
	if err != nil {
		return false
	} else if !ok {
		return true
	}
	msg, err := stored.downloadable()
	if err != nil {
		return true
	}
	result := map[string]any{"chat": chat, "id": evt.MessageID, "ok": false}
	defer func() { a.emit("mediaRetried", result) }()
	if evt.Error != nil {
		return true
	}
	notification, err := whatsmeow.DecryptMediaRetryNotification(evt, msg.GetMediaKey())
	if err != nil || notification.GetResult() != waMmsRetry.MediaRetryNotification_SUCCESS || notification.GetDirectPath() == "" {
		return true
	}
	// Each downloadable message type has a DirectPath field; set it through reflection-free type switch.
	switch part := msg.(type) {
	case *waE2E.ImageMessage:
		part.DirectPath = proto.String(notification.GetDirectPath())
	case *waE2E.StickerMessage:
		part.DirectPath = proto.String(notification.GetDirectPath())
	case *waE2E.VideoMessage:
		part.DirectPath = proto.String(notification.GetDirectPath())
	case *waE2E.AudioMessage:
		part.DirectPath = proto.String(notification.GetDirectPath())
	case *waE2E.DocumentMessage:
		part.DirectPath = proto.String(notification.GetDirectPath())
	}
	if stored.Proto, err = proto.Marshal(msg.(proto.Message)); err != nil {
		return true
	}
	if putJSON(ctx, a.kv, name, stored, 0) != nil {
		return false
	}
	result["ok"] = true
	return true
}

// cleanMime drops parameters such as "; codecs=opus".
func cleanMime(value string) string {
	return strings.ToLower(strings.TrimSpace(strings.SplitN(value, ";", 2)[0]))
}
