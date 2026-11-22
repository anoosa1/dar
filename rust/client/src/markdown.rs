//! Markdown preview and formatting toolbar (`pim/markdown.tsx`, `pim/format-markdown.ts`).
//! Fixed elements only: source HTML stays text and remote images are not fetched.

use leptos::prelude::*;

use crate::icons::Icon;
use crate::ui::{ActionMenu, MenuAction};

const FORMATTING: [(&str, &str, &str, &str, bool); 9] = [
    ("Heading", "heading", "# ", "", false),
    ("Bold", "bold", "**", "**", false),
    ("Italic", "italic", "*", "*", false),
    ("Link", "link", "[", "](https://example.com)", true),
    ("Bullet list", "bullets", "- ", "", false),
    ("Task list", "checklist", "- [ ] ", "", false),
    ("Quote", "quote", "> ", "", false),
    ("Code", "code", "`", "`", false),
    ("Code block", "code", "\n```\n", "\n```\n", false),
];

/// Source-preserving formatting shared by the Notes and Files editors.
#[component]
pub fn MarkdownToolbar(on_format: Callback<(&'static str, &'static str)>, #[prop(into)] disabled: Signal<bool>, children: Children) -> impl IntoView {
    view! {
        <div class="text-formatting" role="group" aria-label="Markdown formatting">
            {FORMATTING[..8].iter().map(|&(label, icon, before, after, group)| view! {
                <button type="button" aria-label=label title=label class=if group { "text-button starts-group" } else { "text-button" } disabled=move || disabled.get() on:click=move |_| { on_format.try_run((before, after)); }><Icon name=icon/></button>
            }).collect_view()}
            <ActionMenu label="More formatting" actions=move |_| FORMATTING[8..].iter().map(|&(label, icon, before, after, _)| MenuAction::new(label, icon).disabled(disabled.get_untracked()).run(move || { on_format.try_run((before, after)); })).collect()>
                <Icon name="more"/>
            </ActionMenu>
            {children()}
        </div>
    }
}

/// Byte offset of a UTF-16 index (textarea selections count UTF-16 units).
pub fn byte_index(text: &str, utf16: u32) -> usize {
    let mut units = 0u32;
    for (index, c) in text.char_indices() {
        if units >= utf16 {
            return index;
        }
        units += c.len_utf16() as u32;
    }
    text.len()
}

pub fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// Block commands affect complete source lines; inline commands wrap the selection.
/// Offsets are UTF-16 units, as the textarea reports them.
pub fn format_markdown(source: &str, start: u32, end: u32, before: &str, after: &str) -> (String, u32, u32) {
    let (s, e) = (byte_index(source, start), byte_index(source, end));
    if ["# ", "- ", "- [ ] ", "> "].contains(&before) && after.is_empty() {
        let line_start = if s == 0 { 0 } else { source[..s].rfind('\n').map(|i| i + 1).unwrap_or(0) };
        let selected_end = if e > s && source.as_bytes()[e - 1] == b'\n' { e - 1 } else { e };
        let line_end = source[selected_end..].find('\n').map(|i| i + selected_end).unwrap_or(source.len());
        let replacement = source[line_start..line_end].split('\n').map(|line| format!("{before}{line}")).collect::<Vec<_>>().join("\n");
        let text = format!("{}{}{}", &source[..line_start], replacement, &source[line_end..]);
        let start16 = utf16_len(&source[..line_start]);
        return (text, start16, start16 + utf16_len(&replacement));
    }
    let text = format!("{}{}{}{}{}", &source[..s], before, &source[s..e], after, &source[e..]);
    let shift = utf16_len(before);
    (text, start + shift, end + shift)
}

#[derive(Clone, Debug, PartialEq)]
enum Inline {
    Text(String),
    Code(String),
    Strong(String),
    Em(String),
    Link(String, String),
}

/// Finds the first inline token at `at` (backticks, bold, italic, links and images).
fn token(rest: &str) -> Option<(usize, Inline)> {
    let line_ok = |s: &str| !s.is_empty() && !s.contains('\n');
    if let Some(body) = rest.strip_prefix('`') {
        let close = body.find('`')?;
        return line_ok(&body[..close]).then(|| (close + 2, Inline::Code(body[..close].into())));
    }
    if let Some(body) = rest.strip_prefix("**")
        && let Some(close) = body.find("**")
            && line_ok(&body[..close]) && !body[..close].contains('*') {
                return Some((close + 4, Inline::Strong(body[..close].into())));
            }
    if let Some(body) = rest.strip_prefix('*') {
        let close = body.find('*')?;
        return line_ok(&body[..close]).then(|| (close + 2, Inline::Em(body[..close].into())));
    }
    let (image, body) = match rest.strip_prefix("![") {
        Some(b) => (true, b),
        None => (false, rest.strip_prefix('[')?),
    };
    let close = body.find(']')?;
    let label = &body[..close];
    if label.contains('\n') {
        return None;
    }
    let target_part = body[close + 1..].strip_prefix('(')?;
    let end = target_part.find(|c: char| c == ')' || c.is_whitespace())?;
    if end == 0 || !target_part[end..].starts_with(')') {
        return None;
    }
    let target = &target_part[..end];
    let length = usize::from(image) + 1 + close + 2 + end + 1;
    let allowed = target.starts_with("http://") || target.starts_with("https://") || is_content_link(target);
    let raw = rest[..length].to_owned();
    Some((length, if !image && allowed { Inline::Link(label.into(), target.into()) } else { Inline::Text(raw) }))
}

fn is_content_link(target: &str) -> bool {
    let Some(rest) = target.strip_prefix("/api/v1/files/") else { return false };
    let Some((id, tail)) = rest.split_once('/') else { return false };
    !id.is_empty() && (tail == "content" || tail.starts_with("content?"))
}

fn inline(source: &str) -> Vec<Inline> {
    let mut nodes = Vec::new();
    let (mut offset, mut at) = (0, 0);
    while at < source.len() {
        let rest = &source[at..];
        if matches!(rest.as_bytes()[0], b'`' | b'*' | b'[' | b'!')
            && let Some((length, node)) = token(rest) {
                nodes.push(Inline::Text(source[offset..at].into()));
                nodes.push(node);
                at += length;
                offset = at;
                continue;
            }
        at += rest.chars().next().map(char::len_utf8).unwrap_or(1);
    }
    nodes.push(Inline::Text(source[offset..].into()));
    nodes
}

fn render_inline(source: &str) -> impl IntoView + use<> {
    inline(source)
        .into_iter()
        .map(|node| match node {
            Inline::Text(t) => t.into_any(),
            Inline::Code(t) => view! { <code>{t}</code> }.into_any(),
            Inline::Strong(t) => view! { <strong>{t}</strong> }.into_any(),
            Inline::Em(t) => view! { <em>{t}</em> }.into_any(),
            Inline::Link(label, href) => {
                let target = href.starts_with("http").then_some("_blank");
                view! { <a href=href rel="noopener noreferrer" target=target>{label}</a> }.into_any()
            }
        })
        .collect_view()
}

fn list_item(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some((false, rest));
        }
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0).then(|| trimmed[digits..].strip_prefix(". ").map(|rest| (true, rest))).flatten()
}

#[component]
pub fn Markdown(#[prop(into)] source: String) -> impl IntoView {
    let lines: Vec<&str> = source.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let mut nodes = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("```") {
            let language = line.trim_start().trim_start_matches("```").to_owned();
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                code.push(lines[i]);
                i += 1;
            }
            i += 1;
            let label = if language.is_empty() { "Code".to_owned() } else { format!("{language} code") };
            nodes.push(view! { <pre><code aria-label=label>{code.join("\n")}</code></pre> }.into_any());
            continue;
        }
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let hashes = line.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&hashes) && line[hashes..].starts_with(|c: char| c.is_whitespace()) && !line[hashes..].trim().is_empty() {
            let text = render_inline(line[hashes..].trim_start());
            nodes.push(match hashes {
                1 => view! { <h1>{text}</h1> }.into_any(),
                2 => view! { <h2>{text}</h2> }.into_any(),
                3 => view! { <h3>{text}</h3> }.into_any(),
                4 => view! { <h4>{text}</h4> }.into_any(),
                5 => view! { <h5>{text}</h5> }.into_any(),
                _ => view! { <h6>{text}</h6> }.into_any(),
            });
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix('>') {
            nodes.push(view! { <blockquote>{render_inline(rest.strip_prefix(' ').unwrap_or(rest))}</blockquote> }.into_any());
            i += 1;
            continue;
        }
        if let Some((ordered, _)) = list_item(line) {
            let mut items = Vec::new();
            while let Some((_, text)) = lines.get(i).and_then(|l| list_item(l)) {
                i += 1;
                let check = text.strip_prefix('[').and_then(|t| {
                    let mark = t.chars().next()?;
                    (matches!(mark, ' ' | 'x' | 'X') && t[1..].starts_with(']')).then(|| (mark == ' ', t[2..].trim_start()))
                });
                items.push(match check {
                    Some((unchecked, rest)) => view! { <li><span aria-label=if unchecked { "Unchecked" } else { "Checked" }>{if unchecked { "☐" } else { "☑" }}</span>" "{render_inline(rest)}</li> }.into_any(),
                    None => view! { <li>{render_inline(text)}</li> }.into_any(),
                });
            }
            nodes.push(if ordered { view! { <ol>{items}</ol> }.into_any() } else { view! { <ul>{items}</ul> }.into_any() });
            continue;
        }
        nodes.push(view! { <p>{render_inline(line)}</p> }.into_any());
        i += 1;
    }
    view! { <div class="markdown-preview">{nodes}</div> }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_tokens() {
        assert_eq!(inline("a **b** `c` *d* [e](https://x.test) [f](javascript:x)"), vec![
            Inline::Text("a ".into()), Inline::Strong("b".into()), Inline::Text(" ".into()), Inline::Code("c".into()), Inline::Text(" ".into()),
            Inline::Em("d".into()), Inline::Text(" ".into()), Inline::Link("e".into(), "https://x.test".into()), Inline::Text(" ".into()),
            Inline::Text("[f](javascript:x)".into()), Inline::Text(String::new()),
        ]);
    }

    #[test]
    fn formatting_wraps_or_prefixes() {
        assert_eq!(format_markdown("one\ntwo", 4, 7, "# ", ""), ("one\n# two".into(), 4, 9));
        assert_eq!(format_markdown("héllo", 1, 3, "**", "**"), ("h**él**lo".into(), 3, 5));
    }
}
