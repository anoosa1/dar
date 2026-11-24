//! iCalendar events (`runtime/pim/calendar.ts`): validation and the projection the apps
//! show, edits (including single occurrences of a series), cancelling an occurrence, imports
//! and expanding a series into a viewer's time zone. Recurrences are wall-clock series
//! (RFC 5545) computed with the `rrule` crate; zones come from `chrono-tz`.
//! Gaps: a custom embedded VTIMEZONE whose TZID is not an IANA name is read as UTC; tasks
//! (VTODO) are read and written in `tasks.rs`.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};

use super::vobject::{
    Component, Property, escape, field, invalid, parse, prop, remove, serialize, set, set_text,
    text,
};
use crate::error::{AppError, Result};

/// A DATE or DATE-TIME value as written: wall time, date-only, UTC (`Z`) or a TZID.
#[derive(Clone, Debug, PartialEq)]
pub struct Time {
    pub local: NaiveDateTime,
    pub date: bool,
    pub utc: bool,
    pub tzid: Option<String>,
}

impl Time {
    pub(crate) fn parse(p: &Property) -> Option<Time> {
        let v = p.value.trim();
        if v.len() == 8 {
            let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
            return Some(Time {
                local: d.and_hms_opt(0, 0, 0)?,
                date: true,
                utc: false,
                tzid: None,
            });
        }
        let (body, utc) = match v.strip_suffix('Z') {
            Some(b) => (b, true),
            None => (v, false),
        };
        let local = NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S").ok()?;
        let tzid = if utc {
            None
        } else {
            p.param("TZID")
                .map(|z| z.trim_start_matches('/').to_owned())
        };
        Some(Time {
            local,
            date: false,
            utc,
            tzid,
        })
    }

    /// ical.js `toString()` without the `Z` (`dateString`).
    pub fn wall_string(&self) -> String {
        if self.date {
            self.local.format("%Y-%m-%d").to_string()
        } else {
            self.local.format("%Y-%m-%dT%H:%M:%S").to_string()
        }
    }

    /// ical.js `toString()`: the recurrence key.
    pub(crate) fn key(&self) -> String {
        format!("{}{}", self.wall_string(), if self.utc { "Z" } else { "" })
    }

    /// The zone name the apps show: "floating" for dates and floating times.
    pub(crate) fn zone_name(&self) -> String {
        if self.date {
            "floating".into()
        } else if self.utc {
            "UTC".into()
        } else {
            self.tzid.clone().unwrap_or_else(|| "floating".into())
        }
    }

    pub(crate) fn instant(&self, viewer: Tz) -> i64 {
        if self.utc {
            return self.local.and_utc().timestamp_millis();
        }
        let zone = if self.date {
            None
        } else {
            self.tzid.as_deref()
        };
        match zone {
            // A TZID that is not an IANA name (a custom VTIMEZONE) is read as UTC.
            Some(name) => match name.parse::<Tz>() {
                Ok(tz) => wall_epoch(self.local, tz),
                Err(_) => self.local.and_utc().timestamp_millis(),
            },
            None => wall_epoch(self.local, viewer),
        }
    }

    pub(crate) fn with_local(&self, local: NaiveDateTime) -> Time {
        Time {
            local,
            ..self.clone()
        }
    }
}

pub fn zone(value: &str) -> Result<Tz> {
    if value == "UTC" {
        return Ok(Tz::UTC);
    }
    value
        .parse::<Tz>()
        .map_err(|_| invalid("Choose a valid IANA timezone."))
}

/// The instant of a wall time; an ambiguous time takes the earlier, a nonexistent one the
/// offset before the change (RFC 5545).
pub fn wall_epoch(local: NaiveDateTime, tz: Tz) -> i64 {
    match tz.from_local_datetime(&local) {
        chrono::LocalResult::Single(t) => t.timestamp_millis(),
        chrono::LocalResult::Ambiguous(a, b) => a.timestamp_millis().min(b.timestamp_millis()),
        chrono::LocalResult::None => {
            let before = tz
                .offset_from_utc_datetime(&(local - Duration::hours(48)))
                .fix()
                .local_minus_utc() as i64;
            local.and_utc().timestamp_millis() - before * 1000
        }
    }
}

pub(crate) fn exists(local: NaiveDateTime, tz: Tz) -> bool {
    !matches!(tz.from_local_datetime(&local), chrono::LocalResult::None)
}

pub fn wall(epoch: i64, tz: Tz) -> NaiveDateTime {
    Utc.timestamp_millis_opt(epoch)
        .single()
        .unwrap_or_default()
        .with_timezone(&tz)
        .naive_local()
}

pub fn iso(epoch: i64) -> String {
    Utc.timestamp_millis_opt(epoch)
        .single()
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

pub(crate) fn time(c: &Component, name: &str) -> Option<Time> {
    c.prop(name).and_then(Time::parse)
}

pub(crate) fn duration_ms(value: &str) -> Option<i64> {
    let (sign, rest) = match value.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, value.strip_prefix('+').unwrap_or(value)),
    };
    let rest = rest.strip_prefix('P')?;
    let (mut total, mut number, mut in_time) = (0i64, String::new(), false);
    for ch in rest.chars() {
        match ch {
            'T' => in_time = true,
            '0'..='9' => number.push(ch),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += n * match (unit, in_time) {
                    ('W', false) => 604_800,
                    ('D', false) => 86_400,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    number.is_empty().then_some(sign * total * 1000)
}

/// DTEND, else DTSTART plus DURATION, else a day for dates and nothing for times.
fn end_of(c: &Component, start: &Time) -> Time {
    if let Some(end) = time(c, "DTEND") {
        return end;
    }
    if let Some(ms) = c.value("DURATION").and_then(duration_ms) {
        return start.with_local(start.local + Duration::milliseconds(ms));
    }
    if start.date {
        start.with_local(start.local + Duration::days(1))
    } else {
        start.clone()
    }
}

fn status(c: &Component) -> String {
    let s = text(c, "STATUS");
    if s.is_empty() { "CONFIRMED".into() } else { s }
}

fn reminder(c: &Component) -> Value {
    let Some(alarm) = c
        .components("VALARM")
        .find(|a| text(a, "ACTION") == "DISPLAY")
    else {
        return Value::Null;
    };
    let Some(trigger) = alarm.prop("TRIGGER") else {
        return Value::Null;
    };
    if trigger.param("RELATED") == Some("END") || trigger.param("VALUE") == Some("DATE-TIME") {
        return Value::Null;
    }
    duration_ms(trigger.value.trim()).map_or(Value::Null, |ms| json!(-ms / 60_000))
}

/// What the apps show for one event component (`projection`).
fn projection(c: &Component) -> Value {
    let start = time(c, "DTSTART").expect("validated start");
    let end = end_of(c, &start);
    json!({
        "title": text(c, "SUMMARY"), "start": start.wall_string(), "end": end.wall_string(), "timezone": start.zone_name(), "allDay": start.date,
        "description": text(c, "DESCRIPTION"), "location": text(c, "LOCATION"), "recurrence": c.value("RRULE").unwrap_or_default(),
        "recurring": c.prop("RRULE").is_some() || c.prop("RDATE").is_some(),
        "attendees": c.props_named("ATTENDEE").map(|p| { let v = p.value.trim(); if v.to_lowercase().starts_with("mailto:") { v[7..].to_owned() } else { v.to_owned() } }).collect::<Vec<_>>(),
        "reminderMinutes": reminder(c), "status": status(c),
    })
}

pub struct Event {
    pub uid: String,
    pub content: String,
    pub data: Value,
    pub root: Component,
}

fn master_index(root: &Component) -> Option<usize> {
    root.children
        .iter()
        .position(|c| c.name == "VEVENT" && c.prop("RECURRENCE-ID").is_none())
}

pub fn read_event(content: &str) -> Result<Event> {
    let root = parse(content, "VCALENDAR")?;
    let events: Vec<&Component> = root.components("VEVENT").collect();
    if text(&root, "VERSION") != "2.0"
        || events.is_empty()
        || root
            .children
            .iter()
            .any(|c| c.name != "VEVENT" && c.name != "VTIMEZONE")
    {
        return Err(invalid("Expected a vCalendar 2.0 event resource."));
    }
    let masters: Vec<&&Component> = events
        .iter()
        .filter(|c| c.prop("RECURRENCE-ID").is_none())
        .collect();
    if masters.len() != 1 {
        return Err(invalid("An event resource needs exactly one master event."));
    }
    let master = masters[0];
    let uid = text(master, "UID");
    if uid.is_empty() || uid.encode_utf16().count() > 1024 {
        return Err(invalid("An event needs a stable UID."));
    }
    let mut keys = Vec::new();
    for c in &events {
        let Some(start) = time(c, "DTSTART").filter(|_| text(c, "UID") == uid) else {
            return Err(invalid(
                "Event components must share a UID and have a start.",
            ));
        };
        let end = end_of(c, &start);
        if start.date != end.date
            || end.local < start.local && start.utc == end.utc && start.tzid == end.tzid
        {
            return Err(invalid("Invalid event interval."));
        }
        let name = start.zone_name();
        if name != "floating"
            && name != "UTC"
            && !root
                .components("VTIMEZONE")
                .any(|t| text(t, "TZID") == name)
        {
            zone(&name)?;
        }
        if let Some(id) = time(c, "RECURRENCE-ID") {
            let key = id.key();
            if keys.contains(&key) {
                return Err(invalid("Duplicate recurrence exception."));
            }
            keys.push(key);
        }
        if !["", "CONFIRMED", "TENTATIVE", "CANCELLED"].contains(&text(c, "STATUS").as_str()) {
            return Err(invalid("Invalid event status."));
        }
        if c.components("VALARM")
            .any(|a| !["DISPLAY", "AUDIO", "EMAIL"].contains(&text(a, "ACTION").as_str()))
        {
            return Err(invalid("Invalid alarm action."));
        }
    }
    let data = projection(master);
    Ok(Event {
        uid,
        content: serialize(&root),
        data,
        root,
    })
}

/// A to-do (VTODO), validated and projected as Tasks shows it (`tasks.rs`).
pub fn read_task(content: &str) -> Result<Event> {
    super::tasks::read_task(content)
}

/// An event or a task, as a calendar resource holds either.
pub fn read_any(content: &str) -> Result<Event> {
    if dar_dav::ical::is_task(content) {
        read_task(content)
    } else {
        read_event(content)
    }
}

/// A form's date or date and time, checked in its zone (`localTime`).
fn local_time(value: &str, all_day: bool, zone_name: &str) -> Result<Time> {
    let bad = || invalid("Invalid calendar date.");
    if all_day {
        let d = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| bad())?;
        return Ok(Time {
            local: d.and_hms_opt(0, 0, 0).ok_or_else(bad)?,
            date: true,
            utc: false,
            tzid: None,
        });
    }
    let full = if value.len() == 16 {
        format!("{value}:00")
    } else {
        value.to_owned()
    };
    let local = NaiveDateTime::parse_from_str(&full, "%Y-%m-%dT%H:%M:%S")
        .map_err(|_| invalid("Invalid date and time."))?;
    if zone_name != "floating" && !exists(local, zone(zone_name)?) {
        return Err(invalid(
            "That local time does not exist because the clocks change.",
        ));
    }
    Ok(Time {
        local,
        date: false,
        utc: zone_name == "UTC",
        tzid: (zone_name != "UTC" && zone_name != "floating").then(|| zone_name.to_owned()),
    })
}

pub(crate) fn time_property(name: &str, t: &Time) -> Property {
    if t.date {
        return prop(
            name,
            &[("VALUE", "DATE")],
            &t.local.format("%Y%m%d").to_string(),
        );
    }
    let value = t.local.format("%Y%m%dT%H%M%S").to_string();
    match (&t.tzid, t.utc) {
        (_, true) => prop(name, &[], &format!("{value}Z")),
        (Some(zone), false) => prop(name, &[("TZID", zone)], &value),
        (None, false) => prop(name, &[], &value),
    }
}

fn offset_text(seconds: i32) -> String {
    let n = seconds.abs();
    let mut out = format!(
        "{}{:02}{:02}",
        if seconds < 0 { '-' } else { '+' },
        n / 3600,
        n % 3600 / 60
    );
    if n % 60 != 0 {
        out.push_str(&format!("{:02}", n % 60));
    }
    out
}

/// A VTIMEZONE for `zone` from the IANA data: the year's transitions as yearly rules
/// (`timezoneComponent`, simplified to the event's year).
pub fn timezone_component(name: &str, year: i32) -> Result<Component> {
    let tz = zone(name)?;
    let offset = |ms: i64| {
        tz.offset_from_utc_datetime(
            &Utc.timestamp_millis_opt(ms)
                .single()
                .unwrap_or_default()
                .naive_utc(),
        )
        .fix()
        .local_minus_utc()
    };
    let start = NaiveDate::from_ymd_opt(year, 1, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .ok_or_else(|| invalid("Invalid year."))?
        .and_utc()
        .timestamp_millis();
    let end = start + 366 * 86_400_000;
    let mut transitions = Vec::new();
    let mut at = start;
    let mut current = offset(at);
    while at < end {
        let next = at + 3_600_000;
        let o = offset(next);
        if o != current {
            // The minute of the change.
            let (mut lo, mut hi) = (at, next);
            while hi - lo > 60_000 {
                let mid = lo + (hi - lo) / 2 / 60_000 * 60_000;
                let mid = if mid == lo { lo + 60_000 } else { mid };
                if offset(mid) == current {
                    lo = mid
                } else {
                    hi = mid
                }
            }
            transitions.push((hi, current, o));
            current = o;
        }
        at = next;
    }
    let mut c = Component {
        name: "VTIMEZONE".into(),
        ..Default::default()
    };
    c.props.push(prop("TZID", &[], name));
    c.props.push(prop("X-DAR-GENERATED", &[], "1"));
    const DAYS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];
    if transitions.is_empty() {
        let o = offset(start);
        let mut standard = Component {
            name: "STANDARD".into(),
            ..Default::default()
        };
        standard
            .props
            .push(prop("DTSTART", &[], &format!("{year}0101T000000")));
        standard
            .props
            .push(prop("TZOFFSETFROM", &[], &offset_text(o)));
        standard
            .props
            .push(prop("TZOFFSETTO", &[], &offset_text(o)));
        c.children.push(standard);
    }
    for (at, from, to) in transitions {
        let local = (Utc.timestamp_millis_opt(at).single().unwrap_or_default()
            + Duration::seconds(from as i64))
        .naive_utc();
        let last_day = NaiveDate::from_ymd_opt(
            local.year() + i32::from(local.month() == 12),
            local.month() % 12 + 1,
            1,
        )
        .and_then(|d| d.pred_opt())
        .map_or(31, |d| d.day());
        let week = if local.day() + 7 > last_day {
            -1
        } else {
            local.day().div_ceil(7) as i32
        };
        let mut part = Component {
            name: if to > from {
                "DAYLIGHT".into()
            } else {
                "STANDARD".into()
            },
            ..Default::default()
        };
        part.props.push(prop(
            "DTSTART",
            &[],
            &local.format("%Y%m%dT%H%M%S").to_string(),
        ));
        part.props
            .push(prop("TZOFFSETFROM", &[], &offset_text(from)));
        part.props.push(prop("TZOFFSETTO", &[], &offset_text(to)));
        part.props.push(prop(
            "RRULE",
            &[],
            &format!(
                "FREQ=YEARLY;BYMONTH={};BYDAY={week}{}",
                local.month(),
                DAYS[local.weekday().num_days_from_monday() as usize]
            ),
        ));
        c.children.push(part);
    }
    Ok(c)
}

const RULE_KEYS: [&str; 14] = [
    "FREQ",
    "UNTIL",
    "COUNT",
    "INTERVAL",
    "BYSECOND",
    "BYMINUTE",
    "BYHOUR",
    "BYDAY",
    "BYMONTHDAY",
    "BYYEARDAY",
    "BYWEEKNO",
    "BYMONTH",
    "BYSETPOS",
    "WKST",
];

pub(crate) fn checked_rule(value: &str, start: &Time) -> Result<String> {
    let bad = || invalid("Invalid recurrence rule.");
    let mut parts = Vec::new();
    for part in value
        .trim()
        .trim_start_matches("RRULE:")
        .split(';')
        .filter(|p| !p.is_empty())
    {
        let (k, v) = part.split_once('=').ok_or_else(bad)?;
        let k = k.to_uppercase();
        if !RULE_KEYS.contains(&k.as_str()) || v.is_empty() {
            return Err(bad());
        }
        parts.push(format!("{k}={}", v.to_uppercase()));
    }
    let rule = parts.join(";");
    if !rule.starts_with("FREQ=") && !parts.iter().any(|p| p.starts_with("FREQ=")) {
        return Err(bad());
    }
    rule_set(&rule, start.local, &[], &[]).ok_or_else(bad)?;
    Ok(rule)
}

/// A wall-clock series: DTSTART, RRULE, RDATE and EXDATE as naive times (UTC stands in).
fn rule_set(
    rule: &str,
    start: NaiveDateTime,
    rdates: &[NaiveDateTime],
    exdates: &[NaiveDateTime],
) -> Option<rrule::RRuleSet> {
    let z = |t: NaiveDateTime| t.format("%Y%m%dT%H%M%SZ").to_string();
    let rule: Vec<String> = rule
        .split(';')
        .map(|part| match part.split_once('=') {
            Some(("UNTIL", until)) if until.len() == 8 => format!("UNTIL={until}T235959Z"),
            Some(("UNTIL", until)) if !until.ends_with('Z') => format!("UNTIL={until}Z"),
            _ => part.to_owned(),
        })
        .collect();
    let mut text = format!("DTSTART:{}", z(start));
    if !rule.join(";").is_empty() {
        text.push_str(&format!("\nRRULE:{}", rule.join(";")));
    }
    let mut set: rrule::RRuleSet = text.parse().ok()?;
    let as_rrule = |t: NaiveDateTime| t.and_utc().with_timezone(&rrule::Tz::UTC);
    for t in rdates {
        set = set.rdate(as_rrule(*t));
    }
    for t in exdates {
        set = set.exdate(as_rrule(*t));
    }
    Some(set)
}

pub struct Write<'a> {
    pub input: &'a Value,
    pub previous: Option<&'a str>,
    pub occurrence: Option<&'a str>,
}

/// A form edit of an event, a new event, or an edit of one occurrence (`writeEvent`).
pub fn write_event(w: Write<'_>) -> Result<Event> {
    let p = w
        .input
        .as_object()
        .ok_or_else(|| invalid("Invalid event."))?;
    let get = |name: &str| p.get(name).cloned().unwrap_or(Value::Null);
    let zone_name = field(&p.get("timezone").cloned().unwrap_or(json!("UTC")), 100)?;
    if zone_name != "floating" {
        zone(&zone_name)?;
    }
    let all_day = p.get("allDay") == Some(&json!(true));
    let start = local_time(&field(&get("start"), 4096)?, all_day, &zone_name)?;
    let end = local_time(&field(&get("end"), 4096)?, all_day, &zone_name)?;
    let compare = if zone_name == "floating" || zone_name == "UTC" {
        Tz::UTC
    } else {
        zone(&zone_name)?
    };
    if wall_epoch(end.local, compare) <= wall_epoch(start.local, compare) {
        return Err(invalid("End must be after start."));
    }
    let previous = w.previous.map(read_event).transpose()?;
    let mut root = match &previous {
        Some(e) => e.root.clone(),
        None => parse(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Dar//Calendar//EN\r\nEND:VCALENDAR\r\n",
            "VCALENDAR",
        )?,
    };
    let master = match master_index(&root) {
        Some(i) => i,
        None => {
            let mut c = Component {
                name: "VEVENT".into(),
                ..Default::default()
            };
            set_text(&mut c, "UID", &crate::auth::uuid());
            root.children.push(c);
            root.children.len() - 1
        }
    };
    let mut index = master;
    if let Some(occurrence) = w.occurrence {
        if previous.is_none() {
            return Err(invalid("An occurrence needs an existing series."));
        }
        let key_time = parse_key(occurrence).ok_or_else(|| invalid("Invalid occurrence."))?;
        let master_start = time(&root.children[master], "DTSTART").expect("validated start");
        let key_time = Time {
            tzid: if key_time.utc || key_time.date {
                None
            } else {
                master_start.tzid.clone()
            },
            ..key_time
        };
        let found = root.children.iter().position(|c| {
            c.name == "VEVENT"
                && time(c, "RECURRENCE-ID").is_some_and(|t| t.key() == key_time.key())
        });
        index = match found {
            Some(i) => i,
            None => {
                let mut copy = root.children[master].clone();
                for name in ["RRULE", "RDATE", "EXDATE"] {
                    remove(&mut copy, name);
                }
                set(&mut copy, time_property("RECURRENCE-ID", &key_time));
                root.children.push(copy);
                root.children.len() - 1
            }
        };
    }
    let title = field(&get("title"), 1024)?.trim().to_owned();
    if title.is_empty() {
        return Err(invalid("An event needs a title."));
    }
    let description = field(&p.get("description").cloned().unwrap_or(json!("")), 16_384)?;
    let location = field(&p.get("location").cloned().unwrap_or(json!("")), 2048)?;
    let state = field(&p.get("status").cloned().unwrap_or(json!("CONFIRMED")), 20)?;
    let recurrence = field(&p.get("recurrence").cloned().unwrap_or(json!("")), 2048)?;
    let rule = if w.occurrence.is_none() && !recurrence.is_empty() {
        Some(checked_rule(&recurrence, &start)?)
    } else {
        None
    };
    let attendees = p.get("attendees").cloned().unwrap_or(json!([]));
    let attendees = attendees
        .as_array()
        .filter(|a| a.len() <= 100)
        .ok_or_else(|| invalid("Too many attendees."))?
        .clone();
    let minutes = get("reminderMinutes");
    {
        let c = &mut root.children[index];
        for (name, value) in [
            ("SUMMARY", &title),
            ("DESCRIPTION", &description),
            ("LOCATION", &location),
            ("STATUS", &state),
        ] {
            set_text(c, name, value);
        }
        set(c, time_property("DTSTART", &start));
        remove(c, "DURATION");
        set(c, time_property("DTEND", &end));
        if w.occurrence.is_none() {
            remove(c, "RRULE");
            if let Some(rule) = &rule {
                c.props.push(prop("RRULE", &[], rule));
            }
        }
        let old: Vec<Property> = c.props_named("ATTENDEE").cloned().collect();
        remove(c, "ATTENDEE");
        for attendee in &attendees {
            let email = field(attendee, 320)?;
            let parts: Vec<&str> = email.split('@').collect();
            if parts.len() != 2
                || parts.iter().any(|p| p.is_empty())
                || email.chars().any(char::is_whitespace)
            {
                return Err(invalid("Invalid attendee email."));
            }
            let uri = format!("mailto:{email}");
            let kept = old
                .iter()
                .find(|a| a.value.to_lowercase() == uri.to_lowercase())
                .cloned();
            c.props
                .push(kept.unwrap_or_else(|| prop("ATTENDEE", &[], &uri)));
        }
        // Imported alarms stay unless the visible reminder changed.
        let changed = previous.is_none() || minutes != reminder(c);
        if changed {
            c.children
                .retain(|a| !(a.name == "VALARM" && text(a, "ACTION") == "DISPLAY"));
            if !minutes.is_null() && minutes != json!("") {
                let m = minutes
                    .as_i64()
                    .filter(|m| {
                        (0..=10_080).contains(m) && minutes.is_i64()
                            || minutes.as_f64() == Some(*m as f64)
                    })
                    .ok_or_else(|| invalid("Reminder must be between 0 and 10,080 minutes."))?;
                let mut alarm = Component {
                    name: "VALARM".into(),
                    ..Default::default()
                };
                alarm.props.push(prop("ACTION", &[], "DISPLAY"));
                alarm.props.push(prop("TRIGGER", &[], &format!("-PT{m}M")));
                alarm.props.push(prop("DESCRIPTION", &[], &escape(&title)));
                alarm.props.push(prop("X-DAR-MANAGED", &[], "1"));
                c.children.push(alarm);
            }
        }
        set(
            c,
            prop(
                "DTSTAMP",
                &[],
                &Utc::now().format("%Y%m%dT%H%M%SZ").to_string(),
            ),
        );
        let sequence = text(c, "SEQUENCE").parse::<i64>().unwrap_or(0) + 1;
        set(c, prop("SEQUENCE", &[], &sequence.to_string()));
    }
    if !all_day && zone_name != "UTC" && zone_name != "floating" {
        let definition = root
            .children
            .iter()
            .position(|c| c.name == "VTIMEZONE" && text(c, "TZID") == zone_name);
        let stale = definition.is_some_and(|i| {
            let d = &root.children[i];
            let first = d
                .components("STANDARD")
                .next()
                .and_then(|s| time(s, "DTSTART"))
                .map(|t| t.local.year());
            text(d, "X-DAR-GENERATED") == "1" && first.is_some_and(|y| y > start.local.year())
        });
        if definition.is_none() || stale {
            if let Some(i) = definition {
                root.children.remove(i);
            }
            root.children
                .insert(0, timezone_component(&zone_name, start.local.year())?);
        }
    }
    read_event(&serialize(&root))
}

/// A recurrence key ("2026-11-18", "2026-11-18T10:00:00" or with "Z") as a time.
pub(crate) fn parse_key(value: &str) -> Option<Time> {
    if let Ok(d) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Some(Time {
            local: d.and_hms_opt(0, 0, 0)?,
            date: true,
            utc: false,
            tzid: None,
        });
    }
    let (body, utc) = match value.strip_suffix('Z') {
        Some(b) => (b, true),
        None => (value, false),
    };
    let local = NaiveDateTime::parse_from_str(body, "%Y-%m-%dT%H:%M:%S").ok()?;
    Some(Time {
        local,
        date: false,
        utc,
        tzid: None,
    })
}

/// Cancels one occurrence of a series (`cancelOccurrence`).
pub fn cancel_occurrence(content: &str, recurrence_id: &str) -> Result<Event> {
    let around = parse_key(recurrence_id)
        .ok_or_else(|| invalid("Occurrence does not exist."))?
        .local
        .and_utc()
        .timestamp_millis();
    let resource =
        json!({ "id": "", "collectionId": "", "href": "", "revision": 0, "content": content });
    let found = expand(
        &resource,
        "UTC",
        around - 2 * 86_400_000,
        around + 2 * 86_400_000,
        false,
        true,
    )?
    .into_iter()
    .find(|o| o["recurrenceId"] == recurrence_id);
    let mut occurrence = found.ok_or_else(|| invalid("Occurrence does not exist."))?;
    occurrence["status"] = json!("CANCELLED");
    write_event(Write {
        input: &occurrence,
        previous: Some(content),
        occurrence: Some(recurrence_id),
    })
}

fn values(c: &Component, name: &str) -> Vec<(Time, Option<Time>)> {
    let mut out = Vec::new();
    for p in c.props_named(name) {
        for value in p.value.split(',') {
            let mut parts = value.splitn(2, '/');
            let start = parts.next().unwrap_or_default();
            let item = Property {
                value: start.to_owned(),
                ..p.clone()
            };
            let Some(t) = Time::parse(&item) else {
                continue;
            };
            // A period's end is a time or a duration.
            let end = parts.next().and_then(|e| match duration_ms(e) {
                Some(ms) => Some(t.with_local(t.local + Duration::milliseconds(ms))),
                None => Time::parse(&Property {
                    value: e.to_owned(),
                    ..p.clone()
                }),
            });
            out.push((t, end));
        }
    }
    out
}

/// A time read in the series' own frame: UTC values move to the series' zone.
fn in_frame(t: &Time, master: &Time, viewer: Tz) -> NaiveDateTime {
    if t.utc && !master.utc {
        let tz = master
            .tzid
            .as_deref()
            .and_then(|z| z.parse::<Tz>().ok())
            .unwrap_or(viewer);
        return wall(t.local.and_utc().timestamp_millis(), tz);
    }
    t.local
}

fn overlaps(start: i64, end: i64, from: i64, to: i64) -> bool {
    start < to && (end > from || end == start && start >= from)
}

/// The occurrences of an event resource in `[from, to)` for a viewer's zone (`expand`).
pub fn expand(
    resource: &Value,
    viewer_zone: &str,
    from: i64,
    to: i64,
    first_only: bool,
    include_cancelled: bool,
) -> Result<Vec<Value>> {
    let viewer = zone(viewer_zone)?;
    if to <= from || !first_only && to - from > 366 * 86_400_000 {
        return Err(invalid(
            "Calendar windows must be between one instant and one year.",
        ));
    }
    let event = read_event(resource["content"].as_str().unwrap_or_default())?;
    let root = &event.root;
    let master = &root.children[master_index(root).expect("validated master")];
    let start = time(master, "DTSTART").expect("validated start");
    let end = end_of(master, &start);
    let length = end.local - start.local;
    let recurring = master.prop("RRULE").is_some() || master.prop("RDATE").is_some();
    let overrides: Vec<&Component> = root
        .components("VEVENT")
        .filter(|c| c.prop("RECURRENCE-ID").is_some())
        .collect();
    let override_for = |key: &str| {
        overrides
            .iter()
            .find(|c| time(c, "RECURRENCE-ID").is_some_and(|t| t.key() == key))
            .copied()
    };
    let base = |occurrence: &Value| -> Map<String, Value> {
        let mut map = occurrence.as_object().cloned().unwrap_or_default();
        for (k, v) in [
            ("resourceId", "id"),
            ("collectionId", "collectionId"),
            ("href", "href"),
            ("revision", "revision"),
        ] {
            map.insert(k.into(), resource[v].clone());
        }
        map
    };
    let mut result: Vec<Value> = Vec::new();
    let push = |component: &Component,
                s: &Time,
                e: &Time,
                key: String,
                result: &mut Vec<Value>|
     -> Result<bool> {
        let (start_at, end_at) = (s.instant(viewer), e.instant(viewer));
        if (include_cancelled || status(component) != "CANCELLED")
            && overlaps(start_at, end_at, from, to)
        {
            let mut map = base(&projection(component));
            map.insert("recurring".into(), json!(recurring));
            map.insert("start".into(), json!(s.wall_string()));
            map.insert("end".into(), json!(e.wall_string()));
            map.insert("recurrenceId".into(), json!(key));
            map.insert("startAt".into(), json!(iso(start_at)));
            map.insert("endAt".into(), json!(iso(end_at)));
            result.push(Value::Object(map));
            if result.len() > 2000 {
                return Err(AppError::with(
                    "operationTooLarge",
                    413,
                    "Too many occurrences in this window.",
                ));
            }
            return Ok(true);
        }
        Ok(false)
    };
    // Occurrence starts in the series' wall-clock frame.
    let starts: Vec<(NaiveDateTime, Option<NaiveDateTime>)> = if !recurring {
        vec![(start.local, None)]
    } else {
        let rdates = values(master, "RDATE");
        let exdates: Vec<NaiveDateTime> = values(master, "EXDATE")
            .iter()
            .map(|(t, _)| in_frame(t, &start, viewer))
            .collect();
        let periods: Vec<(NaiveDateTime, Option<NaiveDateTime>)> = rdates
            .iter()
            .map(|(t, e)| {
                (
                    in_frame(t, &start, viewer),
                    e.as_ref().map(|e| in_frame(e, &start, viewer)),
                )
            })
            .collect();
        let rule = master.value("RRULE").unwrap_or_default().to_owned();
        let mut rule = rule;
        // An UNTIL in UTC bounds the wall-clock series at its local time.
        if let Some(until) = rule
            .split(';')
            .find_map(|p| p.strip_prefix("UNTIL="))
            .filter(|u| u.ends_with('Z') && !start.utc)
            .map(str::to_owned)
            && let Ok(t) = NaiveDateTime::parse_from_str(&until, "%Y%m%dT%H%M%SZ")
        {
            let local = in_frame(
                &Time {
                    local: t,
                    date: false,
                    utc: true,
                    tzid: None,
                },
                &start,
                viewer,
            );
            rule = rule.replace(
                &format!("UNTIL={until}"),
                &format!("UNTIL={}", local.format("%Y%m%dT%H%M%S")),
            );
        }
        let set = rule_set(
            &rule,
            start.local,
            &periods.iter().map(|p| p.0).collect::<Vec<_>>(),
            &exdates,
        )
        .ok_or_else(|| invalid("Invalid recurrence rule."))?;
        let frame = |ms: i64| match (&start.tzid, start.utc, start.date) {
            (Some(z), false, false) => wall(ms, z.parse::<Tz>().unwrap_or(viewer)),
            (_, true, _) => wall(ms, Tz::UTC),
            _ => wall(ms, viewer),
        };
        let margin = length.num_milliseconds().max(0) + 2 * 86_400_000;
        let (lower, upper) = (frame(from - margin), frame(to + 2 * 86_400_000));
        // The first pending occurrence (tasks) walks the series lazily, as ical.js does.
        let dates: Vec<NaiveDateTime> = if first_only {
            (&set)
                .into_iter()
                .map(|d| d.naive_utc())
                .skip_while(|d| *d < lower)
                .take_while(|d| *d <= upper)
                .take(10_000)
                .collect()
        } else {
            let found = set
                .after(lower.and_utc().with_timezone(&rrule::Tz::UTC))
                .before(upper.and_utc().with_timezone(&rrule::Tz::UTC))
                .all(10_000);
            if found.limited {
                return Err(AppError::with(
                    "operationTooLarge",
                    413,
                    "This series exceeds the recurrence expansion limit.",
                ));
            }
            found.dates.iter().map(|d| d.naive_utc()).collect()
        };
        let mut list: Vec<(NaiveDateTime, Option<NaiveDateTime>)> =
            dates.into_iter().map(|d| (d, None)).collect();
        if !exdates.contains(&start.local)
            && !list.iter().any(|(t, _)| *t == start.local)
            && start.local >= lower
            && start.local <= upper
        {
            list.push((start.local, None));
        }
        for (t, e) in &periods {
            if let Some(item) = list.iter_mut().find(|(s, _)| s == t) {
                item.1 = *e;
            }
        }
        list.sort();
        list
    };
    let mut seen = Vec::new();
    for (local, period_end) in starts {
        let s = start.with_local(local);
        let key = s.key();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key.clone());
        let pushed = match override_for(&key) {
            Some(c) => {
                let os = time(c, "DTSTART").expect("validated start");
                let oe = end_of(c, &os);
                push(c, &os, &oe, key, &mut result)?
            }
            None => {
                let e = start.with_local(period_end.unwrap_or(local + length));
                push(master, &s, &e, key, &mut result)?
            }
        };
        if pushed && first_only {
            break;
        }
    }
    // Overrides moved far from their original slot are checked on their own.
    for c in &overrides {
        if first_only && !result.is_empty() {
            break;
        }
        let key = time(c, "RECURRENCE-ID").expect("override").key();
        if seen.contains(&key) {
            continue;
        }
        let os = time(c, "DTSTART").expect("validated start");
        let oe = end_of(c, &os);
        push(c, &os, &oe, key, &mut result)?;
    }
    result.sort_by(|a, b| a["startAt"].as_str().cmp(&b["startAt"].as_str()));
    Ok(result)
}

/// One resource per UID from an import (`splitCalendarImport`).
pub fn split_import(content: &str) -> Result<Vec<String>> {
    let root = parse(content, "VCALENDAR")?;
    let mut groups: Vec<(String, Vec<Component>)> = Vec::new();
    for c in root
        .children
        .iter()
        .filter(|c| c.name == "VEVENT" || c.name == "VTODO")
    {
        let uid = text(c, "UID");
        if uid.is_empty() {
            return Err(invalid("Imported resources need UIDs."));
        }
        let key = format!("{}/{uid}", c.name);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(c.clone()),
            None => groups.push((key, vec![c.clone()])),
        }
    }
    if groups.is_empty() || groups.len() > 1000 {
        return Err(invalid("Import needs between 1 and 1,000 event resources."));
    }
    Ok(groups
        .into_iter()
        .map(|(_, components)| {
            let mut copy = root.clone();
            copy.children
                .retain(|c| c.name != "VEVENT" && c.name != "VTODO");
            copy.children.extend(components);
            serialize(&copy)
        })
        .collect())
}

/// Every resource of a calendar as one file, with one definition per time zone.
pub fn export(contents: &[&str]) -> Result<String> {
    let mut root = parse(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Dar//Calendar//EN\r\nEND:VCALENDAR\r\n",
        "VCALENDAR",
    )?;
    let mut zones: Vec<(String, String)> = Vec::new();
    for content in contents {
        for c in read_any(content)?.root.children {
            if c.name == "VTIMEZONE" {
                let (id, data) = (text(&c, "TZID"), serialize(&c));
                match zones.iter().find(|(z, _)| *z == id) {
                    Some((_, previous)) if *previous != data => {
                        return Err(invalid(
                            "Conflicting timezone definitions; export these events separately.",
                        ));
                    }
                    Some(_) => continue,
                    None => zones.push((id, data)),
                }
            }
            root.children.push(c);
        }
    }
    Ok(serialize(&root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(content: &str) -> Value {
        json!({ "id": "r", "collectionId": "c", "href": "e.ics", "revision": 1, "content": content })
    }

    fn ms(text: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(text)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn writes_and_expands_a_weekly_series_across_daylight_saving() {
        let input = json!({ "title": "Standup", "start": "2026-10-19T09:00", "end": "2026-10-19T09:15", "timezone": "Europe/Berlin", "allDay": false, "recurrence": "FREQ=WEEKLY;COUNT=3", "reminderMinutes": 10 });
        let event = write_event(Write {
            input: &input,
            previous: None,
            occurrence: None,
        })
        .unwrap();
        assert_eq!(event.data["reminderMinutes"], 10);
        assert!(
            event.content.contains("BEGIN:VTIMEZONE")
                && event.content.contains("TZID=Europe/Berlin"),
            "{}",
            event.content
        );
        let list = expand(
            &resource(&event.content),
            "UTC",
            ms("2026-10-01T00:00:00Z"),
            ms("2026-12-01T00:00:00Z"),
            false,
            false,
        )
        .unwrap();
        let starts: Vec<&str> = list
            .iter()
            .map(|o| o["startAt"].as_str().unwrap())
            .collect();
        // 09:00 Berlin is 07:00 UTC in summer time and 08:00 UTC after October 25.
        assert_eq!(
            starts,
            [
                "2026-10-19T07:00:00.000Z",
                "2026-10-26T08:00:00.000Z",
                "2026-11-02T08:00:00.000Z"
            ]
        );
        assert_eq!(list[1]["recurrenceId"], "2026-10-26T09:00:00");

        // Moving one occurrence and cancelling another.
        let moved = json!({ "title": "Standup (moved)", "start": "2026-10-26T11:00", "end": "2026-10-26T11:15", "timezone": "Europe/Berlin" });
        let edited = write_event(Write {
            input: &moved,
            previous: Some(&event.content),
            occurrence: Some("2026-10-26T09:00:00"),
        })
        .unwrap();
        let cancelled = cancel_occurrence(&edited.content, "2026-11-02T09:00:00").unwrap();
        let list = expand(
            &resource(&cancelled.content),
            "Europe/Berlin",
            ms("2026-10-01T00:00:00Z"),
            ms("2026-12-01T00:00:00Z"),
            false,
            false,
        )
        .unwrap();
        let titles: Vec<(&str, &str)> = list
            .iter()
            .map(|o| (o["title"].as_str().unwrap(), o["start"].as_str().unwrap()))
            .collect();
        assert_eq!(
            titles,
            [
                ("Standup", "2026-10-19T09:00:00"),
                ("Standup (moved)", "2026-10-26T11:00:00")
            ]
        );
        assert_eq!(
            expand(
                &resource(&cancelled.content),
                "UTC",
                ms("2026-10-01T00:00:00Z"),
                ms("2026-12-01T00:00:00Z"),
                false,
                true
            )
            .unwrap()
            .len(),
            3
        );
    }

    #[test]
    fn all_day_events_and_validation() {
        let input = json!({ "title": "Holiday", "start": "2026-12-24", "end": "2026-12-26", "allDay": true, "timezone": "UTC" });
        let event = write_event(Write {
            input: &input,
            previous: None,
            occurrence: None,
        })
        .unwrap();
        assert!(event.content.contains("DTSTART;VALUE=DATE:20261224"));
        let list = expand(
            &resource(&event.content),
            "Asia/Riyadh",
            ms("2026-12-01T00:00:00Z"),
            ms("2027-01-01T00:00:00Z"),
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            (list[0]["start"].as_str(), list[0]["startAt"].as_str()),
            (Some("2026-12-24"), Some("2026-12-23T21:00:00.000Z")),
            "dates are local to the viewer"
        );
        let gap = json!({ "title": "x", "start": "2026-03-29T02:30", "end": "2026-03-29T03:30", "timezone": "Europe/Berlin" });
        assert!(
            write_event(Write {
                input: &gap,
                previous: None,
                occurrence: None
            })
            .is_err(),
            "a time inside the spring-forward gap"
        );
        let backwards = json!({ "title": "x", "start": "2026-03-20T10:00", "end": "2026-03-20T09:00", "timezone": "UTC" });
        assert!(
            write_event(Write {
                input: &backwards,
                previous: None,
                occurrence: None
            })
            .is_err()
        );
        assert_eq!(split_import("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20260101T000000Z\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:b\r\nDTSTART:20260101T000000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").unwrap().len(), 2);
    }
}
