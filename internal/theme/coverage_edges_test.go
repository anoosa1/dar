package theme

import (
	"strings"
	"testing"
)

func TestFixedFocusCSS(t *testing.T) {
	resolved := Resolved{
		Appearance: Appearance{WallpaperVisibility: 80},
		Active:     MochaPink(), HasWallpaper: true, WallpaperURL: `/wallpaper?name="quoted"`,
	}
	css := CSS(resolved, "")
	for _, needle := range []string{
		`color-scheme:dark`, `--dar-wallpaper-image:url`, `--dar-glass-blur:18px`,
	} {
		if !strings.Contains(css, needle) {
			t.Fatalf("Focus CSS missing %q", needle)
		}
	}
	for _, obsolete := range []string{`data-theme-mode="light"`, `prefers-color-scheme:light`, `--dar-glass-blur:12px`} {
		if strings.Contains(css, obsolete) {
			t.Fatalf("Focus CSS contains obsolete dynamic rule %q", obsolete)
		}
	}
	if css := CSS(Resolved{Active: MochaPink()}, ""); strings.Contains(css, `url("`) {
		t.Fatalf("wallpaper-free CSS has wallpaper rules: %s", css)
	}
}
