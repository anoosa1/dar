package main

import (
	"bytes"
	"context"
	"encoding/json"
	"image"
	"image/color"
	"image/png"
	"io/fs"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/theme"
)

type appearanceTestFiles struct{ data []byte }

func (f appearanceTestFiles) ReadFile(_ context.Context, uid, path string) ([]byte, error) {
	if uid != "alice" || path != "/Wallpapers/test.png" {
		return nil, fs.ErrNotExist
	}
	return f.data, nil
}
func (f appearanceTestFiles) Stat(_ context.Context, uid, path string) (fs.FileInfo, error) {
	if uid != "alice" || path != "/Wallpapers/test.png" {
		return nil, fs.ErrNotExist
	}
	return appearanceTestInfo{size: int64(len(f.data))}, nil
}

type appearanceTestInfo struct{ size int64 }

func (appearanceTestInfo) Name() string       { return "test.png" }
func (i appearanceTestInfo) Size() int64      { return i.size }
func (appearanceTestInfo) Mode() fs.FileMode  { return 0600 }
func (appearanceTestInfo) ModTime() time.Time { return time.Unix(1, 0) }
func (appearanceTestInfo) IsDir() bool        { return false }
func (appearanceTestInfo) Sys() any           { return nil }

func testWallpaperPNG(t *testing.T) []byte {
	t.Helper()
	img := image.NewNRGBA(image.Rect(0, 0, 4, 4))
	for y := 0; y < 4; y++ {
		for x := 0; x < 4; x++ {
			img.SetNRGBA(x, y, color.NRGBA{R: 30, G: 150, B: 190, A: 255})
		}
	}
	var out bytes.Buffer
	if err := png.Encode(&out, img); err != nil {
		t.Fatal(err)
	}
	return out.Bytes()
}

func TestAppearanceEndpointsPreviewAndPersist(t *testing.T) {
	srv := testSettingsServer(t)
	service := theme.NewService(srv.DB)
	service.BindFiles(appearanceTestFiles{data: testWallpaperPNG(t)})

	getRec, getReq := filesSettingsRequest(http.MethodGet, "/api/settings/appearance", nil)
	handleAppearanceSettings(service)(getRec, getReq)
	if getRec.Code != http.StatusOK {
		t.Fatalf("get status=%d body=%s", getRec.Code, getRec.Body.String())
	}
	var response theme.Response
	if err := json.Unmarshal(getRec.Body.Bytes(), &response); err != nil {
		t.Fatal(err)
	}
	if response.Settings.Active != theme.MochaPink() || response.Settings.Mode != "dark" || response.Settings.WallpaperVisibility != 50 {
		t.Fatalf("default response=%+v", response)
	}
	for _, obsolete := range []string{"presets", "color_source", "dynamic_mode", "blur_px"} {
		if strings.Contains(getRec.Body.String(), `"`+obsolete+`"`) {
			t.Fatalf("wallpaper-only response contains obsolete field %q", obsolete)
		}
	}

	draft := theme.Draft{WallpaperPath: "/Wallpapers/test.png", WallpaperVisibility: 75}
	previewRec, previewReq := filesSettingsRequest(http.MethodPost, "/api/settings/appearance/preview", draft)
	handleAppearancePreview(service)(previewRec, previewReq)
	if previewRec.Code != http.StatusOK || !strings.Contains(previewRec.Body.String(), `"has_wallpaper":true`) {
		t.Fatalf("preview status=%d body=%s", previewRec.Code, previewRec.Body.String())
	}
	if loaded, _ := service.Load(context.Background(), "alice"); loaded != theme.DefaultAppearance() {
		t.Fatal("preview persisted appearance")
	}

	saveRec, saveReq := filesSettingsRequest(http.MethodPut, "/api/settings/appearance", draft)
	handleAppearanceSettings(service)(saveRec, saveReq)
	if saveRec.Code != http.StatusOK {
		t.Fatalf("save status=%d body=%s", saveRec.Code, saveRec.Body.String())
	}
	loaded, _ := service.Load(context.Background(), "alice")
	if loaded.WallpaperPath != draft.WallpaperPath || loaded.WallpaperVisibility != 75 {
		t.Fatalf("saved appearance=%+v", loaded)
	}
}

func TestAppearanceSettingsPaneContract(t *testing.T) {
	html := appearancePaneHTML()
	for _, want := range []string{`id="appearance-choose-wallpaper"`, `id="appearance-upload-wallpaper"`, `id="appearance-remove-wallpaper"`, `id="appearance-visibility"`, `/files/assets/picker.js`, `Catppuccin Mocha`} {
		if !strings.Contains(html, want) {
			t.Errorf("appearance pane missing %q", want)
		}
	}
	for _, obsolete := range []string{`appearance-presets`, `appearance-source`, `appearance-dynamic-mode`, `appearance-blur`} {
		if strings.Contains(html, obsolete) {
			t.Errorf("appearance pane contains obsolete control %q", obsolete)
		}
	}
	script := settingsScripts()
	for _, want := range []string{`wallpaper_path`, `wallpaper_visibility`, `--dar-wallpaper-visibility`} {
		if !strings.Contains(script, want) {
			t.Errorf("appearance script missing %q", want)
		}
	}
}

func TestAppearanceRejectsInvalidImage(t *testing.T) {
	srv := testSettingsServer(t)
	service := theme.NewService(srv.DB)
	service.BindFiles(appearanceTestFiles{data: []byte("not an image")})
	rec, req := filesSettingsRequest(http.MethodPut, "/api/settings/appearance", theme.Draft{WallpaperPath: "/Wallpapers/test.png", WallpaperVisibility: 50})
	handleAppearanceSettings(service)(rec, req)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("invalid image status=%d body=%s", rec.Code, rec.Body.String())
	}
}

func TestAppearanceEndpointAuthorizationMethodAndFailureMatrix(t *testing.T) {
	srv := testSettingsServer(t)
	service := theme.NewService(srv.DB)
	service.BindFiles(appearanceTestFiles{data: testWallpaperPNG(t)})

	for _, tc := range []struct {
		handler http.HandlerFunc
		method  string
		body    any
		status  int
		auth    bool
	}{
		{handleAppearanceSettings(service), http.MethodGet, nil, http.StatusUnauthorized, false},
		{handleAppearanceSettings(service), http.MethodDelete, nil, http.StatusMethodNotAllowed, true},
		{handleAppearanceSettings(service), http.MethodPut, []byte("{"), http.StatusBadRequest, true},
		{handleAppearanceSettings(service), http.MethodPut, theme.Draft{WallpaperVisibility: 101}, http.StatusBadRequest, true},
		{handleAppearancePreview(service), http.MethodPost, nil, http.StatusUnauthorized, false},
		{handleAppearancePreview(service), http.MethodPost, []byte("{"), http.StatusBadRequest, true},
		{handleAppearancePreview(service), http.MethodPost, theme.Draft{WallpaperPath: "/missing.png", WallpaperVisibility: 50}, http.StatusBadRequest, true},
		{handleAppearanceWallpaper(service), http.MethodGet, nil, http.StatusNotFound, false},
	} {
		var rec *httptest.ResponseRecorder
		var req *http.Request
		if tc.auth {
			rec, req = filesSettingsRequest(tc.method, "/api/settings/appearance", tc.body)
		} else {
			var body []byte
			if raw, ok := tc.body.([]byte); ok {
				body = raw
			}
			rec = httptest.NewRecorder()
			req = httptest.NewRequest(tc.method, "/api/settings/appearance", bytes.NewReader(body))
		}
		tc.handler(rec, req)
		if rec.Code != tc.status {
			t.Errorf("%s auth=%v status=%d want=%d body=%s", tc.method, tc.auth, rec.Code, tc.status, rec.Body.String())
		}
	}

	draft := theme.Draft{WallpaperPath: "/Wallpapers/test.png", WallpaperVisibility: 50}
	rec, req := filesSettingsRequest(http.MethodPut, "/api/settings/appearance", draft)
	handleAppearanceSettings(service)(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("save wallpaper status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec, req = filesSettingsRequest(http.MethodGet, "/api/settings/appearance/wallpaper", nil)
	handleAppearanceWallpaper(service)(rec, req)
	if rec.Code != http.StatusOK || rec.Header().Get("Content-Type") != "image/png" {
		t.Fatalf("wallpaper status=%d type=%q", rec.Code, rec.Header().Get("Content-Type"))
	}

	if err := srv.DB.Close(); err != nil {
		t.Fatal(err)
	}
	rec, req = filesSettingsRequest(http.MethodGet, "/api/settings/appearance", nil)
	handleAppearanceSettings(service)(rec, req)
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("closed appearance status=%d body=%s", rec.Code, rec.Body.String())
	}
}
