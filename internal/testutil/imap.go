package testutil

import (
	"bytes"
	"errors"
	"net"
	"strings"
	"sync"
	"testing"

	"github.com/emersion/go-imap/backend/memory"
	"github.com/emersion/go-imap/server"
)

// IMAPServer is a hermetic RFC 3501 service backed by go-imap's in-memory
// mailbox. Its credentials are intentionally fixed and scoped to the listener.
type IMAPServer struct {
	Address  string
	Username string
	Password string
	Log      *bytes.Buffer

	listener net.Listener
	server   *server.Server
	done     chan error
}

type logoutClosingListener struct{ net.Listener }

func (l logoutClosingListener) Accept() (net.Conn, error) {
	conn, err := l.Listener.Accept()
	if err != nil {
		return nil, err
	}
	return &logoutClosingConn{Conn: conn}, nil
}

type logoutClosingConn struct {
	net.Conn
	mu   sync.Mutex
	tail string
}

func (c *logoutClosingConn) Write(p []byte) (int, error) {
	n, err := c.Conn.Write(p)
	c.mu.Lock()
	c.tail += string(p[:n])
	closeAfterWrite := strings.Contains(c.tail, "LOGOUT completed\r\n")
	if len(c.tail) > 128 {
		c.tail = c.tail[len(c.tail)-128:]
	}
	c.mu.Unlock()
	if err == nil && closeAfterWrite {
		go c.Conn.Close()
	}
	return n, err
}

// StartIMAP starts a local plaintext IMAP server suitable for exercising
// clients through Dar's loopback-only mail tunnel.
func StartIMAP(t testing.TB) *IMAPServer {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	service := server.New(memory.New())
	service.AllowInsecureAuth = true
	debug := &bytes.Buffer{}
	service.Debug = debug
	fixture := &IMAPServer{
		Address:  listener.Addr().String(),
		Username: "username",
		Password: "password",
		Log:      debug,
		listener: listener,
		server:   service,
		done:     make(chan error, 1),
	}
	go func() {
		fixture.done <- service.Serve(logoutClosingListener{Listener: listener})
	}()
	t.Cleanup(func() {
		_ = service.Close()
		err := <-fixture.done
		if err != nil && !errors.Is(err, net.ErrClosed) {
			t.Errorf("IMAP server: %v", err)
		}
	})
	return fixture
}
