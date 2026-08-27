package prayer

import (
	"errors"
	"fmt"
	"math"
	"time"
)

type Method string

const (
	MethodMWL      Method = "mwl"
	MethodISNA     Method = "isna"
	MethodEgyptian Method = "egyptian"
	MethodKarachi  Method = "karachi"
)

type AsrSchool string

const (
	AsrStandard AsrSchool = "standard"
	AsrHanafi   AsrSchool = "hanafi"
)

type methodParameters struct {
	FajrAngle float64
	IshaAngle float64
}

var methodPresets = map[Method]methodParameters{
	MethodMWL:      {FajrAngle: 18, IshaAngle: 17},
	MethodISNA:     {FajrAngle: 15, IshaAngle: 15},
	MethodEgyptian: {FajrAngle: 19.5, IshaAngle: 17.5},
	MethodKarachi:  {FajrAngle: 18, IshaAngle: 18},
}

var ErrTimesUnavailable = errors.New("prayer times unavailable for this date and location")

type Times struct {
	Fajr     time.Time `json:"fajr"`
	Sunrise  time.Time `json:"sunrise"`
	Dhuhr    time.Time `json:"dhuhr"`
	Asr      time.Time `json:"asr"`
	Maghrib  time.Time `json:"maghrib"`
	Isha     time.Time `json:"isha"`
	Midnight time.Time `json:"midnight"`
}

type Calculation struct {
	Latitude  float64
	Longitude float64
	Location  *time.Location
	Method    Method
	AsrSchool AsrSchool
}

func (c Calculation) Calculate(date time.Time) (Times, error) {
	if c.Location == nil || c.Latitude < -90 || c.Latitude > 90 || c.Longitude < -180 || c.Longitude > 180 {
		return Times{}, fmt.Errorf("%w: invalid coordinates or timezone", ErrTimesUnavailable)
	}
	params, ok := methodPresets[c.Method]
	if !ok {
		return Times{}, fmt.Errorf("unknown calculation method %q", c.Method)
	}
	if c.AsrSchool != AsrStandard && c.AsrSchool != AsrHanafi {
		return Times{}, fmt.Errorf("unknown Asr school %q", c.AsrSchool)
	}
	date = date.In(c.Location)
	day := time.Date(date.Year(), date.Month(), date.Day(), 12, 0, 0, 0, c.Location)
	base, err := c.calculateDay(day, params)
	if err != nil {
		return Times{}, err
	}
	next, err := c.calculateDay(day.AddDate(0, 0, 1), params)
	if err != nil {
		return Times{}, err
	}
	base.Midnight = roundMinute(base.Maghrib.Add(next.Sunrise.Sub(base.Maghrib) / 2))
	return base, nil
}

func (c Calculation) calculateDay(day time.Time, params methodParameters) (Times, error) {
	jd := julianDate(day.Year(), int(day.Month()), day.Day()) - c.Longitude/(15*24)
	values := []float64{5, 6, 12, 13, 18, 18}
	for iteration := 0; iteration < 2; iteration++ {
		for i := range values {
			values[i] /= 24
		}
		fajr := sunAngleTime(jd, c.Latitude, -params.FajrAngle, values[0], true)
		sunrise := sunAngleTime(jd, c.Latitude, -0.833, values[1], true)
		dhuhr := midDay(jd, values[2])
		factor := 1.0
		if c.AsrSchool == AsrHanafi {
			factor = 2
		}
		asr := asrTime(jd, c.Latitude, factor, values[3])
		sunset := sunAngleTime(jd, c.Latitude, -0.833, values[4], false)
		isha := sunAngleTime(jd, c.Latitude, -params.IshaAngle, values[5], false)
		values = []float64{fajr, sunrise, dhuhr, asr, sunset, isha}
	}

	_, offset := day.Zone()
	adjust := float64(offset)/3600 - c.Longitude/15
	for i := range values {
		values[i] += adjust
	}
	if math.IsNaN(values[1]) || math.IsNaN(values[4]) {
		return Times{}, ErrTimesUnavailable
	}
	night := timeDiff(values[4], values[1])
	if night <= 0 || math.IsNaN(night) {
		return Times{}, ErrTimesUnavailable
	}
	// Angle-based portions prevent twilight prayers from disappearing during
	// bright high-latitude nights while preserving normal astronomical times.
	fajrPortion := params.FajrAngle / 60 * night
	if diff := timeDiff(values[0], values[1]); math.IsNaN(values[0]) || diff > fajrPortion {
		values[0] = values[1] - fajrPortion
	}
	ishaPortion := params.IshaAngle / 60 * night
	if diff := timeDiff(values[4], values[5]); math.IsNaN(values[5]) || diff > ishaPortion {
		values[5] = values[4] + ishaPortion
	}
	for _, value := range values {
		if math.IsNaN(value) || math.IsInf(value, 0) {
			return Times{}, ErrTimesUnavailable
		}
	}
	toTime := func(hours float64) time.Time {
		minutes := int(math.Floor(hours*60 + 0.5))
		return time.Date(day.Year(), day.Month(), day.Day(), 0, minutes, 0, 0, c.Location)
	}
	return Times{
		Fajr: toTime(values[0]), Sunrise: toTime(values[1]), Dhuhr: toTime(values[2]),
		Asr: toTime(values[3]), Maghrib: toTime(values[4]), Isha: toTime(values[5]),
	}, nil
}

func roundMinute(value time.Time) time.Time {
	return value.Add(30 * time.Second).Truncate(time.Minute)
}

func julianDate(year, month, day int) float64 {
	if month <= 2 {
		year--
		month += 12
	}
	a := math.Floor(float64(year) / 100)
	b := 2 - a + math.Floor(a/4)
	return math.Floor(365.25*float64(year+4716)) + math.Floor(30.6001*float64(month+1)) + float64(day) + b - 1524.5
}

func sunPosition(jd float64) (declination, equation float64) {
	d := jd - 2451545
	g := fixAngle(357.529 + 0.98560028*d)
	q := fixAngle(280.459 + 0.98564736*d)
	l := fixAngle(q + 1.915*sinDeg(g) + 0.020*sinDeg(2*g))
	e := 23.439 - 0.00000036*d
	ra := atan2Deg(cosDeg(e)*sinDeg(l), cosDeg(l)) / 15
	ra = fixHour(ra)
	equation = q/15 - ra
	declination = asinDeg(sinDeg(e) * sinDeg(l))
	return declination, equation
}

func midDay(jd, timePortion float64) float64 {
	_, equation := sunPosition(jd + timePortion)
	return fixHour(12 - equation)
}

func sunAngleTime(jd, latitude, altitude, timePortion float64, morning bool) float64 {
	declination, _ := sunPosition(jd + timePortion)
	noon := midDay(jd, timePortion)
	denominator := cosDeg(declination) * cosDeg(latitude)
	if math.Abs(denominator) < 1e-12 {
		return math.NaN()
	}
	ratio := (sinDeg(altitude) - sinDeg(declination)*sinDeg(latitude)) / denominator
	if ratio < -1 || ratio > 1 {
		return math.NaN()
	}
	delta := acosDeg(ratio) / 15
	if morning {
		return noon - delta
	}
	return noon + delta
}

func asrTime(jd, latitude, factor, timePortion float64) float64 {
	declination, _ := sunPosition(jd + timePortion)
	altitude := atanDeg(1 / (factor + tanDeg(math.Abs(latitude-declination))))
	return sunAngleTime(jd, latitude, altitude, timePortion, false)
}

func timeDiff(first, second float64) float64 { return fixHour(second - first) }
func fixAngle(v float64) float64             { return fix(v, 360) }
func fixHour(v float64) float64              { return fix(v, 24) }
func fix(v, cycle float64) float64 {
	v -= cycle * math.Floor(v/cycle)
	if v < 0 {
		v += cycle
	}
	return v
}
func sinDeg(v float64) float64      { return math.Sin(v * math.Pi / 180) }
func cosDeg(v float64) float64      { return math.Cos(v * math.Pi / 180) }
func tanDeg(v float64) float64      { return math.Tan(v * math.Pi / 180) }
func asinDeg(v float64) float64     { return math.Asin(v) * 180 / math.Pi }
func acosDeg(v float64) float64     { return math.Acos(v) * 180 / math.Pi }
func atanDeg(v float64) float64     { return math.Atan(v) * 180 / math.Pi }
func atan2Deg(y, x float64) float64 { return math.Atan2(y, x) * 180 / math.Pi }
