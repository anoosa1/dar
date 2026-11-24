//! CardDAV and CalDAV (`next/runtime/pim/dav.ts`), using the same checked domain operations
//! as native edits.

use std::collections::BTreeMap;

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::ical::{self, Component};
use crate::xml::{self, D, Xml, escape};
use crate::{App, Dav, DavError, DavResult, Principal, Request, encode_component, http_date};

pub const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
const CALENDARSERVER: &str = "http://calendarserver.org/ns/";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PimKind {
    Contacts,
    Calendar,
}

#[derive(Clone, Debug)]
pub struct Collection {
    pub id: String,
    pub kind: PimKind,
    pub name: String,
    pub revision: u64,
    pub modified_at: i64,
}

#[derive(Clone, Debug)]
pub struct Resource {
    pub id: String,
    pub href: String,
    pub revision: u64,
    pub modified_at: i64,
    pub content: String,
}

/// Dar's `service_pim` operations used by DAV.
pub trait PimBackend: Send + Sync + 'static {
    fn collections(&self, session: &str) -> DavResult<Vec<Collection>>;
    fn list(
        &self,
        session: &str,
        collection_id: &str,
        metadata_only: bool,
    ) -> DavResult<Vec<Resource>>;
    /// Validate and store `content`; `revision` 0 creates. Returns the stored row.
    fn put(
        &self,
        session: &str,
        collection: &Collection,
        href: &str,
        content: &str,
        revision: u64,
    ) -> DavResult<Resource>;
    fn delete(
        &self,
        session: &str,
        collection_id: &str,
        href: &str,
        revision: u64,
    ) -> DavResult<()>;
}

pub fn pim_etag(id: &str, revision: u64) -> String {
    format!("\"{id}:{revision}\"")
}

/// `requireMatch`: creating needs `If-None-Match: *`; changing needs the current `If-Match`.
fn require_match(request: &Request, row: Option<&Resource>) -> DavResult<u64> {
    match row {
        None if request.header("if-none-match") == Some("*") => Ok(0),
        None => Err(DavError::new(428, "preconditionRequired")),
        Some(row) => match request.header("if-match") {
            None => Err(DavError::new(428, "preconditionRequired")),
            Some(m) if m == pim_etag(&row.id, row.revision) => Ok(row.revision),
            Some(_) => Err(DavError::new(412, "revisionConflict")),
        },
    }
}

type Props = BTreeMap<String, String>;

fn key(uri: &str, name: &str) -> String {
    format!("{uri}|{name}")
}

struct Context<'a> {
    kind: PimKind,
    principal: String,
    user: String,
    pim_access: bool,
    at_principal: bool,
    collections: &'a [Collection],
}

enum Row<'a> {
    Collection(&'a Collection),
    Resource(&'a Resource),
}

impl Context<'_> {
    fn props(&self, name: &str, is_collection: bool, row: Option<Row>) -> Props {
        let property_collection = match &row {
            Some(Row::Collection(c)) => self.collections.iter().find(|x| x.id == c.id),
            _ => None,
        };
        let contacts = self.kind == PimKind::Contacts;
        let mut map = Props::new();
        map.insert(
            key(D, "displayname"),
            format!("<d:displayname>{}</d:displayname>", escape(name)),
        );
        let mut types = String::new();
        if is_collection {
            types.push_str("<d:collection/>");
        }
        if is_collection && property_collection.is_some() {
            types.push_str(if contacts {
                "<a:addressbook/>"
            } else {
                "<c:calendar/>"
            });
        }
        if self.at_principal {
            types.push_str("<d:principal/>");
        }
        map.insert(
            key(D, "resourcetype"),
            format!("<d:resourcetype>{types}</d:resourcetype>"),
        );
        map.insert(
            key(D, "current-user-principal"),
            format!(
                "<d:current-user-principal><d:href>{}</d:href></d:current-user-principal>",
                self.principal
            ),
        );
        if self.pim_access {
            map.insert(key(CARDDAV, "addressbook-home-set"), format!("<a:addressbook-home-set><d:href>/dav/addressbooks/{}/</d:href></a:addressbook-home-set>", self.user));
            map.insert(key(CALDAV, "calendar-home-set"), format!("<c:calendar-home-set><d:href>/dav/calendars/{}/</d:href></c:calendar-home-set>", self.user));
        }
        let (id, revision, modified, content) = match &row {
            Some(Row::Collection(c)) => (c.id.as_str(), c.revision, c.modified_at, None),
            Some(Row::Resource(r)) => (
                r.id.as_str(),
                r.revision,
                r.modified_at,
                Some(r.content.as_str()),
            ),
            None => ("", 0, 0, None),
        };
        if row.is_some() {
            map.insert(
                key(D, "getetag"),
                format!("<d:getetag>{}</d:getetag>", escape(&pim_etag(id, revision))),
            );
            map.insert(
                key(D, "getlastmodified"),
                format!(
                    "<d:getlastmodified>{}</d:getlastmodified>",
                    http_date(modified)
                ),
            );
        }
        if let Some(content) = content.filter(|c| !c.is_empty()) {
            map.insert(
                key(D, "getcontenttype"),
                format!(
                    "<d:getcontenttype>{}</d:getcontenttype>",
                    if contacts {
                        "text/vcard"
                    } else {
                        "text/calendar"
                    }
                ),
            );
            map.insert(
                key(D, "getcontentlength"),
                format!("<d:getcontentlength>{}</d:getcontentlength>", content.len()),
            );
            if contacts {
                map.insert(
                    key(CARDDAV, "address-data"),
                    format!("<a:address-data>{}</a:address-data>", escape(content)),
                );
            } else {
                map.insert(
                    key(CALDAV, "calendar-data"),
                    format!("<c:calendar-data>{}</c:calendar-data>", escape(content)),
                );
            }
        }
        if is_collection && let Some(c) = property_collection {
            let (ns, reports) = if contacts {
                ("a", ["addressbook-query", "addressbook-multiget"])
            } else {
                ("c", ["calendar-query", "calendar-multiget"])
            };
            let set: String = reports
                .iter()
                .map(|r| {
                    format!(
                        "<d:supported-report><d:report><{ns}:{r}/></d:report></d:supported-report>"
                    )
                })
                .collect();
            map.insert(
                key(D, "supported-report-set"),
                format!("<d:supported-report-set>{set}</d:supported-report-set>"),
            );
            map.insert(
                key(CALENDARSERVER, "getctag"),
                format!(
                    "<cs:getctag>{}</cs:getctag>",
                    escape(&pim_etag(&c.id, c.revision))
                ),
            );
            if !contacts {
                map.insert(key(CALDAV, "supported-calendar-component-set"), "<c:supported-calendar-component-set><c:comp name=\"VEVENT\"/><c:comp name=\"VTODO\"/></c:supported-calendar-component-set>".into());
            }
        }
        map
    }
}

fn response(href: &str, properties: &Props, requested: Option<&[Xml]>) -> String {
    let wanted: Vec<(String, String)> = match requested {
        Some(nodes) => nodes
            .iter()
            .map(|n| (n.uri.clone(), n.name.clone()))
            .collect(),
        None => properties
            .keys()
            .filter(|k| !k.ends_with("-data"))
            .map(|k| {
                let (uri, name) = k.rsplit_once('|').unwrap();
                (uri.to_owned(), name.to_owned())
            })
            .collect(),
    };
    let (mut found, mut missing) = (String::new(), String::new());
    for (uri, name) in wanted {
        match properties.get(&key(&uri, &name)) {
            Some(content) => found.push_str(content),
            None => missing.push_str(&format!("<x:{name} xmlns:x=\"{}\"/>", escape(&uri))),
        }
    }
    let mut out = format!("<d:response><d:href>{}</d:href>", escape(href));
    if !found.is_empty() {
        out.push_str(&format!(
            "<d:propstat><d:prop>{found}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>"
        ));
    }
    if !missing.is_empty() {
        out.push_str(&format!("<d:propstat><d:prop>{missing}</d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>"));
    }
    out.push_str("</d:response>");
    out
}

fn multi(responses: Vec<String>) -> Response {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><d:multistatus xmlns:d=\"DAV:\" xmlns:a=\"{CARDDAV}\" xmlns:c=\"{CALDAV}\" xmlns:cs=\"{CALENDARSERVER}\">{}</d:multistatus>",
        responses.concat()
    );
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        body,
    )
        .into_response()
}

fn unsupported() -> DavError {
    DavError::new(403, "unsupportedFilter")
}

pub async fn handle(dav: &Dav, request: Request) -> DavResult<Response> {
    request.check_origin(&dav.origin)?;
    let mut response = serve(dav, &request)?;
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        "dav",
        HeaderValue::from_static("1, calendar-access, addressbook"),
    );
    h.insert(
        header::ALLOW,
        HeaderValue::from_static("OPTIONS, PROPFIND, REPORT, GET, HEAD, PUT, DELETE"),
    );
    Ok(response)
}

fn serve(dav: &Dav, request: &Request) -> DavResult<Response> {
    let method = request.method.as_str();
    if method == "OPTIONS" {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let (username, digest) = request.credentials()?;
    let write = matches!(method, "PUT" | "DELETE");
    let pathname = request.path.as_str();
    let mut kind = if pathname.starts_with("/dav/calendars/") {
        PimKind::Calendar
    } else {
        PimKind::Contacts
    };
    let app = |k: PimKind| {
        if k == PimKind::Contacts {
            App::Contacts
        } else {
            App::Calendar
        }
    };
    let denied = |e: &DavError| e.status == 401 || e.status == 403;
    let discovery = pathname == "/dav/" || pathname.starts_with("/dav/principals/");
    // The generic DAV root also exposes the Files home to WebDAV and SAF mounts.
    let files: Option<Principal> = if discovery {
        match dav
            .accounts
            .authenticate(&username, &digest, App::Files, false)
        {
            Ok(p) => Some(p),
            Err(e) if denied(&e) => None,
            Err(e) => return Err(e),
        }
    } else {
        None
    };
    let mut pim_access = true;
    let principal = match dav
        .accounts
        .authenticate(&username, &digest, app(kind), write)
    {
        Ok(p) => p,
        Err(e) if discovery && denied(&e) => {
            kind = PimKind::Calendar;
            match dav
                .accounts
                .authenticate(&username, &digest, App::Calendar, write)
            {
                Ok(p) => p,
                Err(e) if files.is_some() && denied(&e) => {
                    pim_access = false;
                    files.clone().unwrap()
                }
                Err(e) => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let session = principal.session_id.as_str();
    let user = encode_component(&principal.username);
    let home = format!(
        "/dav/{}/{user}/",
        if kind == PimKind::Contacts {
            "addressbooks"
        } else {
            "calendars"
        }
    );
    let principal_href = format!("/dav/principals/{user}/");
    let collections: Vec<Collection> = if pim_access {
        dav.pim
            .collections(session)?
            .into_iter()
            .filter(|c| c.kind == kind)
            .collect()
    } else {
        Vec::new()
    };
    let at_home = pathname == home;
    let at_root = matches!(pathname, "/dav/" | "/dav/addressbooks/" | "/dav/calendars/");
    let at_principal = pathname == principal_href;
    let tail: Vec<&str> = pathname
        .strip_prefix(&home)
        .map(|t| t.split('/').filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();
    if !at_root && !at_principal && !at_home && (tail.is_empty() || tail.len() > 2) {
        return Err(DavError::not_found());
    }
    let collection = tail
        .first()
        .and_then(|id| collections.iter().find(|c| c.id == *id));
    if !tail.is_empty() && collection.is_none() {
        return Err(DavError::not_found());
    }
    let href = tail.get(1).copied();
    let path = collection.map_or_else(|| pathname.to_owned(), |c| format!("{home}{}/", c.id));
    let items: Vec<Resource> = match collection {
        Some(c) => dav.pim.list(session, &c.id, write)?,
        None => Vec::new(),
    };
    let resource = href.and_then(|h| items.iter().find(|r| r.href == h));
    let ctx = Context {
        kind,
        principal: principal_href.clone(),
        user: user.clone(),
        pim_access,
        at_principal,
        collections: &collections,
    };

    let request_xml: Option<Xml> = if matches!(method, "PROPFIND" | "REPORT") {
        if request.body.len() > 262_144 {
            return Err(DavError::new(413, "requestTooLarge"));
        }
        let text =
            String::from_utf8(request.body.to_vec()).map_err(|_| DavError::bad("invalidXml"))?;
        if text.trim().is_empty() {
            None
        } else {
            Some(xml::parse(&text)?)
        }
    } else {
        None
    };
    let requested: Option<&[Xml]> = request_xml
        .as_ref()
        .and_then(|x| x.child(D, "prop"))
        .map(|p| p.children.as_slice());

    match method {
        "PROPFIND" => {
            let depth = request.header("depth").unwrap_or("1");
            if depth != "0" && depth != "1" {
                return Err(DavError::new(403, "finiteDepthRequired"));
            }
            if href.is_some() && resource.is_none() {
                return Err(DavError::not_found());
            }
            let name = resource
                .map(|r| r.href.clone())
                .or(collection.map(|c| c.name.clone()))
                .unwrap_or_else(|| principal.username.clone());
            let row = resource
                .map(Row::Resource)
                .or(collection.map(Row::Collection));
            let mut result = vec![response(
                pathname,
                &ctx.props(&name, href.is_none(), row),
                requested,
            )];
            if pathname == "/dav/"
                && depth != "0"
                && let Some(files) = &files
            {
                let mut props = Props::new();
                props.insert(
                    key(D, "displayname"),
                    "<d:displayname>Files</d:displayname>".into(),
                );
                props.insert(
                    key(D, "resourcetype"),
                    "<d:resourcetype><d:collection/></d:resourcetype>".into(),
                );
                result.push(response(
                    &format!("/dav/files/{}/", encode_component(&files.username)),
                    &props,
                    requested,
                ));
            }
            if depth != "0" && href.is_none() {
                if collection.is_some() {
                    for row in &items {
                        result.push(response(
                            &format!("{path}{}", row.href),
                            &ctx.props(&row.href, false, Some(Row::Resource(row))),
                            requested,
                        ));
                    }
                } else if at_home {
                    for c in &collections {
                        result.push(response(
                            &format!("{home}{}/", c.id),
                            &ctx.props(&c.name, true, Some(Row::Collection(c))),
                            requested,
                        ));
                    }
                }
            }
            Ok(multi(result))
        }
        "REPORT" => {
            let (Some(_), None, Some(report)) = (collection, href, request_xml.as_ref()) else {
                return Err(DavError::new(403, "invalidReport"));
            };
            let (ns, prefix) = if kind == PimKind::Contacts {
                (CARDDAV, "addressbook")
            } else {
                (CALDAV, "calendar")
            };
            if report.uri != ns
                || (report.name != format!("{prefix}-query")
                    && report.name != format!("{prefix}-multiget"))
            {
                return Err(DavError::new(403, "unsupportedReport"));
            }
            if report.name.ends_with("multiget") {
                let base =
                    url::Url::parse(&dav.origin).map_err(|_| DavError::bad("invalidHref"))?;
                let mut responses = Vec::new();
                for node in report.children(D, "href") {
                    let target = base
                        .join(node.text.trim())
                        .map_err(|_| DavError::bad("invalidHref"))?;
                    let name = target
                        .path()
                        .strip_prefix(path.as_str())
                        .filter(|n| !n.contains('/'));
                    let Some(name) = name.filter(|_| target.origin() == base.origin()) else {
                        return Err(DavError::bad("invalidHref"));
                    };
                    responses.push(match items.iter().find(|r| r.href == name) {
                        Some(row) => response(target.path(), &ctx.props(&row.href, false, Some(Row::Resource(row))), requested),
                        None => format!("<d:response><d:href>{}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>", escape(target.path())),
                    });
                }
                return Ok(multi(responses));
            }
            let mut rows: Vec<&Resource> = items.iter().collect();
            if let Some(filter) = report.child(ns, "filter") {
                rows = if kind == PimKind::Contacts {
                    contact_filter(filter, rows)?
                } else {
                    calendar_filter(filter, rows)?
                };
            }
            Ok(multi(
                rows.into_iter()
                    .map(|row| {
                        response(
                            &format!("{path}{}", row.href),
                            &ctx.props(&row.href, false, Some(Row::Resource(row))),
                            requested,
                        )
                    })
                    .collect(),
            ))
        }
        _ => {
            let (Some(collection), Some(href)) = (collection, href) else {
                return Err(DavError::new(405, "methodNotAllowed"));
            };
            match method {
                "GET" | "HEAD" => {
                    let resource = resource.ok_or(DavError::not_found())?;
                    let content_type = if kind == PimKind::Contacts {
                        "text/vcard; charset=utf-8"
                    } else {
                        "text/calendar; charset=utf-8"
                    };
                    let body = if method == "HEAD" {
                        String::new()
                    } else {
                        resource.content.clone()
                    };
                    let mut r = (
                        StatusCode::OK,
                        [
                            (header::CONTENT_TYPE, content_type.to_owned()),
                            (header::ETAG, pim_etag(&resource.id, resource.revision)),
                        ],
                        body,
                    )
                        .into_response();
                    r.headers_mut().insert(
                        header::CONTENT_LENGTH,
                        HeaderValue::from(resource.content.len()),
                    );
                    Ok(r)
                }
                "PUT" => {
                    let revision = require_match(request, resource)?;
                    if request.body.len() > 262_144 {
                        return Err(DavError::new(413, "requestTooLarge"));
                    }
                    let content = String::from_utf8(request.body.to_vec())
                        .map_err(|_| DavError::bad("invalidInput"))?;
                    let row = dav.pim.put(session, collection, href, &content, revision)?;
                    let status = if resource.is_some() {
                        StatusCode::NO_CONTENT
                    } else {
                        StatusCode::CREATED
                    };
                    Ok((status, [(header::ETAG, pim_etag(&row.id, row.revision))]).into_response())
                }
                "DELETE" => {
                    let resource = resource.ok_or(DavError::not_found())?;
                    let revision = require_match(request, Some(resource))?;
                    dav.pim.delete(session, &collection.id, href, revision)?;
                    Ok(StatusCode::NO_CONTENT.into_response())
                }
                _ => Err(DavError::new(405, "methodNotAllowed")),
            }
        }
    }
}

const COLLATIONS: [&str; 2] = ["i;unicode-casemap", "i;ascii-casemap"];
const MATCH_TYPES: [&str; 4] = ["contains", "equals", "starts-with", "ends-with"];

fn contact_filter<'a>(filter: &Xml, rows: Vec<&'a Resource>) -> DavResult<Vec<&'a Resource>> {
    let filters: Vec<&Xml> = filter.children(CARDDAV, "prop-filter").collect();
    if filter.children.len() != filters.len()
        || filter
            .attr("test")
            .is_some_and(|t| t != "anyof" && t != "allof")
    {
        return Err(unsupported());
    }
    for f in &filters {
        let name = f.attr("name").unwrap_or("");
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(unsupported());
        }
        let absent = f.children(CARDDAV, "is-not-defined").count();
        let tests: Vec<&Xml> = f.children(CARDDAV, "text-match").collect();
        if if absent > 0 {
            f.children.len() != 1
        } else {
            f.children.len() != tests.len()
        } {
            return Err(unsupported());
        }
        for t in tests {
            if t.attr("collation")
                .is_some_and(|c| !COLLATIONS.contains(&c))
                || t.attr("match-type")
                    .is_some_and(|m| !MATCH_TYPES.contains(&m))
                || t.attr("negate-condition")
                    .is_some_and(|n| n != "yes" && n != "no")
            {
                return Err(unsupported());
            }
        }
    }
    let any = filter.attr("test") == Some("anyof");
    let mut kept = Vec::new();
    for row in rows {
        let card = ical::parse(&row.content, "VCARD").map_err(|_| DavError::bad("invalidInput"))?;
        let results: Vec<bool> = filters
            .iter()
            .map(|f| {
                let name = f.attr("name").unwrap_or("").to_ascii_uppercase();
                let values: Vec<String> =
                    card.props_named(&name).flat_map(|p| p.values()).collect();
                if f.child(CARDDAV, "is-not-defined").is_some() {
                    return values.is_empty();
                }
                let tests: Vec<&Xml> = f.children(CARDDAV, "text-match").collect();
                if tests.is_empty() {
                    return !values.is_empty();
                }
                tests.iter().all(|t| {
                    let wanted = t.text.to_lowercase();
                    let kind = t.attr("match-type").unwrap_or("contains");
                    let matched = values.iter().any(|v| {
                        let text = v.to_lowercase();
                        match kind {
                            "equals" => text == wanted,
                            "starts-with" => text.starts_with(&wanted),
                            "ends-with" => text.ends_with(&wanted),
                            _ => text.contains(&wanted),
                        }
                    });
                    if t.attr("negate-condition") == Some("yes") {
                        !matched
                    } else {
                        matched
                    }
                })
            })
            .collect();
        if if any {
            results.iter().any(|r| *r)
        } else {
            results.iter().all(|r| *r)
        } {
            kept.push(row);
        }
    }
    Ok(kept)
}

fn calendar_filter<'a>(filter: &Xml, rows: Vec<&'a Resource>) -> DavResult<Vec<&'a Resource>> {
    let root = filter
        .child(CALDAV, "comp-filter")
        .ok_or_else(unsupported)?;
    let components: Vec<&Xml> = root.children(CALDAV, "comp-filter").collect();
    if filter.children.len() != 1
        || root.children.len() != components.len()
        || root.attr("name") != Some("VCALENDAR")
        || components.len() != 1
    {
        return Err(unsupported());
    }
    let component = components[0];
    let name = component.attr("name").unwrap_or("");
    if name != "VEVENT" && name != "VTODO" {
        return Err(unsupported());
    }
    let range = component.child(CALDAV, "time-range");
    let properties: Vec<&Xml> = component.children(CALDAV, "prop-filter").collect();
    if component.children.len() != usize::from(range.is_some()) + properties.len() {
        return Err(unsupported());
    }
    let parsed: Vec<(&Resource, Component)> = rows
        .into_iter()
        .filter(|r| ical::is_task(&r.content) == (name == "VTODO"))
        .map(|r| {
            ical::parse(&r.content, "VCALENDAR")
                .map(|c| (r, c))
                .map_err(|_| DavError::bad("invalidInput"))
        })
        .collect::<DavResult<_>>()?;
    let mut kept: Vec<(&Resource, Component)> = parsed;
    for property in properties {
        let prop = property.attr("name").unwrap_or("").to_ascii_uppercase();
        let absent = property.child(CALDAV, "is-not-defined");
        let text = property.child(CALDAV, "text-match");
        let expected_children = if absent.is_some() || text.is_some() {
            1
        } else {
            0
        };
        let collation = text.and_then(|t| t.attr("collation"));
        if prop.is_empty()
            || property.children.len() != expected_children
            || collation
                .is_some_and(|c| !["i;ascii-casemap", "i;unicode-casemap", "i;octet"].contains(&c))
        {
            return Err(unsupported());
        }
        kept.retain(|(_, root)| {
            let values: Vec<String> = root
                .components(name)
                .flat_map(|c| c.props_named(&prop).map(|p| p.value.clone()))
                .collect();
            if absent.is_some() {
                return values.is_empty();
            }
            let Some(text) = text else {
                return !values.is_empty();
            };
            let fold = |v: &str| {
                if collation == Some("i;octet") {
                    v.to_owned()
                } else {
                    v.to_lowercase()
                }
            };
            let matched = values.iter().any(|v| fold(v).contains(&fold(&text.text)));
            !values.is_empty()
                && if text.attr("negate-condition") == Some("yes") {
                    !matched
                } else {
                    matched
                }
        });
    }
    if let Some(range) = range {
        let (start, end) = (range.attr("start"), range.attr("end"));
        if start.is_none() && end.is_none() {
            return Err(DavError::bad("invalidTimeRange"));
        }
        let from = match start {
            Some(s) => ical::dav_date(s).ok_or(DavError::bad("invalidTimeRange"))?,
            None => ical::dav_date("00010101T000000Z").unwrap(),
        };
        let to = match end {
            Some(e) => ical::dav_date(e).ok_or(DavError::bad("invalidTimeRange"))?,
            None => ical::dav_date("99991231T235959Z").unwrap(),
        };
        if from >= to {
            return Err(DavError::bad("invalidTimeRange"));
        }
        kept.retain(|(_, root)| ical::matches_time_range(root, name, from, to));
    }
    Ok(kept.into_iter().map(|(r, _)| r).collect())
}
