//! Timetable exports: clipboard text, iCalendar (.ics), CSV, and JSON backup.

use std::collections::BTreeMap;

use serde_json::json;

use crate::prayers::calc::{DayTimes, civil_to_hijri};
use crate::prayers::model::{
    ALL_PRAYERS, DayLog, Location, OBLIGATORY_PRAYERS, PrayerSettings, YearLog,
};
use crate::prayers::tz::{CivilDate, format_ics_utc, format_time_in_tz};

pub fn format_day_clipboard(
    date: CivilDate,
    times: &DayTimes,
    loc: &Location,
    use_24h: bool,
    hijri_offset: i32,
) -> String {
    let hijri = civil_to_hijri(date, hijri_offset);
    let mut lines = Vec::new();
    lines.push(format!(
        "{} \u{2014} {}, {} {} {} ({})",
        loc.label,
        date.weekday_name(),
        date.short_month_name(),
        date.day,
        date.year,
        hijri.format_full()
    ));
    for p in ALL_PRAYERS {
        let val = times
            .get(p)
            .map(|ms| format_time_in_tz(ms, &loc.timezone, use_24h))
            .unwrap_or_else(|| "\u{2014}".into());
        lines.push(format!("{:<8} {}", p.label(), val));
    }
    lines.join("\n")
}

pub fn generate_ics(days: &[DayTimes], loc: &Location, title: &str) -> String {
    let mut out = String::from(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Dar//Prayer Times//EN\r\nCALSCALE:GREGORIAN\r\nMETHOD:PUBLISH\r\n",
    );
    out.push_str(&format!("X-WR-CALNAME:{title}\r\n"));
    for d in days {
        let iso = d.date.format_iso().replace('-', "");
        for p in OBLIGATORY_PRAYERS {
            let Some(start_ms) = d.get(p) else { continue };
            let end_ms = start_ms + 5.0 * 60_000.0;
            out.push_str("BEGIN:VEVENT\r\n");
            out.push_str(&format!("UID:dar-prayers-{iso}-{}@dar\r\n", p.as_str()));
            out.push_str(&format!("DTSTART:{}\r\n", format_ics_utc(start_ms)));
            out.push_str(&format!("DTEND:{}\r\n", format_ics_utc(end_ms)));
            out.push_str(&format!("SUMMARY:{} \u{2014} {}\r\n", p.label(), loc.label));
            out.push_str(&format!("LOCATION:{}\r\n", loc.label));
            out.push_str("END:VEVENT\r\n");
        }
    }
    out.push_str("END:VCALENDAR\r\n");
    out
}

pub fn generate_csv(
    days: &[DayTimes],
    loc: &Location,
    log: &BTreeMap<String, DayLog>,
    use_24h: bool,
    hijri_offset: i32,
) -> String {
    let mut out = String::from(
        "Date,Hijri,Fajr,Sunrise,Dhuhr,Asr,Maghrib,Isha,FajrStatus,DhuhrStatus,AsrStatus,MaghribStatus,IshaStatus\n",
    );
    for d in days {
        let iso = d.date.format_iso();
        let hijri = civil_to_hijri(d.date, hijri_offset).format_full();
        let day_log = log.get(&iso).copied().unwrap_or_default();
        let mut cols = vec![iso, format!("\"{hijri}\"")];
        for p in ALL_PRAYERS {
            cols.push(
                d.get(p)
                    .map(|ms| format_time_in_tz(ms, &loc.timezone, use_24h))
                    .unwrap_or_default(),
            );
        }
        for p in OBLIGATORY_PRAYERS {
            cols.push(day_log.get(p).as_str().to_owned());
        }
        out.push_str(&cols.join(","));
        out.push('\n');
    }
    out
}

pub fn generate_json_backup(
    settings: &PrayerSettings,
    logs: &BTreeMap<u32, YearLog>,
) -> String {
    serde_json::to_string_pretty(&json!({
        "app": "org.dar.prayer-times",
        "schemaVersion": 2,
        "settings": settings,
        "logs": logs,
    }))
    .unwrap_or_else(|_| "{}".into())
}
