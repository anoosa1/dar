package theme

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"path"
	"strings"

	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
)

const appearancePreferenceKey = "appearance"

type FileReader interface {
	ReadFile(context.Context, string, string) ([]byte, error)
	Stat(context.Context, string, string) (fs.FileInfo, error)
}

type Appearance struct {
	WallpaperPath       string `json:"wallpaper_path"`
	WallpaperVisibility int    `json:"wallpaper_visibility"`
}

type Draft struct {
	WallpaperPath       string `json:"wallpaper_path"`
	WallpaperVisibility int    `json:"wallpaper_visibility"`
}

type Resolved struct {
	Appearance
	Active             Palette `json:"active"`
	Mode               string  `json:"mode"`
	HasWallpaper       bool    `json:"has_wallpaper"`
	WallpaperURL       string  `json:"wallpaper_url"`
	PublicWallpaperURL string  `json:"-"`
}

type Response struct {
	Settings Resolved `json:"settings"`
}

type Service struct {
	db    *db.DB
	files FileReader
}

func NewService(database *db.DB) *Service {
	return &Service{db: database}
}

func (s *Service) BindFiles(files FileReader) {
	s.files = files
}

func DefaultAppearance() Appearance {
	return Appearance{WallpaperVisibility: 50}
}

func (s *Service) Load(ctx context.Context, userID string) (Appearance, error) {
	out := DefaultAppearance()
	if s == nil || s.db == nil || strings.TrimSpace(userID) == "" {
		return out, nil
	}
	conn, release, err := s.db.Conn(ctx)
	if err != nil {
		return out, err
	}
	defer release()
	ok, err := preferences.GetJSON(conn, userID, "global", appearancePreferenceKey, &out)
	if err != nil {
		return DefaultAppearance(), err
	}
	if !ok {
		return DefaultAppearance(), nil
	}
	if err := validateStored(&out); err != nil {
		return DefaultAppearance(), nil
	}
	return out, nil
}

func (s *Service) Resolve(ctx context.Context, userID string) (Resolved, error) {
	appearance, err := s.Load(ctx, userID)
	if err != nil {
		return resolveAppearance(DefaultAppearance(), false), err
	}
	hasWallpaper := false
	if appearance.WallpaperPath != "" && s.files != nil {
		_, statErr := s.files.Stat(ctx, userID, appearance.WallpaperPath)
		hasWallpaper = statErr == nil
	}
	return resolveAppearance(appearance, hasWallpaper), nil
}

func (s *Service) Preview(ctx context.Context, userID string, draft Draft) (Resolved, error) {
	current, err := s.Load(ctx, userID)
	if err != nil {
		return Resolved{}, err
	}
	next, hasWallpaper, err := s.prepare(ctx, userID, current, draft)
	if err != nil {
		return Resolved{}, err
	}
	return resolveAppearance(next, hasWallpaper), nil
}

func (s *Service) Save(ctx context.Context, userID string, draft Draft) (Resolved, error) {
	current, err := s.Load(ctx, userID)
	if err != nil {
		return Resolved{}, err
	}
	next, hasWallpaper, err := s.prepare(ctx, userID, current, draft)
	if err != nil {
		return Resolved{}, err
	}
	conn, release, err := s.db.Conn(ctx)
	if err != nil {
		return Resolved{}, err
	}
	defer release()
	if err := preferences.SetJSON(conn, userID, "global", appearancePreferenceKey, next); err != nil {
		return Resolved{}, err
	}
	return resolveAppearance(next, hasWallpaper), nil
}

func (s *Service) prepare(ctx context.Context, userID string, _ Appearance, draft Draft) (Appearance, bool, error) {
	next := Appearance{
		WallpaperPath:       strings.TrimSpace(draft.WallpaperPath),
		WallpaperVisibility: draft.WallpaperVisibility,
	}
	if err := validateStored(&next); err != nil {
		return Appearance{}, false, err
	}
	hasWallpaper := false
	if next.WallpaperPath != "" {
		if s.files == nil {
			return Appearance{}, false, errors.New("Files is not available")
		}
		if err := validateWallpaperPath(next.WallpaperPath); err != nil {
			return Appearance{}, false, err
		}
		data, err := s.files.ReadFile(ctx, userID, next.WallpaperPath)
		if err != nil {
			return Appearance{}, false, errors.New("wallpaper file was not found")
		}
		if err := ValidateWallpaper(data); err != nil {
			return Appearance{}, false, err
		}
		hasWallpaper = true
	}
	return next, hasWallpaper, nil
}

func validateStored(value *Appearance) error {
	if value.WallpaperVisibility < 0 || value.WallpaperVisibility > 100 {
		return errors.New("wallpaper visibility must be between 0 and 100")
	}
	if value.WallpaperPath != "" {
		return validateWallpaperPath(value.WallpaperPath)
	}
	return nil
}

func validateWallpaperPath(value string) error {
	if !strings.HasPrefix(value, "/") || strings.ContainsRune(value, '\x00') || path.Clean(value) != value ||
		value == "/" || strings.HasPrefix(value, "/.local/share/Trash") || !validImageExtension(value) {
		return errors.New("wallpaper must be a JPEG, PNG, or GIF in Files")
	}
	return nil
}

func resolveAppearance(value Appearance, hasWallpaper bool) Resolved {
	resolved := Resolved{
		Appearance: value, Active: MochaPink(), Mode: "dark",
		HasWallpaper: hasWallpaper,
	}
	if hasWallpaper {
		resolved.WallpaperURL = "/api/appearance/wallpaper"
	}
	return resolved
}

func (s *Service) ReadWallpaper(ctx context.Context, userID string) ([]byte, fs.FileInfo, error) {
	appearance, err := s.Load(ctx, userID)
	if err != nil {
		return nil, nil, err
	}
	if appearance.WallpaperPath == "" || s.files == nil {
		return nil, nil, fs.ErrNotExist
	}
	info, err := s.files.Stat(ctx, userID, appearance.WallpaperPath)
	if err != nil || info.IsDir() {
		return nil, nil, fs.ErrNotExist
	}
	data, err := s.files.ReadFile(ctx, userID, appearance.WallpaperPath)
	return data, info, err
}

func (s *Service) ServeWallpaper(w http.ResponseWriter, r *http.Request, userID string) {
	data, info, err := s.ReadWallpaper(r.Context(), userID)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	// The route is stable while the selected Files path can change. Require
	// revalidation so a newly saved wallpaper is never hidden by an older
	// memory-cache entry at the same per-user URL.
	w.Header().Set("Cache-Control", "private, no-cache")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	http.ServeContent(w, r, info.Name(), info.ModTime(), bytes.NewReader(data))
}

type contextKey struct{}

func WithResolved(ctx context.Context, resolved Resolved) context.Context {
	return context.WithValue(ctx, contextKey{}, resolved)
}

func FromContext(ctx context.Context) Resolved {
	if value, ok := ctx.Value(contextKey{}).(Resolved); ok {
		return value
	}
	return resolveAppearance(DefaultAppearance(), false)
}

func (s *Service) Response(ctx context.Context, userID string) (Response, error) {
	resolved, err := s.Resolve(ctx, userID)
	if err != nil {
		return Response{}, fmt.Errorf("resolve appearance: %w", err)
	}
	return Response{Settings: resolved}, nil
}
