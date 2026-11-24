//! Display helpers (`pim/presentation.ts`).

/// Only actionable, supported contact schemes become links (`contactHref`).
pub fn contact_href(kind: &str, value: &str) -> Option<String> {
    let value = value.trim();
    match kind {
        "Email" => {
            let ok = value.split('@').count() == 2
                && value.split('@').all(|p| !p.is_empty())
                && !value
                    .chars()
                    .any(|c| c.is_whitespace() || c == '<' || c == '>');
            ok.then(|| {
                format!(
                    "mailto:{}",
                    js_sys::encode_uri_component(value)
                        .as_string()
                        .unwrap_or_default()
                        .replace("%40", "@")
                )
            })
        }
        "Phone" => {
            let ok = !value.is_empty()
                && value
                    .chars()
                    .all(|c| c.is_ascii_digit() || "+(). -".contains(c))
                && value.chars().any(|c| c.is_ascii_digit());
            ok.then(|| {
                format!(
                    "tel:{}",
                    value
                        .chars()
                        .filter(|c| !"(). -".contains(*c))
                        .collect::<String>()
                )
            })
        }
        "Website" => {
            let has_scheme = value.find(':').is_some_and(|i| {
                i > 0
                    && value[..i]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
                    && value
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic())
            });
            let candidate = if has_scheme {
                value.to_owned()
            } else {
                format!("https://{value}")
            };
            let url = web_sys::Url::new(&candidate).ok()?;
            (["https:", "http:"].contains(&url.protocol().as_str())
                && url.username().is_empty()
                && url.password().is_empty())
            .then(|| url.href())
        }
        _ => None,
    }
}

/// "Every week · 3 occurrences" for the rules the editor offers (`recurrenceLabel`).
pub fn recurrence_label(rule: &str, recurring: bool) -> String {
    if rule.is_empty() {
        return if recurring {
            "Custom schedule".into()
        } else {
            "Does not repeat".into()
        };
    }
    let parts: Vec<(String, String)> = rule
        .to_uppercase()
        .split(';')
        .filter_map(|p| p.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned())))
        .collect();
    let get = |k: &str| {
        parts
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.clone())
    };
    let unit = match get("FREQ").as_deref() {
        Some("DAILY") => "day",
        Some("WEEKLY") => "week",
        Some("MONTHLY") => "month",
        Some("YEARLY") => "year",
        Some("HOURLY") => "hour",
        _ => return "Custom schedule".into(),
    };
    if parts
        .iter()
        .any(|(k, _)| !["FREQ", "INTERVAL", "COUNT", "UNTIL"].contains(&k.as_str()))
    {
        return "Custom schedule".into();
    }
    let interval: u32 = get("INTERVAL").and_then(|v| v.parse().ok()).unwrap_or(1);
    let base = if interval == 1 {
        format!("Every {unit}")
    } else {
        format!("Every {interval} {unit}s")
    };
    if let Some(count) = get("COUNT") {
        return format!("{base} · {count} occurrences");
    }
    if let Some(until) = get("UNTIL") {
        if until.len() < 8 || !until[..8].bytes().all(|b| b.is_ascii_digit()) {
            return "Custom schedule".into();
        }
        let date = format!("{}-{}-{}", &until[0..4], &until[4..6], &until[6..8]);
        return format!("{base} · Until {}", task_date(&date, true));
    }
    base
}

/// A stored wall date without moving date-only values into another day (`taskDate`).
pub fn task_date(value: &str, all_day: bool) -> String {
    if value.is_empty() {
        return "No due date".into();
    }
    let text = if value.len() == 10 {
        format!("{value}T12:00:00Z")
    } else {
        format!("{}Z", value.trim_end_matches('Z'))
    };
    let ms = js_sys::Date::parse(&text);
    if ms.is_nan() {
        return value.into();
    }
    let mut options = vec![
        ("year", "numeric"),
        ("month", "short"),
        ("day", "numeric"),
        ("timeZone", "UTC"),
    ];
    if !all_day {
        options.extend([("hour", "numeric"), ("minute", "2-digit")]);
    }
    crate::dates::local_date_time(ms, &options)
}
