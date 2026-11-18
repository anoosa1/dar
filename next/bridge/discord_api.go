package main

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"strconv"
	"strings"
	"time"
)

// Dar talks to Discord as the web client does: the same headers and client properties, and a
// session token rather than a bot token. Discord's terms do not allow this; the person
// accepted that when linking.
const (
	discordUserAgent    = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36"
	discordBrowserVer   = "142.0.0.0"
	discordClientBuild  = 470000
	discordAPIBase      = "https://discord.com/api/v9"
	discordGatewayURL   = "wss://gateway.discord.gg/?v=9&encoding=json"
	discordEpochMS      = 1420070400000
	discordMaxBodyBytes = 64 << 20
)

// discordProperties are the client properties sent on identify and in X-Super-Properties.
func discordProperties() map[string]any {
	return map[string]any{
		"os": "Linux", "browser": "Chrome", "device": "", "system_locale": "en-US", "has_client_mods": false,
		"browser_user_agent": discordUserAgent, "browser_version": discordBrowserVer, "os_version": "",
		"referrer": "", "referring_domain": "", "referrer_current": "", "referring_domain_current": "",
		"release_channel": "stable", "client_build_number": discordClientBuild, "client_event_source": nil,
	}
}

type discordAPI struct {
	base   string
	token  string
	client *http.Client
	props  string
}

func newDiscordAPI(base, token string) *discordAPI {
	raw, _ := json.Marshal(discordProperties())
	return &discordAPI{base: base, token: token, client: &http.Client{Timeout: 45 * time.Second}, props: base64.StdEncoding.EncodeToString(raw)}
}

func (api *discordAPI) header(req *http.Request) {
	if api.token != "" {
		req.Header.Set("Authorization", api.token)
	}
	req.Header.Set("User-Agent", discordUserAgent)
	req.Header.Set("X-Super-Properties", api.props)
	req.Header.Set("X-Discord-Locale", "en-US")
	req.Header.Set("X-Discord-Timezone", "UTC")
	req.Header.Set("Accept", "*/*")
}

// do sends one request, waiting out short rate limits. Errors carry codes the runtime maps for people.
func (api *discordAPI) do(ctx context.Context, method, path string, body any, out any) error {
	var payload []byte
	if body != nil {
		var err error
		if payload, err = json.Marshal(body); err != nil {
			return err
		}
	}
	for attempt := 0; ; attempt++ {
		req, err := http.NewRequestWithContext(ctx, method, api.base+path, bytes.NewReader(payload))
		if err != nil {
			return err
		}
		api.header(req)
		if body != nil {
			req.Header.Set("Content-Type", "application/json")
		}
		resp, err := api.client.Do(req)
		if err != nil {
			if ctx.Err() != nil {
				return ctx.Err()
			}
			return fail("unreachable", "Dar could not reach Discord.")
		}
		raw, err := io.ReadAll(io.LimitReader(resp.Body, discordMaxBodyBytes))
		resp.Body.Close()
		if err != nil {
			return fail("unreachable", "Dar could not reach Discord.")
		}
		if resp.StatusCode == http.StatusTooManyRequests && attempt < 3 {
			var limit struct {
				RetryAfter float64 `json:"retry_after"`
			}
			_ = json.Unmarshal(raw, &limit)
			wait := time.Duration(limit.RetryAfter*1000) * time.Millisecond
			if wait <= 0 {
				if seconds, err := strconv.ParseFloat(resp.Header.Get("Retry-After"), 64); err == nil {
					wait = time.Duration(seconds*1000) * time.Millisecond
				}
			}
			if wait > 15*time.Second {
				return fail("rateLimited", "Discord asked Dar to slow down. Try again in a minute.")
			}
			select {
			case <-time.After(wait + 100*time.Millisecond):
				continue
			case <-ctx.Done():
				return ctx.Err()
			}
		}
		if resp.StatusCode >= 300 {
			return discordError(resp.StatusCode, raw)
		}
		if out != nil && len(raw) > 0 && resp.StatusCode != http.StatusNoContent {
			if err = json.Unmarshal(raw, out); err != nil {
				return fail("discordError", "Discord's answer could not be read.")
			}
		}
		return nil
	}
}

func discordError(status int, raw []byte) error {
	var body struct {
		Code       int      `json:"code"`
		Message    string   `json:"message"`
		CaptchaKey []string `json:"captcha_key"`
	}
	_ = json.Unmarshal(raw, &body)
	switch {
	case len(body.CaptchaKey) > 0 || body.Code == 50035 && strings.Contains(string(raw), "captcha"):
		return fail("captcha", "Discord asked for a check that Dar cannot show. Finish it in the Discord app, then try again.")
	case status == http.StatusUnauthorized:
		return fail("loggedOut", "Discord signed Dar out.")
	case status == http.StatusForbidden:
		return fail("forbidden", "Discord does not allow this here.")
	case status == http.StatusNotFound:
		return fail("notFound", "Discord no longer has this.")
	case status == http.StatusTooManyRequests:
		return fail("rateLimited", "Discord asked Dar to slow down. Try again in a minute.")
	case status == http.StatusBadRequest:
		return fail("invalidInput", "Discord did not accept this.")
	}
	return fail("discordError", "Discord could not do this right now.")
}

// snowflakeTime is when a Discord ID was made, in Unix milliseconds.
func snowflakeTime(id string) int64 {
	n, err := strconv.ParseUint(id, 10, 64)
	if err != nil || n == 0 {
		return 0
	}
	return int64(n>>22) + discordEpochMS
}

func snowflakeLess(a, b string) bool {
	if len(a) != len(b) {
		return len(a) < len(b)
	}
	return a < b
}

func validSnowflake(id string) bool {
	if len(id) == 0 || len(id) > 25 {
		return false
	}
	for _, r := range id {
		if r < '0' || r > '9' {
			return false
		}
	}
	return true
}

var errDiscordGone = errors.New("discord account stopped")
