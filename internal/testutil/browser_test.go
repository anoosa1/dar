package testutil

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestBrowserDrivesDOMUploadsAccessibilityAndScreenshots(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		_, _ = w.Write([]byte(`<!doctype html>
			<html><head><title>CDP fixture</title></head><body>
			<label for="name">Name</label><input id="name">
			<label for="upload">Upload</label><input id="upload" type="file">
			<button id="save" onclick="document.body.dataset.saved = document.querySelector('#name').value">Save</button>
			</body></html>`))
	}))
	defer server.Close()

	browser := LaunchBrowser(t)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := browser.Navigate(ctx, server.URL); err != nil {
		t.Fatal(err)
	}
	if err := browser.Fill(ctx, "#name", "Ada"); err != nil {
		t.Fatal(err)
	}
	if err := browser.Click(ctx, "#save"); err != nil {
		t.Fatal(err)
	}
	var saved string
	if err := browser.Evaluate(ctx, "document.body.dataset.saved", &saved); err != nil {
		t.Fatal(err)
	}
	if saved != "Ada" {
		t.Fatalf("saved = %q", saved)
	}

	upload := filepath.Join(t.TempDir(), "upload.txt")
	if err := os.WriteFile(upload, []byte("fixture"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := browser.Upload(ctx, "#upload", upload); err != nil {
		t.Fatal(err)
	}
	var filename string
	if err := browser.Evaluate(ctx, "document.querySelector('#upload').files[0].name", &filename); err != nil {
		t.Fatal(err)
	}
	if filename != "upload.txt" {
		t.Fatalf("uploaded filename = %q", filename)
	}

	html, err := browser.HTML(ctx)
	if err != nil || !strings.Contains(html, "CDP fixture") {
		t.Fatalf("HTML error=%v body=%q", err, html)
	}
	tree, err := browser.AccessibilityTree(ctx)
	if err != nil || len(tree) == 0 {
		t.Fatalf("accessibility tree len=%d error=%v", len(tree), err)
	}
	screenshot := filepath.Join(t.TempDir(), "page.png")
	if err := browser.Screenshot(ctx, screenshot); err != nil {
		t.Fatal(err)
	}
	if info, err := os.Stat(screenshot); err != nil || info.Size() < 100 {
		t.Fatalf("screenshot info=%v error=%v", info, err)
	}
	browser.AssertHealthy(t)
}

func TestBrowserRecordsConsoleAndRuntimeFailures(t *testing.T) {
	browser := &Browser{}
	browser.recordEvent(cdpMessage{
		Method: "Runtime.consoleAPICalled",
		Params: json.RawMessage(`{"type":"error","args":[{"value":"broken"}]}`),
	})
	browser.recordEvent(cdpMessage{
		Method: "Runtime.exceptionThrown",
		Params: json.RawMessage(`{"exceptionDetails":{"text":"uncaught"}}`),
	})
	browser.recordEvent(cdpMessage{
		Method: "Log.entryAdded",
		Params: json.RawMessage(`{"entry":{"level":"error","text":"network failed"}}`),
	})
	if len(browser.errors) != 3 {
		t.Fatalf("errors = %v", browser.errors)
	}
	if len(browser.logs) != 2 {
		t.Fatalf("logs = %v", browser.logs)
	}
}
