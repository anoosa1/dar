package cam16

import (
	"math"
	"testing"

	"github.com/anoosa/dar/internal/theme/material/colorutils"
)

func camNear(t *testing.T, got, want, tolerance float64) {
	t.Helper()
	if math.Abs(got-want) > tolerance {
		t.Fatalf("got %.8f, want %.8f ± %.8f", got, want, tolerance)
	}
}

func TestCAM16RoundTripsAndConstructors(t *testing.T) {
	for _, color := range []int{
		colorutils.ArgbFromRgb(0, 0, 0),
		colorutils.ArgbFromRgb(255, 255, 255),
		colorutils.ArgbFromRgb(255, 0, 0),
		colorutils.ArgbFromRgb(0, 255, 0),
		colorutils.ArgbFromRgb(0, 0, 255),
		colorutils.ArgbFromRgb(103, 80, 164),
	} {
		cam := FromInt(color)
		if cam.GetHue() < 0 || cam.GetHue() >= 360 || cam.GetChroma() < 0 || cam.GetJ() < 0 || cam.GetJ() > 100.001 {
			t.Fatalf("invalid CAM16 for %#x: %#v", color, cam)
		}
		if got := cam.ToInt(); colorDistance(got, color) > 5 {
			t.Errorf("default round trip %#x -> %#x", color, got)
		}
		xyz := cam.xyzInViewingConditions(DefaultViewingConditions, nil)
		if len(xyz) != 3 {
			t.Fatalf("xyz length = %d", len(xyz))
		}
		buffer := make([]float64, 3)
		if got := cam.xyzInViewingConditions(DefaultViewingConditions, buffer); &got[0] != &buffer[0] {
			t.Fatal("xyz did not reuse supplied buffer")
		}
		fromXYZ := FromXyzInViewingConditions(xyz[0], xyz[1], xyz[2], DefaultViewingConditions)
		camNear(t, fromXYZ.GetHue(), cam.GetHue(), 1e-3)
		camNear(t, fromXYZ.GetChroma(), cam.GetChroma(), 1e-3)

		jch := fromJch(cam.j, cam.chroma, cam.hue)
		camNear(t, jch.GetHue(), cam.GetHue(), 1e-7)
		ucs := fromUcs(cam.jstar, cam.astar, cam.bstar)
		camNear(t, ucs.GetHue(), cam.GetHue(), 1e-3)
		if cam.distance(cam) != 0 {
			t.Fatal("self distance was not zero")
		}
	}

	explicit := NewCam16(10, 20, 30, 40, 50, 60, 70, 80, 90)
	if explicit.GetHue() != 10 || explicit.GetChroma() != 20 || explicit.GetJ() != 30 {
		t.Fatalf("explicit CAM16 = %#v", explicit)
	}
}

func TestCAM16CustomViewingConditions(t *testing.T) {
	dim := MakeViewingConditions(colorutils.WhitePointD65, 10, 20, 0.5, false)
	discounted := MakeViewingConditions(colorutils.WhitePointD65, 200, 0, 2, true)
	for _, conditions := range []*ViewingConditions{dim, discounted, ViewingConditionsDefaultWithBackgroundLstar(80)} {
		if conditions.GetAw() <= 0 || conditions.GetN() <= 0 || conditions.GetNbb() <= 0 ||
			conditions.GetNcb() <= 0 || conditions.GetC() <= 0 || conditions.GetNc() <= 0 ||
			conditions.GetFl() <= 0 || conditions.GetFlRoot() <= 0 || conditions.GetZ() <= 0 {
			t.Fatalf("invalid viewing conditions: %#v", conditions)
		}
		rgbD := conditions.GetRgbD()
		if len(rgbD) != 3 {
			t.Fatalf("rgbD = %v", rgbD)
		}

		cam := fromIntInViewingConditions(colorutils.ArgbFromRgb(20, 100, 200), conditions)
		if got := cam.Viewed(conditions); colorDistance(got, colorutils.ArgbFromRgb(20, 100, 200)) > 8 {
			t.Errorf("custom viewing round trip = %#x", got)
		}
		jch := fromJchInViewingConditions(cam.j, cam.chroma, cam.hue, conditions)
		if jch.GetHue() < 0 || jch.GetHue() >= 360 {
			t.Fatalf("JCh hue = %v", jch.GetHue())
		}
		ucs := fromUcsInViewingConditions(cam.jstar, cam.astar, cam.bstar, conditions)
		if ucs.GetChroma() < 0 {
			t.Fatalf("UCS chroma = %v", ucs.GetChroma())
		}
	}
}

func colorDistance(a, b int) int {
	abs := func(value int) int {
		if value < 0 {
			return -value
		}
		return value
	}
	return abs(colorutils.RedFromArgb(a)-colorutils.RedFromArgb(b)) +
		abs(colorutils.GreenFromArgb(a)-colorutils.GreenFromArgb(b)) +
		abs(colorutils.BlueFromArgb(a)-colorutils.BlueFromArgb(b))
}
