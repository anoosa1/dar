//! A parsed WebDAV request body (`next/runtime/dav-xml.ts`): namespaced elements,
//! attributes without a namespace, text. No DTDs; bounded size and depth.

use std::collections::BTreeMap;

use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;

use crate::DavError;

pub const D: &str = "DAV:";

#[derive(Clone, Debug, Default)]
pub struct Xml {
    pub uri: String,
    pub name: String,
    pub attrs: BTreeMap<String, String>,
    pub text: String,
    pub children: Vec<Xml>,
}

impl Xml {
    pub fn is(&self, uri: &str, name: &str) -> bool {
        self.uri == uri && self.name == name
    }
    pub fn children<'a>(&'a self, uri: &'a str, name: &'a str) -> impl Iterator<Item = &'a Xml> + 'a {
        self.children.iter().filter(move |n| n.is(uri, name))
    }
    pub fn child(&self, uri: &str, name: &str) -> Option<&Xml> {
        self.children.iter().find(|n| n.is(uri, name))
    }
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }
}

fn namespace(result: ResolveResult<'_>) -> Result<String, DavError> {
    match result {
        ResolveResult::Bound(ns) => Ok(ns.0.to_owned()),
        ResolveResult::Unbound => Ok(String::new()),
        ResolveResult::Unknown(_) => Err(DavError::bad("invalidXml")),
    }
}

pub fn parse(text: &str) -> Result<Xml, DavError> {
    let invalid = || DavError::bad("invalidXml");
    let mut reader = NsReader::from_str(text);
    let mut stack: Vec<Xml> = Vec::new();
    let mut root: Option<Xml> = None;
    let mut nodes = 0usize;
    loop {
        let (resolved, event) = reader.read_resolved_event().map_err(|_| invalid())?;
        let uri = namespace(resolved)?;
        match event {
            Event::DocType(_) => return Err(invalid()),
            Event::Start(ref e) | Event::Empty(ref e) => {
                nodes += 1;
                if nodes > 10_000 || stack.len() > 32 {
                    return Err(DavError::new(413, "requestTooLarge"));
                }
                let mut node = Xml { uri, name: e.local_name().as_ref().to_owned(), ..Default::default() };
                for attr in e.attributes() {
                    let attr = attr.map_err(|_| invalid())?;
                    let key = attr.key.as_ref();
                    if key == "xmlns" || key.starts_with("xmlns:") {
                        continue;
                    }
                    let (ns, local) = reader.resolver().resolve_attribute(attr.key);
                    if matches!(ns, ResolveResult::Unbound) {
                        let value = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).map_err(|_| invalid())?;
                        node.attrs.insert(local.as_ref().to_owned(), value.into_owned());
                    }
                }
                if matches!(event, Event::Empty(_)) {
                    attach(&mut stack, &mut root, node)?;
                } else {
                    stack.push(node);
                }
            }
            Event::End(_) => {
                let node = stack.pop().ok_or_else(invalid)?;
                attach(&mut stack, &mut root, node)?;
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.xml10_content());
                } else if !t.xml10_content().trim().is_empty() {
                    return Err(invalid());
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref().map_err(|_| invalid())? {
                    Some(c) => c.to_string(),
                    None => quick_xml::escape::resolve_predefined_entity(&r).ok_or_else(invalid)?.to_owned(),
                };
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&resolved);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(invalid());
    }
    root.ok_or_else(invalid)
}

fn attach(stack: &mut [Xml], root: &mut Option<Xml>, node: Xml) -> Result<(), DavError> {
    match stack.last_mut() {
        Some(parent) => parent.children.push(node),
        None if root.is_none() => *root = Some(node),
        None => return Err(DavError::bad("invalidXml")),
    }
    Ok(())
}

pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_namespaces_and_entities() {
        let x = parse(r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:x"><d:prop><c:a k="v&amp;w">x &lt; y&#65;</c:a><d:b/></d:prop></d:propfind>"#).unwrap();
        assert!(x.is(D, "propfind"));
        let prop = x.child(D, "prop").unwrap();
        assert_eq!(prop.children[0].uri, "urn:x");
        assert_eq!(prop.children[0].attr("k"), Some("v&w"));
        assert_eq!(prop.children[0].text, "x < yA");
        assert!(prop.children[1].is(D, "b"));
    }

    #[test]
    fn rejects_doctypes_and_unknown_prefixes() {
        assert!(parse(r#"<!DOCTYPE x [<!ENTITY a "b">]><x/>"#).is_err());
        assert!(parse(r#"<q:x/>"#).is_err());
        assert!(parse(r#"<x/><y/>"#).is_err());
        let deep = "<a>".repeat(40) + &"</a>".repeat(40);
        assert!(parse(&deep).is_err());
    }
}
