//! Builds article DOM from server-sanitized HTML through a second allowlist (`news/render.ts`).
//! `DOMParser` documents are inert (no scripts run, no images load); only copied nodes reach the
//! page. Images load through Dar's proxy or are replaced by a note; links open in a new tab.

use wasm_bindgen::JsCast;

use crate::ui::document;

const ALLOWED: &[&str] = &[
    "a", "abbr", "b", "blockquote", "br", "caption", "cite", "code", "dd", "del", "dfn", "div",
    "dl", "dt", "em", "figcaption", "figure", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "i",
    "img", "ins", "kbd", "li", "mark", "ol", "p", "pre", "q", "s", "samp", "small", "span",
    "strong", "sub", "sup", "table", "tbody", "td", "tfoot", "th", "thead", "time", "tr", "u",
    "ul",
];

const DROPPED: &[&str] = &[
    "script", "style", "iframe", "object", "embed", "svg", "math", "form", "template", "noscript",
    "link", "meta", "base", "video", "audio", "canvas",
];

const SAFE_ATTRS: &[&str] = &["colspan", "rowspan", "start", "datetime", "title", "dir"];

fn safe_url(value: Option<String>, schemes: &[&str]) -> Option<String> {
    let value = value.filter(|v| !v.is_empty())?;
    let url = web_sys::Url::new(&value).ok()?;
    let protocol = url.protocol();
    schemes.contains(&protocol.as_str()).then(|| url.href())
}

fn walk(
    doc: &web_sys::Document,
    from: &web_sys::Node,
    to: &web_sys::Node,
    depth: usize,
    images: &str,
    hidden: &mut usize,
) {
    let children = from.child_nodes();
    for i in 0..children.length() {
        let Some(child) = children.item(i) else { continue };
        let node_type = child.node_type();
        if node_type == web_sys::Node::TEXT_NODE {
            let text = child.text_content().unwrap_or_default();
            let _ = to.append_child(&doc.create_text_node(&text));
            continue;
        }
        if node_type != web_sys::Node::ELEMENT_NODE {
            continue;
        }
        let element: web_sys::Element = child.unchecked_into();
        let tag = element.tag_name().to_lowercase();
        if DROPPED.contains(&tag.as_str()) {
            continue;
        }
        if !ALLOWED.contains(&tag.as_str()) || depth > 60 {
            walk(doc, &element, to, depth, images, hidden);
            continue;
        }
        if tag == "img" {
            let Some(src) = safe_url(element.get_attribute("src"), &["http:", "https:"]) else {
                continue;
            };
            if images == "block" {
                *hidden += 1;
                continue;
            }
            let Ok(el) = doc.create_element("img") else { continue };
            let image: web_sys::HtmlImageElement = el.unchecked_into();
            let encoded = js_sys::encode_uri_component(&src).as_string().unwrap_or_default();
            image.set_src(&format!("/api/v1/news/image?url={encoded}"));
            image.set_alt(&element.get_attribute("alt").unwrap_or_default());
            let _ = image.set_attribute("loading", "lazy");
            let _ = image.set_attribute("decoding", "async");
            let _ = image.set_attribute("referrerpolicy", "no-referrer");
            let _ = to.append_child(&image);
            continue;
        }
        let Ok(copy) = doc.create_element(&tag) else { continue };
        if tag == "a"
            && let Some(href) = safe_url(element.get_attribute("href"), &["http:", "https:", "mailto:"])
        {
            let _ = copy.set_attribute("href", &href);
            let _ = copy.set_attribute("target", "_blank");
            let _ = copy.set_attribute("rel", "noopener noreferrer");
        }
        for &name in SAFE_ATTRS {
            if let Some(value) = element.get_attribute(name)
                && !value.is_empty()
                && (name != "dir" || value == "rtl" || value == "ltr")
                && value.encode_utf16().count() < 300
            {
                let _ = copy.set_attribute(name, &value);
            }
        }
        walk(doc, &element, &copy, depth + 1, images, hidden);
        let _ = to.append_child(&copy);
    }
}

/// Parses `html` in an inert `DOMParser` document and rebuilds safe nodes into a fragment.
pub fn render_article(html: &str, images: &str) -> web_sys::DocumentFragment {
    let doc = document();
    let out = doc.create_document_fragment();
    let Ok(parser) = web_sys::DomParser::new() else {
        return out;
    };
    let Ok(source) = parser.parse_from_string(
        &format!("<!doctype html><body>{html}</body>"),
        web_sys::SupportedType::TextHtml,
    ) else {
        return out;
    };
    let mut hidden = 0usize;
    if let Some(body) = source.body() {
        walk(&doc, &body, &out, 0, images, &mut hidden);
    }
    if hidden > 0
        && let Ok(note) = doc.create_element("p")
    {
        note.set_class_name("news-images-hidden");
        note.set_text_content(Some(&format!(
            "{hidden} image{} hidden by your News settings.",
            if hidden == 1 { "" } else { "s" }
        )));
        let _ = out.insert_before(&note, out.first_child().as_ref());
    }
    out
}
