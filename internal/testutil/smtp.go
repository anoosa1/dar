package testutil

import (
	"bufio"
	"crypto/rand"
	"crypto/rsa"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"math/big"
	"net"
	"net/textproto"
	"strings"
	"sync"
	"testing"
	"time"
)

type SMTPOptions struct {
	Auth             string
	Username         string
	Password         string
	ImplicitTLS      bool
	STARTTLS         bool
	RejectRecipients bool
}

type SMTPMessage struct {
	From       string
	Recipients []string
	Data       []byte
}

type SMTPServer struct {
	Address   string
	ClientTLS *tls.Config

	options  SMTPOptions
	listener net.Listener
	wg       sync.WaitGroup
	mu       sync.Mutex
	messages []SMTPMessage
	errs     []error
}

func StartSMTP(t testing.TB, options SMTPOptions) *SMTPServer {
	t.Helper()
	if options.Auth == "" {
		options.Auth = "PLAIN"
	}
	if options.Username == "" {
		options.Username = "username"
	}
	if options.Password == "" {
		options.Password = "password"
	}
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	var serverTLS *tls.Config
	var clientTLS *tls.Config
	if options.ImplicitTLS || options.STARTTLS {
		serverTLS, clientTLS = localTLS(t)
	}
	if options.ImplicitTLS {
		listener = tls.NewListener(listener, serverTLS)
	}
	fixture := &SMTPServer{Address: listener.Addr().String(), ClientTLS: clientTLS, options: options, listener: listener}
	fixture.wg.Add(1)
	go func() {
		defer fixture.wg.Done()
		for {
			conn, err := listener.Accept()
			if err != nil {
				if !errors.Is(err, net.ErrClosed) {
					fixture.recordError(err)
				}
				return
			}
			fixture.wg.Add(1)
			go func() {
				defer fixture.wg.Done()
				fixture.serve(conn, serverTLS)
			}()
		}
	}()
	t.Cleanup(func() {
		_ = listener.Close()
		fixture.wg.Wait()
		fixture.mu.Lock()
		defer fixture.mu.Unlock()
		for _, err := range fixture.errs {
			t.Errorf("SMTP fixture: %v", err)
		}
	})
	return fixture
}

func (s *SMTPServer) Messages() []SMTPMessage {
	s.mu.Lock()
	defer s.mu.Unlock()
	result := make([]SMTPMessage, len(s.messages))
	copy(result, s.messages)
	return result
}

func (s *SMTPServer) recordError(err error) {
	s.mu.Lock()
	s.errs = append(s.errs, err)
	s.mu.Unlock()
}

func (s *SMTPServer) serve(conn net.Conn, serverTLS *tls.Config) {
	defer conn.Close()
	reader := bufio.NewReader(conn)
	writer := bufio.NewWriter(conn)
	write := func(format string, args ...any) error {
		if _, err := fmt.Fprintf(writer, format+"\r\n", args...); err != nil {
			return err
		}
		return writer.Flush()
	}
	if err := write("220 localhost ESMTP Dar test fixture"); err != nil {
		return
	}
	authenticated := false
	from := ""
	var recipients []string
	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			if !errors.Is(err, io.EOF) {
				s.recordError(err)
			}
			return
		}
		line = strings.TrimSpace(line)
		command, rest, _ := strings.Cut(line, " ")
		switch strings.ToUpper(command) {
		case "EHLO", "HELO":
			_ = write("250-localhost")
			if s.options.STARTTLS {
				_ = write("250-STARTTLS")
			}
			_ = write("250 AUTH %s", strings.ToUpper(s.options.Auth))
		case "STARTTLS":
			if !s.options.STARTTLS {
				_ = write("454 TLS unavailable")
				continue
			}
			if err := write("220 Ready to start TLS"); err != nil {
				return
			}
			tlsConn := tls.Server(conn, serverTLS)
			if err := tlsConn.Handshake(); err != nil {
				s.recordError(err)
				return
			}
			conn = tlsConn
			reader = bufio.NewReader(conn)
			writer = bufio.NewWriter(conn)
		case "AUTH":
			ok, err := s.authenticate(reader, write, rest)
			if err != nil {
				s.recordError(err)
				return
			}
			authenticated = ok
		case "MAIL":
			if !authenticated {
				_ = write("530 Authentication required")
			} else {
				from = strings.TrimPrefix(rest, "FROM:")
				recipients = nil
				_ = write("250 Sender accepted")
			}
		case "RCPT":
			if s.options.RejectRecipients {
				_ = write("550 Recipient rejected")
			} else {
				recipients = append(recipients, strings.TrimPrefix(rest, "TO:"))
				_ = write("250 Recipient accepted")
			}
		case "DATA":
			if len(recipients) == 0 {
				_ = write("554 No recipients")
				continue
			}
			if err := write("354 End data with <CR><LF>.<CR><LF>"); err != nil {
				return
			}
			data, err := io.ReadAll(textproto.NewReader(reader).DotReader())
			if err != nil {
				s.recordError(err)
				return
			}
			s.mu.Lock()
			s.messages = append(s.messages, SMTPMessage{From: from, Recipients: append([]string(nil), recipients...), Data: data})
			s.mu.Unlock()
			_ = write("250 Message accepted")
		case "RSET":
			from = ""
			recipients = nil
			_ = write("250 Reset")
		case "QUIT":
			_ = write("221 Bye")
			return
		default:
			_ = write("502 Command not implemented")
		}
	}
}

func (s *SMTPServer) authenticate(reader *bufio.Reader, write func(string, ...any) error, rest string) (bool, error) {
	mechanism, initial, _ := strings.Cut(rest, " ")
	var username, password string
	switch strings.ToUpper(mechanism) {
	case "PLAIN":
		if initial == "" {
			if err := write("334"); err != nil {
				return false, err
			}
			line, err := reader.ReadString('\n')
			if err != nil {
				return false, err
			}
			initial = strings.TrimSpace(line)
		}
		raw, err := base64.StdEncoding.DecodeString(initial)
		if err != nil {
			_ = write("535 Invalid credentials")
			return false, nil
		}
		parts := strings.Split(string(raw), "\x00")
		if len(parts) == 3 {
			username, password = parts[1], parts[2]
		}
	case "LOGIN":
		if initial == "" {
			if err := write("334 VXNlcm5hbWU6"); err != nil {
				return false, err
			}
			line, err := reader.ReadString('\n')
			if err != nil {
				return false, err
			}
			initial = strings.TrimSpace(line)
		}
		rawUser, err := base64.StdEncoding.DecodeString(initial)
		if err != nil {
			return false, err
		}
		username = string(rawUser)
		if err := write("334 UGFzc3dvcmQ6"); err != nil {
			return false, err
		}
		line, err := reader.ReadString('\n')
		if err != nil {
			return false, err
		}
		rawPassword, err := base64.StdEncoding.DecodeString(strings.TrimSpace(line))
		if err != nil {
			return false, err
		}
		password = string(rawPassword)
	default:
		_ = write("504 Unsupported authentication")
		return false, nil
	}
	if username != s.options.Username || password != s.options.Password {
		_ = write("535 Invalid credentials")
		return false, nil
	}
	_ = write("235 Authentication successful")
	return true, nil
}

func localTLS(t testing.TB) (*tls.Config, *tls.Config) {
	t.Helper()
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatal(err)
	}
	template := &x509.Certificate{
		SerialNumber: big.NewInt(1),
		Subject:      pkix.Name{CommonName: "localhost"},
		NotBefore:    time.Now().Add(-time.Hour),
		NotAfter:     time.Now().Add(time.Hour),
		KeyUsage:     x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment | x509.KeyUsageCertSign,
		ExtKeyUsage:  []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		IsCA:         true,
		DNSNames:     []string{"localhost"},
		IPAddresses:  []net.IP{net.ParseIP("127.0.0.1")},
	}
	der, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		t.Fatal(err)
	}
	certificate, err := tls.X509KeyPair(
		pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}),
		pem.EncodeToMemory(&pem.Block{Type: "RSA PRIVATE KEY", Bytes: x509.MarshalPKCS1PrivateKey(key)}),
	)
	if err != nil {
		t.Fatal(err)
	}
	roots := x509.NewCertPool()
	parsed, err := x509.ParseCertificate(der)
	if err != nil {
		t.Fatal(err)
	}
	roots.AddCert(parsed)
	return &tls.Config{Certificates: []tls.Certificate{certificate}, MinVersion: tls.VersionTLS12},
		&tls.Config{RootCAs: roots, ServerName: "127.0.0.1", MinVersion: tls.VersionTLS12}
}
