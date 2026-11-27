//! Typed HTTP wrappers for `/api/v1/prayers/*` (`If-Match` concurrency + CSRF).

use serde_json::{Value, json};

use crate::api::{Req, RequestError, request};
use crate::prayers::model::{
    DayLog, PatchLogResponse, PrayerKey, PrayerSettings, PrayerStatus, StoredSettings, YearLog,
};

pub const SETTINGS_PATH: &str = "/api/v1/prayers/settings";
pub const LOG_PATH: &str = "/api/v1/prayers/log";

pub fn if_match_header(revision: u64) -> String {
    format!("\"{revision}\"")
}

pub fn put_settings_body(settings: &PrayerSettings) -> Value {
    json!({
        "schemaVersion": 2,
        "value": settings,
    })
}

pub fn patch_prayer_body(prayer: PrayerKey, status: PrayerStatus) -> Value {
    json!({
        "prayer": prayer.as_str(),
        "status": status.as_str(),
    })
}

pub fn patch_day_body(day: &DayLog) -> Value {
    json!({
        "day": day,
    })
}

pub async fn fetch_settings() -> Result<Option<StoredSettings>, RequestError> {
    request::<Option<StoredSettings>>(SETTINGS_PATH, Req::default()).await
}

pub async fn put_settings(
    csrf: &str,
    revision: u64,
    settings: &PrayerSettings,
) -> Result<StoredSettings, RequestError> {
    let if_match = if_match_header(revision);
    request(
        SETTINGS_PATH,
        Req {
            method: "PUT",
            body: Some(put_settings_body(settings).to_string()),
            csrf: Some(csrf),
            headers: &[("if-match", &if_match)],
            ..Default::default()
        },
    )
    .await
}

pub async fn fetch_year_log(year: u32) -> Result<YearLog, RequestError> {
    request(&format!("{LOG_PATH}?year={year}"), Req::default()).await
}

pub async fn patch_day_prayer(
    csrf: &str,
    revision: u64,
    date: &str,
    prayer: PrayerKey,
    status: PrayerStatus,
) -> Result<PatchLogResponse, RequestError> {
    let if_match = if_match_header(revision);
    request(
        &format!("{LOG_PATH}/{date}"),
        Req {
            method: "PATCH",
            body: Some(patch_prayer_body(prayer, status).to_string()),
            csrf: Some(csrf),
            headers: &[("if-match", &if_match)],
            ..Default::default()
        },
    )
    .await
}

pub async fn patch_day_all(
    csrf: &str,
    revision: u64,
    date: &str,
    day: &DayLog,
) -> Result<PatchLogResponse, RequestError> {
    let if_match = if_match_header(revision);
    request(
        &format!("{LOG_PATH}/{date}"),
        Req {
            method: "PATCH",
            body: Some(patch_day_body(day).to_string()),
            csrf: Some(csrf),
            headers: &[("if-match", &if_match)],
            ..Default::default()
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_builders_match_backend_contract() {
        assert_eq!(if_match_header(4), "\"4\"");
        let body = patch_prayer_body(PrayerKey::Asr, PrayerStatus::OnTime);
        assert_eq!(body["prayer"], "asr");
        assert_eq!(body["status"], "on_time");
    }
}
