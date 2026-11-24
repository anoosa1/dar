//! vCard contacts (`runtime/pim/contacts.ts`): the projection the apps show, and edits that
//! keep everything a client stored that the form does not manage.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Map, Value, json};

use super::vobject::{
    Component, escape, field, invalid, list, parse, prop, remove, serialize, set_text, structured,
    text, unescape,
};
use crate::error::AppError;

pub struct Parsed {
    pub uid: String,
    pub content: String,
    pub data: Value,
}

fn label(p: &dar_dav::ical::Property, fallback: &str) -> String {
    p.param("TYPE")
        .map(|t| t.replace(',', ", "))
        .unwrap_or_else(|| fallback.to_owned())
}

fn values(c: &Component, name: &str) -> Value {
    Value::Array(
        c.props_named(name)
            .map(|p| json!({ "value": structured(&p.value).join(";"), "label": label(p, "other") }))
            .collect(),
    )
}

fn extract_photo_from_component(c: &Component) -> Option<(String, Vec<u8>)> {
    let p = c.prop("PHOTO")?;
    let val = p.value.trim();
    if val.is_empty() {
        return None;
    }
    let (mime, b64_part) = if let Some(rest) = val.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',')?;
        let mime = meta.split(';').next()?.trim().to_ascii_lowercase();
        if !meta.to_ascii_lowercase().contains("base64") {
            return None;
        }
        (mime, data)
    } else {
        let enc = p.param("ENCODING").unwrap_or_default().to_ascii_uppercase();
        if !enc.is_empty() && enc != "B" && enc != "BASE64" {
            return None;
        }
        if val.starts_with("http://") || val.starts_with("https://") {
            return None;
        }
        let kind = p.param("TYPE").unwrap_or("JPEG").to_ascii_uppercase();
        let mime = if kind.contains("PNG") {
            "image/png"
        } else if kind.contains("WEBP") {
            "image/webp"
        } else if kind.contains("GIF") {
            "image/gif"
        } else {
            "image/jpeg"
        }
        .to_owned();
        (mime, val)
    };
    if !matches!(
        mime.as_str(),
        "image/jpeg" | "image/png" | "image/webp" | "image/gif"
    ) {
        return None;
    }
    let cleaned: String = b64_part
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect();
    let bytes = B64.decode(cleaned).ok()?;
    if bytes.is_empty() || bytes.len() > 200_000 {
        return None;
    }
    Some((mime, bytes))
}

pub fn extract_photo(content: &str) -> Option<(String, Vec<u8>)> {
    let c = parse(content, "VCARD").ok()?;
    extract_photo_from_component(&c)
}

pub fn read_contact(content: &str) -> Result<Parsed, AppError> {
    let mut c = parse(content, "VCARD")?;
    let version = text(&c, "VERSION");
    if version != "3.0" && version != "4.0" {
        return Err(invalid("Only vCard 3.0 and 4.0 are supported."));
    }
    if text(&c, "UID").is_empty() {
        set_text(&mut c, "UID", &crate::auth::uuid());
    }
    if text(&c, "FN").trim().is_empty() {
        return Err(invalid("A contact needs a display name."));
    }
    let names = c
        .prop("N")
        .map(|p| structured(&p.value))
        .unwrap_or_default();
    let part = |i: usize| names.get(i).cloned().unwrap_or_default();
    let organization = c
        .prop("ORG")
        .map(|p| {
            structured(&p.value)
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .unwrap_or_default();
    let addresses: Vec<Value> = c
        .props_named("ADR")
        .map(|p| {
            let a = structured(&p.value);
            let at = |i: usize| a.get(i).cloned().unwrap_or_default();
            json!({ "label": p.param("TYPE").unwrap_or("home"), "poBox": at(0), "extended": at(1), "street": at(2), "city": at(3), "region": at(4), "postalCode": at(5), "country": at(6) })
        })
        .collect();
    let groups: Vec<String> = c
        .props_named("CATEGORIES")
        .flat_map(|p| list(&p.value))
        .collect();
    let socials: Vec<Value> = c
        .props_named("X-SOCIALPROFILE")
        .map(|p| {
            json!({
                "service": p.param("TYPE").unwrap_or("other").to_ascii_lowercase(),
                "value": unescape(&p.value),
            })
        })
        .collect();
    let related: Vec<Value> = c
        .props_named("RELATED")
        .map(|p| {
            json!({
                "relation": p.param("TYPE").unwrap_or("other").to_ascii_lowercase(),
                "value": unescape(&p.value),
            })
        })
        .collect();
    let has_photo = extract_photo_from_component(&c).is_some();
    let data = json!({
        "displayName": text(&c, "FN"), "lastName": part(0), "firstName": part(1), "middleName": part(2), "prefix": part(3), "suffix": part(4),
        "nickname": text(&c, "NICKNAME"), "pronouns": text(&c, "X-DAR-PRONOUNS"),
        "organization": organization, "department": text(&c, "X-DAR-DEPARTMENT"), "title": text(&c, "TITLE"),
        "notes": text(&c, "NOTE"), "birthday": text(&c, "BDAY"), "anniversary": text(&c, "ANNIVERSARY"),
        "emails": values(&c, "EMAIL"), "phones": values(&c, "TEL"), "urls": values(&c, "URL"), "addresses": addresses,
        "socials": socials, "related": related,
        "groups": groups, "favorite": text(&c, "X-DAR-FAVORITE") == "1", "hasPhoto": has_photo,
    });
    Ok(Parsed {
        uid: text(&c, "UID"),
        content: serialize(&c),
        data,
    })
}

fn valid_birthday(value: &str) -> bool {
    let full = value.len() == 10 && value.as_bytes()[4] == b'-' && value.as_bytes()[7] == b'-';
    let partial = value.len() == 7 && value.starts_with("--") && value.as_bytes()[4] == b'-';
    if !full && !partial {
        return false;
    }
    let date = if partial {
        format!("2000{}", &value[1..])
    } else {
        value.to_owned()
    };
    chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").is_ok()
}

fn label_ok(value: &str, spaces: bool) -> bool {
    !value.is_empty()
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '_' || c == '-' || spaces && (c == ' ' || c == ',')
        })
}

pub fn write_contact(input: &Value, previous: Option<&str>) -> Result<Parsed, AppError> {
    let p = input
        .as_object()
        .ok_or_else(|| invalid("Invalid contact."))?;
    let previous_data = previous.map(read_contact).transpose()?;
    let get = |name: &str| p.get(name).cloned().unwrap_or(json!(""));
    let mut data = Map::new();
    for name in [
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
        "notes",
        "birthday",
        "anniversary",
    ] {
        data.insert(
            name.into(),
            json!(field(
                &get(name),
                if name == "notes" { 16_384 } else { 1024 }
            )?),
        );
    }
    let s =
        |data: &Map<String, Value>, name: &str| data[name].as_str().unwrap_or_default().to_owned();
    let display = {
        let d = s(&data, "displayName");
        if d.trim().is_empty() {
            format!("{} {}", s(&data, "firstName"), s(&data, "lastName"))
                .trim()
                .to_owned()
        } else {
            d.trim().to_owned()
        }
    };
    if display.is_empty() {
        return Err(invalid("A contact needs a name."));
    }
    data.insert("displayName".into(), json!(display));
    let birthday = s(&data, "birthday");
    let unchanged_birthday = previous_data
        .as_ref()
        .is_some_and(|old| old.data["birthday"] == birthday.as_str());
    if !birthday.is_empty() && !unchanged_birthday && !valid_birthday(&birthday) {
        return Err(invalid("Birthday must be YYYY-MM-DD or --MM-DD."));
    }
    let anniversary = s(&data, "anniversary");
    let unchanged_anniversary = previous_data
        .as_ref()
        .is_some_and(|old| old.data["anniversary"] == anniversary.as_str());
    if !anniversary.is_empty() && !unchanged_anniversary && !valid_birthday(&anniversary) {
        return Err(invalid("Anniversary must be YYYY-MM-DD or --MM-DD."));
    }
    let uid = previous_data
        .as_ref()
        .map(|d| d.uid.clone())
        .unwrap_or_else(crate::auth::uuid);
    let mut lines = vec![
        "BEGIN:VCARD".to_owned(),
        "VERSION:3.0".to_owned(),
        format!("UID:{}", escape(&uid)),
        format!("FN:{}", escape(&display)),
    ];
    lines.push(format!(
        "N:{}",
        ["lastName", "firstName", "middleName", "prefix", "suffix"]
            .iter()
            .map(|k| escape(&s(&data, k)))
            .collect::<Vec<_>>()
            .join(";")
    ));
    for (name, property) in [("emails", "EMAIL"), ("phones", "TEL"), ("urls", "URL")] {
        let items = p.get(name).cloned().unwrap_or(json!([]));
        let items = items
            .as_array()
            .filter(|i| i.len() <= 30)
            .ok_or_else(|| invalid("Too many contact values."))?;
        for item in items {
            let item = item
                .as_object()
                .ok_or_else(|| invalid("Invalid contact value."))?;
            let value = field(item.get("value").unwrap_or(&Value::Null), 2048)?;
            let label = field(item.get("label").unwrap_or(&json!("other")), 50)?;
            if !label_ok(&label, true) {
                return Err(invalid("Invalid contact label."));
            }
            if !value.trim().is_empty() {
                let types: Vec<&str> = label.split([',', ' ']).filter(|t| !t.is_empty()).collect();
                lines.push(format!(
                    "{property};TYPE={}:{}",
                    types.join(","),
                    escape(&value)
                ));
            }
        }
    }
    for (name, property, key_field) in [
        ("socials", "X-SOCIALPROFILE", "service"),
        ("related", "RELATED", "relation"),
    ] {
        let items = p.get(name).cloned().unwrap_or(json!([]));
        let items = items
            .as_array()
            .filter(|i| i.len() <= 20)
            .ok_or_else(|| invalid("Too many contact values."))?;
        for item in items {
            let item = item
                .as_object()
                .ok_or_else(|| invalid("Invalid contact value."))?;
            let value = field(item.get("value").unwrap_or(&Value::Null), 1024)?;
            let kind = field(item.get(key_field).unwrap_or(&json!("other")), 50)?;
            if !label_ok(&kind, false) {
                return Err(invalid("Invalid contact label."));
            }
            if !value.trim().is_empty() {
                lines.push(format!("{property};TYPE={kind}:{}", escape(value.trim())));
            }
        }
    }
    let addresses = p.get("addresses").cloned().unwrap_or(json!([]));
    let addresses = addresses
        .as_array()
        .filter(|a| a.len() <= 20)
        .ok_or_else(|| invalid("Too many addresses."))?;
    for address in addresses {
        let address = address
            .as_object()
            .ok_or_else(|| invalid("Invalid postal address."))?;
        let label = field(address.get("label").unwrap_or(&json!("home")), 50)?;
        if !label_ok(&label, false) {
            return Err(invalid("Invalid address label."));
        }
        let mut parts = Vec::new();
        for key in [
            "poBox",
            "extended",
            "street",
            "city",
            "region",
            "postalCode",
            "country",
        ] {
            parts.push(escape(&field(
                address.get(key).unwrap_or(&json!("")),
                1024,
            )?));
        }
        lines.push(format!("ADR;TYPE={label}:{}", parts.join(";")));
    }
    for (name, property) in [
        ("nickname", "NICKNAME"),
        ("pronouns", "X-DAR-PRONOUNS"),
        ("organization", "ORG"),
        ("department", "X-DAR-DEPARTMENT"),
        ("title", "TITLE"),
        ("notes", "NOTE"),
        ("birthday", "BDAY"),
        ("anniversary", "ANNIVERSARY"),
    ] {
        let value = s(&data, name);
        if !value.is_empty()
            && !(name == "birthday" && unchanged_birthday)
            && !(name == "anniversary" && unchanged_anniversary)
        {
            lines.push(format!("{property}:{}", escape(&value)));
        }
    }
    let raw_groups = p.get("groups").cloned().unwrap_or(json!([]));
    let raw_groups = raw_groups
        .as_array()
        .filter(|g| g.len() <= 50)
        .ok_or_else(|| invalid("Too many groups."))?;
    let mut groups: Vec<String> = Vec::new();
    for group in raw_groups {
        let g = field(group, 100)?.trim().to_owned();
        if !g.is_empty() && !groups.contains(&g) {
            groups.push(g);
        }
    }
    if !groups.is_empty() {
        lines.push(format!(
            "CATEGORIES:{}",
            groups
                .iter()
                .map(|g| escape(g))
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    if p.get("favorite") == Some(&json!(true)) {
        lines.push("X-DAR-FAVORITE:1".into());
    }
    let remove_photo = p.get("removePhoto") == Some(&json!(true));
    let new_photo = p
        .get("photoData")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());
    let photo_changed = remove_photo || new_photo.is_some();
    if let Some(raw_b64) = new_photo
        && !remove_photo
    {
        let mime = p
            .get("photoMime")
            .and_then(Value::as_str)
            .unwrap_or("image/jpeg")
            .to_ascii_lowercase();
        let vtype = match mime.as_str() {
            "image/jpeg" | "image/jpg" => "JPEG",
            "image/png" => "PNG",
            "image/webp" => "WEBP",
            _ => return Err(invalid("Photo must be JPEG, PNG, or WebP.")),
        };
        let cleaned: String = raw_b64
            .chars()
            .filter(|ch| !ch.is_ascii_whitespace())
            .collect();
        let bytes = B64
            .decode(&cleaned)
            .map_err(|_| invalid("Invalid contact photo encoding."))?;
        if bytes.is_empty() || bytes.len() > 180_000 {
            return Err(invalid("Contact photo must be 180 KiB or smaller."));
        }
        lines.push(format!(
            "PHOTO;ENCODING=b;TYPE={vtype}:{}",
            B64.encode(&bytes)
        ));
    }
    lines.push("END:VCARD".into());
    let updated = parse(&lines.join("\r\n"), "VCARD")?;
    let Some(previous) = previous else {
        return read_contact(&serialize(&updated));
    };
    let old_data = previous_data.expect("previous").data;
    let mut old = parse(previous, "VCARD")?;
    let same_names = ["lastName", "firstName", "middleName", "prefix", "suffix"]
        .iter()
        .all(|k| data[*k] == old_data[*k]);
    for name in [
        "UID",
        "FN",
        "N",
        "NICKNAME",
        "X-DAR-PRONOUNS",
        "EMAIL",
        "TEL",
        "URL",
        "X-SOCIALPROFILE",
        "RELATED",
        "ADR",
        "ORG",
        "X-DAR-DEPARTMENT",
        "TITLE",
        "NOTE",
        "BDAY",
        "ANNIVERSARY",
        "CATEGORIES",
        "X-DAR-FAVORITE",
        "PHOTO",
    ] {
        if name == "BDAY" && unchanged_birthday
            || name == "ANNIVERSARY" && unchanged_anniversary
            || name == "ORG" && data["organization"] == old_data["organization"]
            || name == "N" && same_names
            || name == "PHOTO" && !photo_changed
        {
            continue;
        }
        let previous_props: Vec<_> = old.props_named(name).cloned().collect();
        let position = old
            .props
            .iter()
            .position(|p| p.name == name)
            .unwrap_or(old.props.len());
        remove(&mut old, name);
        for (at, new) in (position.min(old.props.len())..).zip(updated.props_named(name)) {
            // Values untouched by an edit keep their parameters (PREF, PID, custom labels).
            let kept = previous_props
                .iter()
                .find(|p| p.value == new.value && p.param("TYPE") == new.param("TYPE"));
            old.props
                .insert(at, kept.cloned().unwrap_or_else(|| new.clone()));
        }
    }
    let _ = prop;
    read_contact(&serialize(&old))
}

/// Parses RFC-4180 CSV rows (supporting Google Contacts / standard exports) into vCard 3.0 cards.
pub fn csv_to_vcards(csv_text: &str) -> Result<Vec<String>, AppError> {
    if csv_text.len() > 2 * 1024 * 1024 {
        return Err(invalid("Import exceeds 2 MiB."));
    }
    let text = csv_text.trim_start_matches('\u{feff}');
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cell.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                cell.push(c);
            }
        } else {
            match c {
                '"' if cell.is_empty() => in_quotes = true,
                ',' => {
                    row.push(std::mem::take(&mut cell));
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    row.push(std::mem::take(&mut cell));
                    if row.iter().any(|v| !v.trim().is_empty()) {
                        rows.push(std::mem::take(&mut row));
                    } else {
                        row.clear();
                    }
                }
                '\n' => {
                    row.push(std::mem::take(&mut cell));
                    if row.iter().any(|v| !v.trim().is_empty()) {
                        rows.push(std::mem::take(&mut row));
                    } else {
                        row.clear();
                    }
                }
                _ => cell.push(c),
            }
        }
        if rows.len() > 1001 {
            return Err(invalid("Import exceeds 1,000 resources."));
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        if row.iter().any(|v| !v.trim().is_empty()) {
            rows.push(row);
        }
    }
    if rows.len() < 2 {
        return Err(invalid(
            "CSV file needs a header row and at least one contact.",
        ));
    }
    let headers: Vec<String> = rows[0]
        .iter()
        .map(|h| h.trim().to_ascii_lowercase())
        .collect();
    let mut cards = Vec::new();
    for r in rows.iter().skip(1) {
        let get_any = |names: &[&str]| -> String {
            for name in names {
                if let Some(idx) = headers.iter().position(|h| h == name)
                    && let Some(val) = r.get(idx)
                {
                    let trimmed = val.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_owned();
                    }
                }
            }
            String::new()
        };
        let first = get_any(&["first name", "given name", "firstname"]);
        let last = get_any(&["last name", "family name", "lastname"]);
        let middle = get_any(&["middle name", "additional name"]);
        let prefix = get_any(&["name prefix", "prefix"]);
        let suffix = get_any(&["name suffix", "suffix"]);
        let nickname = get_any(&["nickname"]);
        let display = get_any(&["display name", "name", "full name", "fn"]);
        let org = get_any(&[
            "organization name",
            "organization 1 - name",
            "company",
            "organization",
        ]);
        let dept = get_any(&[
            "organization department",
            "organization 1 - department",
            "department",
        ]);
        let title = get_any(&[
            "organization title",
            "organization 1 - title",
            "job title",
            "title",
        ]);
        let notes = get_any(&["notes", "note"]);
        let bday_raw = get_any(&["birthday"]);
        let birthday = if valid_birthday(&bday_raw) {
            bday_raw
        } else {
            String::new()
        };
        let mut emails: Vec<Value> = Vec::new();
        let mut phones: Vec<Value> = Vec::new();
        let mut urls: Vec<Value> = Vec::new();
        for (i, h) in headers.iter().enumerate() {
            let val = r.get(i).map(|v| v.trim()).unwrap_or_default();
            if val.is_empty() {
                continue;
            }
            let clean_label = |raw: &str, fallback: &str| -> String {
                let s: String = raw
                    .trim()
                    .trim_start_matches('*')
                    .trim()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                if s.is_empty() {
                    fallback.to_owned()
                } else {
                    s.to_ascii_lowercase()
                }
            };
            if h == "email"
                || h == "e-mail"
                || h == "email address"
                || (h.contains("mail") && h.ends_with("value"))
            {
                let lbl_key = h.replace("value", "label");
                let lbl = headers
                    .iter()
                    .position(|x| *x == lbl_key || *x == h.replace("value", "type"))
                    .and_then(|j| r.get(j))
                    .map(|s| clean_label(s, "home"))
                    .unwrap_or_else(|| "home".into());
                for part in val.split(":::") {
                    if !part.trim().is_empty() {
                        emails.push(json!({ "value": part.trim(), "label": lbl }));
                    }
                }
            } else if h == "phone"
                || h == "phone number"
                || h == "mobile"
                || (h.contains("phone") && h.ends_with("value"))
            {
                let lbl_key = h.replace("value", "label");
                let lbl = headers
                    .iter()
                    .position(|x| *x == lbl_key || *x == h.replace("value", "type"))
                    .and_then(|j| r.get(j))
                    .map(|s| clean_label(s, "mobile"))
                    .unwrap_or_else(|| "mobile".into());
                for part in val.split(":::") {
                    if !part.trim().is_empty() {
                        phones.push(json!({ "value": part.trim(), "label": lbl }));
                    }
                }
            } else if h == "website"
                || h == "url"
                || (h.contains("website") && h.ends_with("value"))
            {
                urls.push(json!({ "value": val, "label": "other" }));
            }
        }
        let raw_groups = get_any(&["labels", "group membership", "groups", "categories"]);
        let groups: Vec<String> = raw_groups
            .split([',', ';'])
            .flat_map(|p| p.split(":::"))
            .map(|g| g.trim().trim_start_matches("* ").trim().to_owned())
            .filter(|g| !g.is_empty() && !g.eq_ignore_ascii_case("myContacts"))
            .collect();
        let fallback_name = if !display.is_empty() {
            display
        } else if !first.is_empty() || !last.is_empty() {
            format!("{first} {last}").trim().to_owned()
        } else if !org.is_empty() {
            org.clone()
        } else if let Some(e) = emails.first().and_then(|e| e["value"].as_str()) {
            e.to_owned()
        } else {
            continue;
        };
        let parsed = write_contact(
            &json!({
                "displayName": fallback_name,
                "firstName": first,
                "lastName": last,
                "middleName": middle,
                "prefix": prefix,
                "suffix": suffix,
                "nickname": nickname,
                "organization": org,
                "department": dept,
                "title": title,
                "notes": notes,
                "birthday": birthday,
                "emails": emails,
                "phones": phones,
                "urls": urls,
                "groups": groups,
            }),
            None,
        )?;
        cards.push(parsed.content);
    }
    if cards.is_empty() {
        return Err(invalid("No valid contacts found in CSV."));
    }
    Ok(cards)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_reads_and_keeps_client_data() {
        let input = json!({ "firstName": "Amina", "lastName": "Rahman", "emails": [{ "value": "amina@example.com", "label": "home" }], "groups": ["Family", "Family", " "], "favorite": true, "birthday": "--04-02" });
        let made = write_contact(&input, None).unwrap();
        assert_eq!(made.data["displayName"], "Amina Rahman");
        assert_eq!(made.data["groups"], json!(["Family"]));
        assert_eq!(
            (
                made.data["favorite"].as_bool(),
                made.data["birthday"].as_str()
            ),
            (Some(true), Some("--04-02"))
        );
        // A client's extra property and an email's PREF survive an edit elsewhere.
        let stored = made
            .content
            .replace("END:VCARD", "X-CLIENT:kept\r\nEND:VCARD")
            .replace("EMAIL;TYPE=home", "EMAIL;TYPE=home;PREF=1");
        let edited = write_contact(&json!({ "firstName": "Amina", "lastName": "Rahman", "title": "Engineer", "emails": [{ "value": "amina@example.com", "label": "home" }] }), Some(&stored)).unwrap();
        assert!(
            edited.content.contains("X-CLIENT:kept") && edited.content.contains("PREF=1"),
            "{}",
            edited.content
        );
        assert_eq!(
            (edited.uid, edited.data["title"].as_str()),
            (made.uid, Some("Engineer"))
        );
        assert!(
            write_contact(&json!({ "birthday": "1990-02-30", "firstName": "x" }), None).is_err()
        );
        assert!(write_contact(&json!({}), None).is_err());
    }

    #[test]
    fn photo_extra_fields_and_csv_import() {
        let png = B64.encode(b"\x89PNG\r\n\x1a\nfake");
        let made = write_contact(
            &json!({
                "firstName": "Bilal",
                "lastName": "Haddad",
                "nickname": "B",
                "pronouns": "he/him",
                "department": "Design",
                "anniversary": "2020-05-14",
                "socials": [{ "service": "github", "value": "https://github.com/bilal" }],
                "related": [{ "relation": "manager", "value": "Amina Rahman" }],
                "photoData": png,
                "photoMime": "image/png",
            }),
            None,
        )
        .unwrap();
        assert_eq!(made.data["hasPhoto"], json!(true));
        assert_eq!(made.data["nickname"], "B");
        assert_eq!(made.data["pronouns"], "he/him");
        assert_eq!(made.data["department"], "Design");
        assert_eq!(made.data["anniversary"], "2020-05-14");
        assert_eq!(made.data["socials"][0]["service"], "github");
        assert_eq!(made.data["related"][0]["relation"], "manager");
        let (mime, bytes) = extract_photo(&made.content).unwrap();
        assert_eq!(mime, "image/png");
        assert!(bytes.starts_with(b"\x89PNG"));
        // Editing without photoData preserves PHOTO; removePhoto strips it.
        let kept = write_contact(
            &json!({ "firstName": "Bilal", "lastName": "Haddad", "title": "Lead" }),
            Some(&made.content),
        )
        .unwrap();
        assert_eq!(kept.data["hasPhoto"], json!(true));
        let cleared = write_contact(
            &json!({ "firstName": "Bilal", "lastName": "Haddad", "removePhoto": true }),
            Some(&kept.content),
        )
        .unwrap();
        assert_eq!(cleared.data["hasPhoto"], json!(false));

        let csv = "First Name,Last Name,E-mail 1 - Value,E-mail 1 - Label,Phone 1 - Value,Labels\nClara,Lindqvist,clara@example.se,Work,+46 70 555 0123,* myContacts ::: Friends\n";
        let vcards = csv_to_vcards(csv).unwrap();
        assert_eq!(vcards.len(), 1);
        let parsed = read_contact(&vcards[0]).unwrap();
        assert_eq!(parsed.data["displayName"], "Clara Lindqvist");
        assert_eq!(parsed.data["emails"][0]["value"], "clara@example.se");
        assert_eq!(parsed.data["groups"], json!(["Friends"]));
    }
}
