package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"mime"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

func mimeByName(name string) string {
	if t := mime.TypeByExtension(strings.ToLower(filepath.Ext(name))); t != "" {
		return strings.SplitN(t, ";", 2)[0]
	}
	return "application/octet-stream"
}

func (b *bridge) discordAccount(id string) *dcAccount {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.discord[id]
}

func (b *bridge) discordRunning(id string) (*dcAccount, error) {
	a := b.discordAccount(id)
	if a == nil || a.linking.Load() || a.api == nil {
		return nil, fail("notConnected", "Discord is not connected.")
	}
	return a, nil
}

// dcLink runs Discord's QR sign-in; the code, the scan and the result arrive as events.
func (b *bridge) dcLink(_ context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[accountParams](raw)
	if err != nil {
		return nil, err
	}
	if !validAccount(p.Account) {
		return nil, fail("invalidInput", "Unknown account.")
	}
	b.stop(p.Account)
	a := b.newDiscordAccount(p.Account)
	a.linking.Store(true)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	a.cancel = cancel
	b.mu.Lock()
	b.discord[p.Account] = a
	b.mu.Unlock()
	go func() {
		defer cancel()
		token, _, err := remoteAuth(ctx, b.discordURLs.remoteAuth, newDiscordAPI(b.discordURLs.api, ""), remoteAuthEvents{
			code: func(url string, left time.Duration) {
				a.emit("linkCode", map[string]any{"code": url, "expiresIn": int(left.Seconds())})
			},
			scanned: func(user remoteAuthUser) { a.emit("linkScanned", map[string]any{"name": user.Username}) },
		})
		if err == nil {
			err = a.finishLink(ctx, token)
		}
		if err != nil {
			reason := "pairingFailed"
			var coded *codedError
			if errors.As(err, &coded) && (coded.code == "timeout" || coded.code == "cancelled" || coded.code == "captcha" || coded.code == "connectFailed") {
				reason = coded.code
			}
			if !a.stopped.Load() {
				a.emit("linkFailed", map[string]any{"reason": reason})
				b.stop(a.id)
			}
		}
	}()
	return map[string]any{}, nil
}

// finishLink checks the token, keeps it in the encrypted key store and connects.
func (a *dcAccount) finishLink(ctx context.Context, token string) error {
	api := newDiscordAPI(a.bridge.discordURLs.api, token)
	var me dcUser
	if err := api.do(ctx, http.MethodGet, "/users/@me", nil, &me); err != nil || me.ID == "" {
		return fail("pairingFailed", "Discord did not accept the sign-in.")
	}
	err := a.kv.transaction(ctx, func(ctx context.Context) error {
		if err := a.kv.put(ctx, keyDiscordToken, []byte(token), 0); err != nil {
			return err
		}
		return putJSON(ctx, a.kv, keyDiscordUser, me, 0)
	})
	if err != nil {
		return err
	}
	a.mu.Lock()
	a.me = me
	a.mu.Unlock()
	a.emit("paired", map[string]any{"jid": me.ID, "name": me.name()})
	if a.stopped.Load() {
		return nil
	}
	a.linking.Store(false)
	a.start(token)
	return nil
}

func (b *bridge) dcConnect(ctx context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[accountParams](raw)
	if err != nil {
		return nil, err
	}
	if !validAccount(p.Account) {
		return nil, fail("invalidInput", "Unknown account.")
	}
	b.stop(p.Account)
	a := b.newDiscordAccount(p.Account)
	if err = a.kv.open(ctx); err != nil {
		return nil, err
	}
	token, ok, err := a.kv.get(ctx, keyDiscordToken)
	if err != nil {
		return nil, err
	} else if !ok || len(token) == 0 {
		return nil, fail("notLinked", "This account is not linked.")
	}
	me, _, _ := getJSON[dcUser](ctx, a.kv, keyDiscordUser)
	a.me = me
	b.mu.Lock()
	b.discord[p.Account] = a
	b.mu.Unlock()
	a.start(string(token))
	return map[string]any{"jid": me.ID}, nil
}

// dcLogout ends the session on Discord's side; the runtime then deletes the token.
func (b *bridge) dcLogout(ctx context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[accountParams](raw)
	if err != nil {
		return nil, err
	}
	a, err := b.discordRunning(p.Account)
	if err != nil {
		return nil, err
	}
	err = a.api.do(ctx, http.MethodPost, "/auth/logout", map[string]any{"provider": nil, "voip_provider": nil}, nil)
	b.stop(p.Account)
	if err != nil {
		var coded *codedError
		if errors.As(err, &coded) && coded.code == "loggedOut" {
			return map[string]any{}, nil
		}
		return nil, fail("logoutFailed", "Discord did not confirm the sign-out. Remove the session under Devices in Discord's settings.")
	}
	return map[string]any{}, nil
}

type dcChatParams struct {
	Account  string `json:"account"`
	Chat     string `json:"chat"`
	ID       string `json:"id"`
	Text     string `json:"text"`
	QuotedID string `json:"quotedId"`
	Emoji    string `json:"emoji"`
	Remove   bool   `json:"remove"`
	Last     string `json:"last"`
	Before   string `json:"before"`
	After    string `json:"after"`
	ServerID string `json:"serverId"`
	Muted    *int64 `json:"mutedUntil"`
	User     string `json:"user"`
	Path     string `json:"path"`
}

func (b *bridge) dcTarget(raw json.RawMessage) (*dcAccount, dcChatParams, error) {
	p, err := decode[dcChatParams](raw)
	if err != nil {
		return nil, p, err
	}
	a, err := b.discordRunning(p.Account)
	if err != nil {
		return nil, p, err
	}
	if p.Chat != "" && !validSnowflake(p.Chat) || p.ID != "" && !validSnowflake(p.ID) {
		return nil, p, fail("invalidInput", "Unknown chat or message.")
	}
	return a, p, nil
}

func (b *bridge) dcSend(ctx context.Context, raw json.RawMessage) (any, error) {
	a, p, err := b.dcTarget(raw)
	if err != nil {
		return nil, err
	}
	if strings.TrimSpace(p.Text) == "" || len([]rune(p.Text)) > 4000 {
		return nil, fail("invalidInput", "Discord messages can be up to 4,000 characters.")
	}
	body := map[string]any{"content": p.Text, "nonce": p.ID, "enforce_nonce": true, "tts": false, "flags": 0}
	if p.QuotedID != "" && validSnowflake(p.QuotedID) {
		body["message_reference"] = map[string]any{"message_id": p.QuotedID, "channel_id": p.Chat, "fail_if_not_exists": false}
	}
	var sent dcMessage
	if err = a.api.do(ctx, http.MethodPost, "/channels/"+p.Chat+"/messages", body, &sent); err != nil {
		return nil, err
	}
	return map[string]any{"id": sent.ID, "at": snowflakeTime(sent.ID)}, nil
}

func (b *bridge) dcMarkRead(ctx context.Context, raw json.RawMessage) (any, error) {
	a, p, err := b.dcTarget(raw)
	if err != nil {
		return nil, err
	}
	if !validSnowflake(p.Last) {
		return map[string]any{}, nil
	}
	if err = a.api.do(ctx, http.MethodPost, "/channels/"+p.Chat+"/messages/"+p.Last+"/ack", map[string]any{"token": nil}, nil); err != nil {
		return nil, fail("readFailed", "Discord did not take the read mark.")
	}
	return map[string]any{}, nil
}

func (b *bridge) dcReact(ctx context.Context, raw json.RawMessage) (any, error) {
	a, p, err := b.dcTarget(raw)
	if err != nil {
		return nil, err
	}
	if p.Emoji == "" || len(p.Emoji) > 120 {
		return nil, fail("invalidInput", "Choose a reaction.")
	}
	method := http.MethodPut
	if p.Remove {
		method = http.MethodDelete
	}
	path := "/channels/" + p.Chat + "/messages/" + p.ID + "/reactions/" + url.PathEscape(p.Emoji) + "/@me?location=Message&type=0"
	return map[string]any{}, a.api.do(ctx, method, path, nil, nil)
}

func (b *bridge) dcEdit(ctx context.Context, raw json.RawMessage) (any, error) {
	a, p, err := b.dcTarget(raw)
	if err != nil {
		return nil, err
	}
	if strings.TrimSpace(p.Text) == "" || len([]rune(p.Text)) > 4000 {
		return nil, fail("invalidInput", "Discord messages can be up to 4,000 characters.")
	}
	return map[string]any{}, a.api.do(ctx, http.MethodPatch, "/channels/"+p.Chat+"/messages/"+p.ID, map[string]any{"content": p.Text}, nil)
}

func (b *bridge) dcRevoke(ctx context.Context, raw json.RawMessage) (any, error) {
	a, p, err := b.dcTarget(raw)
	if err != nil {
		return nil, err
	}
	return map[string]any{}, a.api.do(ctx, http.MethodDelete, "/channels/"+p.Chat+"/messages/"+p.ID, nil, nil)
}

// dcMute mutes a channel or DM on Discord, so other devices follow.
func (b *bridge) dcMute(ctx context.Context, raw json.RawMessage) (any, error) {
	a, p, err := b.dcTarget(raw)
	if err != nil {
		return nil, err
	}
	if p.Muted == nil {
		return nil, fail("invalidInput", "Discord channels can be muted, not pinned or archived.")
	}
	guild := "%40me"
	if p.ServerID != "" {
		if !validSnowflake(p.ServerID) {
			return nil, fail("invalidInput", "Unknown server.")
		}
		guild = p.ServerID
	}
	config := map[string]any{"selected_time_window": -1, "end_time": nil}
	if *p.Muted > 0 {
		end := time.UnixMilli(*p.Muted).UTC()
		config = map[string]any{"selected_time_window": int(time.Until(end).Seconds()), "end_time": end.Format(time.RFC3339)}
	}
	override := map[string]any{"muted": *p.Muted != 0, "mute_config": config}
	return map[string]any{}, a.api.do(ctx, http.MethodPatch, "/users/@me/guilds/"+guild+"/settings", map[string]any{"channel_overrides": map[string]any{p.Chat: override}}, nil)
}

// dcHistory fetches messages around what Dar has: before the oldest, after the newest
// (up to 500), or the latest 50.
func (b *bridge) dcHistory(ctx context.Context, raw json.RawMessage) (any, error) {
	a, p, err := b.dcTarget(raw)
	if err != nil {
		return nil, err
	}
	fetch := func(query string) ([]dcMessage, error) {
		var list []dcMessage
		err := a.api.do(ctx, http.MethodGet, "/channels/"+p.Chat+"/messages?"+query, nil, &list)
		return list, err
	}
	count := 0
	switch {
	case validSnowflake(p.Before):
		list, err := fetch("limit=50&before=" + p.Before)
		if err != nil {
			return nil, err
		}
		if !a.storeMessages(ctx, "history", list, nil) {
			return nil, fail("storeFailed", "The messages could not be stored.")
		}
		count = len(list)
		return map[string]any{"count": count, "more": len(list) == 50}, nil
	case validSnowflake(p.After):
		after := p.After
		for page := 0; page < 5; page++ {
			list, err := fetch("limit=100&after=" + after)
			if err != nil {
				return nil, err
			}
			if len(list) == 0 {
				break
			}
			if !a.storeMessages(ctx, "history", list, nil) {
				return nil, fail("storeFailed", "The messages could not be stored.")
			}
			count += len(list)
			for _, m := range list {
				if snowflakeLess(after, m.ID) {
					after = m.ID
				}
			}
			if len(list) < 100 {
				break
			}
		}
	default:
		list, err := fetch("limit=50")
		if err != nil {
			return nil, err
		}
		if !a.storeMessages(ctx, "history", list, nil) {
			return nil, fail("storeFailed", "The messages could not be stored.")
		}
		count = len(list)
	}
	return map[string]any{"count": count}, nil
}

// dcFriends lists friends for starting a direct message.
func (b *bridge) dcFriends(_ context.Context, raw json.RawMessage) (any, error) {
	p, err := decode[accountParams](raw)
	if err != nil {
		return nil, err
	}
	a, err := b.discordRunning(p.Account)
	if err != nil {
		return nil, err
	}
	a.mu.Lock()
	type friend struct {
		ID       string `json:"id"`
		Name     string `json:"name"`
		Username string `json:"username"`
	}
	list := []friend{}
	for id := range a.friends {
		u := a.users[id]
		list = append(list, friend{ID: id, Name: u.name(), Username: u.Username})
	}
	a.mu.Unlock()
	sort.Slice(list, func(i, j int) bool { return strings.ToLower(list[i].Name) < strings.ToLower(list[j].Name) })
	return map[string]any{"friends": list}, nil
}

// dcOpenDM opens (or finds) the direct message with someone and stores it as a chat.
func (b *bridge) dcOpenDM(ctx context.Context, raw json.RawMessage) (any, error) {
	a, p, err := b.dcTarget(raw)
	if err != nil {
		return nil, err
	}
	if !validSnowflake(p.User) {
		return nil, fail("invalidInput", "Choose someone to message.")
	}
	var c dcChannel
	if err = a.api.do(ctx, http.MethodPost, "/users/@me/channels", map[string]any{"recipients": []string{p.User}}, &c); err != nil {
		return nil, err
	}
	a.mu.Lock()
	a.privates[c.ID] = &c
	chat := a.privateChat(&c)
	a.mu.Unlock()
	if !a.ingest(ctx, map[string]any{"kind": "chats", "chats": []dcChatOut{chat}}) {
		return nil, fail("storeFailed", "The chat could not be stored.")
	}
	return map[string]any{"chat": c.ID}, nil
}

func discordMediaHost(raw string) bool {
	u, err := url.Parse(raw)
	if err != nil || u.Scheme != "https" {
		return false
	}
	host := strings.ToLower(u.Hostname())
	return host == "cdn.discordapp.com" || host == "media.discordapp.net" || host == "cdn.discord.com" || host == "media.discord.com"
}

// dcDownload fetches an attachment into the runtime's download directory, refreshing its
// signed link when Discord has expired it.
func (b *bridge) dcDownload(ctx context.Context, raw json.RawMessage) (any, error) {
	a, p, err := b.dcTarget(raw)
	if err != nil {
		return nil, err
	}
	dir := os.Getenv("DAR_CHAT_DOWNLOADS")
	if dir == "" || filepath.Dir(p.Path) != filepath.Clean(dir) || !downloadName.MatchString(filepath.Base(p.Path)) {
		return nil, fail("invalidInput", "Downloads go to the runtime's download directory.")
	}
	name := mediaName(p.Chat, p.ID)
	stored, ok, err := getJSON[dcStoredMedia](ctx, a.kv, name)
	if err != nil {
		return nil, err
	} else if !ok || !discordMediaHost(stored.URL) {
		return nil, fail("noMedia", "This attachment cannot be downloaded.")
	}
	status, err := fetchTo(ctx, a.api.client, stored.URL, p.Path)
	if status == http.StatusNotFound || status == http.StatusForbidden || status == http.StatusGone {
		var refreshed struct {
			URLs []struct {
				Original  string `json:"original"`
				Refreshed string `json:"refreshed"`
			} `json:"refreshed_urls"`
		}
		if a.api.do(ctx, http.MethodPost, "/attachments/refresh-urls", map[string]any{"attachment_urls": []string{stored.URL}}, &refreshed) == nil &&
			len(refreshed.URLs) == 1 && discordMediaHost(refreshed.URLs[0].Refreshed) {
			stored.URL = refreshed.URLs[0].Refreshed
			_ = putJSON(ctx, a.kv, name, stored, 0)
			status, err = fetchTo(ctx, a.api.client, stored.URL, p.Path)
		}
	}
	if err != nil {
		_ = os.Remove(p.Path)
		if status == http.StatusNotFound || status == http.StatusForbidden || status == http.StatusGone {
			return nil, fail("expired", "Discord no longer has this file.")
		}
		return nil, fail("downloadFailed", "The attachment could not be downloaded.")
	}
	info, _ := os.Stat(p.Path)
	return map[string]any{"size": info.Size()}, nil
}

const discordDownloadLimit = 1 << 30

func fetchTo(ctx context.Context, client *http.Client, link, path string) (int, error) {
	ctx, cancel := context.WithTimeout(ctx, 15*time.Minute)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, link, nil)
	if err != nil {
		return 0, err
	}
	req.Header.Set("User-Agent", discordUserAgent)
	resp, err := (&http.Client{Transport: client.Transport}).Do(req)
	if err != nil {
		return 0, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return resp.StatusCode, fmt.Errorf("status %d", resp.StatusCode)
	}
	file, err := os.OpenFile(path, os.O_RDWR|os.O_CREATE|os.O_TRUNC, 0o600)
	if err != nil {
		return 0, err
	}
	n, err := io.Copy(file, io.LimitReader(resp.Body, discordDownloadLimit+1))
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	if err == nil && n > discordDownloadLimit {
		err = fmt.Errorf("too large")
	}
	return resp.StatusCode, err
}
