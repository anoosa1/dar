//! Pure-Rust solar astronomy prayer times, Qibla bearing, Sun position, and Hijri calendar.

use crate::prayers::model::{
    ALL_PRAYERS, CalculationSettings, Location, PrayerKey, method_preset,
};
use crate::prayers::tz::{CivilDate, local_minutes_of_day};

const KAABA_LAT_DEG: f64 = 21.422487;
const KAABA_LON_DEG: f64 = 39.826206;
const EARTH_RADIUS_KM: f64 = 6371.0088;

/// Computed prayer timestamps (`epoch_ms`) for one civil date at a location.
#[derive(Clone, Debug, PartialEq)]
pub struct DayTimes {
    pub date: CivilDate,
    pub times_ms: [Option<f64>; 6],
    pub solar_noon_ms: f64,
    pub midnight_ms: Option<f64>,
    pub last_third_ms: Option<f64>,
    pub daylight_minutes: Option<i32>,
}

impl DayTimes {
    pub fn get(&self, key: PrayerKey) -> Option<f64> {
        self.times_ms[key.index()]
    }
}

/// Active window and upcoming prayer at a specific instant.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentWindow {
    pub today: CivilDate,
    pub current_prayer: PrayerKey,
    pub next_prayer: PrayerKey,
    pub next_date: CivilDate,
    pub next_time_ms: f64,
    pub remaining_ms: f64,
    pub progress: f64,
    pub is_tomorrow_fajr: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HijriDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl HijriDate {
    pub const fn month_name(self) -> &'static str {
        match self.month {
            1 => "Muharram",
            2 => "Safar",
            3 => "Rabi\u{02bb} al-Awwal",
            4 => "Rabi\u{02bb} al-Thani",
            5 => "Jumada al-Awwal",
            6 => "Jumada al-Thani",
            7 => "Rajab",
            8 => "Sha\u{02bb}ban",
            9 => "Ramadan",
            10 => "Shawwal",
            11 => "Dhu al-Qi\u{02bb}dah",
            12 => "Dhu al-Hijjah",
            _ => "",
        }
    }

    pub const fn short_month_name(self) -> &'static str {
        match self.month {
            1 => "Muharram",
            2 => "Safar",
            3 => "Rabi\u{02bb} I",
            4 => "Rabi\u{02bb} II",
            5 => "Jumada I",
            6 => "Jumada II",
            7 => "Rajab",
            8 => "Sha\u{02bb}ban",
            9 => "Ramadan",
            10 => "Shawwal",
            11 => "Dhu al-Qi\u{02bb}dah",
            12 => "Dhu al-Hijjah",
            _ => "",
        }
    }

    pub fn format_short(self) -> String {
        format!("{} {}", self.day, self.short_month_name())
    }

    pub fn format_full(self) -> String {
        format!("{} {} {} AH", self.day, self.month_name(), self.year)
    }
}

/// Julian Day at 0h UTC of `date`.
pub fn julian_day_zero_utc(date: CivilDate) -> f64 {
    date.days_since_epoch() as f64 + 2440587.5
}

/// NOAA solar declination (degrees) and equation of time (minutes) at Julian Day `jd`.
pub fn sun_position(jd: f64) -> (f64, f64) {
    let d = jd - 2451545.0;
    let g = (357.529 + 0.98560028 * d).to_radians();
    let q = (280.459 + 0.98564736 * d).rem_euclid(360.0);
    let l = (q + 1.915 * g.sin() + 0.020 * (2.0 * g).sin()).rem_euclid(360.0);
    let e = (23.439 - 0.00000036 * d).to_radians();
    let l_rad = l.to_radians();

    let mut ra = e.cos().mul_add(l_rad.sin(), 0.0).atan2(l_rad.cos()).to_degrees() / 15.0;
    ra = ra.rem_euclid(24.0);
    let eqt = (q / 15.0 - ra) * 60.0;
    let eqt = if eqt > 720.0 {
        eqt - 1440.0
    } else if eqt < -720.0 {
        eqt + 1440.0
    } else {
        eqt
    };
    let decl_deg = (e.sin() * l_rad.sin()).asin().to_degrees();
    (decl_deg, eqt)
}

fn hour_angle_deg(lat_deg: f64, decl_deg: f64, sun_altitude_deg: f64) -> Option<f64> {
    let lat = lat_deg.to_radians();
    let decl = decl_deg.to_radians();
    let alt = sun_altitude_deg.to_radians();
    let denom = lat.cos() * decl.cos();
    if denom.abs() < 1e-12 {
        return None;
    }
    let cos_ha = (alt.sin() - lat.sin() * decl.sin()) / denom;
    if !(-1.0..=1.0).contains(&cos_ha) {
        None
    } else {
        Some(cos_ha.acos().to_degrees())
    }
}

fn asr_altitude_deg(lat_deg: f64, decl_deg: f64, shadow_factor: f64) -> f64 {
    let diff = (lat_deg - decl_deg).abs().to_radians();
    (1.0 / (shadow_factor + diff.tan())).atan().to_degrees()
}

fn raw_sun_times_utc_hours(
    date: CivilDate,
    loc: &Location,
    calc: &CalculationSettings,
) -> (f64, [Option<f64>; 6]) {
    let preset = method_preset(&calc.method);
    let fajr_angle = if calc.method == "Custom" {
        calc.custom_fajr_angle.unwrap_or(preset.fajr_angle)
    } else {
        preset.fajr_angle
    };
    let isha_minutes = if calc.method == "Custom" {
        calc.custom_isha_minutes
    } else {
        preset.isha_minutes
    };
    let isha_angle = if calc.method == "Custom" {
        calc.custom_isha_angle.unwrap_or(preset.isha_angle)
    } else {
        preset.isha_angle
    };
    let shadow_factor = if calc.asr_rule == "hanafi" { 2.0 } else { 1.0 };

    // Estimate local solar noon for Julian Day offset.
    let jd_base = julian_day_zero_utc(date) + 0.5 - (loc.longitude / 360.0);
    let (decl_mid, eqt_mid) = sun_position(jd_base);
    let noon_hours = (720.0 - 4.0 * loc.longitude - eqt_mid) / 60.0;

    let refine = |approx_hours: f64, fn_calc: &dyn Fn(f64) -> Option<f64>| -> Option<f64> {
        let h = fn_calc(decl_mid)?;
        let jd_r = julian_day_zero_utc(date) + h / 24.0;
        let (decl_r, eqt_r) = sun_position(jd_r);
        let noon_r = (720.0 - 4.0 * loc.longitude - eqt_r) / 60.0;
        let delta = h - approx_hours;
        let _ = delta;
        // Re-evaluate hour angle using refined declination.
        let sign = if h >= approx_hours { 1.0 } else { -1.0 };
        let diff_orig = (h - approx_hours).abs();
        let _ = (sign, diff_orig);
        let _ = decl_r;
        Some(noon_r + (h - noon_hours))
    };

    // Directly compute refined hour for any target sun elevation and side (-1 morning, +1 evening).
    let compute_side = |alt_deg: f64, side: f64| -> Option<f64> {
        let ha1 = hour_angle_deg(loc.latitude, decl_mid, alt_deg)?;
        let approx = noon_hours + side * ha1 / 15.0;
        let (decl2, eqt2) = sun_position(julian_day_zero_utc(date) + approx / 24.0);
        let noon2 = (720.0 - 4.0 * loc.longitude - eqt2) / 60.0;
        let ha2 = hour_angle_deg(loc.latitude, decl2, alt_deg)?;
        Some(noon2 + side * ha2 / 15.0)
    };
    let _ = refine;

    let mut sunrise = compute_side(-0.8333, -1.0);
    let mut sunset = compute_side(-0.8333, 1.0);
    let dhuhr = Some(noon_hours);
    let asr = {
        let alt1 = asr_altitude_deg(loc.latitude, decl_mid, shadow_factor);
        if let Some(ha1) = hour_angle_deg(loc.latitude, decl_mid, alt1) {
            let approx = noon_hours + ha1 / 15.0;
            let (decl2, eqt2) = sun_position(julian_day_zero_utc(date) + approx / 24.0);
            let noon2 = (720.0 - 4.0 * loc.longitude - eqt2) / 60.0;
            let alt2 = asr_altitude_deg(loc.latitude, decl2, shadow_factor);
            hour_angle_deg(loc.latitude, decl2, alt2).map(|ha2| noon2 + ha2 / 15.0)
        } else {
            None
        }
    };

    let mut fajr = compute_side(-fajr_angle, -1.0);
    let mut isha = if let Some(mins) = isha_minutes {
        sunset.map(|m| m + mins as f64 / 60.0)
    } else {
        compute_side(-isha_angle, 1.0)
    };

    // High-latitude fallback / cap if Fajr or Isha fail to reach twilight angle or exceed rule bound.
    if calc.high_latitude_rule != "none" {
        if sunrise.is_none() || sunset.is_none() {
            // Polar edge fallback anchored on solar noon +/- 6h so UI remains usable.
            sunrise = sunrise.or(Some(noon_hours - 6.0));
            sunset = sunset.or(Some(noon_hours + 6.0));
        }
        if let (Some(sr), Some(ss)) = (sunrise, sunset) {
            let night_hours = (sr + 24.0 - ss).max(0.0);
            let portion = |angle: f64| -> f64 {
                match calc.high_latitude_rule.as_str() {
                    "middle" => night_hours / 2.0,
                    "seventh" => night_hours / 7.0,
                    "angle" => (angle / 60.0) * night_hours,
                    _ => night_hours / 2.0,
                }
            };
            let f_portion = portion(fajr_angle);
            if fajr.is_none_or(|f| (sr - f) > f_portion) {
                fajr = Some(sr - f_portion);
            }
            if isha_minutes.is_none() {
                let i_portion = portion(isha_angle);
                if isha.is_none_or(|i| (i - ss) > i_portion) {
                    isha = Some(ss + i_portion);
                }
            }
        }
    }

    (
        noon_hours,
        [fajr, sunrise, dhuhr, asr, sunset, isha],
    )
}

fn day_times_from_raw(
    date: CivilDate,
    loc: &Location,
    calc: &CalculationSettings,
) -> DayTimes {
    let base_ms = date.days_since_epoch() as f64 * 86_400_000.0;
    let (noon_hours, raw_hours) = raw_sun_times_utc_hours(date, loc, calc);
    let mut times_ms = [None; 6];
    for p in ALL_PRAYERS {
        times_ms[p.index()] = raw_hours[p.index()].map(|h| {
            let off = calc.offset_minutes.get(p) as f64 * 60_000.0;
            base_ms + h * 3_600_000.0 + off
        });
    }
    let solar_noon_ms = base_ms + noon_hours * 3_600_000.0;
    let daylight_minutes = match (times_ms[PrayerKey::Sunrise.index()], times_ms[PrayerKey::Maghrib.index()]) {
        (Some(sr), Some(mg)) => Some(((mg - sr) / 60_000.0).round() as i32),
        _ => None,
    };
    DayTimes {
        date,
        times_ms,
        solar_noon_ms,
        midnight_ms: None,
        last_third_ms: None,
        daylight_minutes,
    }
}

/// Computes prayer times and Sunna night markers (`Midnight`, `Last third`) for a civil date.
pub fn compute_day_times(
    date: CivilDate,
    loc: &Location,
    calc: &CalculationSettings,
) -> DayTimes {
    let mut day = day_times_from_raw(date, loc, calc);
    let next = day_times_from_raw(date.add_days(1), loc, calc);
    if let (Some(maghrib), Some(next_fajr)) = (day.get(PrayerKey::Maghrib), next.get(PrayerKey::Fajr))
        && next_fajr > maghrib
    {
        let night_ms = next_fajr - maghrib;
        day.midnight_ms = Some(maghrib + night_ms * 0.5);
        day.last_third_ms = Some(maghrib + night_ms * (2.0 / 3.0));
    }
    day
}

/// Finds the active prayer window and upcoming prayer at `now_ms`.
pub fn compute_current_window(
    now_ms: f64,
    loc: &Location,
    calc: &CalculationSettings,
) -> CurrentWindow {
    let today = crate::prayers::tz::civil_date_in_tz(now_ms, &loc.timezone);
    let yest_times = compute_day_times(today.add_days(-1), loc, calc);
    let today_times = compute_day_times(today, loc, calc);
    let tom_times = compute_day_times(today.add_days(1), loc, calc);

    let mut events: Vec<(CivilDate, PrayerKey, f64)> = Vec::with_capacity(18);
    for d in [&yest_times, &today_times, &tom_times] {
        for p in ALL_PRAYERS {
            if let Some(ms) = d.get(p) {
                events.push((d.date, p, ms));
            }
        }
    }
    events.sort_by(|a, b| a.2.total_cmp(&b.2));

    let next_idx = events
        .iter()
        .position(|(_, _, ms)| *ms > now_ms)
        .unwrap_or(events.len().saturating_sub(1));
    let (next_date, next_prayer, next_time_ms) = events
        .get(next_idx)
        .cloned()
        .unwrap_or((today.add_days(1), PrayerKey::Fajr, now_ms + 3_600_000.0));
    let (prev_prayer, prev_time_ms) = if next_idx > 0 {
        let prev = &events[next_idx - 1];
        (prev.1, prev.2)
    } else {
        (PrayerKey::Isha, next_time_ms - 3_600_000.0)
    };

    let span = (next_time_ms - prev_time_ms).max(1.0);
    let progress = ((now_ms - prev_time_ms) / span).clamp(0.0, 1.0);
    let remaining_ms = (next_time_ms - now_ms).max(0.0);

    CurrentWindow {
        today,
        current_prayer: prev_prayer,
        next_prayer,
        next_date,
        next_time_ms,
        remaining_ms,
        progress,
        is_tomorrow_fajr: next_date > today && next_prayer == PrayerKey::Fajr,
    }
}

/// Solar `(azimuth_deg 0..360 clockwise from North, altitude_deg -90..90)` at `epoch_ms`.
pub fn sun_azimuth_altitude(epoch_ms: f64, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
    let jd = epoch_ms / 86_400_000.0 + 2440587.5;
    let (decl_deg, eqt_min) = sun_position(jd);
    let utc_minutes = (epoch_ms / 60_000.0).rem_euclid(1440.0);
    let solar_time_min = (utc_minutes + eqt_min + 4.0 * lon_deg).rem_euclid(1440.0);
    let ha_deg = solar_time_min / 4.0 - 180.0;

    let lat = lat_deg.to_radians();
    let decl = decl_deg.to_radians();
    let ha = ha_deg.to_radians();

    let sin_alt = (lat.sin() * decl.sin() + lat.cos() * decl.cos() * ha.cos()).clamp(-1.0, 1.0);
    let alt_rad = sin_alt.asin();

    let y = -decl.cos() * ha.sin();
    let x = decl.sin() * lat.cos() - decl.cos() * lat.sin() * ha.cos();
    let az_deg = y.atan2(x).to_degrees().rem_euclid(360.0);
    (az_deg, alt_rad.to_degrees())
}

/// Solar azimuth (`0..360`) and normalized altitude `[0..1]` for the Horizon Arc SVG at `epoch_ms`.
pub fn sun_arc_xy(epoch_ms: f64, tz: &str, sunrise_ms: Option<f64>, sunset_ms: Option<f64>) -> (f64, f64) {
    let x_frac = local_minutes_of_day(epoch_ms, tz) / 1440.0;
    let y_norm = if let (Some(sr), Some(ss)) = (sunrise_ms, sunset_ms)
        && ss > sr
    {
        let sr_m = local_minutes_of_day(sr, tz) / 1440.0;
        let ss_m = local_minutes_of_day(ss, tz) / 1440.0;
        if x_frac >= sr_m && x_frac <= ss_m {
            let phase = (x_frac - sr_m) / (ss_m - sr_m);
            (phase * std::f64::consts::PI).sin()
        } else {
            // Gentle night dip below the horizon line.
            let night_phase = if x_frac < sr_m {
                x_frac / sr_m.max(0.05)
            } else {
                (1.0 - x_frac) / (1.0 - ss_m).max(0.05)
            };
            -0.35 * ((1.0 - night_phase).clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2).sin()
        }
    } else {
        0.0
    };
    (x_frac, y_norm)
}

/// Great-circle Qibla initial bearing (0..360 deg from true North), distance in km, and compass wind label.
pub fn qibla_bearing_and_distance(lat_deg: f64, lon_deg: f64) -> (f64, f64, &'static str) {
    let phi1 = lat_deg.to_radians();
    let lam1 = lon_deg.to_radians();
    let phi2 = KAABA_LAT_DEG.to_radians();
    let lam2 = KAABA_LON_DEG.to_radians();
    let dlam = lam2 - lam1;

    let y = dlam.sin() * phi2.cos();
    let x = phi1.cos() * phi2.sin() - phi1.sin() * phi2.cos() * dlam.cos();
    let bearing = y.atan2(x).to_degrees().rem_euclid(360.0);

    let dphi = phi2 - phi1;
    let a = (dphi / 2.0).sin().powi(2) + phi1.cos() * phi2.cos() * (dlam / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().min(1.0).asin();
    let distance_km = EARTH_RADIUS_KM * c;

    const WINDS: [&str; 16] = [
        "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW",
        "NW", "NNW",
    ];
    let idx = ((bearing / 22.5).round() as usize) % 16;
    (bearing, distance_km, WINDS[idx])
}

/// Civil Gregorian date to Islamic (Kuwaiti / tabular arithmetic) Hijri date, adjusted by `offset_days`.
pub fn civil_to_hijri(date: CivilDate, offset_days: i32) -> HijriDate {
    let shifted = date.add_days(offset_days as i64);
    let jd = shifted.days_since_epoch() + 2440588;
    let mut l = jd - 1948440 + 10632;
    let n = (l - 1) / 10631;
    l = l - 10631 * n + 354;
    let j = ((10985 - l) / 5316) * ((50 * l) / 17719) + (l / 5670) * ((43 * l) / 15238);
    l = l - ((30 - j) / 15) * ((17719 * j) / 50) - (j / 16) * ((15238 * j) / 43) + 29;
    let m = (24 * l) / 709;
    let day = (l - (709 * m) / 24) as u8;
    let year = (30 * n + j - 30) as i32;
    let month = m.clamp(1, 12) as u8;
    HijriDate { year, month, day }
}

/// Formats `EpochMs` difference between `new_val` and `old_val` in whole minutes (signed).
pub fn minutes_delta(new_ms: Option<f64>, old_ms: Option<f64>) -> i32 {
    match (new_ms, old_ms) {
        (Some(n), Some(o)) => ((n - o) / 60_000.0).round() as i32,
        _ => 0,
    }
}

pub fn format_coord_lat(lat: f64) -> String {
    if lat >= 0.0 {
        format!("{lat:.4}\u{00b0} N")
    } else {
        format!("{:.4}\u{00b0} S", -lat)
    }
}

pub fn format_coord_lon(lon: f64) -> String {
    if lon >= 0.0 {
        format!("{lon:.4}\u{00b0} E")
    } else {
        format!("{:.4}\u{00b0} W", -lon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prayers::model::OffsetMinutes;
    use crate::prayers::tz::format_time_in_tz;

    fn toronto() -> Location {
        Location {
            label: "Toronto".into(),
            latitude: 43.6532,
            longitude: -79.3832,
            timezone: "America/Toronto".into(),
        }
    }

    fn isna_calc(asr_rule: &str) -> CalculationSettings {
        CalculationSettings {
            method: "ISNA".into(),
            asr_rule: asr_rule.into(),
            high_latitude_rule: "middle".into(),
            offset_minutes: OffsetMinutes::default(),
            custom_fajr_angle: None,
            custom_isha_angle: None,
            custom_isha_minutes: None,
        }
    }

    #[test]
    fn toronto_nov8_2026_standard_vs_hanafi_asr() {
        let d = CivilDate::from_ymd(2026, 11, 8).unwrap();
        let std_t = compute_day_times(d, &toronto(), &isna_calc("standard"));
        let han_t = compute_day_times(d, &toronto(), &isna_calc("hanafi"));
        let fajr_s = format_time_in_tz(std_t.get(PrayerKey::Fajr).unwrap(), "America/Toronto", true);
        let dhuhr_s = format_time_in_tz(std_t.get(PrayerKey::Dhuhr).unwrap(), "America/Toronto", true);
        let maghrib_s = format_time_in_tz(std_t.get(PrayerKey::Maghrib).unwrap(), "America/Toronto", true);
        assert!(fajr_s.starts_with("05:4"), "fajr={fajr_s}");
        assert!(dhuhr_s.starts_with("12:0"), "dhuhr={dhuhr_s}");
        assert!(maghrib_s.starts_with("16:5"), "maghrib={maghrib_s}");

        let diff_min = (han_t.get(PrayerKey::Asr).unwrap() - std_t.get(PrayerKey::Asr).unwrap()) / 60_000.0;
        assert!((30.0..=55.0).contains(&diff_min), "asr diff={diff_min}");
    }

    #[test]
    fn makkah_umm_al_qura_isha_plus_90m() {
        let d = CivilDate::from_ymd(2026, 11, 8).unwrap();
        let makkah = Location {
            label: "Makkah".into(),
            latitude: 21.4225,
            longitude: 39.8262,
            timezone: "Asia/Riyadh".into(),
        };
        let calc = CalculationSettings {
            method: "UmmAlQura".into(),
            asr_rule: "standard".into(),
            high_latitude_rule: "middle".into(),
            offset_minutes: OffsetMinutes::default(),
            custom_fajr_angle: None,
            custom_isha_angle: None,
            custom_isha_minutes: None,
        };
        let t = compute_day_times(d, &makkah, &calc);
        let mag = t.get(PrayerKey::Maghrib).unwrap();
        let isha = t.get(PrayerKey::Isha).unwrap();
        assert!(((isha - mag) / 60_000.0 - 90.0).abs() < 0.01);
    }

    #[test]
    fn oslo_summer_high_latitude_succeeds() {
        let d = CivilDate::from_ymd(2026, 6, 21).unwrap();
        let oslo = Location {
            label: "Oslo".into(),
            latitude: 59.9139,
            longitude: 10.7522,
            timezone: "Europe/Oslo".into(),
        };
        let calc = CalculationSettings {
            method: "MWL".into(),
            asr_rule: "standard".into(),
            high_latitude_rule: "middle".into(),
            offset_minutes: OffsetMinutes::default(),
            custom_fajr_angle: None,
            custom_isha_angle: None,
            custom_isha_minutes: None,
        };
        let t = compute_day_times(d, &oslo, &calc);
        assert!(t.get(PrayerKey::Fajr).is_some());
        assert!(t.get(PrayerKey::Isha).is_some());
        assert!(t.get(PrayerKey::Fajr).unwrap() < t.get(PrayerKey::Sunrise).unwrap());
    }

    #[test]
    fn qibla_bearing_toronto_and_post_isha_rollover() {
        let (bearing, km, wind) = qibla_bearing_and_distance(43.6532, -79.3832);
        assert!((54.0..=55.5).contains(&bearing), "bearing={bearing}");
        assert!((10_400.0..=10_600.0).contains(&km), "km={km}");
        assert!(matches!(wind, "NE" | "ENE"));

        // 2026-11-09T04:30:00Z = 2026-11-08 23:30 EST in Toronto (after Isha -> Next: Fajr).
        let now_ms = 1_794_198_600_000.0;
        let win = compute_current_window(now_ms, &toronto(), &isna_calc("standard"));
        assert_eq!(win.next_prayer, PrayerKey::Fajr);
        assert!(win.is_tomorrow_fajr);
    }

    #[test]
    fn hijri_date_nov8_2026() {
        let h = civil_to_hijri(CivilDate::from_ymd(2026, 11, 8).unwrap(), 0);
        assert_eq!(h.year, 1448);
        assert!(matches!(h.month, 5 | 6));
    }
}
