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

package hct

import (
	"github.com/anoosa/dar/internal/theme/material/cam16"
	"github.com/anoosa/dar/internal/theme/material/colorutils"
)

// HCT (Hue, Chroma, Tone) is a color system that provides a perceptually
// accurate color measurement. It also accurately predicts how colors will
// appear in different lighting environments.
type HCT struct {
	hue    float64
	chroma float64
	tone   float64
	argb   int
}

// From creates an HCT color from hue, chroma, and tone.
//
// hue: 0 <= hue < 360; invalid values are corrected.
// chroma: 0 <= chroma < ?; represents colorfulness. The returned color may have
//
//	lower chroma than requested, as chroma's maximum varies by hue and tone.
//
// tone: 0 <= tone <= 100; invalid values are corrected.
//
// Returns an HCT representation of a color under default viewing conditions.
func From(hue, chroma, tone float64) HCT {
	argb := solveToInt(hue, chroma, tone)
	return FromInt(argb)
}

// FromInt creates an HCT color from an ARGB color representation.
//
// argb: ARGB representation of a color.
//
// Returns an HCT representation of the color under default viewing conditions.
func FromInt(argb int) HCT {
	var hct HCT
	hct.setInternalState(argb)
	return hct
}

// GetHue returns the hue component of the HCT color.
func (h *HCT) GetHue() float64 {
	return h.hue
}

// GetChroma returns the chroma component of the HCT color.
func (h *HCT) GetChroma() float64 {
	return h.chroma
}

// GetTone returns the tone component of the HCT color.
func (h *HCT) GetTone() float64 {
	return h.tone
}

// ToInt returns the ARGB integer representation of the HCT color.
func (h *HCT) ToInt() int {
	return h.argb
}

func (h *HCT) ARGB() (alpha, red, green, blue int) {
	alpha, red, green, blue = argbComponents(h.argb)
	return
}

// SetHue sets the hue of the color. Chroma may decrease because chroma has a different maximum
// for any given hue and tone.
//
// newHue: 0 <= newHue < 360; invalid values are corrected.
func (h *HCT) SetHue(newHue float64) {
	h.setInternalState(solveToInt(newHue, h.chroma, h.tone))
}

// SetChroma sets the chroma of the color. Chroma may decrease because chroma has a different maximum
// for any given hue and tone.
//
// newChroma: 0 <= newChroma < ?.
func (h *HCT) SetChroma(newChroma float64) {
	h.setInternalState(solveToInt(h.hue, newChroma, h.tone))
}

// SetTone sets the tone of the color. Chroma may decrease because chroma has a different maximum
// for any given hue and tone.
//
// newTone: 0 <= newTone <= 100; invalid values are corrected.
func (h *HCT) SetTone(newTone float64) {
	h.setInternalState(solveToInt(h.hue, h.chroma, newTone))
}

// setInternalState updates the internal state of the HCT color using the provided ARGB value.
// It recalculates the hue, chroma, and tone based on the ARGB value and updates the internal
// HCT color representation.
func (h *HCT) setInternalState(argb int) {
	h.argb = argb
	cam := cam16.FromInt(argb)
	h.hue = cam.GetHue()
	h.chroma = cam.GetChroma()
	h.tone = colorutils.LstarFromArgb(argb)
}

/** Converts a color from RGB components to ARGB format. */
func argbFromRgb(red, green, blue int) int {
	return (255 << 24) | ((red & 255) << 16) | ((green & 255) << 8) | (blue & 255)
}

func argbComponents(argb int) (int, int, int, int) {
	alpha := (argb >> 24) & 255
	red := (argb >> 16) & 255
	green := (argb >> 8) & 255
	blue := argb & 255
	return alpha, red, green, blue
}

/**
 * Converts an L* value to an ARGB representation.
 *
 * @param lstar L* in L*a*b*
 * @return ARGB representation of grayscale color with lightness matching L*
 */
func argbFromLstar(lstar float64) int {
	y := colorutils.YFromLstar(lstar)
	component := colorutils.Delinearized(y)
	return argbFromRgb(component, component, component)
}
