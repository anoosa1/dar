package theme

import (
	"fmt"
	"strings"
)

func CSS(resolved Resolved, wallpaperURL string) string {
	if wallpaperURL == "" {
		wallpaperURL = resolved.WallpaperURL
	}
	var b strings.Builder
	b.WriteString(":root{")
	b.WriteString(paletteVariables(resolved.Active))
	writeGlassVariables(&b, resolved)
	b.WriteString("}")
	if resolved.HasWallpaper && wallpaperURL != "" {
		b.WriteString(`:root{--dar-wallpaper-image:url("`)
		b.WriteString(cssURL(wallpaperURL))
		b.WriteString(`");}`)
	}
	return b.String()
}

func writeGlassVariables(b *strings.Builder, resolved Resolved) {
	alpha := 1.0
	if resolved.HasWallpaper {
		// Nextcloud keeps its app chrome near 85% opacity over a visible
		// wallpaper. Keep the useful adjustment range around that reference
		// instead of making every app surface washed out at higher visibility.
		alpha = 0.97 - (float64(resolved.WallpaperVisibility)/100)*0.15
	}
	fmt.Fprintf(b, "--dar-surface-alpha:1;--dar-glass-alpha:%.3f;--dar-glass-blur:18px;--dar-wallpaper-visibility:%.2f;--dar-wallpaper-image:none;", alpha, float64(resolved.WallpaperVisibility)/100)
}

func paletteVariables(p Palette) string {
	values := []struct {
		name, value string
	}{
		{"background", p.Background}, {"on-background", p.OnBackground},
		{"surface", p.Surface}, {"surface-dim", p.SurfaceDim}, {"surface-bright", p.SurfaceBright},
		{"surface-container-lowest", p.SurfaceContainerLowest}, {"surface-container-low", p.SurfaceContainerLow},
		{"surface-container", p.SurfaceContainer}, {"surface-container-high", p.SurfaceContainerHigh},
		{"surface-container-highest", p.SurfaceContainerHighest},
		{"primary", p.Primary}, {"on-primary", p.OnPrimary}, {"primary-container", p.PrimaryContainer},
		{"on-primary-container", p.OnPrimaryContainer}, {"secondary", p.Secondary}, {"on-secondary", p.OnSecondary},
		{"secondary-container", p.SecondaryContainer}, {"on-secondary-container", p.OnSecondaryContainer},
		{"tertiary", p.Tertiary}, {"on-tertiary", p.OnTertiary}, {"tertiary-container", p.TertiaryContainer},
		{"on-tertiary-container", p.OnTertiaryContainer}, {"on-surface", p.OnSurface},
		{"on-surface-variant", p.OnSurfaceVariant}, {"outline", p.Outline}, {"outline-variant", p.OutlineVariant},
		{"error", p.Error}, {"on-error", p.OnError}, {"error-container", p.ErrorContainer},
		{"on-error-container", p.OnErrorContainer}, {"warning", p.Warning}, {"on-warning", p.OnWarning},
		{"warning-container", p.WarningContainer}, {"on-warning-container", p.OnWarningContainer},
		{"success", p.Success}, {"on-success", p.OnSuccess}, {"success-container", p.SuccessContainer},
		{"on-success-container", p.OnSuccessContainer}, {"info", p.Info}, {"on-info", p.OnInfo},
		{"info-container", p.InfoContainer}, {"on-info-container", p.OnInfoContainer},
		{"inverse-surface", p.InverseSurface}, {"inverse-on-surface", p.InverseOnSurface},
		{"inverse-primary", p.InversePrimary}, {"scrim", p.Scrim}, {"shadow", p.Shadow},
		{"pink", p.Pink}, {"mauve", p.Mauve}, {"red", p.Red}, {"peach", p.Peach},
		{"yellow", p.Yellow}, {"green", p.Green}, {"teal", p.Teal}, {"sky", p.Sky}, {"blue", p.Blue},
	}
	var b strings.Builder
	for _, value := range values {
		if value.value == "" {
			value.value = "#000000"
		}
		r, g, blue, _ := hexRGB(value.value)
		fmt.Fprintf(&b, "--dar-%s:%d %d %d;", value.name, r, g, blue)
	}
	if p.Dark {
		b.WriteString("color-scheme:dark;")
	} else {
		b.WriteString("color-scheme:light;")
	}
	return b.String()
}

func cssURL(value string) string {
	value = strings.ReplaceAll(value, `\`, `%5C`)
	value = strings.ReplaceAll(value, `"`, `%22`)
	value = strings.ReplaceAll(value, "\n", "")
	value = strings.ReplaceAll(value, "\r", "")
	return value
}

// StandaloneAppearanceCSS styles app-owned documents that cannot use the
// shared Tailwind shell, including public share and reader pages.
func StandaloneAppearanceCSS(resolved Resolved, wallpaperURL string) string {
	p := resolved.Active
	alpha := 1.0
	backgroundImage := "none"
	if resolved.HasWallpaper && wallpaperURL != "" {
		alpha = 0.97 - (float64(resolved.WallpaperVisibility)/100)*0.15
		backgroundImage = `url("` + cssURL(wallpaperURL) + `")`
	}
	return fmt.Sprintf(`:root{color-scheme:%s;background:%s;color:%s;--glass-blur:%dpx}html,body{height:100%%}*{box-sizing:border-box}body{font:16px/1.6 system-ui,sans-serif;margin:0;height:100dvh;overflow:hidden;display:flex;align-items:flex-start;padding:clamp(1rem,8dvh,4rem) 1rem 1rem;background-color:%s;background-image:%s;background-size:cover;background-position:center;background-attachment:fixed;color:%s}main{width:min(34rem,100%%);max-height:100%%;margin:0 auto;overflow-y:auto;overscroll-behavior:contain;scrollbar-gutter:stable;background:color-mix(in srgb,%s %.1f%%,transparent);border:0;border-radius:1rem;padding:1.5rem;-webkit-backdrop-filter:blur(var(--glass-blur));backdrop-filter:blur(var(--glass-blur));box-shadow:none}h1{margin-top:0;color:%s}p,label{color:%s}a{color:%s}form{display:grid;gap:1rem}input{width:100%%;margin-top:.35rem;padding:.75rem 1rem;border:1px solid %s;border-radius:.75rem;background:%s;color:%s}button{justify-self:start;border:0;border-radius:999px;padding:.7rem 1.2rem;background:%s;color:%s;font:inherit;font-weight:600;cursor:pointer}button:hover{filter:brightness(1.08)}ul{padding-left:1.25rem}`,
		"dark", p.Background, p.OnBackground, 18,
		p.Background, backgroundImage, p.OnBackground, p.Surface, alpha*100,
		p.Primary, p.OnSurfaceVariant, p.Secondary, p.OutlineVariant, p.SurfaceContainer, p.OnSurface,
		p.Primary, p.OnPrimary)
}
