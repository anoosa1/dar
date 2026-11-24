//! RSS 2.0, Atom, RDF (RSS 1.0) and JSON Feed parser (`next/runtime/news/feed.ts`).

use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use chrono::{DateTime, NaiveDate, NaiveDateTime};
use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::digests::Md5;
use crate::error::{AppError, Result};
use crate::news::html::{self, absolute, escape_html, sanitize, text_of, trunc_utf16};

pub const LIMIT_FEEDS: usize = 500;
pub const LIMIT_ITEMS_PER_FEED: usize = 1000;
pub const LIMIT_TITLE: usize = 1000;
pub const LIMIT_BODY: usize = 200 * 1024;
pub const LIMIT_URL: usize = 2048;

const ATOM: &str = "http://www.w3.org/2005/Atom";
const MEDIA: &str = "http://search.yahoo.com/mrss/";

/// A parsed entry as stored by the fetcher. Bodies are sanitized HTML with absolute URLs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsEntry {
    pub guid: String,
    pub guid_hash: String,
    pub url: String,
    pub title: String,
    pub author: String,
    pub pub_date: i64,
    pub updated_date: i64,
    pub body: String,
    pub enclosure_mime: String,
    pub enclosure_link: String,
    pub media_thumbnail: String,
    pub media_description: String,
    pub rtl: bool,
    pub fingerprint: String,
    pub content_hash: String,
    pub excerpt: String,
}

#[derive(Clone, Debug)]
pub struct ParsedFeed {
    pub kind: &'static str,
    pub title: String,
    pub link: String,
    pub entries: Vec<NewsEntry>,
}

enum Child {
    Node(Node),
    Text(String),
}

struct Node {
    name: String,
    uri: String,
    attrs: Vec<(String, String)>,
    content: Vec<Child>,
}

impl Node {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn set_attr(&mut self, key: String, value: String) {
        if let Some((_, slot)) = self.attrs.iter_mut().find(|(k, _)| *k == key) {
            *slot = value;
        } else {
            self.attrs.push((key, value));
        }
    }
}

fn md5(value: &str) -> String {
    let mut h = Md5::default();
    h.update(value.as_bytes());
    h.hex()
}

fn not_a_feed() -> AppError {
    AppError::with(
        "notAFeed",
        422,
        "That address is not an RSS, Atom or JSON feed.",
    )
}

/// Windows-1252 mapping for bytes `0x80..=0x9f`.
const WIN1252: [char; 32] = [
    '\u{20ac}', '\u{0081}', '\u{201a}', '\u{0192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02c6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008d}', '\u{017d}', '\u{008f}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02dc}', '\u{2122}', '\u{0161}', '\u{203a}', '\u{0153}', '\u{009d}', '\u{017e}', '\u{0178}',
];

fn decode_windows_1252(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if (0x80..=0x9f).contains(&b) {
            out.push(WIN1252[(b - 0x80) as usize]);
        } else {
            out.push(b as char);
        }
    }
    out
}

fn decode_iso_8859_15(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        let ch = match b {
            0xa4 => '\u{20ac}',
            0xa6 => '\u{0160}',
            0xa8 => '\u{0161}',
            0xb4 => '\u{017d}',
            0xb8 => '\u{017e}',
            0xbc => '\u{0152}',
            0xbd => '\u{0153}',
            0xbe => '\u{0178}',
            other => other as char,
        };
        out.push(ch);
    }
    out
}

fn decode_utf16(bytes: &[u8], le: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| {
            if le {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

/// Decodes using the XML declaration or Content-Type charset; unknown labels fall back to UTF-8.
pub fn decode_body(body: &[u8], content_type: &str) -> String {
    static ENC_RE: OnceLock<Regex> = OnceLock::new();
    static CHARSET_RE: OnceLock<Regex> = OnceLock::new();
    let enc_re = ENC_RE.get_or_init(|| {
        Regex::new(r#"encoding=["']([A-Za-z0-9._-]{1,40})["']"#).expect("enc regex")
    });
    let charset_re = CHARSET_RE
        .get_or_init(|| Regex::new(r"(?i)charset=([A-Za-z0-9._-]{1,40})").expect("charset regex"));

    if body.starts_with(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8_lossy(&body[3..]).into_owned();
    }
    if body.starts_with(&[0xff, 0xfe]) {
        return decode_utf16(&body[2..], true);
    }
    if body.starts_with(&[0xfe, 0xff]) {
        return decode_utf16(&body[2..], false);
    }
    let head: String = body.iter().take(300).map(|&b| b as char).collect();
    let label = enc_re
        .captures(&head)
        .and_then(|c| c.get(1))
        .or_else(|| charset_re.captures(content_type).and_then(|c| c.get(1)))
        .map(|m| m.as_str().to_ascii_lowercase());

    match label.as_deref() {
        None | Some("utf-8" | "utf8" | "unicode-1-1-utf-8") => {
            String::from_utf8_lossy(body).into_owned()
        }
        Some(
            "windows-1252" | "cp1252" | "x-cp1252" | "iso-8859-1" | "iso8859-1" | "iso88591"
            | "iso_8859-1" | "iso_8859-1:1987" | "latin1" | "latin-1" | "l1" | "us-ascii" | "ascii"
            | "ansi_x3.4-1968",
        ) => decode_windows_1252(body),
        Some("iso-8859-15" | "iso8859-15" | "iso885915" | "iso_8859-15" | "latin9" | "l9") => {
            decode_iso_8859_15(body)
        }
        Some("utf-16le" | "utf-16") => decode_utf16(body, true),
        Some("utf-16be") => decode_utf16(body, false),
        _ => String::from_utf8_lossy(body).into_owned(),
    }
}

fn attach(stack: &mut [Node], root: &mut Option<Node>, node: Node) {
    if let Some(parent) = stack.last_mut() {
        parent.content.push(Child::Node(node));
    } else if root.is_none() {
        *root = Some(node);
    }
}

fn parse_xml(text: &str) -> Result<Node> {
    let mut reader = NsReader::from_str(text);
    // Real feeds contain undefined HTML entities and other recoverable mistakes; keep going.
    reader.config_mut().check_end_names = false;
    reader.config_mut().allow_unmatched_ends = true;
    reader.config_mut().allow_dangling_amp = true;
    reader.config_mut().check_comments = false;

    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    let mut nodes = 0usize;

    loop {
        let (resolved, event) = match reader.read_resolved_event() {
            Ok(pair) => pair,
            Err(_) => break,
        };
        let uri = match resolved {
            ResolveResult::Bound(ns) => ns.0.to_owned(),
            _ => String::new(),
        };
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                nodes += 1;
                if nodes > 200_000 || stack.len() > 64 {
                    return Err(AppError::with(
                        "feedTooLarge",
                        413,
                        "The feed is too large.",
                    ));
                }
                let mut node = Node {
                    name: e.local_name().as_ref().to_owned(),
                    uri,
                    attrs: Vec::new(),
                    content: Vec::new(),
                };
                let mut raw_attrs = e.attributes();
                raw_attrs.with_checks(false);
                for attr in raw_attrs.flatten() {
                    let raw_key = attr.key.as_ref();
                    if raw_key == "xmlns" || raw_key.starts_with("xmlns:") {
                        continue;
                    }
                    let local = attr.key.local_name().as_ref().to_owned();
                    let val = attr
                        .normalized_value_with(quick_xml::XmlVersion::Implicit1_0, 1, html::entity)
                        .map(|v| v.into_owned())
                        .unwrap_or_else(|_| html::decode(&attr.value));
                    node.set_attr(local, val);
                }
                if matches!(event, Event::Empty(_)) {
                    attach(&mut stack, &mut root, node);
                } else {
                    stack.push(node);
                }
            }
            Event::End(_) => {
                if let Some(node) = stack.pop() {
                    attach(&mut stack, &mut root, node);
                }
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.content
                        .push(Child::Text(t.xml10_content().into_owned()));
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    top.content
                        .push(Child::Text(t.xml10_content().into_owned()));
                }
            }
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref() {
                    Ok(Some(c)) => c.to_string(),
                    Ok(None) => quick_xml::escape::resolve_predefined_entity(&r)
                        .or_else(|| html::entity(&r))
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("&{};", &*r)),
                    Err(_) => format!("&{};", &*r),
                };
                if let Some(top) = stack.last_mut() {
                    top.content.push(Child::Text(resolved));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    while let Some(node) = stack.pop() {
        attach(&mut stack, &mut root, node);
    }
    root.ok_or_else(not_a_feed)
}

fn kids<'a>(node: Option<&'a Node>, name: Option<&str>, uri: Option<&str>) -> Vec<&'a Node> {
    let Some(node) = node else { return Vec::new() };
    node.content
        .iter()
        .filter_map(|c| match c {
            Child::Node(n)
                if name.is_none_or(|k| n.name == k) && uri.is_none_or(|u| n.uri == u) =>
            {
                Some(n)
            }
            _ => None,
        })
        .collect()
}

fn kid<'a>(node: Option<&'a Node>, name: &str, uri: Option<&str>) -> Option<&'a Node> {
    kids(node, Some(name), uri).into_iter().next()
}

fn text_in(node: Option<&Node>) -> String {
    let Some(node) = node else {
        return String::new();
    };
    let mut out = String::new();
    for c in &node.content {
        match c {
            Child::Text(t) => out.push_str(t),
            Child::Node(n) => out.push_str(&text_in(Some(n))),
        }
    }
    out
}

fn value(node: Option<&Node>, name: &str, uri: Option<&str>) -> String {
    text_in(kid(node, name, uri)).trim().to_owned()
}

fn serialize(node: &Node) -> String {
    let mut out = String::new();
    for c in &node.content {
        match c {
            Child::Text(t) => out.push_str(&escape_html(t)),
            Child::Node(n) => {
                out.push('<');
                out.push_str(&n.name);
                for (k, v) in &n.attrs {
                    out.push(' ');
                    out.push_str(k);
                    out.push_str("=\"");
                    out.push_str(&escape_html(v));
                    out.push('"');
                }
                out.push('>');
                out.push_str(&serialize(n));
                out.push_str("</");
                out.push_str(&n.name);
                out.push('>');
            }
        }
    }
    out
}

fn parse_one_date(raw: &str) -> i64 {
    static TZ_RE: OnceLock<Regex> = OnceLock::new();
    let tz_re = TZ_RE.get_or_init(|| Regex::new(r"\s+(UT|Z|UTC)$").expect("tz regex"));
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return 0;
    }
    let normalized = tz_re.replace(trimmed, " GMT");
    let candidate = normalized.as_ref();

    let ms = DateTime::parse_from_rfc3339(trimmed)
        .or_else(|_| DateTime::parse_from_rfc2822(candidate))
        .or_else(|_| DateTime::parse_from_rfc2822(trimmed))
        .map(|d| d.timestamp_millis())
        .ok()
        .or_else(|| {
            httpdate::parse_http_date(candidate)
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
        })
        .or_else(|| {
            // Common feed date variants: space instead of T, missing weekday, missing seconds, date-only.
            for fmt in [
                "%Y-%m-%d %H:%M:%S%.f%:z",
                "%Y-%m-%d %H:%M:%S%.f%#z",
                "%d %b %Y %H:%M:%S %z",
                "%a, %d %b %Y %H:%M %z",
            ] {
                if let Ok(dt) = DateTime::parse_from_str(trimmed, fmt) {
                    return Some(dt.timestamp_millis());
                }
            }
            for fmt in [
                "%Y-%m-%d %H:%M:%S%.f GMT",
                "%d %b %Y %H:%M:%S GMT",
                "%a, %d %b %Y %H:%M GMT",
                "%Y-%m-%dT%H:%M:%S%.f",
                "%Y-%m-%d %H:%M:%S%.f",
            ] {
                if let Ok(ndt) = NaiveDateTime::parse_from_str(candidate, fmt) {
                    return Some(ndt.and_utc().timestamp_millis());
                }
            }
            if let Ok(nd) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
                return nd
                    .and_hms_opt(0, 0, 0)
                    .map(|ndt| ndt.and_utc().timestamp_millis());
            }
            None
        });
    ms.filter(|&t| t > 0).unwrap_or(0)
}

fn parse_dates(values: &[&str]) -> i64 {
    for v in values {
        let t = parse_one_date(v);
        if t > 0 {
            return t;
        }
    }
    0
}

fn rtl_language(lang: &str) -> bool {
    static RTL_RE: OnceLock<Regex> = OnceLock::new();
    let re = RTL_RE.get_or_init(|| {
        Regex::new(r"(?i)^(ar|fa|he|iw|ur|ps|sd|yi|dv|ckb|ug)\b").expect("rtl regex")
    });
    re.is_match(lang.trim())
}

fn paragraphs(text: &str) -> String {
    static DOUBLE_NL: OnceLock<Regex> = OnceLock::new();
    let re = DOUBLE_NL.get_or_init(|| Regex::new(r"\n{2,}").expect("nl regex"));
    format!("<p>{}</p>", re.replace_all(&escape_html(text), "</p><p>"))
}

struct EntryInput<'a> {
    guid: &'a str,
    url: &'a str,
    title: &'a str,
    author: &'a str,
    pub_date: i64,
    updated_date: i64,
    html: &'a str,
    enclosure_mime: &'a str,
    enclosure_link: &'a str,
    thumbnail: &'a str,
    media_description: &'a str,
    rtl: bool,
}

fn make_entry(input: EntryInput<'_>, base: &str, fetched_at: i64) -> NewsEntry {
    let url = absolute(input.url, base, &["http", "https"]);
    let body_base = if !url.is_empty() { url.as_str() } else { base };
    let body = sanitize(input.html, body_base, LIMIT_BODY - 1024);
    let title = trunc_utf16(&text_of(input.title, 100_000), LIMIT_TITLE).to_owned();
    let guid_raw = if !input.guid.trim().is_empty() {
        input.guid.trim().to_owned()
    } else if !url.is_empty() {
        url.clone()
    } else {
        md5(&format!("{title}{body}"))
    };
    let guid = trunc_utf16(&guid_raw, LIMIT_URL).to_owned();
    let enclosure_link = absolute(input.enclosure_link, base, &["http", "https"]);
    let pub_date = if input.pub_date > 0 {
        input.pub_date
    } else {
        fetched_at
    };
    let updated_date = if input.updated_date > 0 {
        input.updated_date
    } else {
        pub_date
    };
    let author = trunc_utf16(&text_of(input.author, 100_000), 500).to_owned();
    let media_thumbnail = absolute(input.thumbnail, base, &["http", "https"]);
    let media_description =
        trunc_utf16(&text_of(input.media_description, 100_000), 4000).to_owned();
    let enclosure_mime = trunc_utf16(input.enclosure_mime, 200).to_owned();
    let guid_hash = md5(&guid);
    let fingerprint = md5(&format!("{title}\n{url}\n{body}\n{enclosure_link}"));
    let content_hash = md5(&format!(
        "{title}\n{author}\n{url}\n{body}\n{enclosure_link}\n{media_thumbnail}"
    ));
    let excerpt = trunc_utf16(&text_of(&body, 2000), 300).to_owned();

    NewsEntry {
        guid,
        guid_hash,
        url,
        title,
        author,
        pub_date,
        updated_date,
        body,
        enclosure_mime,
        enclosure_link,
        media_thumbnail,
        media_description,
        rtl: input.rtl,
        fingerprint,
        content_hash,
        excerpt,
    }
}

struct MediaInfo {
    thumbnail: String,
    description: String,
    link: String,
    media_type: String,
}

fn media(node: &Node) -> MediaInfo {
    let group = kid(Some(node), "group", None).unwrap_or(node);
    let content = kids(Some(group), Some("content"), None)
        .into_iter()
        .find(|c| {
            let t = c.attr("type").unwrap_or("");
            let m = c.attr("medium").unwrap_or("");
            t.starts_with("audio/") || t.starts_with("video/") || m == "audio" || m == "video"
        });
    let thumb = kid(Some(group), "thumbnail", None)
        .or_else(|| kid(Some(node), "thumbnail", None))
        .or_else(|| {
            kids(Some(group), Some("content"), None)
                .into_iter()
                .find(|c| {
                    c.attr("medium") == Some("image")
                        || c.attr("type").unwrap_or("").starts_with("image/")
                })
        });
    let image = kid(Some(node), "image", None);
    let thumbnail = thumb
        .and_then(|t| t.attr("url").or_else(|| t.attr("href")))
        .or_else(|| image.and_then(|i| i.attr("href")))
        .unwrap_or("")
        .to_owned();
    let description = value(Some(group), "description", Some(MEDIA));
    let link = content.and_then(|c| c.attr("url")).unwrap_or("").to_owned();
    let media_type = content
        .and_then(|c| {
            c.attr("type").map(str::to_owned).or_else(|| {
                c.attr("medium")
                    .filter(|m| !m.is_empty())
                    .map(|m| format!("{m}/*"))
            })
        })
        .unwrap_or_default();
    MediaInfo {
        thumbnail,
        description,
        link,
        media_type,
    }
}

fn hostname_of(feed_url: &str) -> String {
    url::Url::parse(feed_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

pub fn parse_feed(
    body: &[u8],
    content_type: &str,
    feed_url: &str,
    fetched_at: i64,
) -> Result<ParsedFeed> {
    static XML_START_RE: OnceLock<Regex> = OnceLock::new();
    static FEED_TAG_RE: OnceLock<Regex> = OnceLock::new();
    let xml_start_re = XML_START_RE.get_or_init(|| {
        Regex::new(r"(?i)^<(\?xml|rss|feed|rdf:RDF|!--|!DOCTYPE)").expect("xml start regex")
    });
    let feed_tag_re = FEED_TAG_RE
        .get_or_init(|| Regex::new(r"(?i)<(rss|feed|rdf:RDF)[\s>]").expect("feed tag regex"));

    let text = decode_body(body, content_type);
    let start = text.trim_start();
    if content_type.contains("json") || start.starts_with('{') {
        return parse_json_feed(start, feed_url, fetched_at);
    }
    let head = trunc_utf16(start, 4000);
    if !xml_start_re.is_match(start) || !feed_tag_re.is_match(head) {
        return Err(not_a_feed());
    }
    let root = parse_xml(start)?;
    if root.name == "feed" {
        let atom_link = |node: &Node, rel: &str| -> String {
            kids(Some(node), Some("link"), None)
                .into_iter()
                .find(|l| l.attr("rel").unwrap_or("alternate") == rel)
                .and_then(|l| l.attr("href"))
                .unwrap_or("")
                .to_owned()
        };
        let site = {
            let resolved = absolute(&atom_link(&root, "alternate"), feed_url, &["http", "https"]);
            if resolved.is_empty() {
                feed_url.to_owned()
            } else {
                resolved
            }
        };
        let rtl = rtl_language(root.attr("lang").unwrap_or(""));
        let atom_content = |node: Option<&Node>| -> String {
            let Some(node) = node else {
                return String::new();
            };
            let kind = node.attr("type").unwrap_or("text").to_ascii_lowercase();
            if kind == "xhtml" {
                serialize(kid(Some(node), "div", None).unwrap_or(node))
            } else if kind == "html" || kind == "text/html" {
                text_in(Some(node))
            } else {
                paragraphs(&text_in(Some(node)))
            }
        };
        let entries = kids(Some(&root), Some("entry"), Some(ATOM))
            .into_iter()
            .take(LIMIT_ITEMS_PER_FEED)
            .map(|e| {
                let enclosure = kids(Some(e), Some("link"), None)
                    .into_iter()
                    .find(|l| l.attr("rel") == Some("enclosure"));
                let m = media(e);
                let guid = value(Some(e), "id", None);
                let alt = atom_link(e, "alternate");
                let url = if !alt.is_empty() {
                    alt
                } else {
                    atom_link(e, "related")
                };
                let title = text_in(kid(Some(e), "title", None));
                let author = value(
                    kid(Some(e), "author", None).or_else(|| kid(Some(&root), "author", None)),
                    "name",
                    None,
                );
                let pub_s = value(Some(e), "published", None);
                let upd_s = value(Some(e), "updated", None);
                let iss_s = value(Some(e), "issued", None);
                let pub_date = parse_dates(&[&pub_s, &upd_s, &iss_s]);
                let updated_date = parse_dates(&[&upd_s]);
                let html_body = {
                    let c = atom_content(kid(Some(e), "content", None));
                    if !c.is_empty() {
                        c
                    } else {
                        atom_content(kid(Some(e), "summary", None))
                    }
                };
                let enclosure_mime = enclosure
                    .and_then(|l| l.attr("type"))
                    .unwrap_or(&m.media_type);
                let enclosure_link = enclosure.and_then(|l| l.attr("href")).unwrap_or(&m.link);
                make_entry(
                    EntryInput {
                        guid: &guid,
                        url: &url,
                        title: &title,
                        author: &author,
                        pub_date,
                        updated_date,
                        html: &html_body,
                        enclosure_mime,
                        enclosure_link,
                        thumbnail: &m.thumbnail,
                        media_description: &m.description,
                        rtl: rtl || rtl_language(e.attr("lang").unwrap_or("")),
                    },
                    &site,
                    fetched_at,
                )
            })
            .collect();
        let raw_title = text_of(&text_in(kid(Some(&root), "title", None)), 100_000);
        let title = trunc_utf16(&raw_title, LIMIT_TITLE);
        let title = if title.is_empty() {
            hostname_of(feed_url)
        } else {
            title.to_owned()
        };
        return Ok(ParsedFeed {
            kind: "Atom",
            title,
            link: site,
            entries,
        });
    }

    let channel = if root.name == "rss" || root.name == "RDF" {
        kid(Some(&root), "channel", None)
    } else {
        None
    };
    let Some(channel) = channel else {
        return Err(not_a_feed());
    };
    let channel_link = kids(Some(channel), Some("link"), None)
        .into_iter()
        .find(|l| l.uri != ATOM)
        .map(|l| text_in(Some(l)).trim().to_owned())
        .unwrap_or_default();
    let site = {
        let resolved = absolute(&channel_link, feed_url, &["http", "https"]);
        if resolved.is_empty() {
            feed_url.to_owned()
        } else {
            resolved
        }
    };
    let rtl = rtl_language(&value(Some(channel), "language", None));
    let item_nodes = if root.name == "RDF" {
        kids(Some(&root), Some("item"), None)
    } else {
        kids(Some(channel), Some("item"), None)
    };
    let entries = item_nodes
        .into_iter()
        .take(LIMIT_ITEMS_PER_FEED)
        .map(|i| {
            let enclosure = kid(Some(i), "enclosure", None);
            let m = media(i);
            let guid_node = kid(Some(i), "guid", None);
            let link_node = kids(Some(i), Some("link"), None)
                .into_iter()
                .find(|l| l.uri != ATOM);
            let guid_text = text_in(guid_node).trim().to_owned();
            let link_text = text_in(link_node).trim().to_owned();
            let about = i.attr("about").unwrap_or("");
            let url = if !link_text.is_empty() {
                link_text
            } else if guid_node.is_some_and(|g| g.attr("isPermaLink") != Some("false"))
                && (guid_text.starts_with("http:") || guid_text.starts_with("https:"))
            {
                guid_text.clone()
            } else {
                about.to_owned()
            };
            let guid = if !guid_text.is_empty() {
                guid_text
            } else {
                about.to_owned()
            };
            let title = text_in(kid(Some(i), "title", None));
            let author = {
                let c = value(Some(i), "creator", None);
                if !c.is_empty() {
                    c
                } else {
                    value(Some(i), "author", None)
                }
            };
            let d1 = value(Some(i), "pubDate", None);
            let d2 = value(Some(i), "date", None);
            let d3 = value(Some(i), "published", None);
            let u1 = value(Some(i), "updated", None);
            let u2 = value(Some(i), "modified", None);
            let pub_date = parse_dates(&[&d1, &d2, &d3]);
            let updated_date = parse_dates(&[&u1, &u2]);
            let html_body = {
                let enc = value(Some(i), "encoded", None);
                if !enc.is_empty() {
                    enc
                } else {
                    text_in(kid(Some(i), "description", None))
                }
            };
            let enclosure_mime = enclosure
                .and_then(|e| e.attr("type"))
                .unwrap_or(&m.media_type);
            let enclosure_link = enclosure.and_then(|e| e.attr("url")).unwrap_or(&m.link);
            let thumbnail = if !m.thumbnail.is_empty() {
                m.thumbnail.as_str()
            } else {
                kid(Some(i), "image", None)
                    .and_then(|img| img.attr("href"))
                    .unwrap_or("")
            };
            make_entry(
                EntryInput {
                    guid: &guid,
                    url: &url,
                    title: &title,
                    author: &author,
                    pub_date,
                    updated_date,
                    html: &html_body,
                    enclosure_mime,
                    enclosure_link,
                    thumbnail,
                    media_description: &m.description,
                    rtl,
                },
                &site,
                fetched_at,
            )
        })
        .collect();

    let raw_title = text_of(&value(Some(channel), "title", None), 100_000);
    let title = trunc_utf16(&raw_title, LIMIT_TITLE);
    let title = if title.is_empty() {
        hostname_of(feed_url)
    } else {
        title.to_owned()
    };
    Ok(ParsedFeed {
        kind: "RSS",
        title,
        link: site,
        entries,
    })
}

fn json_str(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or("")
}

fn json_id(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(other) => other.to_string(),
    }
}

fn parse_json_feed(text: &str, feed_url: &str, fetched_at: i64) -> Result<ParsedFeed> {
    let data: Value = serde_json::from_str(text).map_err(|_| not_a_feed())?;
    let Some(obj) = data.as_object() else {
        return Err(not_a_feed());
    };
    if !obj
        .get("version")
        .and_then(Value::as_str)
        .is_some_and(|v| v.starts_with("https://jsonfeed.org/version/"))
    {
        return Err(not_a_feed());
    }
    let Some(items) = obj.get("items").and_then(Value::as_array) else {
        return Err(not_a_feed());
    };
    let site = {
        let resolved = absolute(
            json_str(obj.get("home_page_url")),
            feed_url,
            &["http", "https"],
        );
        if resolved.is_empty() {
            feed_url.to_owned()
        } else {
            resolved
        }
    };
    let rtl = rtl_language(json_str(obj.get("language")));
    let entries = items
        .iter()
        .take(LIMIT_ITEMS_PER_FEED)
        .filter_map(Value::as_object)
        .map(|i| {
            let author = i
                .get("authors")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .or_else(|| i.get("author"))
                .and_then(Value::as_object)
                .map(|a| json_str(a.get("name")))
                .unwrap_or("");
            let attachment = i
                .get("attachments")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_object);
            let html_body = {
                let ch = json_str(i.get("content_html"));
                if !ch.is_empty() {
                    ch.to_owned()
                } else {
                    let ct = json_str(i.get("content_text"));
                    if !ct.is_empty() {
                        paragraphs(ct)
                    } else {
                        escape_html(json_str(i.get("summary")))
                    }
                }
            };
            let guid = json_id(i.get("id"));
            let url = {
                let u = json_str(i.get("url"));
                if !u.is_empty() {
                    u
                } else {
                    json_str(i.get("external_url"))
                }
            };
            let pub_s = json_str(i.get("date_published"));
            let mod_s = json_str(i.get("date_modified"));
            let thumb = {
                let img = json_str(i.get("image"));
                if !img.is_empty() {
                    img
                } else {
                    json_str(i.get("banner_image"))
                }
            };
            make_entry(
                EntryInput {
                    guid: &guid,
                    url,
                    title: json_str(i.get("title")),
                    author,
                    pub_date: parse_dates(&[pub_s, mod_s]),
                    updated_date: parse_dates(&[mod_s]),
                    html: &html_body,
                    enclosure_mime: attachment
                        .map(|a| json_str(a.get("mime_type")))
                        .unwrap_or(""),
                    enclosure_link: attachment.map(|a| json_str(a.get("url"))).unwrap_or(""),
                    thumbnail: thumb,
                    media_description: json_str(i.get("summary")),
                    rtl: rtl || rtl_language(json_str(i.get("language"))),
                },
                &site,
                fetched_at,
            )
        })
        .collect();
    let title = trunc_utf16(json_str(obj.get("title")), LIMIT_TITLE);
    let title = if title.is_empty() {
        hostname_of(feed_url)
    } else {
        title.to_owned()
    };
    Ok(ParsedFeed {
        kind: "JSON Feed",
        title,
        link: site,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rss_atom_rdf_json_and_windows_1252() {
        let rss_xml = br#"<?xml version="1.0"?><rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:media="http://search.yahoo.com/mrss/"><channel><title>Example &amp; Co</title><link>https://example.com/</link><language>ar</language>
    <item><title>First</title><link>/posts/1</link><guid isPermaLink="false">post-1</guid><pubDate>Tue, 10 Nov 2026 09:00:00 GMT</pubDate><dc:creator>Sara</dc:creator><description>Short</description><content:encoded><![CDATA[<p>Full <script>x</script>body</p>]]></content:encoded>
    <enclosure url="https://cdn.example.com/ep1.mp3" type="audio/mpeg" length="1"/><media:thumbnail url="https://cdn.example.com/t.jpg"/></item>
    <item><title>Second &nbsp; entity</title><link>https://example.com/posts/2</link><description>&lt;b&gt;Bold&lt;/b&gt; summary</description></item></channel></rss>"#;
        let rss = parse_feed(
            rss_xml,
            "application/rss+xml",
            "https://example.com/feed",
            1000,
        )
        .unwrap();
        assert_eq!(rss.kind, "RSS");
        assert_eq!(rss.title, "Example & Co");
        assert_eq!(rss.entries.len(), 2);
        let (first, second) = (&rss.entries[0], &rss.entries[1]);
        assert_eq!(first.url, "https://example.com/posts/1");
        assert_eq!(first.guid, "post-1");
        assert_eq!(first.author, "Sara");
        assert_eq!(
            first.pub_date,
            DateTime::parse_from_rfc3339("2026-11-10T09:00:00Z")
                .unwrap()
                .timestamp_millis()
        );
        assert_eq!(first.body, "<p>Full body</p>");
        assert_eq!(first.enclosure_link, "https://cdn.example.com/ep1.mp3");
        assert_eq!(first.enclosure_mime, "audio/mpeg");
        assert_eq!(first.media_thumbnail, "https://cdn.example.com/t.jpg");
        assert!(first.rtl);
        assert_eq!(first.guid_hash.len(), 32);
        assert_eq!(second.guid, "https://example.com/posts/2");
        assert_eq!(second.title, "Second entity");
        assert_eq!(second.body, "<b>Bold</b> summary");
        assert_eq!(second.pub_date, 1000);

        let atom_xml = br#"<feed xmlns="http://www.w3.org/2005/Atom"><title type="text">Atom site</title><link href="https://atom.example/"/><link rel="self" href="https://atom.example/feed"/>
    <entry><id>tag:atom.example,2026:1</id><title type="html">A &lt;em&gt;title&lt;/em&gt;</title><link rel="alternate" href="https://atom.example/1"/><link rel="enclosure" href="https://atom.example/v.mp4" type="video/mp4"/><updated>2026-11-11T10:00:00Z</updated><author><name>Omar</name></author>
    <content type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml"><p>XHTML <a href="/x">link</a></p></div></content></entry></feed>"#;
        let atom = parse_feed(
            atom_xml,
            "application/atom+xml",
            "https://atom.example/feed",
            1000,
        )
        .unwrap();
        assert_eq!(atom.kind, "Atom");
        assert_eq!(atom.link, "https://atom.example/");
        let a = &atom.entries[0];
        assert_eq!(a.title, "A title");
        assert_eq!(a.author, "Omar");
        assert_eq!(a.enclosure_mime, "video/mp4");
        assert!(
            a.body
                .contains(r#"<p>XHTML <a href="https://atom.example/x">link</a></p>"#)
        );
        assert_eq!(
            a.pub_date,
            DateTime::parse_from_rfc3339("2026-11-11T10:00:00Z")
                .unwrap()
                .timestamp_millis()
        );

        let rdf_xml = br#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns="http://purl.org/rss/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/"><channel rdf:about="https://rdf.example/"><title>RDF</title><link>https://rdf.example/</link></channel><item rdf:about="https://rdf.example/a"><title>Item A</title><link>https://rdf.example/a</link><dc:date>2026-11-09T08:00:00Z</dc:date></item></rdf:RDF>"#;
        let rdf = parse_feed(
            rdf_xml,
            "application/rdf+xml",
            "https://rdf.example/rss",
            1000,
        )
        .unwrap();
        assert_eq!(rdf.entries[0].url, "https://rdf.example/a");
        assert_eq!(
            rdf.entries[0].pub_date,
            DateTime::parse_from_rfc3339("2026-11-09T08:00:00Z")
                .unwrap()
                .timestamp_millis()
        );

        let json_body = serde_json::json!({
            "version": "https://jsonfeed.org/version/1.1",
            "title": "JSON",
            "home_page_url": "https://json.example/",
            "items": [{ "id": 7, "url": "https://json.example/7", "title": "Seven", "content_text": "Plain\n\nText", "date_published": "2026-11-08T00:00:00Z", "authors": [{ "name": "Ali" }], "attachments": [{ "url": "https://json.example/a.mp3", "mime_type": "audio/mpeg" }] }]
        })
        .to_string();
        let jf = parse_feed(
            json_body.as_bytes(),
            "application/feed+json",
            "https://json.example/feed.json",
            1000,
        )
        .unwrap();
        assert_eq!(jf.kind, "JSON Feed");
        assert_eq!(jf.entries[0].guid, "7");
        assert_eq!(jf.entries[0].body, "<p>Plain</p><p>Text</p>");
        assert_eq!(jf.entries[0].author, "Ali");
        assert_eq!(jf.entries[0].enclosure_link, "https://json.example/a.mp3");

        let mut latin = Vec::new();
        latin.extend_from_slice(
            br#"<?xml version="1.0" encoding="windows-1252"?><rss><channel><title>Caf"#,
        );
        latin.push(0xe9);
        latin.extend_from_slice(br#"</title><item><title>x</title><link>https://l.example/</link></item></channel></rss>"#);
        assert_eq!(
            parse_feed(&latin, "text/xml", "https://l.example/f", 1000)
                .unwrap()
                .title,
            "Caf\u{00e9}"
        );

        let err = parse_feed(
            b"<html><body>hi</body></html>",
            "text/html",
            "https://x.example/",
            1000,
        )
        .unwrap_err();
        assert_eq!(err.code, "notAFeed");
    }
}
