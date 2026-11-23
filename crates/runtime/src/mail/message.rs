//! Messages as files: what the index keeps from an `.eml`, how a message reads in the browser
//! (sanitized HTML, inline `cid:` images, remote images blocked or proxied, attachments) and how
//! a new message is written.

use std::sync::OnceLock;

use mail_parser::{Address, MessageParser, MimeHeaders};
use regex::Regex;
use serde_json::{Value, json};

use crate::news::html::{sanitize, text_of};

fn parse(data: &[u8]) -> Option<mail_parser::Message<'_>> {
    MessageParser::default().parse(data)
}

fn addresses(a: Option<&Address<'_>>) -> Vec<Value> {
    a.map(|a| a.iter().filter_map(|x| x.address().map(|addr| json!({ "name": x.name().unwrap_or_default(), "address": addr }))).take(100).collect()).unwrap_or_default()
}

fn ids(value: &mail_parser::HeaderValue<'_>) -> Vec<String> {
    match value {
        mail_parser::HeaderValue::Text(t) => vec![t.to_string()],
        mail_parser::HeaderValue::TextList(l) => l.iter().map(|t| t.to_string()).collect(),
        _ => Vec::new(),
    }
}

fn collapse(text: &str, max: usize) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

fn plain_text(m: &mail_parser::Message<'_>) -> String {
    if let Some(t) = m.text_bodies().find(|p| p.is_text() && !p.is_text_html()).and_then(|p| p.text_contents()) {
        return t.to_owned();
    }
    m.html_bodies().find(|p| p.is_text_html()).and_then(|p| p.text_contents()).map(|h| text_of(h, 20_000)).unwrap_or_default()
}

/// What the index keeps: identities for threads, sender, recipients, date, a snippet.
pub fn meta(data: &[u8], fallback_date: f64) -> Value {
    let Some(m) = parse(data) else {
        return json!({ "subject": "", "date": fallback_date, "recipients": { "to": [], "cc": [] } });
    };
    let message_id = m.message_id().unwrap_or_default().to_owned();
    let references = ids(m.references());
    let reply_to = ids(m.in_reply_to());
    let thread = references.first().or(reply_to.first()).cloned().unwrap_or_else(|| message_id.clone());
    let from = m.from().and_then(|a| a.first());
    let date = m.date().map(|d| d.to_timestamp() as f64 * 1000.0).filter(|d| *d > 0.0).unwrap_or(fallback_date);
    json!({
        "messageId": message_id,
        "threadId": thread,
        "subject": m.subject().unwrap_or_default(),
        "fromName": from.and_then(|f| f.name()).unwrap_or_default(),
        "fromAddress": from.and_then(|f| f.address()).unwrap_or_default(),
        "recipients": { "to": addresses(m.to()), "cc": addresses(m.cc()) },
        "date": date,
        "snippet": collapse(&plain_text(&m), 200),
        "attachments": m.attachments().any(|a| a.content_id().is_none() || a.attachment_name().is_some()),
    })
}

fn cid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)src\s*=\s*["']?cid:([^"'\s>]+)["']?"#).expect("cid regex"))
}

fn img_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"<img src="([^"]*)""#).expect("img regex"))
}

fn encode(value: &str) -> String {
    value.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// The message for reading. `base` is the API prefix for this message's parts
/// (`<origin>/api/v1/mail/messages/<id>`); remote images load through `proxy` when allowed.
pub fn render(data: &[u8], base: &str, proxy: &str, images: bool) -> Value {
    let Some(m) = parse(data) else {
        return json!({ "html": "", "text": String::from_utf8_lossy(data), "attachments": [], "blockedImages": 0 });
    };
    let html_source = m.html_bodies().find(|p| p.is_text_html()).and_then(|p| p.text_contents()).map(str::to_owned);
    let mut blocked = 0;
    let html = html_source.map(|source| {
        let with_cid = cid_re().replace_all(&source, |c: &regex::Captures| format!("src=\"{base}/cid/{}\"", encode(&c[1]))).into_owned();
        let clean = sanitize(&with_cid, base, 2 * 1024 * 1024);
        img_re().replace_all(&clean, |c: &regex::Captures| {
            let src = &c[1];
            if src.starts_with(base) {
                c[0].to_owned()
            } else if images {
                format!("<img src=\"{proxy}{}\"", encode(&src.replace("&amp;", "&")))
            } else {
                blocked += 1;
                "<img data-blocked=\"1\"".to_owned()
            }
        }).into_owned()
    }).unwrap_or_default();
    let attachments: Vec<Value> = m.attachments().enumerate().map(|(i, a)| {
        let kind = a.content_type().map(|t| format!("{}/{}", t.ctype(), t.subtype().unwrap_or("octet-stream"))).unwrap_or_else(|| "application/octet-stream".into());
        json!({
            "index": i, "name": a.attachment_name().unwrap_or("attachment"), "type": kind.to_lowercase(), "size": a.contents().len(),
            "inline": a.content_id().is_some() && a.attachment_name().is_none(),
        })
    }).collect();
    let header = |name: &str| m.header(name).and_then(|h| h.as_text()).unwrap_or_default().to_owned();
    json!({
        "html": html,
        "text": if html.is_empty() { plain_text(&m) } else { String::new() },
        // The plain part as written (the Markdown source of a message written in Markdown).
        "textPart": m.text_bodies().find(|p| p.is_text() && !p.is_text_html()).and_then(|p| p.text_contents()).unwrap_or_default().replace("\r\n", "\n"),
        "format": if header("X-Dar-Format").trim().eq_ignore_ascii_case("markdown") { "markdown" } else { "plain" },
        "attachments": attachments,
        "blockedImages": blocked,
        "replyTo": addresses(m.reply_to()),
        "to": addresses(m.to()),
        "cc": addresses(m.cc()),
        "references": ids(m.references()),
        "listUnsubscribe": header("List-Unsubscribe"),
    })
}

/// An attachment's name, type and bytes, by its index or its Content-ID.
pub fn part(data: &[u8], index: Option<usize>, cid: Option<&str>) -> Option<(String, String, Vec<u8>)> {
    let m = parse(data)?;
    let a = match (index, cid) {
        (Some(i), _) => m.attachments().nth(i)?,
        (None, Some(cid)) => m.attachments().find(|a| a.content_id().is_some_and(|c| c.trim_matches(['<', '>']) == cid))?,
        _ => return None,
    };
    let kind = a.content_type().map(|t| format!("{}/{}", t.ctype(), t.subtype().unwrap_or("octet-stream"))).unwrap_or_else(|| "application/octet-stream".into());
    Some((a.attachment_name().unwrap_or("attachment").to_owned(), kind.to_lowercase(), a.contents().to_vec()))
}

pub struct Outgoing {
    pub data: Vec<u8>,
    pub message_id: String,
    pub recipients: Vec<String>,
}

pub struct Attachment {
    pub name: String,
    pub kind: String,
    pub data: Vec<u8>,
}

pub struct Draft<'a> {
    pub from_name: &'a str,
    pub from: &'a str,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: &'a str,
    pub text: &'a str,
    pub in_reply_to: Option<&'a str>,
    pub references: Vec<String>,
    pub attachments: Vec<Attachment>,
    pub date_ms: i64,
    pub id_seed: &'a str,
    /// Written in Markdown: sent with an HTML part made from it.
    pub markdown: bool,
}

/// A plain-text message, or Markdown with its HTML alternative (with attachments). Bcc
/// recipients are in the envelope only.
pub fn build(d: Draft<'_>) -> std::io::Result<Outgoing> {
    use mail_builder::MessageBuilder;
    use mail_builder::headers::address::Address as To;
    let domain = d.from.rsplit_once('@').map(|(_, h)| h).unwrap_or("dar.localhost");
    let message_id = format!("{}@{domain}", d.id_seed);
    let list = |v: &[String]| -> To<'static> { To::new_list(v.iter().map(|a| To::new_address(None::<String>, a.clone())).collect()) };
    let mut b = MessageBuilder::new()
        .from((d.from_name.to_owned(), d.from.to_owned()))
        .subject(d.subject.to_owned())
        .date(d.date_ms / 1000)
        .message_id(message_id.clone())
        .text_body(d.text.to_owned());
    if d.markdown {
        b = b.html_body(super::markdown::document(d.text)).header("X-Dar-Format", mail_builder::headers::text::Text::new("markdown"));
    }
    if !d.to.is_empty() {
        b = b.to(list(&d.to));
    }
    if !d.cc.is_empty() {
        b = b.cc(list(&d.cc));
    }
    if let Some(r) = d.in_reply_to.filter(|r| !r.is_empty()) {
        b = b.in_reply_to(r.to_owned());
    }
    if !d.references.is_empty() {
        b = b.references(d.references.clone());
    }
    for a in d.attachments {
        b = b.attachment(a.kind, a.name, a.data);
    }
    let data = b.write_to_vec()?;
    let recipients = d.to.iter().chain(&d.cc).chain(&d.bcc).cloned().collect();
    Ok(Outgoing { data, message_id, recipients })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"From: Lina Karam <lina@example.com>\r\nTo: You <you@example.com>\r\nSubject: Re: Venue\r\nDate: Sun, 22 Nov 2026 10:24:00 +0000\r\nMessage-ID: <c@example.com>\r\nReferences: <a@example.com> <b@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=x\r\n\r\n--x\r\nContent-Type: text/html\r\n\r\n<p onclick=\"x()\">Hi <img src=\"cid:logo\"><img src=\"https://tracker.example/p.gif\"></p><script>bad()</script>\r\n--x\r\nContent-Type: image/png\r\nContent-ID: <logo>\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--x--\r\n";

    #[test]
    fn indexes_threads_and_renders_safely() {
        let m = meta(SAMPLE, 0.0);
        assert_eq!(m["threadId"], "a@example.com");
        assert_eq!(m["messageId"], "c@example.com");
        assert_eq!(m["fromName"], "Lina Karam");
        assert_eq!(m["date"], 1795343040000.0);
        let r = render(SAMPLE, "https://dar.example/api/v1/mail/messages/1", "https://dar.example/api/v1/mail/image?url=", false);
        let html = r["html"].as_str().unwrap();
        assert!(html.contains("src=\"https://dar.example/api/v1/mail/messages/1/cid/logo\""), "{html}");
        assert!(!html.contains("tracker.example") && !html.contains("script") && !html.contains("onclick"), "{html}");
        assert_eq!(r["blockedImages"], 1);
        let (_, kind, bytes) = part(SAMPLE, None, Some("logo")).unwrap();
        assert_eq!(kind, "image/png");
        assert_eq!(&bytes[..4], b"\x89PNG");
    }

    #[test]
    fn builds_plain_messages() {
        let out = build(Draft {
            from_name: "Anas", from: "anas@example.com", to: vec!["a@example.com".into()], cc: vec![], bcc: vec!["hidden@example.com".into()],
            subject: "Hello", text: "Line one\nLine two", in_reply_to: Some("c@example.com"), references: vec!["a@example.com".into()],
            attachments: vec![], date_ms: 1_795_343_040_000, id_seed: "seed", markdown: false,
        }).unwrap();
        let text = String::from_utf8_lossy(&out.data);
        assert!(text.contains("Subject: Hello") && text.contains("In-Reply-To: <c@example.com>") && !text.contains("hidden@"), "{text}");
        assert_eq!(out.recipients, vec!["a@example.com", "hidden@example.com"]);
        assert_eq!(out.message_id, "seed@example.com");
        let md = build(Draft {
            from_name: "Anas", from: "anas@example.com", to: vec!["a@example.com".into()], cc: vec![], bcc: vec![], subject: "Plan", text: "Hello **team**\n\n- one",
            in_reply_to: None, references: vec![], attachments: vec![], date_ms: 1_795_343_040_000, id_seed: "md", markdown: true,
        }).unwrap();
        let r = render(&md.data, "https://dar.example/m", "", false);
        assert_eq!(r["format"], "markdown");
        assert_eq!(r["textPart"], "Hello **team**\n\n- one");
        assert!(r["html"].as_str().unwrap().contains("<strong>team</strong>"), "{r}");
        assert!(String::from_utf8_lossy(&md.data).contains("multipart/alternative"));
    }
}
