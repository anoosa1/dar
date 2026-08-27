package testutil

import (
	"net"
	"net/smtp"
	"strings"
	"testing"
)

func TestSMTPServerAcceptsAuthenticatedMessage(t *testing.T) {
	fixture := StartSMTP(t, SMTPOptions{})
	host, _, _ := net.SplitHostPort(fixture.Address)
	client, err := smtp.Dial(fixture.Address)
	if err != nil {
		t.Fatal(err)
	}
	if err := client.Auth(smtp.PlainAuth("", fixture.options.Username, fixture.options.Password, host)); err != nil {
		t.Fatal(err)
	}
	if err := client.Mail("sender@example.test"); err != nil {
		t.Fatal(err)
	}
	if err := client.Rcpt("recipient@example.test"); err != nil {
		t.Fatal(err)
	}
	data, err := client.Data()
	if err != nil {
		t.Fatal(err)
	}
	_, _ = data.Write([]byte("Subject: Fixture\r\n\r\nHello"))
	if err := data.Close(); err != nil {
		t.Fatal(err)
	}
	if err := client.Quit(); err != nil {
		t.Fatal(err)
	}
	messages := fixture.Messages()
	if len(messages) != 1 || !strings.Contains(string(messages[0].Data), "Hello") {
		t.Fatalf("messages = %+v", messages)
	}
}
