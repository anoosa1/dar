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

#[derive(Debug, Default, PartialEq)]
struct Query {
    tags: Vec<String>,
    folder: Option<String>,
    text: Option<String>,
    filters: Vec<(String, Op, String)>,
    sort: Option<(String, bool)>,
    show: Vec<String>,
    list: bool,
    limit: usize,
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

/// Runs a query block over the library; the note holding it is left out.
pub fn run(index: &NoteIndex, source_id: &str, source: &str) -> QueryOutput {
    let query = match parse(source) {
        Ok(q) => q,
        Err(error) => return QueryOutput { error: Some(error), ..QueryOutput::default() },
    };
    let mut hits: Vec<usize> = (0..index.notes.len())
        .filter(|&i| {
            let note = &index.notes[i];
            note.id != source_id
                && query.tags.iter().all(|t| has_tag(note, t))
                && query.folder.as_ref().is_none_or(|f| f.is_empty() || note.path.to_lowercase().starts_with(&format!("{}/", f.to_lowercase())))
                && query.text.as_ref().is_none_or(|t| note.title.to_lowercase().contains(t) || note.name.to_lowercase().contains(t) || note.aliases.iter().any(|a| a.to_lowercase().contains(t)))
                && query.filters.iter().all(|(field, op, wanted)| matches(note, field, *op, wanted))
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
    QueryOutput { columns: query.show.clone(), rows, list: query.list, total, error: None }
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
}
