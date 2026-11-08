# Dar prayer calculations, version 1

This is original, dependency-free TypeScript, not vendored Adhan or PrayTimes code.
It implements published astronomical equations and selectable prayer conventions.

The solar model uses the [US Naval Observatory approximate solar coordinates](https://aa.usno.navy.mil/faq/sun_approx).
Each event is found by bracketing and bisecting the geometric solar altitude, with
the Sun's changing declination and equation of time evaluated at each candidate.
Sunrise/sunset use the [USNO sea-level −0.833333° convention](https://aa.usno.navy.mil/faq/RST_defs).
This assumes an unobstructed horizon and average refraction. It does not model
terrain, observer elevation or weather. Numerical convergence is not a promise
of subsecond observational accuracy.

The [published prayer definitions](https://praytimes.org/calculation) supply the
angle pairs for MWL (18°/17°), ISNA (15°/15°), Egyptian (19.5°/17.5°), and Karachi
(18°/18°). These are parameters, not claims that every local authority uses the
same settings. Asr adds either one or two object heights to the noon shadow.
Dhuhr adds one minute after solar transit; Maghrib uses sunset. No Ramadan,
lunar-calendar or Umm al-Qura fixed-interval assumptions are made.

Optional high-latitude bounds use half, one seventh or angle/60 of the actual
adjacent sunset-to-sunrise night. Adjusted results are labelled. Without the
needed sunrise/sunset anchors, the bound is unavailable; no nearest-city rule is
invented. Polar solar events remain unavailable. A solar transit can still exist
without sunrise/sunset; showing transit/Dhuhr is not a polar religious ruling.

Inputs use Gregorian civil dates in 2000–2099, finite latitude/longitude and an
explicit IANA timezone. UTC instants drive the astronomy; Intl maps the requested
civil date to its solar transit and renders the resulting events. There are no
host-local date accessors. At the supported year boundaries, adjacent-night
calculations extend by one day; the public requested-date range stays bounded.
User minute offsets apply last, followed by rounding to the nearest minute.
Large offsets can alter chronological order; the next-prayer selector sorts
actual instants and omits sunrise. Settings never silently choose another method.

Calculated estimates can differ from local mosque timetables. Reference fixtures
and tolerances are documented in the tests. This library is scoped to the listed
conventions and is not a claim of universal religious or astronomical accuracy.
