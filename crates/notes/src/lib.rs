//! Notes' Markdown model, shared by the runtime (index, backlinks, mentions) and the client
//! (preview, autocomplete, link updates). Notes are Obsidian-style Markdown: YAML properties,
//! `[[wikilinks]]` with `#heading`, `#^block` and `|label`, `![[embeds]]`, `#tags`, `^block` ids
//! and `%%comments%%`. Everything here is derived from the text; nothing is stored.

use std::borrow::Cow;
use std::ops::Range;

pub use pulldown_cmark;
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

mod front;
mod vault;

pub use front::{Prop, front_matter};
pub use vault::{Vault, is_note_target, md_target, percent_decode};

/// The Markdown dialect Notes, Files and Mail previews share.
pub fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_MATH
        | Options::ENABLE_WIKILINKS
}

/// `%%comments%%` (outside fenced code) become spaces, keeping every byte offset and line break.
pub fn mask_comments(text: &str) -> Cow<'_, str> {
    if !text.contains("%%") {
        return Cow::Borrowed(text);
    }
    let mut out = text.as_bytes().to_vec();
    let mut changed = false;
    let mut fence = false;
    let mut open: Option<usize> = None;
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        if open.is_none() && is_fence(line) {
            fence = !fence;
        } else if !fence {
            let mut at = 0;
            while let Some(found) = line[at..].find("%%") {
                let index = line_start + at + found;
                match open.take() {
                    Some(start) => {
                        mask_run(&mut out, text, start, index + 2);
                        changed = true;
                    }
                    None => open = Some(index),
                }
                at += found + 2;
            }
        }
        line_start += line.len();
    }
    if !changed {
        return Cow::Borrowed(text);
    }
    // Only ASCII bytes were replaced wholesale, and whole multi-byte characters lie inside each range.
    Cow::Owned(String::from_utf8(out).unwrap_or_else(|_| text.to_owned()))
}

/// Blanks `text[start..end]` in `out`, keeping line breaks. A comment that opens a line with text
/// after it starts with a zero-width space, so the text is not read as indented code.
fn mask_run(out: &mut [u8], text: &str, start: usize, end: usize) {
    for byte in &mut out[start..end] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
    let last = text[start..end].rfind('\n').map_or(start, |i| start + i + 1);
    let line_start = text[..last].rfind('\n').map_or(0, |i| i + 1);
    let opens_line = text[line_start..last].trim_start_matches([' ', '\t']).is_empty() || last > start;
    let text_after = text[end..].split('\n').next().is_some_and(|rest| !rest.trim().is_empty());
    if opens_line && text_after && end - last >= 3 {
        out[last..last + 3].copy_from_slice("\u{200b}".as_bytes());
    }
}

fn is_fence(line: &str) -> bool {
    let trimmed = line.trim_start_matches(' ');
    line.len() - trimmed.len() <= 3 && (trimmed.starts_with("```") || trimmed.starts_with("~~~"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// The note or file named, without `#fragment`; percent-decoded for Markdown links.
    pub target: String,
    /// `Heading`, `^block` or `Heading#Sub`, when given.
    pub fragment: Option<String>,
    /// `[[target|label]]`'s label, when given.
    pub label: Option<String>,
    pub embed: bool,
    /// `[[wikilink]]` rather than `[label](path.md)`.
    pub wiki: bool,
    /// The whole link in the source.
    pub range: Range<usize>,
    /// The target as written (without the fragment), the part a rename rewrites.
    pub target_range: Range<usize>,
    /// Zero-based line.
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    pub line: usize,
    pub offset: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub properties: Vec<(String, Prop)>,
    /// Where the text after the properties starts.
    pub body: usize,
    /// Unique tags without `#`, from properties and the text, in order of appearance.
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
    pub links: Vec<Link>,
    pub headings: Vec<Heading>,
    /// `^block` ids.
    pub blocks: Vec<String>,
    /// Open and completed checkboxes.
    pub tasks: (usize, usize),
    /// Code, links, raw HTML and properties: where mentions are not looked for.
    pub skip: Vec<Range<usize>>,
}

/// Zero-based line of a byte offset.
pub struct Lines(Vec<usize>);

impl Lines {
    pub fn new(text: &str) -> Self {
        Self(std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect())
    }
    pub fn line(&self, offset: usize) -> usize {
        self.0.partition_point(|start| *start <= offset).saturating_sub(1)
    }
    pub fn start(&self, line: usize) -> usize {
        self.0.get(line).copied().unwrap_or(0)
    }
}

/// The text of a line, without its line break.
pub fn line_text(text: &str, lines: &Lines, line: usize) -> String {
    let start = lines.start(line);
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    text[start..end].trim_end_matches('\r').to_owned()
}

fn push_unique(list: &mut Vec<String>, value: &str) {
    let lower = value.to_lowercase();
    if !value.is_empty() && !list.iter().any(|v| v.to_lowercase() == lower) {
        list.push(value.to_owned());
    }
}

fn tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '/')
}

/// `#tag` names in a stretch of plain text (Obsidian's rule: after a space or at a line start,
/// letters, digits, `_`, `-` and `/`, not only digits).
pub fn tags_in(source: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let text = &source[range.clone()];
    for (index, _) in text.match_indices('#') {
        let at = range.start + index;
        let before = source[..at].chars().next_back();
        if before.is_some_and(|c| !c.is_whitespace() && !"([{,;\"'".contains(c)) {
            continue;
        }
        let name_len: usize = text[index + 1..].chars().take_while(|c| tag_char(*c)).map(char::len_utf8).sum();
        let mut name = &text[index + 1..index + 1 + name_len];
        name = name.trim_end_matches('/');
        if name.is_empty() || name.chars().all(|c| c.is_ascii_digit() || c == '/' || c == '-' || c == '_') {
            continue;
        }
        found.push(at..at + 1 + name.len());
    }
    found
}

/// What a note says about itself and where it points.
pub fn facts(text: &str) -> Facts {
    let mut facts = Facts::default();
    if let Some((properties, body)) = front_matter(text) {
        facts.body = body;
        facts.skip.push(0..body);
        for (key, value) in &properties {
            match key.to_lowercase().as_str() {
                "tags" | "tag" => value.items().iter().for_each(|t| push_unique(&mut facts.tags, t.trim_start_matches('#'))),
                "aliases" | "alias" => value.items().iter().for_each(|a| push_unique(&mut facts.aliases, a)),
                _ => {}
            }
        }
        facts.properties = properties;
    }
    let masked_text = mask_comments(text);
    let masked: &str = &masked_text;
    let lines = Lines::new(text);
    let mut heading: Option<Heading> = None;
    let mut in_link = 0usize;
    let mut in_code = false;
    let mut in_meta = false;
    let mut pending_text: Option<Range<usize>> = None;
    let flush = |pending: &mut Option<Range<usize>>, facts: &mut Facts| {
        if let Some(range) = pending.take() {
            for tag in tags_in(masked, range) {
                push_unique(&mut facts.tags, &text[tag.start + 1..tag.end]);
            }
        }
    };
    let base = facts.body;
    for (event, local) in Parser::new_ext(&masked[base..], options()).into_offset_iter() {
        let range = local.start + base..local.end + base;
        if !matches!(event, Event::Text(_)) {
            flush(&mut pending_text, &mut facts);
        }
        match event {
            Event::Start(Tag::MetadataBlock(_)) => in_meta = true,
            Event::End(TagEnd::MetadataBlock(_)) => in_meta = false,
            Event::Start(Tag::CodeBlock(_)) => {
                in_code = true;
                facts.skip.push(range);
            }
            Event::End(TagEnd::CodeBlock) => in_code = false,
            Event::Code(_) | Event::Html(_) | Event::InlineHtml(_) | Event::InlineMath(_) | Event::DisplayMath(_) => {
                if let Some(h) = heading.as_mut()
                    && let Event::Code(code) = &event
                {
                    h.text.push_str(code);
                }
                facts.skip.push(range);
            }
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some(Heading { level: level as u8, text: String::new(), line: lines.line(range.start), offset: range.start });
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(mut h) = heading.take() {
                    h.text = h.text.trim().to_owned();
                    if !h.text.is_empty() {
                        facts.headings.push(h);
                    }
                }
            }
            Event::Start(Tag::Link { link_type, dest_url, .. }) | Event::Start(Tag::Image { link_type, dest_url, .. }) => {
                in_link += 1;
                facts.skip.push(range.clone());
                if let Some(link) = link_at(text, range, link_type, &dest_url, &lines) {
                    facts.links.push(link);
                }
            }
            Event::End(TagEnd::Link) | Event::End(TagEnd::Image) => in_link = in_link.saturating_sub(1),
            Event::TaskListMarker(done) => {
                if done {
                    facts.tasks.1 += 1;
                } else {
                    facts.tasks.0 += 1;
                }
            }
            Event::Text(t) => {
                if let Some(h) = heading.as_mut() {
                    h.text.push_str(&t);
                }
                if !in_code && !in_meta && in_link == 0 {
                    pending_text = Some(match pending_text.take() {
                        Some(previous) if previous.end == range.start => previous.start..range.end,
                        Some(previous) => {
                            let mut tmp = Some(previous);
                            flush(&mut tmp, &mut facts);
                            range
                        }
                        None => range,
                    });
                }
            }
            _ => {}
        }
    }
    flush(&mut pending_text, &mut facts);
    facts.blocks = block_ids(text, facts.body);
    facts
}

fn link_at(text: &str, range: Range<usize>, link_type: LinkType, dest: &str, lines: &Lines) -> Option<Link> {
    let raw = &text[range.clone()];
    let line = lines.line(range.start);
    if let LinkType::WikiLink { .. } = link_type {
        let embed = raw.starts_with('!');
        let start = range.start + if embed { 3 } else { 2 };
        let inner = &text[start..range.end.saturating_sub(2).max(start)];
        let (written, label) = inner.split_once('|').map_or((inner, None), |(t, l)| (t, Some(l.to_owned())));
        let (target, fragment) = written.split_once('#').map_or((written, None), |(t, f)| (t, Some(f.to_owned())));
        return Some(Link {
            target: target.trim().to_owned(),
            fragment,
            label,
            embed,
            wiki: true,
            target_range: start..start + target.len(),
            range,
            line,
        });
    }
    if !matches!(link_type, LinkType::Inline) || dest.contains(':') || dest.starts_with('#') {
        return None;
    }
    let (path, fragment) = dest.split_once('#').map_or((dest, None), |(p, f)| (p, Some(percent_decode(f))));
    let lower = path.to_lowercase();
    if !(lower.ends_with(".md") || lower.ends_with(".markdown")) {
        return None;
    }
    let open = raw.rfind("](")? + 2;
    let angle = raw[open..].starts_with('<');
    let start = range.start + open + usize::from(angle);
    let end = start + text[start..].find(['#', '>', ')', ' ']).unwrap_or(0);
    Some(Link {
        target: percent_decode(path),
        fragment,
        label: None,
        embed: raw.starts_with('!'),
        wiki: false,
        target_range: start..end,
        range,
        line,
    })
}

/// `^block` ids at the end of a line (outside fenced code).
fn block_ids(text: &str, body: usize) -> Vec<String> {
    let mut ids = Vec::new();
    let mut fence = false;
    for line in text[body..].lines() {
        if is_fence(line) {
            fence = !fence;
            continue;
        }
        if !fence && let Some(id) = block_id(line) {
            push_unique(&mut ids, id);
        }
    }
    ids
}

/// The `^id` closing a line, if any.
pub fn block_id(line: &str) -> Option<&str> {
    let trimmed = line.trim_end();
    let at = trimmed.rfind('^')?;
    let id = &trimmed[at + 1..];
    let before_ok = at == 0 || trimmed[..at].ends_with(char::is_whitespace);
    (before_ok && !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then_some(id)
}

/// Whole-word, case-insensitive occurrences of any of `names` outside links, code and properties.
pub fn mentions(text: &str, facts: &Facts, names: &[String]) -> Vec<Range<usize>> {
    let mut found: Vec<Range<usize>> = Vec::new();
    let names: Vec<Vec<char>> = names.iter().filter(|n| n.chars().count() >= 3).map(|n| n.to_lowercase().chars().collect()).collect();
    let mut fence = false;
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let start = line_start;
        line_start += line.len();
        if start < facts.body {
            continue;
        }
        if is_fence(line) {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        for (index, _) in line.char_indices() {
            let at = start + index;
            if found.last().is_some_and(|r| at < r.end) || facts.skip.iter().any(|r| r.contains(&at)) {
                continue;
            }
            if line[..index].chars().next_back().is_some_and(char::is_alphanumeric) {
                continue;
            }
            for name in &names {
                if let Some(len) = matches_at(&line[index..], name)
                    && !line[index + len..].chars().next().is_some_and(char::is_alphanumeric)
                {
                    found.push(at..at + len);
                    break;
                }
            }
        }
    }
    found
}

fn matches_at(text: &str, name: &[char]) -> Option<usize> {
    let mut chars = text.char_indices();
    for expected in name {
        let (_, c) = chars.next()?;
        let mut lowered = c.to_lowercase();
        if lowered.next() != Some(*expected) || lowered.next().is_some() {
            return None;
        }
    }
    Some(chars.next().map_or(text.len(), |(i, _)| i))
}

/// Applies `replace` to every link's target; returns the new text when something changed.
pub fn rewrite(text: &str, mut replace: impl FnMut(&Link) -> Option<String>) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for link in facts(text).links {
        if let Some(next) = replace(&link)
            && next != text[link.target_range.clone()]
        {
            out.push_str(&text[at..link.target_range.start]);
            out.push_str(&next);
            at = link.target_range.end;
        }
    }
    (at > 0).then(|| {
        out.push_str(&text[at..]);
        out
    })
}

/// A heading's anchor as previews and links compare it: lowercase words joined by `-`.
pub fn slug(heading: &str) -> String {
    let mut out = String::new();
    for c in heading.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if (c.is_whitespace() || c == '-') && !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\ntitle: Plan\ntags: [work, \"#urgent\"]\naliases:\n  - The plan\n---\n# Head `x`\n\nSee [[Other note#Part|alias]] and ![[pic.png]] #my_tag/sub #2024 a#b `[[no]]` [md](Sub/Other%20note.md#Top) [[a]]b\n\n- [ ] task ^blk1\n- [x] done\n\n```\n[[in code]] #nope\n```\n%%[[hidden]]%% #seen\n";

    #[test]
    fn extracts_facts() {
        let f = facts(NOTE);
        assert_eq!(f.tags, ["work", "urgent", "my_tag/sub", "seen"]);
        assert_eq!(f.aliases, ["The plan"]);
        assert_eq!(f.headings, [Heading { level: 1, text: "Head x".into(), line: 6, offset: NOTE.find("# Head").unwrap() }]);
        let targets: Vec<_> = f.links.iter().map(|l| (l.target.as_str(), l.fragment.as_deref(), l.embed, l.wiki, &NOTE[l.target_range.clone()])).collect();
        assert_eq!(
            targets,
            [
                ("Other note", Some("Part"), false, true, "Other note"),
                ("pic.png", None, true, true, "pic.png"),
                ("Sub/Other note.md", Some("Top"), false, false, "Sub/Other%20note.md"),
                ("a", None, false, true, "a"),
            ]
        );
        assert_eq!(f.links[0].line, 8);
        assert_eq!(f.links[0].label.as_deref(), Some("alias"));
        assert_eq!(f.blocks, ["blk1"]);
        assert_eq!(f.tasks, (1, 1));
    }

    #[test]
    fn masks_comments_in_place() {
        let text = "a %%b\nc%% d\n```\n%%x%%\n```";
        let masked = mask_comments(text);
        assert_eq!(masked, "a    \n\u{200b} d\n```\n%%x%%\n```");
        assert_eq!(masked.len(), text.len());
    }

    #[test]
    fn rewrites_targets_only() {
        let text = "[[Old]] [[Old#H|label]] ![[Old]] [x](Old.md) [[Other]]";
        let next = rewrite(text, |l| (l.target == "Old" || l.target == "Old.md").then(|| if l.wiki { "New name".into() } else { "New%20name.md".into() })).unwrap();
        assert_eq!(next, "[[New name]] [[New name#H|label]] ![[New name]] [x](New%20name.md) [[Other]]");
        assert_eq!(rewrite(text, |_| None), None);
    }

    #[test]
    fn finds_mentions_outside_links() {
        let text = "---\ntitle: Project Apollo\n---\nproject apollo starts. [[Project Apollo]] `Project Apollo` Project Apollos\n";
        let f = facts(text);
        let found = mentions(text, &f, &["Project Apollo".into()]);
        assert_eq!(found.iter().map(|r| &text[r.clone()]).collect::<Vec<_>>(), ["project apollo"]);
    }

    #[test]
    fn slugs_and_blocks() {
        assert_eq!(slug("  Héllo, World — 2 "), "héllo-world-2");
        assert_eq!(block_id("text ^abc-1"), Some("abc-1"));
        assert_eq!(block_id("x^abc"), None);
    }
}
