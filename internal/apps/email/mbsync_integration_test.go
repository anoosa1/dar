package email

import (
	"context"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/testutil"
)

func TestRealMBSyncAgainstHermeticIMAP(t *testing.T) {
	mbsync, err := exec.LookPath("mbsync")
	if err != nil {
		t.Fatalf("mbsync is required for the protocol regression suite: %v", err)
	}
	fixture := testutil.StartIMAP(t)
	host, portText, err := net.SplitHostPort(fixture.Address)
	if err != nil {
		t.Fatal(err)
	}
	port, err := strconv.Atoi(portText)
	if err != nil {
		t.Fatal(err)
	}

	_, source, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("resolve repository root")
	}
	root := filepath.Clean(filepath.Join(filepath.Dir(source), "..", "..", ".."))
	tunnel := filepath.Join(t.TempDir(), "dar-mail-tunnel")
	build := exec.Command("go", "build", "-o", tunnel, "./cmd/server")
	build.Dir = root
	if output, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build mail tunnel: %v\n%s", err, output)
	}

	a := testApp(t, testKey(t, "current"))
	a.mbsyncPath = mbsync
	a.tunnelPath = tunnel
	t.Setenv("DAR_OUTBOUND_ALLOWED_HOSTS", host)
	account, err := a.createAccount(context.Background(), "alice", Account{
		Email:        "alice@example.test",
		Name:         "Alice",
		Username:     fixture.Username,
		Password:     fixture.Password,
		IMAPHost:     host,
		IMAPPort:     port,
		IMAPSecurity: securityNone,
		SMTPHost:     host,
		SMTPPort:     2525,
		SMTPSecurity: securityNone,
		Enabled:      false,
		SyncMinutes:  5,
		MaxMessages:  100,
		SaveSent:     true,
	})
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if output, err := a.runMBSync(ctx, "alice", account, false, false); err != nil {
		t.Fatalf("mbsync: %v\n%s\nIMAP transcript:\n%s", err, output, fixture.Log.String())
	}
	matches, err := filepath.Glob(filepath.Join(a.accountRoot("alice", account.ID), "mail", "INBOX", "*", "*"))
	if err != nil {
		t.Fatal(err)
	}
	found := false
	for _, path := range matches {
		info, err := os.Stat(path)
		if err == nil && info.Mode().IsRegular() {
			found = true
			break
		}
	}
	if !found {
		t.Fatalf("mbsync produced no Maildir message under %s", a.accountRoot("alice", account.ID))
	}
}
