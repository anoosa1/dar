// Package credentials provides versioned AES-GCM encryption and key rotation.
package credentials

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"regexp"
	"strings"
)

type Key struct {
	ID       string
	Material []byte
}

type Ring struct {
	current string
	keys    map[string][]byte
}

var keyIDPattern = regexp.MustCompile(`^[A-Za-z0-9_-]{1,40}$`)

// Parse decodes comma-separated id:base64-key entries. The first key encrypts
// new values; all keys remain available for transparent rotation.
func Parse(raw string) ([]Key, error) {
	if strings.TrimSpace(raw) == "" {
		return nil, errors.New("credential encryption keys are not configured")
	}
	var out []Key
	seen := map[string]bool{}
	for _, entry := range strings.Split(raw, ",") {
		parts := strings.SplitN(strings.TrimSpace(entry), ":", 2)
		if len(parts) != 2 || !keyIDPattern.MatchString(parts[0]) || seen[parts[0]] {
			return nil, errors.New("invalid or duplicate credential encryption key id")
		}
		material, err := base64.StdEncoding.DecodeString(parts[1])
		if err != nil || len(material) != 32 {
			return nil, fmt.Errorf("credential encryption key %q must be base64-encoded 32 bytes", parts[0])
		}
		seen[parts[0]] = true
		out = append(out, Key{ID: parts[0], Material: material})
	}
	return out, nil
}

func New(keys []Key) (Ring, error) {
	r := Ring{keys: map[string][]byte{}}
	for i, key := range keys {
		if !keyIDPattern.MatchString(key.ID) || len(key.Material) != 32 {
			return Ring{}, errors.New("invalid credential key ring")
		}
		if _, exists := r.keys[key.ID]; exists {
			return Ring{}, errors.New("duplicate credential key id")
		}
		r.keys[key.ID] = append([]byte(nil), key.Material...)
		if i == 0 {
			r.current = key.ID
		}
	}
	if r.current == "" {
		return Ring{}, errors.New("credential encryption is not configured")
	}
	return r, nil
}

func (r Ring) Encrypt(plaintext string, aad []byte) (string, []byte, []byte, error) {
	block, err := aes.NewCipher(r.keys[r.current])
	if err != nil {
		return "", nil, nil, err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", nil, nil, err
	}
	nonce := make([]byte, gcm.NonceSize())
	if _, err := io.ReadFull(rand.Reader, nonce); err != nil {
		return "", nil, nil, err
	}
	return r.current, nonce, gcm.Seal(nil, nonce, []byte(plaintext), aad), nil
}

func (r Ring) Decrypt(keyID string, nonce, ciphertext, aad []byte) (string, bool, error) {
	key, ok := r.keys[keyID]
	if !ok {
		return "", false, fmt.Errorf("credential key %q is unavailable", keyID)
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return "", false, err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", false, err
	}
	plain, err := gcm.Open(nil, nonce, ciphertext, aad)
	if err != nil {
		return "", false, errors.New("credential could not be decrypted")
	}
	return string(plain), keyID != r.current, nil
}
