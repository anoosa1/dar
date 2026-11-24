//! Pure date/time math, chronological sorting, multi-day span track layout, overlapping
//! timed-event lanes, mini-month dots, year heatmap density and URL link extraction.

use std::cell::RefCell;
use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Value, json};
use wasm_bindgen::JsValue;

use super::{Collection, Item};
use crate::date_popover::{add_days, format_date};

pub const BASE: &str = "/api/v1/calendar/calendars";

pub const QUICK_COLORS: [(&str, &str); 8] = [
    ("#8b4f8c", "Orchid"),
    ("#3663c7", "Cobalt"),
    ("#267a5c", "Sage"),
    ("#b8631f", "Ochre"),
    ("#a94a76", "Rose"),
    ("#5b4b8a", "Violet"),
    ("#2f7d8c", "Teal"),
    ("#b23a48", "Crimson"),
];

thread_local! {
    static FORMATS: RefCell<HashMap<String, js_sys::Intl::DateTimeFormat>> = RefCell::new(HashMap::new());
    static TIMES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

#[derive(Deserialize)]
pub struct Calendars {
    pub items: Vec<Collection>,
}

#[derive(Clone)]
pub struct Editor {
    pub data: Value,
    pub resource: Option<Item>,
    pub occurrence: Option<String>,
    pub collection_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuickCreate {
    pub date: String,
    pub hour: u32,
    pub duration_mins: u32,
    pub all_day: bool,
}

#[derive(Clone)]
pub struct UndoToast {
    pub message: String,
    pub previous_item: Item,
    pub previous_data: Value,
    pub occurrence: Option<String>,
}

/// One placed multi-day (or single all-day) bar in a 7-day week row.
#[derive(Clone, Debug)]
pub struct SpanPlacement {
    pub event: Value,
    /// 1-based CSS grid column start (1..=7).
    pub col_start: usize,
    /// Number of columns spanned (1..=7).
    pub col_span: usize,
    /// 1-based track row (1..=N).
    pub row: usize,
    pub continues_left: bool,
    pub continues_right: bool,
}

/// "YYYY-MM-DDTHH:MM" of an instant in a zone, cached.
pub fn local(epoch: f64, zone: &str) -> String {
    let key = format!("{zone} {epoch}");
    if let Some(known) = TIMES.with(|t| t.borrow().get(&key).cloned()) {
        return known;
    }
    let value = FORMATS.with(|f| {
        let mut f = f.borrow_mut();
        let format = f.entry(zone.to_owned()).or_insert_with(|| {
            let options = js_sys::Object::new();
            for (k, v) in [
                ("timeZone", zone),
                ("year", "numeric"),
                ("month", "2-digit"),
                ("day", "2-digit"),
                ("hour", "2-digit"),
                ("minute", "2-digit"),
                ("hourCycle", "h23"),
            ] {
                let _ = js_sys::Reflect::set(&options, &k.into(), &v.into());
            }
            js_sys::Intl::DateTimeFormat::new(&js_sys::Array::of1(&"en-CA".into()), &options)
        });
        let parts = format.format_to_parts(&js_sys::Date::new(&JsValue::from_f64(epoch)));
        let mut map = HashMap::new();
        for part in parts.iter() {
            let get = |k: &str| {
                js_sys::Reflect::get(&part, &k.into())
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
            };
            map.insert(get("type"), get("value"));
        }
        let p = |k: &str| map.get(k).cloned().unwrap_or_default();
        format!(
            "{}-{}-{}T{}:{}",
            p("year"),
            p("month"),
            p("day"),
            p("hour"),
            p("minute")
        )
    });
    TIMES.with(|t| {
        let mut t = t.borrow_mut();
        if t.len() >= 10_000 {
            t.clear();
        }
        t.insert(key, value.clone());
    });
    value
}

/// Short timezone label for the time-grid corner (e.g. "EST", "EDT", "UTC").
pub fn zone_short(zone: &str) -> String {
    if zone == "UTC" {
        return "UTC".into();
    }
    let options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&options, &"timeZone".into(), &zone.into());
    let _ = js_sys::Reflect::set(&options, &"timeZoneName".into(), &"short".into());
    let fmt = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::of1(&"en-US".into()), &options);
    let parts = fmt.format_to_parts(&js_sys::Date::new_0());
    for part in parts.iter() {
        let ty = js_sys::Reflect::get(&part, &"type".into())
            .ok()
            .and_then(|v| v.as_string());
        if ty.as_deref() == Some("timeZoneName")
            && let Some(val) = js_sys::Reflect::get(&part, &"value".into())
                .ok()
                .and_then(|v| v.as_string())
        {
            return val;
        }
    }
    zone.split('/').next_back().unwrap_or(zone).to_owned()
}

pub fn parse_ms(value: &str) -> f64 {
    js_sys::Date::parse(value)
}

pub fn at(event: &Value, key: &str) -> f64 {
    parse_ms(event[key].as_str().unwrap_or_default())
}

pub fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_owned()
}

pub fn weekday(date: &str) -> u32 {
    js_sys::Date::new(&JsValue::from_f64(parse_ms(&format!("{date}T12:00:00Z")))).get_utc_day()
}

pub fn is_weekend(date: &str) -> bool {
    matches!(weekday(date), 0 | 6)
}

pub fn day(date: &str, options: &[(&str, &str)]) -> String {
    format_date(date, options)
}

/// Format an `HH:MM` wall time in either `"24h"` or `"12h"` display format.
pub fn format_hhmm(hhmm: &str, format: &str) -> String {
    if format != "12h" || hhmm.len() < 5 {
        return hhmm.to_owned();
    }
    let Ok(h) = hhmm[0..2].parse::<u32>() else {
        return hhmm.to_owned();
    };
    let m = &hhmm[3..5];
    let suffix = if h < 12 { "AM" } else { "PM" };
    let h12 = match h % 12 {
        0 => 12,
        n => n,
    };
    if m == "00" {
        format!("{h12} {suffix}")
    } else {
        format!("{h12}:{m} {suffix}")
    }
}

/// Format an hour index `0..24` for the time-grid gutter.
pub fn format_hour_label(hour: u32, format: &str) -> String {
    if format == "12h" {
        match hour {
            0 => "12 AM".into(),
            1..=11 => format!("{hour} AM"),
            12 => "12 PM".into(),
            13..=23 => format!("{} PM", hour - 12),
            _ => format!("{hour:02}:00"),
        }
    } else {
        format!("{hour:02}:00")
    }
}

/// Format duration between two timestamps in milliseconds (e.g. "30m", "1h 30m").
pub fn duration_short(start_ms: f64, end_ms: f64) -> String {
    let mins = ((end_ms - start_ms) / 60_000.0).round().max(0.0) as i64;
    if mins >= 1440 {
        let d = mins / 1440;
        format!("{d}d")
    } else if mins >= 60 {
        let h = mins / 60;
        let rem = mins % 60;
        if rem > 0 {
            format!("{h}h {rem}m")
        } else {
            format!("{h}h")
        }
    } else {
        format!("{mins}m")
    }
}

/// The visible days of a view (`(start_date, count)`).
pub fn range(anchor: &str, view: &str, first: u32) -> (String, i64) {
    match view {
        "day" => (anchor.to_owned(), 1),
        "agenda" => (anchor.to_owned(), 30),
        "year" => {
            let y: i32 = anchor[..4].parse().unwrap_or(2026);
            let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
            (format!("{y:04}-01-01"), if leap { 366 } else { 365 })
        }
        "month" => {
            let start = format!("{}-01", &anchor[..7]);
            let offset = (weekday(&start) + 7 - first) % 7;
            (add_days(&start, -(offset as i64)), 42)
        }
        _ => (
            add_days(anchor, -(((weekday(anchor) + 7 - first) % 7) as i64)),
            7,
        ),
    }
}

/// Shift anchor by one view period (`-1` or `+1`), including `"year"`.
pub fn shift_view(anchor: &str, view: &str, direction: i64) -> String {
    if view == "year" {
        let y: i32 = anchor[..4].parse().unwrap_or(2026);
        let next_y = (y as i64 + direction).clamp(1, 9999);
        return format!("{next_y:04}{}", &anchor[4..]);
    }
    crate::date_popover::shift(anchor, view, direction)
}

/// "November 2026", "Nov 23 – 29, 2026", "Tuesday, November 24, 2026" or "2026".
pub fn period_title(anchor: &str, view: &str, days: &[String]) -> String {
    match view {
        "year" => anchor[..4].to_owned(),
        "day" => day(
            anchor,
            &[
                ("weekday", "long"),
                ("month", "long"),
                ("day", "numeric"),
                ("year", "numeric"),
            ],
        ),
        "week" => {
            let (first, last) = (&days[0], days.last().expect("week"));
            let month = |d: &str| day(d, &[("month", "short")]);
            let d = |date: &str| date[8..].trim_start_matches('0').to_owned();
            if first[..4] != last[..4] {
                format!(
                    "{} {}, {} – {} {}, {}",
                    month(first),
                    d(first),
                    &first[..4],
                    month(last),
                    d(last),
                    &last[..4]
                )
            } else if first[..7] == last[..7] {
                format!(
                    "{} {} – {}, {}",
                    month(first),
                    d(first),
                    d(last),
                    &first[..4]
                )
            } else {
                format!(
                    "{} {} – {} {}, {}",
                    month(first),
                    d(first),
                    month(last),
                    d(last),
                    &first[..4]
                )
            }
        }
        _ => day(anchor, &[("month", "long"), ("year", "numeric")]),
    }
}

/// The event card's date line; all-day ranges show inclusive end dates.
pub fn event_when(event: &Value, zone: &str) -> String {
    if event["allDay"] == json!(true) {
        let start = &s(event, "start")[..10.min(s(event, "start").len())];
        let end_raw = s(event, "end");
        let end_prefix = &end_raw[..10.min(end_raw.len())];
        let last = if end_prefix.len() == 10 {
            add_days(end_prefix, -1)
        } else {
            start.to_owned()
        };
        return if last.as_str() <= start {
            day(
                start,
                &[("weekday", "long"), ("month", "long"), ("day", "numeric")],
            )
        } else {
            format!(
                "{} – {}",
                day(start, &[("month", "short"), ("day", "numeric")]),
                day(
                    &last,
                    &[("month", "short"), ("day", "numeric"), ("year", "numeric")]
                )
            )
        };
    }
    let (start_at, end_at) = (at(event, "startAt"), at(event, "endAt"));
    let (start, end) = (
        local(start_at, zone)[..10].to_owned(),
        local(end_at, zone)[..10].to_owned(),
    );
    if start == end || end_at - start_at <= 86_400_000.0 && &local(end_at, zone)[11..] == "00:00" {
        day(
            &start,
            &[("weekday", "long"), ("month", "long"), ("day", "numeric")],
        )
    } else {
        format!(
            "{} – {}",
            day(
                &start,
                &[("weekday", "short"), ("month", "short"), ("day", "numeric")]
            ),
            day(
                &end,
                &[("weekday", "short"), ("month", "short"), ("day", "numeric")]
            )
        )
    }
}

pub fn reminder_text(minutes: i64) -> String {
    if minutes % 1440 == 0 {
        format!(
            "{} day{}",
            minutes / 1440,
            if minutes == 1440 { "" } else { "s" }
        )
    } else if minutes % 60 == 0 {
        format!(
            "{} hour{}",
            minutes / 60,
            if minutes == 60 { "" } else { "s" }
        )
    } else {
        format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
    }
}

pub fn occurrence_key(e: &Value) -> String {
    format!("{}/{}", s(e, "resourceId"), s(e, "recurrenceId"))
}

/// Inclusive civil date bounds `[first_day, last_day]` for any event in `zone`.
pub fn event_day_bounds(event: &Value, zone: &str) -> (String, String) {
    if event["allDay"] == json!(true) {
        let start_raw = s(event, "start");
        let end_raw = s(event, "end");
        let from = if start_raw.len() >= 10 {
            start_raw[..10].to_owned()
        } else {
            local(at(event, "startAt"), zone)[..10].to_owned()
        };
        let to = if end_raw.len() >= 10 {
            let last = add_days(&end_raw[..10], -1);
            if last < from { from.clone() } else { last }
        } else {
            from.clone()
        };
        return (from, to);
    }
    let from = local(at(event, "startAt"), zone)[..10].to_owned();
    let end = local(at(event, "endAt"), zone);
    let end_day = &end[..10];
    let to = if end_day > from.as_str() && &end[11..] == "00:00" {
        add_days(end_day, -1)
    } else {
        end_day.to_owned()
    };
    let to = if to < from { from.clone() } else { to };
    (from, to)
}

pub fn on_day(event: &Value, date: &str, zone: &str) -> bool {
    let (from, to) = event_day_bounds(event, zone);
    from.as_str() <= date && date <= to.as_str()
}

/// True when an event should render as a horizontal spanning ribbon (all-day OR multi-day).
pub fn is_spanning(event: &Value, zone: &str) -> bool {
    if event["allDay"] == json!(true) {
        return true;
    }
    let (from, to) = event_day_bounds(event, zone);
    to > from
}

/// Sort events chronologically: all-day / multi-day spans first (longer first), then timed
/// events by start timestamp ascending, longer duration first, then title.
pub fn sort_events(events: &mut [Value]) {
    events.sort_by(|a, b| {
        let a_all = a["allDay"] == json!(true);
        let b_all = b["allDay"] == json!(true);
        b_all
            .cmp(&a_all)
            .then_with(|| at(a, "startAt").total_cmp(&at(b, "startAt")))
            .then_with(|| {
                let da = at(a, "endAt") - at(a, "startAt");
                let db = at(b, "endAt") - at(b, "startAt");
                db.total_cmp(&da)
            })
            .then_with(|| {
                s(a, "title")
                    .to_lowercase()
                    .cmp(&s(b, "title").to_lowercase())
            })
    });
}

/// Compute aligned non-overlapping horizontal tracks for all-day / multi-day events in a week.
pub fn week_spans(week_days: &[String], events: &[Value], zone: &str) -> Vec<SpanPlacement> {
    if week_days.is_empty() {
        return Vec::new();
    }
    let first_day = &week_days[0];
    let last_day = &week_days[week_days.len() - 1];
    let mut candidates: Vec<(usize, usize, bool, bool, Value)> = Vec::new();
    for ev in events {
        if !is_spanning(ev, zone) {
            continue;
        }
        let (from, to) = event_day_bounds(ev, zone);
        if to.as_str() < first_day.as_str() || from.as_str() > last_day.as_str() {
            continue;
        }
        let continues_left = from.as_str() < first_day.as_str();
        let continues_right = to.as_str() > last_day.as_str();
        let start_idx = week_days
            .iter()
            .position(|d| d.as_str() >= from.as_str())
            .unwrap_or(0);
        let end_idx = week_days
            .iter()
            .rposition(|d| d.as_str() <= to.as_str())
            .unwrap_or(week_days.len() - 1);
        let span = (end_idx + 1).saturating_sub(start_idx).max(1);
        candidates.push((start_idx, span, continues_left, continues_right, ev.clone()));
    }
    // Sort by start column asc, span desc (longest ribbons sit on top!), title asc.
    candidates.sort_by(|a, b| {
        a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then_with(|| {
            s(&a.4, "title")
                .to_lowercase()
                .cmp(&s(&b.4, "title").to_lowercase())
        })
    });

    let cols = week_days.len();
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    let mut placements = Vec::new();
    for (start_idx, span, cl, cr, event) in candidates {
        let end = (start_idx + span).min(cols);
        let row_idx = occupied
            .iter()
            .position(|track| (start_idx..end).all(|c| !track[c]))
            .unwrap_or_else(|| {
                occupied.push(vec![false; cols]);
                occupied.len() - 1
            });
        for c in start_idx..end {
            occupied[row_idx][c] = true;
        }
        placements.push(SpanPlacement {
            event,
            col_start: start_idx + 1,
            col_span: end - start_idx,
            row: row_idx + 1,
            continues_left: cl,
            continues_right: cr,
        });
    }
    placements
}

/// Side-by-side lanes for overlapping timed events, per day.
pub fn day_lanes(days: &[String], events: &[Value], zone: &str) -> HashMap<String, (usize, usize)> {
    let mut result = HashMap::new();
    for date in days {
        let mut rows: Vec<&Value> = events
            .iter()
            .filter(|e| e["allDay"] != json!(true) && local(at(e, "startAt"), zone)[..10] == *date)
            .collect();
        rows.sort_by(|a, b| {
            at(a, "startAt")
                .total_cmp(&at(b, "startAt"))
                .then(at(b, "endAt").total_cmp(&at(a, "endAt")))
        });
        let (mut cluster, mut ends, mut cluster_end): (Vec<(String, usize)>, Vec<f64>, f64) =
            (Vec::new(), Vec::new(), 0.0);
        let mut flush = |cluster: &mut Vec<(String, usize)>, ends: &mut Vec<f64>| {
            let lanes = ends.len().max(1);
            for (key, lane) in cluster.drain(..) {
                result.insert(key, (lane, lanes));
            }
            ends.clear();
        };
        for e in rows {
            let start = at(e, "startAt");
            let end = at(e, "endAt").max(start + 900_000.0);
            if !cluster.is_empty() && start >= cluster_end {
                flush(&mut cluster, &mut ends);
            }
            let lane = match ends.iter().position(|v| *v <= start) {
                Some(l) => {
                    ends[l] = end;
                    l
                }
                None => {
                    ends.push(end);
                    ends.len() - 1
                }
            };
            cluster.push((format!("{date}/{}", occurrence_key(e)), lane));
            cluster_end = cluster_end.max(end);
        }
        flush(&mut cluster, &mut ends);
    }
    result
}

/// Up to 3 distinct calendar colors per date for the sidebar MiniMonth & phone 7-day strip.
pub fn calendar_dots(
    days: &[String],
    events: &[Value],
    calendars: &[Collection],
    zone: &str,
) -> HashMap<String, Vec<String>> {
    let color_of = |cid: &str| -> String {
        calendars
            .iter()
            .find(|c| c.id == cid)
            .map(|c| c.color.clone())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "#73396f".into())
    };
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for d in days {
        let mut colors: Vec<String> = Vec::new();
        for ev in events {
            if colors.len() >= 3 {
                break;
            }
            if on_day(ev, d, zone) {
                let c = color_of(&s(ev, "collectionId"));
                if !colors.contains(&c) {
                    colors.push(c);
                }
            }
        }
        if !colors.is_empty() {
            map.insert(d.clone(), colors);
        }
    }
    map
}

/// Extract an http(s) URL embedded in a location string (e.g. "Room B / https://meet.example/x").
/// Returns `(prefix_text, url, suffix_text)` if found.
pub fn split_location_url(location: &str) -> Option<(String, String, String)> {
    let idx = location
        .find("https://")
        .or_else(|| location.find("http://"))?;
    let rest = &location[idx..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '>' | '"' | '\''))
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches([',', '.', ';']);
    if url.len() < 10 {
        return None;
    }
    let prefix = location[..idx]
        .trim_end_matches([' ', '/', '·', '-', '|'])
        .to_owned();
    let suffix = rest[url.len()..].trim().to_owned();
    Some((prefix, url.to_owned(), suffix))
}

/// Relative human countdown until `target_ms` from `now_ms` (e.g. "in 25m", "in 1h 35m", "Now").
pub fn relative_countdown(now_ms: f64, target_ms: f64) -> String {
    let diff_mins = ((target_ms - now_ms) / 60_000.0).ceil() as i64;
    if diff_mins <= 0 {
        "Now".into()
    } else if diff_mins < 60 {
        format!("in {diff_mins}m")
    } else {
        let h = diff_mins / 60;
        let m = diff_mins % 60;
        if m == 0 {
            format!("in {h}h")
        } else {
            format!("in {h}h {m}m")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_12h_and_24h_times() {
        assert_eq!(format_hhmm("00:00", "12h"), "12 AM");
        assert_eq!(format_hhmm("09:30", "12h"), "9:30 AM");
        assert_eq!(format_hhmm("12:00", "12h"), "12 PM");
        assert_eq!(format_hhmm("15:45", "12h"), "3:45 PM");
        assert_eq!(format_hhmm("15:45", "24h"), "15:45");
        assert_eq!(format_hour_label(0, "12h"), "12 AM");
        assert_eq!(format_hour_label(13, "12h"), "1 PM");
        assert_eq!(format_hour_label(13, "24h"), "13:00");
    }

    #[test]
    fn extracts_embedded_location_urls() {
        let split = split_location_url("Design Studio / https://meet.dar.local/cal-ux").unwrap();
        assert_eq!(split.0, "Design Studio");
        assert_eq!(split.1, "https://meet.dar.local/cal-ux");
        assert_eq!(split.2, "");
        assert!(split_location_url("Room 4B, 2nd floor").is_none());
    }

    #[test]
    fn formats_relative_countdown() {
        assert_eq!(relative_countdown(0.0, 25.0 * 60_000.0), "in 25m");
        assert_eq!(relative_countdown(0.0, 95.0 * 60_000.0), "in 1h 35m");
        assert_eq!(relative_countdown(1000.0, 500.0), "Now");
    }
}
