//! Book format detection, XML parsing, EPUB package/TOC reading, and ComicInfo.xml parsing
//! (`next/runtime/books/formats.ts` and `next/shared/books.ts`).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::OnceLock;

use quick_xml::Reader;
use quick_xml::events::Event;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BookFormat {
    Epub,
    Pdf,
    Cbz,
    Cbr,
    Cb7,
    Cbt,
}

impl BookFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Epub => "epub",
            Self::Pdf => "pdf",
            Self::Cbz => "cbz",
            Self::Cbr => "cbr",
            Self::Cb7 => "cb7",
            Self::Cbt => "cbt",
        }
    }
}

/// The book format from a file name; hidden files and other types are not books.
pub fn book_format(name: &str) -> Option<BookFormat> {
    if name.starts_with('.') {
        return None;
    }
    let (_, ext) = name.rsplit_once('.')?;
    if ext.is_empty() || ext.len() > 8 || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    match ext.to_ascii_lowercase().as_str() {
        "epub" => Some(BookFormat::Epub),
        "pdf" => Some(BookFormat::Pdf),
        "cbz" => Some(BookFormat::Cbz),
        "cbr" => Some(BookFormat::Cbr),
        "cb7" => Some(BookFormat::Cb7),
        "cbt" => Some(BookFormat::Cbt),
        _ => None,
    }
}

/// File types other readers use that Dar recognizes but cannot open.
pub fn unsupported_book(name: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\.(mobi|azw3?|kfx|prc|lit|fb2|djvu)$").expect("regex"))
        .is_match(name)
}

/// Validated stored reading position; anything else reads as `None` (`bookPosition`).
pub fn book_position(value: &Value) -> Option<Value> {
    let obj = value.as_object()?;
    if obj.keys().any(|k| !matches!(k.as_str(), "spine" | "page" | "fraction")) {
        return None;
    }
    let valid_index = |v: Option<&Value>| match v {
        None => true,
        Some(n) => n.as_f64().is_some_and(|f| f.is_finite() && f.fract() == 0.0 && (0.0..1_000_000.0).contains(&f)),
    };
    if !valid_index(obj.get("spine"))
        || !valid_index(obj.get("page"))
        || obj.get("spine").is_none() == obj.get("page").is_none()
    {
        return None;
    }
    let fraction = obj.get("fraction")?.as_f64().filter(|f| f.is_finite() && (0.0..=1.0).contains(f))?;
    Some(match obj.get("spine") {
        Some(spine) => json!({ "spine": spine, "fraction": fraction }),
        None => json!({ "page": obj["page"], "fraction": fraction }),
    })
}

fn serialize_series_index<S: serde::Serializer>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
    match value {
        None => serializer.serialize_none(),
        Some(v) if v.is_finite() && v.fract() == 0.0 && *v >= i64::MIN as f64 && *v <= i64::MAX as f64 => {
            serializer.serialize_i64(*v as i64)
        }
        Some(v) if v.is_finite() => serializer.serialize_f64(*v),
        Some(_) => serializer.serialize_none(),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookMeta {
    pub title: String,
    pub authors: Vec<String>,
    pub series: String,
    #[serde(serialize_with = "serialize_series_index")]
    pub series_index: Option<f64>,
    pub language: String,
    pub publisher: String,
    pub published: String,
    pub description: String,
    pub subjects: Vec<String>,
    pub pages: u64,
    pub cover: bool,
    pub right_to_left: bool,
    pub error: String,
}

pub fn empty_meta(title: impl Into<String>) -> BookMeta {
    BookMeta {
        title: title.into(),
        authors: Vec::new(),
        series: String::new(),
        series_index: None,
        language: String::new(),
        publisher: String::new(),
        published: String::new(),
        description: String::new(),
        subjects: Vec::new(),
        pages: 0,
        cover: false,
        right_to_left: false,
        error: String::new(),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpineItem {
    pub href: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BookTocEntry {
    pub label: String,
    pub href: String,
    pub depth: usize,
    pub spine: isize,
}

pub struct EpubPackage {
    pub meta: BookMeta,
    pub opf: String,
    pub spine: Vec<SpineItem>,
    pub toc: Vec<BookTocEntry>,
    pub cover: Option<String>,
    pub encrypted: bool,
}

// ---------- Lenient XML tree ----------

#[derive(Debug)]
pub struct Node {
    pub name: String,
    pub attrs: HashMap<String, String>,
    pub content: Vec<Content>,
}

#[derive(Debug)]
pub enum Content {
    Node(Node),
    Text(String),
}

impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }
}

fn resolve_entity(name: &str) -> Option<&'static str> {
    if let Some(predefined) = quick_xml::escape::resolve_predefined_entity(name) {
        return Some(predefined);
    }
    match name {
        "nbsp" => Some("\u{00a0}"),
        "ndash" => Some("\u{2013}"),
        "mdash" => Some("\u{2014}"),
        "lsquo" => Some("\u{2018}"),
        "rsquo" => Some("\u{2019}"),
        "ldquo" => Some("\u{201c}"),
        "rdquo" => Some("\u{201d}"),
        "hellip" => Some("\u{2026}"),
        "middot" => Some("\u{00b7}"),
        "copy" => Some("\u{00a9}"),
        "reg" => Some("\u{00ae}"),
        "trade" => Some("\u{2122}"),
        "bull" => Some("\u{2022}"),
        "laquo" => Some("\u{00ab}"),
        "raquo" => Some("\u{00bb}"),
        "times" => Some("\u{00d7}"),
        "divide" => Some("\u{00f7}"),
        "deg" => Some("\u{00b0}"),
        "sect" => Some("\u{00a7}"),
        "para" => Some("\u{00b6}"),
        _ => None,
    }
}

fn make_node(e: &quick_xml::events::BytesStart<'_>) -> Node {
    let name_binding = e.name();
    let raw_name: &str = name_binding.as_ref();
    let local = match raw_name.find(':') {
        Some(colon) => &raw_name[colon + 1..],
        None => raw_name,
    };
    let mut attrs = HashMap::new();
    for attr in e.html_attributes().with_checks(false) {
        let Ok(attr) = attr else { continue };
        let key = attr.key.as_ref().to_ascii_lowercase();
        let value = match attr.normalized_value_with(quick_xml::XmlVersion::Implicit1_0, 1, resolve_entity) {
            Ok(v) => v.into_owned(),
            Err(_) => attr.value.into_owned(),
        };
        attrs.insert(key, value);
    }
    Node { name: local.to_ascii_lowercase(), attrs, content: Vec::new() }
}

fn attach(stack: &mut [Node], root: &mut Option<Node>, node: Node) {
    if let Some(parent) = stack.last_mut() {
        parent.content.push(Content::Node(node));
    } else if root.is_none() {
        *root = Some(node);
    }
}

/// A small XML tree. Book files often carry HTML entities or other recoverable mistakes;
/// parsing continues past them and keeps whatever was read before hitting limits or errors.
pub fn parse_xml(text: &str, max_nodes: usize) -> Option<Node> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut reader = Reader::from_str(text);
    reader.config_mut().allow_dangling_amp = true;
    reader.config_mut().allow_unmatched_ends = true;
    reader.config_mut().check_end_names = false;
    reader.config_mut().check_comments = false;

    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    let mut nodes = 0usize;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                nodes += 1;
                if nodes > max_nodes || stack.len() > 100 {
                    break;
                }
                stack.push(make_node(e));
            }
            Ok(Event::Empty(ref e)) => {
                nodes += 1;
                if nodes > max_nodes || stack.len() > 100 {
                    break;
                }
                let node = make_node(e);
                attach(&mut stack, &mut root, node);
            }
            Ok(Event::End(_)) => {
                if let Some(node) = stack.pop() {
                    attach(&mut stack, &mut root, node);
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(top) = stack.last_mut() {
                    top.content.push(Content::Text(t.xml10_content().into_owned()));
                }
            }
            Ok(Event::CData(t)) => {
                if let Some(top) = stack.last_mut() {
                    top.content.push(Content::Text(t.xml10_content().into_owned()));
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if let Some(top) = stack.last_mut() {
                    let piece = match r.resolve_char_ref() {
                        Ok(Some(c)) => c.to_string(),
                        Ok(None) => resolve_entity(&r).map(str::to_owned).unwrap_or_else(|| format!("&{};", &*r)),
                        Err(_) => format!("&{};", &*r),
                    };
                    top.content.push(Content::Text(piece));
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }

    while let Some(node) = stack.pop() {
        attach(&mut stack, &mut root, node);
    }
    root
}

fn kids<'a>(node: Option<&'a Node>, name: Option<&str>) -> Vec<&'a Node> {
    let Some(node) = node else { return Vec::new() };
    node.content
        .iter()
        .filter_map(|c| match c {
            Content::Node(child) if name.is_none_or(|n| child.name == n) => Some(child),
            _ => None,
        })
        .collect()
}

fn kid<'a>(node: Option<&'a Node>, name: &str) -> Option<&'a Node> {
    let node = node?;
    node.content.iter().find_map(|c| match c {
        Content::Node(child) if child.name == name => Some(child),
        _ => None,
    })
}

fn walk<'a>(node: Option<&'a Node>, out: &mut Vec<&'a Node>) {
    let Some(node) = node else { return };
    out.push(node);
    for child in &node.content {
        if let Content::Node(n) = child {
            walk(Some(n), out);
        }
    }
}

pub fn text_of(node: Option<&Node>) -> String {
    let Some(node) = node else { return String::new() };
    let mut out = String::new();
    fn collect(node: &Node, out: &mut String) {
        for item in &node.content {
            match item {
                Content::Text(s) => out.push_str(s),
                Content::Node(child) => collect(child, out),
            }
        }
    }
    collect(node, &mut out);
    out
}

pub(crate) fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn clean(value: &str, max: usize) -> String {
    static WS: OnceLock<Regex> = OnceLock::new();
    let ws = WS.get_or_init(|| Regex::new(r"\s+").expect("regex"));
    truncate_chars(ws.replace_all(value, " ").trim(), max)
}

/// Book descriptions are often HTML, sometimes escaped twice; keep plain paragraphs.
pub fn plain_description(value: &str) -> String {
    static ESCAPED_TAG: OnceLock<Regex> = OnceLock::new();
    static BLOCK_TAG: OnceLock<Regex> = OnceLock::new();
    static ANY_TAG: OnceLock<Regex> = OnceLock::new();
    static DEC_ENTITY: OnceLock<Regex> = OnceLock::new();
    static LINE_WS: OnceLock<Regex> = OnceLock::new();

    let escaped_tag = ESCAPED_TAG.get_or_init(|| Regex::new(r"(?i)&lt;/?[a-z]").expect("regex"));
    let block_tag = BLOCK_TAG.get_or_init(|| Regex::new(r"(?i)<\s*(br|/p|/div|/li)\b[^>]*>").expect("regex"));
    let any_tag = ANY_TAG.get_or_init(|| Regex::new(r"<[^>]*>").expect("regex"));
    let dec_entity = DEC_ENTITY.get_or_init(|| Regex::new(r"&#(\d+);").expect("regex"));
    let line_ws = LINE_WS.get_or_init(|| Regex::new(r"[ \t]+").expect("regex"));

    let unescaped = if escaped_tag.is_match(value) {
        value
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&amp;", "&")
    } else {
        value.to_owned()
    };

    let with_newlines = block_tag.replace_all(&unescaped, "\n");
    let stripped = any_tag.replace_all(&with_newlines, "");
    let decoded = stripped
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"");
    let decoded = dec_entity.replace_all(&decoded, |caps: &regex::Captures<'_>| {
        caps.get(1)
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .filter(|&code| code > 0 && code < 0x11_0000)
            .and_then(char::from_u32)
            .map(|c| c.to_string())
            .unwrap_or_default()
    });

    let joined = decoded
        .split('\n')
        .map(|line| line_ws.replace_all(line, " ").trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    truncate_chars(&joined, 4000)
}

fn is_rtl(lang: &str) -> bool {
    static RTL: OnceLock<Regex> = OnceLock::new();
    RTL.get_or_init(|| Regex::new(r"(?i)^(ar|fa|he|iw|ur|ps|sd|yi|dv|ckb|ug)\b").expect("regex"))
        .is_match(lang.trim())
}

/// Percent-decodes UTF-8 like JavaScript's `decodeURIComponent` (leaves `+` untouched).
pub(crate) fn percent_decode_utf8(input: &str) -> Result<String, ()> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(());
            }
            let hi = hex_nibble(bytes[i + 1]).ok_or(())?;
            let lo = hex_nibble(bytes[i + 2]).ok_or(())?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn posix_dirname(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.starts_with('/') { "/" } else { "." };
    }
    match trimmed.rfind('/') {
        Some(0) => "/",
        Some(idx) => &trimmed[..idx],
        None => ".",
    }
}

fn posix_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let absolute = path.starts_with('/');
    let trailing_slash = path.ends_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            if let Some(&last) = parts.last()
                && last != ".."
            {
                parts.pop();
            } else if !absolute {
                parts.push("..");
            }
        } else {
            parts.push(seg);
        }
    }
    let mut out = String::new();
    if absolute {
        out.push('/');
    }
    out.push_str(&parts.join("/"));
    if trailing_slash && !out.ends_with('/') && !out.is_empty() {
        out.push('/');
    }
    if out.is_empty() {
        if absolute { "/".into() } else { ".".into() }
    } else {
        out
    }
}

/// Resolves a manifest href against its document; `None` when it leaves the archive.
pub fn resolve_href(base: &str, href: &str) -> Option<String> {
    static SCHEME: OnceLock<Regex> = OnceLock::new();
    let scheme = SCHEME.get_or_init(|| Regex::new(r"(?i)^[a-z][a-z0-9+.-]*:").expect("regex"));

    let raw = href.split('#').next().unwrap_or("");
    let target = percent_decode_utf8(raw).unwrap_or_else(|_| raw.to_owned());
    if target.is_empty() || scheme.is_match(&target) {
        return None;
    }
    let dir = posix_dirname(base);
    let combined = if dir == "." {
        // Match Node's path.posix.join(".", target): leading slash on target is joined, not absolute.
        format!("./{target}")
    } else {
        format!("{dir}/{target}")
    };
    let joined = posix_normalize(&combined);
    if joined.starts_with("../") || joined == ".." || joined == "." || joined.starts_with('/') {
        None
    } else {
        Some(joined)
    }
}

struct ManifestItem {
    href: String,
    media_type: String,
    properties: String,
}

/// `container.xml` -> package document: metadata, reading order, cover and table of contents.
pub async fn read_epub<G, R, Fut>(get: G, read: R, fallback_title: &str) -> Result<EpubPackage, String>
where
    G: Fn(&str) -> Option<u64>,
    R: Fn(&str, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    static OEBPS_PKG: OnceLock<Regex> = OnceLock::new();
    static FONT_OBFUSCATION: OnceLock<Regex> = OnceLock::new();
    static SPINE_MIME: OnceLock<Regex> = OnceLock::new();
    static IMAGE_MIME: OnceLock<Regex> = OnceLock::new();
    static COVER_PROP: OnceLock<Regex> = OnceLock::new();
    static COVER_NAME: OnceLock<Regex> = OnceLock::new();
    static NAV_PROP: OnceLock<Regex> = OnceLock::new();
    static TOC_TYPE: OnceLock<Regex> = OnceLock::new();

    let oebps_pkg = OEBPS_PKG.get_or_init(|| Regex::new(r"oebps-package|^$").expect("regex"));
    let font_obf = FONT_OBFUSCATION
        .get_or_init(|| Regex::new(r"idpf\.org/2008/embedding|ns\.adobe\.com/pdf/enc#RC").expect("regex"));
    let spine_mime = SPINE_MIME.get_or_init(|| Regex::new(r"x?html|xml").expect("regex"));
    let image_mime = IMAGE_MIME.get_or_init(|| Regex::new(r"^image/(jpeg|png|gif|webp|svg\+xml)$").expect("regex"));
    let cover_prop = COVER_PROP.get_or_init(|| Regex::new(r"\bcover-image\b").expect("regex"));
    let cover_name = COVER_NAME.get_or_init(|| Regex::new(r"(?i)cover").expect("regex"));
    let nav_prop = NAV_PROP.get_or_init(|| Regex::new(r"\bnav\b").expect("regex"));
    let toc_type = TOC_TYPE.get_or_init(|| Regex::new(r"\btoc\b").expect("regex"));

    let container_bytes = read("META-INF/container.xml", 256 * 1024).await?;
    let container = parse_xml(&String::from_utf8_lossy(&container_bytes), 200_000);
    let mut container_nodes = Vec::new();
    walk(container.as_ref(), &mut container_nodes);
    let opf = container_nodes
        .into_iter()
        .find(|n| {
            n.name == "rootfile"
                && oebps_pkg.is_match(n.attr("media-type").unwrap_or(""))
                && n.attr("full-path").is_some_and(|p| !p.is_empty())
        })
        .and_then(|n| n.attr("full-path"))
        .map(str::to_owned);
    let Some(opf) = opf.filter(|p| get(p).is_some()) else {
        return Err("No package document.".into());
    };

    // Font obfuscation is not DRM; any other encrypted resource is.
    let mut encrypted = false;
    if get("META-INF/encryption.xml").is_some() {
        let enc_bytes = read("META-INF/encryption.xml", 1024 * 1024).await?;
        let doc = parse_xml(&String::from_utf8_lossy(&enc_bytes), 200_000);
        let mut enc_nodes = Vec::new();
        walk(doc.as_ref(), &mut enc_nodes);
        encrypted = enc_nodes
            .into_iter()
            .any(|n| n.name == "encryptionmethod" && !font_obf.is_match(n.attr("algorithm").unwrap_or("")));
    }

    let pkg_bytes = read(&opf, 4 * 1024 * 1024).await?;
    let Some(pkg) = parse_xml(&String::from_utf8_lossy(&pkg_bytes), 200_000) else {
        return Err("Damaged package document.".into());
    };

    let metadata = kid(Some(&pkg), "metadata");
    let manifest = kid(Some(&pkg), "manifest");
    let spine_node = kid(Some(&pkg), "spine");

    // Preserve manifest insertion order while supporting O(1) id lookup.
    let mut item_list: Vec<ManifestItem> = Vec::new();
    let mut item_by_id: HashMap<String, usize> = HashMap::new();
    for item in kids(manifest, Some("item")) {
        let href = resolve_href(&opf, item.attr("href").unwrap_or(""));
        if let (Some(id), Some(href)) = (item.attr("id").filter(|s| !s.is_empty()), href) {
            let entry = ManifestItem {
                href,
                media_type: item.attr("media-type").unwrap_or("").to_owned(),
                properties: item.attr("properties").unwrap_or("").to_owned(),
            };
            if let Some(&idx) = item_by_id.get(id) {
                item_list[idx] = entry;
            } else {
                item_by_id.insert(id.to_owned(), item_list.len());
                item_list.push(entry);
            }
        }
    }
    let get_item = |id: &str| item_by_id.get(id).map(|&idx| &item_list[idx]);

    let meta_nodes = kids(metadata, None);
    let values = |name: &str| -> Vec<String> {
        meta_nodes
            .iter()
            .filter(|n| n.name == name)
            .map(|n| clean(&text_of(Some(n)), 500))
            .filter(|s| !s.is_empty())
            .collect()
    };
    let refines = |id: Option<&str>, property: &str| -> Option<&Node> {
        let id = id.filter(|s| !s.is_empty())?;
        let target = format!("#{id}");
        meta_nodes
            .iter()
            .copied()
            .find(|n| n.name == "meta" && n.attr("refines") == Some(target.as_str()) && n.attr("property") == Some(property))
    };
    let named = |name: &str| -> Option<&str> {
        meta_nodes
            .iter()
            .find(|n| n.name == "meta" && n.attr("name") == Some(name))
            .and_then(|n| n.attr("content"))
    };

    let creators: Vec<&Node> = meta_nodes
        .iter()
        .copied()
        .filter(|n| n.name == "creator")
        .filter(|n| {
            let role = n
                .attr("opf:role")
                .or_else(|| n.attr("role"))
                .map(str::to_owned)
                .unwrap_or_else(|| refines(n.attr("id"), "role").map(|r| text_of(Some(r)).trim().to_owned()).unwrap_or_default());
            role.is_empty() || role == "aut"
        })
        .collect();

    let title = values("title").into_iter().next().unwrap_or_else(|| fallback_title.to_owned());
    let mut meta = empty_meta(title);

    let mut seen_authors = HashSet::new();
    for creator in creators {
        let author = clean(&text_of(Some(creator)), 200);
        if !author.is_empty() && seen_authors.insert(author.clone()) {
            meta.authors.push(author);
            if meta.authors.len() >= 10 {
                break;
            }
        }
    }
    meta.language = values("language").first().map(|s| truncate_chars(s, 35)).unwrap_or_default();
    meta.publisher = values("publisher").first().map(|s| truncate_chars(s, 200)).unwrap_or_default();
    meta.published = values("date").first().map(|s| truncate_chars(s, 10)).unwrap_or_default();
    meta.subjects = values("subject").into_iter().take(12).map(|s| truncate_chars(&s, 60)).collect();
    let desc_raw = meta_nodes
        .iter()
        .filter(|n| n.name == "description")
        .map(|n| text_of(Some(n)))
        .collect::<Vec<_>>()
        .join("\n");
    meta.description = plain_description(&desc_raw);

    // Calibre series, or EPUB 3 belongs-to-collection of type series.
    let collection = meta_nodes.iter().copied().find(|n| {
        n.name == "meta"
            && n.attr("property") == Some("belongs-to-collection")
            && match refines(n.attr("id"), "collection-type") {
                None => true,
                Some(ct) => text_of(Some(ct)).trim() == "series",
            }
    });
    let series_raw = named("calibre:series")
        .map(str::to_owned)
        .unwrap_or_else(|| collection.map(|c| text_of(Some(c))).unwrap_or_default());
    meta.series = clean(&series_raw, 200);

    let index_raw = named("calibre:series_index")
        .map(str::to_owned)
        .unwrap_or_else(|| collection.and_then(|c| refines(c.attr("id"), "group-position")).map(|r| text_of(Some(r))).unwrap_or_default());
    let parsed_index = if index_raw.trim().is_empty() {
        Some(0.0)
    } else {
        index_raw.trim().parse::<f64>().ok()
    };
    meta.series_index = if !meta.series.is_empty() {
        parsed_index.filter(|&v| v.is_finite() && (0.0..100_000.0).contains(&v))
    } else {
        None
    };

    let ppd = spine_node.and_then(|s| s.attr("page-progression-direction"));
    meta.right_to_left = ppd == Some("rtl") || (ppd.is_none_or(str::is_empty) && is_rtl(&meta.language));

    let mut spine: Vec<SpineItem> = Vec::new();
    for itemref in kids(spine_node, Some("itemref")) {
        if spine.len() >= 5000 {
            break;
        }
        let Some(item) = itemref.attr("idref").and_then(&get_item) else {
            continue;
        };
        if spine_mime.is_match(&item.media_type)
            && let Some(size) = get(&item.href)
        {
            spine.push(SpineItem { href: item.href.clone(), size });
        }
    }
    if spine.is_empty() {
        return Err("Empty reading order.".into());
    }

    // Cover: EPUB 3 cover-image, EPUB 2 <meta name="cover">, else an image named like a cover.
    let cover = item_list
        .iter()
        .find(|i| cover_prop.is_match(&i.properties))
        .map(|i| i.href.clone())
        .or_else(|| named("cover").and_then(&get_item).map(|i| i.href.clone()))
        .or_else(|| {
            item_list
                .iter()
                .filter(|i| image_mime.is_match(&i.media_type))
                .find(|i| cover_name.is_match(&i.href))
                .map(|i| i.href.clone())
        });
    meta.cover = cover.as_ref().is_some_and(|c| get(c).is_some());

    let position = |href: Option<&str>| -> isize {
        match href {
            Some(h) => spine.iter().position(|s| s.href == h).map(|i| i as isize).unwrap_or(-1),
            None => -1,
        }
    };

    let mut toc: Vec<BookTocEntry> = Vec::new();
    if let Some(nav) = item_list.iter().find(|i| nav_prop.is_match(&i.properties))
        && get(&nav.href).is_some()
    {
        let nav_bytes = read(&nav.href, 4 * 1024 * 1024).await?;
        let doc = parse_xml(&String::from_utf8_lossy(&nav_bytes), 200_000);
        let mut all_nodes = Vec::new();
        walk(doc.as_ref(), &mut all_nodes);
        let navs: Vec<&Node> = all_nodes.into_iter().filter(|n| n.name == "nav").collect();
        let toc_nav = navs
            .iter()
            .copied()
            .find(|n| toc_type.is_match(n.attr("epub:type").or_else(|| n.attr("type")).unwrap_or("")))
            .or_else(|| navs.first().copied());

        fn walk_nav_list(
            ol: Option<&Node>,
            depth: usize,
            nav_href: &str,
            toc: &mut Vec<BookTocEntry>,
            position: &dyn Fn(Option<&str>) -> isize,
        ) {
            for li in kids(ol, Some("li")) {
                if toc.len() >= 2000 {
                    return;
                }
                let a = kid(Some(li), "a").or_else(|| kid(Some(li), "span"));
                let raw_href = a.and_then(|n| n.attr("href"));
                let resolved = raw_href.and_then(|h| resolve_href(nav_href, h));
                let label = clean(&text_of(a), 200);
                if !label.is_empty() {
                    let full_href = match (&resolved, raw_href) {
                        (Some(r), Some(raw)) => match raw.split_once('#') {
                            Some((_, frag)) => format!("{r}#{frag}"),
                            None => r.clone(),
                        },
                        _ => String::new(),
                    };
                    toc.push(BookTocEntry {
                        label,
                        href: full_href,
                        depth: depth.min(6),
                        spine: position(resolved.as_deref()),
                    });
                }
                walk_nav_list(kid(Some(li), "ol"), depth + 1, nav_href, toc, position);
            }
        }
        walk_nav_list(kid(toc_nav, "ol"), 0, &nav.href, &mut toc, &position);
    }

    let ncx = match spine_node.and_then(|s| s.attr("toc")) {
        Some(id) => get_item(id),
        None => item_list.iter().find(|i| i.media_type == "application/x-dtbncx+xml"),
    };
    if toc.is_empty()
        && let Some(ncx) = ncx
        && get(&ncx.href).is_some()
    {
        let ncx_bytes = read(&ncx.href, 4 * 1024 * 1024).await?;
        let doc = parse_xml(&String::from_utf8_lossy(&ncx_bytes), 200_000);

        fn walk_ncx(
            node: Option<&Node>,
            depth: usize,
            ncx_href: &str,
            toc: &mut Vec<BookTocEntry>,
            position: &dyn Fn(Option<&str>) -> isize,
        ) {
            for point in kids(node, Some("navpoint")) {
                if toc.len() >= 2000 {
                    return;
                }
                let src = kid(Some(point), "content").and_then(|c| c.attr("src")).unwrap_or("");
                let resolved = resolve_href(ncx_href, src);
                let label = clean(&text_of(kid(kid(Some(point), "navlabel"), "text")), 200);
                if !label.is_empty() {
                    let full_href = match &resolved {
                        Some(r) => match src.split_once('#') {
                            Some((_, frag)) => format!("{r}#{frag}"),
                            None => r.clone(),
                        },
                        None => String::new(),
                    };
                    toc.push(BookTocEntry {
                        label,
                        href: full_href,
                        depth: depth.min(6),
                        spine: position(resolved.as_deref()),
                    });
                }
                walk_ncx(Some(point), depth + 1, ncx_href, toc, position);
            }
        }
        walk_ncx(kid(doc.as_ref(), "navmap"), 0, &ncx.href, &mut toc, &position);
    }

    if toc.is_empty() {
        toc = spine
            .iter()
            .enumerate()
            .map(|(index, item)| BookTocEntry {
                label: format!("Section {}", index + 1),
                href: item.href.clone(),
                depth: 0,
                spine: index as isize,
            })
            .collect();
    }

    let total_size: u64 = spine.iter().map(|s| s.size).sum();
    meta.pages = ((total_size as f64 / 2200.0).round() as u64).max(1);

    let has_cover = meta.cover;
    Ok(EpubPackage { meta, opf, spine, toc, cover: if has_cover { cover } else { None }, encrypted })
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ia = a.chars().peekable();
    let mut ib = b.chars().peekable();
    loop {
        match (ia.peek(), ib.peek()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&ca), Some(&cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let mut da = String::new();
                while let Some(&c) = ia.peek() {
                    if c.is_ascii_digit() {
                        da.push(c);
                        ia.next();
                    } else {
                        break;
                    }
                }
                let mut db = String::new();
                while let Some(&c) = ib.peek() {
                    if c.is_ascii_digit() {
                        db.push(c);
                        ib.next();
                    } else {
                        break;
                    }
                }
                let ta = da.trim_start_matches('0');
                let tb = db.trim_start_matches('0');
                match ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)) {
                    Ordering::Equal => {}
                    ord => return ord,
                }
            }
            (Some(_), Some(_)) => {
                let ca = ia.next().expect("peeked");
                let cb = ib.next().expect("peeked");
                match ca.to_lowercase().cmp(cb.to_lowercase()) {
                    Ordering::Equal => {}
                    ord => return ord,
                }
            }
        }
    }
}

/// Comic pages: images in natural name order, skipping macOS metadata and hidden files.
pub fn comic_pages(names: Vec<String>) -> Vec<String> {
    static IMG: OnceLock<Regex> = OnceLock::new();
    static SKIP: OnceLock<Regex> = OnceLock::new();
    let img = IMG.get_or_init(|| Regex::new(r"(?i)\.(jpe?g|png|gif|webp|avif|bmp)$").expect("regex"));
    let skip = SKIP.get_or_init(|| Regex::new(r"(^|/)(__MACOSX|\.)").expect("regex"));

    let mut pages: Vec<String> = names.into_iter().filter(|name| img.is_match(name) && !skip.is_match(name)).collect();
    pages.sort_by(|a, b| natural_cmp(a, b));
    pages.truncate(5000);
    pages
}

/// ComicInfo.xml (ComicRack) fields that matter for a library.
pub fn read_comic_info(xml: &str, meta: &mut BookMeta) {
    let doc = parse_xml(xml, 20_000);
    let field = |name: &str| -> String { clean(&text_of(kid(doc.as_ref(), &name.to_ascii_lowercase())), 500) };

    let title = field("Title");
    if !title.is_empty() {
        meta.title = truncate_chars(&title, 300);
    }
    let series = field("Series");
    if !series.is_empty() {
        meta.series = truncate_chars(&series, 200);
        let num = field("Number");
        meta.series_index = if !num.is_empty() { num.parse::<f64>().ok().filter(|n| n.is_finite()) } else { None };
        if title.is_empty() {
            meta.title = if !num.is_empty() {
                format!("{} #{num}", meta.series)
            } else {
                meta.series.clone()
            };
        }
    }
    let writers: Vec<String> = field("Writer")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .take(10)
        .map(str::to_owned)
        .collect();
    if !writers.is_empty() {
        meta.authors = writers;
    }
    meta.description = plain_description(&field("Summary"));
    meta.publisher = truncate_chars(&field("Publisher"), 200);
    let year = field("Year");
    if !year.is_empty() {
        meta.published = truncate_chars(&year, 4);
    }
    meta.language = truncate_chars(&field("LanguageISO"), 35);
    meta.right_to_left = field("Manga").eq_ignore_ascii_case("YesAndRightToLeft");
}
