//! Tasks as iCalendar to-dos (`runtime/pim/tasks.ts`): validation, the projection Tasks shows,
//! form edits that keep unknown properties and exceptions, edits of one occurrence of a
//! recurring task, occurrences (through the event expander) and the day a task is due.

use chrono::{Duration, NaiveDate, NaiveDateTime, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};

use super::calendar::{Event, Time, checked_rule, duration_ms, exists, expand, iso, parse_key, time, time_property, timezone_component, wall, wall_epoch, zone};
use super::vobject::{Component, Property, escape, field, invalid, parse, prop, remove, serialize, set, set_text, text};
use crate::error::Result;

const STATUSES: [&str; 4] = ["NEEDS-ACTION", "IN-PROCESS", "COMPLETED", "CANCELLED"];

pub fn is_task(content: &str) -> bool {
    dar_dav::ical::is_task(content)
}

fn value(c: &Component, name: &str) -> String {
    text(c, name)
}

/// DUE, else DTSTART plus DURATION.
fn effective_due(c: &Component) -> Option<Time> {
    if let Some(due) = time(c, "DUE") {
        return Some(due);
    }
    let start = time(c, "DTSTART")?;
    let ms = c.value("DURATION").and_then(duration_ms)?;
    Some(start.with_local(start.local + Duration::milliseconds(ms)))
}

fn display_alarm(c: &Component) -> Option<&Component> {
    c.components("VALARM").find(|a| text(a, "ACTION") == "DISPLAY")
}

/// The fields Tasks shows (`data`).
fn data(c: &Component) -> Value {
    let start = time(c, "DTSTART");
    let due = effective_due(c);
    let anchor = start.as_ref().or(due.as_ref());
    let timezone = anchor.map_or("floating".to_owned(), |t| t.zone_name());
    let completed = time(c, "COMPLETED").map(|t| iso(t.instant(Tz::UTC))).unwrap_or_default();
    let categories: Vec<String> = c.props_named("CATEGORIES").flat_map(|p| p.values()).collect();
    let parent = c.props_named("RELATED-TO").find(|p| p.param("RELTYPE").is_none_or(|r| r.eq_ignore_ascii_case("PARENT"))).map(|p| p.value.trim().to_owned()).unwrap_or_default();
    let reminder = display_alarm(c).and_then(|a| a.prop("TRIGGER")).filter(|t| t.param("VALUE") != Some("DATE-TIME")).and_then(|t| duration_ms(t.value.trim())).map_or(Value::Null, |ms| json!(-(ms as f64) / 60_000.0));
    let state = value(c, "STATUS");
    json!({
        "kind": "task", "title": value(c, "SUMMARY"), "description": value(c, "DESCRIPTION"),
        "start": start.as_ref().map(|t| t.wall_string()).unwrap_or_default(), "due": due.as_ref().map(|t| t.wall_string()).unwrap_or_default(),
        "timezone": timezone, "allDay": anchor.is_some_and(|t| t.date),
        "priority": number(&value(c, "PRIORITY")), "status": if state.is_empty() { "NEEDS-ACTION".to_owned() } else { state },
        "progress": number(&value(c, "PERCENT-COMPLETE")), "completedAt": completed, "categories": categories, "parentUid": parent,
        "recurrence": c.value("RRULE").unwrap_or_default().trim(), "recurring": c.prop("RRULE").is_some() || c.prop("RDATE").is_some(),
        "reminderMinutes": reminder,
    })
}

/// `Number(value ?? 0)`: an empty value is 0, text that is not a number is NaN (None).
fn number(value: &str) -> Value {
    if value.trim().is_empty() {
        return json!(0);
    }
    match value.trim().parse::<f64>() {
        Ok(n) if n.fract() == 0.0 => json!(n as i64),
        Ok(n) => json!(n),
        Err(_) => Value::Null,
    }
}

fn integer_in(v: &Value, low: i64, high: i64) -> bool {
    v.as_i64().is_some_and(|n| (low..=high).contains(&n))
}

fn masters(root: &Component) -> Vec<usize> {
    root.children.iter().enumerate().filter(|(_, c)| c.name == "VTODO" && c.prop("RECURRENCE-ID").is_none()).map(|(i, _)| i).collect()
}

/// A task resource, validated (`readTask`).
pub fn read_task(content: &str) -> Result<Event> {
    let root = parse(content, "VCALENDAR")?;
    let found = masters(&root);
    if text(&root, "VERSION") != "2.0" || found.len() != 1 || root.children.iter().any(|c| c.name != "VTODO" && c.name != "VTIMEZONE") {
        return Err(invalid("Expected one VTODO series and its timezone definitions."));
    }
    let master = &root.children[found[0]];
    let uid = value(master, "UID");
    if uid.is_empty() || uid.encode_utf16().count() > 1024 {
        return Err(invalid("A task needs a stable UID."));
    }
    let mut seen: Vec<String> = Vec::new();
    for c in root.components("VTODO") {
        let p = data(c);
        let start = time(c, "DTSTART");
        let due = time(c, "DUE");
        let duration = c.value("DURATION").map(|d| duration_ms(d.trim()));
        if value(c, "UID") != uid {
            return Err(invalid("Task components must share a UID."));
        }
        if !STATUSES.contains(&p["status"].as_str().unwrap_or_default()) || !integer_in(&p["progress"], 0, 100) || !integer_in(&p["priority"], 0, 9) {
            return Err(invalid("Invalid task status, progress or priority."));
        }
        if p["recurring"] == true && start.is_none() {
            return Err(invalid("Recurring tasks need a start date."));
        }
        if let Some(duration) = duration {
            let raw = c.value("DURATION").unwrap_or_default();
            let timed = raw.contains('T');
            if start.is_none() || due.is_some() || duration.is_none_or(|ms| ms <= 0) || start.as_ref().is_some_and(|s| s.date) && timed {
                return Err(invalid("A positive duration requires a start and cannot accompany DUE."));
            }
        }
        for key in ["DTSTART", "DUE", "RECURRENCE-ID", "COMPLETED", "CREATED"] {
            let Some(t) = time(c, key) else {
                if c.prop(key).is_some() {
                    return Err(invalid("Invalid calendar date."));
                }
                continue;
            };
            let _ = t;
            let tzid = c.prop(key).and_then(|p| p.param("TZID")).map(|z| z.trim_start_matches('/').to_owned()).unwrap_or_default();
            if !tzid.is_empty() && !root.components("VTIMEZONE").any(|z| text(z, "TZID") == tzid) {
                zone(&tzid)?;
            }
        }
        if let (Some(s), Some(d)) = (&start, &due)
            && (s.date != d.date || d.instant(Tz::UTC) < s.instant(Tz::UTC))
        {
            return Err(invalid("The due date must not precede the start."));
        }
        if c.prop("RECURRENCE-ID").is_some() {
            let id = value(c, "RECURRENCE-ID");
            if seen.contains(&id) {
                return Err(invalid("Duplicate task exception."));
            }
            seen.push(id);
        }
        if p["parentUid"] == json!(uid) {
            return Err(invalid("A task cannot be its own parent."));
        }
        let rule = p["recurrence"].as_str().unwrap_or_default();
        if !rule.is_empty() {
            let anchor = start.clone().or(due.clone()).unwrap_or(Time { local: NaiveDate::from_ymd_opt(2000, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).expect("date"), date: false, utc: true, tzid: None });
            checked_rule(rule, &anchor).map_err(|_| invalid("Invalid recurrence rule."))?;
        }
    }
    let data = data(master);
    Ok(Event { uid, content: serialize(&root), data, root })
}

/// Sets DTSTART, DUE or RECURRENCE-ID from a form value (`setTime`).
fn set_time(c: &mut Component, key: &str, value: &str, all_day: bool, zone_name: &str) -> Result<()> {
    remove(c, key);
    if value.is_empty() {
        return Ok(());
    }
    let value = value.strip_suffix('Z').unwrap_or(value);
    let date_only = value.len() == 10 && NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok();
    if if all_day { !date_only } else { value.len() == 10 } {
        return Err(invalid("Use a date-only value or date/time matching the task type."));
    }
    let iana = zone(zone_name).ok();
    let t = if all_day {
        let d = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| invalid("Invalid calendar date."))?;
        Time { local: d.and_hms_opt(0, 0, 0).expect("midnight"), date: true, utc: false, tzid: None }
    } else {
        let full = if value.len() == 16 { format!("{value}:00") } else { value.to_owned() };
        let local = NaiveDateTime::parse_from_str(&full, "%Y-%m-%dT%H:%M:%S").map_err(|_| invalid("Invalid date and time."))?;
        // Embedded custom zones remain valid imports; native IANA edits reject gaps.
        if zone_name != "floating"
            && let Some(tz) = iana
            && !exists(local, tz)
        {
            return Err(invalid("That local time does not exist because the clocks change."));
        }
        Time { local, date: false, utc: zone_name == "UTC", tzid: (zone_name != "UTC" && zone_name != "floating").then(|| zone_name.to_owned()) }
    };
    c.props.push(time_property(key, &t));
    Ok(())
}

/// `ICAL.Duration.fromSeconds(seconds).toString()`.
fn duration_text(seconds: i64) -> String {
    if seconds == 0 {
        return "PT0S".into();
    }
    let mut secs = seconds.abs();
    let mut days = secs / 86_400;
    let mut weeks = 0;
    if days % 7 == 0 {
        weeks = days / 7;
        days = 0;
    }
    secs -= (days + 7 * weeks) * 86_400;
    let (hours, minutes, rest) = (secs / 3600, secs % 3600 / 60, secs % 60);
    let mut out = String::from(if seconds < 0 { "-P" } else { "P" });
    if weeks > 0 {
        out += &format!("{weeks}W");
    }
    if days > 0 {
        out += &format!("{days}D");
    }
    if hours > 0 || minutes > 0 || rest > 0 {
        out.push('T');
        if hours > 0 {
            out += &format!("{hours}H");
        }
        if minutes > 0 {
            out += &format!("{minutes}M");
        }
        if rest > 0 {
            out += &format!("{rest}S");
        }
    }
    out
}

fn now_utc() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

fn input_value<'a>(p: &'a Map<String, Value>, key: &str, baseline: &'a Value) -> &'a Value {
    match p.get(key) {
        Some(v) if !v.is_null() => v,
        _ => &baseline[key],
    }
}

/// Edits the source component in place so unknown properties and exceptions survive (`writeTask`).
pub fn write_task(input: &Value, previous: Option<&str>, occurrence_id: Option<&str>) -> Result<Event> {
    let p = input.as_object().ok_or_else(|| invalid("Invalid task."))?;
    let old = previous.map(read_task).transpose()?;
    let mut root = match &old {
        Some(o) => o.root.clone(),
        None => {
            let mut root = Component { name: "VCALENDAR".into(), ..Default::default() };
            set(&mut root, prop("VERSION", &[], "2.0"));
            set(&mut root, prop("PRODID", &[], "-//Dar//Tasks//EN"));
            root.children.push(Component { name: "VTODO".into(), ..Default::default() });
            root
        }
    };
    let master = masters(&root).first().copied().unwrap_or(0);
    let mut index = master;
    if let Some(occurrence) = occurrence_id {
        let Some(old) = old.as_ref().filter(|o| o.data["recurring"] == true) else { return Err(invalid("Not a recurring task.")) };
        let stamp = occurrence.strip_suffix('Z').unwrap_or(occurrence);
        let found = root.children.iter().position(|t| t.name == "VTODO" && t.prop("RECURRENCE-ID").is_some() && value(t, "RECURRENCE-ID").trim_end_matches('Z') == stamp.replace(['-', ':'], ""));
        let found = found.or_else(|| root.children.iter().position(|t| t.name == "VTODO" && time(t, "RECURRENCE-ID").is_some_and(|r| r.wall_string() == stamp)));
        index = match found {
            Some(i) => i,
            None => {
                let mut copy = root.children[master].clone();
                for name in ["RRULE", "RDATE", "EXDATE"] {
                    remove(&mut copy, name);
                }
                let around = parse_key(stamp).ok_or_else(|| invalid("Invalid occurrence."))?.local.and_utc().timestamp_millis();
                let slot = task_occurrences(&json!({ "id": "slot", "collectionId": "", "href": "", "uid": old.uid, "content": previous.unwrap_or_default(), "projection": "{}", "revision": 0, "modifiedAt": 0 }), "UTC", around - 2 * 86_400_000, around + 2 * 86_400_000, false, false)?
                    .into_iter()
                    .find(|o| o["recurrenceId"].as_str().unwrap_or_default().trim_end_matches('Z') == stamp);
                let Some(slot) = slot else { return Err(invalid("That occurrence is not part of this series.")) };
                let (all_day, zone_name) = (old.data["allDay"] == true, old.data["timezone"].as_str().unwrap_or("floating").to_owned());
                set_time(&mut copy, "RECURRENCE-ID", occurrence, all_day, &zone_name)?;
                remove(&mut copy, "DURATION");
                let (slot_day, slot_zone) = (slot["allDay"] == true, slot["timezone"].as_str().unwrap_or("floating").to_owned());
                set_time(&mut copy, "DTSTART", slot["start"].as_str().unwrap_or_default(), slot_day, &slot_zone)?;
                set_time(&mut copy, "DUE", slot["due"].as_str().unwrap_or_default(), slot_day, &slot_zone)?;
                root.children.push(copy);
                root.children.len() - 1
            }
        };
        if index == master {
            return Err(invalid("Invalid occurrence."));
        }
    }
    let baseline = data(&root.children[index]);
    let title = field(input_value(p, "title", &baseline), 1000)?.trim().to_owned();
    if title.is_empty() {
        return Err(invalid("A task needs a title."));
    }
    let zone_name = field(input_value(p, "timezone", &baseline), 100)?;
    let all_day = match p.get("allDay") {
        None => baseline["allDay"] == true,
        Some(v) => *v == json!(true),
    };
    if zone_name != "floating" && !root.components("VTIMEZONE").any(|z| text(z, "TZID") == zone_name) {
        zone(&zone_name)?;
    }
    let status = field(input_value(p, "status", &baseline), 30)?;
    let progress = match p.get("progress").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())),
        None if status == "COMPLETED" => Some(100.0),
        None => baseline["progress"].as_f64(),
    };
    let priority = match p.get("priority").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())),
        None => baseline["priority"].as_f64(),
    };
    let number_text = |n: Option<f64>| n.map(|n| if n.fract() == 0.0 { (n as i64).to_string() } else { n.to_string() }).unwrap_or_else(|| "NaN".into());
    let uid = old.as_ref().map(|o| o.uid.clone()).unwrap_or_else(crate::auth::uuid);
    let description = field(input_value(p, "description", &baseline), 65_536)?;
    let starts = [("start", "DTSTART"), ("due", "DUE")];
    let mut values: Vec<(String, &str)> = Vec::new();
    for (key, name) in starts {
        values.push((field(input_value(p, key, &baseline), 100)?, name));
    }
    {
        let c = &mut root.children[index];
        set_text(c, "UID", &uid);
        set_text(c, "SUMMARY", &title);
        set_text(c, "DESCRIPTION", &description);
        set_text(c, "STATUS", &status);
        set(c, prop("PERCENT-COMPLETE", &[], &if status == "COMPLETED" { "100".to_owned() } else { number_text(progress) }));
        set(c, prop("PRIORITY", &[], &number_text(priority)));
        if p.contains_key("due") {
            remove(c, "DURATION");
        }
        for (v, name) in &values {
            if *name != "DUE" || c.prop("DURATION").is_none() {
                set_time(c, name, v, all_day, &zone_name)?;
            }
        }
        remove(c, "COMPLETED");
        if status == "COMPLETED" {
            let at = baseline["completedAt"].as_str().filter(|s| !s.is_empty()).and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc)).unwrap_or_else(Utc::now);
            c.props.push(prop("COMPLETED", &[], &at.format("%Y%m%dT%H%M%SZ").to_string()));
        }
        if let Some(parent) = p.get("parentUid") {
            c.props.retain(|t| !(t.name == "RELATED-TO" && t.param("RELTYPE").is_none_or(|r| r.eq_ignore_ascii_case("PARENT"))));
            if parent.as_str().is_some_and(|s| !s.is_empty()) {
                let value = field(parent, 1024)?;
                c.props.push(prop("RELATED-TO", &[("RELTYPE", "PARENT")], &escape(&value)));
            }
        }
        if let Some(categories) = p.get("categories") {
            let list = categories.as_array().filter(|a| a.len() <= 50).ok_or_else(|| invalid("Invalid categories."))?;
            remove(c, "CATEGORIES");
            if !list.is_empty() {
                let values = list.iter().map(|v| field(v, 100).map(|v| escape(&v))).collect::<Result<Vec<_>>>()?;
                c.props.push(prop("CATEGORIES", &[], &values.join(",")));
            }
        }
        if let Some(recurrence) = p.get("recurrence").filter(|_| occurrence_id.is_none()) {
            remove(c, "RRULE");
            let rule = if recurrence.is_null() { String::new() } else { field(recurrence, 1024)? };
            if !rule.is_empty() {
                let anchor = time(c, "DTSTART").or_else(|| time(c, "DUE")).unwrap_or(Time { local: NaiveDate::from_ymd_opt(2000, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).expect("date"), date: false, utc: true, tzid: None });
                let checked = checked_rule(&rule, &anchor).map_err(|_| invalid("Invalid recurrence rule."))?;
                c.props.push(prop("RRULE", &[], &checked));
            }
        }
        if let Some(minutes) = p.get("reminderMinutes")
            && !(minutes.is_null() && baseline["reminderMinutes"].is_null())
        {
            c.children.retain(|a| !(a.name == "VALARM" && text(a, "ACTION") == "DISPLAY"));
            if !minutes.is_null() {
                let m = minutes.as_f64().or_else(|| minutes.as_str().and_then(|s| s.trim().parse().ok()));
                let has_time = time(c, "DTSTART").is_some() || time(c, "DUE").is_some();
                let Some(m) = m.filter(|m| m.fract() == 0.0 && (0.0..=10_080.0).contains(m) && has_time) else { return Err(invalid("Invalid task reminder.")) };
                let mut alarm = Component { name: "VALARM".into(), ..Default::default() };
                alarm.props.push(prop("ACTION", &[], "DISPLAY"));
                alarm.props.push(prop("DESCRIPTION", &[], &escape(&title)));
                let params: &[(&str, &str)] = if time(c, "DTSTART").is_none() { &[("RELATED", "END")] } else { &[] };
                alarm.props.push(prop("TRIGGER", params, &duration_text(-(m as i64) * 60)));
                c.children.push(alarm);
            }
        }
    }
    if zone_name != "UTC" && zone_name != "floating" && !root.components("VTIMEZONE").any(|z| text(z, "TZID") == zone_name) {
        let c = &root.children[index];
        if let Some(anchor) = time(c, "DTSTART").or_else(|| time(c, "DUE")) {
            use chrono::Datelike;
            root.children.push(timezone_component(&zone_name, anchor.local.year())?);
        }
    }
    set(&mut root.children[index], prop("DTSTAMP", &[], &now_utc()));
    if occurrence_id.is_some()
        && let Some(old) = &old
    {
        let master_status = value(&root.children[master], "STATUS");
        if status != "COMPLETED" && master_status == "COMPLETED" {
            let m = &mut root.children[master];
            set(m, prop("STATUS", &[], "NEEDS-ACTION"));
            set(m, prop("PERCENT-COMPLETE", &[], "0"));
            remove(m, "COMPLETED");
        }
        if status == "COMPLETED" {
            let pending = next_task_occurrence(&json!({ "id": "pending", "collectionId": "", "href": "", "uid": old.uid, "content": serialize(&root), "projection": "{}", "revision": 0, "modifiedAt": 0 }), "UTC")?;
            if pending.is_none() {
                let m = &mut root.children[master];
                set(m, prop("STATUS", &[], "COMPLETED"));
                set(m, prop("PERCENT-COMPLETE", &[], "100"));
                set(m, prop("COMPLETED", &[], &now_utc()));
            }
        }
    }
    read_task(&serialize(&root))
}

pub fn next_task_occurrence(row: &Value, zone_name: &str) -> Result<Option<Value>> {
    let from = NaiveDate::from_ymd_opt(1, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).expect("date").and_utc().timestamp_millis();
    let to = NaiveDate::from_ymd_opt(9999, 12, 31).and_then(|d| d.and_hms_opt(23, 59, 59)).expect("date").and_utc().timestamp_millis();
    Ok(task_occurrences(row, zone_name, from, to, true, true)?.into_iter().next())
}

/// A task's occurrences in `[from, to)`: each to-do becomes an event from its start (or due) to
/// its due, expanded like an event series (`taskOccurrences`).
pub fn task_occurrences(row: &Value, zone_name: &str, from: i64, to: i64, pending_only: bool, first: bool) -> Result<Vec<Value>> {
    let task = read_task(row["content"].as_str().unwrap_or_default())?;
    let status = task.data["status"].as_str().unwrap_or_default();
    if pending_only && (status == "COMPLETED" || status == "CANCELLED") {
        return Ok(Vec::new());
    }
    if task.data["start"] == "" && task.data["due"] == "" {
        return Ok(Vec::new());
    }
    let mut converted = Component { name: "VCALENDAR".into(), ..Default::default() };
    set(&mut converted, prop("VERSION", &[], "2.0"));
    for component in &task.root.children {
        if component.name == "VTIMEZONE" {
            converted.children.push(component.clone());
            continue;
        }
        let mut event = component.clone();
        event.name = "VEVENT".into();
        let start = time(&event, "DTSTART").or_else(|| effective_due(&event)).ok_or_else(|| invalid("Invalid task."))?;
        let due_zone = event.prop("DUE").and_then(|p| p.param("TZID")).map(str::to_owned);
        if time(&event, "DTSTART").is_none() {
            let mut p: Property = time_property("DTSTART", &start);
            if let Some(z) = &due_zone {
                p.params.retain(|(k, _)| k != "TZID");
                p.params.push(("TZID".into(), z.clone()));
            }
            set(&mut event, p);
        }
        let mut end = effective_due(&event).unwrap_or_else(|| start.clone());
        if start.date {
            end = end.with_local(end.local + Duration::days(1));
        }
        let end_zone = due_zone.or_else(|| event.prop("DTSTART").and_then(|p| p.param("TZID")).map(str::to_owned));
        remove(&mut event, "DUE");
        remove(&mut event, "DURATION");
        let mut end_prop = time_property("DTEND", &end);
        if let Some(z) = end_zone.filter(|_| !end.utc && !end.date) {
            end_prop.params.retain(|(k, _)| k != "TZID");
            end_prop.params.push(("TZID".into(), z));
        }
        set(&mut event, end_prop);
        remove(&mut event, "STATUS");
        let state = data(component)["status"].as_str().unwrap_or_default().to_owned();
        if pending_only && (state == "COMPLETED" || state == "CANCELLED") {
            set(&mut event, prop("STATUS", &[], "CANCELLED"));
        }
        // Task alarms may be relative to the end, which events do not show; actions stay valid.
        event.children.retain(|a| a.name != "VALARM" || ["DISPLAY", "AUDIO", "EMAIL"].contains(&text(a, "ACTION").as_str()));
        converted.children.push(event);
    }
    let resource = json!({ "id": row["id"], "collectionId": row["collectionId"], "href": row["href"], "revision": row["revision"], "content": serialize(&converted) });
    let viewer = zone(zone_name)?;
    let occurrences = expand(&resource, zone_name, from, to, first, !pending_only)?;
    let mut out = Vec::new();
    for o in occurrences {
        let key = o["recurrenceId"].as_str().unwrap_or_default().trim_end_matches('Z').to_owned();
        let exception = task.root.components("VTODO").find(|c| c.prop("RECURRENCE-ID").is_some() && time(c, "RECURRENCE-ID").is_some_and(|t| t.wall_string() == key));
        let component = exception.unwrap_or_else(|| task.root.components("VTODO").find(|c| c.prop("RECURRENCE-ID").is_none()).expect("master"));
        let p = data(component);
        let all_day = p["allDay"] == true;
        let end_text = o["end"].as_str().unwrap_or_default();
        let end = if all_day {
            NaiveDate::parse_from_str(end_text, "%Y-%m-%d").ok().map(|d| (d - Duration::days(1)).format("%Y-%m-%d").to_string()).unwrap_or_default()
        } else {
            end_text.to_owned()
        };
        let due = if p["due"] != "" { end } else { String::new() };
        let start = if p["start"] != "" { o["start"].as_str().unwrap_or_default().to_owned() } else { String::new() };
        let related = display_alarm(component).and_then(|a| a.prop("TRIGGER")).and_then(|t| t.param("RELATED")).map(str::to_owned);
        let end_at = chrono::DateTime::parse_from_rfc3339(o["endAt"].as_str().unwrap_or_default()).map(|d| d.timestamp_millis()).unwrap_or(0);
        let start_at = chrono::DateTime::parse_from_rfc3339(o["startAt"].as_str().unwrap_or_default()).map(|d| d.timestamp_millis()).unwrap_or(0);
        let end_epoch = if all_day && !due.is_empty() { NaiveDate::parse_from_str(&due, "%Y-%m-%d").map(|d| wall_epoch(d.and_hms_opt(0, 0, 0).expect("midnight"), viewer)).unwrap_or(end_at) } else { end_at };
        let alarm_at = if related.as_deref() == Some("END") && p["start"] != "" { end_epoch } else { start_at };
        let mut map = o.as_object().cloned().unwrap_or_default();
        if let Value::Object(fields) = &p {
            for (k, v) in fields {
                map.insert(k.clone(), v.clone());
            }
        }
        map.insert("start".into(), json!(start));
        map.insert("due".into(), json!(due));
        map.insert("recurrenceId".into(), o["recurrenceId"].clone());
        map.insert("startAt".into(), o["startAt"].clone());
        map.insert("endAt".into(), o["endAt"].clone());
        map.insert("alarmAt".into(), json!(alarm_at));
        out.push(Value::Object(map));
    }
    Ok(out)
}

/// Date-only tasks keep their calendar day; timed tasks use the viewer's day (`taskDue`).
pub fn task_due(row: &Value, viewer_zone: &str, occurrence: Option<&Value>) -> Result<(String, String)> {
    let task = read_task(row["content"].as_str().unwrap_or_default())?;
    let p = occurrence.unwrap_or(&task.data);
    let due = p["due"].as_str().unwrap_or_default();
    if due.is_empty() {
        return Ok((String::new(), "9999".into()));
    }
    if p["allDay"] == true {
        return Ok((due.chars().take(10).collect(), due.to_owned()));
    }
    let viewer = zone(viewer_zone)?;
    let epoch = match occurrence {
        Some(o) => chrono::DateTime::parse_from_rfc3339(o["endAt"].as_str().unwrap_or_default()).map(|d| d.timestamp_millis()).unwrap_or(0),
        None => {
            let master = task.root.components("VTODO").find(|c| c.prop("RECURRENCE-ID").is_none()).expect("master");
            let t = effective_due(master).ok_or_else(|| invalid("Invalid task."))?;
            let zone_name = p["timezone"].as_str().unwrap_or("floating");
            if t.utc || t.tzid.as_deref().is_some_and(|z| z.parse::<Tz>().is_ok()) {
                t.instant(viewer)
            } else {
                wall_epoch(t.local, if zone_name == "floating" { viewer } else { zone(zone_name).unwrap_or(viewer) })
            }
        }
    };
    Ok((wall(epoch, viewer).format("%Y-%m-%d").to_string(), iso(epoch)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_round_trip() {
        let made = write_task(&json!({ "title": "Pay rent", "due": "2026-12-01", "allDay": true, "priority": 1, "reminderMinutes": 60 }), None, None).unwrap();
        assert_eq!(made.data["due"], "2026-12-01");
        assert_eq!(made.data["allDay"], true);
        assert_eq!(made.data["reminderMinutes"], 60.0);
        assert!(made.content.contains("TRIGGER;RELATED=END:-PT1H"));
        let done = write_task(&json!({ "status": "COMPLETED" }), Some(&made.content), None).unwrap();
        assert_eq!((done.data["status"].as_str(), done.data["progress"].as_i64()), (Some("COMPLETED"), Some(100)));
        assert!(!done.data["completedAt"].as_str().unwrap().is_empty());
        assert_eq!(duration_text(-604_800), "-P1W");
        assert_eq!(duration_text(0), "PT0S");
        let (day, _) = task_due(&json!({ "content": made.content }), "UTC", None).unwrap();
        assert_eq!(day, "2026-12-01");
    }

    #[test]
    fn recurring_occurrences() {
        let made = write_task(&json!({ "title": "Water plants", "start": "2026-01-05T09:00", "due": "2026-01-05T10:00", "timezone": "Europe/Berlin", "recurrence": "FREQ=WEEKLY" }), None, None).unwrap();
        let row = json!({ "id": "r", "collectionId": "c", "href": "h", "revision": 1, "content": made.content });
        let next = next_task_occurrence(&row, "UTC").unwrap().unwrap();
        assert_eq!(next["start"], "2026-01-05T09:00:00");
        let edited = write_task(&json!({ "status": "COMPLETED" }), Some(&made.content), Some("2026-01-05T09:00:00")).unwrap();
        let row = json!({ "id": "r", "collectionId": "c", "href": "h", "revision": 2, "content": edited.content });
        let next = next_task_occurrence(&row, "UTC").unwrap().unwrap();
        assert_eq!(next["start"], "2026-01-12T09:00:00");
    }
}
