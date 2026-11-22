package main

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"errors"
	"net/http"
	"strings"
	"time"

	"github.com/coder/websocket"
)

// Discord's QR sign-in ("remote auth", protocol v2): Dar shows a code, the Discord app on a
// phone approves it, and Discord hands over a session token encrypted to a key only this
// process holds. Dar never sees the password.

const remoteAuthURL = "wss://remote-auth-gateway.discord.gg/?v=2"

type remoteAuthUser struct {
	ID       string
	Username string
}

type remoteAuthEvents struct {
	code    func(url string, expiresIn time.Duration)
	scanned func(user remoteAuthUser)
}

type remoteAuthFrame struct {
	Op                   string `json:"op"`
	HeartbeatInterval    int    `json:"heartbeat_interval,omitempty"`
	TimeoutMS            int    `json:"timeout_ms,omitempty"`
	EncodedPublicKey     string `json:"encoded_public_key,omitempty"`
	EncryptedNonce       string `json:"encrypted_nonce,omitempty"`
	Nonce                string `json:"nonce,omitempty"`
	Fingerprint          string `json:"fingerprint,omitempty"`
	EncryptedUserPayload string `json:"encrypted_user_payload,omitempty"`
	Ticket               string `json:"ticket,omitempty"`
}

func oaepDecrypt(key *rsa.PrivateKey, encoded string) ([]byte, error) {
	raw, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		return nil, err
	}
	return rsa.DecryptOAEP(sha256.New(), nil, key, raw, nil)
}

// remoteAuth runs one sign-in until the phone approves it (returning the token), the code
// expires, the person cancels on the phone, or ctx ends.
func remoteAuth(ctx context.Context, gatewayURL string, api *discordAPI, events remoteAuthEvents) (string, remoteAuthUser, error) {
	var user remoteAuthUser
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		return "", user, err
	}
	spki, err := x509.MarshalPKIXPublicKey(&key.PublicKey)
	if err != nil {
		return "", user, err
	}
	digest := sha256.Sum256(spki)
	fingerprint := base64.RawURLEncoding.EncodeToString(digest[:])

	conn, _, err := websocket.Dial(ctx, gatewayURL, &websocket.DialOptions{HTTPHeader: http.Header{"Origin": {"https://discord.com"}, "User-Agent": {discordUserAgent}}})
	if err != nil {
		return "", user, fail("connectFailed", "Dar could not reach Discord. Check the server's internet connection.")
	}
	defer conn.CloseNow()
	conn.SetReadLimit(1 << 20)
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	write := func(f remoteAuthFrame) error {
		raw, _ := json.Marshal(f)
		return conn.Write(ctx, websocket.MessageText, raw)
	}
	acked := make(chan struct{}, 1)
	for {
		_, raw, err := conn.Read(ctx)
		if err != nil {
			if ctx.Err() != nil {
				return "", user, fail("timeout", "The code expired.")
			}
			switch websocket.CloseStatus(err) {
			case 4003:
				return "", user, fail("timeout", "The code expired.")
			case websocket.StatusNormalClosure:
				return "", user, fail("cancelled", "Sign-in was cancelled on the phone.")
			}
			return "", user, fail("pairingFailed", "Discord ended the sign-in.")
		}
		var f remoteAuthFrame
		if json.Unmarshal(raw, &f) != nil {
			continue
		}
		switch f.Op {
		case "hello":
			interval := time.Duration(max(f.HeartbeatInterval, 5000)) * time.Millisecond
			if f.TimeoutMS > 0 {
				timeout := time.Duration(f.TimeoutMS) * time.Millisecond
				var stop context.CancelFunc
				ctx, stop = context.WithTimeout(ctx, timeout)
				defer stop()
			}
			go func() {
				ticker := time.NewTicker(interval)
				defer ticker.Stop()
				acked <- struct{}{}
				for {
					select {
					case <-ctx.Done():
						return
					case <-ticker.C:
						select {
						case <-acked:
						default:
							// No answer since the last heartbeat: the connection is gone.
							conn.CloseNow()
							return
						}
						if write(remoteAuthFrame{Op: "heartbeat"}) != nil {
							return
						}
					}
				}
			}()
			if err = write(remoteAuthFrame{Op: "init", EncodedPublicKey: base64.StdEncoding.EncodeToString(spki)}); err != nil {
				return "", user, fail("pairingFailed", "Discord ended the sign-in.")
			}
		case "heartbeat_ack":
			select {
			case acked <- struct{}{}:
			default:
			}
		case "nonce_proof":
			nonce, err := oaepDecrypt(key, f.EncryptedNonce)
			if err != nil {
				return "", user, fail("pairingFailed", "Discord's sign-in could not be read.")
			}
			if err = write(remoteAuthFrame{Op: "nonce_proof", Nonce: base64.RawURLEncoding.EncodeToString(nonce)}); err != nil {
				return "", user, fail("pairingFailed", "Discord ended the sign-in.")
			}
		case "pending_remote_init":
			if f.Fingerprint != fingerprint {
				return "", user, fail("pairingFailed", "Discord's sign-in did not match this request.")
			}
			left := 2 * time.Minute
			if deadline, ok := ctx.Deadline(); ok {
				left = time.Until(deadline)
			}
			events.code("https://discord.com/ra/"+fingerprint, left)
		case "pending_ticket":
			payload, err := oaepDecrypt(key, f.EncryptedUserPayload)
			if err != nil {
				return "", user, fail("pairingFailed", "Discord's sign-in could not be read.")
			}
			// id:discriminator:avatar:username
			parts := strings.SplitN(string(payload), ":", 4)
			if len(parts) == 4 {
				user = remoteAuthUser{ID: parts[0], Username: parts[3]}
				events.scanned(user)
			}
		case "pending_login":
			var result struct {
				EncryptedToken string `json:"encrypted_token"`
			}
			if err = api.do(ctx, http.MethodPost, "/users/@me/remote-auth/login", map[string]any{"ticket": f.Ticket}, &result); err != nil {
				var coded *codedError
				if errors.As(err, &coded) && coded.code == "captcha" {
					return "", user, err
				}
				return "", user, fail("pairingFailed", "Discord did not complete the sign-in.")
			}
			token, err := oaepDecrypt(key, result.EncryptedToken)
			if err != nil || len(token) == 0 {
				return "", user, fail("pairingFailed", "Discord's sign-in could not be read.")
			}
			_ = conn.Close(websocket.StatusNormalClosure, "")
			return string(token), user, nil
		case "cancel":
			return "", user, fail("cancelled", "Sign-in was cancelled on the phone.")
		}
	}
}
