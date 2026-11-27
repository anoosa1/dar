//! Civil calendar arithmetic and browser Intl timezone/geolocation/clipboard bridges.
//! Native unit-test builds provide deterministic fallbacks (no external chrono crate needed).

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivilDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl CivilDate {
    pub fn from_ymd(year: i32, month: u8, day: u8) -> Option<Self> {
        if !(1..=12).contains(&month) {
            return None;
        }
        let max_d = days_in_month(year, month);
        if day < 1 || day > max_d {
            return None;
        }
        Some(Self { year, month, day })
    }

    /// Days since 1970-01-01 (Howard Hinnant's civil-from-days / days-from-civil).
    pub const fn days_since_epoch(self) -> i64 {
        let y = self.year as i64 - if self.month <= 2 { 1 } else { 0 };
        let m = self.month as i64;
        let d = self.day as i64;
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    pub const fn from_days_since_epoch(z: i64) -> Self {
        let z = z + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = mp + if mp < 10 { 3 } else { -9 };
        let year = (y + if m <= 2 { 1 } else { 0 }) as i32;
        Self {
            year,
            month: m as u8,
            day: d as u8,
        }
    }

    pub const fn add_days(self, delta: i64) -> Self {
        Self::from_days_since_epoch(self.days_since_epoch() + delta)
    }

    /// Day of week: 0 = Sunday .. 5 = Friday (Jumu'ah) .. 6 = Saturday.
    pub const fn weekday(self) -> u8 {
        ((self.days_since_epoch() + 4).rem_euclid(7)) as u8
    }

    pub fn format_iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    pub fn parse_iso(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let year = s[..4].parse::<i32>().ok()?;
        let month = s[5..7].parse::<u8>().ok()?;
        let day = s[8..10].parse::<u8>().ok()?;
        Self::from_ymd(year, month, day)
    }

    pub const fn month_name(self) -> &'static str {
        month_name(self.month)
    }

    pub const fn short_month_name(self) -> &'static str {
        short_month_name(self.month)
    }

    pub const fn weekday_name(self) -> &'static str {
        match self.weekday() {
            0 => "Sunday",
            1 => "Monday",
            2 => "Tuesday",
            3 => "Wednesday",
            4 => "Thursday",
            5 => "Friday",
            _ => "Saturday",
        }
    }

    pub const fn short_weekday_name(self) -> &'static str {
        match self.weekday() {
            0 => "Sun",
            1 => "Mon",
            2 => "Tue",
            3 => "Wed",
            4 => "Thu",
            5 => "Fri",
            _ => "Sat",
        }
    }
}

pub const fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub const fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

pub const fn month_name(month: u8) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "",
    }
}

pub const fn short_month_name(month: u8) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "",
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use wasm_bindgen::prelude::wasm_bindgen;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    #[wasm_bindgen(
        inline_js = r#"
export function now_ms() {
    return Date.now();
}
export function browser_timezone() {
    try {
        return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
    } catch {
        return "UTC";
    }
}
export function is_valid_timezone(tz) {
    try {
        new Intl.DateTimeFormat("en-US", { timeZone: tz });
        return true;
    } catch {
        return false;
    }
}
export function timezone_offset_minutes(epoch_ms, tz) {
    try {
        const fmt = new Intl.DateTimeFormat("en-US", {
            timeZone: tz,
            year: "numeric",
            month: "2-digit",
            day: "2-digit",
            hour: "2-digit",
            minute: "2-digit",
            second: "2-digit",
            hourCycle: "h23"
        });
        const parts = {};
        for (const p of fmt.formatToParts(new Date(epoch_ms))) {
            if (p.type !== "literal") parts[p.type] = p.value;
        }
        const y = Number(parts.year);
        const m = Number(parts.month) - 1;
        const d = Number(parts.day);
        const hh = Number(parts.hour);
        const mm = Number(parts.minute);
        const ss = Number(parts.second);
        const utcEquiv = Date.UTC(y, m, d, hh, mm, ss);
        const aligned = Math.floor(epoch_ms / 1000) * 1000;
        return Math.round((utcEquiv - aligned) / 60000);
    } catch {
        return 0;
    }
}
export function request_geolocation_promise() {
    return new Promise((resolve, reject) => {
        if (!navigator.geolocation) {
            reject(new Error("unavailable"));
            return;
        }
        navigator.geolocation.getCurrentPosition(
            (pos) => resolve([pos.coords.latitude, pos.coords.longitude]),
            (err) => reject(err || new Error("denied")),
            { enableHighAccuracy: false, timeout: 5000, maximumAge: 60000 }
        );
    });
}
export function trigger_print() {
    window.print();
}
export function copy_to_clipboard_promise(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
        return navigator.clipboard.writeText(text).then(() => true, () => false);
    }
    return Promise.resolve(false);
}
"#
    )]
    extern "C" {
        pub fn now_ms() -> f64;
        pub fn browser_timezone() -> String;
        pub fn is_valid_timezone(tz: &str) -> bool;
        pub fn timezone_offset_minutes(epoch_ms: f64, tz: &str) -> i32;
        fn request_geolocation_promise() -> js_sys::Promise;
        pub fn trigger_print();
        fn copy_to_clipboard_promise(text: &str) -> js_sys::Promise;
    }

    pub async fn request_geolocation() -> Result<(f64, f64), String> {
        let err_msg = "Location was not available. Enter coordinates manually.".to_owned();
        let val = JsFuture::from(request_geolocation_promise())
            .await
            .map_err(|_| err_msg.clone())?;
        let arr: js_sys::Array = val.dyn_into().map_err(|_| err_msg.clone())?;
        let lat = arr.get(0).as_f64().ok_or_else(|| err_msg.clone())?;
        let lon = arr.get(1).as_f64().ok_or(err_msg)?;
        Ok((lat, lon))
    }

    pub fn trigger_download(filename: &str, mime: &str, content: &str) {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let parts = js_sys::Array::of1(&JsValue::from_str(content));
        let opts = web_sys::BlobPropertyBag::new();
        opts.set_type(mime);
        let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts) else {
            return;
        };
        let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
            return;
        };
        if let Ok(el) = doc.create_element("a")
            && let Ok(anchor) = el.dyn_into::<web_sys::HtmlAnchorElement>()
        {
            anchor.set_href(&url);
            anchor.set_download(filename);
            if let Some(body) = doc.body() {
                let _ = body.append_child(&anchor);
                anchor.click();
                let _ = body.remove_child(&anchor);
            } else {
                anchor.click();
            }
        }
        let _ = web_sys::Url::revoke_object_url(&url);
    }

    pub async fn copy_to_clipboard(text: &str) -> bool {
        JsFuture::from(copy_to_clipboard_promise(text))
            .await
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    pub fn now_ms() -> f64 {
        1_794_160_800_000.0 // 2026-11-08T18:00:00Z
    }

    pub fn browser_timezone() -> String {
        "UTC".into()
    }

    pub fn is_valid_timezone(tz: &str) -> bool {
        if tz == "UTC" || tz == "GMT" {
            return true;
        }
        let Some((a, b)) = tz.split_once('/') else {
            return false;
        };
        !a.is_empty()
            && !b.is_empty()
            && !tz.contains(' ')
            && tz
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'_' | b'-' | b'+'))
    }

    pub fn timezone_offset_minutes(_epoch_ms: f64, tz: &str) -> i32 {
        match tz {
            "America/Toronto" | "America/New_York" => -300,
            "America/Chicago" => -360,
            "America/Los_Angeles" => -480,
            "Europe/London" | "UTC" => 0,
            "Europe/Paris" | "Europe/Berlin" | "Europe/Oslo" => 60,
            "Africa/Cairo" | "Europe/Istanbul" => 120,
            "Asia/Riyadh" => 180,
            "Asia/Dubai" => 240,
            "Asia/Karachi" => 300,
            "Asia/Dhaka" => 360,
            "Asia/Jakarta" => 420,
            "Asia/Kuala_Lumpur" | "Asia/Singapore" => 480,
            "Australia/Sydney" => 660,
            _ => 0,
        }
    }

    pub async fn request_geolocation() -> Result<(f64, f64), String> {
        Err("Location was not available. Enter coordinates manually.".into())
    }

    pub fn trigger_print() {}
    pub fn trigger_download(_filename: &str, _mime: &str, _content: &str) {}
    pub async fn copy_to_clipboard(_text: &str) -> bool {
        false
    }
}

pub use imp::{
    browser_timezone, copy_to_clipboard, is_valid_timezone, now_ms, request_geolocation,
    timezone_offset_minutes, trigger_download, trigger_print,
};

/// Returns the civil calendar date in `tz` corresponding to `epoch_ms`.
pub fn civil_date_in_tz(epoch_ms: f64, tz: &str) -> CivilDate {
    let offset_ms = timezone_offset_minutes(epoch_ms, tz) as f64 * 60_000.0;
    let local_days = ((epoch_ms + offset_ms) / 86_400_000.0).floor() as i64;
    CivilDate::from_days_since_epoch(local_days)
}

/// Converts a local `(date, minutes_since_midnight)` in `tz` to UTC `epoch_ms` using two-pass DST convergence.
pub fn epoch_ms_for_civil_minutes(date: CivilDate, minutes_since_midnight: f64, tz: &str) -> f64 {
    let naive_utc =
        (date.days_since_epoch() as f64) * 86_400_000.0 + minutes_since_midnight * 60_000.0;
    let off1 = timezone_offset_minutes(naive_utc, tz) as f64 * 60_000.0;
    let guess1 = naive_utc - off1;
    let off2 = timezone_offset_minutes(guess1, tz) as f64 * 60_000.0;
    naive_utc - off2
}

/// Returns minutes since local midnight `[0, 1440)` for `epoch_ms` in `tz`.
pub fn local_minutes_of_day(epoch_ms: f64, tz: &str) -> f64 {
    let offset_ms = timezone_offset_minutes(epoch_ms, tz) as f64 * 60_000.0;
    let local_ms = epoch_ms + offset_ms;
    (local_ms / 60_000.0).rem_euclid(1440.0)
}

/// Formats `epoch_ms` in `tz` as `"5:44 AM"` (`12h`) or `"05:44"` (`24h`), rounded to the nearest minute.
pub fn format_time_in_tz(epoch_ms: f64, tz: &str, use_24h: bool) -> String {
    let offset_ms = timezone_offset_minutes(epoch_ms, tz) as f64 * 60_000.0;
    let total_min = ((epoch_ms + offset_ms) / 60_000.0).round() as i64;
    let mins_in_day = total_min.rem_euclid(1440);
    let h24 = (mins_in_day / 60) as u32;
    let m = (mins_in_day % 60) as u32;
    if use_24h {
        format!("{h24:02}:{m:02}")
    } else {
        let suffix = if h24 < 12 { "AM" } else { "PM" };
        let h12 = match h24 % 12 {
            0 => 12,
            other => other,
        };
        format!("{h12}:{m:02} {suffix}")
    }
}

/// Formats UTC ISO timestamp for `.ics` (`YYYYMMDDTHHMMSSZ`).
pub fn format_ics_utc(epoch_ms: f64) -> String {
    let total_secs = (epoch_ms / 1000.0).round() as i64;
    let days = total_secs.div_euclid(86_400);
    let secs_in_day = total_secs.rem_euclid(86_400);
    let d = CivilDate::from_days_since_epoch(days);
    let hh = secs_in_day / 3600;
    let mm = (secs_in_day % 3600) / 60;
    let ss = secs_in_day % 60;
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        d.year, d.month, d.day, hh, mm, ss
    )
}

/// Formats remaining duration concisely (`"1h 12m"` or `"42m 18s"` when `< 1h`).
pub fn format_countdown(remaining_ms: f64) -> String {
    let total_secs = (remaining_ms / 1000.0).ceil().max(0.0) as u64;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours}h {mins:02}m")
    } else {
        format!("{mins}m {secs:02}s")
    }
}

pub fn format_duration_minutes(minutes: i32) -> String {
    let clamped = minutes.max(0);
    let h = clamped / 60;
    let m = clamped % 60;
    format!("{h}h {m:02}m")
}
