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

import "math"

// Signum is the signum function.
// Returns 1 if num > 0, -1 if num < 0, and 0 if num = 0
//   - num The number to evaluate.
func Signum(num float64) int {
	if num < 0 {
		return -1
	} else if num == 0 {
		return 0
	} else {
		return 1
	}
}

// Lerp is the linear interpolation function.
// Returns start if amount = 0 and stop if amount = 1
//   - start The starting value.
//   - stop The ending value.
//   - amount The interpolation factor.
func Lerp(start, stop, amount float64) float64 {
	return (1.0-amount)*start + amount*stop
}

// ClampInt clamps an integer between two integers.
// Returns input when min <= input <= max, and either min or max otherwise.
//   - min The minimum value.
//   - max The maximum value.
//   - input The value to clamp.
func ClampInt(min, max, input int) int {
	if input < min {
		return min
	} else if input > max {
		return max
	}
	return input
}

// ClampDouble clamps a floating-point number between two floating-point numbers.
// Returns input when min <= input <= max, and either min or max otherwise.
//   - min The minimum value.
//   - max The maximum value.
//   - input The value to clamp.
func ClampDouble(min, max, input float64) float64 {
	if input < min {
		return min
	} else if input > max {
		return max
	}
	return input
}

// SanitizeDegreesInt sanitizes a degree measure as an integer.
// Returns a degree measure between 0 (inclusive) and 360 (exclusive).
//   - degrees The degree measure to sanitize.
func SanitizeDegreesInt(degrees int) int {
	degrees = degrees % 360
	if degrees < 0 {
		degrees = degrees + 360
	}
	return degrees
}

// SanitizeDegreesDouble sanitizes a degree measure as a floating-point number.
// Returns a degree measure between 0.0 (inclusive) and 360.0 (exclusive).
//   - degrees The degree measure to sanitize.
func SanitizeDegreesDouble(degrees float64) float64 {
	degrees = degrees - 360.0*float64(int(degrees/360.0)) // TODO is this the same? degrees = math.Mod(degrees, 360.0)
	if degrees < 0 {
		degrees = degrees + 360.0
	}
	return degrees
}

// RotationDirection calculates the sign of direction change needed to travel from one angle to another.
// For angles that are 180 degrees apart from each other, both directions have the same travel
// distance, so either direction is shortest. The value 1.0 is returned in this case.
//   - from The angle travel starts from, in degrees.
//   - to The angle travel ends at, in degrees.
//
// Returns -1 if decreasing from leads to the shortest travel distance, 1 if increasing from leads
// to the shortest travel distance.
func RotationDirection(from, to float64) float64 {
	increasingDifference := SanitizeDegreesDouble(to - from)
	if increasingDifference <= 180.0 {
		return 1.0
	}
	return -1.0
}

// DifferenceDegrees calculates the distance of two points on a circle, represented using degrees.
//   - a The first angle, in degrees.
//   - b The second angle, in degrees.
func DifferenceDegrees(a, b float64) float64 {
	return 180.0 - math.Abs(math.Abs(a-b)-180.0)
}

// MatrixMultiply multiplies a 1x3 row vector with a 3x3 matrix.
//   - row The 1x3 row vector.
//   - matrix The 3x3 matrix.
func MatrixMultiply(row [3]float64, matrix [3][3]float64) [3]float64 {
	a := row[0]*matrix[0][0] + row[1]*matrix[0][1] + row[2]*matrix[0][2]
	b := row[0]*matrix[1][0] + row[1]*matrix[1][1] + row[2]*matrix[1][2]
	c := row[0]*matrix[2][0] + row[1]*matrix[2][1] + row[2]*matrix[2][2]
	return [3]float64{a, b, c}
}
