package fullstack

import (
	"bytes"
	"context"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/testutil"
)

type runningStack struct {
	baseURL string
	dar     *exec.Cmd
	oidc    *exec.Cmd
	logs    *bytes.Buffer
	cover   string
}

func repositoryRoot(t *testing.T) string {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("resolve source path")
	}
	return filepath.Clean(filepath.Join(filepath.Dir(file), "..", ".."))
}

func availablePort(t *testing.T) int {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	port := listener.Addr().(*net.TCPAddr).Port
	if err := listener.Close(); err != nil {
		t.Fatal(err)
	}
	return port
}

func startStack(t *testing.T) *runningStack {
	t.Helper()
	root := repositoryRoot(t)
	work := t.TempDir()
	serverBinary := filepath.Join(work, "dar-server")
	oidcBinary := filepath.Join(work, "dar-test-oidc")
	for output, pkg := range map[string]string{serverBinary: "./cmd/server", oidcBinary: "./cmd/test-oidc"} {
		args := []string{"build", "-o", output}
		if pkg == "./cmd/server" {
			args = append(args, "-cover", "-coverpkg=./...")
		}
		args = append(args, pkg)
		command := exec.Command("go", args...)
		command.Dir = root
		if data, err := command.CombinedOutput(); err != nil {
			t.Fatalf("build %s: %v\n%s", pkg, err, data)
		}
	}

	darPort := availablePort(t)
	oidcPort := availablePort(t)
	logs := &bytes.Buffer{}
	oidcCommand := exec.Command(oidcBinary, "--host", "127.0.0.1", "--port", fmt.Sprint(oidcPort))
	oidcCommand.Stdout = logs
	oidcCommand.Stderr = logs
	if err := oidcCommand.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if oidcCommand.Process != nil {
			_ = oidcCommand.Process.Kill()
		}
		_ = oidcCommand.Wait()
	})
	waitHTTP(t, fmt.Sprintf("http://127.0.0.1:%d/health", oidcPort))

	baseURL := fmt.Sprintf("http://127.0.0.1:%d", darPort)
	coverRoot := os.Getenv("DAR_PROCESS_COVERDIR")
	if coverRoot == "" {
		coverRoot = filepath.Join(work, "process-coverage")
	}
	coverDir := coverRoot
	if err := os.MkdirAll(coverDir, 0o700); err != nil {
		t.Fatal(err)
	}
	darCommand := exec.Command(serverBinary, "--port", fmt.Sprint(darPort), "--db", filepath.Join(work, "dar.sqlite"))
	darCommand.Dir = root
	darCommand.Stdout = logs
	darCommand.Stderr = logs
	darCommand.Env = append(os.Environ(),
		"DAR_OIDC_ISSUER="+fmt.Sprintf("http://127.0.0.1:%d", oidcPort),
		"DAR_OIDC_CLIENT_ID=test-client",
		"DAR_OIDC_CLIENT_SECRET=test-secret",
		"DAR_OIDC_DISPLAY_NAME=Test OIDC",
		"DAR_BASE_URL="+baseURL,
		"DAR_DATA_ROOT="+filepath.Join(work, "data"),
		"DAR_OUTBOUND_ALLOWED_HOSTS=127.0.0.1,localhost",
		"DAR_CODEX_PATH="+filepath.Join(root, "internal/apps/codex/testdata/fake-codex"),
		"DAR_NEWS_REFRESH_MINUTES=60",
		"DAR_CALENDAR_REFRESH_MINUTES=60",
		"DAR_EMAIL_SYNC_MINUTES=60",
		"GOCOVERDIR="+coverDir,
	)
	if err := darCommand.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if darCommand.Process != nil {
			_ = darCommand.Process.Signal(os.Interrupt)
		}
		done := make(chan struct{})
		go func() {
			_ = darCommand.Wait()
			close(done)
		}()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			_ = darCommand.Process.Kill()
		}
		if t.Failed() {
			t.Logf("full-stack logs:\n%s", logs.String())
		}
	})
	waitHTTP(t, baseURL+"/health")
	return &runningStack{baseURL: baseURL, dar: darCommand, oidc: oidcCommand, logs: logs, cover: coverDir}
}

func waitHTTP(t *testing.T, target string) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for {
		response, err := (&http.Client{Timeout: 300 * time.Millisecond}).Get(target)
		if err == nil {
			_ = response.Body.Close()
			if response.StatusCode < 500 {
				return
			}
		}
		if time.Now().After(deadline) {
			t.Fatalf("service at %s did not become ready: %v", target, err)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func TestAuthenticatedBrowserPagesAndPrimaryControls(t *testing.T) {
	stack := startStack(t)
	browser := testutil.LaunchBrowser(t)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()

	artifactRoot := os.Getenv("DAR_TEST_ARTIFACTS")
	if artifactRoot == "" {
		artifactRoot = t.TempDir()
	}
	t.Cleanup(func() {
		if t.Failed() {
			_ = os.MkdirAll(artifactRoot, 0o700)
			artifactContext, cancelArtifacts := context.WithTimeout(context.Background(), 2*time.Second)
			defer cancelArtifacts()
			_ = browser.Screenshot(artifactContext, filepath.Join(artifactRoot, "browser-failure.png"))
			if html, err := browser.HTML(artifactContext); err == nil {
				_ = os.WriteFile(filepath.Join(artifactRoot, "browser-failure.html"), []byte(html), 0o600)
			}
		}
	})

	if err := browser.Navigate(ctx, stack.baseURL+"/login"); err != nil {
		t.Fatal(err)
	}
	if err := browser.Click(ctx, `a[href*="/authorize"]`); err != nil {
		t.Fatal(err)
	}
	testutil.Wait(t, 10*time.Second, func() bool {
		var path string
		return browser.Evaluate(context.Background(), "location.pathname", &path) == nil && path == "/home"
	})

	pages := []struct {
		path     string
		root     string
		control  string
		assertJS string
	}{
		{"/activity", "#activity-app", "#activity-settings-open", `document.querySelector("#activity-settings-dialog").open`},
		{"/files", "#files-app", "#files-new-button", `!document.querySelector("#files-new-menu").classList.contains("hidden")`},
		{"/contacts", "#contacts-app", `button[onclick="showCreate()"]`, `document.querySelector("#edit-first") !== null`},
		{"/calendar", "#calendar-app", `button[onclick="openSettings()"]`, `!document.querySelector("#calendar-settings-modal").classList.contains("hidden")`},
		{"/todo", "#todo-app", "#sort-tasks", `document.querySelector("#quick-title") !== null`},
		{"/notes", "#notes-app", "#new-category", `!document.querySelector("#notes-dialog").classList.contains("hidden")`},
		{"/news", "#news-app", "#news-settings-button", `!document.querySelector("#modal-backdrop").classList.contains("hidden")`},
		{"/email", "#email-app", "#settings-button", `!document.querySelector("#modal-backdrop").classList.contains("hidden")`},
		{"/codex", "#codex-app", "#codex-settings", `document.querySelector("#codex-settings-dialog").open`},
	}
	for _, page := range pages {
		t.Run(strings.TrimPrefix(page.path, "/"), func(t *testing.T) {
			if err := browser.Navigate(ctx, stack.baseURL+page.path); err != nil {
				t.Fatal(err)
			}
			var present bool
			if err := browser.Evaluate(ctx, fmt.Sprintf("document.querySelector(%q) !== null", page.root), &present); err != nil || !present {
				t.Fatalf("root %s present=%v error=%v", page.root, present, err)
			}
			if err := browser.Click(ctx, page.control); err != nil {
				t.Fatal(err)
			}
			testutil.Wait(t, 5*time.Second, func() bool {
				actionContext, cancelAction := context.WithTimeout(context.Background(), time.Second)
				defer cancelAction()
				var state bool
				return browser.Evaluate(actionContext, page.assertJS, &state) == nil && state
			})
			tree, err := browser.AccessibilityTree(ctx)
			if err != nil || len(tree) == 0 {
				t.Fatalf("accessibility tree len=%d error=%v", len(tree), err)
			}
		})
	}
	browser.AssertHealthy(t)
}
