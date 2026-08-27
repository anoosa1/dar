// Copyright 2021 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is provided "AS IS", WITHOUT WARRANTIES
// OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Modifications Copyright 2025 Torben Schinke
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is provided "AS IS", WITHOUT WARRANTIES
// OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package cam16

import (
	"github.com/anoosa/dar/internal/theme/material/colorutils"
	"github.com/anoosa/dar/internal/theme/material/mathutils"
	"math"
)

// ViewingConditions models an observers measurement of a color.
// In traditional color spaces, a color can be identified solely by the observer's measurement of
// the color. Color appearance models such as CAM16 also use information about the environment where
// the color was observed, known as the viewing conditions.
//
// For example, white under the traditional assumption of a midday sun white point is accurately
// measured as a slightly chromatic blue by CAM16. (roughly, hue 203, chroma 3, lightness 100)
//
// This class caches intermediate values of the CAM16 conversion process that depend only on
// viewing conditions, enabling speed ups.
type ViewingConditions struct {
	Aw     float64
	Nbb    float64
	Ncb    float64
	C      float64
	Nc     float64
	N      float64
	RgbD   []float64
	Fl     float64
	FlRoot float64
	Z      float64
}

// DefaultViewingConditions is the sRGB-like viewing conditions.
var DefaultViewingConditions = ViewingConditionsDefaultWithBackgroundLstar(50.0)

func (vc *ViewingConditions) GetAw() float64 {
	return vc.Aw
}

func (vc *ViewingConditions) GetN() float64 {
	return vc.N
}

func (vc *ViewingConditions) GetNbb() float64 {
	return vc.Nbb
}

func (vc *ViewingConditions) GetNcb() float64 {
	return vc.Ncb
}

func (vc *ViewingConditions) GetC() float64 {
	return vc.C
}

func (vc *ViewingConditions) GetNc() float64 {
	return vc.Nc
}

func (vc *ViewingConditions) GetRgbD() []float64 {
	return vc.RgbD
}

func (vc *ViewingConditions) GetFl() float64 {
	return vc.Fl
}

func (vc *ViewingConditions) GetFlRoot() float64 {
	return vc.FlRoot
}

func (vc *ViewingConditions) GetZ() float64 {
	return vc.Z
}

// MakeViewingConditions creates ViewingConditions from a simple, physically relevant, set of parameters.
//
//   - whitePoint White point, measured in the XYZ color space. default = D65, or sunny day afternoon
//   - adaptingLuminance The luminance of the adapting field. Informally, how bright it is in the room where the color is viewed. Can be calculated from lux by multiplying lux by 0.0586. default = 11.72, or 200 lux.
//   - backgroundLstar The lightness of the area surrounding the color. measured by L* in L*a*b*. default = 50.0
//   - surround A general description of the lighting surrounding the color. 0 is pitch dark, like watching a movie in a theater. 1.0 is a dimly light room, like watching TV at home at night. 2.0 means there is no difference between the lighting on the color and around it. default = 2.0
//   - discountingIlluminant Whether the eye accounts for the tint of the ambient lighting, such as knowing an apple is still red in green light. default = false, the eye does not perform this process on self-luminous objects like displays.
func MakeViewingConditions(
	whitePoint []float64,
	adaptingLuminance float64,
	backgroundLstar float64,
	surround float64,
	discountingIlluminant bool) *ViewingConditions {

	backgroundLstar = math.Max(0.1, backgroundLstar)
	matrix := XyzToCam16rgb
	xyz := whitePoint
	rW := (xyz[0] * matrix[0][0]) + (xyz[1] * matrix[0][1]) + (xyz[2] * matrix[0][2])
	gW := (xyz[0] * matrix[1][0]) + (xyz[1] * matrix[1][1]) + (xyz[2] * matrix[1][2])
	bW := (xyz[0] * matrix[2][0]) + (xyz[1] * matrix[2][1]) + (xyz[2] * matrix[2][2])
	f := 0.8 + (surround / 10.0)
	c := 0.0
	if f >= 0.9 {
		c = mathutils.Lerp(0.59, 0.69, ((f - 0.9) * 10.0))
	} else {
		c = mathutils.Lerp(0.525, 0.59, ((f - 0.8) * 10.0))
	}
	d := 0.0
	if discountingIlluminant {
		d = 1.0
	} else {
		d = f * (1.0 - ((1.0 / 3.6) * math.Exp((-adaptingLuminance-42.0)/92.0)))
	}
	d = mathutils.ClampDouble(0.0, 1.0, d)
	nc := f
	rgbD := []float64{
		d*(100.0/rW) + 1.0 - d, d*(100.0/gW) + 1.0 - d, d*(100.0/bW) + 1.0 - d,
	}
	k := 1.0 / (5.0*adaptingLuminance + 1.0)
	k4 := k * k * k * k
	k4F := 1.0 - k4
	fl := (k4 * adaptingLuminance) + (0.1 * k4F * k4F * math.Cbrt(5.0*adaptingLuminance))
	n := colorutils.YFromLstar(backgroundLstar) / whitePoint[1]
	z := 1.48 + math.Sqrt(n)
	nbb := 0.725 / math.Pow(n, 0.2)
	ncb := nbb
	rgbAFactors := []float64{
		math.Pow(fl*rgbD[0]*rW/100.0, 0.42),
		math.Pow(fl*rgbD[1]*gW/100.0, 0.42),
		math.Pow(fl*rgbD[2]*bW/100.0, 0.42),
	}

	rgbA := []float64{
		(400.0 * rgbAFactors[0]) / (rgbAFactors[0] + 27.13),
		(400.0 * rgbAFactors[1]) / (rgbAFactors[1] + 27.13),
		(400.0 * rgbAFactors[2]) / (rgbAFactors[2] + 27.13),
	}

	aw := ((2.0 * rgbA[0]) + rgbA[1] + (0.05 * rgbA[2])) * nbb
	return &ViewingConditions{
		N:      n,
		Aw:     aw,
		Nbb:    nbb,
		Ncb:    ncb,
		C:      c,
		Nc:     nc,
		RgbD:   rgbD,
		Fl:     fl,
		FlRoot: math.Pow(fl, 0.25),
		Z:      z,
	}
}

// ViewingConditionsDefaultWithBackgroundLstar creates sRGB-like viewing conditions with a custom background lstar.
//
// Default viewing a lstar of 50, midgray.
func ViewingConditionsDefaultWithBackgroundLstar(lstar float64) *ViewingConditions {
	return MakeViewingConditions(
		colorutils.WhitePointD65,
		(200.0 / math.Pi * colorutils.YFromLstar(50.0) / 100.0),
		lstar,
		2.0,
		false,
	)
}
