package testutil

import (
	"testing"

	"github.com/emersion/go-imap/client"
)

func TestIMAPServer(t *testing.T) {
	fixture := StartIMAP(t)
	conn, err := client.Dial(fixture.Address)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Logout()
	if err := conn.Login(fixture.Username, fixture.Password); err != nil {
		t.Fatal(err)
	}
	status, err := conn.Select("INBOX", true)
	if err != nil {
		t.Fatal(err)
	}
	if status.Messages != 1 {
		t.Fatalf("messages = %d, want 1", status.Messages)
	}
}
