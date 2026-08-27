package theme

import (
	"fmt"
	"math"
	"strconv"
	"testing"
)

func TestMochaPinkPalette(t *testing.T) {
	p := MochaPink()
	for name, got := range map[string]string{
		"base":              p.Base,
		"text":              p.Text,
		"pink":              p.Pink,
		"primary":           p.Primary,
		"primary container": p.PrimaryContainer,
		"error":             p.Error,
		"warning":           p.Warning,
	} {
		if got == "" {
			t.Fatalf("%s is empty", name)
		}
	}
	if p.Base != "#1e1e2e" || p.Pink != "#f5c2e7" || p.PrimaryContainer != "#52455a" {
		t.Fatalf("unexpected Mocha Pink palette: %#v", p)
	}
}

func TestMochaPinkSemanticContrast(t *testing.T) {
	p := MochaPink()
	for _, pair := range []struct {
		name       string
		foreground string
		background string
		minimum    float64
	}{
		{"body", p.OnSurface, p.Surface, 4.5},
		{"secondary text", p.OnSurfaceVariant, p.Surface, 4.5},
		{"raised surface", p.OnSurface, p.SurfaceContainerHighest, 4.5},
		{"primary button", p.OnPrimary, p.Primary, 4.5},
		{"selected item", p.Primary, p.PrimaryContainer, 4.5},
		{"error", p.Error, p.Surface, 4.5},
	} {
		if ratio := contrastRatio(pair.foreground, pair.background); ratio < pair.minimum {
			t.Errorf("%s contrast %.2f is below %.2f", pair.name, ratio, pair.minimum)
		}
	}
}

func contrastRatio(foreground, background string) float64 {
	a := relativeLuminance(foreground)
	b := relativeLuminance(background)
	return (math.Max(a, b) + 0.05) / (math.Min(a, b) + 0.05)
}

func relativeLuminance(value string) float64 {
	if len(value) != 7 || value[0] != '#' {
		panic(fmt.Sprintf("invalid color %q", value))
	}
	channel := func(offset int) float64 {
		n, err := strconv.ParseUint(value[offset:offset+2], 16, 8)
		if err != nil {
			panic(err)
		}
		v := float64(n) / 255
		if v <= 0.04045 {
			return v / 12.92
		}
		return math.Pow((v+0.055)/1.055, 2.4)
	}
	return 0.2126*channel(1) + 0.7152*channel(3) + 0.0722*channel(5)
}
