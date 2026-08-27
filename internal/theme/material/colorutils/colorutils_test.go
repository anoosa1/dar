package colorutils

import (
	"math"
	"testing"
)

func near(t *testing.T, got, want, tolerance float64) {
	t.Helper()
	if math.Abs(got-want) > tolerance {
		t.Fatalf("got %.8f, want %.8f ± %.8f", got, want, tolerance)
	}
}

func TestARGBComponentsAndOpacity(t *testing.T) {
	color := ArgbFromRgb(0x12, 0x34, 0x56)
	if AlphaFromArgb(color) != 255 || RedFromArgb(color) != 0x12 || GreenFromArgb(color) != 0x34 || BlueFromArgb(color) != 0x56 || !IsOpaque(color) {
		t.Fatalf("components for %#x were incorrect", color)
	}
	if IsOpaque(0x7f123456) {
		t.Fatal("translucent color reported opaque")
	}
	if got := ArgbFromRgb(0x112, -1, 0x256); RedFromArgb(got) != 0x12 || GreenFromArgb(got) != 0xff || BlueFromArgb(got) != 0x56 {
		t.Fatalf("components were not masked: %#x", got)
	}
}

func TestLinearXYZAndLabRoundTrips(t *testing.T) {
	for _, component := range []int{0, 1, 10, 127, 254, 255} {
		if got := Delinearized(Linearized(component)); math.Abs(float64(got-component)) > 1 {
			t.Errorf("component %d round trip = %d", component, got)
		}
	}
	if Delinearized(-10) != 0 || Delinearized(200) != 255 {
		t.Fatal("delinearized values were not clamped")
	}
	for _, color := range []int{
		ArgbFromRgb(0, 0, 0),
		ArgbFromRgb(255, 255, 255),
		ArgbFromRgb(255, 0, 0),
		ArgbFromRgb(0, 255, 0),
		ArgbFromRgb(0, 0, 255),
		ArgbFromRgb(103, 80, 164),
	} {
		xyz := XyzFromArgb(color)
		if got := ArgbFromXyz(xyz[0], xyz[1], xyz[2]); channelDistance(got, color) > 3 {
			t.Errorf("XYZ round trip %#x -> %v -> %#x", color, xyz, got)
		}
		lab := LabFromArgb(color)
		if got := ArgbFromLab(lab[0], lab[1], lab[2]); channelDistance(got, color) > 3 {
			t.Errorf("Lab round trip %#x -> %v -> %#x", color, lab, got)
		}
		near(t, LstarFromY(YFromLstar(lab[0])), lab[0], 1e-7)
	}
}

func TestLstarAndMatrixHelpers(t *testing.T) {
	for _, tone := range []float64{0, 1, 8, 50, 99, 100} {
		color := ArgbFromLstar(tone)
		near(t, LstarFromArgb(color), tone, 0.6)
	}
	vector := []float64{1, 2, 3}
	identity := [][]float64{{1, 0, 0}, {0, 1, 0}, {0, 0, 1}}
	got := MatrixMultiply(vector, identity)
	if got[0] != 1 || got[1] != 2 || got[2] != 3 {
		t.Fatalf("identity multiply = %v", got)
	}
	if LabInvf(1) != 1 || LabInvf(0) >= 0 || LabF(1) != 1 || LabF(0) <= 0 {
		t.Fatalf("Lab branch helpers returned unexpected values")
	}
	lin := []float64{Linearized(12), Linearized(34), Linearized(56)}
	if got := ArgbFromLinrgb(lin); channelDistance(got, ArgbFromRgb(12, 34, 56)) > 1 {
		t.Fatalf("linear RGB round trip = %#x", got)
	}
}

func channelDistance(a, b int) int {
	abs := func(value int) int {
		if value < 0 {
			return -value
		}
		return value
	}
	return abs(RedFromArgb(a)-RedFromArgb(b)) +
		abs(GreenFromArgb(a)-GreenFromArgb(b)) +
		abs(BlueFromArgb(a)-BlueFromArgb(b))
}
