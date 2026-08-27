package email

import (
	"context"
	"crypto/tls"
	"net"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/outbound"
	"github.com/anoosa/dar/internal/testutil"
)

func smtpFixtureAccount(t *testing.T, fixture *testutil.SMTPServer, mode securityMode) Account {
	t.Helper()
	host, portText, err := net.SplitHostPort(fixture.Address)
	if err != nil {
		t.Fatal(err)
	}
	port, err := strconv.Atoi(portText)
	if err != nil {
		t.Fatal(err)
	}
	return Account{
		Email:        "sender@example.test",
		Username:     "username",
		SMTPHost:     host,
		SMTPPort:     port,
		SMTPSecurity: mode,
	}
}

func TestSMTPAuthenticationTransportAndDeliveryMatrix(t *testing.T) {
	cases := []struct {
		name    string
		options testutil.SMTPOptions
		mode    securityMode
	}{
		{"plain", testutil.SMTPOptions{Auth: "PLAIN"}, securityNone},
		{"login", testutil.SMTPOptions{Auth: "LOGIN"}, securityNone},
		{"starttls", testutil.SMTPOptions{Auth: "PLAIN", STARTTLS: true}, securitySTARTTLS},
		{"implicit_tls", testutil.SMTPOptions{Auth: "PLAIN", ImplicitTLS: true}, securityTLS},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			fixture := testutil.StartSMTP(t, tc.options)
			account := smtpFixtureAccount(t, fixture, tc.mode)
			policy, err := outbound.ParsePolicy(account.SMTPHost, "")
			if err != nil {
				t.Fatal(err)
			}
			a := &App{outbound: policy}
			if fixture.ClientTLS != nil {
				a.smtpTLSConfig = func(string) *tls.Config {
					return fixture.ClientTLS.Clone()
				}
			}
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			if err := a.testSMTP(ctx, account, "password"); err != nil {
				t.Fatal(err)
			}
			raw := []byte("From: sender@example.test\r\nTo: recipient@example.test\r\nSubject: Matrix\r\n\r\nhello")
			if err := a.deliver(ctx, "alice", account, "password", raw, []string{"recipient@example.test"}); err != nil {
				t.Fatal(err)
			}
			messages := fixture.Messages()
			if len(messages) != 1 || !strings.Contains(string(messages[0].Data), "Subject: Matrix") {
				t.Fatalf("messages = %+v", messages)
			}
		})
	}
}

func TestSMTPFailuresArePropagated(t *testing.T) {
	fixture := testutil.StartSMTP(t, testutil.SMTPOptions{RejectRecipients: true})
	account := smtpFixtureAccount(t, fixture, securityNone)
	policy, err := outbound.ParsePolicy(account.SMTPHost, "")
	if err != nil {
		t.Fatal(err)
	}
	a := &App{outbound: policy}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := a.testSMTP(ctx, account, "wrong-password"); err == nil {
		t.Fatal("invalid SMTP credentials were accepted")
	}
	raw := []byte("Subject: rejected\r\n\r\nbody")
	if err := a.deliver(ctx, "alice", account, "password", raw, []string{"rejected@example.test"}); err == nil {
		t.Fatal("recipient rejection was ignored")
	}
}
