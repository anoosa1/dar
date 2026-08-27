package theme

import (
	"fmt"
	"math"
	"strconv"
	"strings"
)

func completePalette(p Palette) Palette {
	if p.Surface == "" {
		p.Surface = p.Base
	}
	if p.Background == "" {
		p.Background = p.Base
	}
	p.OnBackground = p.Text
	p.SurfaceContainerLowest = p.Crust
	p.SurfaceContainerLow = p.Mantle
	p.SurfaceContainer = p.Surface0
	p.SurfaceContainerHigh = p.Surface1
	p.SurfaceContainerHighest = p.Surface2
	p.SurfaceDim = p.Crust
	p.SurfaceBright = p.Surface2
	p.OnSurface = p.Text
	p.OnSurfaceVariant = p.Subtext0
	p.Outline = p.Overlay1
	p.OutlineVariant = p.Overlay0
	if p.Primary == "" {
		p.Primary = p.Pink
	}
	p.OnPrimary = contrastText(p.Primary)
	p.PrimaryContainer = mixHex(p.Primary, p.Base, 0.24)
	p.OnPrimaryContainer = contrastText(p.PrimaryContainer)
	if p.Secondary == "" {
		p.Secondary = p.Blue
	}
	p.OnSecondary = contrastText(p.Secondary)
	p.SecondaryContainer = mixHex(p.Secondary, p.Base, 0.24)
	p.OnSecondaryContainer = contrastText(p.SecondaryContainer)
	if p.Tertiary == "" {
		p.Tertiary = p.Mauve
	}
	p.OnTertiary = contrastText(p.Tertiary)
	p.TertiaryContainer = mixHex(p.Tertiary, p.Base, 0.24)
	p.OnTertiaryContainer = contrastText(p.TertiaryContainer)
	p.Error = p.Red
	p.OnError = contrastText(p.Error)
	p.ErrorContainer = mixHex(p.Error, p.Base, 0.24)
	p.OnErrorContainer = contrastText(p.ErrorContainer)
	p.Warning = p.Yellow
	p.OnWarning = contrastText(p.Warning)
	p.WarningContainer = mixHex(p.Warning, p.Base, 0.24)
	p.OnWarningContainer = contrastText(p.WarningContainer)
	p.Success = p.Green
	p.OnSuccess = contrastText(p.Success)
	p.SuccessContainer = mixHex(p.Success, p.Base, 0.24)
	p.OnSuccessContainer = contrastText(p.SuccessContainer)
	p.Info = p.Blue
	p.OnInfo = contrastText(p.Info)
	p.InfoContainer = mixHex(p.Info, p.Base, 0.24)
	p.OnInfoContainer = contrastText(p.InfoContainer)
	p.Scrim, p.Shadow = "#000000", "#000000"
	p.InverseSurface = p.Text
	p.InverseOnSurface = p.Base
	p.InversePrimary = p.Mauve
	return p
}

func hexRGB(value string) (int, int, int, error) {
	value = strings.TrimPrefix(strings.TrimSpace(value), "#")
	if len(value) != 6 {
		return 0, 0, 0, fmt.Errorf("invalid color")
	}
	n, err := strconv.ParseUint(value, 16, 32)
	if err != nil {
		return 0, 0, 0, err
	}
	return int(n >> 16), int((n >> 8) & 255), int(n & 255), nil
}

func mixHex(foreground, background string, amount float64) string {
	fr, fg, fb, _ := hexRGB(foreground)
	br, bg, bb, _ := hexRGB(background)
	channel := func(a, b int) int { return int(float64(a)*amount + float64(b)*(1-amount) + 0.5) }
	return fmt.Sprintf("#%02x%02x%02x", channel(fr, br), channel(fg, bg), channel(fb, bb))
}

func contrastText(background string) string {
	if contrastRatioColors("#000000", background) >= contrastRatioColors("#ffffff", background) {
		return "#000000"
	}
	return "#ffffff"
}

func contrastRatioColors(foreground, background string) float64 {
	luminance := func(value string) float64 {
		r, g, b, _ := hexRGB(value)
		channel := func(v int) float64 {
			x := float64(v) / 255
			if x <= 0.04045 {
				return x / 12.92
			}
			return math.Pow((x+0.055)/1.055, 2.4)
		}
		return 0.2126*channel(r) + 0.7152*channel(g) + 0.0722*channel(b)
	}
	a, b := luminance(foreground), luminance(background)
	if a < b {
		a, b = b, a
	}
	return (a + 0.05) / (b + 0.05)
}
