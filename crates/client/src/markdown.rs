//! Markdown preview and formatting toolbar, shared by Notes, Files and Mail. The preview parses
//! with `dar-notes` (pulldown-cmark): tables, footnotes, task lists, Obsidian callouts, wikilinks,
//! tags, highlights and properties. Fixed elements only: source HTML stays text and remote
//! images are not fetched.

use std::ops::Range;

use dar_notes::pulldown_cmark::{Alignment, CodeBlockKind, Event, LinkType, Parser, Tag};
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

/// What a preview does with note links, tags and checkboxes. Files and Mail previews leave it
/// empty: links between notes then show as text.
#[derive(Clone, Copy, Default)]
pub struct NoteLinks {
    /// A link's target (and whether it is a wikilink) to the ID of the note it names.
    pub resolve: Option<Callback<(String, bool), Option<String>>>,
    /// Follows a link: target, wikilink, fragment (`Heading` or `^block`).
    pub follow: Option<Callback<(String, bool, Option<String>)>>,
    pub tag: Option<Callback<String>>,
    /// Toggles the checkbox whose `[ ]` starts at this byte offset of the source.
    pub toggle: Option<Callback<usize>>,
}

#[derive(Debug)]
enum Node {
    El(Tag<'static>, Range<usize>, Vec<Node>),
    Text(String),
    Code(String),
    Html(String),
    Math(String, bool),
    Task(bool, usize),
    Note(String),
    Soft,
    Hard,
    Rule,
}

fn tree(source: &str, base: usize) -> Vec<Node> {
    let mut stack: Vec<(Tag<'static>, Range<usize>, Vec<Node>)> = Vec::new();
    let mut root: Vec<Node> = Vec::new();
    fn push(stack: &mut [(Tag<'static>, Range<usize>, Vec<Node>)], root: &mut Vec<Node>, node: Node) {
        let list = match stack.last_mut() {
            Some(top) => &mut top.2,
            None => root,
        };
        if let (Node::Text(next), Some(Node::Text(previous))) = (&node, list.last_mut()) {
            previous.push_str(next);
            return;
        }
        list.push(node);
    }
    for (event, range) in Parser::new_ext(source, dar_notes::options()).into_offset_iter() {
        let range = range.start + base..range.end + base;
        match event {
            Event::Start(tag) => stack.push((tag.into_static(), range, Vec::new())),
            Event::End(_) => {
                if let Some((tag, range, children)) = stack.pop() {
                    push(&mut stack, &mut root, Node::El(tag, range, children));
                }
            }
            Event::Text(t) => push(&mut stack, &mut root, Node::Text(t.into_string())),
            Event::Code(t) => push(&mut stack, &mut root, Node::Code(t.into_string())),
            Event::Html(t) | Event::InlineHtml(t) => push(&mut stack, &mut root, Node::Html(t.into_string())),
            Event::InlineMath(t) => push(&mut stack, &mut root, Node::Math(t.into_string(), false)),
            Event::DisplayMath(t) => push(&mut stack, &mut root, Node::Math(t.into_string(), true)),
            Event::FootnoteReference(label) => push(&mut stack, &mut root, Node::Note(label.into_string())),
            Event::SoftBreak => push(&mut stack, &mut root, Node::Soft),
            Event::HardBreak => push(&mut stack, &mut root, Node::Hard),
            Event::Rule => push(&mut stack, &mut root, Node::Rule),
            Event::TaskListMarker(done) => push(&mut stack, &mut root, Node::Task(done, range.start)),
        }
    }
    root
}

fn plain(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node {
            Node::Text(t) | Node::Code(t) | Node::Math(t, _) => out.push_str(t),
            Node::Soft | Node::Hard => out.push(' '),
            Node::El(_, _, children) => out.push_str(&plain(children)),
            _ => {}
        }
    }
    out
}

/// `==highlighted==` stretches of a text: (text, highlighted).
fn highlights(text: &str) -> Vec<(&str, bool)> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("==") {
        let Some(close) = rest[open + 2..].find("==") else { break };
        if close == 0 {
            parts.push((&rest[..open + 2], false));
            rest = &rest[open + 2..];
            continue;
        }
        parts.push((&rest[..open], false));
        parts.push((&rest[open + 2..open + 2 + close], true));
        rest = &rest[open + 4 + close..];
    }
    parts.push((rest, false));
    parts.retain(|(t, _)| !t.is_empty());
    parts
}

/// `[!type]±  Title` opening a blockquote: (type, fold, title).
fn callout(text: &str) -> Option<(String, Option<bool>, String)> {
    let rest = text.strip_prefix("[!")?;
    let close = rest.find(']')?;
    let kind = rest[..close].trim().to_lowercase();
    if kind.is_empty() || !kind.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    let mut after = &rest[close + 1..];
    let fold = match after.chars().next() {
        Some('-') => Some(false),
        Some('+') => Some(true),
        _ => None,
    };
    if fold.is_some() {
        after = &after[1..];
    }
    let title = after.trim();
    let title = if title.is_empty() {
        let mut chars = kind.chars();
        chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
    } else {
        title.to_owned()
    };
    Some((kind, fold, title))
}

fn is_content_link(target: &str) -> bool {
    let Some(rest) = target.strip_prefix("/api/v1/files/") else { return false };
    let Some((id, tail)) = rest.split_once('/') else { return false };
    !id.is_empty() && (tail == "content" || tail.starts_with("content?"))
}

fn web_link(target: &str) -> bool {
    let lower = target.to_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
}

struct Render {
    links: NoteLinks,
}

impl Render {
    fn nodes(&self, nodes: Vec<Node>) -> Vec<AnyView> {
        nodes.into_iter().map(|n| self.node(n)).collect()
    }

    fn text(&self, text: &str) -> AnyView {
        highlights(text)
            .into_iter()
            .map(|(part, marked)| {
                let pieces = self.tags(part);
                if marked { view! { <mark>{pieces}</mark> }.into_any() } else { pieces.into_any() }
            })
            .collect_view()
            .into_any()
    }

    fn tags(&self, text: &str) -> Vec<AnyView> {
        let mut out = Vec::new();
        let mut at = 0;
        for range in dar_notes::tags_in(text, 0..text.len()) {
            out.push(text[at..range.start].to_owned().into_any());
            let name = text[range.start + 1..range.end].to_owned();
            let label = text[range.clone()].to_owned();
            out.push(match self.links.tag {
                Some(cb) => view! { <button type="button" class="note-tag" on:click=move |_| { cb.try_run(name.clone()); }>{label}</button> }.into_any(),
                None => view! { <span class="note-tag">{label}</span> }.into_any(),
            });
            at = range.end;
        }
        out.push(text[at..].to_owned().into_any());
        out
    }

    /// A trailing `^block` id comes off the last text of a paragraph or item.
    fn block_id(children: &mut [Node]) -> Option<String> {
        let Some(Node::Text(text)) = children.last_mut() else { return None };
        let id = dar_notes::block_id(text)?.to_owned();
        let cut = text.trim_end().len() - id.len() - 1;
        text.truncate(cut);
        let trimmed = text.trim_end().len();
        text.truncate(trimmed);
        Some(id)
    }

    fn note_link(&self, target: String, wiki: bool, fragment: Option<String>, label: AnyView, embed: bool) -> AnyView {
        let class = if embed { "note-embed" } else { "note-link" };
        let (Some(resolve), Some(follow)) = (self.links.resolve, self.links.follow) else {
            return view! { <span class=class>{label}</span> }.into_any();
        };
        if wiki && !dar_notes::is_note_target(&target) {
            return view! { <span class=format!("{class} is-file")>{label}</span> }.into_any();
        }
        let resolved = resolve.try_run((target.clone(), wiki)).flatten();
        let unresolved = resolved.is_none();
        let href = resolved.map(|id| format!("/notes?note={id}")).unwrap_or_else(|| "/notes".into());
        let title = unresolved.then(|| format!("Create \u{201c}{target}\u{201d}"));
        let icon = embed.then(|| view! { <Icon name="file"/> });
        view! {
            <a class=format!("{class}{}", if unresolved { " is-unresolved" } else { "" }) href=href title=title data-target=target.clone()
                on:click=move |e: web_sys::MouseEvent| {
                    if e.ctrl_key() || e.meta_key() || e.shift_key() || e.button() != 0 {
                        return;
                    }
                    e.prevent_default();
                    follow.try_run((target.clone(), wiki, fragment.clone()));
                }>{icon}{label}</a>
        }
        .into_any()
    }

    fn node(&self, node: Node) -> AnyView {
        match node {
            Node::Text(t) => self.text(&t),
            Node::Code(t) => view! { <code>{t}</code> }.into_any(),
            Node::Html(t) => t.into_any(),
            Node::Math(t, false) => view! { <code class="math">{t}</code> }.into_any(),
            Node::Math(t, true) => view! { <pre class="math"><code>{t}</code></pre> }.into_any(),
            Node::Note(label) => view! { <sup class="footnote-ref">{label}</sup> }.into_any(),
            Node::Soft => " ".into_any(),
            Node::Hard => view! { <br/> }.into_any(),
            Node::Rule => view! { <hr/> }.into_any(),
            Node::Task(done, offset) => match self.links.toggle {
                Some(cb) => view! {
                    <input type="checkbox" class="note-check" prop:checked=done aria-label=if done { "Completed task" } else { "Open task" }
                        on:change=move |_| { cb.try_run(offset); }/>
                }
                .into_any(),
                None => view! { <span class="note-check-mark" aria-label=if done { "Checked" } else { "Unchecked" }>{if done { "\u{2611}" } else { "\u{2610}" }}</span>" " }.into_any(),
            },
            Node::El(tag, range, mut children) => match tag {
                Tag::Paragraph => {
                    let block = Self::block_id(&mut children);
                    view! { <p data-block=block>{self.nodes(children)}</p> }.into_any()
                }
                Tag::Heading { level, .. } => {
                    let anchor = dar_notes::slug(&plain(&children));
                    let inner = self.nodes(children);
                    match level as u8 {
                        1 => view! { <h1 data-anchor=anchor>{inner}</h1> }.into_any(),
                        2 => view! { <h2 data-anchor=anchor>{inner}</h2> }.into_any(),
                        3 => view! { <h3 data-anchor=anchor>{inner}</h3> }.into_any(),
                        4 => view! { <h4 data-anchor=anchor>{inner}</h4> }.into_any(),
                        5 => view! { <h5 data-anchor=anchor>{inner}</h5> }.into_any(),
                        _ => view! { <h6 data-anchor=anchor>{inner}</h6> }.into_any(),
                    }
                }
                Tag::BlockQuote(_) => self.quote(children),
                Tag::CodeBlock(kind) => {
                    let language = match kind {
                        CodeBlockKind::Fenced(lang) => lang.split_whitespace().next().unwrap_or_default().to_owned(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    let label = if language.is_empty() { "Code".to_owned() } else { format!("{language} code") };
                    view! { <pre><code aria-label=label>{plain(&children)}</code></pre> }.into_any()
                }
                Tag::HtmlBlock => view! { <p class="markdown-html">{plain_html(&children)}</p> }.into_any(),
                Tag::List(Some(start)) => view! { <ol start=(start != 1).then(|| start.to_string())>{self.nodes(children)}</ol> }.into_any(),
                Tag::List(None) => view! { <ul>{self.nodes(children)}</ul> }.into_any(),
                Tag::Item => {
                    let task = matches!(children.first(), Some(Node::Task(..)));
                    let block = Self::block_id(&mut children);
                    view! { <li class=task.then_some("task-item") data-block=block>{self.nodes(children)}</li> }.into_any()
                }
                Tag::FootnoteDefinition(label) => {
                    let (label, shown) = (label.to_string(), label.to_string());
                    view! { <div class="footnote" data-footnote=label><sup>{shown}</sup>{self.nodes(children)}</div> }.into_any()
                }
                Tag::Table(alignments) => {
                    let classes: Vec<Option<&'static str>> = alignments
                        .iter()
                        .map(|a| match a {
                            Alignment::Center => Some("align-center"),
                            Alignment::Right => Some("align-right"),
                            _ => None,
                        })
                        .collect();
                    let mut rows = children.into_iter();
                    let head = rows.next();
                    let head = head.map(|h| self.row(h, &classes, true));
                    let body: Vec<AnyView> = rows.map(|r| self.row(r, &classes, false)).collect();
                    view! { <div class="markdown-table"><table><thead>{head}</thead><tbody>{body}</tbody></table></div> }.into_any()
                }
                Tag::Emphasis => view! { <em>{self.nodes(children)}</em> }.into_any(),
                Tag::Strong => view! { <strong>{self.nodes(children)}</strong> }.into_any(),
                Tag::Strikethrough => view! { <del>{self.nodes(children)}</del> }.into_any(),
                Tag::Link { link_type: LinkType::WikiLink { .. }, dest_url, .. } => {
                    let (target, fragment) = split_fragment(&dest_url);
                    let label = self.nodes(children).into_any();
                    self.note_link(target, true, fragment, label, false)
                }
                Tag::Image { link_type: LinkType::WikiLink { .. }, dest_url, .. } => {
                    let (target, fragment) = split_fragment(&dest_url);
                    let label = plain(&children).into_any();
                    self.note_link(target, true, fragment, label, true)
                }
                Tag::Link { dest_url, .. } => {
                    let dest = dest_url.to_string();
                    let inner = self.nodes(children).into_any();
                    if web_link(&dest) || is_content_link(&dest) {
                        let blank = web_link(&dest).then_some("_blank");
                        view! { <a href=dest rel="noopener noreferrer" target=blank>{inner}</a> }.into_any()
                    } else if !dest.contains(':') && !dest.starts_with('#') && {
                        let lower = dest.split('#').next().unwrap_or_default().to_lowercase();
                        lower.ends_with(".md") || lower.ends_with(".markdown")
                    } {
                        let (path, fragment) = split_fragment(&dest);
                        self.note_link(dar_notes::percent_decode(&path), false, fragment.map(|f| dar_notes::percent_decode(&f)), inner, false)
                    } else {
                        view! { <span class="markdown-link">{inner}</span> }.into_any()
                    }
                }
                Tag::Image { dest_url, .. } => {
                    let dest = dest_url.to_string();
                    let alt = plain(&children);
                    if is_content_link(&dest) {
                        view! { <img src=dest alt=alt loading="lazy"/> }.into_any()
                    } else if web_link(&dest) {
                        // Remote images are not fetched; the link opens them.
                        let label = if alt.is_empty() { dest.clone() } else { alt };
                        view! { <a class="markdown-remote-image" href=dest rel="noopener noreferrer" target="_blank"><Icon name="external"/>{label}</a> }.into_any()
                    } else {
                        alt.into_any()
                    }
                }
                Tag::MetadataBlock(_) => ().into_any(),
                _ => {
                    let _ = range;
                    self.nodes(children).into_any()
                }
            },
        }
    }

    fn row(&self, row: Node, classes: &[Option<&'static str>], head: bool) -> AnyView {
        let Node::El(_, _, cells) = row else { return ().into_any() };
        let cells: Vec<AnyView> = cells
            .into_iter()
            .enumerate()
            .map(|(i, cell)| {
                let class = classes.get(i).copied().flatten();
                let inner = match cell {
                    Node::El(_, _, children) => self.nodes(children),
                    other => vec![self.node(other)],
                };
                if head { view! { <th class=class>{inner}</th> }.into_any() } else { view! { <td class=class>{inner}</td> }.into_any() }
            })
            .collect();
        view! { <tr>{cells}</tr> }.into_any()
    }

    /// Obsidian callouts (`> [!tip] Title`, foldable with `-` or `+`); other quotes stay quotes.
    fn quote(&self, mut children: Vec<Node>) -> AnyView {
        let header = match children.first() {
            Some(Node::El(Tag::Paragraph, _, inner)) => match inner.first() {
                Some(Node::Text(text)) => {
                    let line = text.split('\n').next().unwrap_or_default();
                    callout(line).map(|c| (c, line.len()))
                }
                _ => None,
            },
            _ => None,
        };
        let Some(((kind, fold, title), cut)) = header else {
            return view! { <blockquote>{self.nodes(children)}</blockquote> }.into_any();
        };
        // The title line leaves the first paragraph; what follows its line break stays.
        if let Some(Node::El(Tag::Paragraph, _, inner)) = children.first_mut() {
            if let Some(Node::Text(text)) = inner.first_mut() {
                text.replace_range(..cut, "");
            }
            let rest_starts = inner.iter().position(|n| matches!(n, Node::Soft | Node::Hard));
            match rest_starts {
                Some(index) if matches!(inner.first(), Some(Node::Text(t)) if t.trim().is_empty()) => {
                    inner.drain(..=index);
                }
                Some(_) => {}
                None if inner.iter().all(|n| matches!(n, Node::Text(t) if t.trim().is_empty())) => inner.clear(),
                None => {}
            }
        }
        if matches!(children.first(), Some(Node::El(Tag::Paragraph, _, inner)) if inner.is_empty()) {
            children.remove(0);
        }
        let body = self.nodes(children);
        let class = format!("callout callout-{kind}");
        match fold {
            Some(open) => view! {
                <details class=class data-callout=kind open=open><summary class="callout-title">{title}</summary><div class="callout-content">{body}</div></details>
            }
            .into_any(),
            None => view! {
                <div class=class data-callout=kind role="note"><p class="callout-title">{title}</p><div class="callout-content">{body}</div></div>
            }
            .into_any(),
        }
    }
}

fn plain_html(nodes: &[Node]) -> String {
    nodes.iter().map(|n| if let Node::Html(t) = n { t.as_str() } else { "" }).collect::<String>().trim_end().to_owned()
}

fn split_fragment(dest: &str) -> (String, Option<String>) {
    match dest.split_once('#') {
        Some((target, fragment)) => (target.trim().to_owned(), Some(fragment.trim().to_owned()).filter(|f| !f.is_empty())),
        None => (dest.trim().to_owned(), None),
    }
}

/// Note properties above the text, as Obsidian shows them.
fn properties(props: Vec<(String, dar_notes::Prop)>) -> impl IntoView {
    let rows: Vec<AnyView> = props
        .into_iter()
        .map(|(key, value)| {
            let shown = match &value {
                dar_notes::Prop::List(items) => items.iter().map(|i| view! { <span class="note-property-item">{i.clone()}</span> }.into_any()).collect_view().into_any(),
                dar_notes::Prop::Text(text) => text.clone().into_any(),
            };
            view! { <div class="note-property"><dt>{key}</dt><dd>{shown}</dd></div> }.into_any()
        })
        .collect();
    view! { <dl class="note-properties" aria-label="Properties">{rows}</dl> }
}

/// Renders Markdown with fixed elements only: source HTML stays text, `javascript:` and other
/// unknown links stay text, and remote images are not fetched.
#[component]
pub fn Markdown(#[prop(into)] source: String, #[prop(optional)] links: NoteLinks, #[prop(optional)] inline: bool) -> impl IntoView {
    let masked = dar_notes::mask_comments(&source).into_owned();
    let (props, body) = dar_notes::front_matter(&masked).unwrap_or_default();
    let render = Render { links };
    let nodes = render.nodes(tree(&masked[body..], body));
    let props = (!props.is_empty() && !inline).then(|| properties(props));
    view! { <div class=if inline { "markdown-preview markdown-inline" } else { "markdown-preview" }>{props}{nodes}</div> }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_wraps_or_prefixes() {
        assert_eq!(format_markdown("one\ntwo", 4, 7, "# ", ""), ("one\n# two".into(), 4, 9));
        assert_eq!(format_markdown("héllo", 1, 3, "**", "**"), ("h**él**lo".into(), 3, 5));
    }

    #[test]
    fn highlights_and_callouts() {
        assert_eq!(highlights("a ==b== c ==d"), vec![("a ", false), ("b", true), (" c ==d", false)]);
        assert_eq!(highlights("x ==== y"), vec![("x ==", false), ("== y", false)]);
        assert_eq!(callout("[!tip] Remember"), Some(("tip".into(), None, "Remember".into())));
        assert_eq!(callout("[!FAQ]- "), Some(("faq".into(), Some(false), "Faq".into())));
        assert_eq!(callout("[link] x"), None);
    }

    #[test]
    fn tree_keeps_offsets_and_merges_text() {
        let nodes = tree("> [!note] T\n> body\n\n- [ ] x", 10);
        let Node::El(Tag::BlockQuote(_), _, quote) = &nodes[0] else { panic!("{nodes:?}") };
        let Node::El(Tag::Paragraph, _, inner) = &quote[0] else { panic!() };
        assert!(matches!(&inner[0], Node::Text(t) if t == "[!note] T"));
        let Node::El(Tag::List(None), _, items) = &nodes[1] else { panic!() };
        let Node::El(Tag::Item, _, item) = &items[0] else { panic!() };
        assert!(matches!(item[0], Node::Task(false, 32)));
    }
}
