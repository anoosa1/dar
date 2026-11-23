//! Files over WebDAV (`next/runtime/files/dav.ts`). Standards paths expose logical Files
//! resources; they never map to disk paths.

use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::xml::{self, D, Xml, escape};
use crate::{App, Dav, DavError, DavResult, Request, decode_component, encode_component, http_date};

const MICROSOFT: &str = "urn:schemas-microsoft-com:";
const OWNCLOUD: &str = "http://owncloud.org/ns";
/// Locks are advisory: LOCK always grants a token and writes do not check it. Finder and
/// Windows mount a server read-only unless it locks; a racing write keeps the earlier
/// content as a version.
const LOCK_SECONDS: u64 = 3600;
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Folder,
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// Empty for the root folder, which has no record.
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub size: u64,
    pub media_type: String,
    pub revision: u64,
    /// SHA-256 of the content, hex (content addressing).
    pub blob_id: String,
    pub modified_at: i64,
    pub sha1: Option<String>,
    pub md5: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Listed {
    pub entry: Entry,
    /// Relative to the listed folder.
    pub path: String,
}

/// Where a write lands (`PREPARE`).
#[derive(Clone, Debug)]
pub struct Target {
    pub entry: Option<Entry>,
    pub parent_id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    Move,
    Copy,
}

/// Dar's `service_dav` verbs plus the upload reducers and blob reads.
pub trait FilesBackend: Send + Sync + 'static {
    fn stat(&self, session: &str, path: &str) -> DavResult<Entry>;
    /// Depth 0, 1, or 2 for infinity. `operationTooLarge` when a tree is past the limit.
    fn list(&self, session: &str, path: &str, depth: u8) -> DavResult<(Entry, Vec<Listed>)>;
    fn prepare(&self, session: &str, path: &str) -> DavResult<Target>;
    /// Whether a folder was created.
    fn mkcol(&self, session: &str, path: &str) -> DavResult<bool>;
    fn touch(&self, session: &str, path: &str, modified_at: i64) -> DavResult<()>;
    fn delete(&self, session: &str, path: &str, expected_revision: u64) -> DavResult<()>;
    /// Whether an existing destination was replaced.
    fn transfer(&self, session: &str, how: Transfer, path: &str, destination: &str, expected_revision: u64, overwrite: bool) -> DavResult<bool>;
    /// Store content as the target's next version.
    fn store(&self, session: &str, target: &Target, content: &[u8], media_type: &str, file_time: i64) -> DavResult<Entry>;
    fn read(&self, session: &str, entry: &Entry) -> DavResult<Vec<u8>>;
}

pub fn etag(entry: &Entry) -> String {
    format!("\"{}:{}:{}\"", entry.id, entry.revision, entry.blob_id)
}

/// If-Match is optional: without it a write replaces the current file, whose earlier content
/// stays a version.
fn condition(request: &Request, entry: &Entry) -> DavResult<()> {
    match request.header("if-match").map(str::trim) {
        None | Some("*") => Ok(()),
        Some(value) if value.split(',').any(|v| v.trim() == etag(entry)) => Ok(()),
        Some(_) => Err(DavError::new(412, "revisionConflict")),
    }
}

/// Media types by extension (`mediaTypeOf` in `files/media-types.ts`).
pub fn media_type_of(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" => "text/markdown",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "html" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "ts" | "tsx" => "text/plain",
        "json" => "application/json",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// The client's media type, or one from the name for none or a generic one.
fn media_type(request: &Request, name: &str) -> String {
    let declared = request.header("content-type").map(|v| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase());
    let token = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b));
    match declared {
        Some(t) if t != "application/octet-stream" && t.len() <= 150 && t.split_once('/').is_some_and(|(a, b)| token(a) && token(b)) => t,
        _ => media_type_of(name).to_owned(),
    }
}

const LOCK_PROPERTIES: &str = "<d:supportedlock><d:lockentry><d:lockscope><d:exclusive/></d:lockscope><d:locktype><d:write/></d:locktype></d:lockentry><d:lockentry><d:lockscope><d:shared/></d:lockscope><d:locktype><d:write/></d:locktype></d:lockentry></d:supportedlock><d:lockdiscovery/>";

/// ownCloud's checksum property, which rclone and the ownCloud and Nextcloud clients read.
fn checksums(entry: &Entry) -> String {
    if entry.kind != Kind::File {
        return String::new();
    }
    let legacy = match (&entry.sha1, &entry.md5) {
        (Some(sha1), Some(md5)) => format!("SHA1:{sha1} MD5:{md5} "),
        _ => String::new(),
    };
    format!("<oc:checksums><oc:checksum>{legacy}SHA256:{}</oc:checksum></oc:checksums>", escape(&entry.blob_id))
}

fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn property_response(entry: &Entry, href: &str) -> String {
    format!(
        "<d:response><d:href>{}</d:href><d:propstat><d:prop><d:displayname>{}</d:displayname><d:resourcetype>{}</d:resourcetype><d:getcontentlength>{}</d:getcontentlength><d:getcontenttype>{}</d:getcontenttype><d:getetag>{}</d:getetag><d:getlastmodified>{}</d:getlastmodified><d:creationdate>{}</d:creationdate>{}{LOCK_PROPERTIES}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
        escape(href),
        escape(&entry.name),
        if entry.kind == Kind::Folder { "<d:collection/>" } else { "" },
        entry.size,
        escape(&entry.media_type),
        escape(&etag(entry)),
        http_date(entry.modified_at),
        iso(entry.modified_at),
        checksums(entry),
    )
}

/// Modified-time properties clients set: DAV:lastmodified (Unix seconds; rclone, ownCloud),
/// getlastmodified and Win32LastModifiedTime (HTTP dates).
fn patch(node: &Xml, set: bool) -> (u16, Option<i64>) {
    let text = node.text.trim();
    let unix_seconds = |t: &str| {
        let (whole, frac) = t.split_once('.').unwrap_or((t, "0"));
        (!whole.is_empty() && whole.len() <= 12 && whole.bytes().all(|b| b.is_ascii_digit()) && !frac.is_empty() && frac.bytes().all(|b| b.is_ascii_digit()))
            .then(|| t.parse::<f64>().ok())
            .flatten()
    };
    if set && node.is(D, "lastmodified")
        && let Some(seconds) = unix_seconds(text)
    {
        return (200, Some((seconds * 1000.0) as i64));
    }
    if set && (node.is(D, "getlastmodified") || node.is(MICROSOFT, "Win32LastModifiedTime"))
        && let Ok(time) = httpdate::parse_http_date(text)
    {
        let ms = time.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        return (200, Some(ms));
    }
    // Windows sets these on every save; they describe its local copy and change nothing here.
    if node.uri == MICROSOFT && ["Win32CreationTime", "Win32LastAccessTime", "Win32FileAttributes"].contains(&node.name.as_str()) {
        return (200, None);
    }
    // rclone sends back the checksums it read with each time it sets.
    if node.is(OWNCLOUD, "checksums") {
        return (200, None);
    }
    (403, None)
}

fn multistatus(body: String) -> Response {
    (StatusCode::MULTI_STATUS, [(header::CONTENT_TYPE, "application/xml; charset=utf-8")], body).into_response()
}

fn status(code: u16) -> Response {
    StatusCode::from_u16(code).unwrap_or(StatusCode::OK).into_response()
}

fn body_text(request: &Request, limit: usize) -> DavResult<String> {
    if request.body.len() > limit {
        return Err(DavError::new(413, "requestTooLarge"));
    }
    String::from_utf8(request.body.to_vec()).map_err(|_| DavError::bad("invalidXml"))
}

fn with_dav_headers(mut response: Response) -> Response {
    let h = response.headers_mut();
    h.insert(header::ALLOW, HeaderValue::from_static("OPTIONS, PROPFIND, PROPPATCH, GET, HEAD, PUT, MKCOL, DELETE, MOVE, COPY, LOCK, UNLOCK"));
    h.insert("dav", HeaderValue::from_static("1, 2"));
    h.insert("ms-author-via", HeaderValue::from_static("DAV"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub async fn handle(dav: &Dav, request: Request) -> DavResult<Response> {
    request.check_origin(&dav.origin)?;
    let response = match serve(dav, &request) {
        Ok(response) => response,
        Err(error) if error.status == 401 && request.header("authorization").is_none() => {
            let mut r = error.into_response();
            r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"Dar Files\", charset=\"UTF-8\""));
            r
        }
        Err(error) => error.into_response(),
    };
    Ok(with_dav_headers(response))
}

fn serve(dav: &Dav, request: &Request) -> DavResult<Response> {
    let method = request.method.as_str();
    if method == "OPTIONS" {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let (username, digest) = request.credentials()?;
    let write = !matches!(method, "GET" | "HEAD" | "PROPFIND");
    let principal = dav.accounts.authenticate(&username, &digest, App::Files, write)?;
    let session = principal.session_id.as_str();
    let prefix = format!("/dav/files/{}", encode_component(&principal.username));
    let logical = |pathname: &str| -> DavResult<String> {
        let rest = pathname.strip_prefix(&prefix).filter(|r| r.is_empty() || r.starts_with('/')).ok_or(DavError::new(403, "forbidden"))?;
        let mut pieces = Vec::new();
        for raw in rest.split('/').filter(|p| !p.is_empty()) {
            let piece = decode_component(raw)?;
            if piece == "." || piece == ".." || piece.chars().any(|c| (c as u32) < 0x20 || c == '/' || c == '\\') {
                return Err(DavError::bad("invalidPath"));
            }
            pieces.push(piece);
        }
        Ok(pieces.join("/"))
    };
    let path = logical(&request.path)?;
    let files = dav.files.as_ref();

    match method {
        "PROPFIND" => {
            // No Depth means infinity (RFC 4918); the backend takes 2 for it.
            let depth = match request.header("depth").unwrap_or("infinity").to_ascii_lowercase().as_str() {
                "0" => 0,
                "1" => 1,
                "infinity" => 2,
                _ => return Err(DavError::bad("invalidDepth")),
            };
            if request.body.len() > 16_384 {
                return Err(DavError::new(413, "requestTooLarge"));
            }
            let (root, children) = match files.list(session, &path, depth) {
                Ok(listed) => listed,
                // A tree past the listing limit is refused as RFC 4918 asks.
                Err(e) if depth == 2 && e.code == "operationTooLarge" => {
                    return Ok((StatusCode::FORBIDDEN, [(header::CONTENT_TYPE, "application/xml; charset=utf-8")], "<?xml version=\"1.0\" encoding=\"UTF-8\"?><d:error xmlns:d=\"DAV:\"><d:propfind-finite-depth/></d:error>").into_response());
                }
                Err(e) => return Err(e),
            };
            let encode = |p: &str| p.split('/').map(encode_component).collect::<Vec<_>>().join("/");
            let folder = |e: &Entry| if e.kind == Kind::Folder { "/" } else { "" };
            let href = format!("{prefix}{}{}", if path.is_empty() { String::new() } else { format!("/{}", encode(&path)) }, folder(&root));
            let mut out = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><d:multistatus xmlns:d=\"DAV:\" xmlns:oc=\"{OWNCLOUD}\">{}", property_response(&root, &href));
            for child in &children {
                out.push_str(&property_response(&child.entry, &format!("{href}{}{}", encode(&child.path), folder(&child.entry))));
            }
            out.push_str("</d:multistatus>");
            Ok(multistatus(out))
        }
        "PROPPATCH" => {
            let entry = files.stat(session, &path)?;
            condition(request, &entry)?;
            let body = xml::parse(&body_text(request, 16_384)?)?;
            if !body.is(D, "propertyupdate") {
                return Err(DavError::bad("invalidXml"));
            }
            let mut results: Vec<(Xml, u16, Option<i64>)> = Vec::new();
            for action in &body.children {
                if action.uri == D && (action.name == "set" || action.name == "remove") {
                    for prop in action.children(D, "prop") {
                        for node in &prop.children {
                            let (status, time) = patch(node, action.name == "set");
                            results.push((node.clone(), status, time));
                        }
                    }
                }
            }
            // The root folder has no record to date; properties apply together or not at all.
            if entry.id.is_empty() {
                for r in results.iter_mut().filter(|r| r.2.is_some()) {
                    r.1 = 403;
                }
            }
            let failed = results.iter().any(|r| r.1 != 200);
            if !failed && let Some(time) = results.iter().rev().find_map(|r| r.2) {
                files.touch(session, &path, time)?;
            }
            let mut groups: Vec<(u16, Vec<&Xml>)> = Vec::new();
            for (node, status, _) in &results {
                let status = if failed && *status == 200 { 424 } else { *status };
                match groups.iter_mut().find(|(s, _)| *s == status) {
                    Some((_, nodes)) => nodes.push(node),
                    None => groups.push((status, vec![node])),
                }
            }
            let element = |n: &Xml| if n.uri.is_empty() { format!("<{} xmlns=\"\"/>", n.name) } else { format!("<x:{} xmlns:x=\"{}\"/>", n.name, escape(&n.uri)) };
            let reason = |s: u16| match s {
                200 => "OK",
                403 => "Forbidden",
                _ => "Failed Dependency",
            };
            let propstats: String = groups
                .iter()
                .map(|(s, nodes)| format!("<d:propstat><d:prop>{}</d:prop><d:status>HTTP/1.1 {s} {}</d:status></d:propstat>", nodes.iter().map(|n| element(n)).collect::<String>(), reason(*s)))
                .collect();
            Ok(multistatus(format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>{}</d:href>{propstats}</d:response></d:multistatus>", escape(&request.path))))
        }
        "LOCK" => {
            let text = body_text(request, 16_384)?;
            let refresh = request.header("if").and_then(|v| {
                let start = v.find("<opaquelocktoken:")? + 1;
                let end = v[start..].find(['>', ' '])? + start;
                Some(v[start..end].to_owned())
            });
            let mut shared = false;
            if !text.trim().is_empty() {
                let body = xml::parse(&text)?;
                if !body.is(D, "lockinfo") {
                    return Err(DavError::bad("invalidXml"));
                }
                shared = body.child(D, "lockscope").is_some_and(|s| s.child(D, "shared").is_some());
            } else if refresh.is_none() {
                return Err(DavError::bad("invalidXml"));
            }
            let mut created = false;
            if !path.is_empty() {
                // Locking an unmapped URL creates an empty file (RFC 4918 §9.10.4).
                let target = files.prepare(session, &path)?;
                if target.entry.is_none() {
                    files.store(session, &target, &[], media_type_of(&target.name), 0)?;
                    created = true;
                }
            }
            let requested = request
                .header("timeout")
                .and_then(|t| t.to_ascii_lowercase().split("second-").nth(1).and_then(|n| n.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u64>().ok()))
                .unwrap_or(LOCK_SECONDS);
            let token = refresh.unwrap_or_else(|| format!("opaquelocktoken:{}", uuid()));
            let body = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><d:prop xmlns:d=\"DAV:\"><d:lockdiscovery><d:activelock><d:locktype><d:write/></d:locktype><d:lockscope><d:{}/></d:lockscope><d:depth>{}</d:depth><d:timeout>Second-{}</d:timeout><d:locktoken><d:href>{}</d:href></d:locktoken><d:lockroot><d:href>{}</d:href></d:lockroot></d:activelock></d:lockdiscovery></d:prop>",
                if shared { "shared" } else { "exclusive" },
                if request.header("depth") == Some("0") { "0" } else { "infinity" },
                requested.clamp(1, LOCK_SECONDS),
                escape(&token),
                escape(&request.path),
            );
            let status = if created { StatusCode::CREATED } else { StatusCode::OK };
            Ok((status, [(header::CONTENT_TYPE, "application/xml; charset=utf-8".to_owned()), (header::HeaderName::from_static("lock-token"), format!("<{token}>"))], body).into_response())
        }
        "UNLOCK" => {
            if request.header("lock-token").is_none() {
                return Err(DavError::bad("lockTokenRequired"));
            }
            files.stat(session, &path)?;
            Ok(status(204))
        }
        "GET" | "HEAD" => {
            let entry = files.stat(session, &path)?;
            if entry.kind != Kind::File {
                return Err(DavError::new(405, "isDirectory"));
            }
            let content = files.read(session, &entry)?;
            Ok(download(request, &entry, content))
        }
        "MKCOL" => {
            if request.header("content-length").is_some_and(|l| l != "0") || !request.body.is_empty() {
                return Err(DavError::new(415, "unsupportedBody"));
            }
            // An existing resource answers 405, which clients take as "already there".
            let exists = match files.stat(session, &path) {
                Ok(_) => true,
                Err(e) if e.code == "notFound" => false,
                Err(e) => return Err(e),
            };
            if exists || !files.mkcol(session, &path)? {
                return Err(DavError::new(405, "alreadyExists"));
            }
            Ok(status(201))
        }
        "PUT" => {
            let target = files.prepare(session, &path)?;
            if let Some(entry) = &target.entry {
                if entry.kind != Kind::File {
                    return Err(DavError::new(405, "isDirectory"));
                }
                if request.header("if-none-match") == Some("*") {
                    return Err(DavError::new(412, "revisionConflict"));
                }
                condition(request, entry)?;
            }
            // Finder streams uploads without Content-Length and states their size in X-Expected-Entity-Length.
            let length = request.header("content-length").or(request.header("x-expected-entity-length")).ok_or(DavError::new(411, "lengthRequired"))?;
            if length.is_empty() || length.len() > 16 || !length.bytes().all(|b| b.is_ascii_digit()) {
                return Err(DavError::bad("invalidInput"));
            }
            let size: u64 = length.parse().map_err(|_| DavError::bad("invalidInput"))?;
            if size > MAX_FILE_BYTES {
                return Err(DavError::new(413, "uploadTooLarge"));
            }
            if size != request.body.len() as u64 {
                return Err(DavError::bad("incompleteUpload"));
            }
            // rclone, ownCloud and Nextcloud clients send the file's modified time in Unix seconds.
            let file_time = request.header("x-oc-mtime").and_then(|m| {
                let ok = m.split('.').next().is_some_and(|w| !w.is_empty() && w.len() <= 12 && w.bytes().all(|b| b.is_ascii_digit()));
                ok.then(|| m.parse::<f64>().ok()).flatten().map(|s| (s * 1000.0) as i64)
            });
            let stored = files.store(session, &target, &request.body, &media_type(request, &target.name), file_time.unwrap_or(0))?;
            let mut response = status(if target.entry.is_some() { 204 } else { 201 });
            if let Ok(tag) = HeaderValue::from_str(&etag(&stored)) {
                response.headers_mut().insert(header::ETAG, tag);
            }
            if file_time.is_some() {
                response.headers_mut().insert("x-oc-mtime", HeaderValue::from_static("accepted"));
            }
            Ok(response)
        }
        "DELETE" | "MOVE" | "COPY" => {
            let entry = files.stat(session, &path)?;
            condition(request, &entry)?;
            if method == "DELETE" {
                files.delete(session, &path, entry.revision)?;
                return Ok(status(204));
            }
            let destination = request.header("destination").ok_or(DavError::bad("destinationRequired"))?;
            let base = url::Url::parse(&dav.origin).map_err(|_| DavError::bad("invalidDestination"))?;
            let target = base.join(destination).map_err(|_| DavError::bad("invalidDestination"))?;
            if target.origin() != base.origin() || target.query().is_some() || target.fragment().is_some() {
                return Err(DavError::bad("invalidDestination"));
            }
            let destination = logical(target.path())?;
            let how = if method == "MOVE" { Transfer::Move } else { Transfer::Copy };
            let replaced = files.transfer(session, how, &path, &destination, entry.revision, request.header("overwrite") != Some("F"))?;
            Ok(status(if replaced { 204 } else { 201 }))
        }
        _ => Err(DavError::new(405, "methodNotAllowed")),
    }
}

/// GET or HEAD with a single byte range.
fn download(request: &Request, entry: &Entry, content: Vec<u8>) -> Response {
    let total = content.len() as u64;
    let range = request.header("range").and_then(|r| r.strip_prefix("bytes=")).filter(|r| !r.contains(',')).and_then(|r| {
        let (a, b) = r.split_once('-')?;
        let (start, end) = match (a.parse::<u64>().ok(), b.parse::<u64>().ok()) {
            (Some(s), Some(e)) => (s, e.min(total.saturating_sub(1))),
            (Some(s), None) if b.is_empty() => (s, total.saturating_sub(1)),
            (None, Some(n)) if a.is_empty() => (total.saturating_sub(n), total.saturating_sub(1)),
            _ => return None,
        };
        (start <= end && start < total).then_some((start, end))
    });
    let head = request.method == Method::HEAD;
    let mut headers = vec![
        (header::CONTENT_TYPE, entry.media_type.clone()),
        (header::ETAG, etag(entry)),
        (header::LAST_MODIFIED, http_date(entry.modified_at)),
        (header::ACCEPT_RANGES, "bytes".into()),
    ];
    let (status, body) = match range {
        Some((start, end)) => {
            headers.push((header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}")));
            (StatusCode::PARTIAL_CONTENT, content[start as usize..=end as usize].to_vec())
        }
        None if request.header("range").is_some() && total > 0 && request.header("range").is_some_and(|r| r.starts_with("bytes=") && !r.contains(',')) => {
            return (StatusCode::RANGE_NOT_SATISFIABLE, [(header::CONTENT_RANGE, format!("bytes */{total}"))]).into_response();
        }
        None => (StatusCode::OK, content),
    };
    headers.push((header::CONTENT_LENGTH, body.len().to_string()));
    let mut response = (status, if head { Vec::new() } else { body }).into_response();
    for (name, value) in headers {
        if let Ok(v) = HeaderValue::from_str(&value) {
            response.headers_mut().insert(name, v);
        }
    }
    response
}

pub(crate) fn uuid() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("random source");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}
