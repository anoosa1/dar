//! vCard contacts (`runtime/pim/contacts.ts`): the projection the apps show, and edits that
//! keep everything a client stored that the form does not manage.

use serde_json::{Map, Value, json};

use super::vobject::{Component, escape, field, invalid, list, parse, prop, remove, serialize, set_text, structured, text};
use crate::error::AppError;

pub struct Parsed {
    pub uid: String,
    pub content: String,
    pub data: Value,
}

fn label(p: &dar_dav::ical::Property, fallback: &str) -> String {
    p.param("TYPE").map(|t| t.replace(',', ", ")).unwrap_or_else(|| fallback.to_owned())
}

fn values(c: &Component, name: &str) -> Value {
    Value::Array(c.props_named(name).map(|p| json!({ "value": structured(&p.value).join(";"), "label": label(p, "other") })).collect())
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
    let names = c.prop("N").map(|p| structured(&p.value)).unwrap_or_default();
    let part = |i: usize| names.get(i).cloned().unwrap_or_default();
    let organization = c.prop("ORG").map(|p| structured(&p.value).into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")).unwrap_or_default();
    let addresses: Vec<Value> = c
        .props_named("ADR")
        .map(|p| {
            let a = structured(&p.value);
            let at = |i: usize| a.get(i).cloned().unwrap_or_default();
            json!({ "label": p.param("TYPE").unwrap_or("home"), "poBox": at(0), "extended": at(1), "street": at(2), "city": at(3), "region": at(4), "postalCode": at(5), "country": at(6) })
        })
        .collect();
    let groups: Vec<String> = c.props_named("CATEGORIES").flat_map(|p| list(&p.value)).collect();
    let data = json!({
        "displayName": text(&c, "FN"), "lastName": part(0), "firstName": part(1), "middleName": part(2), "prefix": part(3), "suffix": part(4),
        "organization": organization, "title": text(&c, "TITLE"), "notes": text(&c, "NOTE"), "birthday": text(&c, "BDAY"),
        "emails": values(&c, "EMAIL"), "phones": values(&c, "TEL"), "urls": values(&c, "URL"), "addresses": addresses,
        "groups": groups, "favorite": text(&c, "X-DAR-FAVORITE") == "1",
    });
    Ok(Parsed { uid: text(&c, "UID"), content: serialize(&c), data })
}

fn valid_birthday(value: &str) -> bool {
    let full = value.len() == 10 && value.as_bytes()[4] == b'-' && value.as_bytes()[7] == b'-';
    let partial = value.len() == 7 && value.starts_with("--") && value.as_bytes()[4] == b'-';
    if !full && !partial {
        return false;
    }
    let date = if partial { format!("2000{}", &value[1..]) } else { value.to_owned() };
    chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").is_ok()
}

fn label_ok(value: &str, spaces: bool) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || spaces && (c == ' ' || c == ','))
}

pub fn write_contact(input: &Value, previous: Option<&str>) -> Result<Parsed, AppError> {
    let p = input.as_object().ok_or_else(|| invalid("Invalid contact."))?;
    let previous_data = previous.map(read_contact).transpose()?;
    let get = |name: &str| p.get(name).cloned().unwrap_or(json!(""));
    let mut data = Map::new();
    for name in ["displayName", "firstName", "lastName", "middleName", "prefix", "suffix", "organization", "title", "notes", "birthday"] {
        data.insert(name.into(), json!(field(&get(name), if name == "notes" { 16_384 } else { 1024 })?));
    }
    let s = |data: &Map<String, Value>, name: &str| data[name].as_str().unwrap_or_default().to_owned();
    let display = { let d = s(&data, "displayName"); if d.trim().is_empty() { format!("{} {}", s(&data, "firstName"), s(&data, "lastName")).trim().to_owned() } else { d.trim().to_owned() } };
    if display.is_empty() {
        return Err(invalid("A contact needs a name."));
    }
    data.insert("displayName".into(), json!(display));
    let birthday = s(&data, "birthday");
    let unchanged_birthday = previous_data.as_ref().is_some_and(|old| old.data["birthday"] == birthday.as_str());
    if !birthday.is_empty() && !unchanged_birthday && !valid_birthday(&birthday) {
        return Err(invalid("Birthday must be YYYY-MM-DD or --MM-DD."));
    }
    let uid = previous_data.as_ref().map(|d| d.uid.clone()).unwrap_or_else(crate::auth::uuid);
    let mut lines = vec!["BEGIN:VCARD".to_owned(), "VERSION:3.0".to_owned(), format!("UID:{}", escape(&uid)), format!("FN:{}", escape(&display))];
    lines.push(format!("N:{}", ["lastName", "firstName", "middleName", "prefix", "suffix"].iter().map(|k| escape(&s(&data, k))).collect::<Vec<_>>().join(";")));
    for (name, property) in [("emails", "EMAIL"), ("phones", "TEL"), ("urls", "URL")] {
        let items = p.get(name).cloned().unwrap_or(json!([]));
        let items = items.as_array().filter(|i| i.len() <= 30).ok_or_else(|| invalid("Too many contact values."))?;
        for item in items {
            let item = item.as_object().ok_or_else(|| invalid("Invalid contact value."))?;
            let value = field(item.get("value").unwrap_or(&Value::Null), 2048)?;
            let label = field(item.get("label").unwrap_or(&json!("other")), 50)?;
            if !label_ok(&label, true) {
                return Err(invalid("Invalid contact label."));
            }
            if !value.trim().is_empty() {
                let types: Vec<&str> = label.split([',', ' ']).filter(|t| !t.is_empty()).collect();
                lines.push(format!("{property};TYPE={}:{}", types.join(","), escape(&value)));
            }
        }
    }
    let addresses = p.get("addresses").cloned().unwrap_or(json!([]));
    let addresses = addresses.as_array().filter(|a| a.len() <= 20).ok_or_else(|| invalid("Too many addresses."))?;
    for address in addresses {
        let address = address.as_object().ok_or_else(|| invalid("Invalid postal address."))?;
        let label = field(address.get("label").unwrap_or(&json!("home")), 50)?;
        if !label_ok(&label, false) {
            return Err(invalid("Invalid address label."));
        }
        let mut parts = Vec::new();
        for key in ["poBox", "extended", "street", "city", "region", "postalCode", "country"] {
            parts.push(escape(&field(address.get(key).unwrap_or(&json!("")), 1024)?));
        }
        lines.push(format!("ADR;TYPE={label}:{}", parts.join(";")));
    }
    for (name, property) in [("organization", "ORG"), ("title", "TITLE"), ("notes", "NOTE"), ("birthday", "BDAY")] {
        let value = s(&data, name);
        if !value.is_empty() && !(name == "birthday" && unchanged_birthday) {
            lines.push(format!("{property}:{}", escape(&value)));
        }
    }
    let raw_groups = p.get("groups").cloned().unwrap_or(json!([]));
    let raw_groups = raw_groups.as_array().filter(|g| g.len() <= 50).ok_or_else(|| invalid("Too many groups."))?;
    let mut groups: Vec<String> = Vec::new();
    for group in raw_groups {
        let g = field(group, 100)?.trim().to_owned();
        if !g.is_empty() && !groups.contains(&g) {
            groups.push(g);
        }
    }
    if !groups.is_empty() {
        lines.push(format!("CATEGORIES:{}", groups.iter().map(|g| escape(g)).collect::<Vec<_>>().join(",")));
    }
    if p.get("favorite") == Some(&json!(true)) {
        lines.push("X-DAR-FAVORITE:1".into());
    }
    lines.push("END:VCARD".into());
    let updated = parse(&lines.join("\r\n"), "VCARD")?;
    let Some(previous) = previous else { return read_contact(&serialize(&updated)) };
    let old_data = previous_data.expect("previous").data;
    let mut old = parse(previous, "VCARD")?;
    let same_names = ["lastName", "firstName", "middleName", "prefix", "suffix"].iter().all(|k| data[*k] == old_data[*k]);
    for name in ["UID", "FN", "N", "EMAIL", "TEL", "URL", "ADR", "ORG", "TITLE", "NOTE", "BDAY", "CATEGORIES", "X-DAR-FAVORITE"] {
        if name == "BDAY" && unchanged_birthday || name == "ORG" && data["organization"] == old_data["organization"] || name == "N" && same_names {
            continue;
        }
        let previous_props: Vec<_> = old.props_named(name).cloned().collect();
        let position = old.props.iter().position(|p| p.name == name).unwrap_or(old.props.len());
        remove(&mut old, name);
        for (at, new) in (position.min(old.props.len())..).zip(updated.props_named(name)) {
            // Values untouched by an edit keep their parameters (PREF, PID, custom labels).
            let kept = previous_props.iter().find(|p| p.value == new.value && p.param("TYPE") == new.param("TYPE"));
            old.props.insert(at, kept.cloned().unwrap_or_else(|| new.clone()));
        }
    }
    let _ = prop;
    read_contact(&serialize(&old))
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
        assert_eq!((made.data["favorite"].as_bool(), made.data["birthday"].as_str()), (Some(true), Some("--04-02")));
        // A client's extra property and an email's PREF survive an edit elsewhere.
        let stored = made.content.replace("END:VCARD", "X-CLIENT:kept\r\nEND:VCARD").replace("EMAIL;TYPE=home", "EMAIL;TYPE=home;PREF=1");
        let edited = write_contact(&json!({ "firstName": "Amina", "lastName": "Rahman", "title": "Engineer", "emails": [{ "value": "amina@example.com", "label": "home" }] }), Some(&stored)).unwrap();
        assert!(edited.content.contains("X-CLIENT:kept") && edited.content.contains("PREF=1"), "{}", edited.content);
        assert_eq!((edited.uid, edited.data["title"].as_str()), (made.uid, Some("Engineer")));
        assert!(write_contact(&json!({ "birthday": "1990-02-30", "firstName": "x" }), None).is_err());
        assert!(write_contact(&json!({}), None).is_err());
    }
}
