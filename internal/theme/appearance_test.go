package theme

import (
	"bytes"
	"context"
	"errors"
	"image"
	"image/color"
	"image/gif"
	"image/png"
	"io/fs"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
)

func TestFocusAppearanceAlwaysUsesMochaPink(t *testing.T) {
	resolved := resolveAppearance(Appearance{WallpaperVisibility: 70}, false)
	want := MochaPink()
	if resolved.Active != want || resolved.Mode != "dark" {
		t.Fatalf("resolved appearance=%+v, want fixed dark Mocha", resolved)
	}
	if resolved.Active.Base != "#1e1e2e" || resolved.Active.Primary != "#f5c2e7" {
		t.Fatalf("unexpected Focus colors: base=%s primary=%s", resolved.Active.Base, resolved.Active.Primary)
	}
	css := CSS(resolved, "")
	for _, value := range []string{"color-scheme:dark", "--dar-primary:245 194 231", "--dar-glass-blur:18px"} {
		if !strings.Contains(css, value) {
			t.Errorf("Focus CSS missing %q", value)
		}
	}
}

func wallpaperPNG(t *testing.T, width, height int) []byte {
	t.Helper()
	img := image.NewNRGBA(image.Rect(0, 0, width, height))
	for y := 0; y < height; y++ {
		for x := 0; x < width; x++ {
			img.SetNRGBA(x, y, color.NRGBA{R: 40, G: 130, B: 220, A: 255})
		}
	}
	var encoded bytes.Buffer
	if err := png.Encode(&encoded, img); err != nil {
		t.Fatal(err)
	}
	return encoded.Bytes()
}

func TestValidateWallpaperFormatsAndLimits(t *testing.T) {
	if err := ValidateWallpaper(wallpaperPNG(t, 8, 8)); err != nil {
		t.Fatalf("valid PNG rejected: %v", err)
	}
	palette := color.Palette{color.RGBA{R: 230, G: 40, B: 70, A: 255}}
	frame := image.NewPaletted(image.Rect(0, 0, 3, 3), palette)
	var animated bytes.Buffer
	if err := gif.EncodeAll(&animated, &gif.GIF{Image: []*image.Paletted{frame}, Delay: []int{1}}); err != nil {
		t.Fatal(err)
	}
	if err := ValidateWallpaper(animated.Bytes()); err != nil {
		t.Fatalf("valid GIF rejected: %v", err)
	}
	if err := ValidateWallpaper([]byte("not an image")); err == nil {
		t.Fatal("malformed wallpaper was accepted")
	}
	if err := ValidateWallpaper(make([]byte, maxWallpaperBytes+1)); err == nil {
		t.Fatal("oversized wallpaper was accepted")
	}
	if err := ValidateWallpaper(wallpaperPNG(t, maxWallpaperSide+1, 1)); err == nil {
		t.Fatal("over-dimension wallpaper was accepted")
	}
}

type memoryFiles struct {
	data map[string][]byte
}

func (m memoryFiles) ReadFile(_ context.Context, uid, path string) ([]byte, error) {
	value, ok := m.data[uid+"\x00"+path]
	if !ok {
		return nil, fs.ErrNotExist
	}
	return append([]byte(nil), value...), nil
}

func (m memoryFiles) Stat(_ context.Context, uid, path string) (fs.FileInfo, error) {
	value, ok := m.data[uid+"\x00"+path]
	if !ok {
		return nil, fs.ErrNotExist
	}
	return memoryInfo{size: int64(len(value)), name: filepath.Base(path)}, nil
}

type memoryInfo struct {
	size int64
	name string
	dir  bool
}

func (m memoryInfo) Name() string { return m.name }
func (m memoryInfo) Size() int64  { return m.size }
func (m memoryInfo) Mode() fs.FileMode {
	if m.dir {
		return fs.ModeDir | 0700
	}
	return 0600
}
func (m memoryInfo) ModTime() time.Time { return time.Unix(1, 0) }
func (m memoryInfo) IsDir() bool        { return m.dir }
func (m memoryInfo) Sys() any           { return nil }

type failingFiles struct {
	readData []byte
	readErr  error
	statInfo fs.FileInfo
	statErr  error
}

func (f failingFiles) ReadFile(context.Context, string, string) ([]byte, error) {
	return f.readData, f.readErr
}

func (f failingFiles) Stat(context.Context, string, string) (fs.FileInfo, error) {
	return f.statInfo, f.statErr
}

func TestServicePreviewDoesNotPersistAndSaveDoes(t *testing.T) {
	database, err := db.Open(filepath.Join(t.TempDir(), "appearance.sqlite"), preferences.Migrations())
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()

	service := NewService(database)
	files := memoryFiles{data: map[string][]byte{"alice\x00/Wallpapers/blue.png": wallpaperPNG(t, 8, 8)}}
	service.BindFiles(files)
	draft := Draft{WallpaperPath: "/Wallpapers/blue.png", WallpaperVisibility: 70}

	preview, err := service.Preview(context.Background(), "alice", draft)
	if err != nil || !preview.HasWallpaper || preview.Active != MochaPink() {
		t.Fatalf("preview=%+v err=%v", preview, err)
	}
	if loaded, _ := service.Load(context.Background(), "alice"); loaded != DefaultAppearance() {
		t.Fatalf("preview persisted settings: %+v", loaded)
	}
	saved, err := service.Save(context.Background(), "alice", draft)
	if err != nil {
		t.Fatal(err)
	}
	if loaded, _ := service.Load(context.Background(), "alice"); loaded != (Appearance{WallpaperPath: draft.WallpaperPath, WallpaperVisibility: 70}) {
		t.Fatalf("save did not persist: %+v", loaded)
	}
	if saved.Mode != "dark" || saved.WallpaperVisibility != 70 || saved.Active != MochaPink() {
		t.Fatalf("unexpected resolved save: %+v", saved)
	}

	wallpaper := httptest.NewRecorder()
	service.ServeWallpaper(wallpaper, httptest.NewRequest(http.MethodGet, "/api/appearance/wallpaper", nil), "alice")
	if wallpaper.Code != http.StatusOK || wallpaper.Header().Get("Cache-Control") != "private, no-cache" {
		t.Fatalf("wallpaper response status=%d cache-control=%q", wallpaper.Code, wallpaper.Header().Get("Cache-Control"))
	}
	delete(files.data, "alice\x00/Wallpapers/blue.png")
	missing, err := service.Resolve(context.Background(), "alice")
	if err != nil || missing.HasWallpaper || missing.WallpaperPath != "/Wallpapers/blue.png" {
		t.Fatalf("missing wallpaper did not retain the selected path: %+v err=%v", missing, err)
	}
	if _, err := service.Save(context.Background(), "alice", Draft{WallpaperPath: "/../secret.png", WallpaperVisibility: 70}); err == nil {
		t.Fatal("traversal wallpaper was accepted")
	}
	if loaded, _ := service.Load(context.Background(), "alice"); loaded.WallpaperPath != "/Wallpapers/blue.png" {
		t.Fatalf("failed save partially mutated appearance: %+v", loaded)
	}
}

func TestAppearanceServiceValidationAndFailureMatrix(t *testing.T) {
	for _, visibility := range []int{-1, 101} {
		value := DefaultAppearance()
		value.WallpaperVisibility = visibility
		if err := validateStored(&value); err == nil {
			t.Fatalf("invalid visibility %d was accepted", visibility)
		}
	}
	for _, value := range []string{"relative.png", "/", "/a/../b.png", "/.local/share/Trash/a.png", "/picture.svg", "/bad\x00.png"} {
		if err := validateWallpaperPath(value); err == nil {
			t.Errorf("invalid wallpaper path %q was accepted", value)
		}
	}

	ctx := context.Background()
	database, err := db.Open(filepath.Join(t.TempDir(), "appearance-errors.sqlite"), preferences.Migrations())
	if err != nil {
		t.Fatal(err)
	}
	service := NewService(database)
	if got, err := (*Service)(nil).Load(ctx, "alice"); err != nil || got != DefaultAppearance() {
		t.Fatalf("nil service load=%+v err=%v", got, err)
	}
	if got, err := NewService(nil).Response(ctx, "alice"); err != nil || got.Settings.Active != MochaPink() {
		t.Fatalf("default response=%+v err=%v", got, err)
	}
	if _, _, err := service.ReadWallpaper(ctx, "alice"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("missing wallpaper error=%v", err)
	}
	if saved, err := service.Save(ctx, "alice", Draft{WallpaperVisibility: 50}); err != nil || saved.Mode != "dark" {
		t.Fatalf("wallpaper-free save=%+v err=%v", saved, err)
	}
	if got := FromContext(WithResolved(ctx, Resolved{Mode: "dark"})); got.Mode != "dark" {
		t.Fatalf("context resolved=%+v", got)
	}
	if got := FromContext(ctx); got.Active != MochaPink() {
		t.Fatalf("context default=%+v", got)
	}

	draft := Draft{WallpaperPath: "/wallpaper.png", WallpaperVisibility: 50}
	if _, err := service.Preview(ctx, "alice", draft); err == nil || !strings.Contains(err.Error(), "Files") {
		t.Fatalf("preview without Files error=%v", err)
	}
	service.BindFiles(failingFiles{readErr: fs.ErrNotExist, statErr: fs.ErrNotExist})
	if _, err := service.Preview(ctx, "alice", draft); err == nil || !strings.Contains(err.Error(), "not found") {
		t.Fatalf("missing file preview error=%v", err)
	}
	service.BindFiles(failingFiles{readData: []byte("not an image"), statInfo: memoryInfo{name: "wallpaper.png"}})
	if _, err := service.Preview(ctx, "alice", draft); err == nil {
		t.Fatal("malformed wallpaper was accepted")
	}

	conn, release, err := database.Conn(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if err := preferences.SetJSON(conn, "invalid-user", "global", appearancePreferenceKey, Appearance{WallpaperVisibility: 999}); err != nil {
		release()
		t.Fatal(err)
	}
	release()
	if loaded, err := service.Load(ctx, "invalid-user"); err != nil || loaded != DefaultAppearance() {
		t.Fatalf("invalid stored appearance=%+v err=%v", loaded, err)
	}

	service.BindFiles(failingFiles{readData: []byte("data"), statInfo: memoryInfo{name: "wallpaper.png", dir: true}})
	conn, release, err = database.Conn(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if err := preferences.SetJSON(conn, "directory-user", "global", appearancePreferenceKey, Appearance{WallpaperPath: "/wallpaper.png", WallpaperVisibility: 50}); err != nil {
		release()
		t.Fatal(err)
	}
	release()
	if _, _, err := service.ReadWallpaper(ctx, "directory-user"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("directory wallpaper error=%v", err)
	}

	if err := database.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := service.Load(ctx, "alice"); err == nil {
		t.Fatal("closed database load succeeded")
	}
	if _, err := service.Resolve(ctx, "alice"); err == nil {
		t.Fatal("closed database resolve succeeded")
	}
	if _, err := service.Preview(ctx, "alice", Draft{WallpaperVisibility: 50}); err == nil {
		t.Fatal("closed database preview succeeded")
	}
	if _, err := service.Save(ctx, "alice", Draft{WallpaperVisibility: 50}); err == nil {
		t.Fatal("closed database save succeeded")
	}
	if _, err := service.Response(ctx, "alice"); err == nil || !strings.Contains(err.Error(), "resolve appearance") {
		t.Fatalf("closed database response error=%v", err)
	}
}

func TestStandaloneAppearanceUsesFixedFocusTreatment(t *testing.T) {
	resolved := resolveAppearance(DefaultAppearance(), true)
	css := StandaloneAppearanceCSS(resolved, "/s/token/appearance/wallpaper")
	for _, want := range []string{"html,body{height:100%}", "height:100dvh;overflow:hidden", "overflow-y:auto", "overscroll-behavior:contain", "scrollbar-gutter:stable", "background-attachment:fixed", "--glass-blur:18px", "color-scheme:dark"} {
		if !strings.Contains(css, want) {
			t.Errorf("standalone Focus CSS does not contain %q", want)
		}
	}
	if strings.Contains(css, "min-height:100vh") {
		t.Fatal("standalone CSS reintroduced document viewport scrolling")
	}
}
