package email

import (
	"fmt"

	"github.com/anoosa/dar/internal/credentials"
)

type CredentialKey struct {
	ID  string
	Key []byte
}

type keyRing struct {
	ring credentials.Ring
	err  error
}

// ParseCredentialKeys is retained for callers while the operator-facing
// configuration is shared by all credential-bearing apps.
func ParseCredentialKeys(raw string) ([]CredentialKey, error) {
	parsed, err := credentials.Parse(raw)
	if err != nil {
		return nil, err
	}
	out := make([]CredentialKey, 0, len(parsed))
	for _, key := range parsed {
		out = append(out, CredentialKey{ID: key.ID, Key: key.Material})
	}
	return out, nil
}

func newKeyRing(keys []CredentialKey) keyRing {
	shared := make([]credentials.Key, 0, len(keys))
	for _, key := range keys {
		shared = append(shared, credentials.Key{ID: key.ID, Material: key.Key})
	}
	ring, err := credentials.New(shared)
	return keyRing{ring: ring, err: err}
}

func credentialAAD(userID, accountID string) []byte {
	// Preserve the original AAD so existing Email ciphertext remains readable.
	return []byte("dar-email-v1\x00" + userID + "\x00" + accountID)
}

func (r keyRing) encrypt(userID, accountID, plaintext string) (string, []byte, []byte, error) {
	if r.err != nil {
		return "", nil, nil, r.err
	}
	return r.ring.Encrypt(plaintext, credentialAAD(userID, accountID))
}

func (r keyRing) decrypt(userID, accountID, keyID string, nonce, ciphertext []byte) (string, bool, error) {
	if r.err != nil {
		return "", false, r.err
	}
	plain, rotate, err := r.ring.Decrypt(keyID, nonce, ciphertext, credentialAAD(userID, accountID))
	if err != nil {
		return "", false, fmt.Errorf("email credential: %w", err)
	}
	return plain, rotate, nil
}
