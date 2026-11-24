//! Port of Dar's standards routes under `/dav`: Files over WebDAV (`next/runtime/files/dav.ts`)
//! and Contacts/Calendar over CardDAV/CalDAV (`next/runtime/pim/dav.ts`). Not yet wired into Dar.
//!
//! The host supplies app-password checks ([`Accounts`]), Files operations ([`files::FilesBackend`],
//! Dar's `service_dav` and upload reducers) and PIM operations ([`pim::PimBackend`], `service_pim`).
//! [`memory`] has in-memory versions for tests. Git over HTTP stays out of scope here.

pub mod files;
pub mod ical;
pub mod memory;
pub mod pim;
pub mod xml;

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use base64::Engine as _;
use sha2::{Digest, Sha256};

/// An error with Dar's status and code (`AppError`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DavError {
    pub status: u16,
    pub code: &'static str,
}

impl DavError {
    pub fn new(status: u16, code: &'static str) -> Self {
        DavError { status, code }
    }
    pub fn bad(code: &'static str) -> Self {
        DavError::new(400, code)
    }
    pub fn not_found() -> Self {
        DavError::new(404, "notFound")
    }
    pub fn unauthenticated() -> Self {
        DavError::new(401, "unauthenticated")
    }
}

impl std::fmt::Display for DavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.code)
    }
}

impl IntoResponse for DavError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::BAD_REQUEST);
        let mut response = (
            status,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            format!("{{\"error\":\"{}\"}}", self.code),
        )
            .into_response();
        if self.status == 401 {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Basic realm=\"Dar\", charset=\"UTF-8\""),
            );
        }
        response
    }
}

pub type DavResult<T> = Result<T, DavError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum App {
    Files,
    Contacts,
    Calendar,
}

/// The account and app-password session a request acts for.
#[derive(Clone, Debug)]
pub struct Principal {
    pub session_id: String,
    pub username: String,
}

/// App-password checks (`service_protocol_authenticate`, `service_pim_authenticate`).
pub trait Accounts: Send + Sync + 'static {
    /// `secret_digest` is the SHA-256 of the app password, hex encoded (`digestToken`).
    /// 401 for wrong credentials, 403 when the password lacks `app` or write access.
    fn authenticate(
        &self,
        username: &str,
        secret_digest: &str,
        app: App,
        write: bool,
    ) -> DavResult<Principal>;
}

#[derive(Clone)]
pub struct Dav {
    /// Dar's configured origin, for example `https://dar.example`.
    pub origin: String,
    pub accounts: Arc<dyn Accounts>,
    pub files: Arc<dyn files::FilesBackend>,
    pub pim: Arc<dyn pim::PimBackend>,
}

/// One standards request, already read.
pub struct Request {
    pub method: Method,
    /// The raw (percent-encoded) path.
    pub path: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// Basic credentials, with the app-password shape Dar issues (43 URL-safe characters).
    pub fn credentials(&self) -> DavResult<(String, String)> {
        let header = self
            .header("authorization")
            .filter(|h| h.starts_with("Basic ") && h.len() <= 2048)
            .ok_or(DavError::unauthenticated())?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(header[6..].trim())
            .map_err(|_| DavError::unauthenticated())?;
        let decoded = String::from_utf8(decoded).map_err(|_| DavError::unauthenticated())?;
        let (user, secret) = decoded
            .split_once(':')
            .filter(|(u, _)| !u.is_empty())
            .ok_or(DavError::unauthenticated())?;
        let shaped = secret.len() == 43
            && secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !shaped {
            return Err(DavError::unauthenticated());
        }
        Ok((user.to_owned(), digest_token(secret)))
    }

    /// A request from a browser page on another origin is refused.
    pub fn check_origin(&self, origin: &str) -> DavResult<()> {
        match self.header("origin") {
            Some(o) if o != origin => Err(DavError::new(403, "invalidOrigin")),
            _ => Ok(()),
        }
    }
}

pub fn digest_token(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `encodeURIComponent`.
pub fn encode_component(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `decodeURIComponent`, failing on malformed escapes or UTF-8.
pub fn decode_component(value: &str) -> DavResult<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = value
                .get(i + 1..i + 3)
                .ok_or(DavError::bad("invalidPath"))?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| DavError::bad("invalidPath"))?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| DavError::bad("invalidPath"))
}

pub fn http_date(ms: i64) -> String {
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms.max(0) as u64);
    httpdate::fmt_http_date(at)
}

pub(crate) fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

async fn handle(
    State(dav): State<Dav>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_owned();
    if matches!(
        path.as_str(),
        "/dav" | "/.well-known/carddav" | "/.well-known/caldav"
    ) {
        return (
            StatusCode::PERMANENT_REDIRECT,
            [(header::LOCATION, "/dav/")],
        )
            .into_response();
    }
    let request = Request {
        method,
        path,
        headers,
        body,
    };
    let result = if request.path.starts_with("/dav/files/") || request.path == "/dav/files" {
        files::handle(&dav, request).await
    } else {
        pim::handle(&dav, request).await
    };
    result.unwrap_or_else(IntoResponse::into_response)
}

/// `/dav/…` and the well-known redirects.
pub fn router(dav: Dav) -> Router {
    Router::new()
        .route("/dav", any(handle))
        .route("/dav/", any(handle))
        .route("/dav/{*rest}", any(handle))
        .route("/.well-known/carddav", any(handle))
        .route("/.well-known/caldav", any(handle))
        // Uploads are buffered in this prototype; Dar streams them to the blob store.
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024))
        .with_state(dav)
}
