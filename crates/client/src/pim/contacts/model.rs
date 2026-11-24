//! Contacts data model, sorting, birthday countdowns, duplicate detection, merge, and vCard QR.

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::pim::{Collection, Item};

pub const BASE: &str = "/api/v1/contacts/books";

pub const VIEW_MODES: [(&str, &str, &str); 3] = [
    ("list", "List", "list"),
    ("table", "Table", "table"),
    ("cards", "Cards", "grid"),
];

pub const SORT_OPTIONS: [(&str, &str); 3] = [
    ("firstName", "First name"),
    ("lastName", "Last name"),
    ("organization", "Organization"),
];

pub const EMAIL_LABELS: [(&str, &str); 3] =
    [("work", "Work"), ("home", "Home"), ("other", "Other")];

pub const PHONE_LABELS: [(&str, &str); 4] = [
    ("mobile", "Mobile"),
    ("work", "Work"),
    ("home", "Home"),
    ("other", "Other"),
];

pub const URL_LABELS: [(&str, &str); 4] = [
    ("work", "Work"),
    ("home", "Home"),
    ("blog", "Blog"),
    ("other", "Other"),
];

pub const ADDRESS_TYPE_LABELS: [(&str, &str); 3] =
    [("home", "Home"), ("work", "Work"), ("other", "Other")];

pub const SOCIAL_SERVICES: [(&str, &str); 7] = [
    ("github", "GitHub"),
    ("linkedin", "LinkedIn"),
    ("mastodon", "Mastodon"),
    ("signal", "Signal"),
    ("matrix", "Matrix"),
    ("x", "X"),
    ("other", "Other"),
];

pub const RELATED_TYPES: [(&str, &str); 6] = [
    ("spouse", "Spouse"),
    ("partner", "Partner"),
    ("manager", "Manager"),
    ("assistant", "Assistant"),
    ("parent", "Parent"),
    ("other", "Other"),
];

pub const ADDRESS_LABELS: [(&str, &str); 7] = [
    ("street", "Street"),
    ("extended", "Apartment or suite"),
    ("poBox", "PO box"),
    ("city", "City"),
    ("region", "Region"),
    ("postalCode", "Postal code"),
    ("country", "Country"),
];

#[derive(Deserialize)]
pub struct Books {
    pub items: Vec<Collection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct People {
    pub items: Vec<Item>,
    pub groups: Vec<String>,
    pub next_cursor: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContactScope {
    All,
    Favorites,
    Ungrouped,
    Merge,
    Book(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactFilter {
    All,
    Email,
    Phone,
    Birthday,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpcomingBirthday {
    pub collection_id: String,
    pub href: String,
    pub name: String,
    pub days_until: u32,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DuplicatePair {
    pub key: String,
    pub reason: String,
    pub primary: Item,
    pub secondary: Item,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContactToast {
    pub message: String,
    pub undo: Option<UndoAction>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UndoAction {
    ToggleFavorite {
        collection_id: String,
        href: String,
        revision: u64,
        previous_data: Value,
    },
}

pub fn s(data: &Value, key: &str) -> String {
    data[key].as_str().unwrap_or_default().to_owned()
}

pub fn empty() -> Value {
    json!({
        "displayName": "",
        "firstName": "",
        "lastName": "",
        "middleName": "",
        "prefix": "",
        "suffix": "",
        "nickname": "",
        "pronouns": "",
        "organization": "",
        "department": "",
        "title": "",
        "notes": "",
        "birthday": "",
        "anniversary": "",
        "emails": [],
        "phones": [],
        "urls": [],
        "addresses": [],
        "socials": [],
        "related": [],
        "groups": [],
        "favorite": false,
        "hasPhoto": false,
    })
}

pub fn capitalized(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

pub fn display_name(data: &Value, order: &str) -> String {
    if order == "lastFirst" {
        let first = s(data, "firstName");
        let last = s(data, "lastName");
        if !last.is_empty() && !first.is_empty() {
            return format!("{last}, {first}");
        }
    }
    let dn = s(data, "displayName");
    if !dn.is_empty() {
        dn
    } else {
        let joined = [s(data, "firstName"), s(data, "lastName")]
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if joined.is_empty() {
            "Unnamed contact".into()
        } else {
            joined
        }
    }
}

pub fn work_line(data: &Value) -> String {
    [
        s(data, "title"),
        s(data, "department"),
        s(data, "organization"),
    ]
    .into_iter()
    .filter(|v| !v.is_empty())
    .collect::<Vec<_>>()
    .join(" · ")
}

pub fn subtitle(data: &Value) -> String {
    let work = work_line(data);
    if !work.is_empty() {
        work
    } else {
        data["emails"][0]["value"]
            .as_str()
            .or(data["phones"][0]["value"].as_str())
            .unwrap_or("Contact")
            .to_owned()
    }
}

pub fn photo_url(collection_id: &str, href: &str, revision: u64) -> String {
    format!("{BASE}/{collection_id}/items/{href}?photo=1&v={revision}")
}

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x300..=0x36f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe20..=0xfe2f)
}

/// The letter heading for a row under the current sort mode.
pub fn sort_letter(data: &Value, sort: &str) -> String {
    let preferred = match sort {
        "lastName" => s(data, "lastName"),
        "organization" => s(data, "organization"),
        _ => s(data, "firstName"),
    };
    let key = if preferred.is_empty() {
        if sort == "organization" {
            return "#".into();
        }
        s(data, "displayName")
    } else {
        preferred
    };
    let decomposed: String = js_sys::JsString::from(key.trim()).normalize("NFD").into();
    let first = decomposed
        .chars()
        .find(|c| !is_mark(*c))
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_default();
    if first.chars().next().is_some_and(char::is_alphabetic) {
        first
    } else {
        "#".into()
    }
}

pub fn sort_contacts(rows: &mut [Item], sort: &str) {
    if sort == "organization" {
        rows.sort_by(|a, b| {
            let oa = s(&a.data, "organization").to_lowercase();
            let ob = s(&b.data, "organization").to_lowercase();
            let ka = (oa.is_empty(), oa, s(&a.data, "displayName").to_lowercase());
            let kb = (ob.is_empty(), ob, s(&b.data, "displayName").to_lowercase());
            ka.cmp(&kb)
        });
    } else if sort == "lastName" {
        rows.sort_by(|a, b| {
            let la = s(&a.data, "lastName");
            let lb = s(&b.data, "lastName");
            let ka = (
                if la.is_empty() {
                    s(&a.data, "displayName").to_lowercase()
                } else {
                    la.to_lowercase()
                },
                s(&a.data, "firstName").to_lowercase(),
            );
            let kb = (
                if lb.is_empty() {
                    s(&b.data, "displayName").to_lowercase()
                } else {
                    lb.to_lowercase()
                },
                s(&b.data, "firstName").to_lowercase(),
            );
            ka.cmp(&kb)
        });
    }
}

/// vCard dates (YYYY-MM-DD or --MM-DD) formatted without time zone shifts.
pub fn format_date(value: &str) -> String {
    let date = |y: i32, m: u32, d: u32| {
        js_sys::Date::utc(y as f64, (m - 1) as f64) + (d as f64 - 1.0) * 86_400_000.0
    };
    let parts: Vec<&str> = value.split('-').collect();
    match parts.as_slice() {
        [y, m, d] if y.len() == 4 => match (y.parse(), m.parse(), d.parse()) {
            (Ok(y), Ok(m), Ok(d)) => crate::dates::local_date(
                date(y, m, d),
                &[
                    ("year", "numeric"),
                    ("month", "long"),
                    ("day", "numeric"),
                    ("timeZone", "UTC"),
                ],
            ),
            _ => value.into(),
        },
        ["", "", m, d] => match (m.parse(), d.parse()) {
            (Ok(m), Ok(d)) => crate::dates::local_date(
                date(2000, m, d),
                &[("month", "long"), ("day", "numeric"), ("timeZone", "UTC")],
            ),
            _ => value.into(),
        },
        _ => value.into(),
    }
}

fn parse_month_day(value: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = value.split('-').collect();
    match parts.as_slice() {
        [y, m, d] if y.len() == 4 => Some((m.parse().ok()?, d.parse().ok()?)),
        ["", "", m, d] => Some((m.parse().ok()?, d.parse().ok()?)),
        _ => None,
    }
}

/// Returns contacts with birthdays in the next 60 days, sorted soonest first.
pub fn upcoming_birthdays(items: &[Item]) -> Vec<UpcomingBirthday> {
    let now = js_sys::Date::new_0();
    let cur_year = now.get_full_year() as i32;
    let today_utc = js_sys::Date::utc(cur_year as f64, now.get_month() as f64)
        + (now.get_date() as f64 - 1.0) * 86_400_000.0;
    let mut out = Vec::new();
    for item in items {
        let bday = s(&item.data, "birthday");
        let Some((m, d)) = parse_month_day(&bday) else {
            continue;
        };
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            continue;
        }
        let target_this_year =
            js_sys::Date::utc(cur_year as f64, (m - 1) as f64) + (d as f64 - 1.0) * 86_400_000.0;
        let target = if target_this_year + 1.0 < today_utc {
            js_sys::Date::utc((cur_year + 1) as f64, (m - 1) as f64)
                + (d as f64 - 1.0) * 86_400_000.0
        } else {
            target_this_year
        };
        let days = ((target - today_utc) / 86_400_000.0).round().max(0.0) as u32;
        if days <= 60 {
            let label = match days {
                0 => "Today".into(),
                1 => "Tomorrow".into(),
                n => format!("In {n}d"),
            };
            out.push(UpcomingBirthday {
                collection_id: item.collection_id.clone(),
                href: item.href.clone(),
                name: display_name(&item.data, "firstLast"),
                days_until: days,
                label,
            });
        }
    }
    out.sort_by(|a, b| a.days_until.cmp(&b.days_until).then(a.name.cmp(&b.name)));
    out.truncate(4);
    out
}

pub fn contact_groups(data: &Value) -> Vec<String> {
    data["groups"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .filter(|g| !g.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub fn matches_filter(data: &Value, filter: ContactFilter) -> bool {
    match filter {
        ContactFilter::All => true,
        ContactFilter::Email => data["emails"].as_array().is_some_and(|a| !a.is_empty()),
        ContactFilter::Phone => data["phones"].as_array().is_some_and(|a| !a.is_empty()),
        ContactFilter::Birthday => !s(data, "birthday").is_empty(),
    }
}

pub fn group_counts(items: &[Item]) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    for item in items {
        for g in contact_groups(&item.data) {
            *map.entry(g).or_insert(0) += 1;
        }
    }
    map
}

pub fn book_counts(items: &[Item]) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    for item in items {
        *map.entry(item.collection_id.clone()).or_insert(0) += 1;
    }
    map
}

fn norm_phone(v: &str) -> String {
    v.chars().filter(char::is_ascii_digit).collect()
}

fn norm_name(data: &Value) -> String {
    display_name(data, "firstLast")
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Detects likely duplicate contact pairs by shared email, normalized phone, or exact name.
pub fn find_duplicates(items: &[Item], dismissed: &HashSet<String>) -> Vec<DuplicatePair> {
    let mut pairs = Vec::new();
    let mut seen_keys = HashSet::new();
    let cap = items.len().min(400);
    for i in 0..cap {
        let a = &items[i];
        let a_emails: HashSet<String> = a.data["emails"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["value"].as_str())
            .map(|e| e.trim().to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        let a_phones: HashSet<String> = a.data["phones"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["value"].as_str())
            .map(norm_phone)
            .filter(|p| p.len() >= 7)
            .collect();
        let a_name = norm_name(&a.data);
        for b in items.iter().take(cap).skip(i + 1) {
            let reason = if b.data["emails"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v["value"].as_str())
                .any(|e| a_emails.contains(&e.trim().to_lowercase()))
            {
                Some("Same email address")
            } else if b.data["phones"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v["value"].as_str())
                .map(norm_phone)
                .any(|p| p.len() >= 7 && a_phones.contains(&p))
            {
                Some("Same phone number")
            } else if !a_name.is_empty() && a_name == norm_name(&b.data) {
                Some("Same name")
            } else {
                None
            };
            if let Some(reason) = reason {
                let (k1, k2) = if a.id <= b.id {
                    (&a.id, &b.id)
                } else {
                    (&b.id, &a.id)
                };
                let key = format!("{k1}::{k2}");
                if !dismissed.contains(&key) && seen_keys.insert(key.clone()) {
                    pairs.push(DuplicatePair {
                        key,
                        reason: reason.to_owned(),
                        primary: a.clone(),
                        secondary: b.clone(),
                    });
                }
            }
        }
    }
    pairs
}

/// Combines two contact projections into one, preserving all distinct values from both.
pub fn merge_contact_data(primary: &Value, secondary: &Value) -> Value {
    let mut out = primary.clone();
    for field in [
        "displayName",
        "firstName",
        "lastName",
        "middleName",
        "prefix",
        "suffix",
        "nickname",
        "pronouns",
        "organization",
        "department",
        "title",
        "birthday",
        "anniversary",
    ] {
        if s(&out, field).trim().is_empty() && !s(secondary, field).trim().is_empty() {
            out[field] = json!(s(secondary, field));
        }
    }
    // Concatenate distinct notes.
    let n1 = s(&out, "notes");
    let n2 = s(secondary, "notes");
    if n1.trim().is_empty() {
        out["notes"] = json!(n2);
    } else if !n2.trim().is_empty() && !n1.contains(n2.trim()) {
        out["notes"] = json!(format!("{}\n\n{}", n1.trim_end(), n2.trim()));
    }
    // Merge typed arrays (emails, phones, urls) deduplicating by normalized value.
    for key in ["emails", "phones", "urls"] {
        let mut merged = out[key].as_array().cloned().unwrap_or_default();
        let mut seen: HashSet<String> = merged
            .iter()
            .filter_map(|v| v["value"].as_str())
            .map(|v| {
                if key == "phones" {
                    norm_phone(v)
                } else {
                    v.trim().to_lowercase()
                }
            })
            .collect();
        for item in secondary[key].as_array().into_iter().flatten() {
            let raw = item["value"].as_str().unwrap_or_default();
            let norm = if key == "phones" {
                norm_phone(raw)
            } else {
                raw.trim().to_lowercase()
            };
            if !norm.is_empty() && seen.insert(norm) {
                merged.push(item.clone());
            }
        }
        out[key] = Value::Array(merged);
    }
    // Merge socials & related.
    for (key, kind_k) in [("socials", "service"), ("related", "relation")] {
        let mut merged = out[key].as_array().cloned().unwrap_or_default();
        let mut seen: HashSet<(String, String)> = merged
            .iter()
            .map(|v| {
                (
                    v[kind_k].as_str().unwrap_or_default().to_lowercase(),
                    v["value"]
                        .as_str()
                        .unwrap_or_default()
                        .trim()
                        .to_lowercase(),
                )
            })
            .collect();
        for item in secondary[key].as_array().into_iter().flatten() {
            let tup = (
                item[kind_k].as_str().unwrap_or_default().to_lowercase(),
                item["value"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_lowercase(),
            );
            if !tup.1.is_empty() && seen.insert(tup) {
                merged.push(item.clone());
            }
        }
        out[key] = Value::Array(merged);
    }
    // Merge addresses.
    let mut addrs = out["addresses"].as_array().cloned().unwrap_or_default();
    for a in secondary["addresses"].as_array().into_iter().flatten() {
        if !addrs.contains(a) {
            addrs.push(a.clone());
        }
    }
    out["addresses"] = Value::Array(addrs);
    // Merge groups.
    let mut groups = contact_groups(&out);
    for g in contact_groups(secondary) {
        if !groups.contains(&g) {
            groups.push(g);
        }
    }
    out["groups"] = json!(groups);
    if secondary["favorite"] == json!(true) {
        out["favorite"] = json!(true);
    }
    out
}

/// Builds a compact vCard 3.0 payload for QR code sharing (omitting bulky notes/photo).
pub fn vcard_qr_payload(data: &Value) -> String {
    let esc = |v: &str| {
        v.replace('\\', "\\\\")
            .replace(';', "\\;")
            .replace(',', "\\,")
            .replace('\n', "\\n")
    };
    let name = display_name(data, "firstLast");
    let mut lines = vec![
        "BEGIN:VCARD".to_owned(),
        "VERSION:3.0".to_owned(),
        format!(
            "N:{};{};{};{};{}",
            esc(&s(data, "lastName")),
            esc(&s(data, "firstName")),
            esc(&s(data, "middleName")),
            esc(&s(data, "prefix")),
            esc(&s(data, "suffix"))
        ),
        format!("FN:{}", esc(&name)),
    ];
    let org = s(data, "organization");
    if !org.is_empty() {
        lines.push(format!("ORG:{}", esc(&org)));
    }
    let title = s(data, "title");
    if !title.is_empty() {
        lines.push(format!("TITLE:{}", esc(&title)));
    }
    for e in data["emails"].as_array().into_iter().flatten().take(2) {
        if let Some(v) = e["value"].as_str().filter(|v| !v.is_empty()) {
            let l = e["label"].as_str().unwrap_or("HOME").to_uppercase();
            lines.push(format!("EMAIL;TYPE={l}:{}", esc(v)));
        }
    }
    for p in data["phones"].as_array().into_iter().flatten().take(2) {
        if let Some(v) = p["value"].as_str().filter(|v| !v.is_empty()) {
            let l = p["label"].as_str().unwrap_or("CELL").to_uppercase();
            lines.push(format!("TEL;TYPE={l}:{}", esc(v)));
        }
    }
    if let Some(u) = data["urls"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|u| u["value"].as_str())
        .filter(|v| !v.is_empty())
    {
        lines.push(format!("URL:{}", esc(u)));
    }
    lines.push("END:VCARD".to_owned());
    lines.join("\n")
}

pub fn search_params(pairs: &[(&str, String)]) -> String {
    let params = web_sys::UrlSearchParams::new().expect("params");
    for (k, v) in pairs {
        params.append(k, v);
    }
    params.to_string().as_string().unwrap_or_default()
}
