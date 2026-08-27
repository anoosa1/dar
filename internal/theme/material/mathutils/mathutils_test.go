package mathutils

import (
	"math"
	"testing"
)

func closeTo(t *testing.T, got, want float64) {
	t.Helper()
	if math.Abs(got-want) > 1e-9 {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestScalarUtilities(t *testing.T) {
	for _, tc := range []struct {
		in   float64
		want int
	}{{-2, -1}, {0, 0}, {3, 1}} {
		if got := Signum(tc.in); got != tc.want {
			t.Errorf("Signum(%v) = %d, want %d", tc.in, got, tc.want)
		}
	}
	closeTo(t, Lerp(10, 20, 0), 10)
	closeTo(t, Lerp(10, 20, 0.5), 15)
	closeTo(t, Lerp(10, 20, 1), 20)
	for _, tc := range []struct {
		input int
		want  int
	}{{-1, 0}, {5, 5}, {11, 10}} {
		if got := ClampInt(0, 10, tc.input); got != tc.want {
			t.Errorf("ClampInt input %d = %d, want %d", tc.input, got, tc.want)
		}
	}
	for _, tc := range []struct {
		input float64
		want  float64
	}{{-1, 0}, {5, 5}, {11, 10}} {
		closeTo(t, ClampDouble(0, 10, tc.input), tc.want)
	}
}

func TestAngleUtilities(t *testing.T) {
	for _, tc := range []struct {
		input int
		want  int
	}{{0, 0}, {360, 0}, {721, 1}, {-1, 359}, {-721, 359}} {
		if got := SanitizeDegreesInt(tc.input); got != tc.want {
			t.Errorf("SanitizeDegreesInt(%d) = %d, want %d", tc.input, got, tc.want)
		}
	}
	for _, tc := range []struct {
		input float64
		want  float64
	}{{0, 0}, {360, 0}, {721.5, 1.5}, {-1.5, 358.5}} {
		closeTo(t, SanitizeDegreesDouble(tc.input), tc.want)
	}
	closeTo(t, RotationDirection(10, 190), 1)
	closeTo(t, RotationDirection(10, 191), -1)
	closeTo(t, DifferenceDegrees(10, 350), 20)
	closeTo(t, DifferenceDegrees(0, 180), 180)
	closeTo(t, Degrees(math.Pi), 180)
	closeTo(t, Radians(180), math.Pi)
}

func TestMatrixAndToleranceUtilities(t *testing.T) {
	matrix := [3][3]float64{{1, 2, 3}, {4, 5, 6}, {7, 8, 9}}
	if got := MatrixMultiply([3]float64{1, 2, 3}, matrix); got != [3]float64{14, 32, 50} {
		t.Fatalf("matrix result = %v", got)
	}
	ToBeCloseTo(1.001, 1, 2)
	defer func() {
		if recover() == nil {
			t.Fatal("ToBeCloseTo did not panic outside tolerance")
		}
	}()
	ToBeCloseTo(1.1, 1, 3)
}
