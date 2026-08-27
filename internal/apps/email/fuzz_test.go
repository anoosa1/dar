package email

import (
	"os"
	"path/filepath"
	"testing"
)

func FuzzMIMEMessageParsing(f *testing.F) {
	for _, seed := range []string{
		"From: alice@example.test\r\nTo: bob@example.test\r\nSubject: Hello\r\n\r\nBody",
		"MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nhello\r\n--x--\r\n",
		"Subject: =?utf-8?q?caf=C3=A9?=\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=",
	} {
		f.Add(seed)
	}
	root := f.TempDir()
	f.Fuzz(func(t *testing.T, raw string) {
		if len(raw) > 1<<20 {
			return
		}
		path := filepath.Join(root, "message")
		if err := os.WriteFile(path, []byte(raw), 0o600); err != nil {
			t.Fatal(err)
		}
		_, _ = parseMailFile(path)
		_ = decodeHeader(raw)
		_ = parseMailDate(raw)
	})
}
