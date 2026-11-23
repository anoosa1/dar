//! Just enough iCalendar and vCard for DAV: content lines, components, property values,
//! and RFC 4791 time-range matching for events and tasks (as `expand` and
//! `taskMatchesRange` in `next/runtime/pim` decide it). Recurrence uses the `rrule` crate.
//! Gap: time zones resolve by IANA name only; a custom embedded VTIMEZONE falls back to UTC.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};

#[derive(Clone, Debug, Default)]
pub struct Property {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub value: String,
}

impl Property {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Values split on unescaped `;` and `,`, then unescaped (ical.js `getValues().flat()`).
    pub fn values(&self) -> Vec<String> {
        let mut out = vec![String::new()];
        let mut chars = self.value.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some('n' | 'N') => out.last_mut().unwrap().push('\n'),
                    Some(other) => out.last_mut().unwrap().push(other),
                    None => {}
                },
                ';' | ',' => out.push(String::new()),
                c => out.last_mut().unwrap().push(c),
            }
        }
        out.retain(|v| !v.is_empty());
        out
    }
}

#[derive(Clone, Debug, Default)]
pub struct Component {
    pub name: String,
    pub props: Vec<Property>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn prop(&self, name: &str) -> Option<&Property> {
        self.props.iter().find(|p| p.name == name)
    }
    pub fn props_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Property> + 'a {
        self.props.iter().filter(move |p| p.name == name)
    }
    pub fn components<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Component> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
    pub fn value(&self, name: &str) -> Option<&str> {
        self.prop(name).map(|p| p.value.as_str())
    }
}

fn parse_line(line: &str) -> Option<Property> {
    let name_end = line.find([';', ':'])?;
    let name = line[..name_end].to_ascii_uppercase();
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
        return None;
    }
    let mut rest = &line[name_end..];
    let mut params = Vec::new();
    while let Some(after) = rest.strip_prefix(';') {
        let eq = after.find('=')?;
        let key = after[..eq].to_ascii_uppercase();
        let mut value = String::new();
        let mut tail = &after[eq + 1..];
        loop {
            if let Some(quoted) = tail.strip_prefix('"') {
                let close = quoted.find('"')?;
                value.push_str(&quoted[..close]);
                tail = &quoted[close + 1..];
            } else {
                let end = tail.find([';', ':', ',']).unwrap_or(tail.len());
                value.push_str(&tail[..end]);
                tail = &tail[end..];
            }
            match tail.strip_prefix(',') {
                Some(more) => {
                    value.push(',');
                    tail = more;
                }
                None => break,
            }
        }
        params.push((key, value));
        rest = tail;
    }
    let value = rest.strip_prefix(':')?;
    Some(Property { name, params, value: value.to_owned() })
}

/// Parse one standards resource whose root is `expected` (`VCALENDAR` or `VCARD`).
pub fn parse(text: &str, expected: &str) -> Result<Component, String> {
    if text.len() > 262_144 || text.trim().is_empty() {
        return Err("Resource is empty or too large.".into());
    }
    let mut lines: Vec<String> = Vec::new();
    for raw in text.trim_start_matches('\u{feff}').split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(cont) = raw.strip_prefix([' ', '\t']) {
            lines.last_mut().ok_or("A resource cannot start with a continuation line.")?.push_str(cont);
        } else if !raw.is_empty() {
            lines.push(raw.to_owned());
        }
    }
    let mut stack: Vec<Component> = Vec::new();
    let mut root = None;
    for line in lines {
        let prop = parse_line(&line).ok_or("The standards resource could not be parsed.")?;
        match prop.name.as_str() {
            "BEGIN" => {
                if root.is_some() || stack.len() > 16 {
                    return Err("Unexpected content after the resource.".into());
                }
                stack.push(Component { name: prop.value.to_ascii_uppercase(), ..Default::default() });
            }
            "END" => {
                let done = stack.pop().ok_or("Unexpected resource ending.")?;
                if done.name != prop.value.to_ascii_uppercase() {
                    return Err("Mismatched resource ending.".into());
                }
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => root = Some(done),
                }
            }
            _ => stack.last_mut().ok_or("Text outside a standards resource.")?.props.push(prop),
        }
    }
    let root = root.filter(|_| stack.is_empty()).ok_or("Incomplete resource.")?;
    if root.name != expected {
        return Err(format!("Expected {expected}."));
    }
    Ok(root)
}

pub fn is_task(content: &str) -> bool {
    content.lines().any(|l| l.trim_end().eq_ignore_ascii_case("BEGIN:VTODO"))
}

// ---------- Time ----------

#[derive(Clone, Copy, Debug)]
enum When {
    Date(NaiveDate),
    Utc(DateTime<Utc>),
    Zoned(NaiveDateTime, chrono_tz::Tz),
    /// Floating times are read in the viewer's zone, which is UTC for DAV queries.
    Floating(NaiveDateTime),
}

impl When {
    fn parse(prop: &Property) -> Option<When> {
        let v = prop.value.trim();
        if v.len() == 8 {
            return NaiveDate::parse_from_str(v, "%Y%m%d").ok().map(When::Date);
        }
        if let Some(utc) = v.strip_suffix('Z') {
            return NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok().map(|t| When::Utc(Utc.from_utc_datetime(&t)));
        }
        let local = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
        Some(match prop.param("TZID").and_then(|z| z.trim_start_matches('/').parse::<chrono_tz::Tz>().ok()) {
            Some(zone) => When::Zoned(local, zone),
            None => When::Floating(local),
        })
    }

    fn instant(self) -> i64 {
        match self {
            When::Date(d) => d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis(),
            When::Utc(t) => t.timestamp_millis(),
            // A nonexistent local time moves forward like ical.js does; ambiguous takes the first.
            When::Zoned(t, z) => z.from_local_datetime(&t).earliest().or_else(|| z.from_local_datetime(&(t + Duration::hours(1))).earliest()).map_or(0, |t| t.timestamp_millis()),
            When::Floating(t) => t.and_utc().timestamp_millis(),
        }
    }
}

/// An RFC 5545 duration in milliseconds.
fn duration(value: &str) -> Option<i64> {
    let (sign, rest) = match value.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, value.strip_prefix('+').unwrap_or(value)),
    };
    let rest = rest.strip_prefix('P')?;
    let mut total = 0i64;
    let mut number = String::new();
    let mut in_time = false;
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => number.push(c),
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

/// Dar's overlap rule: starts before the window ends, and ends after it starts
/// (an instantaneous occurrence counts at the window start).
fn overlaps(start: i64, end: i64, from: i64, to: i64) -> bool {
    start < to && (end > from || (end == start && start >= from))
}

struct Span {
    start: When,
    length: i64,
}

fn span(c: &Component, end_property: &str) -> Option<Span> {
    let start = When::parse(c.prop("DTSTART")?)?;
    let s = start.instant();
    let length = if let Some(end) = c.prop(end_property).and_then(When::parse) {
        (end.instant() - s).max(0)
    } else if let Some(d) = c.value("DURATION").and_then(duration) {
        d.max(0)
    } else if matches!(start, When::Date(_)) {
        86_400_000
    } else {
        0
    };
    Some(Span { start, length })
}

fn rrule_time(ms: i64) -> DateTime<rrule::Tz> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default().with_timezone(&rrule::Tz::UTC)
}

/// Occurrence starts of a recurring master inside `[from - length, to]`, or `None` when the
/// rule cannot be expanded.
fn recurrence_starts(master: &Component, span: &Span, from: i64, to: i64, skip: &[i64]) -> Option<Vec<i64>> {
    let dtstart = match span.start {
        When::Zoned(t, z) => format!("DTSTART;TZID={}:{}", z.name(), t.format("%Y%m%dT%H%M%S")),
        other => format!("DTSTART:{}", Utc.timestamp_millis_opt(other.instant()).single()?.format("%Y%m%dT%H%M%SZ")),
    };
    let mut text = dtstart;
    for rule in master.props_named("RRULE") {
        // A date-only UNTIL becomes the end of that day, which the rrule crate accepts.
        let value: Vec<String> = rule
            .value
            .split(';')
            .map(|part| match part.split_once('=') {
                Some(("UNTIL", until)) if until.len() == 8 => format!("UNTIL={until}T235959Z"),
                _ => part.to_owned(),
            })
            .collect();
        text.push_str(&format!("\nRRULE:{}", value.join(";")));
    }
    let mut set: rrule::RRuleSet = text.parse().ok()?;
    for (name, add) in [("RDATE", true), ("EXDATE", false)] {
        for prop in master.props_named(name) {
            for value in prop.value.split(',') {
                // Periods (start/end) contribute their start.
                let item = Property { value: value.split('/').next().unwrap_or(value).to_owned(), ..prop.clone() };
                if let Some(t) = When::parse(&item) {
                    set = if add { set.rdate(rrule_time(t.instant())) } else { set.exdate(rrule_time(t.instant())) };
                }
            }
        }
    }
    let margin = span.length + 86_400_000;
    let result = set.after(rrule_time(from - margin)).before(rrule_time(to + 86_400_000)).all(10_000);
    let mut starts: Vec<i64> = result.dates.iter().map(|d| d.timestamp_millis()).filter(|s| !skip.contains(s)).collect();
    // DTSTART is part of the recurrence set even when only RDATE is present.
    let first = span.start.instant();
    if !starts.contains(&first) && !skip.contains(&first) && first >= from - margin && first <= to {
        starts.push(first);
    }
    Some(starts)
}

fn component_matches(root: &Component, kind: &str, from: i64, to: i64) -> bool {
    let all: Vec<&Component> = root.components(kind).collect();
    let Some(master) = all.iter().find(|c| c.prop("RECURRENCE-ID").is_none()).or(all.first()).copied() else {
        return false;
    };
    let end_property = if kind == "VTODO" { "DUE" } else { "DTEND" };
    let overrides: Vec<&Component> = all.iter().filter(|c| c.prop("RECURRENCE-ID").is_some()).copied().collect();
    let recurring = master.prop("RRULE").is_some() || master.prop("RDATE").is_some();
    if kind == "VTODO" && !recurring {
        return task_matches(master, from, to);
    }
    let Some(series) = span(master, end_property).or_else(|| {
        // A task with only DUE recurs on its due date.
        let due = When::parse(master.prop("DUE")?)?;
        Some(Span { start: due, length: 0 })
    }) else {
        return false;
    };
    if !recurring {
        let s = series.start.instant();
        return overlaps(s, s + series.length, from, to);
    }
    // Each override is checked on its own; its original slot leaves the series.
    let replaced: Vec<i64> = overrides.iter().filter_map(|c| When::parse(c.prop("RECURRENCE-ID")?)).map(When::instant).collect();
    for o in &overrides {
        if let Some(sp) = span(o, end_property) {
            let s = sp.start.instant();
            if overlaps(s, s + sp.length, from, to) {
                return true;
            }
        }
    }
    match recurrence_starts(master, &series, from, to, &replaced) {
        Some(starts) => starts.into_iter().any(|s| overlaps(s, s + series.length, from, to)),
        None => {
            let s = series.start.instant();
            overlaps(s, s + series.length, from, to)
        }
    }
}

/// RFC 4791 §9.9 for a non-recurring task.
fn task_matches(c: &Component, s: i64, e: i64) -> bool {
    let at = |name: &str| c.prop(name).and_then(When::parse).map(When::instant);
    let (start, due, completed, created) = (at("DTSTART"), at("DUE"), at("COMPLETED"), at("CREATED"));
    if let Some(start) = start {
        if let Some(d) = c.value("DURATION").and_then(duration) {
            let end = start + d;
            return s <= end && (e > start || e >= end);
        }
        if let Some(due) = due {
            return (s < due || s <= start) && (e > start || e >= due);
        }
        return s <= start && e > start;
    }
    if let Some(due) = due {
        return s < due && e >= due;
    }
    match (completed, created) {
        (Some(done), Some(made)) => (s <= made || s <= done) && (e >= made || e >= done),
        (Some(done), None) => s <= done && e >= done,
        (None, made) => made.is_none_or(|made| e > made),
    }
}

/// Whether a calendar resource has a `kind` (VEVENT or VTODO) occurrence in `[from, to)`.
pub fn matches_time_range(root: &Component, kind: &str, from: i64, to: i64) -> bool {
    component_matches(root, kind, from, to)
}

/// `YYYYMMDDTHHMMSSZ` as used by CalDAV time-range attributes.
pub fn dav_date(value: &str) -> Option<i64> {
    let utc = value.strip_suffix('Z')?;
    if utc.len() != 15 {
        return None;
    }
    NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok().map(|t| t.and_utc().timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(s: &str) -> i64 {
        dav_date(s).unwrap()
    }

    fn calendar(body: &str) -> Component {
        parse(&format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{body}END:VCALENDAR\r\n"), "VCALENDAR").unwrap()
    }

    #[test]
    fn unfolds_and_reads_params() {
        let card = parse("BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Ami\r\n na\r\nN:Doe;Jane;;;\r\nEMAIL;TYPE=\"work,pref\":j@example.test\r\nEND:VCARD\r\n", "VCARD").unwrap();
        assert_eq!(card.value("FN"), Some("Amina"));
        assert_eq!(card.prop("N").unwrap().values(), ["Doe", "Jane"]);
        assert_eq!(card.prop("EMAIL").unwrap().param("type"), Some("work,pref"));
        assert!(parse("BEGIN:VCARD\r\nEND:VCALENDAR\r\n", "VCARD").is_err());
        assert!(parse("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n", "VCARD").is_err());
    }

    #[test]
    fn single_events() {
        let c = calendar("BEGIN:VEVENT\r\nUID:a\r\nDTSTART;TZID=Europe/Berlin:20260310T100000\r\nDTEND;TZID=Europe/Berlin:20260310T110000\r\nEND:VEVENT\r\n");
        assert!(matches_time_range(&c, "VEVENT", ms("20260310T090000Z"), ms("20260310T093000Z")));
        assert!(!matches_time_range(&c, "VEVENT", ms("20260310T100000Z"), ms("20260310T120000Z")));
        let all_day = calendar("BEGIN:VEVENT\r\nUID:b\r\nDTSTART;VALUE=DATE:20260310\r\nEND:VEVENT\r\n");
        assert!(matches_time_range(&all_day, "VEVENT", ms("20260310T230000Z"), ms("20260311T000000Z")));
        assert!(!matches_time_range(&all_day, "VEVENT", ms("20260311T000000Z"), ms("20260312T000000Z")));
    }

    #[test]
    fn recurring_events_with_exceptions() {
        let c = calendar(concat!(
            "BEGIN:VEVENT\r\nUID:r\r\nDTSTART:20260105T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=WEEKLY;COUNT=10\r\nEXDATE:20260112T090000Z\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:r\r\nRECURRENCE-ID:20260119T090000Z\r\nDTSTART:20260301T090000Z\r\nDURATION:PT1H\r\nEND:VEVENT\r\n",
        ));
        assert!(matches_time_range(&c, "VEVENT", ms("20260126T080000Z"), ms("20260126T100000Z")), "a later week");
        assert!(!matches_time_range(&c, "VEVENT", ms("20260112T080000Z"), ms("20260112T100000Z")), "excluded");
        assert!(!matches_time_range(&c, "VEVENT", ms("20260119T080000Z"), ms("20260119T100000Z")), "moved away");
        assert!(matches_time_range(&c, "VEVENT", ms("20260301T080000Z"), ms("20260301T100000Z")), "moved here");
        assert!(!matches_time_range(&c, "VEVENT", ms("20260401T000000Z"), ms("20260501T000000Z")), "after COUNT");
    }

    #[test]
    fn leap_day_yearly_skips_other_years() {
        let c = calendar("BEGIN:VEVENT\r\nUID:l\r\nDTSTART;VALUE=DATE:20240229\r\nRRULE:FREQ=YEARLY\r\nEND:VEVENT\r\n");
        assert!(!matches_time_range(&c, "VEVENT", ms("20250301T000000Z"), ms("20250302T000000Z")));
        assert!(matches_time_range(&c, "VEVENT", ms("20280229T000000Z"), ms("20280301T000000Z")));
    }

    #[test]
    fn task_ranges() {
        let due = calendar("BEGIN:VTODO\r\nUID:t\r\nDUE:20260310T120000Z\r\nEND:VTODO\r\n");
        assert!(matches_time_range(&due, "VTODO", ms("20260310T000000Z"), ms("20260311T000000Z")));
        assert!(!matches_time_range(&due, "VTODO", ms("20260311T000000Z"), ms("20260312T000000Z")));
        let undated = calendar("BEGIN:VTODO\r\nUID:u\r\nEND:VTODO\r\n");
        assert!(matches_time_range(&undated, "VTODO", ms("20200101T000000Z"), ms("20200102T000000Z")));
    }

    #[test]
    fn durations() {
        assert_eq!(duration("PT1H30M"), Some(5_400_000));
        assert_eq!(duration("P1W"), Some(604_800_000));
        assert_eq!(duration("-P1D"), Some(-86_400_000));
        assert_eq!(duration("P1H"), None);
    }
}
