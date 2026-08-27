package hct

import (
	"math"
	"testing"

	"github.com/anoosa/dar/internal/theme/material/colorutils"
)

func TestHCTConstructionMutationAndComponents(t *testing.T) {
	for hue := -30.0; hue <= 390; hue += 30 {
		for _, chroma := range []float64{0, 10, 40, 80, 120} {
			for _, tone := range []float64{-1, 0, 1, 25, 50, 75, 99, 100, 101} {
				color := From(hue, chroma, tone)
				if color.GetHue() < 0 || color.GetHue() >= 360 {
					t.Fatalf("hue out of range: %v", color.GetHue())
				}
				if color.GetTone() < 0 || color.GetTone() > 100 {
					t.Fatalf("tone out of range: %v", color.GetTone())
				}
				if color.GetChroma() < 0 {
					t.Fatalf("negative chroma: %v", color.GetChroma())
				}
				roundTrip := FromInt(color.ToInt())
				if roundTrip.ToInt() != color.ToInt() {
					t.Fatalf("ARGB round trip changed %#x", color.ToInt())
				}
				alpha, red, green, blue := color.ARGB()
				if alpha != 255 || red != colorutils.RedFromArgb(color.ToInt()) || green != colorutils.GreenFromArgb(color.ToInt()) || blue != colorutils.BlueFromArgb(color.ToInt()) {
					t.Fatalf("components mismatch for %#x", color.ToInt())
				}
			}
		}
	}

	color := From(20, 60, 50)
	color.SetHue(240)
	if math.Abs(color.GetHue()-240) > 20 {
		t.Fatalf("set hue = %v", color.GetHue())
	}
	before := color.GetChroma()
	color.SetChroma(10)
	if color.GetChroma() > before && before > 10 {
		t.Fatalf("set chroma increased unexpectedly: %v -> %v", before, color.GetChroma())
	}
	color.SetTone(80)
	if math.Abs(color.GetTone()-80) > 1 {
		t.Fatalf("set tone = %v", color.GetTone())
	}
}

func TestInternalARGBHelpers(t *testing.T) {
	color := argbFromRgb(1, 2, 3)
	a, r, g, b := argbComponents(color)
	if a != 255 || r != 1 || g != 2 || b != 3 {
		t.Fatalf("components = %d %d %d %d", a, r, g, b)
	}
	for _, tone := range []float64{0, 25, 50, 75, 100} {
		if got := colorutils.LstarFromArgb(argbFromLstar(tone)); math.Abs(got-tone) > 0.6 {
			t.Errorf("tone %v round trip = %v", tone, got)
		}
	}
}

func FuzzHCTNeverEscapesColorBounds(f *testing.F) {
	for _, seed := range [][3]float64{{0, 0, 0}, {360, 100, 50}, {-20, -1, 101}, {200, 40, 80}} {
		f.Add(seed[0], seed[1], seed[2])
	}
	f.Fuzz(func(t *testing.T, hue, chroma, tone float64) {
		if math.IsNaN(hue) || math.IsNaN(chroma) || math.IsNaN(tone) ||
			math.IsInf(hue, 0) || math.IsInf(chroma, 0) || math.IsInf(tone, 0) {
			return
		}
		color := From(hue, chroma, tone)
		a, r, g, b := color.ARGB()
		for _, component := range []int{a, r, g, b} {
			if component < 0 || component > 255 {
				t.Fatalf("component out of bounds: %d", component)
			}
		}
	})
}
