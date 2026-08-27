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

// CAM16 is a color appearance model. Colors are not just defined by their hex code, but rather, a hex
// code and viewing conditions.
//
// CAM16 instances also have coordinates in the CAM16-UCS space, called J*, a*, b*, or jstar,
// astar, bstar in code. CAM16-UCS is included in the CAM16 specification, and should be used when
// measuring distances between colors.
//
// In traditional color spaces, a color can be identified solely by the observer's measurement of
// the color. Color appearance models such as CAM16 also use information about the environment where
// the color was observed, known as the viewing conditions.
//
// For example, white under the traditional assumption of a midday sun white point is accurately
// measured as a slightly chromatic blue by CAM16. (roughly, hue 203, chroma 3, lightness 100)
type CAM16 struct {

	// CAM16 color dimensions.
	hue    float64
	chroma float64
	j      float64
	q      float64
	m      float64
	s      float64
	// Coordinates in UCS space. Used to determine color distance, like delta E equations in L*a*b*.
	jstar float64
	astar float64
	bstar float64
	// Avoid allocations during conversion by pre-allocating an array.
	tempArray []float64
}

func NewCam16(
	hue float64,
	chroma float64,
	j float64,
	q float64,
	m float64,
	s float64,
	jstar float64,
	astar float64,
	bstar float64) *CAM16 {
	return &CAM16{
		hue:    hue,
		chroma: chroma,
		j:      j,
		q:      q,
		m:      m,
		s:      s,
		jstar:  jstar,
		astar:  astar,
		bstar:  bstar,
	}
}

// XyzToCam16rgb transforms XYZ color space coordinates to 'cone'/'RGB' responses in CAM16.
var XyzToCam16rgb = [][]float64{
	{0.401288, 0.650173, -0.051461},
	{-0.250268, 1.204414, 0.045854},
	{-0.002079, 0.048952, 0.953127},
}

// Cam16rgbToXyz transforms 'cone'/'RGB' responses in CAM16 to XYZ color space coordinates.
var Cam16rgbToXyz = [][]float64{
	{1.8620678, -1.0112547, 0.14918678},
	{0.38752654, 0.62144744, -0.00897398},
	{-0.01584150, -0.03412294, 1.0499644},
}

// distance calculates the distance between two CAM16 colors in the CAM16-UCS space.
//
// other: The other CAM16 color to compare with.
// returns: The distance between the two colors.
func (c *CAM16) distance(other *CAM16) float64 {
	dJ := c.jstar - other.jstar
	dA := c.astar - other.astar
	dB := c.bstar - other.bstar
	dEPrime := math.Sqrt(dJ*dJ + dA*dA + dB*dB)
	dE := 1.41 * math.Pow(dEPrime, 0.63)
	return dE
}

// FromInt creates a CAM16 color from a color, assuming the color was viewed in default viewing conditions.
//
// argb: ARGB representation of a color.
// returns: A CAM16 color.
func FromInt(argb int) *CAM16 {
	return fromIntInViewingConditions(argb, DefaultViewingConditions)
}

// fromIntInViewingConditions creates a CAM16 color from a color in defined viewing conditions.
//
// argb: ARGB representation of a color.
// viewingConditions: Information about the environment where the color was observed.
// returns: A CAM16 color.
func fromIntInViewingConditions(argb int, viewingConditions *ViewingConditions) *CAM16 {
	// Transform ARGB int to XYZ
	red := (argb & 0x00ff0000) >> 16
	green := (argb & 0x0000ff00) >> 8
	blue := argb & 0x000000ff
	redL := colorutils.Linearized(red)
	greenL := colorutils.Linearized(green)
	blueL := colorutils.Linearized(blue)
	x := 0.41233895*redL + 0.35762064*greenL + 0.18051042*blueL
	y := 0.2126*redL + 0.7152*greenL + 0.0722*blueL
	z := 0.01932141*redL + 0.11916382*greenL + 0.95034478*blueL

	return FromXyzInViewingConditions(x, y, z, viewingConditions)
}

// FromXyzInViewingConditions creates a CAM16 color from XYZ coordinates in specific viewing conditions.
//
// x: The X coordinate.
// y: The Y coordinate.
// z: The Z coordinate.
// viewingConditions: Information about the environment where the color was observed.
// returns: A CAM16 color.
func FromXyzInViewingConditions(x, y, z float64, viewingConditions *ViewingConditions) *CAM16 {
	// Transform XYZ to 'cone'/'rgb' responses
	matrix := XyzToCam16rgb
	rT := (x * matrix[0][0]) + (y * matrix[0][1]) + (z * matrix[0][2])
	gT := (x * matrix[1][0]) + (y * matrix[1][1]) + (z * matrix[1][2])
	bT := (x * matrix[2][0]) + (y * matrix[2][1]) + (z * matrix[2][2])

	// Discount illuminant
	rD := viewingConditions.RgbD[0] * rT
	gD := viewingConditions.RgbD[1] * gT
	bD := viewingConditions.RgbD[2] * bT

	// Chromatic adaptation
	rAF := math.Pow(viewingConditions.Fl*math.Abs(rD)/100.0, 0.42)
	gAF := math.Pow(viewingConditions.Fl*math.Abs(gD)/100.0, 0.42)
	bAF := math.Pow(viewingConditions.Fl*math.Abs(bD)/100.0, 0.42)
	rA := math.Copysign(1, rD) * 400.0 * rAF / (rAF + 27.13)
	gA := math.Copysign(1, gD) * 400.0 * gAF / (gAF + 27.13)
	bA := math.Copysign(1, bD) * 400.0 * bAF / (bAF + 27.13)

	// redness-greenness
	a := (11.0*rA + -12.0*gA + bA) / 11.0
	// yellowness-blueness
	b := (rA + gA - 2.0*bA) / 9.0

	// auxiliary components
	u := (20.0*rA + 20.0*gA + 21.0*bA) / 20.0
	p2 := (40.0*rA + 20.0*gA + bA) / 20.0

	// hue
	atan2 := math.Atan2(b, a)
	atanDegrees := mathutils.Degrees(atan2)
	hue := atanDegrees
	if atanDegrees < 0 {
		hue += 360.0
	} else if atanDegrees >= 360 {
		hue -= 360.0
	}
	hueRadians := mathutils.Radians(hue)

	// achromatic response to color
	ac := p2 * viewingConditions.Nbb

	// CAM16 lightness and brightness
	j := 100.0 * math.Pow(ac/viewingConditions.Aw, viewingConditions.C*viewingConditions.Z)
	q := 4.0 / viewingConditions.C * math.Sqrt(j/100.0) * (viewingConditions.Aw + 4.0) * viewingConditions.FlRoot

	// CAM16 chroma, colorfulness, and saturation.
	huePrime := hue
	if hue < 20.14 {
		huePrime += 360
	}
	eHue := 0.25 * (math.Cos(mathutils.Radians(huePrime)+2.0) + 3.8)
	p1 := 50000.0 / 13.0 * eHue * viewingConditions.Nc * viewingConditions.Ncb
	t := p1 * math.Hypot(a, b) / (u + 0.305)
	alpha := math.Pow(1.64-math.Pow(0.29, viewingConditions.N), 0.73) * math.Pow(t, 0.9)
	// CAM16 chroma, colorfulness, saturation
	c := alpha * math.Sqrt(j/100.0)
	m := c * viewingConditions.FlRoot
	s := 50.0 * math.Sqrt((alpha*viewingConditions.C)/(viewingConditions.Aw+4.0))

	// CAM16-UCS components
	jstar := (1.0 + 100.0*0.007) * j / (1.0 + 0.007*j)
	mstar := 1.0 / 0.0228 * math.Log1p(0.0228*m)
	astar := mstar * math.Cos(hueRadians)
	bstar := mstar * math.Sin(hueRadians)

	return NewCam16(hue, c, j, q, m, s, jstar, astar, bstar)
}

// fromJch creates a CAM16 color from CAM16 lightness, chroma, and hue.
//
// j: CAM16 lightness.
// c: CAM16 chroma.
// h: CAM16 hue.
// returns: A CAM16 color.
func fromJch(j, c, h float64) *CAM16 {
	return fromJchInViewingConditions(j, c, h, DefaultViewingConditions)
}

// fromJchInViewingConditions creates a CAM16 color from CAM16 lightness, chroma, and hue in specific viewing conditions.
//
// j: CAM16 lightness.
// c: CAM16 chroma.
// h: CAM16 hue.
// viewingConditions: Information about the environment where the color was observed.
// returns: A CAM16 color.
func fromJchInViewingConditions(j, c, h float64, viewingConditions *ViewingConditions) *CAM16 {
	q := 4.0 / viewingConditions.C * math.Sqrt(j/100.0) * (viewingConditions.Aw + 4.0) * viewingConditions.FlRoot
	m := c * viewingConditions.FlRoot
	alpha := c / math.Sqrt(j/100.0)
	s := 50.0 * math.Sqrt((alpha*viewingConditions.C)/(viewingConditions.Aw+4.0))

	hueRadians := mathutils.Radians(h)
	jstar := (1.0 + 100.0*0.007) * j / (1.0 + 0.007*j)
	mstar := 1.0 / 0.0228 * math.Log1p(0.0228*m)
	astar := mstar * math.Cos(hueRadians)
	bstar := mstar * math.Sin(hueRadians)

	return NewCam16(h, c, j, q, m, s, jstar, astar, bstar)
}

// fromUcs creates a CAM16 color from CAM16-UCS coordinates.
//
// jstar: CAM16-UCS lightness.
// astar: CAM16-UCS a dimension. Like a* in L*a*b*, it is a Cartesian coordinate on the Y axis.
// bstar: CAM16-UCS b dimension. Like a* in L*a*b*, it is a Cartesian coordinate on the X axis.
// returns: A CAM16 color.
func fromUcs(jstar, astar, bstar float64) *CAM16 {
	return fromUcsInViewingConditions(jstar, astar, bstar, DefaultViewingConditions)
}

// fromUcsInViewingConditions creates a CAM16 color from CAM16-UCS coordinates in defined viewing conditions.
//
// jstar: CAM16-UCS lightness.
// astar: CAM16-UCS a dimension. Like a* in L*a*b*, it is a Cartesian coordinate on the Y axis.
// bstar: CAM16-UCS b dimension. Like a* in L*a*b*, it is a Cartesian coordinate on the X axis.
// viewingConditions: Information about the environment where the color was observed.
// returns: A CAM16 color.
func fromUcsInViewingConditions(jstar, astar, bstar float64, viewingConditions *ViewingConditions) *CAM16 {
	m := math.Hypot(astar, bstar)
	m2 := math.Expm1(m*0.0228) / 0.0228
	c := m2 / viewingConditions.FlRoot
	h := math.Atan2(bstar, astar) * (180.0 / math.Pi)
	if h < 0.0 {
		h += 360.0
	}
	j := jstar / (1.0 - (jstar-100.0)*0.007)
	return fromJchInViewingConditions(j, c, h, viewingConditions)
}

// ToInt returns the ARGB representation of the color. Assumes the color was viewed in default viewing conditions,
// which are near-identical to the default viewing conditions for sRGB.
func (c *CAM16) ToInt() int {
	return c.Viewed(DefaultViewingConditions)
}

// Viewed returns the ARGB representation of the color, in defined viewing conditions.
//
// @param viewingConditions Information about the environment where the color will be viewed.
// @return ARGB representation of color
func (c *CAM16) Viewed(viewingConditions *ViewingConditions) int {
	xyz := c.xyzInViewingConditions(viewingConditions, nil)
	return colorutils.ArgbFromXyz(xyz[0], xyz[1], xyz[2])
}

// xyzInViewingConditions calculates the XYZ representation of the color in defined viewing conditions.
func (c *CAM16) xyzInViewingConditions(viewingConditions *ViewingConditions, returnArray []float64) []float64 {
	alpha := 0.0
	if c.GetChroma() != 0.0 && c.GetJ() != 0.0 {
		alpha = c.GetChroma() / math.Sqrt(c.GetJ()/100.0)
	}

	t := math.Pow(alpha/math.Pow(1.64-math.Pow(0.29, viewingConditions.GetN()), 0.73), 1.0/0.9)
	hRad := math.Pi * c.GetHue() / 180.0

	eHue := 0.25 * (math.Cos(hRad+2.0) + 3.8)
	ac := viewingConditions.GetAw() * math.Pow(c.GetJ()/100.0, 1.0/viewingConditions.GetC()/viewingConditions.GetZ())
	p1 := eHue * (50000.0 / 13.0) * viewingConditions.GetNc() * viewingConditions.GetNcb()
	p2 := ac / viewingConditions.GetNbb()

	hSin := math.Sin(hRad)
	hCos := math.Cos(hRad)

	gamma := 23.0 * (p2 + 0.305) * t / (23.0*p1 + 11.0*t*hCos + 108.0*t*hSin)
	a := gamma * hCos
	b := gamma * hSin
	rA := (460.0*p2 + 451.0*a + 288.0*b) / 1403.0
	gA := (460.0*p2 - 891.0*a - 261.0*b) / 1403.0
	bA := (460.0*p2 - 220.0*a - 6300.0*b) / 1403.0

	rCBase := math.Max(0, (27.13*math.Abs(rA))/(400.0-math.Abs(rA)))
	rC := math.Copysign(100.0/viewingConditions.GetFl()*math.Pow(rCBase, 1.0/0.42), rA)
	gCBase := math.Max(0, (27.13*math.Abs(gA))/(400.0-math.Abs(gA)))
	gC := math.Copysign(100.0/viewingConditions.GetFl()*math.Pow(gCBase, 1.0/0.42), gA)
	bCBase := math.Max(0, (27.13*math.Abs(bA))/(400.0-math.Abs(bA)))
	bC := math.Copysign(100.0/viewingConditions.GetFl()*math.Pow(bCBase, 1.0/0.42), bA)
	rF := rC / viewingConditions.GetRgbD()[0]
	gF := gC / viewingConditions.GetRgbD()[1]
	bF := bC / viewingConditions.GetRgbD()[2]

	matrix := Cam16rgbToXyz
	x := rF*matrix[0][0] + gF*matrix[0][1] + bF*matrix[0][2]
	y := rF*matrix[1][0] + gF*matrix[1][1] + bF*matrix[1][2]
	z := rF*matrix[2][0] + gF*matrix[2][1] + bF*matrix[2][2]

	if returnArray != nil {
		returnArray[0] = x
		returnArray[1] = y
		returnArray[2] = z
		return returnArray
	} else {
		return []float64{x, y, z}
	}
}

func (c *CAM16) GetChroma() float64 {
	return c.chroma
}

func (c *CAM16) GetHue() float64 {
	return c.hue
}

func (c *CAM16) GetJ() float64 {
	return c.j
}
