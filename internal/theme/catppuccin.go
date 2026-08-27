// Package theme defines Dar's application color palette.
package theme

import "strings"

// Palette contains the Catppuccin colors and Dar's semantic color mapping.
type Palette struct {
	ID        string
	Name      string
	Family    string
	Dark      bool
	Rosewater string
	Flamingo  string
	Pink      string
	Mauve     string
	Red       string
	Maroon    string
	Peach     string
	Yellow    string
	Green     string
	Teal      string
	Sky       string
	Sapphire  string
	Blue      string
	Lavender  string
	Text      string
	Subtext1  string
	Subtext0  string
	Overlay2  string
	Overlay1  string
	Overlay0  string
	Surface2  string
	Surface1  string
	Surface0  string
	Base      string
	Mantle    string
	Crust     string

	Surface                 string
	SurfaceContainerLow     string
	SurfaceContainer        string
	SurfaceContainerHigh    string
	SurfaceContainerHighest string
	Primary                 string
	OnPrimary               string
	PrimaryContainer        string
	OnPrimaryContainer      string
	Secondary               string
	OnSecondary             string
	SecondaryContainer      string
	OnSecondaryContainer    string
	Tertiary                string
	OnTertiary              string
	TertiaryContainer       string
	OnTertiaryContainer     string
	Background              string
	OnBackground            string
	OnSurface               string
	OnSurfaceVariant        string
	Outline                 string
	OutlineVariant          string
	Error                   string
	OnError                 string
	ErrorContainer          string
	OnErrorContainer        string
	Warning                 string
	OnWarning               string
	WarningContainer        string
	OnWarningContainer      string
	Success                 string
	OnSuccess               string
	SuccessContainer        string
	OnSuccessContainer      string
	Info                    string
	OnInfo                  string
	InfoContainer           string
	OnInfoContainer         string
	InverseSurface          string
	InverseOnSurface        string
	InversePrimary          string
	SurfaceDim              string
	SurfaceBright           string
	SurfaceContainerLowest  string
	Scrim                   string
	Shadow                  string
}

// MochaPink returns Catppuccin Mocha with Pink as Dar's primary accent.
func MochaPink() Palette {
	return completePalette(Palette{
		ID:        "catppuccin-mocha",
		Name:      "Catppuccin Mocha",
		Family:    "Catppuccin",
		Dark:      true,
		Rosewater: "#f5e0dc",
		Flamingo:  "#f2cdcd",
		Pink:      "#f5c2e7",
		Mauve:     "#cba6f7",
		Red:       "#f38ba8",
		Maroon:    "#eba0ac",
		Peach:     "#fab387",
		Yellow:    "#f9e2af",
		Green:     "#a6e3a1",
		Teal:      "#94e2d5",
		Sky:       "#89dceb",
		Sapphire:  "#74c7ec",
		Blue:      "#89b4fa",
		Lavender:  "#b4befe",
		Text:      "#cdd6f4",
		Subtext1:  "#bac2de",
		Subtext0:  "#a6adc8",
		Overlay2:  "#9399b2",
		Overlay1:  "#7f849c",
		Overlay0:  "#6c7086",
		Surface2:  "#585b70",
		Surface1:  "#45475a",
		Surface0:  "#313244",
		Base:      "#1e1e2e",
		Mantle:    "#181825",
		Crust:     "#11111b",
	})
}

// StandaloneCSS returns the dependency-free base styling used by app-owned
// documents that cannot use Dar's Tailwind page shell.
func (p Palette) StandaloneCSS() string {
	replacer := strings.NewReplacer(
		"{{base}}", p.Base,
		"{{mantle}}", p.Mantle,
		"{{surface0}}", p.Surface0,
		"{{surface1}}", p.Surface1,
		"{{text}}", p.Text,
		"{{subtext}}", p.Subtext0,
		"{{outline}}", p.Overlay0,
		"{{pink}}", p.Pink,
		"{{blue}}", p.Blue,
	)
	return replacer.Replace(`:root{color-scheme:dark;background:{{base}};color:{{text}}}*{box-sizing:border-box}body{font:16px/1.6 system-ui,sans-serif;margin:0;padding:1rem;background:{{base}};color:{{text}}}main{width:min(34rem,100%);margin:8vh auto;background:{{mantle}};border:1px solid {{outline}};border-radius:1rem;padding:1.5rem}h1{margin-top:0;color:{{pink}}}p,label{color:{{subtext}}}a{color:{{blue}}}form{display:grid;gap:1rem}input{width:100%;margin-top:.35rem;padding:.75rem 1rem;border:1px solid {{outline}};border-radius:.75rem;background:{{surface0}};color:{{text}}}button{justify-self:start;border:0;border-radius:999px;padding:.7rem 1.2rem;background:{{pink}};color:{{base}};font:inherit;font-weight:600;cursor:pointer}button:hover{background:{{surface1}};color:{{pink}}}ul{padding-left:1.25rem}`)
}
