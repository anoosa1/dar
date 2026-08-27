package theme

import (
	"bytes"
	"errors"
	"image"
	_ "image/gif"
	_ "image/jpeg"
	_ "image/png"
	"strings"
)

const (
	maxWallpaperBytes  = 20 << 20
	maxWallpaperSide   = 8192
	maxWallpaperPixels = 32_000_000
)

// ValidateWallpaper validates a wallpaper without deriving any colors from it.
func ValidateWallpaper(data []byte) error {
	if len(data) == 0 || len(data) > maxWallpaperBytes {
		return errors.New("wallpaper must be between 1 byte and 20 MiB")
	}
	config, format, err := image.DecodeConfig(bytes.NewReader(data))
	if err != nil || (format != "jpeg" && format != "png" && format != "gif") {
		return errors.New("wallpaper must be JPEG, PNG, or GIF")
	}
	if config.Width <= 0 || config.Height <= 0 || config.Width > maxWallpaperSide || config.Height > maxWallpaperSide || int64(config.Width)*int64(config.Height) > maxWallpaperPixels {
		return errors.New("wallpaper dimensions exceed the 8192px/32MP limit")
	}
	return nil
}

func validImageExtension(name string) bool {
	name = strings.ToLower(strings.TrimSpace(name))
	return strings.HasSuffix(name, ".jpg") || strings.HasSuffix(name, ".jpeg") ||
		strings.HasSuffix(name, ".png") || strings.HasSuffix(name, ".gif")
}
