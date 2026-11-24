//! SCIM 2.0 resources (`runtime/identity/scim-model.ts`): one validator for SCIM and native
//! administration, resource rendering, bounded filters, PATCH and attribute projection.
//! Passwords never enter stored documents.

use serde_json::{Map, Value, json};

use crate::error::{AppError, Result, scim_error};

pub const USER: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
pub const GROUP: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
pub const ENTERPRISE: &str = "urn:ietf:params:scim:schemas:extension:enterprise:2.0:User";
pub const LIST: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";
pub const PATCH: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";

type Doc = Map<String, Value>;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Users,
    Groups,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Users => "Users",
            Kind::Groups => "Groups",
        }
    }
}

/// `String(value)` for ids and the like.
fn text_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

fn invalid(detail: &str) -> AppError {
    scim_error(400, detail, Some("invalidValue"))
}

fn object(value: &Value) -> Result<Doc> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| invalid("Expected an object."))
}

/// A text attribute: at most `max` UTF-8 bytes, no control characters but tab and newlines.
fn text(value: &Value, max: usize) -> Result<String> {
    match value.as_str() {
        Some(s)
            if s.len() <= max
                && !s
                    .chars()
                    .any(|c| matches!(c as u32, 0..=8 | 0x0b | 0x0c | 0x0e..=0x1f)) =>
        {
            Ok(s.to_owned())
        }
        _ => Err(invalid("Invalid text attribute.")),
    }
}

/// Attribute names matched without regard to case, each at most once.
fn canonical(source: &Doc, names: &[&str]) -> Result<Doc> {
    let mut result = Doc::new();
    for (key, value) in source {
        let Some(name) = names.iter().find(|n| n.eq_ignore_ascii_case(key)) else {
            let shown: String = key.chars().take(100).collect();
            return Err(invalid(&format!("Unsupported attribute: {shown}.")));
        };
        if result.contains_key(*name) {
            return Err(invalid("Duplicate attribute."));
        }
        result.insert((*name).to_owned(), value.clone());
    }
    Ok(result)
}

fn strings(source: &Value, names: &[&str]) -> Result<Value> {
    let value = canonical(&object(source)?, names)?;
    let mut out = Doc::new();
    for (k, v) in value {
        out.insert(k, json!(text(&v, 1024)?));
    }
    Ok(Value::Object(out))
}

const USER_ATTRIBUTES: [&str; 13] = [
    "userName",
    "externalId",
    "displayName",
    "name",
    "active",
    "emails",
    "password",
    "title",
    "userType",
    "locale",
    "timezone",
    "preferredLanguage",
    ENTERPRISE,
];
const GROUP_ATTRIBUTES: [&str; 3] = ["displayName", "externalId", "members"];
const READ_ONLY: [&str; 4] = ["id", "meta", "groups", "schemas"];

fn valid_user_name(name: &str) -> bool {
    let b = name.as_bytes();
    (3..=64).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// `encodeURIComponent`.
pub fn encode(value: &str) -> String {
    crate::git::encode_component(value)
}

/// One validator for SCIM and native administration (`normalize`).
pub fn normalize(kind: Kind, input: &Value) -> Result<(Doc, Option<String>)> {
    let names: Vec<&str> = match kind {
        Kind::Users => USER_ATTRIBUTES
            .iter()
            .chain(READ_ONLY.iter())
            .copied()
            .collect(),
        Kind::Groups => GROUP_ATTRIBUTES
            .iter()
            .chain(READ_ONLY.iter())
            .copied()
            .collect(),
    };
    let mut p = canonical(&object(input)?, &names)?;
    let own = if kind == Kind::Users { USER } else { GROUP };
    if let Some(schemas) = p.get("schemas") {
        let ok = schemas.as_array().is_some_and(|s| {
            s.iter().any(|v| v == own)
                && s.iter()
                    .all(|v| v == own || kind == Kind::Users && v == ENTERPRISE)
        });
        if !ok {
            return Err(invalid("Unsupported schemas."));
        }
    }
    for key in READ_ONLY {
        p.remove(key);
    }
    let password = match p.remove("password") {
        None => None,
        Some(v) => Some(text(&v, 1024)?),
    };
    for key in [
        "externalId",
        "displayName",
        "title",
        "userType",
        "locale",
        "timezone",
        "preferredLanguage",
    ] {
        if let Some(v) = p.get(key).cloned() {
            p.insert(
                key.into(),
                json!(text(&v, if key == "displayName" { 100 } else { 1024 })?),
            );
        }
    }
    if kind == Kind::Users {
        let user = p
            .get("userName")
            .and_then(Value::as_str)
            .filter(|n| valid_user_name(n))
            .ok_or_else(|| {
                invalid("userName must contain 3–64 letters, digits, dots, underscores or hyphens.")
            })?
            .to_lowercase();
        p.insert("userName".into(), json!(user));
        if !p.contains_key("displayName") {
            p.insert("displayName".into(), json!(user));
        }
        if p["displayName"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            return Err(invalid("displayName cannot be empty."));
        }
        match p.get("active") {
            None => {
                p.insert("active".into(), json!(true));
            }
            Some(Value::Bool(_)) => {}
            Some(_) => return Err(invalid("active must be a boolean.")),
        }
        if let Some(name) = p.get("name").cloned() {
            p.insert(
                "name".into(),
                strings(
                    &name,
                    &[
                        "formatted",
                        "familyName",
                        "givenName",
                        "middleName",
                        "honorificPrefix",
                        "honorificSuffix",
                    ],
                )?,
            );
        }
        if let Some(emails) = p.get("emails").cloned() {
            let list = emails
                .as_array()
                .filter(|l| l.len() <= 30)
                .ok_or_else(|| invalid("At most 30 emails are supported."))?;
            let mut primary = 0;
            let mut out = Vec::new();
            for v in list {
                let mut e = canonical(&object(v)?, &["value", "type", "primary", "display"])?;
                let value = text(e.get("value").unwrap_or(&Value::Null), 320)?;
                let ok = value.split_once('@').is_some_and(|(a, b)| {
                    !a.is_empty()
                        && !b.is_empty()
                        && !b.contains('@')
                        && !value.chars().any(char::is_whitespace)
                });
                if !ok {
                    return Err(invalid("Invalid email address."));
                }
                e.insert("value".into(), json!(value));
                if let Some(t) = e.get("type").cloned() {
                    e.insert("type".into(), json!(text(&t, 100)?));
                }
                if let Some(d) = e.get("display").cloned() {
                    e.insert("display".into(), json!(text(&d, 1024)?));
                }
                match e.get("primary") {
                    None => {}
                    Some(Value::Bool(b)) => {
                        if *b {
                            primary += 1;
                            if primary > 1 {
                                return Err(invalid("Only one email may be primary."));
                            }
                        }
                    }
                    Some(_) => return Err(invalid("primary must be a boolean.")),
                }
                out.push(Value::Object(e));
            }
            p.insert("emails".into(), Value::Array(out));
        }
        if let Some(zone) = p.get("timezone").and_then(Value::as_str)
            && zone.parse::<chrono_tz::Tz>().is_err()
            && zone != "UTC"
        {
            return Err(invalid("Invalid IANA timezone."));
        }
        if let Some(enterprise) = p.get(ENTERPRISE).cloned() {
            let mut e = canonical(
                &object(&enterprise)?,
                &[
                    "employeeNumber",
                    "costCenter",
                    "organization",
                    "division",
                    "department",
                    "manager",
                ],
            )?;
            for (k, v) in e.clone() {
                if k != "manager" {
                    e.insert(k, json!(text(&v, 1024)?));
                }
            }
            if let Some(manager) = e.get("manager").cloned() {
                let mut m = canonical(&object(&manager)?, &["value", "$ref", "displayName"])?;
                if let Some(v) = m.get("value").cloned() {
                    m.insert("value".into(), json!(text(&v, 100)?));
                }
                if let Some(r) = m.get("$ref").cloned() {
                    m.insert("$ref".into(), json!(text(&r, 2048)?));
                }
                m.remove("displayName");
                e.insert("manager".into(), Value::Object(m));
            }
            p.insert(ENTERPRISE.into(), Value::Object(e));
        }
    } else {
        if !p
            .get("displayName")
            .and_then(Value::as_str)
            .is_some_and(|d| !d.trim().is_empty())
        {
            return Err(invalid("displayName is required."));
        }
        let members = p.get("members").cloned().unwrap_or(json!([]));
        let list = members
            .as_array()
            .filter(|l| l.len() <= 2000)
            .ok_or_else(|| invalid("At most 2000 members are supported."))?;
        let mut out: Vec<String> = Vec::new();
        for v in list {
            let m = canonical(&object(v)?, &["value", "$ref", "display", "type"])?;
            let id = text(m.get("value").unwrap_or(&Value::Null), 100)?;
            if m.get("type")
                .is_some_and(|t| t.as_str().map(str::to_lowercase).as_deref() != Some("user"))
            {
                return Err(invalid("Nested groups are unsupported."));
            }
            if let Some(r) = m.get("$ref")
                && !r
                    .as_str()
                    .unwrap_or_default()
                    .ends_with(&format!("/Users/{}", encode(&id)))
            {
                return Err(invalid("Membership must reference a User."));
            }
            if !out.contains(&id) {
                out.push(id);
            }
        }
        p.insert(
            "members".into(),
            Value::Array(out.into_iter().map(|id| json!({ "value": id })).collect()),
        );
    }
    let size = Value::Object(p.clone()).to_string().len();
    if size
        > if kind == Kind::Users {
            16_384
        } else {
            256 * 1024
        }
    {
        return Err(invalid("The directory resource is too large."));
    }
    Ok((p, password))
}

pub fn version(row: &Value) -> String {
    format!("W/\"{}\"", row["revision"])
}

fn iso(ms: &Value) -> String {
    chrono::DateTime::from_timestamp_millis(ms.as_f64().unwrap_or(0.0) as i64)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// A directory record as SCIM shows it (`resource`).
pub fn resource(kind: Kind, row: &Value, base: &str) -> Value {
    let mut data = row.as_object().cloned().unwrap_or_default();
    for key in ["revision", "createdAt", "modifiedAt", "administrator"] {
        data.remove(key);
    }
    let id = row["id"].as_str().unwrap_or_default();
    let mut schemas = vec![json!(if kind == Kind::Users { USER } else { GROUP })];
    if data.get(ENTERPRISE).is_some_and(|e| !e.is_null()) {
        schemas.push(json!(ENTERPRISE));
    }
    data.insert("schemas".into(), Value::Array(schemas));
    data.insert(
        "meta".into(),
        json!({ "resourceType": if kind == Kind::Users { "User" } else { "Group" }, "created": iso(&row["createdAt"]), "lastModified": iso(&row["modifiedAt"]),
            "location": format!("{base}/{}/{}", kind.name(), encode(id)), "version": version(row) }),
    );
    if kind == Kind::Users {
        let groups: Vec<Value> = data
            .get("groups")
            .and_then(Value::as_array)
            .map(|g| {
                g.iter()
                    .map(|g| {
                        let mut g = g.clone();
                        g["$ref"] =
                            json!(format!("{base}/Groups/{}", encode(&text_of(&g["value"]))));
                        g["type"] = json!("direct");
                        g
                    })
                    .collect()
            })
            .unwrap_or_default();
        data.insert("groups".into(), Value::Array(groups));
    } else {
        let members: Vec<Value> = data
            .get("members")
            .and_then(Value::as_array)
            .map(|m| {
                m.iter()
                    .map(|m| {
                        let mut m = m.clone();
                        m["$ref"] =
                            json!(format!("{base}/Users/{}", encode(&text_of(&m["value"]))));
                        m["type"] = json!("User");
                        m
                    })
                    .collect()
            })
            .unwrap_or_default();
        data.insert("members".into(), Value::Array(members));
    }
    Value::Object(data)
}

fn property(value: &Value, key: &str) -> Option<Value> {
    value
        .as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
}

fn path_parts(path: &str) -> Vec<String> {
    if path.eq_ignore_ascii_case(ENTERPRISE) {
        return vec![ENTERPRISE.into()];
    }
    let prefix = format!("{}:", ENTERPRISE.to_lowercase());
    if path.to_lowercase().starts_with(&prefix) {
        let mut parts = vec![ENTERPRISE.to_owned()];
        parts.extend(path[prefix.len()..].split('.').map(str::to_owned));
        return parts;
    }
    path.split('.').map(str::to_owned).collect()
}

/// The values at an attribute path, through arrays (`values`).
pub fn values(value: &Value, path: &str) -> Vec<Value> {
    let flat = |list: Vec<Option<Value>>| -> Vec<Option<Value>> {
        list.into_iter()
            .flat_map(|v| match v {
                Some(Value::Array(a)) => a.into_iter().map(Some).collect::<Vec<_>>(),
                other => vec![other],
            })
            .collect()
    };
    let mut list = vec![Some(value.clone())];
    for part in path_parts(path) {
        list = flat(list)
            .into_iter()
            .map(|v| v.and_then(|v| property(&v, &part)))
            .collect();
    }
    flat(list).into_iter().flatten().collect()
}

pub type Predicate = Box<dyn Fn(&Value) -> bool + Send + Sync>;

fn tokens(source: &str) -> Vec<String> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '"' {
            let start = i;
            i += 1;
            let mut closed = false;
            while i < chars.len() {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    i += 1;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if closed {
                out.push(chars[start..i].iter().collect());
            } else {
                // An unclosed quote is part of a bare token, as the regular expression reads it.
                let mut j = start + 1;
                while j < chars.len() && !chars[j].is_whitespace() && !"()[]".contains(chars[j]) {
                    j += 1;
                }
                out.push(chars[start..j].iter().collect());
                i = j;
            }
        } else if "[]()".contains(c) {
            out.push(c.to_string());
            i += 1;
        } else {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() && !"()[]".contains(chars[i]) {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
        }
    }
    out
}

struct Parser {
    tokens: Vec<String>,
    i: usize,
    depth: usize,
}

fn bad_filter() -> AppError {
    scim_error(
        400,
        "Unsupported or malformed filter.",
        Some("invalidFilter"),
    )
}

impl Parser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.i).map(String::as_str)
    }
    fn next(&mut self) -> Option<String> {
        let t = self.tokens.get(self.i).cloned();
        self.i += 1;
        t
    }
    fn expression(&mut self) -> Result<Predicate> {
        self.depth += 1;
        if self.depth > 12 {
            return Err(bad_filter());
        }
        let mut left = self.conjunction()?;
        while self.peek().is_some_and(|t| t.eq_ignore_ascii_case("or")) {
            self.i += 1;
            let right = self.conjunction()?;
            let a = left;
            left = Box::new(move |v| a(v) || right(v));
        }
        self.depth -= 1;
        Ok(left)
    }
    fn conjunction(&mut self) -> Result<Predicate> {
        let mut left = self.atom()?;
        while self.peek().is_some_and(|t| t.eq_ignore_ascii_case("and")) {
            self.i += 1;
            let right = self.atom()?;
            let a = left;
            left = Box::new(move |v| a(v) && right(v));
        }
        Ok(left)
    }
    fn atom(&mut self) -> Result<Predicate> {
        if self.peek().is_some_and(|t| t.eq_ignore_ascii_case("not")) {
            self.i += 1;
            if self.next().as_deref() != Some("(") {
                return Err(bad_filter());
            }
            let f = self.expression()?;
            if self.next().as_deref() != Some(")") {
                return Err(bad_filter());
            }
            return Ok(Box::new(move |v| !f(v)));
        }
        if self.peek() == Some("(") {
            self.i += 1;
            let f = self.expression()?;
            if self.next().as_deref() != Some(")") {
                return Err(bad_filter());
            }
            return Ok(f);
        }
        let attr = self.next().ok_or_else(bad_filter)?;
        let lower = attr.to_lowercase();
        if attr.is_empty()
            || !attr
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_$:.-".contains(c))
            || ["__proto__", "constructor", "prototype"]
                .iter()
                .any(|s| attr.contains(s))
        {
            return Err(bad_filter());
        }
        if self.peek() == Some("[") {
            self.i += 1;
            let f = self.expression()?;
            if self.next().as_deref() != Some("]") {
                return Err(bad_filter());
            }
            return Ok(Box::new(move |v| values(v, &attr).iter().any(|x| f(x))));
        }
        let op = self.next().map(|o| o.to_lowercase()).unwrap_or_default();
        if op == "pr" {
            return Ok(Box::new(move |v| {
                values(v, &attr).iter().any(|x| !x.is_null() && x != "")
            }));
        }
        if !["eq", "ne", "co", "sw", "ew", "gt", "ge", "lt", "le"].contains(&op.as_str()) {
            return Err(bad_filter());
        }
        let target: Value =
            serde_json::from_str(&self.next().unwrap_or_default()).map_err(|_| bad_filter())?;
        if !(target.is_null() || target.is_string() || target.is_boolean() || target.is_number()) {
            return Err(bad_filter());
        }
        let exact = ["id", "members.value", "groups.value"].contains(&lower.as_str());
        let fold = move |x: &Value| match x {
            Value::String(s) if !exact => json!(s.to_lowercase()),
            other => other.clone(),
        };
        Ok(Box::new(move |value| {
            let list = values(value, &attr);
            let b = fold(&target);
            let matches = |raw: &Value| {
                let a = fold(raw);
                if op == "eq" || op == "ne" {
                    return a == b;
                }
                match (a.as_str(), b.as_str()) {
                    (Some(a), Some(b)) => match op.as_str() {
                        "co" => a.contains(b),
                        "sw" => a.starts_with(b),
                        "ew" => a.ends_with(b),
                        "gt" => a > b,
                        "ge" => a >= b,
                        "lt" => a < b,
                        _ => a <= b,
                    },
                    _ => match (a.as_f64(), b.as_f64()) {
                        // As in JavaScript, every other operator on numbers compares like `le`.
                        (Some(a), Some(b)) => match op.as_str() {
                            "gt" => a > b,
                            "ge" => a >= b,
                            "lt" => a < b,
                            _ => a <= b,
                        },
                        _ => false,
                    },
                }
            };
            if op == "ne" {
                !list.iter().any(matches)
            } else {
                list.iter().any(matches)
            }
        }))
    }
}

/// Bounded recursive descent SCIM filters, including value paths (`filterPredicate`).
pub fn filter_predicate(source: &str) -> Result<Predicate> {
    if source.len() > 2048 {
        return Err(scim_error(
            400,
            "Filter is too long.",
            Some("invalidFilter"),
        ));
    }
    let tokens = tokens(source);
    if tokens.is_empty() {
        return Err(bad_filter());
    }
    let mut parser = Parser {
        tokens,
        i: 0,
        depth: 0,
    };
    let result = parser.expression()?;
    if parser.i != parser.tokens.len() {
        return Err(bad_filter());
    }
    Ok(result)
}

fn actual_key(value: &Doc, key: &str) -> String {
    value
        .keys()
        .find(|k| k.eq_ignore_ascii_case(key))
        .cloned()
        .unwrap_or_else(|| key.to_owned())
}

fn unsafe_name(name: &str) -> bool {
    ["__proto__", "constructor", "prototype"].contains(&name.to_lowercase().as_str())
}

fn clear_primary(list: &mut [Value]) {
    for email in list.iter_mut() {
        if let Some(e) = email.as_object_mut() {
            e.remove("primary");
        }
    }
}

/// Walks to the object holding the last part, creating objects on the way unless removing.
fn parent_of<'a>(root: &'a mut Doc, parts: &[String], create: bool) -> Result<&'a mut Doc> {
    let mut current = root;
    for part in parts {
        let key = actual_key(current, part);
        if !current.contains_key(&key) && create {
            current.insert(key.clone(), json!({}));
        }
        current = current
            .get_mut(&key)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| invalid("Expected an object."))?;
    }
    Ok(current)
}

/// Applies a PatchOp document to a resource (`applyPatch`).
pub fn apply_patch(kind: Kind, current: &Value, input: &Value) -> Result<Value> {
    let p = canonical(&object(input)?, &["schemas", "Operations"])?;
    let schemas_ok = p
        .get("schemas")
        .and_then(Value::as_array)
        .is_some_and(|s| s.len() == 1 && s[0] == PATCH);
    let operations = p
        .get("Operations")
        .and_then(Value::as_array)
        .filter(|o| !o.is_empty() && o.len() <= 100);
    let (true, Some(operations)) = (schemas_ok, operations) else {
        return Err(invalid("Invalid PatchOp document."));
    };
    let mut result = current.as_object().cloned().unwrap_or_default();
    for key in ["schemas", "meta", "id", "groups"] {
        result.remove(key);
    }
    for entry in operations {
        let operation = canonical(&object(entry)?, &["op", "path", "value"])?;
        let op = text_of(operation.get("op").unwrap_or(&Value::Null)).to_lowercase();
        if !["add", "replace", "remove"].contains(&op.as_str()) {
            return Err(invalid("Unsupported PATCH operation."));
        }
        let value = operation.get("value").cloned().unwrap_or(Value::Null);
        let Some(path) = operation.get("path") else {
            if op == "remove" {
                return Err(scim_error(400, "remove needs a path.", Some("noTarget")));
            }
            for (k, v) in object(&value)? {
                if READ_ONLY.iter().any(|r| r.eq_ignore_ascii_case(&k)) {
                    return Err(scim_error(
                        400,
                        "Attribute is read-only.",
                        Some("mutability"),
                    ));
                }
                if unsafe_name(&k) {
                    return Err(invalid("Invalid PATCH attribute."));
                }
                let key = actual_key(&result, &k);
                match (op.as_str(), result.get_mut(&key), &v) {
                    ("add", Some(Value::Array(existing)), Value::Array(more)) => {
                        if key.eq_ignore_ascii_case("emails")
                            && more
                                .iter()
                                .any(|e| property(e, "primary") == Some(json!(true)))
                        {
                            clear_primary(existing);
                        }
                        existing.extend(more.iter().cloned());
                    }
                    ("add", Some(Value::Object(existing)), Value::Object(more)) => {
                        for (mk, mv) in more {
                            existing.insert(mk.clone(), mv.clone());
                        }
                    }
                    _ => {
                        result.insert(key, v);
                    }
                }
            }
            continue;
        };
        let path = text(path, 512)?;
        let invalid_path = || scim_error(400, "Unsupported PATCH path.", Some("invalidPath"));
        let (attribute, filter, sub) = match path.find('[') {
            Some(open) => {
                let close = path
                    .rfind(']')
                    .filter(|c| *c > open)
                    .ok_or_else(invalid_path)?;
                let after = &path[close + 1..];
                let sub = match after.strip_prefix('.') {
                    Some(s)
                        if !s.is_empty()
                            && s.chars()
                                .all(|c| c.is_alphanumeric() || c == '_' || c == '$') =>
                    {
                        Some(s.to_owned())
                    }
                    None if after.is_empty() => None,
                    _ => return Err(invalid_path()),
                };
                (
                    path[..open].to_owned(),
                    Some(path[open + 1..close].to_owned()),
                    sub,
                )
            }
            None => (path.clone(), None, None),
        };
        if attribute.is_empty() || attribute.contains(']') || filter.as_deref() == Some("") {
            return Err(invalid_path());
        }
        let lowered = path.to_lowercase();
        if ["constructor", "prototype", "__proto__"]
            .iter()
            .any(|w| lowered.split(['.', ':', '[']).any(|segment| segment == *w))
        {
            return Err(invalid_path());
        }
        let parts = path_parts(&attribute);
        let root = parts[0].to_lowercase();
        if ["id", "meta", "groups", "schemas"].contains(&root.as_str()) {
            return Err(scim_error(
                400,
                "Attribute is read-only.",
                Some("mutability"),
            ));
        }
        let allowed: &[&str] = if kind == Kind::Users {
            &USER_ATTRIBUTES
        } else {
            &GROUP_ATTRIBUTES
        };
        if !allowed.iter().any(|a| a.to_lowercase() == root) {
            return Err(invalid_path());
        }
        let parent = parent_of(&mut result, &parts[..parts.len() - 1], op != "remove")?;
        let key = actual_key(parent, &parts[parts.len() - 1]);
        if let Some(filter) = filter {
            let f = filter_predicate(&filter)?;
            let Some(array) = parent.get(&key).and_then(Value::as_array).cloned() else {
                return Err(scim_error(
                    400,
                    "The target is not multivalued.",
                    Some("invalidPath"),
                ));
            };
            if !array.iter().any(|x| f(x)) {
                return Err(scim_error(400, "No matching attribute.", Some("noTarget")));
            }
            let mut array = array;
            let primary_value = sub
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("primary"))
                && value == json!(true)
                || property(&value, "primary") == Some(json!(true));
            if key.eq_ignore_ascii_case("emails") && op != "remove" && primary_value {
                for email in array.iter_mut() {
                    if !f(email)
                        && let Some(e) = email.as_object_mut()
                    {
                        e.remove("primary");
                    }
                }
            }
            let mut next = Vec::new();
            for v in array {
                if !f(&v) {
                    next.push(v);
                    continue;
                }
                if let Some(sub) = &sub {
                    let mut copy = object(&v)?;
                    let k = actual_key(&copy, sub);
                    if op == "remove" {
                        copy.remove(&k);
                    } else {
                        copy.insert(k, value.clone());
                    }
                    next.push(Value::Object(copy));
                } else if op == "remove" {
                } else if op == "add" {
                    let mut merged = object(&v)?;
                    for (mk, mv) in object(&value)? {
                        merged.insert(mk, mv);
                    }
                    next.push(Value::Object(merged));
                } else {
                    match &value {
                        Value::Array(a) => next.extend(a.iter().cloned()),
                        other => next.push(other.clone()),
                    }
                }
            }
            parent.insert(key, Value::Array(next));
        } else if op == "remove" {
            if parent.remove(&key).is_none() {
                return Err(scim_error(400, "No matching attribute.", Some("noTarget")));
            }
        } else if let (true, Some(Value::Array(existing))) = (op == "add", parent.get_mut(&key)) {
            let entries = match &value {
                Value::Array(a) => a.clone(),
                other => vec![other.clone()],
            };
            if key.eq_ignore_ascii_case("emails")
                && entries
                    .iter()
                    .any(|e| property(e, "primary") == Some(json!(true)))
            {
                clear_primary(existing);
            }
            existing.extend(entries);
        } else if let (true, Some(Value::Object(existing)), Value::Object(more)) =
            (op == "add", parent.get_mut(&key), &value)
        {
            for (mk, mv) in more {
                existing.insert(mk.clone(), mv.clone());
            }
        } else {
            parent.insert(key, value);
        }
    }
    Ok(Value::Object(result))
}

/// `attributes` / `excludedAttributes` (`project`).
pub fn project(document: &Value, include: Option<&str>, exclude: Option<&str>) -> Result<Value> {
    if include.is_some() && exclude.is_some() {
        return Err(invalid(
            "attributes and excludedAttributes cannot be combined.",
        ));
    }
    let paths: Vec<&str> = include
        .or(exclude)
        .unwrap_or_default()
        .split(',')
        .filter(|p| !p.is_empty())
        .collect();
    if paths.len() > 100 {
        return Err(invalid("Too many attribute paths."));
    }
    fn pick(from: &Value, to: Option<Value>, parts: &[String]) -> Option<Value> {
        if let Value::Array(items) = from {
            let mut result = match to {
                Some(Value::Array(a)) => a,
                _ => Vec::new(),
            };
            for (i, v) in items.iter().enumerate() {
                let previous = result.get(i).cloned();
                let picked = pick(v, previous, parts).unwrap_or(Value::Null);
                if i < result.len() {
                    result[i] = picked;
                } else {
                    while result.len() < i {
                        result.push(Value::Null);
                    }
                    result.push(picked);
                }
            }
            return Some(Value::Array(result));
        }
        let Some(source) = from.as_object() else {
            return to;
        };
        let key = actual_key(source, &parts[0]);
        let Some(inner) = source.get(&key) else {
            return to;
        };
        let mut result = match to {
            Some(Value::Object(o)) => o,
            _ => Map::new(),
        };
        if parts.len() == 1 {
            result.insert(key, inner.clone());
        } else {
            let next = pick(inner, result.get(&key).cloned(), &parts[1..]);
            if let Some(n) = next {
                result.insert(key, n);
            }
        }
        Some(Value::Object(result))
    }
    fn omit(value: &mut Value, parts: &[String]) {
        match value {
            Value::Array(items) => items.iter_mut().for_each(|v| omit(v, parts)),
            Value::Object(object) => {
                let key = actual_key(object, &parts[0]);
                if parts.len() == 1 {
                    object.remove(&key);
                } else if let Some(inner) = object.get_mut(&key) {
                    omit(inner, &parts[1..]);
                }
            }
            _ => {}
        }
    }
    let mut output = if include.is_some() {
        json!({ "schemas": document["schemas"], "id": document["id"] })
    } else {
        document.clone()
    };
    for path in paths {
        let parts = path_parts(path);
        if parts.iter().any(|p| unsafe_name(p)) {
            return Err(invalid("Invalid attribute path."));
        }
        if include.is_some() {
            if let Some(picked) = pick(document, Some(output.clone()), &parts) {
                output = picked;
            }
        } else if !["id", "schemas"].contains(&parts[0].to_lowercase().as_str()) {
            omit(&mut output, &parts);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_patch_and_projection() {
        let user = json!({ "id": "U1", "userName": "amina", "active": true, "emails": [{ "value": "a@x.example", "type": "work", "primary": true }, { "value": "b@y.example", "type": "home" }] });
        assert!(filter_predicate("userName eq \"AMINA\"").unwrap()(&user));
        assert!(filter_predicate(
            "emails[type eq \"home\" and value co \"y.example\"]"
        )
        .unwrap()(&user));
        assert!(!filter_predicate("not (active eq true)").unwrap()(&user));
        assert!(filter_predicate("id eq \"u1\"").unwrap()(&user) == false);
        assert!(filter_predicate("userName eq").is_err());
        let patched = apply_patch(Kind::Users, &user, &json!({ "schemas": [PATCH], "Operations": [{ "op": "replace", "path": "emails[type eq \"home\"].primary", "value": true }, { "op": "remove", "path": "active" }] })).unwrap();
        assert_eq!(patched["emails"][1]["primary"], true);
        assert!(patched["emails"][0].get("primary").is_none());
        assert!(patched.get("active").is_none());
        let (doc, password) = normalize(
            Kind::Users,
            &json!({ "schemas": [USER], "userName": "Amina", "password": "x".repeat(20) }),
        )
        .unwrap();
        assert_eq!(
            (
                doc["userName"].as_str(),
                doc["displayName"].as_str(),
                password.map(|p| p.len())
            ),
            (Some("amina"), Some("amina"), Some(20))
        );
        let projected = project(&json!({ "schemas": [USER], "id": "U1", "name": { "givenName": "A", "familyName": "B" }, "userName": "a" }), Some("name.givenName"), None).unwrap();
        assert_eq!(
            projected,
            json!({ "schemas": [USER], "id": "U1", "name": { "givenName": "A" } })
        );
    }
}
