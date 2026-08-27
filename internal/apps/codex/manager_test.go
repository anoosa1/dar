package codex

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"testing"
	"time"
)

func fakeCodex(t *testing.T, version string) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("fake app-server script requires a POSIX shell")
	}
	path := filepath.Join(t.TempDir(), "codex")
	script := `#!/bin/sh
if [ "$1" = "--version" ]; then
  echo "codex-cli ` + version + `"
  exit 0
fi
while IFS= read -r line; do
  case "$line" in
    *\"method\":\"initialize\"*) echo '{"id":1,"result":{"codexHome":"'"$CODEX_HOME"'"}}' ;;
    *\"method\":\"initialized\"*) if [ -n "$DAR_TEST_CODEX_CRASH_ONCE" ] && [ ! -f "$DAR_TEST_CODEX_CRASH_ONCE" ]; then touch "$DAR_TEST_CODEX_CRASH_ONCE"; exit 1; fi ;;
  esac
done
`
	if err := os.WriteFile(path, []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestManagerRemovesCrashedProcessAndRestarts(t *testing.T) {
	marker := filepath.Join(t.TempDir(), "crashed")
	t.Setenv("DAR_TEST_CODEX_CRASH_ONCE", marker)
	m := newProcessManager(managerConfig{Path: fakeCodex(t, minimumCLIVersion), HomeRoot: t.TempDir(), IdleTimeout: time.Hour})
	if err := m.Start(context.Background()); err != nil {
		t.Fatal(err)
	}
	first, err := m.client(context.Background(), "alice", "disabled")
	if err != nil {
		t.Fatal(err)
	}
	select {
	case <-first.exited:
	case <-time.After(2 * time.Second):
		t.Fatal("fake process did not crash")
	}
	second, err := m.client(context.Background(), "alice", "disabled")
	if err != nil {
		t.Fatal(err)
	}
	if first == second {
		t.Fatal("crashed process was reused")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := m.Shutdown(ctx); err != nil {
		t.Fatal(err)
	}
}

func TestManagerVersionAndProtectedPerUserHome(t *testing.T) {
	root := filepath.Join(t.TempDir(), "homes")
	m := newProcessManager(managerConfig{Path: fakeCodex(t, minimumCLIVersion), HomeRoot: root, IdleTimeout: time.Hour})
	if err := m.Start(context.Background()); err != nil {
		t.Fatal(err)
	}
	client, err := m.client(context.Background(), "../../alice", "disabled")
	if err != nil {
		t.Fatal(err)
	}
	again, err := m.client(context.Background(), "../../alice", "disabled")
	if err != nil || again != client {
		t.Fatalf("process was not reused: %p %p, %v", client, again, err)
	}
	home, workspace, err := m.userDirs("../../alice")
	if err != nil {
		t.Fatal(err)
	}
	if filepath.Dir(home) != root || filepath.Dir(workspace) != home {
		t.Fatalf("unsafe directories: %q %q", home, workspace)
	}
	for _, path := range []string{root, home, workspace} {
		info, err := os.Stat(path)
		if err != nil {
			t.Fatal(err)
		}
		if info.Mode().Perm() != 0700 {
			t.Fatalf("%s permissions = %o", path, info.Mode().Perm())
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := m.Shutdown(ctx); err != nil {
		t.Fatal(err)
	}
}

func TestManagerRejectsOldCLI(t *testing.T) {
	m := newProcessManager(managerConfig{Path: fakeCodex(t, "0.143.9"), HomeRoot: t.TempDir()})
	if _, err := m.availability(context.Background()); err == nil {
		t.Fatal("old Codex CLI accepted")
	}
}

func TestManagerCapacityRejectsBusyAndEvictsIdleLRU(t *testing.T) {
	m := newProcessManager(managerConfig{Path: fakeCodex(t, minimumCLIVersion), HomeRoot: t.TempDir(), IdleTimeout: time.Hour, MaxProcesses: 1})
	if err := m.Start(context.Background()); err != nil {
		t.Fatal(err)
	}
	first, err := m.client(context.Background(), "alice", "disabled")
	if err != nil {
		t.Fatal(err)
	}
	m.attach("alice")
	if _, err := m.client(context.Background(), "bob", "disabled"); err != errProcessCapacity {
		t.Fatalf("busy capacity error=%v", err)
	}
	m.detach("alice")
	second, err := m.client(context.Background(), "bob", "disabled")
	if err != nil {
		t.Fatal(err)
	}
	if second == first {
		t.Fatal("idle process was not evicted")
	}
	m.mu.Lock()
	_, aliceExists := m.users["alice"]
	_, bobExists := m.users["bob"]
	m.mu.Unlock()
	if aliceExists || !bobExists {
		t.Fatalf("process map alice=%v bob=%v", aliceExists, bobExists)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := m.Shutdown(ctx); err != nil {
		t.Fatal(err)
	}
}

func TestIsolatedCodexEnvRemovesServerCredentials(t *testing.T) {
	env := isolatedCodexEnv([]string{"PATH=/bin", "CODEX_HOME=/shared", "OPENAI_API_KEY=secret", "CODEX_API_KEY=secret2", "SAFE=value"}, "/private/user")
	want := map[string]bool{"PATH=/bin": true, "SAFE=value": true, "CODEX_HOME=/private/user": true}
	if len(env) != len(want) {
		t.Fatalf("environment = %#v", env)
	}
	for _, entry := range env {
		if !want[entry] {
			t.Fatalf("unexpected inherited environment %q", entry)
		}
	}
}

func TestManagerIdleProtectionAndRestart(t *testing.T) {
	m := newProcessManager(managerConfig{Path: fakeCodex(t, minimumCLIVersion), HomeRoot: t.TempDir(), IdleTimeout: 20 * time.Millisecond})
	if err := m.Start(context.Background()); err != nil {
		t.Fatal(err)
	}
	first, err := m.client(context.Background(), "alice", "disabled")
	if err != nil {
		t.Fatal(err)
	}
	m.beginLogin("alice")
	time.Sleep(1100 * time.Millisecond)
	if got, err := m.client(context.Background(), "alice", "cached"); err != nil || got != first {
		t.Fatalf("busy process restarted: %p %p %v", first, got, err)
	}
	m.finishLogin("alice")
	time.Sleep(1100 * time.Millisecond)
	second, err := m.client(context.Background(), "alice", "cached")
	if err != nil {
		t.Fatal(err)
	}
	if second == first {
		t.Fatal("idle process marked for preference restart was reused")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := m.Shutdown(ctx); err != nil {
		t.Fatal(err)
	}
}
