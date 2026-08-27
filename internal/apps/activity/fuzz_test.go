package activity

import "testing"

func FuzzActivityCursor(f *testing.F) {
	for _, seed := range []string{"", "eyJ0IjoxLCJpIjoyfQ", "not-base64", "e30="} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, raw string) {
		if len(raw) > 64<<10 {
			return
		}
		value, err := decodeCursor(raw)
		if err != nil {
			return
		}
		if raw == "" {
			return
		}
		encoded := encodeCursor(value)
		if _, err := decodeCursor(encoded); err != nil {
			t.Fatalf("cursor did not round-trip: %v", err)
		}
	})
}
