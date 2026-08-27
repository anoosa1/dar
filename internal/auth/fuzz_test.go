package auth

import (
	"bytes"
	"testing"
)

func FuzzLimitedJSONDecoder(f *testing.F) {
	for _, seed := range []string{
		`{"sub":"alice","email":"alice@example.test"}`,
		`{"unknown":true}`,
		`[`,
		"",
	} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, raw string) {
		if len(raw) > 2<<20 {
			return
		}
		var target any
		_ = decodeLimitedJSON(bytes.NewBufferString(raw), &target, 1<<20)
	})
}
