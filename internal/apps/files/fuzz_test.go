package files

import (
	"path/filepath"
	"strings"
	"testing"
)

func FuzzUserPathConfinement(f *testing.F) {
	for _, seed := range []string{"docs/report.txt", "../escape", "/absolute", `..\windows`, "a\x00b", ""} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, raw string) {
		if len(raw) > 64<<10 {
			return
		}
		root := filepath.Join(string(filepath.Separator), "tmp", "dar-fuzz-root")
		app := &App{}
		joined := app.safeJoin(root, raw)
		relative, err := filepath.Rel(root, joined)
		if err != nil {
			return
		}
		if relative == ".." || strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
			t.Fatalf("path escaped root: raw=%q joined=%q", raw, joined)
		}
		_ = app.cleanVPath(raw)
		_, _ = editorPath(app, raw)
	})
}
