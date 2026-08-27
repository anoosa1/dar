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

package mathutils

import (
	"fmt"
	"math"
)

// Degrees converts an angle measured in radians to an approximately
// equivalent angle measured in degrees. The conversion from
// radians to degrees is generally inexact; users should
// not expect cos(toRadians(90.0)) to exactly
// equal 0.0.
//
// angrad: an angle in radians
// return: the measurement of the angle in degrees
func Degrees(angrad float64) float64 {
	return angrad * 180.0 / math.Pi
}

// Radians converts an angle measured in degrees to an approximately
// equivalent angle measured in radians. The conversion from
// degrees to radians is generally inexact.
//
// angdeg: an angle in degrees
// return: the measurement of the angle in radians
func Radians(angdeg float64) float64 {
	return angdeg / 180.0 * math.Pi
}

// ToBeCloseTo is test helper which panics if not close to.
func ToBeCloseTo(actual, expected, decimalPlaces float64) {
	// Berechne die Toleranz basierend auf der Anzahl der Dezimalstellen
	tolerance := math.Pow(10, -decimalPlaces)

	// Überprüfe, ob der Unterschied zwischen actual und expected innerhalb der Toleranz liegt
	if math.Abs(actual-expected) > tolerance {
		panic(fmt.Errorf("expected %v to be close to %v (with tolerance %v), but got %v", expected, actual, tolerance, actual))
	}
}
