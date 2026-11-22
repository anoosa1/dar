//! Mail (selected direction A with B's inline reply, `docs/spacetimedb/mail-mocks.html`):
//! accounts and mailboxes in the sidebar, a day-grouped message list, the conversation in the
//! right sheet with an inline reply, a floating compose sheet, and settings with providers for
//! administrators. Messages are read from their `.eml` files in Files.

mod compose;
mod panes;
mod workspace;

pub use workspace::MailWorkspace;

use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;

use crate::ui::document;

pub(crate) const BASE: &str = "/api/v1/mail";

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Provider {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub auth: String,
    pub domains: Vec<String>,
    pub source: String,
    pub enabled: bool,
    pub document: serde_json::Value,
    pub has_secret: bool,
    pub accounts: u32,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Mailbox {
    pub id: String,
    pub name: String,
    pub role: String,
    pub parent_id: String,
    pub total: u32,
    pub unread: u32,
    pub folder_id: String,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Account {
    pub id: String,
    pub provider_id: String,
    pub provider_name: String,
    pub protocol: String,
    pub auth: String,
    pub provider_available: bool,
    pub name: String,
    pub address: String,
    pub display_name: String,
    pub username: String,
    pub color: u32,
    pub folder_id: String,
    pub status: String,
    pub last_error: String,
    pub synced_at: f64,
    pub signature: String,
    pub mailboxes: Vec<Mailbox>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Counts {
    pub inbox: u32,
    pub unread: u32,
    pub flagged: u32,
    pub drafts: u32,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Overview {
    pub providers: Vec<Provider>,
    pub accounts: Vec<Account>,
    pub counts: Counts,
    pub mail_folder: String,
    pub administrator: bool,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Addr {
    pub name: String,
    pub address: String,
}

impl Addr {
    pub fn label(&self) -> String {
        if self.name.is_empty() { self.address.clone() } else { self.name.clone() }
    }
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Recipients {
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Summary {
    pub id: String,
    pub account_id: String,
    pub mailbox_id: String,
    pub subject: String,
    pub from_name: String,
    pub from_address: String,
    pub recipients: Recipients,
    pub date: f64,
    pub snippet: String,
    pub seen: bool,
    pub flagged: bool,
    pub answered: bool,
    pub draft: bool,
    pub attachments: bool,
    pub size: f64,
    pub message_id: String,
    pub file_id: String,
    pub thread_count: u32,
    pub mailbox_role: String,
}

impl Summary {
    pub fn sender(&self) -> String {
        if !self.from_name.is_empty() {
            self.from_name.clone()
        } else if !self.from_address.is_empty() {
            self.from_address.clone()
        } else {
            "Unknown sender".into()
        }
    }
    pub fn subject_or(&self) -> String {
        if self.subject.trim().is_empty() { "No subject".into() } else { self.subject.clone() }
    }
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Part {
    pub index: u32,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub size: f64,
    pub inline: bool,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Body {
    pub html: String,
    pub text: String,
    pub attachments: Vec<Part>,
    pub blocked_images: u32,
    pub reply_to: Vec<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    /// The plain part as written (a Markdown message's source).
    pub text_part: String,
    /// "markdown" for messages written in Markdown in Dar, else "plain".
    pub format: String,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Detail {
    #[serde(flatten)]
    pub summary: Summary,
    pub file_name: String,
    pub mailbox_name: String,
    pub account_name: String,
    pub thread: Vec<Summary>,
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Page {
    pub items: Vec<Summary>,
    pub cursor: Option<String>,
}

/// An account's tone (0–5) as a class, matching the sidebar marks.
pub(crate) fn tone(color: u32) -> String {
    format!("mail-tone-{}", color % 6)
}

pub(crate) fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| c.is_whitespace() || c == '@' || c == '.').filter(|w| !w.is_empty()).collect();
    let letters: String = words.iter().take(2).filter_map(|w| w.chars().next()).flat_map(char::to_uppercase).collect();
    if letters.is_empty() { "?".into() } else { letters }
}

/// A stable avatar tone from a sender's address.
pub(crate) fn avatar_tone(seed: &str) -> String {
    let sum: u32 = seed.bytes().map(u32::from).sum();
    format!("mail-avatar tone-{}", sum % 5)
}

pub(crate) fn encode(value: &str) -> String {
    js_sys::encode_uri_component(value).as_string().unwrap_or_default()
}

/// Base64 for attachment uploads.
pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

const ALLOWED: &[&str] = &[
    "a", "abbr", "b", "blockquote", "br", "caption", "cite", "code", "dd", "del", "div", "dl", "dt", "em", "figcaption", "figure", "h1", "h2", "h3", "h4",
    "h5", "h6", "hr", "i", "img", "ins", "kbd", "li", "mark", "ol", "p", "pre", "q", "s", "small", "span", "strong", "sub", "sup", "table", "tbody", "td",
    "tfoot", "th", "thead", "time", "tr", "u", "ul",
];
const DROPPED: &[&str] = &["script", "style", "iframe", "object", "embed", "svg", "math", "form", "template", "noscript", "link", "meta", "base", "video", "audio", "canvas"];

fn walk(doc: &web_sys::Document, from: &web_sys::Node, to: &web_sys::Node, depth: usize, origin: &str) {
    let children = from.child_nodes();
    for i in 0..children.length() {
        let Some(child) = children.item(i) else { continue };
        match child.node_type() {
            web_sys::Node::TEXT_NODE => {
                let _ = to.append_child(&doc.create_text_node(&child.text_content().unwrap_or_default()));
                continue;
            }
            web_sys::Node::ELEMENT_NODE => {}
            _ => continue,
        }
        let element: web_sys::Element = child.unchecked_into();
        let tag = element.tag_name().to_lowercase();
        if DROPPED.contains(&tag.as_str()) {
            continue;
        }
        if !ALLOWED.contains(&tag.as_str()) || depth > 60 {
            walk(doc, &element, to, depth, origin);
            continue;
        }
        if tag == "img" {
            // The server rewrote every image it allows to Mail's own API; anything else is blocked.
            let Some(src) = element.get_attribute("src").filter(|s| s.starts_with(&format!("{origin}/api/v1/mail/"))) else { continue };
            let Ok(el) = doc.create_element("img") else { continue };
            let _ = el.set_attribute("src", &src);
            let _ = el.set_attribute("alt", &element.get_attribute("alt").unwrap_or_default());
            let _ = el.set_attribute("loading", "lazy");
            let _ = el.set_attribute("referrerpolicy", "no-referrer");
            let _ = to.append_child(&el);
            continue;
        }
        let Ok(copy) = doc.create_element(&tag) else { continue };
        if tag == "a"
            && let Some(href) = element.get_attribute("href").and_then(|h| web_sys::Url::new(&h).ok()).filter(|u| matches!(u.protocol().as_str(), "http:" | "https:" | "mailto:"))
        {
            let _ = copy.set_attribute("href", &href.href());
            let _ = copy.set_attribute("target", "_blank");
            let _ = copy.set_attribute("rel", "noopener noreferrer");
        }
        for name in ["colspan", "rowspan", "start", "title", "dir"] {
            if let Some(value) = element.get_attribute(name).filter(|v| !v.is_empty() && v.len() < 300 && (name != "dir" || v == "rtl" || v == "ltr")) {
                let _ = copy.set_attribute(name, &value);
            }
        }
        walk(doc, &element, &copy, depth + 1, origin);
        let _ = to.append_child(&copy);
    }
}

/// Server-sanitized HTML rebuilt through a second allowlist in an inert document.
pub(crate) fn render_html(html: &str) -> web_sys::DocumentFragment {
    let doc = document();
    let out = doc.create_document_fragment();
    let origin = crate::ui::window().location().origin().unwrap_or_default();
    let Ok(parser) = web_sys::DomParser::new() else { return out };
    let Ok(source) = parser.parse_from_string(&format!("<!doctype html><body>{html}</body>"), web_sys::SupportedType::TextHtml) else { return out };
    if let Some(body) = source.body() {
        walk(&doc, &body, &out, 0, &origin);
    }
    out
}
