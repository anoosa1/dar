package contacts

import "testing"

func FuzzVCardDecoding(f *testing.F) {
	for _, seed := range []string{
		"BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice Example\r\nEMAIL;TYPE=work:alice@example.test\r\nEND:VCARD\r\n",
		"BEGIN:VCARD\r\nVERSION:2.1\r\nN:Doe;Jane;;;\r\nTEL;HOME:123\r\nEND:VCARD\r\n",
		"BEGIN:VCARD\nBROKEN\nEND:VCARD\n",
	} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, raw string) {
		if len(raw) > 1<<20 {
			return
		}
		app := &App{}
		card, err := app.decodeCard([]byte(raw))
		if err != nil {
			return
		}
		contact := app.contactFromCard(card, "fuzz", 0)
		encoded := app.toVCard(contact)
		if _, err := app.decodeCard([]byte(encoded)); err != nil {
			t.Fatalf("encoded vCard did not round-trip: %v", err)
		}
	})
}
