//! OPML 2.0 import and export (`next/runtime/news/opml.ts`).

use std::collections::HashMap;
use std::time::SystemTime;

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::Serialize;
use serde_json::Value;

use crate::error::{AppError, Result};
use crate::news::feed::{LIMIT_FEEDS, LIMIT_TITLE};
use crate::news::html::{absolute, escape_html, trunc_utf16};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OpmlFeed {
    pub url: String,
    pub title: String,
    pub folder: String,
}

fn invalid_opml(message: &str) -> AppError {
    AppError::with("invalidOpml", 422, message)
}

/// Outlines with `xmlUrl` are feeds; an enclosing outline names their folder (nesting is flattened).
pub fn parse_opml(text: &str) -> Result<Vec<OpmlFeed>> {
    let mut reader = Reader::from_str(text);
    let mut feeds: Vec<OpmlFeed> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut outlines = 0usize;
    let mut depth = 0usize;
    let mut opml = false;

    loop {
        let event = reader
            .read_event()
            .map_err(|_| invalid_opml("This file is not valid OPML."))?;
        match event {
            Event::DocType(_) => {
                return Err(invalid_opml(
                    "This OPML file uses a document type, which is not supported.",
                ));
            }
            Event::Start(ref e) | Event::Empty(ref e) => {
                let is_empty = matches!(event, Event::Empty(_));
                if !is_empty {
                    depth += 1;
                }
                let name = e.name();
                if name.as_ref() == "opml" {
                    opml = true;
                }
                if name.as_ref() != "outline" {
                    continue;
                }
                outlines += 1;
                if outlines > 5000 {
                    return Err(invalid_opml("This OPML file has too many entries."));
                }
                let mut attrs: HashMap<String, String> = HashMap::new();
                for attr in e.attributes() {
                    let attr = attr.map_err(|_| invalid_opml("This file is not valid OPML."))?;
                    let val = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|_| invalid_opml("This file is not valid OPML."))?;
                    attrs.insert(attr.key.as_ref().to_owned(), val.into_owned());
                }
                let get = |k: &str| attrs.get(k).map(String::as_str).unwrap_or("");
                let xml_url = {
                    let u = get("xmlUrl");
                    if !u.is_empty() { u } else { get("xmlurl") }
                };
                let label_raw = {
                    let t = get("title");
                    if !t.is_empty() { t } else { get("text") }
                };
                let label = trunc_utf16(label_raw.trim(), 200);
                if !xml_url.is_empty() {
                    let url = absolute(xml_url, "", &["http", "https"]);
                    if !url.is_empty() && !feeds.iter().any(|f| f.url == url) {
                        let folder = stack
                            .iter()
                            .find(|s| !s.is_empty())
                            .cloned()
                            .unwrap_or_default();
                        feeds.push(OpmlFeed {
                            url,
                            title: trunc_utf16(label, LIMIT_TITLE).to_owned(),
                            folder,
                        });
                    }
                }
                if !is_empty {
                    stack.push(if !xml_url.is_empty() {
                        String::new()
                    } else {
                        label.to_owned()
                    });
                }
            }
            Event::End(ref e) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| invalid_opml("This file is not valid OPML."))?;
                if e.name().as_ref() == "outline" {
                    stack.pop();
                }
            }
            Event::GeneralRef(r) => match r.resolve_char_ref() {
                Ok(Some(_)) => {}
                Ok(None) if quick_xml::escape::resolve_predefined_entity(&r).is_some() => {}
                _ => return Err(invalid_opml("This file is not valid OPML.")),
            },
            Event::Eof => break,
            _ => {}
        }
    }
    if !opml || depth != 0 {
        return Err(invalid_opml("This file is not valid OPML."));
    }
    if feeds.len() > LIMIT_FEEDS {
        return Err(invalid_opml(&format!(
            "OPML files can hold at most {LIMIT_FEEDS} feeds."
        )));
    }
    Ok(feeds)
}

pub fn export_opml(data: &Value, owner: &str) -> String {
    let folders = data["folders"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let feeds = data["feeds"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let outline = |f: &Value| -> String {
        let title = escape_html(f["title"].as_str().unwrap_or(""));
        let url = escape_html(f["url"].as_str().unwrap_or(""));
        let link = f["link"].as_str().unwrap_or("");
        let html_url = if link.is_empty() {
            String::new()
        } else {
            format!(r#" htmlUrl="{}""#, escape_html(link))
        };
        format!(r#"<outline type="rss" text="{title}" title="{title}" xmlUrl="{url}"{html_url}/>"#)
    };
    let mut groups = String::new();
    for folder in folders {
        let folder_id = folder["id"].as_u64().unwrap_or(0);
        let name = escape_html(folder["name"].as_str().unwrap_or(""));
        groups.push_str(&format!(r#"<outline text="{name}" title="{name}">"#));
        for feed in feeds
            .iter()
            .filter(|f| f["folderId"].as_u64().unwrap_or(0) == folder_id)
        {
            groups.push_str(&outline(feed));
        }
        groups.push_str("</outline>");
    }
    let mut loose = String::new();
    for feed in feeds
        .iter()
        .filter(|f| f["folderId"].as_u64().unwrap_or(0) == 0)
    {
        loose.push_str(&outline(feed));
    }
    let date = httpdate::fmt_http_date(SystemTime::now());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\"><head><title>{} subscriptions in Dar</title><dateCreated>{date}</dateCreated></head><body>{groups}{loose}</body></opml>\n",
        escape_html(owner)
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn opml_round_trip_and_doctype_rejection() {
        let input = r#"<?xml version="1.0"?><opml version="2.0"><body><outline text="Tech"><outline text="Sub"><outline type="rss" text="LWN" xmlUrl="https://lwn.net/headlines/rss"/></outline></outline><outline text="Loose" xmlUrl="https://loose.example/feed"/><outline text="Bad" xmlUrl="file:///etc/passwd"/></body></opml>"#;
        let feeds = parse_opml(input).unwrap();
        assert_eq!(
            feeds,
            vec![
                OpmlFeed {
                    url: "https://lwn.net/headlines/rss".into(),
                    title: "LWN".into(),
                    folder: "Tech".into()
                },
                OpmlFeed {
                    url: "https://loose.example/feed".into(),
                    title: "Loose".into(),
                    folder: "".into()
                },
            ]
        );
        let err = parse_opml(r#"<!DOCTYPE x [<!ENTITY a "b">]><opml/>"#).unwrap_err();
        assert_eq!(err.code, "invalidOpml");
        assert!(err.message.contains("document type"));

        let exported = export_opml(
            &json!({
                "folders": [{ "id": 1, "name": "Tech & Code" }],
                "feeds": [
                    { "url": "https://lwn.net/headlines/rss", "title": "LWN", "link": "https://lwn.net/", "folderId": 1 },
                    { "url": "https://loose.example/feed", "title": "Loose", "link": "", "folderId": 0 },
                ]
            }),
            "admin",
        );
        assert_eq!(
            parse_opml(&exported).unwrap(),
            vec![
                OpmlFeed {
                    url: "https://lwn.net/headlines/rss".into(),
                    title: "LWN".into(),
                    folder: "Tech & Code".into()
                },
                OpmlFeed {
                    url: "https://loose.example/feed".into(),
                    title: "Loose".into(),
                    folder: "".into()
                },
            ]
        );
    }
}
