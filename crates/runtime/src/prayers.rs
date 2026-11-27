//! Built-in Prayer Times settings and prayer-tracking log API (`/api/v1/prayers/*`).
//! Backed by `service_app_data` under `org.dar.prayer-times`.

use std::collections::{BTreeMap, HashMap};

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use chrono::NaiveDate;
use serde_json::{Map, Value, json};

use crate::db::Database;
use crate::error::{AppError, Result};
use crate::http::{body_object, json_response, ok};

const APP_ID: &str = "org.dar.prayer-times";
const SETTINGS_KEY: &str = "settings";
const BODY_LIMIT: usize = 64 * 1024;
const MAX_LOCATIONS: usize = 24;
const MIN_YEAR: u32 = 2000;
const MAX_YEAR: u32 = 2099;

const METHODS: [&str; 10] = [
    "MWL",
    "ISNA",
    "Egypt",
    "Karachi",
    "UmmAlQura",
    "Diyanet",
    "MUIS",
    "UOIF",
    "Tehran",
    "Custom",
];
const ASR_RULES: [&str; 2] = ["standard", "hanafi"];
const HIGH_LATITUDE_RULES: [&str; 4] = ["none", "middle", "seventh", "angle"];
const ALL_PRAYERS: [&str; 6] = ["fajr", "sunrise", "dhuhr", "asr", "maghrib", "isha"];
const LOG_PRAYERS: [&str; 5] = ["fajr", "dhuhr", "asr", "maghrib", "isha"];
const LOG_STATUSES: [&str; 3] = ["unprayed", "on_time", "late"];

enum LogPatch {
    Single { prayer: String, status: String },
    Day(Map<String, Value>),
}

fn json_with_etag(value: &Value, rev: u64) -> Response {
    let mut response = json_response(value, StatusCode::OK);
    if let Ok(tag) = HeaderValue::from_str(&format!("\"{rev}\"")) {
        response.headers_mut().insert(header::ETAG, tag);
    }
    response
}

fn parse_if_match(headers: &HeaderMap) -> Result<u32> {
    let raw = headers
        .get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let digits = raw
        .strip_prefix('"')
        .and_then(|h| h.strip_suffix('"'))
        .filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    let Some(digits) = digits else {
        return Err(AppError::new("preconditionRequired", 428));
    };
    digits
        .parse::<u64>()
        .ok()
        .filter(|r| *r <= u32::MAX as u64)
        .ok_or_else(AppError::invalid)
        .map(|r| r as u32)
}

fn as_int(value: &Value) -> Option<i64> {
    if let Some(i) = value.as_i64() {
        return Some(i);
    }
    if let Some(u) = value.as_u64() {
        return i64::try_from(u).ok();
    }
    value
        .as_f64()
        .filter(|f| f.is_finite() && f.fract() == 0.0 && *f >= -9e15 && *f <= 9e15)
        .map(|f| f as i64)
}

fn parse_year_str(s: &str) -> Result<u32> {
    if s.len() != 4 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AppError::invalid());
    }
    let year: u32 = s.parse().map_err(|_| AppError::invalid())?;
    if !(MIN_YEAR..=MAX_YEAR).contains(&year) {
        return Err(AppError::invalid());
    }
    Ok(year)
}

fn parse_year(query: &HashMap<String, String>) -> Result<u32> {
    let raw = query.get("year").map(String::as_str).unwrap_or("");
    parse_year_str(raw)
}

fn parse_civil_date(date: &str) -> Result<u32> {
    let bytes = date.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes[..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return Err(AppError::invalid());
    }
    let year = parse_year_str(&date[..4])?;
    let month: u32 = date[5..7].parse().map_err(|_| AppError::invalid())?;
    let day: u32 = date[8..10].parse().map_err(|_| AppError::invalid())?;
    NaiveDate::from_ymd_opt(year as i32, month, day).ok_or_else(AppError::invalid)?;
    Ok(year)
}

fn validate_location(loc: &Value) -> Result<()> {
    let obj = loc.as_object().ok_or_else(AppError::invalid)?;
    let label = obj
        .get("label")
        .and_then(Value::as_str)
        .ok_or_else(AppError::invalid)?;
    if label.trim().is_empty() || label.chars().count() > 100 {
        return Err(AppError::invalid());
    }
    let lat = obj
        .get("latitude")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && (-90.0..=90.0).contains(v));
    let lon = obj
        .get("longitude")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && (-180.0..=180.0).contains(v));
    if lat.is_none() || lon.is_none() {
        return Err(AppError::invalid());
    }
    let tz = obj
        .get("timezone")
        .and_then(Value::as_str)
        .ok_or_else(AppError::invalid)?;
    if tz.trim().is_empty() || tz.len() > 100 || tz.parse::<chrono_tz::Tz>().is_err() {
        return Err(AppError::invalid());
    }
    Ok(())
}

fn validate_settings_value(value: &Value) -> Result<()> {
    let obj = value.as_object().ok_or_else(AppError::invalid)?;
    validate_location(obj.get("location").ok_or_else(AppError::invalid)?)?;

    if let Some(locations) = obj.get("locations").filter(|v| !v.is_null()) {
        let arr = locations.as_array().ok_or_else(AppError::invalid)?;
        if arr.len() > MAX_LOCATIONS {
            return Err(AppError::invalid());
        }
        for item in arr {
            let entry = item.as_object().ok_or_else(AppError::invalid)?;
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(AppError::invalid)?;
            if id.trim().is_empty() || id.chars().count() > 100 {
                return Err(AppError::invalid());
            }
            let tag = entry
                .get("tag")
                .and_then(Value::as_str)
                .ok_or_else(AppError::invalid)?;
            if tag.chars().count() > 100 {
                return Err(AppError::invalid());
            }
            validate_location(entry.get("location").ok_or_else(AppError::invalid)?)?;
        }
    }

    let calc = obj
        .get("calculation")
        .and_then(Value::as_object)
        .ok_or_else(AppError::invalid)?;
    let method = calc
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(AppError::invalid)?;
    if !METHODS.contains(&method) {
        return Err(AppError::invalid());
    }
    let asr = calc
        .get("asrRule")
        .and_then(Value::as_str)
        .ok_or_else(AppError::invalid)?;
    if !ASR_RULES.contains(&asr) {
        return Err(AppError::invalid());
    }
    let high_lat = calc
        .get("highLatitudeRule")
        .and_then(Value::as_str)
        .ok_or_else(AppError::invalid)?;
    if !HIGH_LATITUDE_RULES.contains(&high_lat) {
        return Err(AppError::invalid());
    }
    let offsets = calc
        .get("offsetMinutes")
        .and_then(Value::as_object)
        .ok_or_else(AppError::invalid)?;
    if offsets.keys().any(|k| !ALL_PRAYERS.contains(&k.as_str())) {
        return Err(AppError::invalid());
    }
    for prayer in ALL_PRAYERS {
        let offset = offsets
            .get(prayer)
            .and_then(as_int)
            .ok_or_else(AppError::invalid)?;
        if !(-120..=120).contains(&offset) {
            return Err(AppError::invalid());
        }
    }

    for container in [calc, obj] {
        for key in ["customFajrAngle", "customIshaAngle"] {
            if let Some(v) = container.get(key).filter(|v| !v.is_null()) {
                let valid = v
                    .as_f64()
                    .is_some_and(|a| a.is_finite() && (0.0..=30.0).contains(&a));
                if !valid {
                    return Err(AppError::invalid());
                }
            }
        }
        if let Some(v) = container.get("customIshaMinutes").filter(|v| !v.is_null()) {
            let valid = as_int(v).is_some_and(|m| (1..=180).contains(&m));
            if !valid {
                return Err(AppError::invalid());
            }
        }
    }

    if let Some(display) = obj.get("display").filter(|v| !v.is_null()) {
        let map = display.as_object().ok_or_else(AppError::invalid)?;
        for (k, v) in map {
            match k.as_str() {
                "timeFormat" => {
                    if !matches!(v.as_str(), Some("12h" | "24h")) {
                        return Err(AppError::invalid());
                    }
                }
                "hijriOffset" => {
                    if !as_int(v).is_some_and(|n| (-2..=2).contains(&n)) {
                        return Err(AppError::invalid());
                    }
                }
                _ => {
                    if !v.is_boolean() {
                        return Err(AppError::invalid());
                    }
                }
            }
        }
    }

    Ok(())
}

fn validate_settings_body(input: &Map<String, Value>) -> Result<(u32, Value)> {
    if input.keys().any(|k| k != "schemaVersion" && k != "value") {
        return Err(AppError::invalid());
    }
    let schema_version = match input.get("schemaVersion").and_then(as_int) {
        Some(1) => 1u32,
        Some(2) => 2u32,
        _ => return Err(AppError::invalid()),
    };
    let value = input.get("value").ok_or_else(AppError::invalid)?;
    validate_settings_value(value)?;
    Ok((schema_version, value.clone()))
}

fn empty_day() -> Map<String, Value> {
    let mut day = Map::new();
    for prayer in LOG_PRAYERS {
        day.insert(prayer.to_owned(), json!("unprayed"));
    }
    day
}

fn normalize_day(value: &Value) -> Result<Map<String, Value>> {
    let obj = value.as_object().ok_or_else(AppError::invalid)?;
    if obj.keys().any(|k| !LOG_PRAYERS.contains(&k.as_str())) {
        return Err(AppError::invalid());
    }
    let mut day = empty_day();
    for (k, v) in obj {
        let status = v.as_str().ok_or_else(AppError::invalid)?;
        if !LOG_STATUSES.contains(&status) {
            return Err(AppError::invalid());
        }
        day.insert(k.clone(), json!(status));
    }
    Ok(day)
}

fn is_all_unprayed(day: &Map<String, Value>) -> bool {
    LOG_PRAYERS
        .iter()
        .all(|p| day.get(*p).and_then(Value::as_str) == Some("unprayed"))
}

fn validate_log_days(year: u32, raw_days: &Map<String, Value>) -> Result<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    for (date, val) in raw_days {
        if parse_civil_date(date)? != year {
            return Err(AppError::invalid());
        }
        let normalized = normalize_day(val)?;
        if !is_all_unprayed(&normalized) {
            out.insert(date.clone(), Value::Object(normalized));
        }
    }
    Ok(out)
}

fn days_to_value(days: &BTreeMap<String, Value>) -> Value {
    let mut map = Map::new();
    for (k, v) in days {
        map.insert(k.clone(), v.clone());
    }
    Value::Object(map)
}

fn extract_stored_days(current: &Value) -> BTreeMap<String, Value> {
    if current.is_null() {
        return BTreeMap::new();
    }
    let Some(obj) = current["value"].get("days").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    obj.iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn parse_log_patch(patch: &Map<String, Value>) -> Result<LogPatch> {
    if patch.contains_key("prayer") || patch.contains_key("status") {
        if patch.len() != 2 || !patch.contains_key("prayer") || !patch.contains_key("status") {
            return Err(AppError::invalid());
        }
        let prayer = patch
            .get("prayer")
            .and_then(Value::as_str)
            .filter(|p| LOG_PRAYERS.contains(p))
            .ok_or_else(AppError::invalid)?
            .to_owned();
        let status = patch
            .get("status")
            .and_then(Value::as_str)
            .filter(|s| LOG_STATUSES.contains(s))
            .ok_or_else(AppError::invalid)?
            .to_owned();
        Ok(LogPatch::Single { prayer, status })
    } else if let Some(day_val) = patch.get("day") {
        if patch.len() != 1 {
            return Err(AppError::invalid());
        }
        Ok(LogPatch::Day(normalize_day(day_val)?))
    } else {
        Err(AppError::invalid())
    }
}

fn apply_log_patch(
    days: &mut BTreeMap<String, Value>,
    date: &str,
    patch: LogPatch,
) -> Map<String, Value> {
    let normalized = match patch {
        LogPatch::Single { prayer, status } => {
            let mut day = days
                .get(date)
                .and_then(|v| normalize_day(v).ok())
                .unwrap_or_else(empty_day);
            day.insert(prayer, json!(status));
            day
        }
        LogPatch::Day(day) => day,
    };
    if is_all_unprayed(&normalized) {
        days.remove(date);
    } else {
        days.insert(date.to_owned(), Value::Object(normalized.clone()));
    }
    normalized
}

/// Handles `/api/v1/prayers/*` requests for settings and prayer log records.
pub async fn handle_http(
    db: &Database,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    body: Body,
    session_id: &str,
) -> Result<Response> {
    if path == "/api/v1/prayers/settings" {
        return match *method {
            Method::GET => {
                let result = db
                    .read(
                        "service_app_data",
                        json!([session_id, APP_ID, "get", SETTINGS_KEY, 0, 1, ""]),
                    )
                    .await?;
                if result.is_null() {
                    return ok(Value::Null);
                }
                let rev = result["revision"].as_u64().unwrap_or(0);
                Ok(json_with_etag(&result, rev))
            }
            Method::PUT => {
                let revision = parse_if_match(headers)?;
                let input = body_object(body, BODY_LIMIT, headers).await?;
                let (schema_version, value) = validate_settings_body(&input)?;
                let stored = db
                    .call(
                        "service_app_data",
                        json!([
                            session_id,
                            APP_ID,
                            "put",
                            SETTINGS_KEY,
                            revision,
                            schema_version,
                            value.to_string()
                        ]),
                    )
                    .await?;
                let next_rev = stored["revision"]
                    .as_u64()
                    .unwrap_or(u64::from(revision) + 1);
                Ok(json_with_etag(&stored, next_rev))
            }
            Method::DELETE => {
                let revision = parse_if_match(headers)?;
                let result = db
                    .call(
                        "service_app_data",
                        json!([session_id, APP_ID, "delete", SETTINGS_KEY, revision, 1, ""]),
                    )
                    .await?;
                ok(result)
            }
            _ => Err(AppError::new("methodNotAllowed", 405)),
        };
    }

    if path == "/api/v1/prayers/log" {
        return match *method {
            Method::GET => {
                let year = parse_year(query)?;
                let key = format!("log-{year}");
                let current = db
                    .read(
                        "service_app_data",
                        json!([session_id, APP_ID, "get", key, 0, 1, ""]),
                    )
                    .await?;
                let rev = if current.is_null() {
                    0
                } else {
                    current["revision"].as_u64().unwrap_or(0)
                };
                let days = days_to_value(&extract_stored_days(&current));
                Ok(json_with_etag(
                    &json!({ "year": year, "revision": rev, "days": days }),
                    rev,
                ))
            }
            Method::PUT => {
                let year = parse_year(query)?;
                let revision = parse_if_match(headers)?;
                let input = body_object(body, BODY_LIMIT, headers).await?;
                if input.keys().any(|k| k != "days") {
                    return Err(AppError::invalid());
                }
                let raw_days = input
                    .get("days")
                    .and_then(Value::as_object)
                    .ok_or_else(AppError::invalid)?;
                let pruned = days_to_value(&validate_log_days(year, raw_days)?);
                let key = format!("log-{year}");
                let stored = db
                    .call(
                        "service_app_data",
                        json!([
                            session_id,
                            APP_ID,
                            "put",
                            key,
                            revision,
                            1,
                            json!({ "days": pruned }).to_string()
                        ]),
                    )
                    .await?;
                let next_rev = stored["revision"]
                    .as_u64()
                    .unwrap_or(u64::from(revision) + 1);
                Ok(json_with_etag(
                    &json!({ "year": year, "revision": next_rev, "days": pruned }),
                    next_rev,
                ))
            }
            _ => Err(AppError::new("methodNotAllowed", 405)),
        };
    }

    if let Some(date) = path
        .strip_prefix("/api/v1/prayers/log/")
        .filter(|d| !d.is_empty() && !d.contains('/'))
    {
        if *method != Method::PATCH {
            return Err(AppError::new("methodNotAllowed", 405));
        }
        let year = parse_civil_date(date)?;
        let supplied_rev = parse_if_match(headers)?;
        let input = body_object(body, BODY_LIMIT, headers).await?;
        let patch = parse_log_patch(&input)?;
        let key = format!("log-{year}");
        let current = db
            .read(
                "service_app_data",
                json!([session_id, APP_ID, "get", key, 0, 1, ""]),
            )
            .await?;
        let current_rev = if current.is_null() {
            0
        } else {
            current["revision"].as_u64().unwrap_or(0) as u32
        };
        if current_rev != supplied_rev {
            return Err(AppError::new("conflict", 409));
        }
        let mut days = extract_stored_days(&current);
        let normalized_day = apply_log_patch(&mut days, date, patch);
        let pruned = days_to_value(&days);
        let stored = db
            .call(
                "service_app_data",
                json!([
                    session_id,
                    APP_ID,
                    "put",
                    key,
                    current_rev,
                    1,
                    json!({ "days": pruned }).to_string()
                ]),
            )
            .await?;
        let next_rev = stored["revision"]
            .as_u64()
            .unwrap_or(u64::from(current_rev) + 1);
        return Ok(json_with_etag(
            &json!({
                "year": year,
                "revision": next_rev,
                "date": date,
                "day": normalized_day,
                "days": pruned,
            }),
            next_rev,
        ));
    }

    Err(AppError::not_found())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_location() -> Value {
        json!({
            "label": "Toronto",
            "latitude": 43.6532,
            "longitude": -79.3832,
            "timezone": "America/Toronto"
        })
    }

    fn valid_offsets() -> Value {
        json!({
            "fajr": 0,
            "sunrise": -2,
            "dhuhr": 5,
            "asr": 0,
            "maghrib": 3,
            "isha": -10
        })
    }

    fn valid_settings_v1() -> Map<String, Value> {
        serde_json::from_value(json!({
            "schemaVersion": 1,
            "value": {
                "location": valid_location(),
                "calculation": {
                    "method": "ISNA",
                    "asrRule": "standard",
                    "highLatitudeRule": "angle",
                    "offsetMinutes": valid_offsets()
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn civil_date_and_leap_year_validation() {
        assert_eq!(parse_civil_date("2024-02-29").unwrap(), 2024);
        assert_eq!(parse_civil_date("2000-02-29").unwrap(), 2000);
        assert_eq!(parse_civil_date("2099-12-31").unwrap(), 2099);

        assert!(parse_civil_date("2026-02-29").is_err());
        assert!(parse_civil_date("2026-02-30").is_err());
        assert!(parse_civil_date("2026-04-31").is_err());
        assert!(parse_civil_date("2026-13-01").is_err());
        assert!(parse_civil_date("1999-12-31").is_err());
        assert!(parse_civil_date("2100-01-01").is_err());
        assert!(parse_civil_date("2024-2-29").is_err());
        assert!(parse_civil_date("bad").is_err());
    }

    #[test]
    fn year_and_if_match_parsing() {
        let mut q = HashMap::new();
        assert!(parse_year(&q).is_err());
        q.insert("year".into(), "2026".into());
        assert_eq!(parse_year(&q).unwrap(), 2026);
        q.insert("year".into(), "1999".into());
        assert!(parse_year(&q).is_err());
        q.insert("year".into(), "2100".into());
        assert!(parse_year(&q).is_err());
        q.insert("year".into(), "+2026".into());
        assert!(parse_year(&q).is_err());

        let mut headers = HeaderMap::new();
        assert_eq!(parse_if_match(&headers).unwrap_err().status, 428);
        headers.insert(header::IF_MATCH, HeaderValue::from_static("0"));
        assert_eq!(parse_if_match(&headers).unwrap_err().status, 428);
        headers.insert(header::IF_MATCH, HeaderValue::from_static("\"apps:1\""));
        assert_eq!(parse_if_match(&headers).unwrap_err().status, 428);
        headers.insert(header::IF_MATCH, HeaderValue::from_static("\"0\""));
        assert_eq!(parse_if_match(&headers).unwrap(), 0);
        headers.insert(header::IF_MATCH, HeaderValue::from_static("\"42\""));
        assert_eq!(parse_if_match(&headers).unwrap(), 42);
    }

    #[test]
    fn log_validation_prunes_unprayed_and_rejects_sunrise() {
        let raw: Map<String, Value> = serde_json::from_value(json!({
            "2024-02-29": { "fajr": "on_time", "isha": "late" },
            "2024-03-01": {
                "fajr": "unprayed",
                "dhuhr": "unprayed",
                "asr": "unprayed",
                "maghrib": "unprayed",
                "isha": "unprayed"
            },
            "2024-03-02": {}
        }))
        .unwrap();
        let pruned = validate_log_days(2024, &raw).unwrap();
        assert_eq!(pruned.len(), 1);
        assert_eq!(
            pruned["2024-02-29"],
            json!({
                "fajr": "on_time",
                "dhuhr": "unprayed",
                "asr": "unprayed",
                "maghrib": "unprayed",
                "isha": "late"
            })
        );

        let with_sunrise: Map<String, Value> = serde_json::from_value(json!({
            "2026-05-01": { "fajr": "on_time", "sunrise": "on_time" }
        }))
        .unwrap();
        assert!(validate_log_days(2026, &with_sunrise).is_err());

        let wrong_year: Map<String, Value> = serde_json::from_value(json!({
            "2025-12-31": { "fajr": "on_time" }
        }))
        .unwrap();
        assert!(validate_log_days(2026, &wrong_year).is_err());

        let bad_date: Map<String, Value> = serde_json::from_value(json!({
            "2026-02-30": { "fajr": "on_time" }
        }))
        .unwrap();
        assert!(validate_log_days(2026, &bad_date).is_err());

        let bad_status: Map<String, Value> = serde_json::from_value(json!({
            "2026-05-01": { "fajr": "missed" }
        }))
        .unwrap();
        assert!(validate_log_days(2026, &bad_status).is_err());
    }

    #[test]
    fn log_patch_updates_and_prunes() {
        let mut days = BTreeMap::new();
        let patch1 = parse_log_patch(
            &serde_json::from_value::<Map<String, Value>>(
                json!({ "prayer": "asr", "status": "on_time" }),
            )
            .unwrap(),
        )
        .unwrap();
        let norm1 = apply_log_patch(&mut days, "2026-11-27", patch1);
        assert_eq!(norm1["asr"], "on_time");
        assert_eq!(norm1["fajr"], "unprayed");
        assert!(days.contains_key("2026-11-27"));

        let patch2 = parse_log_patch(
            &serde_json::from_value::<Map<String, Value>>(
                json!({ "prayer": "asr", "status": "unprayed" }),
            )
            .unwrap(),
        )
        .unwrap();
        let norm2 = apply_log_patch(&mut days, "2026-11-27", patch2);
        assert_eq!(norm2["asr"], "unprayed");
        assert!(!days.contains_key("2026-11-27"));

        let patch_day = parse_log_patch(
            &serde_json::from_value::<Map<String, Value>>(
                json!({ "day": { "maghrib": "late" } }),
            )
            .unwrap(),
        )
        .unwrap();
        let norm3 = apply_log_patch(&mut days, "2026-11-28", patch_day);
        assert_eq!(norm3["maghrib"], "late");
        assert_eq!(days.len(), 1);

        for bad in [
            json!({ "prayer": "sunrise", "status": "on_time" }),
            json!({ "day": { "sunrise": "on_time" } }),
            json!({ "prayer": "fajr" }),
            json!({ "prayer": "fajr", "status": "on_time", "extra": 1 }),
            json!({ "day": { "fajr": "on_time" }, "prayer": "fajr" }),
        ] {
            let map: Map<String, Value> = serde_json::from_value(bad).unwrap();
            assert!(parse_log_patch(&map).is_err());
        }
    }

    #[test]
    fn settings_validation_accepts_v1_v2_and_rejects_invalid() {
        let (ver, _) = validate_settings_body(&valid_settings_v1()).unwrap();
        assert_eq!(ver, 1);

        let v2: Map<String, Value> = serde_json::from_value(json!({
            "schemaVersion": 2,
            "value": {
                "location": valid_location(),
                "locations": [
                    { "id": "home", "tag": "Home", "location": valid_location() },
                    { "id": "makkah", "tag": "Travel", "location": { "label": "Makkah", "latitude": 21.4225, "longitude": 39.8262, "timezone": "Asia/Riyadh" } }
                ],
                "calculation": {
                    "method": "Custom",
                    "asrRule": "hanafi",
                    "highLatitudeRule": "middle",
                    "offsetMinutes": valid_offsets(),
                    "customFajrAngle": 18.5,
                    "customIshaAngle": 17.0,
                    "customIshaMinutes": 90
                },
                "display": {
                    "timeFormat": "24h",
                    "showSunrise": true,
                    "showCountdown": false,
                    "hijriOffset": -1
                }
            }
        }))
        .unwrap();
        assert_eq!(validate_settings_body(&v2).unwrap().0, 2);

        for method in METHODS {
            let mut body = valid_settings_v1();
            body["value"]["calculation"]["method"] = json!(method);
            assert!(validate_settings_body(&body).is_ok(), "{method}");
        }

        let mut bad_schema = valid_settings_v1();
        bad_schema["schemaVersion"] = json!(3);
        assert!(validate_settings_body(&bad_schema).is_err());

        let mut bad_outer_key = valid_settings_v1();
        bad_outer_key.insert("extra".into(), json!(true));
        assert!(validate_settings_body(&bad_outer_key).is_err());

        let mut bad_label = valid_settings_v1();
        bad_label["value"]["location"]["label"] = json!("   ");
        assert!(validate_settings_body(&bad_label).is_err());

        let mut long_label = valid_settings_v1();
        long_label["value"]["location"]["label"] = json!("a".repeat(101));
        assert!(validate_settings_body(&long_label).is_err());

        let mut bad_lat = valid_settings_v1();
        bad_lat["value"]["location"]["latitude"] = json!(90.1);
        assert!(validate_settings_body(&bad_lat).is_err());

        let mut bad_lon = valid_settings_v1();
        bad_lon["value"]["location"]["longitude"] = json!(-180.5);
        assert!(validate_settings_body(&bad_lon).is_err());

        let mut bad_tz = valid_settings_v1();
        bad_tz["value"]["location"]["timezone"] = json!("Mars/Olympus");
        assert!(validate_settings_body(&bad_tz).is_err());

        let mut bad_method = valid_settings_v1();
        bad_method["value"]["calculation"]["method"] = json!("Unknown");
        assert!(validate_settings_body(&bad_method).is_err());

        let mut bad_offset = valid_settings_v1();
        bad_offset["value"]["calculation"]["offsetMinutes"]["fajr"] = json!(121);
        assert!(validate_settings_body(&bad_offset).is_err());

        let mut float_offset = valid_settings_v1();
        float_offset["value"]["calculation"]["offsetMinutes"]["isha"] = json!(1.5);
        assert!(validate_settings_body(&float_offset).is_err());

        let mut bad_angle = v2.clone();
        bad_angle["value"]["calculation"]["customFajrAngle"] = json!(30.5);
        assert!(validate_settings_body(&bad_angle).is_err());

        let mut bad_minutes = v2.clone();
        bad_minutes["value"]["calculation"]["customIshaMinutes"] = json!(181);
        assert!(validate_settings_body(&bad_minutes).is_err());

        let mut bad_display = v2.clone();
        bad_display["value"]["display"]["timeFormat"] = json!("12");
        assert!(validate_settings_body(&bad_display).is_err());

        let mut bad_hijri = v2.clone();
        bad_hijri["value"]["display"]["hijriOffset"] = json!(3);
        assert!(validate_settings_body(&bad_hijri).is_err());

        let mut bad_display_bool = v2.clone();
        bad_display_bool["value"]["display"]["showSunrise"] = json!("yes");
        assert!(validate_settings_body(&bad_display_bool).is_err());

        let mut too_many_locs = valid_settings_v1();
        too_many_locs["value"]["locations"] = json!(vec![
            json!({ "id": "x", "tag": "Home", "location": valid_location() });
            25
        ]);
        assert!(validate_settings_body(&too_many_locs).is_err());
    }
}
