//! vCard and iCalendar text (`runtime/pim/standards.ts`): parsing comes from `dar-dav`;
//! this adds editing, escaping and serialization folded at 75 UTF-8 octets.

pub use dar_dav::ical::{Component, Property};

use crate::error::AppError;

pub fn invalid(message: &str) -> AppError {
    AppError::with("invalidInput", 400, message)
}

/// A checked text field: at most `max` UTF-16 units, no NUL, CRLF as LF (`field`).
pub fn field(value: &serde_json::Value, max: usize) -> Result<String, AppError> {
    match value.as_str() {
        Some(s) if s.encode_utf16().count() <= max && !s.contains('\0') => {
            Ok(s.replace("\r\n", "\n").replace('\r', "\n"))
        }
        _ => Err(invalid("Invalid text field.")),
    }
}

pub fn parse(text: &str, expected: &str) -> Result<Component, AppError> {
    dar_dav::ical::parse(text, expected).map_err(|e| invalid(&e))
}

/// TEXT escaping (`escapeText`).
pub fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace("\r\n", "\n")
        .replace('\n', "\\n")
        .replace(';', "\\;")
        .replace(',', "\\,")
}

/// One TEXT value, unescaped.
pub fn unescape(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Splits on unescaped `sep`, keeping escapes for the caller.
pub fn split(value: &str, sep: char) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut escaped = false;
    for c in value.chars() {
        if escaped {
            out.last_mut().unwrap().push('\\');
            out.last_mut().unwrap().push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == sep {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}

/// A structured value's components (N, ADR, ORG), unescaped.
pub fn structured(value: &str) -> Vec<String> {
    split(value, ';')
        .iter()
        .map(|part| unescape(part))
        .collect()
}

/// A list value (CATEGORIES), unescaped.
pub fn list(value: &str) -> Vec<String> {
    split(value, ',')
        .iter()
        .map(|part| unescape(part))
        .collect()
}

pub fn text(c: &Component, name: &str) -> String {
    c.prop(name).map(|p| unescape(&p.value)).unwrap_or_default()
}

pub fn prop(name: &str, params: &[(&str, &str)], value: &str) -> Property {
    Property {
        name: name.to_ascii_uppercase(),
        params: params
            .iter()
            .map(|(k, v)| (k.to_ascii_uppercase(), (*v).to_owned()))
            .collect(),
        value: value.to_owned(),
    }
}

/// Replaces every `name` property with one TEXT value (`updatePropertyWithValue`).
pub fn set_text(c: &mut Component, name: &str, value: &str) {
    set(c, prop(name, &[], &escape(value)));
}

/// Replaces every property with this name, keeping the first one's position.
pub fn set(c: &mut Component, property: Property) {
    match c.props.iter().position(|p| p.name == property.name) {
        Some(index) => {
            c.props[index] = property.clone();
            let name = property.name;
            let mut seen = false;
            c.props.retain(|p| {
                if p.name != name {
                    return true;
                }
                let keep = !seen;
                seen = true;
                keep
            });
        }
        None => c.props.push(property),
    }
}

pub fn remove(c: &mut Component, name: &str) {
    let name = name.to_ascii_uppercase();
    c.props.retain(|p| p.name != name);
}

fn param_value(value: &str) -> String {
    if value.contains([':', ';', ',']) && !value.contains('"') {
        format!("\"{value}\"")
    } else {
        value.to_owned()
    }
}

fn line(p: &Property) -> String {
    let mut out = p.name.clone();
    for (k, v) in &p.params {
        out.push(';');
        out.push_str(k);
        out.push('=');
        out.push_str(&param_value(v));
    }
    out.push(':');
    out.push_str(&p.value);
    out
}

fn write(c: &Component, lines: &mut Vec<String>) {
    lines.push(format!("BEGIN:{}", c.name));
    for p in &c.props {
        lines.push(line(p));
    }
    for child in &c.children {
        write(child, lines);
    }
    lines.push(format!("END:{}", c.name));
}

/// Serializes with CRLF line ends, folding by UTF-8 octets without splitting a scalar.
pub fn serialize(c: &Component) -> String {
    let mut lines = Vec::new();
    write(c, &mut lines);
    let mut out = String::new();
    for l in lines {
        let mut bytes = 0;
        for ch in l.chars() {
            let n = ch.len_utf8();
            if bytes + n > 75 {
                out.push_str("\r\n ");
                bytes = 1;
            }
            out.push(ch);
            bytes += n;
        }
        out.push_str("\r\n");
    }
    out
}

/// Splits an import into its resources (`resources`).
pub fn resources(text: &str, kind: &str) -> Result<Vec<String>, AppError> {
    if text.len() > 2 * 1024 * 1024 {
        return Err(invalid("Import exceeds 2 MiB."));
    }
    let (begin, end) = (
        format!("BEGIN:{}", kind.to_uppercase()),
        format!("END:{}", kind.to_uppercase()),
    );
    let mut result = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for raw in text.trim_start_matches('\u{feff}').split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.to_uppercase() == begin {
            if current.is_some() {
                return Err(invalid("Nested resources are not valid."));
            }
            current = Some(vec![line]);
        } else if line.to_uppercase() == end {
            let mut lines = current
                .take()
                .ok_or_else(|| invalid("Unexpected resource ending."))?;
            lines.push(line);
            result.push(lines.join("\r\n"));
        } else if let Some(lines) = current.as_mut() {
            lines.push(line);
        } else if !line.trim().is_empty() {
            return Err(invalid("Text outside a standards resource."));
        }
        if result.len() > 1000 {
            return Err(invalid("Import exceeds 1,000 resources."));
        }
    }
    if current.is_some() || result.is_empty() {
        return Err(invalid("Incomplete or empty import."));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_folds() {
        let c = parse(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:A\\, B\r\nN:Doe;Jane;;;\r\nEND:VCARD\r\n",
            "VCARD",
        )
        .unwrap();
        assert_eq!(text(&c, "FN"), "A, B");
        assert_eq!(
            structured(&c.prop("N").unwrap().value),
            ["Doe", "Jane", "", "", ""]
        );
        let long = format!("BEGIN:VCARD\r\nNOTE:{}\r\nEND:VCARD\r\n", "é".repeat(60));
        let out = serialize(&parse(&long, "VCARD").unwrap());
        assert!(out.split("\r\n").all(|l| l.len() <= 75), "{out}");
        assert_eq!(text(&parse(&out, "VCARD").unwrap(), "NOTE"), "é".repeat(60));
        assert_eq!(
            resources(
                "BEGIN:VCARD\nFN:a\nEND:VCARD\nBEGIN:VCARD\nFN:b\nEND:VCARD\n",
                "vcard"
            )
            .unwrap()
            .len(),
            2
        );
    }
}
