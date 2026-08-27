package credentials

import (
	"bytes"
	"encoding/base64"
	"testing"
)

func TestParseAndKeyRotation(t *testing.T) {
	newMaterial := bytes.Repeat([]byte{0x11}, 32)
	oldMaterial := bytes.Repeat([]byte{0x22}, 32)
	raw := "new:" + base64.StdEncoding.EncodeToString(newMaterial) + ",old:" + base64.StdEncoding.EncodeToString(oldMaterial)
	keys, err := Parse(raw)
	if err != nil || len(keys) != 2 {
		t.Fatalf("Parse() keys=%d err=%v", len(keys), err)
	}
	oldRing, err := New([]Key{{ID: "old", Material: oldMaterial}})
	if err != nil {
		t.Fatal(err)
	}
	keyID, nonce, ciphertext, err := oldRing.Encrypt("secret", []byte("owner"))
	if err != nil {
		t.Fatal(err)
	}
	rotated, err := New(keys)
	if err != nil {
		t.Fatal(err)
	}
	plain, needsRotation, err := rotated.Decrypt(keyID, nonce, ciphertext, []byte("owner"))
	if err != nil || plain != "secret" || !needsRotation {
		t.Fatalf("Decrypt() plain=%q rotate=%v err=%v", plain, needsRotation, err)
	}
	if _, _, err := rotated.Decrypt(keyID, nonce, ciphertext, []byte("other")); err == nil {
		t.Fatal("ciphertext was not bound to its associated data")
	}
}

func TestKeyringRejectsInvalidConfigurationAndCiphertext(t *testing.T) {
	invalid := []string{"", "bad", "bad:%%%", "bad:" + base64.StdEncoding.EncodeToString([]byte("short"))}
	for _, raw := range invalid {
		if _, err := Parse(raw); err == nil {
			t.Fatalf("Parse(%q) succeeded", raw)
		}
	}
	material := bytes.Repeat([]byte{0x33}, 32)
	if _, err := New(nil); err == nil {
		t.Fatal("empty keyring succeeded")
	}
	for _, keys := range [][]Key{
		{{ID: "bad id", Material: material}},
		{{ID: "short", Material: []byte("short")}},
		{{ID: "duplicate", Material: material}, {ID: "duplicate", Material: material}},
	} {
		if _, err := New(keys); err == nil {
			t.Errorf("invalid key ring accepted: %+v", keys)
		}
	}
	broken := Ring{current: "bad", keys: map[string][]byte{"bad": []byte("short")}}
	if _, _, _, err := broken.Encrypt("secret", nil); err == nil {
		t.Fatal("invalid AES key encrypted data")
	}
	if _, _, err := broken.Decrypt("bad", nil, nil, nil); err == nil {
		t.Fatal("invalid AES key decrypted data")
	}
	ring, err := New([]Key{{ID: "key", Material: material}})
	if err != nil {
		t.Fatal(err)
	}
	keyID, nonce, ciphertext, err := ring.Encrypt("secret", nil)
	if err != nil {
		t.Fatal(err)
	}
	ciphertext[0] ^= 0xff
	if _, _, err := ring.Decrypt(keyID, nonce, ciphertext, nil); err == nil {
		t.Fatal("tampered ciphertext succeeded")
	}
	if _, _, err := ring.Decrypt("missing", nonce, ciphertext, nil); err == nil {
		t.Fatal("unknown key succeeded")
	}
}
