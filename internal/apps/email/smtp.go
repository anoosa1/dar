package email

import (
	"bytes"
	"context"
	"crypto/tls"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"mime"
	"mime/multipart"
	"mime/quotedprintable"
	"net"
	"net/mail"
	"net/smtp"
	"net/textproto"
	"path/filepath"
	"strings"
	"time"
)

type loginAuth struct{ username, password string }

func (a loginAuth) Start(server *smtp.ServerInfo) (string, []byte, error) {
	if !server.TLS && !isLoopbackHost(server.Name) {
		return "", nil, errors.New("LOGIN requires TLS")
	}
	return "LOGIN", []byte(a.username), nil
}
func (a loginAuth) Next(fromServer []byte, more bool) ([]byte, error) {
	if !more {
		return nil, nil
	}
	prompt := strings.ToLower(string(fromServer))
	if decoded, err := base64.StdEncoding.DecodeString(string(fromServer)); err == nil {
		prompt = strings.ToLower(string(decoded))
	}
	if strings.Contains(prompt, "user") {
		return []byte(a.username), nil
	}
	if strings.Contains(prompt, "pass") {
		return []byte(a.password), nil
	}
	return nil, errors.New("unexpected SMTP LOGIN challenge")
}

func (a *App) smtpTLS(host string) *tls.Config {
	if a.smtpTLSConfig != nil {
		return a.smtpTLSConfig(host)
	}
	return &tls.Config{ServerName: host, MinVersion: tls.VersionTLS12}
}

func (a *App) smtpClient(ctx context.Context, acct Account) (*smtp.Client, net.Conn, error) {
	addr := net.JoinHostPort(acct.SMTPHost, fmt.Sprint(acct.SMTPPort))
	var conn net.Conn
	var err error
	if acct.SMTPSecurity == securityTLS {
		plain, dialErr := a.outbound.DialContext(ctx, "tcp", addr)
		if dialErr != nil {
			return nil, nil, dialErr
		}
		tlsConn := tls.Client(plain, a.smtpTLS(acct.SMTPHost))
		err = tlsConn.HandshakeContext(ctx)
		if err != nil {
			_ = plain.Close()
			return nil, nil, err
		}
		conn = tlsConn
	} else {
		conn, err = a.outbound.DialContext(ctx, "tcp", addr)
	}
	if err != nil {
		return nil, nil, err
	}
	deadline := time.Now().Add(30 * time.Second)
	if d, ok := ctx.Deadline(); ok && d.Before(deadline) {
		deadline = d
	}
	_ = conn.SetDeadline(deadline)
	client, err := smtp.NewClient(conn, acct.SMTPHost)
	if err != nil {
		conn.Close()
		return nil, nil, err
	}
	if acct.SMTPSecurity == securitySTARTTLS {
		ok, _ := client.Extension("STARTTLS")
		if !ok {
			client.Close()
			conn.Close()
			return nil, nil, errors.New("SMTP server does not offer STARTTLS")
		}
		if err := client.StartTLS(a.smtpTLS(acct.SMTPHost)); err != nil {
			client.Close()
			conn.Close()
			return nil, nil, err
		}
	}
	return client, conn, nil
}

func authenticateSMTP(client *smtp.Client, acct Account, password string) error {
	ok, authLine := client.Extension("AUTH")
	if !ok {
		return errors.New("SMTP server does not offer authentication")
	}
	upper := strings.ToUpper(authLine)
	var auth smtp.Auth
	if strings.Contains(upper, "PLAIN") {
		auth = smtp.PlainAuth("", acct.Username, password, acct.SMTPHost)
	} else if strings.Contains(upper, "LOGIN") {
		auth = loginAuth{acct.Username, password}
	} else {
		return errors.New("SMTP server offers no supported authentication method")
	}
	return client.Auth(auth)
}

func (a *App) testSMTP(ctx context.Context, acct Account, password string) error {
	if acct.SMTPSecurity == securityNone && !isLoopbackHost(acct.SMTPHost) {
		return errors.New("plaintext SMTP is allowed only for loopback hosts")
	}
	client, conn, err := a.smtpClient(ctx, acct)
	if err != nil {
		return err
	}
	defer conn.Close()
	defer client.Close()
	if err := authenticateSMTP(client, acct, password); err != nil {
		return err
	}
	if err := client.Reset(); err != nil {
		return err
	}
	return client.Quit()
}
func isLoopbackHost(host string) bool {
	if strings.EqualFold(host, "localhost") {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

func formatAddressList(raw string) ([]string, string, error) {
	if strings.TrimSpace(raw) == "" {
		return nil, "", nil
	}
	list, err := mail.ParseAddressList(raw)
	if err != nil {
		return nil, "", err
	}
	var envelope, header []string
	for _, x := range list {
		if !validHeaderValue(x.Address) || !strings.Contains(x.Address, "@") {
			return nil, "", errors.New("invalid recipient")
		}
		envelope = append(envelope, x.Address)
		if x.Name != "" {
			x.Name = mime.QEncoding.Encode("utf-8", x.Name)
		}
		header = append(header, x.String())
	}
	return envelope, strings.Join(header, ", "), nil
}

type outgoingAttachment struct {
	Filename, MIMEType string
	Data               []byte
	ContentID          string
	Inline             bool
}
type outgoingMessage struct {
	FromName, FromEmail, To, Cc, Bcc, Subject, Body, MessageID, InReplyTo, References string
	Attachments                                                                       []outgoingAttachment
	IncludeBcc                                                                        bool
	Draft                                                                             bool
}

func buildMessage(m outgoingMessage) ([]byte, []string, error) {
	if !validHeaderValue(m.Subject) || !validHeaderValue(m.MessageID) || !validHeaderValue(m.InReplyTo) || !validHeaderValue(m.References) {
		return nil, nil, errors.New("invalid message headers")
	}
	toEnv, toHeader, err := formatAddressList(m.To)
	if err != nil {
		return nil, nil, err
	}
	ccEnv, ccHeader, err := formatAddressList(m.Cc)
	if err != nil {
		return nil, nil, err
	}
	bccEnv, bccHeader, err := formatAddressList(m.Bcc)
	if err != nil {
		return nil, nil, err
	}
	recipients := append(append(toEnv, ccEnv...), bccEnv...)
	if !m.Draft && len(recipients) == 0 {
		return nil, nil, errors.New("at least one recipient is required")
	}
	from := (&mail.Address{Name: m.FromName, Address: m.FromEmail}).String()
	var out bytes.Buffer
	write := func(k, v string) {
		if v != "" {
			fmt.Fprintf(&out, "%s: %s\r\n", k, v)
		}
	}
	write("From", from)
	write("To", toHeader)
	write("Cc", ccHeader)
	if m.IncludeBcc {
		write("Bcc", bccHeader)
	}
	write("Date", time.Now().Format(time.RFC1123Z))
	write("Message-ID", formatMessageID(m.MessageID))
	write("In-Reply-To", m.InReplyTo)
	write("References", m.References)
	write("Subject", mime.QEncoding.Encode("utf-8", m.Subject))
	write("MIME-Version", "1.0")
	if len(m.Attachments) == 0 {
		write("Content-Type", `text/plain; charset="UTF-8"`)
		write("Content-Transfer-Encoding", "quoted-printable")
		out.WriteString("\r\n")
		qw := quotedprintable.NewWriter(&out)
		_, _ = io.WriteString(qw, m.Body)
		_ = qw.Close()
		return out.Bytes(), recipients, nil
	}
	mw := multipart.NewWriter(&out)
	write("Content-Type", `multipart/mixed; boundary="`+mw.Boundary()+`"`)
	out.WriteString("\r\n")
	h := textproto.MIMEHeader{}
	h.Set("Content-Type", `text/plain; charset="UTF-8"`)
	h.Set("Content-Transfer-Encoding", "quoted-printable")
	part, _ := mw.CreatePart(h)
	qw := quotedprintable.NewWriter(part)
	_, _ = io.WriteString(qw, m.Body)
	_ = qw.Close()
	for _, att := range m.Attachments {
		h = textproto.MIMEHeader{}
		typ := att.MIMEType
		if typ == "" {
			typ = "application/octet-stream"
		}
		h.Set("Content-Type", typ)
		disp := "attachment"
		if att.Inline {
			disp = "inline"
		}
		h.Set("Content-Disposition", disp+`; filename="`+quoteFilename(filepath.Base(att.Filename))+`"`)
		h.Set("Content-Transfer-Encoding", "base64")
		if att.ContentID != "" {
			h.Set("Content-ID", "<"+strings.Trim(att.ContentID, "<>")+">")
		}
		p, _ := mw.CreatePart(h)
		if err := writeBase64Lines(p, att.Data); err != nil {
			return nil, nil, err
		}
	}
	_ = mw.Close()
	return out.Bytes(), recipients, nil
}

func (a *App) deliver(ctx context.Context, uid string, acct Account, password string, raw []byte, recipients []string) error {
	client, conn, err := a.smtpClient(ctx, acct)
	if err != nil {
		return err
	}
	defer conn.Close()
	defer client.Close()
	if err := authenticateSMTP(client, acct, password); err != nil {
		return err
	}
	if err := client.Mail(acct.Email); err != nil {
		return err
	}
	for _, r := range recipients {
		if err := client.Rcpt(r); err != nil {
			return err
		}
	}
	w, err := client.Data()
	if err != nil {
		return err
	}
	if _, err = w.Write(raw); err != nil {
		return err
	}
	if err = w.Close(); err != nil {
		return err
	}
	return client.Quit()
}
