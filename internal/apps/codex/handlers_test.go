package codex

import (
	"strings"
	"testing"
)

func TestCodexHTMLUsesDraftFirstChatContract(t *testing.T) {
	page := codexHTML("csrf-token")
	for _, want := range []string{
		`id="codex-notice"`,
		`function createDraft()`,
		`await api('/sessions',{method:'POST'`,
		`data.event_cursor||0`,
		`state.attachments=attachments`,
		`fa-terminal`,
	} {
		if !strings.Contains(page, want) {
			t.Fatalf("Codex page is missing %q", want)
		}
	}
	if strings.Contains(page, "async function createSession()") {
		t.Fatal("Codex page still creates an empty server session from New chat")
	}
}
