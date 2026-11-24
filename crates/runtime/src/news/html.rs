//! A small HTML tokenizer and allowlist sanitizer for feed content (`next/runtime/news/html.ts`).
//! Output contains only known formatting elements, http(s) links/images resolved to absolute URLs
//! and no scripts, styles, event handlers, frames or forms. The browser re-sanitizes before display.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use regex::{Captures, Regex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Text(String),
    Open {
        name: String,
        attrs: HashMap<String, String>,
        self_closing: bool,
    },
    Close(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alternate {
    pub url: String,
    pub title: String,
    pub media_type: String,
}

const RAW: &[&str] = &[
    "script", "style", "textarea", "title", "noscript", "iframe", "xmp", "template", "noembed",
    "noframes",
];
const ALLOWED: &[&str] = &[
    "a",
    "abbr",
    "b",
    "blockquote",
    "br",
    "caption",
    "cite",
    "code",
    "dd",
    "del",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "pre",
    "q",
    "s",
    "samp",
    "small",
    "span",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "u",
    "ul",
];
const DROPPED: &[&str] = &[
    "script",
    "style",
    "textarea",
    "title",
    "noscript",
    "iframe",
    "xmp",
    "template",
    "noembed",
    "noframes",
    "svg",
    "math",
    "form",
    "button",
    "input",
    "select",
    "option",
    "object",
    "embed",
    "applet",
    "canvas",
    "audio",
    "video",
    "head",
    "frameset",
    "frame",
    "base",
    "link",
    "meta",
    "source",
    "track",
    "param",
    "picture>source",
    "dialog",
    "portal",
];
const VOIDS: &[&str] = &[
    "br", "hr", "img", "source", "track", "wbr", "input", "meta", "link", "base", "col", "area",
    "param", "embed",
];
const BLOCK_TEXT: &[&str] = &["br", "p", "div", "li", "h1", "h2", "h3", "h4", "tr"];

/// Truncates `s` to at most `max` UTF-16 code units, matching JavaScript `.slice(0, max)`.
pub fn trunc_utf16(s: &str, max: usize) -> &str {
    let mut units = 0usize;
    for (idx, ch) in s.char_indices() {
        let len = ch.len_utf16();
        if units + len > max {
            return &s[..idx];
        }
        units += len;
    }
    s
}

/// Named HTML entity lookup table (case-sensitive, matching `html.ts`).
pub fn entity(name: &str) -> Option<&'static str> {
    match name {
        "amp" => Some("&"),
        "lt" => Some("<"),
        "gt" => Some(">"),
        "quot" => Some("\""),
        "apos" => Some("'"),
        "nbsp" => Some("\u{00a0}"),
        "hellip" => Some("\u{2026}"),
        "mdash" => Some("\u{2014}"),
        "ndash" => Some("\u{2013}"),
        "lsquo" => Some("\u{2018}"),
        "rsquo" => Some("\u{2019}"),
        "ldquo" => Some("\u{201c}"),
        "rdquo" => Some("\u{201d}"),
        "laquo" => Some("\u{00ab}"),
        "raquo" => Some("\u{00bb}"),
        "copy" => Some("\u{00a9}"),
        "reg" => Some("\u{00ae}"),
        "trade" => Some("\u{2122}"),
        "bull" => Some("\u{2022}"),
        "middot" => Some("\u{00b7}"),
        "euro" => Some("\u{20ac}"),
        "pound" => Some("\u{00a3}"),
        "yen" => Some("\u{00a5}"),
        "cent" => Some("\u{00a2}"),
        "deg" => Some("\u{00b0}"),
        "times" => Some("\u{00d7}"),
        "divide" => Some("\u{00f7}"),
        "shy" => Some("\u{00ad}"),
        "zwj" => Some("\u{200d}"),
        "zwnj" => Some("\u{200c}"),
        "thinsp" => Some("\u{2009}"),
        "ensp" => Some("\u{2002}"),
        "emsp" => Some("\u{2003}"),
        "larr" => Some("\u{2190}"),
        "rarr" => Some("\u{2192}"),
        "uarr" => Some("\u{2191}"),
        "darr" => Some("\u{2193}"),
        "harr" => Some("\u{2194}"),
        "hearts" => Some("\u{2665}"),
        "para" => Some("\u{00b6}"),
        "sect" => Some("\u{00a7}"),
        "iexcl" => Some("\u{00a1}"),
        "iquest" => Some("\u{00bf}"),
        "frac12" => Some("\u{00bd}"),
        "frac14" => Some("\u{00bc}"),
        "frac34" => Some("\u{00be}"),
        "plusmn" => Some("\u{00b1}"),
        "micro" => Some("\u{00b5}"),
        "prime" => Some("\u{2032}"),
        "Prime" => Some("\u{2033}"),
        "aacute" => Some("\u{00e1}"),
        "eacute" => Some("\u{00e9}"),
        "iacute" => Some("\u{00ed}"),
        "oacute" => Some("\u{00f3}"),
        "uacute" => Some("\u{00fa}"),
        "agrave" => Some("\u{00e0}"),
        "egrave" => Some("\u{00e8}"),
        "igrave" => Some("\u{00ec}"),
        "ograve" => Some("\u{00f2}"),
        "ugrave" => Some("\u{00f9}"),
        "acirc" => Some("\u{00e2}"),
        "ecirc" => Some("\u{00ea}"),
        "icirc" => Some("\u{00ee}"),
        "ocirc" => Some("\u{00f4}"),
        "ucirc" => Some("\u{00fb}"),
        "auml" => Some("\u{00e4}"),
        "euml" => Some("\u{00eb}"),
        "iuml" => Some("\u{00ef}"),
        "ouml" => Some("\u{00f6}"),
        "uuml" => Some("\u{00fc}"),
        "atilde" => Some("\u{00e3}"),
        "otilde" => Some("\u{00f5}"),
        "ntilde" => Some("\u{00f1}"),
        "ccedil" => Some("\u{00e7}"),
        "szlig" => Some("\u{00df}"),
        "aring" => Some("\u{00e5}"),
        "aelig" => Some("\u{00e6}"),
        "oslash" => Some("\u{00f8}"),
        "Aacute" => Some("\u{00c1}"),
        "Eacute" => Some("\u{00c9}"),
        "Oacute" => Some("\u{00d3}"),
        "Uacute" => Some("\u{00da}"),
        "Auml" => Some("\u{00c4}"),
        "Ouml" => Some("\u{00d6}"),
        "Uuml" => Some("\u{00dc}"),
        "Ntilde" => Some("\u{00d1}"),
        "Ccedil" => Some("\u{00c7}"),
        _ => None,
    }
}

pub fn decode(value: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)&(#x[0-9a-f]{1,6}|#[0-9]{1,7}|[a-z][a-z0-9]{1,31});?")
            .expect("entity regex")
    });
    re.replace_all(value, |caps: &Captures<'_>| {
        let full = caps.get(0).map_or("", |m| m.as_str());
        let code = caps.get(1).map_or("", |m| m.as_str());
        if let Some(rest) = code.strip_prefix('#') {
            let parsed =
                if let Some(hex) = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X')) {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    rest.parse::<u32>().ok()
                };
            match parsed {
                Some(n) if n > 0 && n <= 0x10ffff && !(0xd800..=0xdfff).contains(&n) => {
                    char::from_u32(n).map_or_else(|| "\u{fffd}".to_owned(), |c| c.to_string())
                }
                _ => "\u{fffd}".to_owned(),
            }
        } else {
            entity(code).unwrap_or(full).to_owned()
        }
    })
    .into_owned()
}

pub fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn find_ascii_ci(haystack: &str, needle_lower: &str) -> Option<usize> {
    let nb = needle_lower.as_bytes();
    if nb.is_empty() {
        return Some(0);
    }
    haystack
        .as_bytes()
        .windows(nb.len())
        .position(|w| w.iter().zip(nb).all(|(a, b)| a.to_ascii_lowercase() == *b))
}

pub fn tokenize(html: &str, limit: usize) -> Vec<Token> {
    static ATTR_RE: OnceLock<Regex> = OnceLock::new();
    let attr_re = ATTR_RE.get_or_init(|| {
        Regex::new(r#"([^\s"'>/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?"#)
            .expect("attr regex")
    });

    let mut tokens = Vec::new();
    let bytes = html.as_bytes();
    let mut i = 0usize;
    let push_text = |tokens: &mut Vec<Token>, text: &str| {
        if !text.is_empty() {
            tokens.push(Token::Text(decode(text)));
        }
    };

    while i < bytes.len() && tokens.len() < limit {
        let Some(rel_lt) = html[i..].find('<') else {
            push_text(&mut tokens, &html[i..]);
            break;
        };
        let lt = i + rel_lt;
        push_text(&mut tokens, &html[i..lt]);
        if html[lt..].starts_with("<!--") {
            i = html[lt + 4..]
                .find("-->")
                .map_or(bytes.len(), |end| lt + 4 + end + 3);
            continue;
        }
        if matches!(bytes.get(lt + 1), Some(b'!' | b'?')) {
            i = html[lt..].find('>').map_or(bytes.len(), |end| lt + end + 1);
            continue;
        }
        let close = bytes.get(lt + 1) == Some(&b'/');
        let start = lt + if close { 2 } else { 1 };
        let name_len = {
            let slice = &bytes[start..bytes.len().min(start + 64)];
            if slice.first().is_some_and(u8::is_ascii_alphabetic) {
                1 + slice[1..]
                    .iter()
                    .take_while(|b| b.is_ascii_alphanumeric() || **b == b':' || **b == b'-')
                    .count()
            } else {
                0
            }
        };
        if name_len == 0 {
            push_text(&mut tokens, "<");
            i = lt + 1;
            continue;
        }
        let name = &html[start..start + name_len];
        // Find the tag end outside quoted attribute values.
        let mut j = start + name_len;
        let mut quote = 0u8;
        while j < bytes.len() {
            let c = bytes[j];
            if quote != 0 {
                if c == quote {
                    quote = 0;
                }
            } else if c == b'"' || c == b'\'' {
                quote = c;
            } else if c == b'>' {
                break;
            }
            j += 1;
        }
        let inner = &html[start + name_len..j];
        let lower = name.to_ascii_lowercase();
        i = (j + 1).min(bytes.len());
        if close {
            tokens.push(Token::Close(lower));
            continue;
        }
        let mut attrs = HashMap::new();
        for caps in attr_re.captures_iter(inner) {
            let key = caps[1].to_ascii_lowercase();
            if !attrs.contains_key(&key) && attrs.len() < 40 {
                let raw_val = caps
                    .get(2)
                    .or_else(|| caps.get(3))
                    .or_else(|| caps.get(4))
                    .map_or("", |m| m.as_str());
                attrs.insert(key, decode(raw_val));
            }
        }
        let self_closing = inner.trim_end().ends_with('/');
        let is_raw = RAW.contains(&lower.as_str());
        let is_title = lower == "title";
        let close_needle = if is_raw {
            Some(format!("</{lower}"))
        } else {
            None
        };
        tokens.push(Token::Open {
            name: lower,
            attrs,
            self_closing,
        });
        if let Some(needle) = close_needle {
            let end = find_ascii_ci(&html[i..], &needle).map_or(bytes.len(), |pos| i + pos);
            let body = &html[i..end];
            tokens.push(Token::Text(if is_title {
                decode(body)
            } else {
                String::new()
            }));
            i = end;
        }
    }
    tokens
}

/// Resolves `value` against `base` (when non-empty) and allows only the listed schemes without credentials.
pub fn absolute(value: &str, base: &str, schemes: &[&str]) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let parsed = if !base.is_empty() {
        url::Url::parse(base)
            .ok()
            .and_then(|b| b.join(trimmed).ok())
    } else {
        url::Url::parse(trimmed).ok()
    };
    match parsed {
        Some(u)
            if schemes.contains(&u.scheme())
                && u.username().is_empty()
                && u.password().is_none()
                && u.as_str().len() <= 2048 =>
        {
            u.to_string()
        }
        _ => String::new(),
    }
}

fn digits(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_digit())
}

/// Re-serializes tokens through the allowlist, balancing tags and resolving URLs.
pub fn sanitize_tokens(tokens: &[Token], base: &str, max_bytes: usize) -> String {
    let mut stack: Vec<&str> = Vec::new();
    let mut out = String::new();
    let mut skip = 0usize;
    let mut skip_name = "";

    for token in tokens {
        if out.len() > max_bytes {
            break;
        }
        if skip > 0 {
            match token {
                Token::Open {
                    name, self_closing, ..
                } if name == skip_name && !self_closing => skip += 1,
                Token::Close(name) if name == skip_name => skip -= 1,
                _ => {}
            }
            continue;
        }
        match token {
            Token::Text(text) => out.push_str(&escape_html(text)),
            Token::Open {
                name,
                attrs: a,
                self_closing,
            } => {
                if DROPPED.contains(&name.as_str()) {
                    if !self_closing && !VOIDS.contains(&name.as_str()) {
                        skip = 1;
                        skip_name = name.as_str();
                    }
                    continue;
                }
                if !ALLOWED.contains(&name.as_str()) || stack.len() >= 64 {
                    continue;
                }
                let mut attrs: Vec<String> = Vec::new();
                let mut add = |key: &str, value: &str| {
                    if !value.is_empty() {
                        attrs.push(format!("{key}=\"{}\"", escape_html(value)));
                    }
                };
                let get = |key: &str| a.get(key).map(String::as_str).unwrap_or("");
                if name == "a" {
                    let href = absolute(get("href"), base, &["http", "https", "mailto"]);
                    add("href", &href);
                    add("title", trunc_utf16(get("title"), 300));
                }
                if name == "img" {
                    // Lazy-loading pages often keep the real image in data-src behind a placeholder.
                    let raw_src = a
                        .get("src")
                        .map(String::as_str)
                        .filter(|s| !s.is_empty() && !s.starts_with("data:"))
                        .or_else(|| {
                            a.get("data-src")
                                .map(String::as_str)
                                .filter(|s| !s.is_empty())
                        })
                        .or_else(|| {
                            a.get("data-original")
                                .map(String::as_str)
                                .filter(|s| !s.is_empty())
                        })
                        .or_else(|| {
                            a.get("srcset")
                                .and_then(|s| s.split(',').next())
                                .map(str::trim)
                                .and_then(|s| s.split_whitespace().next())
                                .filter(|s| !s.is_empty())
                        })
                        .unwrap_or("");
                    let src = absolute(raw_src, base, &["http", "https"]);
                    if src.is_empty() {
                        continue;
                    }
                    add("src", &src);
                    add("alt", trunc_utf16(get("alt"), 500));
                    add("title", trunc_utf16(get("title"), 300));
                    for key in ["width", "height"] {
                        let v = get(key);
                        if digits(v, 1, 4) {
                            add(key, v);
                        }
                    }
                }
                if name == "td" || name == "th" {
                    for key in ["colspan", "rowspan"] {
                        let v = get(key);
                        if digits(v, 1, 2) {
                            add(key, v);
                        }
                    }
                }
                if name == "ol" {
                    let start = get("start");
                    if digits(start, 1, 6) {
                        add("start", start);
                    }
                }
                if name == "time" {
                    add("datetime", trunc_utf16(get("datetime"), 64));
                }
                if name == "abbr" {
                    add("title", trunc_utf16(get("title"), 300));
                }
                let dir = get("dir");
                if dir == "rtl" || dir == "ltr" {
                    add("dir", dir);
                }
                out.push('<');
                out.push_str(name);
                if !attrs.is_empty() {
                    out.push(' ');
                    out.push_str(&attrs.join(" "));
                }
                out.push('>');
                if !VOIDS.contains(&name.as_str()) && !self_closing {
                    stack.push(name.as_str());
                }
            }
            Token::Close(name) => {
                if let Some(index) = stack.iter().rposition(|t| *t == name.as_str()) {
                    while stack.len() > index {
                        if let Some(tag) = stack.pop() {
                            out.push_str("</");
                            out.push_str(tag);
                            out.push('>');
                        }
                    }
                }
            }
        }
    }
    while let Some(tag) = stack.pop() {
        out.push_str("</");
        out.push_str(tag);
        out.push('>');
    }
    out
}

pub fn sanitize(html: &str, base: &str, max_bytes: usize) -> String {
    let tokens = tokenize(html, 200_000);
    sanitize_tokens(&tokens, base, max_bytes)
}

/// Visible text, whitespace collapsed.
pub fn text_of_tokens(tokens: &[Token], max: usize) -> String {
    let mut out = String::new();
    let mut skip = 0usize;
    let mut skip_name = "";
    for token in tokens {
        if skip > 0 {
            if let Token::Close(name) = token
                && name == skip_name
            {
                skip -= 1;
            }
            continue;
        }
        match token {
            Token::Open {
                name, self_closing, ..
            } if DROPPED.contains(&name.as_str())
                && !self_closing
                && !VOIDS.contains(&name.as_str()) =>
            {
                skip = 1;
                skip_name = name.as_str();
                continue;
            }
            Token::Text(text) => out.push_str(text),
            Token::Open { name, .. } if BLOCK_TEXT.contains(&name.as_str()) => out.push(' '),
            _ => {}
        }
        if out.len() > max {
            break;
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn text_of(html: &str, max: usize) -> String {
    let tokens = tokenize(html, 200_000);
    text_of_tokens(&tokens, max)
}

pub fn discover_feeds(html: &str, base: &str) -> Vec<Alternate> {
    static ALT_RE: OnceLock<Regex> = OnceLock::new();
    static TYPE_RE: OnceLock<Regex> = OnceLock::new();
    let alt_re = ALT_RE.get_or_init(|| Regex::new(r"(?i)\balternate\b").expect("alt regex"));
    let type_re = TYPE_RE.get_or_init(|| Regex::new(r"(?i)^application/(rss|atom|feed)\+(xml|json)$|^application/json$|^text/xml$|^application/xml$").expect("type regex"));

    let mut found: Vec<Alternate> = Vec::new();
    let mut page_base = base.to_owned();
    for token in tokenize(html, 20_000) {
        let Token::Open { name, attrs, .. } = token else {
            continue;
        };
        if name == "base"
            && let Some(href) = attrs.get("href")
            && !href.is_empty()
        {
            let resolved = absolute(href, base, &["http", "https"]);
            if !resolved.is_empty() {
                page_base = resolved;
            }
        }
        if name == "link" {
            let rel = attrs.get("rel").map(String::as_str).unwrap_or("");
            let media_type = attrs.get("type").map(String::as_str).unwrap_or("");
            if alt_re.is_match(rel) && type_re.is_match(media_type.trim()) {
                let href = attrs.get("href").map(String::as_str).unwrap_or("");
                let url = absolute(href, &page_base, &["http", "https"]);
                if !url.is_empty() && !found.iter().any(|f| f.url == url) {
                    let title = trunc_utf16(
                        attrs.get("title").map(String::as_str).unwrap_or("").trim(),
                        300,
                    )
                    .to_owned();
                    found.push(Alternate {
                        url,
                        title,
                        media_type: media_type.to_ascii_lowercase(),
                    });
                }
            }
        }
        if name == "body" {
            break;
        }
    }
    found.truncate(10);
    found
}

/// Picks the main article from a web page: an `<article>`/`articleBody`/`<main>` element when it
/// holds real text, otherwise the container whose paragraphs hold the most text, with navigation,
/// comments and promotion containers penalized. Returns sanitized HTML.
pub fn extract_article(html: &str, base: &str) -> String {
    static NEG_RE: OnceLock<Regex> = OnceLock::new();
    let negative = NEG_RE.get_or_init(|| Regex::new(r"(?i)comment|footer|sidebar|\bnav|menu|share|social|related|promo|advert|\bad-|newsletter|subscribe|cookie|popup|banner").expect("negative regex"));

    let tokens = tokenize(html, 200_000);
    let mut ends: HashMap<usize, usize> = HashMap::new();
    let mut parents: HashMap<usize, usize> = HashMap::new();
    let mut stack: Vec<(usize, &str)> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        match token {
            Token::Open {
                name, self_closing, ..
            } if !self_closing && !VOIDS.contains(&name.as_str()) => {
                if let Some(&(parent_idx, _)) = stack.last() {
                    parents.insert(i, parent_idx);
                }
                stack.push((i, name.as_str()));
            }
            Token::Close(name) => {
                if let Some(k) = stack.iter().rposition(|(_, n)| *n == name.as_str()) {
                    while stack.len() > k {
                        if let Some((open_idx, _)) = stack.pop() {
                            ends.insert(open_idx, i);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let body_of = |start: usize| -> &[Token] {
        let end = ends.get(&start).copied().unwrap_or(tokens.len());
        if start + 1 <= end && end <= tokens.len() {
            &tokens[start + 1..end]
        } else {
            &[]
        }
    };
    let attrs_of = |t: &Token| -> String {
        match t {
            Token::Open { attrs, .. } => format!(
                "{} {}",
                attrs.get("class").map(String::as_str).unwrap_or(""),
                attrs.get("id").map(String::as_str).unwrap_or("")
            ),
            _ => String::new(),
        }
    };

    let mut best: Option<usize> = None;
    let mut best_score = 0.0f64;
    for (i, t) in tokens.iter().enumerate() {
        let Token::Open { name, attrs, .. } = t else {
            continue;
        };
        if !ends.contains_key(&i) {
            continue;
        }
        let itemprop = attrs.get("itemprop").map(String::as_str).unwrap_or("");
        let role = attrs.get("role").map(String::as_str).unwrap_or("");
        if name == "article" || itemprop == "articleBody" || name == "main" || role == "main" {
            let length = text_of_tokens(body_of(i), 100_000).encode_utf16().count() as f64;
            let boost = if itemprop == "articleBody" {
                3.0
            } else if name == "article" {
                2.0
            } else {
                1.2
            };
            let penalty = if negative.is_match(&attrs_of(t)) {
                0.3
            } else {
                1.0
            };
            let score = length * boost * penalty;
            if length > 400.0 && score > best_score {
                best = Some(i);
                best_score = score;
            }
        }
    }
    if best.is_none() {
        let mut scores: BTreeMap<usize, f64> = BTreeMap::new();
        for (i, t) in tokens.iter().enumerate() {
            let Token::Open { name, .. } = t else {
                continue;
            };
            if name != "p" || !ends.contains_key(&i) {
                continue;
            }
            let Some(&parent) = parents.get(&i) else {
                continue;
            };
            let length = text_of_tokens(body_of(i), 100_000).encode_utf16().count() as f64;
            if length > 40.0 {
                *scores.entry(parent).or_insert(0.0) += length;
            }
        }
        for (index, score) in scores {
            let adjusted = score
                * if negative.is_match(&attrs_of(&tokens[index])) {
                    0.3
                } else {
                    1.0
                };
            if adjusted > best_score {
                best = Some(index);
                best_score = adjusted;
            }
        }
    }
    let Some(best_idx) = best else {
        return String::new();
    };
    let content = sanitize_tokens(body_of(best_idx), base, 200 * 1024);
    if text_of(&content, 100_000).encode_utf16().count() >= 300 {
        content
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizer_keeps_formatting_and_drops_active_content() {
        let input = r#"<div onclick="x()"><p style="color:red">Hi <b>there<i>!</b></p><script>alert(1)</script><style>p{}</style><iframe src="https://evil"></iframe>
    <a href="javascript:alert(1)">bad</a><a href="/post?a=1&amp;b=2" target="_blank">good</a><img src="data:image/png;base64,AA" data-src="img/a.png" onerror="x()"><img src="https://cdn.example/b.jpg" width="640" height="x">
    <form><input value="secret"></form><svg><script>bad()</script></svg><unknown-tag>kept text</unknown-tag><p>&lt;not a tag&gt; &amp; &hellip;</p></div>"#;
        let out = sanitize(input, "https://site.example/blog/", 200 * 1024);
        for forbidden in [
            "script",
            "style=",
            "onclick",
            "onerror",
            "iframe",
            "javascript:",
            "<form",
            "<input",
            "<svg",
            "secret",
            "bad()",
        ] {
            assert!(!out.contains(forbidden), "unexpected {forbidden} in {out}");
        }
        assert!(
            out.contains(r#"<a href="https://site.example/post?a=1&amp;b=2">good</a>"#),
            "{out}"
        );
        assert!(out.contains("<a>bad</a>"), "{out}");
        assert!(
            out.contains(r#"<img src="https://site.example/blog/img/a.png">"#),
            "{out}"
        );
        assert!(
            out.contains(r#"<img src="https://cdn.example/b.jpg" width="640">"#),
            "{out}"
        );
        assert!(out.contains("<p>Hi <b>there<i>!</i></b></p>"), "{out}");
        assert!(out.contains("kept text"), "{out}");
        assert!(out.contains("&lt;not a tag&gt; &amp; \u{2026}"), "{out}");
        assert_eq!(
            sanitize("<p>open <em>tags", "", 200 * 1024),
            "<p>open <em>tags</em></p>"
        );
        assert_eq!(decode("&#x1F600;&#0;&bogus;"), "\u{1f600}\u{fffd}&bogus;");
        assert_eq!(
            text_of("<p>One</p><p>Two <script>no</script>three</p>", 100_000),
            "One Two three"
        );
    }

    #[test]
    fn discovery_and_article_extraction() {
        let html = r#"<html><head><link rel="alternate" type="application/rss+xml" title="Posts" href="/feed.xml"><link rel="alternate" type="application/atom+xml" href="https://other.example/atom"><link rel="stylesheet" href="/s.css"></head><body></body></html>"#;
        assert_eq!(
            discover_feeds(html, "https://site.example/blog/"),
            vec![
                Alternate {
                    url: "https://site.example/feed.xml".into(),
                    title: "Posts".into(),
                    media_type: "application/rss+xml".into()
                },
                Alternate {
                    url: "https://other.example/atom".into(),
                    title: "".into(),
                    media_type: "application/atom+xml".into()
                },
            ]
        );
        let paragraph = |n: usize| {
            format!(
                "<p>{}{n}</p>",
                "Paragraph text that is long enough to count as real article content. ".repeat(3)
            )
        };
        let page = format!(
            r#"<html><body><nav class="menu"><p>{}</p></nav><div id="content">{}<script>track()</script></div><div class="comments">{}</div></body></html>"#,
            "Navigation link text repeated many times. ".repeat(20),
            (1..=5).map(paragraph).collect::<String>(),
            paragraph(9)
        );
        let article = extract_article(&page, "https://site.example/");
        assert!(article.contains("Paragraph text"));
        assert!(!article.contains("Navigation"));
        assert!(!article.contains("track"));
        assert!(!article.contains("9</p>"));

        let with_article = format!(
            "<body><div>{}</div><article><h1>Title</h1>{}</article></body>",
            paragraph(1),
            (1..=6).map(paragraph).collect::<String>()
        );
        assert!(extract_article(&with_article, "https://a.example/").starts_with("<h1>Title</h1>"));
        assert_eq!(extract_article("<p>short</p>", "https://a.example/"), "");
    }
}
