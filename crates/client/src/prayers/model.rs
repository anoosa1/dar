//! Prayer Times domain model, schema compatibility (v1/v2), and input validation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::prayers::tz;

/// All time slots shown in Prayer Times timetables (5 obligatory prayers + Sunrise).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PrayerKey {
    Fajr,
    Sunrise,
    Dhuhr,
    Asr,
    Maghrib,
    Isha,
}

pub const ALL_PRAYERS: [PrayerKey; 6] = [
    PrayerKey::Fajr,
    PrayerKey::Sunrise,
    PrayerKey::Dhuhr,
    PrayerKey::Asr,
    PrayerKey::Maghrib,
    PrayerKey::Isha,
];

pub const OBLIGATORY_PRAYERS: [PrayerKey; 5] = [
    PrayerKey::Fajr,
    PrayerKey::Dhuhr,
    PrayerKey::Asr,
    PrayerKey::Maghrib,
    PrayerKey::Isha,
];

impl PrayerKey {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fajr => "fajr",
            Self::Sunrise => "sunrise",
            Self::Dhuhr => "dhuhr",
            Self::Asr => "asr",
            Self::Maghrib => "maghrib",
            Self::Isha => "isha",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::Fajr => 0,
            Self::Sunrise => 1,
            Self::Dhuhr => 2,
            Self::Asr => 3,
            Self::Maghrib => 4,
            Self::Isha => 5,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Fajr => "Fajr",
            Self::Sunrise => "Sunrise",
            Self::Dhuhr => "Dhuhr",
            Self::Asr => "Asr",
            Self::Maghrib => "Maghrib",
            Self::Isha => "Isha",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Fajr => "Faj",
            Self::Sunrise => "Sun",
            Self::Dhuhr => "Dhu",
            Self::Asr => "Asr",
            Self::Maghrib => "Mag",
            Self::Isha => "Ish",
        }
    }

    pub const fn icon_name(self) -> &'static str {
        match self {
            Self::Fajr => "fajr",
            Self::Sunrise => "sunrise",
            Self::Dhuhr => "sun",
            Self::Asr => "sun",
            Self::Maghrib => "sunset",
            Self::Isha => "moon",
        }
    }

    pub const fn subtitle(self) -> &'static str {
        match self {
            Self::Fajr => "Dawn twilight",
            Self::Sunrise => "Solar horizon \u{00b7} not a prayer",
            Self::Dhuhr => "Solar zenith",
            Self::Asr => "Afternoon shadow",
            Self::Maghrib => "Sunset",
            Self::Isha => "Night twilight",
        }
    }

    pub const fn is_obligatory(self) -> bool {
        !matches!(self, Self::Sunrise)
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fajr" => Some(Self::Fajr),
            "sunrise" => Some(Self::Sunrise),
            "dhuhr" => Some(Self::Dhuhr),
            "asr" => Some(Self::Asr),
            "maghrib" => Some(Self::Maghrib),
            "isha" => Some(Self::Isha),
            _ => None,
        }
    }
}

/// Daily prayer log state stored for each obligatory prayer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrayerStatus {
    #[default]
    Unprayed,
    OnTime,
    Late,
}

impl PrayerStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unprayed => "unprayed",
            Self::OnTime => "on_time",
            Self::Late => "late",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Unprayed => "Not prayed yet",
            Self::OnTime => "Prayed on time",
            Self::Late => "Prayed late",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Unprayed => "Unprayed",
            Self::OnTime => "On time",
            Self::Late => "Late",
        }
    }

    pub const fn next_cycle(self) -> Self {
        match self {
            Self::Unprayed => Self::OnTime,
            Self::OnTime => Self::Late,
            Self::Late => Self::Unprayed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayLog {
    #[serde(default)]
    pub fajr: PrayerStatus,
    #[serde(default)]
    pub dhuhr: PrayerStatus,
    #[serde(default)]
    pub asr: PrayerStatus,
    #[serde(default)]
    pub maghrib: PrayerStatus,
    #[serde(default)]
    pub isha: PrayerStatus,
}

impl DayLog {
    pub fn get(self, key: PrayerKey) -> PrayerStatus {
        match key {
            PrayerKey::Fajr => self.fajr,
            PrayerKey::Dhuhr => self.dhuhr,
            PrayerKey::Asr => self.asr,
            PrayerKey::Maghrib => self.maghrib,
            PrayerKey::Isha => self.isha,
            PrayerKey::Sunrise => PrayerStatus::Unprayed,
        }
    }

    pub fn set(&mut self, key: PrayerKey, status: PrayerStatus) {
        match key {
            PrayerKey::Fajr => self.fajr = status,
            PrayerKey::Dhuhr => self.dhuhr = status,
            PrayerKey::Asr => self.asr = status,
            PrayerKey::Maghrib => self.maghrib = status,
            PrayerKey::Isha => self.isha = status,
            PrayerKey::Sunrise => {}
        }
    }

    pub fn is_all_unprayed(self) -> bool {
        self.fajr == PrayerStatus::Unprayed
            && self.dhuhr == PrayerStatus::Unprayed
            && self.asr == PrayerStatus::Unprayed
            && self.maghrib == PrayerStatus::Unprayed
            && self.isha == PrayerStatus::Unprayed
    }

    /// Returns `(on_time, late, unprayed)` counts over the 5 obligatory prayers.
    pub fn counts(self) -> (usize, usize, usize) {
        let mut on_time = 0usize;
        let mut late = 0usize;
        let mut unprayed = 0usize;
        for prayer in OBLIGATORY_PRAYERS {
            match self.get(prayer) {
                PrayerStatus::OnTime => on_time += 1,
                PrayerStatus::Late => late += 1,
                PrayerStatus::Unprayed => unprayed += 1,
            }
        }
        (on_time, late, unprayed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YearLog {
    pub year: u32,
    pub revision: u64,
    #[serde(default)]
    pub days: BTreeMap<String, DayLog>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchLogResponse {
    pub year: u32,
    pub revision: u64,
    pub date: String,
    pub day: DayLog,
    #[serde(default)]
    pub days: BTreeMap<String, DayLog>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub label: String,
    pub latitude: f64,
    pub longitude: f64,
    pub timezone: String,
}

impl Default for Location {
    fn default() -> Self {
        Self {
            label: String::new(),
            latitude: 0.0,
            longitude: 0.0,
            timezone: tz::browser_timezone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedLocation {
    pub id: String,
    #[serde(default)]
    pub tag: String,
    pub location: Location,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OffsetMinutes {
    #[serde(default)]
    pub fajr: i32,
    #[serde(default)]
    pub sunrise: i32,
    #[serde(default)]
    pub dhuhr: i32,
    #[serde(default)]
    pub asr: i32,
    #[serde(default)]
    pub maghrib: i32,
    #[serde(default)]
    pub isha: i32,
}

impl OffsetMinutes {
    pub fn get(&self, key: PrayerKey) -> i32 {
        match key {
            PrayerKey::Fajr => self.fajr,
            PrayerKey::Sunrise => self.sunrise,
            PrayerKey::Dhuhr => self.dhuhr,
            PrayerKey::Asr => self.asr,
            PrayerKey::Maghrib => self.maghrib,
            PrayerKey::Isha => self.isha,
        }
    }

    pub fn set(&mut self, key: PrayerKey, value: i32) {
        let clamped = value.clamp(-120, 120);
        match key {
            PrayerKey::Fajr => self.fajr = clamped,
            PrayerKey::Sunrise => self.sunrise = clamped,
            PrayerKey::Dhuhr => self.dhuhr = clamped,
            PrayerKey::Asr => self.asr = clamped,
            PrayerKey::Maghrib => self.maghrib = clamped,
            PrayerKey::Isha => self.isha = clamped,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalculationSettings {
    pub method: String,
    pub asr_rule: String,
    pub high_latitude_rule: String,
    #[serde(default)]
    pub offset_minutes: OffsetMinutes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_fajr_angle: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_isha_angle: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_isha_minutes: Option<i32>,
}

impl Default for CalculationSettings {
    fn default() -> Self {
        Self {
            method: "MWL".into(),
            asr_rule: "standard".into(),
            high_latitude_rule: "middle".into(),
            offset_minutes: OffsetMinutes::default(),
            custom_fajr_angle: None,
            custom_isha_angle: None,
            custom_isha_minutes: None,
        }
    }
}

fn default_time_format() -> String {
    "12h".into()
}
fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplaySettings {
    #[serde(default = "default_time_format")]
    pub time_format: String,
    #[serde(default = "default_true")]
    pub show_sunrise: bool,
    #[serde(default = "default_true")]
    pub show_hijri: bool,
    #[serde(default)]
    pub hijri_offset: i32,
    #[serde(default = "default_true")]
    pub show_tracking: bool,
    #[serde(default = "default_true")]
    pub show_sunna: bool,
    #[serde(default = "default_true")]
    pub show_countdown: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            time_format: default_time_format(),
            show_sunrise: true,
            show_hijri: true,
            hijri_offset: 0,
            show_tracking: true,
            show_sunna: true,
            show_countdown: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PrayerSettings {
    pub location: Location,
    #[serde(default)]
    pub locations: Vec<SavedLocation>,
    pub calculation: CalculationSettings,
    #[serde(default)]
    pub display: DisplaySettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSettings {
    pub key: String,
    pub revision: u64,
    pub schema_version: u32,
    pub value: PrayerSettings,
    #[serde(default)]
    pub modified_at: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PrayersView {
    #[default]
    Today,
    Week,
    Month,
    Qibla,
}

impl PrayersView {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Today => "today",
            Self::Week => "week",
            Self::Month => "month",
            Self::Qibla => "qibla",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Week => "Week",
            Self::Month => "Month",
            Self::Qibla => "Qibla",
        }
    }

    pub const fn icon_name(self) -> &'static str {
        match self {
            Self::Today => "clock",
            Self::Week => "calendar",
            Self::Month => "table",
            Self::Qibla => "compass",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "week" => Self::Week,
            "month" => Self::Month,
            "qibla" => Self::Qibla,
            _ => Self::Today,
        }
    }
}

pub struct MethodPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub fajr_angle: f64,
    pub isha_angle: f64,
    pub isha_minutes: Option<i32>,
}

pub const METHODS: &[MethodPreset] = &[
    MethodPreset {
        id: "ISNA",
        name: "ISNA",
        summary: "15\u{00b0} / 15\u{00b0} \u{00b7} North America",
        fajr_angle: 15.0,
        isha_angle: 15.0,
        isha_minutes: None,
    },
    MethodPreset {
        id: "MWL",
        name: "Muslim World League",
        summary: "18\u{00b0} / 17\u{00b0}",
        fajr_angle: 18.0,
        isha_angle: 17.0,
        isha_minutes: None,
    },
    MethodPreset {
        id: "Egypt",
        name: "Egyptian General Authority",
        summary: "19.5\u{00b0} / 17.5\u{00b0}",
        fajr_angle: 19.5,
        isha_angle: 17.5,
        isha_minutes: None,
    },
    MethodPreset {
        id: "Karachi",
        name: "University of Islamic Sciences, Karachi",
        summary: "18\u{00b0} / 18\u{00b0}",
        fajr_angle: 18.0,
        isha_angle: 18.0,
        isha_minutes: None,
    },
    MethodPreset {
        id: "UmmAlQura",
        name: "Umm al-Qura, Makkah",
        summary: "18.5\u{00b0} / 90 min after Maghrib",
        fajr_angle: 18.5,
        isha_angle: 0.0,
        isha_minutes: Some(90),
    },
    MethodPreset {
        id: "Diyanet",
        name: "Diyanet (Turkey)",
        summary: "18\u{00b0} / 17\u{00b0}",
        fajr_angle: 18.0,
        isha_angle: 17.0,
        isha_minutes: None,
    },
    MethodPreset {
        id: "MUIS",
        name: "MUIS (Singapore)",
        summary: "20\u{00b0} / 18\u{00b0}",
        fajr_angle: 20.0,
        isha_angle: 18.0,
        isha_minutes: None,
    },
    MethodPreset {
        id: "UOIF",
        name: "UOIF (France)",
        summary: "12\u{00b0} / 12\u{00b0}",
        fajr_angle: 12.0,
        isha_angle: 12.0,
        isha_minutes: None,
    },
    MethodPreset {
        id: "Tehran",
        name: "Institute of Geophysics, Tehran",
        summary: "17.7\u{00b0} / 14\u{00b0}",
        fajr_angle: 17.7,
        isha_angle: 14.0,
        isha_minutes: None,
    },
    MethodPreset {
        id: "Custom",
        name: "Custom angles",
        summary: "Set custom Fajr & Isha parameters",
        fajr_angle: 18.0,
        isha_angle: 17.0,
        isha_minutes: None,
    },
];

pub fn method_preset(id: &str) -> &'static MethodPreset {
    METHODS
        .iter()
        .find(|m| m.id == id)
        .unwrap_or(&METHODS[1])
}

pub fn method_short_label(id: &str) -> &'static str {
    match id {
        "ISNA" => "ISNA",
        "MWL" => "MWL",
        "Egypt" => "Egypt",
        "Karachi" => "Karachi",
        "UmmAlQura" => "Umm al-Qura",
        "Diyanet" => "Diyanet",
        "MUIS" => "MUIS",
        "UOIF" => "UOIF",
        "Tehran" => "Tehran",
        "Custom" => "Custom",
        _ => "MWL",
    }
}

pub fn validate_label(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Enter a location name. Not saved.");
    }
    if trimmed.chars().count() > 100 {
        return Err("Keep under 100 characters. Not saved.");
    }
    Ok(trimmed.to_owned())
}

pub fn validate_latitude(raw: &str) -> Result<f64, &'static str> {
    let val: f64 = raw
        .trim()
        .parse()
        .map_err(|_| "Between \u{2212}90 and 90. Not saved.")?;
    if !val.is_finite() || !(-90.0..=90.0).contains(&val) {
        return Err("Between \u{2212}90 and 90. Not saved.");
    }
    Ok(val)
}

pub fn validate_longitude(raw: &str) -> Result<f64, &'static str> {
    let val: f64 = raw
        .trim()
        .parse()
        .map_err(|_| "Between \u{2212}180 and 180. Not saved.")?;
    if !val.is_finite() || !(-180.0..=180.0).contains(&val) {
        return Err("Between \u{2212}180 and 180. Not saved.")?;
    }
    Ok(val)
}

pub fn validate_timezone(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 100 || !tz::is_valid_timezone(trimmed) {
        return Err("Use an IANA timezone like America/Toronto. Not saved.");
    }
    Ok(trimmed.to_owned())
}

pub fn validate_offset(raw: &str) -> Result<i32, &'static str> {
    let val: i32 = raw
        .trim()
        .parse()
        .map_err(|_| "Between \u{2212}120 and 120. Not saved.")?;
    if !(-120..=120).contains(&val) {
        return Err("Between \u{2212}120 and 120. Not saved.");
    }
    Ok(val)
}

pub fn slug_id(label: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        "loc".into()
    } else {
        trimmed.chars().take(60).collect()
    }
}

/// Ensures active location is present in `settings.locations` without duplicates.
pub fn ensure_location_listed(settings: &mut PrayerSettings) {
    let exists = settings.locations.iter().any(|item| {
        item.location.label.eq_ignore_ascii_case(&settings.location.label)
            || ((item.location.latitude - settings.location.latitude).abs() < 0.01
                && (item.location.longitude - settings.location.longitude).abs() < 0.01)
    });
    if !exists && !settings.location.label.trim().is_empty() {
        let base_id = slug_id(&settings.location.label);
        let mut id = base_id.clone();
        let mut n = 2usize;
        while settings.locations.iter().any(|l| l.id == id) {
            id = format!("{base_id}-{n}");
            n += 1;
        }
        settings.locations.insert(
            0,
            SavedLocation {
                id,
                tag: if settings.locations.is_empty() {
                    "Home".into()
                } else {
                    String::new()
                },
                location: settings.location.clone(),
            },
        );
        settings.locations.truncate(24);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_settings_deserialize_with_defaults() {
        let raw = r#"{
            "key": "settings",
            "revision": 3,
            "schemaVersion": 1,
            "modifiedAt": 1794160800000,
            "value": {
                "location": {
                    "label": "Toronto",
                    "latitude": 43.6532,
                    "longitude": -79.3832,
                    "timezone": "America/Toronto"
                },
                "calculation": {
                    "method": "ISNA",
                    "asrRule": "standard",
                    "highLatitudeRule": "middle",
                    "offsetMinutes": {"fajr":0,"sunrise":0,"dhuhr":0,"asr":0,"maghrib":0,"isha":0}
                }
            }
        }"#;
        let stored: StoredSettings = serde_json::from_str(raw).expect("v1 parse");
        assert_eq!(stored.schema_version, 1);
        assert!(stored.value.locations.is_empty());
        assert_eq!(stored.value.display.time_format, "12h");
        assert!(stored.value.display.show_tracking);
        assert!(stored.value.display.show_sunrise);
    }

    #[test]
    fn status_cycle_and_day_counts() {
        assert_eq!(PrayerStatus::Unprayed.next_cycle(), PrayerStatus::OnTime);
        assert_eq!(PrayerStatus::OnTime.next_cycle(), PrayerStatus::Late);
        assert_eq!(PrayerStatus::Late.next_cycle(), PrayerStatus::Unprayed);
        let mut day = DayLog::default();
        assert!(day.is_all_unprayed());
        day.set(PrayerKey::Fajr, PrayerStatus::OnTime);
        day.set(PrayerKey::Dhuhr, PrayerStatus::Late);
        assert_eq!(day.counts(), (1, 1, 3));
        assert!(!day.is_all_unprayed());
    }

    #[test]
    fn validation_messages_match_contract() {
        assert_eq!(
            validate_label("  ").unwrap_err(),
            "Enter a location name. Not saved."
        );
        assert_eq!(
            validate_longitude("-279.3832").unwrap_err(),
            "Between \u{2212}180 and 180. Not saved."
        );
        assert_eq!(
            validate_latitude("95").unwrap_err(),
            "Between \u{2212}90 and 90. Not saved."
        );
        assert!(validate_timezone("America/Toronto").is_ok());
        assert!(validate_timezone("BadZone").is_err());
    }
}
