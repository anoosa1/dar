//! Query blocks (Logseq's queries, Obsidian's Bases in a code block): a ` ```query ` block
//! lists the notes that match, as a table or a list. One condition per line:
//!
//! ```text
//! tag: #book #2026        notes with every tag (nested tags count)
//! folder: Reading         notes under a folder
//! text: dune              title, file name or alias contains
//! where: status = reading property or field compared: = != > < >= <= contains
//! sort: rating desc       or `-rating`; default: recently edited first
//! show: author, rating    table columns after the title
//! view: list              table (default) or list
//! limit: 20               at most 200
//! ```
//!
//! Fields besides properties: `title`, `folder`, `path`, `tags`, `modified` (`YYYY-MM-DD`).
//!
//! A ` ```base ` block holds an Obsidian base (a `.base` file's YAML): top-level `filters`
//! (`and`/`or`/`not` lists of expressions such as `file.hasTag("book")`,
//! `file.inFolder("Reading")`, `status == "reading"`, `rating > 3`, `!x`, `a && b`, `a || b`)
//! and the first of its `views` (`type: table|list|cards`, `order`, `sort`, `limit`, own
//! `filters`), with `properties` display names as column headings. What does not map (formulas,
//! summaries, grouping, other functions) is left out and named in a notice under the results.

use std::cmp::Ordering;

use serde_json::Value;

use super::note_links::{NoteIndex, has_tag};
use super::notes::NotesFile;
use crate::markdown::QueryOutput;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
    Contains,
}

/// A base's filter: `and`/`or`/`not` trees of tag, folder and field conditions.
#[derive(Clone, Debug, PartialEq)]
enum Cond {
    /// Any of these tags (nested tags count).
    Tag(Vec<String>),
    Folder(String),
    Field(String, Op, String),
    /// The field has a value.
    Has(String),
    Not(Box<Cond>),
    Any(Vec<Cond>),
    All(Vec<Cond>),
}

#[derive(Debug, Default, PartialEq)]
struct Query {
    tags: Vec<String>,
    folder: Option<String>,
    text: Option<String>,
    filters: Vec<(String, Op, String)>,
    conds: Vec<Cond>,
    sort: Option<(String, bool)>,
    show: Vec<String>,
    /// Column headings for `show`, when they differ from the field names.
    headers: Vec<String>,
    list: bool,
    limit: usize,
    notice: Option<String>,
}

fn parse(source: &str) -> Result<Query, String> {
    let mut query = Query { limit: 50, ..Query::default() };
    for (number, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { return Err(format!("Line {}: write `name: value`.", number + 1)) };
        let value = value.trim();
        match key.trim().to_lowercase().as_str() {
            "tag" | "tags" => query.tags.extend(value.split([' ', ',']).map(|t| t.trim().trim_start_matches('#')).filter(|t| !t.is_empty()).map(str::to_owned)),
            "folder" => query.folder = Some(value.trim_matches('/').to_owned()),
            "text" => query.text = Some(value.to_lowercase()),
            "where" => {
                let ops = [(" contains ", Op::Contains), ("!=", Op::Ne), (">=", Op::Ge), ("<=", Op::Le), ("=", Op::Eq), (">", Op::Gt), ("<", Op::Lt)];
                let found = ops.iter().find_map(|(token, op)| value.split_once(token).map(|(f, v)| (f.trim().to_lowercase(), *op, v.trim().trim_matches('"').to_owned())));
                match found {
                    Some((field, op, wanted)) if !field.is_empty() => query.filters.push((field, op, wanted)),
                    _ => return Err(format!("Line {}: write `where: field = value`.", number + 1)),
                }
            }
            "sort" => {
                let (field, desc) = match value.strip_prefix('-') {
                    Some(f) => (f.trim(), true),
                    None => match value.rsplit_once(' ') {
                        Some((f, dir)) if dir.eq_ignore_ascii_case("desc") => (f.trim(), true),
                        Some((f, dir)) if dir.eq_ignore_ascii_case("asc") => (f.trim(), false),
                        _ => (value, false),
                    },
                };
                query.sort = Some((field.to_lowercase(), desc));
            }
            "show" => query.show = value.split(',').map(|f| f.trim().to_owned()).filter(|f| !f.is_empty()).collect(),
            "view" => query.list = value.eq_ignore_ascii_case("list"),
            "limit" => query.limit = value.parse::<usize>().map_err(|_| format!("Line {}: the limit is a number.", number + 1))?.clamp(1, 200),
            other => return Err(format!("Line {}: \u{201c}{other}\u{201d} is not a query condition.", number + 1)),
        }
    }
    Ok(query)
}

fn folder_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

/// A note's values for a field: built-in fields, else the property (each list item).
fn values(note: &NotesFile, field: &str) -> Vec<String> {
    match field {
        "title" => vec![note.title.clone()],
        "file.name" => vec![note.name.clone()],
        "file.basename" => vec![note.name.rsplit_once('.').map_or(note.name.as_str(), |(stem, _)| stem).to_owned()],
        "folder" => vec![folder_of(&note.path).to_owned()],
        "path" => vec![note.path.clone()],
        "tags" | "tag" => note.tags.clone(),
        "modified" => {
            let date = js_date(note.modified_at);
            vec![date]
        }
        _ => {
            let found = note.properties.iter().find(|(k, _)| k.to_lowercase() == field).map(|(_, v)| v);
            match found {
                Some(Value::Array(items)) => items.iter().map(|i| i.as_str().map(str::to_owned).unwrap_or_else(|| i.to_string())).collect(),
                Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
                Some(Value::String(_)) | None | Some(Value::Null) => Vec::new(),
                Some(other) => vec![other.to_string()],
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn js_date(ms: f64) -> String {
    let d = js_sys::Date::new(&ms.into());
    format!("{:04}-{:02}-{:02}", d.get_full_year(), d.get_month() + 1, d.get_date())
}

#[cfg(not(target_arch = "wasm32"))]
fn js_date(ms: f64) -> String {
    let days = (ms / 86_400_000.0).floor() as i64;
    // Civil date from days since 1970-01-01 (UTC), for native unit tests.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn compare(a: &str, b: &str) -> Ordering {
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => x.total_cmp(&y),
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

fn matches(note: &NotesFile, field: &str, op: Op, wanted: &str) -> bool {
    let found = values(note, field);
    match op {
        Op::Ne => !found.iter().any(|v| compare(v, wanted) == Ordering::Equal),
        Op::Contains => found.iter().any(|v| v.to_lowercase().contains(&wanted.to_lowercase())),
        _ => found.iter().any(|v| {
            let order = compare(v, wanted);
            match op {
                Op::Eq => order == Ordering::Equal,
                Op::Gt => order == Ordering::Greater,
                Op::Lt => order == Ordering::Less,
                Op::Ge => order != Ordering::Less,
                Op::Le => order != Ordering::Greater,
                Op::Ne | Op::Contains => unreachable!(),
            }
        }),
    }
}

fn in_folder(note: &NotesFile, folder: &str) -> bool {
    folder.is_empty() || note.path.to_lowercase().starts_with(&format!("{}/", folder.to_lowercase()))
}

fn eval(note: &NotesFile, cond: &Cond) -> bool {
    match cond {
        Cond::Tag(tags) => tags.iter().any(|t| has_tag(note, t)),
        Cond::Folder(folder) => in_folder(note, folder),
        Cond::Field(field, op, wanted) => matches(note, field, *op, wanted),
        Cond::Has(field) => !values(note, field).is_empty(),
        Cond::Not(inner) => !eval(note, inner),
        Cond::Any(list) => list.iter().any(|c| eval(note, c)),
        Cond::All(list) => list.iter().all(|c| eval(note, c)),
    }
}

/// Runs a query block (or a base) over the library; the note holding it is left out.
pub fn run(index: &NoteIndex, source_id: &str, source: &str) -> QueryOutput {
    let parsed = if is_base(source) { Ok(base::parse(source)) } else { parse(source) };
    let query = match parsed {
        Ok(q) => q,
        Err(error) => return QueryOutput { error: Some(error), ..QueryOutput::default() },
    };
    let mut hits: Vec<usize> = (0..index.notes.len())
        .filter(|&i| {
            let note = &index.notes[i];
            note.id != source_id
                && query.tags.iter().all(|t| has_tag(note, t))
                && query.folder.as_ref().is_none_or(|f| in_folder(note, f))
                && query.text.as_ref().is_none_or(|t| note.title.to_lowercase().contains(t) || note.name.to_lowercase().contains(t) || note.aliases.iter().any(|a| a.to_lowercase().contains(t)))
                && query.filters.iter().all(|(field, op, wanted)| matches(note, field, *op, wanted))
                && query.conds.iter().all(|c| eval(note, c))
        })
        .collect();
    match &query.sort {
        Some((field, desc)) => hits.sort_by(|&a, &b| {
            let (va, vb) = (values(&index.notes[a], field), values(&index.notes[b], field));
            // Notes without the field come last either way.
            let order = match (va.first(), vb.first()) {
                (Some(x), Some(y)) => {
                    let o = compare(x, y);
                    if *desc { o.reverse() } else { o }
                }
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            order.then_with(|| index.notes[a].title.to_lowercase().cmp(&index.notes[b].title.to_lowercase()))
        }),
        None => hits.sort_by(|&a, &b| index.notes[b].modified_at.total_cmp(&index.notes[a].modified_at)),
    }
    let total = hits.len();
    let rows = hits
        .into_iter()
        .take(query.limit)
        .map(|i| {
            let note = &index.notes[i];
            let cells = query.show.iter().map(|f| values(note, &f.to_lowercase()).join(", ")).collect();
            (index.vault.wiki_target(i), if note.title.is_empty() { "Untitled".to_owned() } else { note.title.clone() }, cells)
        })
        .collect();
    let columns = if query.headers.len() == query.show.len() { query.headers.clone() } else { query.show.clone() };
    QueryOutput { columns, rows, list: query.list, total, error: None, notice: query.notice.clone() }
}

/// A ` ```base ` block: Obsidian's base YAML rather than query lines.
fn is_base(source: &str) -> bool {
    source.lines().any(|line| !line.starts_with(char::is_whitespace) && ["filters:", "views:", "formulas:", "properties:", "summaries:"].iter().any(|key| line.starts_with(key)))
}

/// Obsidian Bases: the YAML subset its editor writes, mapped onto [`Query`].
mod base {
    use super::{Cond, Op, Query};

    #[derive(Clone, Debug, PartialEq)]
    pub(super) enum Yaml {
        Text(String),
        List(Vec<Yaml>),
        Map(Vec<(String, Yaml)>),
    }

    impl Yaml {
        fn get(&self, key: &str) -> Option<&Yaml> {
            match self {
                Yaml::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
                _ => None,
            }
        }
        fn text(&self) -> Option<&str> {
            match self {
                Yaml::Text(text) => Some(text.as_str()),
                _ => None,
            }
        }
        /// A list's items; a single value counts as a list of one.
        fn items(&self) -> Vec<&Yaml> {
            match self {
                Yaml::List(items) => items.iter().collect(),
                Yaml::Text(text) if text.is_empty() => Vec::new(),
                other => vec![other],
            }
        }
    }

    struct Line {
        indent: usize,
        text: String,
    }

    fn lines(source: &str) -> Vec<Line> {
        source
            .lines()
            .filter_map(|raw| {
                let text = raw.trim_end();
                let content = text.trim_start();
                (!content.is_empty() && !content.starts_with('#')).then(|| Line { indent: text.len() - content.len(), text: content.to_owned() })
            })
            .collect()
    }

    fn is_item(text: &str) -> bool {
        text == "-" || text.starts_with("- ")
    }

    /// `text` split at each `sep` outside quotes and brackets.
    fn split_top<'a>(text: &'a str, sep: &str) -> Vec<&'a str> {
        let mut parts = Vec::new();
        let (mut depth, mut quote, mut start) = (0i32, None::<char>, 0usize);
        for (i, c) in text.char_indices() {
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
                continue;
            }
            match c {
                '"' | '\'' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ if depth == 0 && i >= start && text[i..].starts_with(sep) => {
                    parts.push(&text[start..i]);
                    start = i + sep.len();
                }
                _ => {}
            }
        }
        parts.push(&text[start..]);
        parts
    }

    /// The `:` ending a mapping key: outside quotes, followed by a space or the line's end.
    fn key_colon(text: &str) -> Option<usize> {
        let mut quote: Option<char> = None;
        let mut previous = ' ';
        for (i, c) in text.char_indices() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None if c == '"' || (c == '\'' && (previous.is_whitespace() || "([,".contains(previous))) => quote = Some(c),
                None if c == ':' && text[i + 1..].chars().next().is_none_or(|n| n == ' ') => return Some(i),
                None => {}
            }
            previous = c;
        }
        None
    }

    fn scalar(text: &str) -> String {
        let t = text.trim();
        if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
            return t[1..t.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\");
        }
        if t.len() >= 2 && t.starts_with('\'') && t.ends_with('\'') {
            return t[1..t.len() - 1].replace("''", "'");
        }
        t.to_owned()
    }

    fn value(text: &str) -> Yaml {
        let t = text.trim();
        match t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            Some(inner) => Yaml::List(split_top(inner, ",").into_iter().map(str::trim).filter(|s| !s.is_empty()).map(|s| Yaml::Text(scalar(s))).collect()),
            None => Yaml::Text(scalar(t)),
        }
    }

    /// The block starting at `lines[*at]`, whose lines sit at `indent`.
    fn block(lines: &mut [Line], at: &mut usize, indent: usize) -> Yaml {
        let Some(first) = lines.get(*at) else { return Yaml::Text(String::new()) };
        let first_text = first.text.clone();
        if is_item(&first_text) {
            let mut items = Vec::new();
            while *at < lines.len() && lines[*at].indent == indent && is_item(&lines[*at].text) {
                let rest = lines[*at].text[1..].to_owned();
                let content = rest.trim_start();
                if content.is_empty() {
                    *at += 1;
                    match lines.get(*at).map(|l| l.indent) {
                        Some(child) if child > indent => items.push(block(lines, at, child)),
                        _ => items.push(Yaml::Text(String::new())),
                    }
                } else {
                    // The item's content continues as a block at its own column (`- key: value`).
                    let column = indent + 1 + (rest.len() - content.len());
                    lines[*at] = Line { indent: column, text: content.to_owned() };
                    items.push(block(lines, at, column));
                }
            }
            return Yaml::List(items);
        }
        if key_colon(&first_text).is_some() {
            let mut entries = Vec::new();
            while *at < lines.len() && lines[*at].indent == indent && !is_item(&lines[*at].text) {
                let text = lines[*at].text.clone();
                let Some(colon) = key_colon(&text) else { break };
                let rest = text[colon + 1..].trim();
                *at += 1;
                let child = if !rest.is_empty() {
                    value(rest)
                } else {
                    match lines.get(*at).map(|l| (l.indent, is_item(&l.text))) {
                        Some((child, _)) if child > indent => block(lines, at, child),
                        // YAML allows a list at its key's own indentation.
                        Some((child, true)) if child == indent => block(lines, at, indent),
                        _ => Yaml::Text(String::new()),
                    }
                };
                entries.push((scalar(&text[..colon]), child));
            }
            return Yaml::Map(entries);
        }
        *at += 1;
        value(&first_text)
    }

    pub(super) fn yaml(source: &str) -> (Yaml, bool) {
        let mut lines = lines(source);
        let Some(indent) = lines.first().map(|l| l.indent) else { return (Yaml::Map(Vec::new()), true) };
        let mut at = 0;
        let root = block(&mut lines, &mut at, indent);
        (root, at == lines.len())
    }

    /// A literal: a quoted string, a number or a boolean.
    fn literal(text: &str) -> Option<String> {
        let t = text.trim();
        let quoted = t.len() >= 2 && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')));
        if quoted {
            return Some(t[1..t.len() - 1].to_owned());
        }
        (t.parse::<f64>().is_ok() || t == "true" || t == "false").then(|| t.to_owned())
    }

    /// A base field as a query field: `file.*` built-ins, `note.x` or `x` properties.
    fn field(name: &str) -> Option<String> {
        let n = name.trim();
        if let Some(f) = n.strip_prefix("file.") {
            let mapped = match f {
                "name" => "file.name",
                "basename" => "file.basename",
                "path" => "path",
                "folder" => "folder",
                "tags" => "tags",
                "mtime" => "modified",
                _ => return None,
            };
            return Some(mapped.to_owned());
        }
        let p = n.strip_prefix("note.").unwrap_or(n);
        let plain = !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-');
        (plain && !n.starts_with("formula.") && !n.starts_with("this.")).then(|| p.to_lowercase())
    }

    /// `(a)` → `a`, when the parentheses enclose the whole expression.
    fn strip_parens(text: &str) -> &str {
        let mut t = text.trim();
        while t.starts_with('(') && t.ends_with(')') {
            let mut depth = 0;
            let closes_at_end = t.char_indices().all(|(i, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                depth > 0 || i == t.len() - 1
            });
            if !closes_at_end {
                break;
            }
            t = t[1..t.len() - 1].trim();
        }
        t
    }

    fn find_top(text: &str, token: &str) -> Option<usize> {
        let parts = split_top(text, token);
        (parts.len() > 1).then(|| parts[0].len())
    }

    /// One filter expression; `None` (and a note in `skipped`) when it does not map.
    fn expr(source: &str, skipped: &mut Vec<String>) -> Option<Cond> {
        let s = strip_parens(source);
        for (sep, any) in [("||", true), ("&&", false)] {
            let parts = split_top(s, sep);
            if parts.len() > 1 {
                let conds: Vec<Cond> = parts.into_iter().filter_map(|p| expr(p, skipped)).collect();
                return (!conds.is_empty()).then(|| if any { Cond::Any(conds) } else { Cond::All(conds) });
            }
        }
        if let Some(rest) = s.strip_prefix('!')
            && !rest.starts_with('=')
        {
            return expr(rest, skipped).map(|c| Cond::Not(Box::new(c)));
        }
        if s.ends_with(')')
            && let Some(open) = s.find('(')
        {
            let (callee, inner) = (s[..open].trim(), &s[open + 1..s.len() - 1]);
            let args: Option<Vec<String>> = split_top(inner, ",").into_iter().filter(|a| !a.trim().is_empty()).map(literal).collect();
            let parsed = args.and_then(|args| match callee {
                "file.hasTag" if !args.is_empty() => Some(Cond::Tag(args.iter().map(|t| t.trim_start_matches('#').to_owned()).collect())),
                "file.inFolder" if args.len() == 1 => Some(Cond::Folder(args[0].trim_matches('/').to_owned())),
                "file.hasProperty" if args.len() == 1 => Some(Cond::Has(args[0].to_lowercase())),
                _ => {
                    let target = callee.strip_suffix(".contains").filter(|_| args.len() == 1)?;
                    field(target).map(|f| Cond::Field(f, Op::Contains, args[0].clone()))
                }
            });
            if parsed.is_none() {
                skipped.push(s.to_owned());
            }
            return parsed;
        }
        for (token, op) in [("==", Op::Eq), ("!=", Op::Ne), (">=", Op::Ge), ("<=", Op::Le), (">", Op::Gt), ("<", Op::Lt)] {
            if let Some(at) = find_top(s, token) {
                let (left, right) = (&s[..at], &s[at + token.len()..]);
                return match (field(left), literal(right)) {
                    (Some(f), Some(v)) => Some(Cond::Field(f, op, v)),
                    _ => {
                        skipped.push(s.to_owned());
                        None
                    }
                };
            }
        }
        // A bare property is true when it has a value.
        match field(s) {
            Some(f) => Some(Cond::Has(f)),
            None => {
                skipped.push(s.to_owned());
                None
            }
        }
    }

    /// `filters`: an expression, or `and`/`or`/`not` lists of them.
    pub(super) fn filter(value: &Yaml, skipped: &mut Vec<String>) -> Option<Cond> {
        match value {
            Yaml::Text(text) if text.trim().is_empty() => None,
            Yaml::Text(text) => expr(text, skipped),
            Yaml::List(items) => {
                let conds: Vec<Cond> = items.iter().filter_map(|i| filter(i, skipped)).collect();
                (!conds.is_empty()).then_some(Cond::All(conds))
            }
            Yaml::Map(entries) => {
                let mut conds = Vec::new();
                for (key, children) in entries {
                    if !matches!(key.as_str(), "and" | "or" | "not") {
                        skipped.push(key.clone());
                        continue;
                    }
                    let list: Vec<Cond> = children.items().into_iter().filter_map(|i| filter(i, skipped)).collect();
                    if list.is_empty() {
                        continue;
                    }
                    conds.push(match key.as_str() {
                        "and" => Cond::All(list),
                        "or" => Cond::Any(list),
                        // `not`: none of them.
                        _ => Cond::Not(Box::new(Cond::Any(list))),
                    });
                }
                match conds.len() {
                    0 => None,
                    1 => conds.pop(),
                    _ => Some(Cond::All(conds)),
                }
            }
        }
    }

    pub(super) fn parse(source: &str) -> Query {
        let mut query = Query { limit: 50, ..Query::default() };
        let mut skipped: Vec<String> = Vec::new();
        let (root, complete) = yaml(source);
        if !complete {
            skipped.push("lines that are not YAML".into());
        }
        let Yaml::Map(entries) = &root else {
            query.notice = Some("A base is YAML with filters and views.".into());
            return query;
        };
        let mut names: Vec<(String, String)> = Vec::new();
        for (key, value) in entries {
            match key.as_str() {
                "filters" => query.conds.extend(filter(value, &mut skipped)),
                "views" => {}
                "properties" => {
                    if let Yaml::Map(props) = value {
                        for (name, config) in props {
                            if let (Some(f), Some(shown)) = (field(name), config.get("displayName").and_then(Yaml::text)) {
                                names.push((f, shown.to_owned()));
                            }
                        }
                    }
                }
                other => skipped.push(other.to_owned()),
            }
        }
        let views = root.get("views").map(Yaml::items).unwrap_or_default();
        match views.len() {
            0 | 1 => {}
            2 => skipped.push("another view".into()),
            n => skipped.push(format!("{} other views", n - 1)),
        }
        if let Some(Yaml::Map(view)) = views.first() {
            for (key, value) in view {
                match key.as_str() {
                    "name" => {}
                    "type" => match value.text().unwrap_or_default() {
                        "table" => {}
                        "list" => query.list = true,
                        "cards" => skipped.push("cards (shown as a table)".into()),
                        other => skipped.push(format!("{other} views")),
                    },
                    "filters" => query.conds.extend(filter(value, &mut skipped)),
                    "order" => {
                        for item in value.items() {
                            let Some(name) = item.text() else { continue };
                            match field(name) {
                                // The first column always names the note.
                                Some(f) if f == "file.name" || f == "file.basename" => {}
                                Some(f) => {
                                    let heading = names.iter().find(|(n, _)| *n == f).map_or_else(|| name.strip_prefix("note.").unwrap_or(name).to_owned(), |(_, shown)| shown.clone());
                                    query.show.push(f);
                                    query.headers.push(heading);
                                }
                                None => skipped.push(name.to_owned()),
                            }
                        }
                    }
                    "sort" => {
                        let items = value.items();
                        for (n, item) in items.iter().enumerate() {
                            let (name, direction) = match item {
                                Yaml::Map(_) => (item.get("property").and_then(Yaml::text).unwrap_or_default(), item.get("direction").and_then(Yaml::text).unwrap_or("ASC")),
                                other => (other.text().unwrap_or_default(), "ASC"),
                            };
                            match field(name) {
                                Some(f) if n == 0 => query.sort = Some((f, direction.eq_ignore_ascii_case("desc"))),
                                _ => skipped.push(format!("sorting by {name}")),
                            }
                        }
                    }
                    "limit" => match value.text().and_then(|t| t.parse::<usize>().ok()) {
                        Some(limit) => query.limit = limit.clamp(1, 200),
                        None => skipped.push("limit".into()),
                    },
                    other => skipped.push(other.to_owned()),
                }
            }
        }
        let mut shown: Vec<String> = Vec::new();
        for item in skipped {
            let item: String = if item.chars().count() > 60 { format!("{}\u{2026}", item.chars().take(59).collect::<String>()) } else { item };
            if !shown.contains(&item) {
                shown.push(item);
            }
        }
        if !shown.is_empty() {
            query.notice = Some(format!("Not supported here, left out: {}.", shown.join(", ")));
        }
        query
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::notes::NotesLibrary;
    use serde_json::json;

    fn note(id: &str, path: &str, props: Value, tags: &[&str], modified: f64) -> NotesFile {
        let title = path.rsplit('/').next().unwrap().trim_end_matches(".md").to_owned();
        NotesFile {
            id: id.into(),
            parent_id: String::new(),
            name: format!("{title}.md"),
            title,
            excerpt: String::new(),
            size: 0.0,
            revision: 1,
            modified_at: modified,
            favorite: false,
            path: path.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            aliases: Vec::new(),
            properties: props.as_object().cloned().unwrap_or_default(),
        }
    }

    fn index() -> NoteIndex {
        let notes = vec![
            note("a", "Reading/Dune.md", json!({ "status": "reading", "rating": "5", "author": "Herbert" }), &["book"], 3.0),
            note("b", "Reading/Emma.md", json!({ "status": "done", "rating": "4" }), &["book/classic"], 2.0),
            note("c", "Reading/Notes on Dune.md", json!({}), &[], 1.0),
            note("d", "Ideas.md", json!({ "rating": "10" }), &["book"], 4.0),
        ];
        NoteIndex::new(Some(&NotesLibrary { root_id: "r".into(), path: "docs/notes".into(), folders: Vec::new(), notes, truncated: false, legacy: 0 }))
    }

    #[test]
    fn filters_sorts_and_shows() {
        let ix = index();
        let out = run(&ix, "x", "tag: #book\nsort: rating desc\nshow: status, rating");
        assert_eq!(out.rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), ["Ideas", "Dune", "Emma"]);
        assert_eq!(out.rows[1].2, ["reading", "5"]);
        assert_eq!(out.columns, ["status", "rating"]);
        let out = run(&ix, "x", "folder: Reading\nwhere: rating >= 4\nwhere: status != done");
        assert_eq!(out.rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["Dune"]);
        let out = run(&ix, "a", "text: dune\nview: list");
        assert!(out.list);
        assert_eq!(out.rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), ["Notes on Dune"]);
        assert_eq!(run(&ix, "x", "colour: red").error.as_deref(), Some("Line 1: \u{201c}colour\u{201d} is not a query condition."));
        assert_eq!(run(&ix, "x", "where: rating").error.as_deref(), Some("Line 1: write `where: field = value`."));
        assert_eq!(js_date(86_400_000.0 * 365.0), "1971-01-01");
    }

    fn titles(out: &QueryOutput) -> Vec<&str> {
        out.rows.iter().map(|r| r.1.as_str()).collect()
    }

    #[test]
    fn reads_base_yaml() {
        use super::base::{Yaml, yaml};
        let text = |t: &str| Yaml::Text(t.into());
        let (root, complete) = yaml("# comment\na:\n  - x\n  - k: v\n    j: [1, 'two']\nb: \"q: r\"\nc:\n- y\n");
        assert!(complete);
        assert_eq!(
            root,
            Yaml::Map(vec![
                ("a".into(), Yaml::List(vec![text("x"), Yaml::Map(vec![("k".into(), text("v")), ("j".into(), Yaml::List(vec![text("1"), text("two")]))])])),
                ("b".into(), text("q: r")),
                ("c".into(), Yaml::List(vec![text("y")])),
            ])
        );
        assert!(!yaml("a: 1\n  stray\n").1);
    }

    #[test]
    fn runs_bases() {
        let ix = index();
        let shelf = "filters:\n  and:\n    - file.hasTag(\"book\")\n    - 'file.inFolder(\"Reading\")'\nviews:\n  - type: table\n    name: Shelf\n    order:\n      - file.name\n      - status\n      - note.rating\n    sort:\n      - property: rating\n        direction: DESC\n    limit: 10\n";
        let out = run(&ix, "x", shelf);
        assert_eq!((out.error.as_deref(), out.notice.as_deref()), (None, None));
        assert_eq!(out.columns, ["status", "rating"]);
        assert_eq!(titles(&out), ["Dune", "Emma"]);
        assert_eq!(out.rows[0].2, ["reading", "5"]);

        let logic = "filters:\n  or:\n    - rating > 4\n    - status == \"done\"\n  not:\n    - file.inFolder(\"Reading\") && status == \"reading\"\nviews:\n- type: list\n";
        let out = run(&ix, "x", logic);
        assert!(out.list);
        assert_eq!(titles(&out), ["Ideas", "Emma"]);

        let partial = "formulas:\n  price: \"cost * 2\"\nfilters: 'file.hasLink(\"x\") || file.hasTag(\"book\")'\nproperties:\n  status:\n    displayName: State\nviews:\n  - type: cards\n    order: [file.name, status, formula.price]\n    groupBy: status\n  - type: table\n";
        let out = run(&ix, "x", partial);
        assert_eq!(out.columns, ["State"]);
        assert_eq!(titles(&out), ["Ideas", "Dune", "Emma"]);
        assert_eq!(out.notice.as_deref(), Some("Not supported here, left out: formulas, file.hasLink(\"x\"), another view, cards (shown as a table), formula.price, groupBy."));

        let out = run(&ix, "x", "filters: '!file.hasProperty(\"status\") && file.name.contains(\"dune\")'");
        assert_eq!(titles(&out), ["Notes on Dune"]);
        let out = run(&ix, "x", "filters: rating > now()");
        assert_eq!(out.total, 4);
        assert_eq!(out.notice.as_deref(), Some("Not supported here, left out: rating > now()."));
    }
}
