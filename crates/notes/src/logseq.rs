//! Logseq graphs to Notes: a graph's `pages/` and `journals/` Markdown files become notes in
//! Obsidian's syntax. Namespaced pages (`a___b.md`, `a%2Fb.md`) become folders (`a/b.md`),
//! journals (`2026_11_23.md`) become `Journal/2026-11-23.md`, block references `((uuid))` become
//! `[[Page#^id]]` (the block with `id:: uuid` gets `^id`), first-block `key:: value` page
//! properties become YAML properties (`alias` as `aliases`), `TODO`/`DONE` markers become
//! checkboxes and `{{embed …}}` becomes `![[…]]`. Everything else is kept as written, except
//! Logseq's own `collapsed::` state and `:LOGBOOK:` drawers.

use std::collections::{HashMap, HashSet};

use crate::percent_decode;

/// A file picked from the graph: its path relative to the graph folder (or just its name).
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub path: String,
    pub text: String,
}

/// A converted note: its path inside the Notes folder (`a/b.md`) and its text.
#[derive(Clone, Debug, PartialEq)]
pub struct Converted {
    pub path: String,
    pub text: String,
    /// The Logseq file it came from.
    pub source: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Import {
    pub notes: Vec<Converted>,
    /// Files left out (not Markdown, or Logseq's own folders).
    pub skipped: Vec<String>,
    /// Block references and embeds rewritten as links.
    pub references: usize,
    /// Block references whose block was not among the files.
    pub unresolved: usize,
}

const OPEN_MARKERS: [&str; 6] = ["TODO", "LATER", "NOW", "DOING", "WAITING", "WAIT"];
const SKIPPED_FOLDERS: [&str; 6] = ["logseq", "assets", "draws", "whiteboards", "bak", "version-files"];

/// `name` without a `.md` or `.markdown` extension (any case), when it has one.
fn strip_markdown_ext(name: &str) -> Option<&str> {
    [".md", ".markdown"].into_iter().find_map(|ext| {
        let at = name.len().checked_sub(ext.len())?;
        name.get(at..).filter(|tail| tail.eq_ignore_ascii_case(ext)).map(|_| &name[..at])
    })
}

fn is_journal_stem(stem: &str) -> bool {
    let b = stem.as_bytes();
    b.len() == 10 && b[4] == b'_' && b[7] == b'_' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Where a graph file goes in the Notes folder, or `None` when it is not a page or journal.
pub fn note_path(relative: &str) -> Option<String> {
    let parts: Vec<&str> = relative.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let (file, dirs) = parts.split_last()?;
    let stem = strip_markdown_ext(file)?;
    if stem.is_empty() || file.starts_with('.') {
        return None;
    }
    if dirs.iter().any(|d| d.starts_with('.') || SKIPPED_FOLDERS.contains(&d.to_lowercase().as_str())) {
        return None;
    }
    let in_journals = dirs.iter().any(|d| d.eq_ignore_ascii_case("journals"));
    let in_pages = dirs.iter().any(|d| d.eq_ignore_ascii_case("pages"));
    if in_journals || (!in_pages && is_journal_stem(stem)) {
        let name = if is_journal_stem(stem) { stem.replace('_', "-") } else { clean_part(&percent_decode(stem)) };
        return (!name.is_empty()).then(|| format!("Journal/{name}.md"));
    }
    let name = percent_decode(&stem.replace("___", "/"));
    let folders: Vec<String> = name.split('/').map(clean_part).filter(|p| !p.is_empty()).collect();
    (!folders.is_empty()).then(|| format!("{}.md", folders.join("/")))
}

/// One folder or file name: trimmed, without `.`/`..` or characters Files rejects.
fn clean_part(part: &str) -> String {
    let cleaned: String = part.chars().map(|c| if c == '\\' || c.is_control() { '-' } else { c }).collect();
    let cleaned = cleaned.trim();
    if cleaned.chars().all(|c| c == '.') { String::new() } else { cleaned.to_owned() }
}

/// `path` or, when it is taken (case-insensitively), `path 2`, `path 3`… before the extension.
pub fn unique_path(path: &str, taken: &mut HashSet<String>) -> String {
    let (stem, ext) = path.rsplit_once('.').filter(|(s, _)| !s.ends_with('/')).map_or((path, String::new()), |(s, e)| (s, format!(".{e}")));
    let mut candidate = path.to_owned();
    let mut n = 1;
    while taken.contains(&candidate.to_lowercase()) {
        n += 1;
        candidate = format!("{stem} {n}{ext}");
    }
    taken.insert(candidate.to_lowercase());
    candidate
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36 && value.bytes().enumerate().all(|(i, b)| if matches!(i, 8 | 13 | 18 | 23) { b == b'-' } else { b.is_ascii_hexdigit() })
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    let t = t.strip_prefix("- ").map_or(t, str::trim_start);
    t.starts_with("```") || t.starts_with("~~~")
}

/// `id:: uuid` (a block property line), with its uuid.
fn id_line(line: &str) -> Option<String> {
    let t = line.trim_start();
    let t = t.strip_prefix("- ").unwrap_or(t);
    let value = t.strip_prefix("id::")?.trim();
    is_uuid(value).then(|| value.to_lowercase())
}

/// `key:: value` with a Logseq property key.
fn property(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once("::")?;
    let key = key.trim();
    let valid = !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') && line.starts_with(key);
    (valid && (value.is_empty() || value.starts_with(' '))).then(|| (key.to_lowercase(), value.trim().to_owned()))
}

/// The page properties at the top of a page: lines `key:: value`, or a first block `- key:: value`
/// whose lines are all properties. Returns them and the byte offset where the rest starts.
fn page_properties(text: &str) -> Option<(Vec<(String, String)>, usize)> {
    let mut props = Vec::new();
    let mut at = 0;
    let mut bullet = false;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let content = line.trim_end_matches(['\n', '\r']);
        let candidate = if index == 0 {
            match content.strip_prefix("- ") {
                Some(rest) => {
                    bullet = true;
                    rest
                }
                None => content,
            }
        } else if bullet {
            match content.strip_prefix("  ").or_else(|| content.strip_prefix('\t')) {
                Some(rest) if !rest.starts_with(char::is_whitespace) => rest,
                _ => break,
            }
        } else {
            content
        };
        let Some(prop) = property(candidate) else { break };
        props.push(prop);
        at += line.len();
    }
    if props.is_empty() {
        return None;
    }
    // A blank line after the properties goes with them.
    let rest = &text[at..];
    if let Some(blank) = rest.split_inclusive('\n').next().filter(|l| l.trim().is_empty() && l.ends_with('\n')) {
        at += blank.len();
    }
    Some((props, at))
}

/// Comma-separated values, with `[[links]]` and `#` removed (`[[a b]], c` → `a b`, `c`).
fn values(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '[' if chars.peek() == Some(&'[') => {
                chars.next();
                depth += 1;
            }
            ']' if depth > 0 && chars.peek() == Some(&']') => {
                chars.next();
                depth -= 1;
            }
            ',' if depth == 0 => items.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    items.push(current);
    items.into_iter().map(|i| i.trim().trim_start_matches('#').trim().to_owned()).filter(|i| !i.is_empty()).collect()
}

/// A YAML scalar: plain when safe, else double-quoted.
fn scalar(value: &str) -> String {
    let risky = value.starts_with(|c: char| "[]{}#&*!|>'\"%@`,?:-".contains(c) || c.is_whitespace())
        || value.ends_with(':')
        || value.ends_with(char::is_whitespace)
        || value.contains(": ")
        || value.contains(" #");
    if value.is_empty() || !risky {
        value.to_owned()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn yaml_list(out: &mut String, name: &str, items: Vec<String>) {
    out.push_str(&format!("{name}:\n"));
    for item in items {
        out.push_str(&format!("  - {}\n", scalar(&item)));
    }
}

fn front(props: &[(String, String)]) -> String {
    let mut out = String::from("---\n");
    for (key, value) in props {
        match key.as_str() {
            // Obsidian tags have no spaces.
            "tags" => yaml_list(&mut out, "tags", values(value).into_iter().map(|t| t.split_whitespace().collect::<Vec<_>>().join("-")).collect()),
            "alias" | "aliases" => yaml_list(&mut out, "aliases", values(value)),
            "id" | "collapsed" => {}
            _ if value.is_empty() => out.push_str(&format!("{key}:\n")),
            _ => out.push_str(&format!("{key}: {}\n", scalar(value))),
        }
    }
    out.push_str("---\n");
    out
}

/// What a block reference points to: the note's link target and the block's `^id`.
#[derive(Clone, Debug, PartialEq)]
struct Target {
    page: String,
    block: Option<String>,
}

impl Target {
    fn link(&self, label: Option<&str>, embed: bool) -> String {
        let fragment = self.block.as_ref().map(|b| format!("#^{b}")).unwrap_or_default();
        let label = label.filter(|l| !l.is_empty()).map(|l| format!("|{l}")).unwrap_or_default();
        format!("{}[[{}{fragment}{label}]]", if embed { "!" } else { "" }, self.page)
    }
}

#[derive(Default)]
struct Stats {
    references: usize,
    unresolved: usize,
}

/// `{{embed [[page]]}}` or `{{embed ((uuid))}}` at the start of `rest`: the embed and its length.
fn embed(rest: &str, refs: &HashMap<String, Target>, stats: &mut Stats) -> Option<(String, usize)> {
    let close = rest.find("}}")?;
    let inner = rest[2..close].trim();
    let arg = inner.strip_prefix("embed")?;
    if !arg.starts_with(char::is_whitespace) {
        return None;
    }
    let arg = arg.trim();
    let text = if let Some(page) = arg.strip_prefix("[[").and_then(|a| a.strip_suffix("]]")) {
        format!("![[{page}]]")
    } else {
        let uuid = arg.strip_prefix("((").and_then(|a| a.strip_suffix("))"))?.trim().to_lowercase();
        if !is_uuid(&uuid) {
            return None;
        }
        let Some(target) = refs.get(&uuid) else {
            stats.unresolved += 1;
            return None;
        };
        stats.references += 1;
        target.link(None, true)
    };
    Some((text, close + 2))
}

/// Embeds and block references in one line, outside inline code.
fn inline(line: &str, refs: &HashMap<String, Target>, stats: &mut Stats) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while !rest.is_empty() {
        if rest.starts_with('`') {
            let ticks = rest.len() - rest.trim_start_matches('`').len();
            let fence = &rest[..ticks];
            if let Some(end) = rest[ticks..].find(fence) {
                let len = ticks + end + ticks;
                out.push_str(&rest[..len]);
                rest = &rest[len..];
            } else {
                out.push_str(fence);
                rest = &rest[ticks..];
            }
            continue;
        }
        if rest.starts_with("{{")
            && let Some((text, len)) = embed(rest, refs, stats)
        {
            out.push_str(&text);
            rest = &rest[len..];
            continue;
        }
        if rest.starts_with("((")
            && let Some(close) = rest[2..].find("))")
            && is_uuid(rest[2..2 + close].trim())
        {
            let uuid = rest[2..2 + close].trim().to_lowercase();
            let end = 2 + close + 2;
            match refs.get(&uuid) {
                Some(target) => {
                    // `[label](((uuid)))`: a labelled block link.
                    let labelled = if out.ends_with("](") && rest[end..].starts_with(')') { out[..out.len() - 2].rfind('[') } else { None };
                    match labelled {
                        Some(open) => {
                            let label = out[open + 1..out.len() - 2].to_owned();
                            out.truncate(open);
                            out.push_str(&target.link(Some(&label), false));
                            rest = &rest[end + 1..];
                        }
                        None => {
                            out.push_str(&target.link(None, false));
                            rest = &rest[end..];
                        }
                    }
                    stats.references += 1;
                }
                None => {
                    stats.unresolved += 1;
                    out.push_str(&rest[..end]);
                    rest = &rest[end..];
                }
            }
            continue;
        }
        let c = rest.chars().next().expect("a character");
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// `- TODO x` → `- [ ] x`, `- DONE x` → `- [x] x` (any indentation).
fn task(line: &str) -> Option<String> {
    let indent = &line[..line.len() - line.trim_start().len()];
    let body = line.trim_start().strip_prefix("- ")?;
    let (marker, rest) = body.split_once(' ').unwrap_or((body, ""));
    let check = if marker == "DONE" {
        "[x]"
    } else if OPEN_MARKERS.contains(&marker) {
        "[ ]"
    } else {
        return None;
    };
    Some(if rest.is_empty() { format!("{indent}- {check}") } else { format!("{indent}- {check} {rest}") })
}

fn normalize(text: &str) -> String {
    text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n")
}

/// Page properties, unless the page already has YAML properties.
fn logseq_properties(text: &str) -> Option<(Vec<(String, String)>, usize)> {
    if text.starts_with("---\n") { None } else { page_properties(text) }
}

/// Converts one page's (normalized) text.
fn convert_text(text: &str, refs: &HashMap<String, Target>, stats: &mut Stats) -> String {
    let (head, body) = match logseq_properties(text) {
        Some((props, at)) => {
            let shown: Vec<(String, String)> = props.iter().filter(|(k, _)| k != "id" && k != "collapsed").cloned().collect();
            (if shown.is_empty() { String::new() } else { front(&shown) }, &text[at..])
        }
        None => (String::new(), text),
    };
    let mut lines: Vec<String> = Vec::new();
    let mut block: Option<usize> = None;
    let mut fence = false;
    let mut drawer = false;
    for line in body.split('\n') {
        if fence || is_fence(line) {
            // A code block's `^id` goes on a line after it, so a bullet opening one is no target.
            if !fence && line.trim_start().starts_with("- ") {
                block = None;
            }
            if is_fence(line) {
                fence = !fence;
            }
            lines.push(line.to_owned());
            continue;
        }
        let trimmed = line.trim();
        if drawer {
            drawer = !trimmed.eq_ignore_ascii_case(":END:");
            continue;
        }
        if trimmed.eq_ignore_ascii_case(":LOGBOOK:") {
            drawer = true;
            continue;
        }
        if let Some(uuid) = id_line(line) {
            if let Some(short) = refs.get(&uuid).and_then(|t| t.block.clone()) {
                match block {
                    Some(at) => {
                        let target = &mut lines[at];
                        let kept = target.trim_end().len();
                        target.truncate(kept);
                        target.push_str(&format!(" ^{short}"));
                    }
                    None => lines.push(format!("{}^{short}", &line[..line.len() - line.trim_start().len()])),
                }
            }
            continue;
        }
        if trimmed.starts_with("collapsed::") && property(trimmed).is_some() {
            continue;
        }
        let converted = task(line).unwrap_or_else(|| line.to_owned());
        if line.trim_start().starts_with("- ") || line.trim_start() == "-" {
            block = Some(lines.len());
        }
        lines.push(inline(&converted, refs, stats));
    }
    format!("{head}{}", lines.join("\n"))
}

/// The `^id` for a block uuid: its first hex digits, longer when another block on the page has them.
fn short_id(uuid: &str, used: &mut HashSet<String>) -> String {
    let hex: String = uuid.chars().filter(char::is_ascii_hexdigit).collect();
    for len in 6..=hex.len() {
        let candidate = hex[..len].to_owned();
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    hex
}

/// Converts a picked graph. `existing` are the paths already in the Notes folder: nothing is
/// overwritten, a taken path gets ` 2` (links to its blocks follow the new name).
pub fn convert(files: &[Source], existing: &[String]) -> Import {
    let mut import = Import::default();
    let mut taken: HashSet<String> = existing.iter().map(|p| p.to_lowercase()).collect();
    // (source path, normalized text, note path)
    let mut planned: Vec<(&str, String, String)> = Vec::new();
    for file in files {
        match note_path(&file.path) {
            Some(path) => planned.push((file.path.as_str(), normalize(&file.text), unique_path(&path, &mut taken))),
            None => import.skipped.push(file.path.clone()),
        }
    }
    // Every block with an `id::` gets a `^id`; references anywhere in the graph find it.
    let mut refs: HashMap<String, Target> = HashMap::new();
    for (_, text, path) in &planned {
        let page = path.strip_suffix(".md").unwrap_or(path).to_owned();
        let mut used = HashSet::new();
        let mut props_end = 0;
        // A reference to the page-properties block names the page itself.
        if let Some((props, at)) = logseq_properties(text) {
            props_end = at;
            for (key, value) in props {
                if key == "id" && is_uuid(&value) {
                    refs.entry(value.to_lowercase()).or_insert(Target { page: page.clone(), block: None });
                }
            }
        }
        let mut fence = false;
        for line in text[props_end..].lines() {
            if is_fence(line) {
                fence = !fence;
                continue;
            }
            if !fence && let Some(uuid) = id_line(line) {
                let block = short_id(&uuid, &mut used);
                refs.entry(uuid).or_insert(Target { page: page.clone(), block: Some(block) });
            }
        }
    }
    for (source, text, path) in planned {
        let mut stats = Stats::default();
        let text = convert_text(&text, &refs, &mut stats);
        import.references += stats.references;
        import.unresolved += stats.unresolved;
        import.notes.push(Converted { path, text, source: source.to_owned() });
    }
    import
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(path: &str, text: &str) -> Source {
        Source { path: path.into(), text: text.into() }
    }

    #[test]
    fn paths() {
        assert_eq!(note_path("pages/Reading.md").as_deref(), Some("Reading.md"));
        assert_eq!(note_path("graph/pages/project___dar___ui.md").as_deref(), Some("project/dar/ui.md"));
        assert_eq!(note_path("pages/project%2Fdar.md").as_deref(), Some("project/dar.md"));
        assert_eq!(note_path("pages/What%3F.md").as_deref(), Some("What?.md"));
        assert_eq!(note_path("journals/2026_11_23.md").as_deref(), Some("Journal/2026-11-23.md"));
        assert_eq!(note_path("2026_11_23.md").as_deref(), Some("Journal/2026-11-23.md"));
        assert_eq!(note_path("journals/Week 47.md").as_deref(), Some("Journal/Week 47.md"));
        assert_eq!(note_path("pages/2026_11_23.md").as_deref(), Some("2026_11_23.md"));
        assert_eq!(note_path("Loose.MD").as_deref(), Some("Loose.md"));
        assert_eq!(note_path("logseq/bak/pages/Reading.md"), None);
        assert_eq!(note_path("logseq/config.edn"), None);
        assert_eq!(note_path("assets/photo.png"), None);
        assert_eq!(note_path("pages/.hidden.md"), None);
        assert_eq!(note_path("pages/a___..___b.md").as_deref(), Some("a/b.md"));
        assert_eq!(note_path("pages/.md"), None);
    }

    #[test]
    fn unique_paths() {
        let mut taken: HashSet<String> = ["reading.md".to_owned(), "reading 2.md".to_owned()].into();
        assert_eq!(unique_path("Reading.md", &mut taken), "Reading 3.md");
        assert_eq!(unique_path("Reading.md", &mut taken), "Reading 4.md");
        assert_eq!(unique_path("Other.md", &mut taken), "Other.md");
        assert_eq!(unique_path("Journal/2026-11-23.md", &mut taken), "Journal/2026-11-23.md");
    }

    #[test]
    fn page_properties_become_yaml() {
        let import = convert(
            &[src("pages/Dune.md", "title:: Dune\ntags:: [[science fiction]], book, #classic\nalias:: [[Dune (novel)]], Arrakis\nrating:: 5\nauthor:: [[Frank Herbert]]\n\n- First block\n")],
            &[],
        );
        assert_eq!(
            import.notes[0].text,
            "---\ntitle: Dune\ntags:\n  - science-fiction\n  - book\n  - classic\naliases:\n  - Dune (novel)\n  - Arrakis\nrating: 5\nauthor: \"[[Frank Herbert]]\"\n---\n- First block\n"
        );
        // The first block holding only properties counts too.
        let import = convert(&[src("pages/B.md", "- type:: book\n  status:: reading\n- Body\n")], &[]);
        assert_eq!(import.notes[0].text, "---\ntype: book\nstatus: reading\n---\n- Body\n");
        // A first block with text is not a property block; YAML is left alone.
        let import = convert(&[src("pages/C.md", "- Some text\n  key:: value\n")], &[]);
        assert_eq!(import.notes[0].text, "- Some text\n  key:: value\n");
        let import = convert(&[src("pages/D.md", "---\ntitle: Kept\n---\nbody:: no\n")], &[]);
        assert_eq!(import.notes[0].text, "---\ntitle: Kept\n---\nbody:: no\n");
        // The parsed YAML reads back with Notes' own properties.
        let (props, _) = crate::front_matter(&convert(&[src("pages/E.md", "tags:: a, b\nnote:: x: y\n- z")], &[]).notes[0].text).unwrap();
        assert_eq!(props[0], ("tags".into(), crate::Prop::List(vec!["a".into(), "b".into()])));
        assert_eq!(props[1], ("note".into(), crate::Prop::Text("x: y".into())));
    }

    #[test]
    fn tasks_embeds_and_drawers() {
        let text = "- TODO Write [[Plan]]\n  - DONE Draft\n    :LOGBOOK:\n    CLOCK: [2026-11-23 Mon 10:00]\n    :END:\n- LATER Read\n- NOW Ship\n- TODOS stay\n- {{embed [[Plan]]}}\n- {{ embed [[Other page]] }}\n- {{query (todo now)}}\n- collapsed block\n  collapsed:: true\n";
        let import = convert(&[src("pages/Tasks.md", text)], &[]);
        assert_eq!(
            import.notes[0].text,
            "- [ ] Write [[Plan]]\n  - [x] Draft\n- [ ] Read\n- [ ] Ship\n- TODOS stay\n- ![[Plan]]\n- ![[Other page]]\n- {{query (todo now)}}\n- collapsed block\n"
        );
    }

    #[test]
    fn block_references() {
        let uuid = "6563a1b2-0f4e-4b1a-9c3d-112233445566";
        let other = "6563a1b2-ffff-4b1a-9c3d-112233445566";
        let missing = "00000000-0000-4000-8000-000000000000";
        let files = [
            src("pages/project___dar.md", &format!("- The key idea\n  id:: {uuid}\n  - child\n- Second\n  id:: {}\n", other.to_uppercase())),
            src(
                "journals/2026_11_23.md",
                &format!("- See (({uuid})) and [this]((({other})))\n- {{{{embed (({uuid}))}}}}\n- Lost (({missing}))\n- `(({uuid}))` in code\n```\n(({uuid}))\n```\n"),
            ),
        ];
        let import = convert(&files, &[]);
        assert_eq!(import.notes[0].path, "project/dar.md");
        assert_eq!(import.notes[0].text, "- The key idea ^6563a1\n  - child\n- Second ^6563a1b\n");
        assert_eq!(import.notes[1].path, "Journal/2026-11-23.md");
        assert_eq!(
            import.notes[1].text,
            format!("- See [[project/dar#^6563a1]] and [[project/dar#^6563a1b|this]]\n- ![[project/dar#^6563a1]]\n- Lost (({missing}))\n- `(({uuid}))` in code\n```\n(({uuid}))\n```\n")
        );
        assert_eq!((import.references, import.unresolved), (3, 1));
        // The ids read back as Notes block ids.
        assert_eq!(crate::block_id("- The key idea ^6563a1"), Some("6563a1"));
    }

    #[test]
    fn references_follow_renamed_pages_and_skips() {
        let uuid = "11111111-2222-4333-8444-555555555555";
        let files = [src("pages/Reading.md", &format!("- quote\n  id:: {uuid}\n")), src("pages/Index.md", &format!("- (({uuid}))\n")), src("logseq/config.edn", "{}"), src("assets/a.png", "")];
        let import = convert(&files, &["reading.md".to_owned()]);
        assert_eq!(import.notes[0].path, "Reading 2.md");
        assert_eq!(import.notes[1].text, "- [[Reading 2#^111111]]\n");
        assert_eq!(import.skipped, ["logseq/config.edn", "assets/a.png"]);
    }

    #[test]
    fn page_property_ids_link_to_the_page() {
        let uuid = "11111111-2222-4333-8444-555555555555";
        let files = [src("pages/Topic.md", &format!("id:: {uuid}\ntype:: idea\n\n- body\n")), src("pages/Ref.md", &format!("- (({uuid}))\n"))];
        let import = convert(&files, &[]);
        assert_eq!(import.notes[0].text, "---\ntype: idea\n---\n- body\n");
        assert_eq!(import.notes[1].text, "- [[Topic]]\n");
    }

    #[test]
    fn values_and_scalars() {
        assert_eq!(values("[[a, b]], c ,#d,"), ["a, b", "c", "d"]);
        assert_eq!(scalar("plain text"), "plain text");
        assert_eq!(scalar("[[Link]]"), "\"[[Link]]\"");
        assert_eq!(scalar("say \"hi\": now"), "\"say \\\"hi\\\": now\"");
        assert_eq!(scalar("true"), "true");
    }
}
