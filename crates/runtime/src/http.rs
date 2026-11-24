//! Dar's HTTP surface for Files (`runtime/server.ts`): the issuer documents, sign-in, the
//! session's API (browser tokens, uploads, downloads, picker, Locked folder, search,
//! preferences, app passwords) and the client's pages. Writes need the instance's own
//! Origin, and cookie-authenticated writes a CSRF token bound to the session.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use tokio::sync::Semaphore;

use crate::auth::{self, Issuer};
use crate::blobs::{BlobFile, Blobs};
use crate::db::Database;
use crate::error::{AppError, Result};

pub struct Config {
    pub origin: String,
    pub development: bool,
    pub database_public_url: String,
    pub database_name: String,
    pub client_dist: PathBuf,
}

pub struct Runtime {
    pub config: Config,
    pub root: PathBuf,
    pub db: Database,
    pub issuer: Issuer,
    pub blobs: Blobs,
    pub bootstrap: String,
    pub oidc: dar_oidc::Provider,
    pub identity: Arc<crate::oidc::Oidc>,
    #[allow(dead_code)]
    pub thumbnails: crate::thumbnails::Thumbnails,
    /// Sign-in and unlock attempts per peer or account: (count, window end in ms).
    attempts: Mutex<HashMap<String, (u32, f64)>>,
    /// Password hashing is memory-hard; at most two run at once.
    hashing: Semaphore,
    pub installed: crate::installed::Installed,
    pub git: crate::git::Repositories,
    pub notes: crate::notes_library::Library,
    pub checksums: crate::maintenance::Worker,
    pub texts: crate::maintenance::Worker,
    pub subscriptions: crate::maintenance::Subscriptions,
    pub news: crate::news::NewsRuntime,
    pub mail: crate::mail::MailRuntime,
    pub books: crate::books::BooksRuntime,
    pub photos: Arc<crate::photos::PhotosRuntime>,
    pub immich: crate::photos::ImmichConnector,
    pub chat: crate::chat::ChatRuntime,
    pub agent: crate::agent::AgentRuntime,
}

pub type Shared = Arc<Runtime>;

const MAX_FILE_BYTES: f64 = 16.0 * 1024.0 * 1024.0 * 1024.0;

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis() as f64
}

/// A JSON response, as `writeJson` sends it.
pub(crate) fn json_response(value: &Value, status: StatusCode) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], value.to_string()).into_response()
}

pub(crate) fn ok(value: Value) -> Result<Response> {
    Ok(json_response(&value, StatusCode::OK))
}

pub(crate) fn json_ok(value: &Value) -> Response {
    json_response(value, StatusCode::OK)
}

/// A string of at most `max` UTF-8 bytes (`string` in server.ts).
pub(crate) fn string(value: Option<&Value>, max: usize) -> Result<String> {
    match value {
        Some(Value::String(s)) if s.len() <= max => Ok(s.clone()),
        _ => Err(AppError::invalid()),
    }
}

pub(crate) fn integer(value: Option<&Value>, max: f64) -> Result<f64> {
    match value.and_then(Value::as_f64) {
        Some(n) if n.fract() == 0.0 && (0.0..=max).contains(&n) && n <= 9_007_199_254_740_991.0 => Ok(n),
        _ => Err(AppError::invalid()),
    }
}

/// A JSON object request body (`body` in server.ts): JSON content type, at most `limit` bytes.
pub(crate) async fn body_object(body: Body, limit: usize, headers: &HeaderMap) -> Result<Map<String, Value>> {
    let json = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("application/json"));
    if !json {
        return Err(AppError::new("invalidContentType", 415));
    }
    let bytes = axum::body::to_bytes(body, limit).await.map_err(|_| AppError::new("requestTooLarge", 413))?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(AppError::new("invalidJson", 400)),
    }
}

impl Runtime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        db: Database,
        issuer: Issuer,
        blobs: Blobs,
        bootstrap: String,
        identity: Arc<crate::oidc::Oidc>,
        root: &std::path::Path,
        registry: crate::apps::Registry,
        news: crate::news::NewsRuntime,
        mail: crate::mail::MailRuntime,
        books: crate::books::BooksRuntime,
        photos: Arc<crate::photos::PhotosRuntime>,
        immich: crate::photos::ImmichConnector,
        chat: crate::chat::ChatRuntime,
        agent: crate::agent::AgentRuntime,
    ) -> Self {
        let oidc = identity.provider.clone();
        Runtime {
            config,
            root: root.to_path_buf(),
            db: db.clone(),
            issuer,
            blobs: blobs.clone(),
            bootstrap,
            oidc,
            identity,
            thumbnails: crate::thumbnails::Thumbnails::new(root),
            attempts: Mutex::new(HashMap::new()),
            hashing: Semaphore::new(2),
            installed: crate::installed::Installed::new(registry),
            git: crate::git::Repositories::new(db.clone(), root, blobs.clone()),
            notes: crate::notes_library::Library::default(),
            checksums: crate::maintenance::checksum_worker(db.clone(), blobs.clone()),
            texts: crate::maintenance::text_index_worker(db.clone(), blobs.clone()),
            subscriptions: crate::maintenance::Subscriptions::new(db.clone(), std::env::var("DAR_NEWS_ALLOW_PRIVATE_NETWORKS").as_deref() == Ok("1")),
            news,
            mail,
            books,
            photos,
            immich,
            chat,
            agent,
        }
    }

    /// Startup: settings decide which installed apps run; the module learns their descriptors.
    pub async fn publish_installed_apps(&self) -> Result<()> {
        self.installed.start(&self.db).await
    }

    fn cookie_name(&self) -> &'static str {
        if self.config.development { "dar_session" } else { "__Host-dar_session" }
    }

    fn cookie(&self, value: &str, age: u64) -> HeaderValue {
        let secure = if self.config.development { "" } else { "; Secure" };
        HeaderValue::from_str(&format!("{}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={age}{secure}", self.cookie_name())).expect("cookie")
    }

    /// Counts an attempt for `key`; more than ten a minute are refused.
    fn attempt(&self, key: &str) -> Result<()> {
        self.attempt_up_to(key, 10)
    }

    /// Counts an attempt for `key`; more than `limit` a minute are refused.
    pub fn attempt_up_to(&self, key: &str, limit: u32) -> Result<()> {
        let mut attempts = self.attempts.lock().expect("attempts");
        let at = now_ms();
        let fresh = !attempts.get(key).is_some_and(|(_, until)| *until > at);
        if fresh && attempts.len() >= 10_000 {
            attempts.retain(|_, (_, until)| *until > at);
            if attempts.len() >= 10_000 {
                return Err(AppError::with("rateLimited", 429, "Too many attempts. Wait a minute and try again."));
            }
        }
        let entry = attempts.entry(key.to_owned()).or_insert((0, at + 60_000.0));
        if fresh {
            *entry = (0, at + 60_000.0);
        }
        entry.0 += 1;
        if entry.0 > limit {
            return Err(AppError::with("rateLimited", 429, "Too many attempts. Wait a minute and try again."));
        }
        Ok(())
    }

    /// Drops attempt windows that ended (maintenance).
    pub fn forget_old_attempts(&self) {
        let at = now_ms();
        self.attempts.lock().expect("attempts").retain(|_, (_, until)| *until > at);
    }

    pub fn succeeded(&self, key: &str) {
        if let Some(entry) = self.attempts.lock().expect("attempts").get_mut(key) {
            entry.0 = entry.0.saturating_sub(1);
        }
    }

    async fn hash(&self, password: String) -> Result<String> {
        let _turn = self.hashing.try_acquire().map_err(|_| AppError::with("rateLimited", 429, "Try signing in again shortly."))?;
        tokio::task::spawn_blocking(move || auth::hash_password(&password)).await.map_err(|_| AppError::internal())
    }

    async fn verify(&self, password: String, verifier: String) -> Result<bool> {
        let _turn = self.hashing.try_acquire().map_err(|_| AppError::with("rateLimited", 429, "Try signing in again shortly."))?;
        tokio::task::spawn_blocking(move || auth::verify_password(&password, &verifier)).await.map_err(|_| AppError::internal())
    }

    /// A deliberately failing verifier, so unknown usernames take as long as known ones.
    fn dummy() -> &'static str {
        "scrypt$131072$8$1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    }

    async fn login(&self, username: &str, password: &str) -> Result<(String, String, Value)> {
        let account = self.db.call("service_account", json!([username.to_lowercase()])).await?;
        let verifier = account["verifier"].as_str().unwrap_or(Self::dummy()).to_owned();
        let valid = self.verify(password.to_owned(), verifier.clone()).await?;
        if !valid || account["enabled"] != json!(true) {
            return Err(AppError::with("invalidCredentials", 401, "The username or password is incorrect."));
        }
        let (secret, digest) = auth::opaque_token();
        let csrf = self.issuer.csrf(&secret);
        let expires = now_ms() + 7.0 * 86_400_000.0;
        self.db.call("service_create_session", json!([auth::uuid(), account["id"], digest, auth::digest_token(&csrf), expires, verifier])).await?;
        let user = json!({ "id": account["id"], "username": account["username"], "displayName": account["displayName"], "administrator": account["administrator"] });
        Ok((secret, csrf, user))
    }

    async fn authenticate(&self, secret: Option<&str>) -> Result<Option<Value>> {
        let Some(secret) = secret.filter(|s| auth::is_token(s)) else { return Ok(None) };
        let value = self.db.read("service_session", json!([auth::digest_token(secret)])).await?;
        Ok((!value.is_null()).then_some(value))
    }

    /// A new account (`authService.createAccount`); returns its id.
    pub async fn create_account(&self, username: &str, display_name: &str, password: &str, first: bool) -> Result<String> {
        auth::validate_new_password(password)?;
        let verifier = self.hash(password.to_owned()).await?;
        let id = auth::uuid();
        self.db.call("service_create_account", json!([id, username.to_lowercase(), display_name, verifier, first])).await?;
        Ok(id)
    }

    /// `authService.changePassword`: the current password, then the new one; every session ends.
    pub async fn change_password(&self, user_id: &str, username: &str, current: &str, replacement: &str) -> Result<()> {
        auth::validate_new_password(replacement)?;
        let account = self.db.call("service_account", json!([username])).await?;
        let verifier = account["verifier"].as_str().unwrap_or(Self::dummy()).to_owned();
        if !self.verify(current.to_owned(), verifier.clone()).await? {
            return Err(AppError::new("invalidCredentials", 401));
        }
        let next = self.hash(replacement.to_owned()).await?;
        self.db.call("service_change_password", json!([user_id, next, verifier])).await?;
        Ok(())
    }

    /// A verifier for a provisioned password (`provisionPassword`).
    pub async fn provision_password(&self, password: &str) -> Result<String> {
        auth::validate_new_password(password)?;
        self.hash(password.to_owned()).await
    }

    /// The account's verifier after checking `password` (`authService.confirm`).
    pub async fn confirm(&self, username: &str, password: &str) -> Result<String> {
        self.confirm_password(username, password).await.map_err(|e| if e.code == "invalidCredentials" { AppError::new("invalidCredentials", 401) } else { e })
    }

    /// The account's verifier after checking `password` (Locked folder unlocks).
    pub async fn confirm_password(&self, username: &str, password: &str) -> Result<String> {
        let account = self.db.call("service_account", json!([username])).await?;
        let verifier = account["verifier"].as_str().unwrap_or(Self::dummy()).to_owned();
        if !self.verify(password.to_owned(), verifier.clone()).await? {
            return Err(AppError::with("invalidCredentials", 403, "That password is not correct."));
        }
        Ok(verifier)
    }
}


fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    cookies.split(';').map(str::trim).find_map(|part| part.strip_prefix(name)?.strip_prefix('=').map(str::to_owned))
}

fn set_security_headers(runtime: &Runtime, response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    if !runtime.config.development {
        headers.insert("strict-transport-security", HeaderValue::from_static("max-age=31536000"));
    }
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        let database = runtime.config.database_public_url.trim_end_matches('/').to_owned();
        let ws = |value: &str| value.replacen("http", "ws", 1);
        // WebAssembly needs 'wasm-unsafe-eval'; scripts, workers and styles still come only from this origin.
        let policy = format!(
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; style-src 'self'; img-src 'self' blob:; media-src 'self' https:; connect-src 'self' {} {database} {}; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            ws(&runtime.config.origin),
            ws(&database)
        );
        headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_str(&policy).expect("policy"));
    }
}

pub async fn chat_live(State(runtime): State<Shared>, request: Request) -> Response {
    let (mut parts, _body) = request.into_parts();
    let origin = parts.headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if origin != Some(runtime.config.origin.as_str()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let secret = cookie_value(&parts.headers, runtime.cookie_name());
    let authenticated = match runtime.authenticate(secret.as_deref()).await {
        Ok(Some(auth)) => auth,
        _ => return StatusCode::UNAUTHORIZED.into_response(),
    };
    if crate::installed::app_disabled(&runtime.db, "chat").await.unwrap_or(true) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let query = parts.uri.query().map(url_params).unwrap_or_default();
    let account_id = query.get("accountId").cloned().unwrap_or_default();
    let session_id = authenticated["session"]["id"].as_str().unwrap_or_default().to_owned();
    if runtime.chat.authorize_live(&session_id, &account_id).await.is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let ws = match <axum::extract::ws::WebSocketUpgrade as axum::extract::FromRequestParts<Shared>>::from_request_parts(&mut parts, &runtime).await {
        Ok(ws) => ws,
        Err(err) => return err.into_response(),
    };
    let runtime_clone = runtime.clone();
    ws.max_message_size(64 * 1024).on_upgrade(move |socket| async move {
        runtime_clone.chat.attach_live(account_id, socket);
    })
}

pub async fn handle(State(runtime): State<Shared>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    let (method, path) = (request.method().clone(), request.uri().path().to_owned());
    let mut response = match route(&runtime, peer, request).await {
        Ok(response) => response,
        Err(error) => {
            let status = error.status;
            let mut r = error.into_response();
            if status == 401 && path.starts_with("/dav/") {
                r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"Dar\", charset=\"UTF-8\""));
            }
            r
        }
    };
    set_security_headers(&runtime, &mut response);
    // A saved note or text file becomes searchable by its text within moments; a new or changed
    // calendar subscription refreshes soon.
    if method != Method::GET && method != Method::HEAD && response.status().as_u16() < 400 {
        if path.starts_with("/api/v1/files/") || path.starts_with("/api/v1/notes/") {
            runtime.texts.nudge();
        }
        if path.starts_with("/api/v1/calendar/") && (path.ends_with("/subscription") || path.ends_with("/refresh") || path.ends_with("/calendars")) {
            runtime.subscriptions.poke();
        }
    }
    response
}

/// The client's address. Only the canonical local TLS proxy may supply `x-dar-client-ip`.
fn peer_of(runtime: &Runtime, peer: SocketAddr, headers: &HeaderMap) -> String {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let host = runtime.config.origin.split_once("://").map(|(_, h)| h).unwrap_or_default();
    let forwarded = header("x-dar-client-ip").filter(|ip| ip.parse::<std::net::IpAddr>().is_ok());
    match forwarded {
        Some(ip) if !runtime.config.development && peer.ip().is_loopback() && header("host") == Some(host) && header("x-forwarded-proto") == Some("https") => ip.to_owned(),
        _ => peer.ip().to_string(),
    }
}

/// The routes of `server.ts`, in its order.
async fn route(shared: &Shared, peer: SocketAddr, request: Request) -> Result<Response> {
    let runtime: &Runtime = shared;
    let (parts, body) = request.into_parts();
    let path = parts.uri.path().to_owned();
    let query: HashMap<String, String> = parts.uri.query().map(url_params).unwrap_or_default();
    let method = parts.method.clone();
    let headers = parts.headers;
    if path.starts_with("/appAssets/") {
        return runtime.installed.asset(&method, &path);
    }
    let issuer_path = format!("{}/", url_path(&runtime.issuer.issuer));
    if let Some(rest) = path.strip_prefix(&issuer_path) {
        return match rest {
            ".well-known/openid-configuration" => ok(json!({ "issuer": runtime.issuer.issuer, "jwks_uri": format!("{}/jwks", runtime.issuer.issuer), "id_token_signing_alg_values_supported": ["ES256"], "subject_types_supported": ["public"] })),
            "jwks" => ok(runtime.issuer.jwks()),
            _ => Err(AppError::not_found()),
        };
    }
    // The self-hosted script that submits the provider's form_post page (no inline code).
    if path == "/oidc-form.js" && method == Method::GET {
        return Ok(([(header::CONTENT_TYPE, "text/javascript"), (header::CACHE_CONTROL, "public, max-age=3600")], r#"document.getElementById("oidc-response")?.requestSubmit();"#).into_response());
    }
    if path == "/api/v1/auth/status" && method == Method::GET {
        return ok(runtime.db.read("service_status", json!([])).await?);
    }
    if (path == "/api/v1/appearance" || path == "/api/v1/appearance/logo") && method == Method::GET {
        let row = runtime.db.read("service_appearance", json!(["", 0, ""])).await?;
        let value: Value = serde_json::from_str(row["value"].as_str().unwrap_or("{}")).unwrap_or_default();
        let logo = value["logo"].as_str().unwrap_or_default();
        if path.ends_with("/logo") {
            if logo.is_empty() {
                return Err(AppError::not_found());
            }
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD.decode(logo).map_err(|_| AppError::not_found())?;
            return Ok(([(header::CONTENT_TYPE, value["mediaType"].as_str().unwrap_or("image/png").to_owned()), (header::CACHE_CONTROL, "no-store".into())], bytes).into_response());
        }
        let url = (!logo.is_empty()).then(|| format!("/api/v1/appearance/logo?revision={}", row["revision"]));
        return ok(json!({ "revision": row["revision"], "logoUrl": url }));
    }
    if let Some(token) = path.strip_prefix("/public/files/")
        && (method == Method::GET || method == Method::HEAD)
    {
        if !auth::is_token(token) {
            return Err(AppError::not_found());
        }
        let entry: BlobFile = serde_json::from_value(runtime.db.read("service_public_download", json!([auth::digest_token(token)])).await?).map_err(|_| AppError::not_found())?;
        return runtime.blobs.download(&method, &headers, &entry, false).await;
    }
    if path == "/scim/v2" || path.starts_with("/scim/v2/") {
        return Ok(crate::identity_http::handle_scim(runtime, &method, &path, &query, &headers, body).await);
    }
    if path == crate::news::NEXTCLOUD_BASE || path.starts_with(&format!("{}/", crate::news::NEXTCLOUD_BASE)) {
        if crate::installed::app_disabled(&runtime.db, "news").await? {
            return Err(AppError::not_found());
        }
        return crate::news::handle_nextcloud(runtime, &method, &path, &query, &headers, &runtime.news, body).await;
    }
    // The Immich mobile app: it finds its API through /.well-known/immich (native client, no Origin header).
    if path == "/.well-known/immich" && method == Method::GET {
        if crate::installed::app_disabled(&runtime.db, "photos").await? {
            return Err(AppError::not_found());
        }
        return ok(crate::photos::immich_well_known());
    }
    if path == crate::photos::IMMICH_BASE || path.starts_with(&format!("{}/", crate::photos::IMMICH_BASE)) {
        if crate::installed::app_disabled(&runtime.db, "photos").await? {
            return Err(AppError::not_found());
        }
        return crate::photos::handle_immich(runtime, &method, &path, &query, &headers, &runtime.photos, &runtime.root, body).await;
    }
    // Git clients (no browser): folder history over smart HTTP, signed in with an app password.
    if path.starts_with("/dav/git/") {
        return crate::git::handle_http(shared.clone(), &method, &path, &query, &headers, body).await;
    }
    let secret = cookie_value(&headers, runtime.cookie_name());
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let writes = matches!(method, Method::POST | Method::PUT | Method::PATCH | Method::DELETE);
    if writes && origin.as_deref() != Some(runtime.config.origin.as_str()) {
        return Err(AppError::new("invalidOrigin", 403));
    }
    // Photos shared links: visitors have no session.
    if path.starts_with("/public/photos/") {
        if crate::installed::app_disabled(&runtime.db, "photos").await? {
            return Err(AppError::with("notFound", 404, "This link is no longer available."));
        }
        let peer_str = peer_of(runtime, peer, &headers);
        return crate::photos::handle_public(runtime, &method, &path, &query, &headers, &peer_str, &runtime.photos, &runtime.root, body).await;
    }
    if (path == "/api/v1/auth/login" || path == "/api/v1/auth/setup") && method == Method::POST {
        let key = peer_of(runtime, peer, &headers);
        runtime.attempt(&key)?;
        let p = body_object(body, 16_384, &headers).await?;
        let (username, password) = (string(p.get("username"), 64)?, string(p.get("password"), 1024)?);
        if path.ends_with("/setup") {
            let supplied = string(p.get("bootstrapToken"), 512)?;
            if !bool::from(subtle::ConstantTimeEq::ct_eq(supplied.as_bytes(), runtime.bootstrap.as_bytes())) {
                return Err(AppError::new("forbidden", 403));
            }
            runtime.create_account(&username, &string(p.get("displayName"), 100)?, &password, true).await?;
        }
        let (secret, csrf, user) = runtime.login(&username, &password).await?;
        runtime.succeeded(&key);
        let mut response = json_response(&json!({ "user": user, "csrfToken": csrf }), StatusCode::OK);
        response.headers_mut().insert(header::SET_COOKIE, runtime.cookie(&secret, 7 * 86400));
        return Ok(response);
    }
    // The OAuth provider sends the browser back here from its own site, so the SameSite=Strict
    // session cookie is absent: the one-time `state` names the session that started it.
    if path == "/api/v1/mail/oauth/callback" && method == Method::GET {
        if crate::installed::app_disabled(&runtime.db, "mail").await? {
            return Err(AppError::not_found());
        }
        let target = runtime.mail.oauth_callback(&query).await;
        return Ok((StatusCode::SEE_OTHER, [(header::LOCATION, target)]).into_response());
    }
    if path.starts_with("/api/") {
        let authenticated = runtime.authenticate(secret.as_deref()).await?.ok_or(AppError::new("unauthenticated", 401))?;
        if writes {
            let csrf = headers.get("x-csrf-token").and_then(|v| v.to_str().ok()).unwrap_or_default();
            if !auth::matches_token(csrf, authenticated["session"]["csrfDigest"].as_str().unwrap_or_default()) {
                return Err(AppError::new("csrfRejected", 403));
            }
        }
        let request = Api { runtime, method: &method, path: &path, query: &query, headers: &headers, authenticated: &authenticated, secret: secret.as_deref().unwrap_or_default(), peer: peer_of(runtime, peer, &headers), raw_peer: peer };
        return api(request, body).await;
    }
    if method != Method::GET && method != Method::HEAD {
        return Err(AppError::not_found());
    }
    // The sign-in-with-Dar page: its interaction API gets its own copy of the browser binding.
    if let Some(uid) = path.strip_prefix("/identity/interaction/") {
        let mut page = client(runtime, &path, &headers).await?;
        if runtime.identity.validate_transport(peer, &headers).is_ok() {
            let incoming = cookie_value(&headers, "dar_oidc_interaction");
            if let Some(cookie) = crate::oidc::page_cookie(&runtime.oidc, uid, incoming.as_deref()) {
                page.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(|_| AppError::internal())?);
            }
        }
        return Ok(page);
    }
    client(runtime, &path, &headers).await
}

/// One authenticated API request.
pub(crate) struct Api<'a> {
    pub runtime: &'a Runtime,
    pub method: &'a Method,
    pub path: &'a str,
    pub query: &'a HashMap<String, String>,
    pub headers: &'a HeaderMap,
    pub authenticated: &'a Value,
    pub secret: &'a str,
    pub peer: String,
    pub raw_peer: SocketAddr,
}

impl Api<'_> {
    pub fn session_id(&self) -> &str {
        self.authenticated["session"]["id"].as_str().unwrap_or_default()
    }
    pub fn user(&self) -> &Value {
        &self.authenticated["user"]
    }
    pub fn user_id(&self) -> &str {
        self.user()["id"].as_str().unwrap_or_default()
    }
    pub fn username(&self) -> &str {
        self.user()["username"].as_str().unwrap_or_default()
    }
    pub fn administrator(&self) -> bool {
        self.user()["administrator"] == json!(true)
    }
    pub fn q(&self, name: &str) -> String {
        self.query.get(name).cloned().unwrap_or_default()
    }
    pub fn q_opt(&self, name: &str) -> Option<String> {
        self.query.get(name).cloned()
    }
    pub async fn body(&self, body: Body, limit: usize) -> Result<Map<String, Value>> {
        body_object(body, limit, self.headers).await
    }
}

fn clear_cookie(runtime: &Runtime, value: Value) -> Response {
    let mut response = json_response(&value, StatusCode::OK);
    response.headers_mut().insert(header::SET_COOKIE, runtime.cookie("", 0));
    response
}

/// Optional apps the administrator can switch off; their APIs answer 404 then.
fn optional_app(path: &str) -> Option<&'static str> {
    ["notes", "tasks", "news", "mail", "chat", "photos", "books", "git"].into_iter().find(|app| {
        let base = format!("/api/v1/{app}");
        path == base || path.starts_with(&(base + "/"))
    })
}

async fn api(r: Api<'_>, body: Body) -> Result<Response> {
    let runtime = r.runtime;
    let db = &runtime.db;
    let (method, path) = (r.method, r.path);
    let session_id = r.session_id().to_owned();
    if path == "/api/v1/search" && *method == Method::GET {
        let term = r.q("q");
        if term.len() > 800 {
            return Err(AppError::invalid());
        }
        let mut found = db.read("service_global_search", json!([session_id, term])).await?;
        // Notes by `#tag` or alias come from the notes cache; Notes being off or unreachable leaves them out.
        if let Ok(hits) = runtime.notes.search(db, &runtime.blobs, &session_id, &term).await {
            crate::notes_library::merge_search(&mut found, hits);
        }
        return ok(found);
    }
    if path == "/api/v1/appSettings" && *method == Method::GET {
        return ok(crate::installed::settings(db).await?);
    }
    if path == "/api/v1/admin/installedApps" {
        return crate::installed::admin(&runtime.installed, db, method, r.headers, body, &session_id, r.administrator(), async |id, enabled| {
            if id == "agent" && !enabled {
                runtime.agent.cancel_all();
            }
            if id == "chat" {
                runtime.chat.set_enabled(enabled).await;
            }
            Ok(())
        })
        .await;
    }
    if path == "/api/v1/apps" || path.starts_with("/api/v1/apps/") {
        return crate::installed::handle(&runtime.installed, db, method, path, r.query, r.headers, body, &session_id).await;
    }
    if path == "/api/v1/files/picker" && *method == Method::GET {
        let kind = r.q_opt("kind").unwrap_or_else(|| "folder".into());
        return ok(db.read("service_file_picker", json!([session_id, r.q("parentId"), r.q("cursor"), kind])).await?);
    }
    if path == "/api/v1/agent/provider/connect" && *method == Method::POST {
        if !r.administrator() {
            return Err(AppError::new("forbidden", 403));
        }
        db.call("service_agent", json!([session_id, "projects", "{}"])).await?;
        let has_body = r.headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) > 0;
        let input = if has_body { r.body(body, 16_384).await? } else { Map::new() };
        if input.keys().any(|k| k != "provider") {
            return Err(AppError::invalid());
        }
        let provider = match input.get("provider") {
            None | Some(Value::Null) => "codex",
            Some(Value::String(s)) if s == "codex" || s == "claude" => s.as_str(),
            _ => return Err(AppError::invalid()),
        };
        return ok(runtime.agent.connect(r.user_id(), provider).await?);
    }
    if path == "/api/v1/agent" || path.starts_with("/api/v1/agent/") {
        return crate::agent::handle_http(runtime, method, path, r.query, r.headers, &session_id, r.user_id(), &runtime.agent, body).await;
    }
    if let Some(app) = optional_app(path)
        && crate::installed::app_disabled(db, app).await?
    {
        return Err(AppError::not_found());
    }
    if path == "/api/v1/chat" || path.starts_with("/api/v1/chat/") {
        return crate::chat::handle_http(runtime, method, path, r.query, r.headers, &session_id, &runtime.chat, body).await;
    }
    if path == "/api/v1/mail" || path.starts_with("/api/v1/mail/") {
        return crate::mail::handle_http(runtime, method, path, r.query, r.headers, &session_id, &runtime.mail, body).await;
    }
    if path == "/api/v1/news" || path.starts_with("/api/v1/news/") {
        return crate::news::handle_http(runtime, method, path, r.query, r.headers, &session_id, r.username(), &runtime.news, body).await;
    }
    if path.starts_with("/api/v1/git/") {
        return crate::git_api::handle(&r, body).await;
    }
    if path == "/api/v1/books" || path.starts_with("/api/v1/books/") {
        return crate::books::handle_http(runtime, method, path, r.query, r.headers, &session_id, &runtime.books, body).await;
    }
    if path == "/api/v1/photos" || path.starts_with("/api/v1/photos/") {
        return crate::photos::handle_http(runtime, method, path, r.query, r.headers, &session_id, r.user_id(), &runtime.photos, &runtime.immich, &runtime.root, body).await;
    }
    // The Locked folder opens for this browser session after the account password is confirmed.
    match (method.clone(), path) {
        (Method::GET, "/api/v1/files/locked") => return ok(db.read("service_locked_folder", json!([session_id, "state", ""])).await?),
        (Method::POST, "/api/v1/files/locked/lock") => return ok(db.call("service_locked_folder", json!([session_id, "lock", ""])).await?),
        (Method::POST, "/api/v1/files/locked/unlock") => {
            let key = format!("unlock:{}", r.user_id());
            runtime.attempt(&key)?;
            let p = r.body(body, 16_384).await?;
            if p.keys().any(|k| k != "password") {
                return Err(AppError::invalid());
            }
            let verifier = runtime.confirm_password(r.username(), &string(p.get("password"), 1024)?).await?;
            runtime.succeeded(&key);
            return ok(db.call("service_locked_folder", json!([session_id, "unlock", verifier])).await?);
        }
        _ => {}
    }
    if path == "/api/v1/notes/library" && *method == Method::GET {
        let term: String = r.q("q").chars().take(200).collect();
        return ok(runtime.notes.library(db, &runtime.blobs, &session_id, &term).await?);
    }
    if path == "/api/v1/notes/attachments" && *method == Method::GET {
        return ok(db.call("service_notes", json!([session_id, "attachments"])).await?);
    }
    if path == "/api/v1/notes/graph" && *method == Method::GET {
        return ok(runtime.notes.graph(db, &runtime.blobs, &session_id).await?);
    }
    if path == "/api/v1/notes/cards" && *method == Method::GET {
        let today = r.q("today");
        let today = if today.is_empty() {
            None
        } else {
            Some(dar_notes::cards::Date::parse(&today).ok_or_else(AppError::invalid)?)
        };
        return ok(runtime.notes.cards(db, &runtime.blobs, &session_id, today).await?);
    }
    if (path == "/api/v1/notes/backlinks" || path == "/api/v1/notes/mentions") && *method == Method::GET {
        let id = r.q("id");
        if id.is_empty() || id.len() > 64 {
            return Err(AppError::invalid());
        }
        let notes = &runtime.notes;
        return ok(if path.ends_with("backlinks") { notes.backlinks(db, &runtime.blobs, &session_id, &id).await? } else { notes.mentions(db, &runtime.blobs, &session_id, &id).await? });
    }
    if ["notes", "tasks"].iter().any(|a| path == format!("/api/v1/{a}") || path.starts_with(&format!("/api/v1/{a}/"))) {
        return crate::pim::workspace::handle(db, &session_id, method, path, r.query, r.headers, body).await;
    }
    if path == "/api/v1/account/profile" {
        let profile = db.call("service_profile", json!([session_id, "read", "{}"])).await?;
        let etag = format!("\"{}:{}\"", profile["id"].as_str().unwrap_or_default(), profile["revision"]);
        let tagged = |value: &Value| {
            let mut response = json_ok(value);
            let tag = format!("\"{}:{}\"", value["id"].as_str().unwrap_or_default(), value["revision"]);
            response.headers_mut().insert(header::ETAG, HeaderValue::from_str(&tag).expect("etag"));
            response
        };
        if *method == Method::GET {
            return Ok(tagged(&profile));
        }
        if *method == Method::PUT {
            let supplied = r.headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()).ok_or(AppError::new("preconditionRequired", 428))?;
            if supplied != etag {
                return Err(AppError::new("revisionConflict", 412));
            }
            let p = r.body(body, 16_384).await?;
            if p.keys().any(|k| !["displayName", "givenName", "familyName", "email"].contains(&k.as_str())) {
                return Err(AppError::with("invalidInput", 400, "Only personal profile fields can be changed here."));
            }
            let text = |key: &str| p.get(key).cloned().filter(|v| !v.is_null()).unwrap_or(json!(""));
            let input = json!({ "revision": profile["revision"], "displayName": p.get("displayName").cloned().unwrap_or(Value::Null), "givenName": text("givenName"), "familyName": text("familyName"), "email": text("email") });
            let result = db.call("service_profile", json!([session_id, "save", input.to_string()])).await?;
            return Ok(tagged(&result));
        }
        return Err(AppError::new("methodNotAllowed", 405));
    }
    if path.starts_with("/api/v1/identity/") && !path.starts_with("/api/v1/identity/interactions/") {
        return crate::identity_http::handle(&r, body).await;
    }
    if let Some(uid) = path.strip_prefix("/api/v1/identity/interactions/") {
        runtime.identity.validate_transport(r.raw_peer, r.headers)?;
        let cookie = cookie_value(r.headers, "dar_oidc_interaction");
        let body = if *method == Method::POST {
            Some(r.body(body, 256 * 1024).await?)
        } else if *method == Method::GET {
            None
        } else {
            return Err(AppError::not_found());
        };
        return ok(crate::oidc::interaction(&runtime.oidc, runtime, uid, cookie.as_deref(), r.authenticated, body).await?);
    }
    if path == "/api/v1/pim/preferences" {
        return preferences(runtime, method, r.headers, body, &session_id).await;
    }
    if ["contacts", "calendar", "pim"].iter().any(|a| path == format!("/api/v1/{a}") || path.starts_with(&format!("/api/v1/{a}/"))) {
        return crate::pim::handle(db, &session_id, method, path, r.query, r.headers, body).await;
    }
    match (method.clone(), path) {
        (Method::GET, "/api/v1/auth/session") => return ok(json!({ "user": r.user(), "csrfToken": runtime.issuer.csrf(r.secret) })),
        (Method::PUT, "/api/v1/appearance") => {
            if !r.administrator() {
                return Err(AppError::new("forbidden", 403));
            }
            let p = r.body(body, 360_000).await?;
            let logo = string(p.get("logo"), 350_000)?;
            let media_type = string(p.get("mediaType"), 20)?;
            if !logo.is_empty() && !valid_logo(&logo, &media_type) {
                return Err(AppError::with("invalidInput", 400, "Choose a PNG or WebP image up to 256 KiB."));
            }
            let revision = integer(p.get("revision"), u32::MAX as f64)?;
            if r.headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) != Some(format!("\"appearance:{revision}\"").as_str()) {
                return Err(AppError::new("preconditionRequired", 428));
            }
            let value = json!({ "logo": logo, "mediaType": if logo.is_empty() { "" } else { media_type.as_str() } }).to_string();
            let row = db.call("service_appearance", json!([session_id, revision as u32, value])).await?;
            let url = (!logo.is_empty()).then(|| format!("/api/v1/appearance/logo?revision={}", row["revision"]));
            return ok(json!({ "revision": row["revision"], "logoUrl": url }));
        }
        (Method::POST, "/api/v1/auth/token") => {
            let expires = r.authenticated["session"]["expiresAt"].as_f64().unwrap_or(0.0);
            let tab = r.q_opt("tab").unwrap_or_else(auth::uuid);
            let token = runtime.issuer.token(&session_id, expires, &tab)?;
            return ok(json!({ "token": token, "databaseUrl": runtime.config.database_public_url, "databaseName": runtime.config.database_name }));
        }
        (Method::POST, "/api/v1/auth/logout") => {
            db.call("service_revoke_session", json!([session_id])).await?;
            runtime.agent.cancel_session(&session_id);
            return Ok(clear_cookie(runtime, json!({ "ok": true })));
        }
        (Method::POST, "/api/v1/auth/password") => {
            let p = r.body(body, 16_384).await?;
            runtime.change_password(r.user_id(), r.username(), &string(p.get("currentPassword"), 1024)?, &string(p.get("password"), 1024)?).await?;
            return Ok(clear_cookie(runtime, json!({ "ok": true })));
        }
        (Method::GET, "/api/v1/auth/sessions") => return ok(json!({ "items": db.read("service_sessions", json!([session_id])).await? })),
        (Method::GET, "/api/v1/auth/app-passwords") => return ok(json!({ "items": db.read("service_app_passwords", json!([session_id])).await? })),
        (Method::POST, "/api/v1/auth/app-passwords") => {
            let p = r.body(body, 16_384).await?;
            let (secret, digest) = auth::opaque_token();
            let id = auth::uuid();
            let scope = string(p.get("scope"), 120)?;
            let grants = auth::password_grants(&scope);
            let apps: Vec<&str> = ["files", "contacts", "calendar", "news", "photos"].into_iter().filter(|app| grants.iter().any(|g| g.starts_with(&format!("{app}:")))).collect();
            let kind = match apps.first() {
                Some(&"contacts") => "addressbooks",
                Some(&"calendar") => "calendars",
                _ => "files",
            };
            db.call("service_create_app_password", json!([session_id, id, digest, string(p.get("label"), 100)?, scope])).await?;
            let origin = &runtime.config.origin;
            let username = encode(r.username());
            let mut value = json!({ "id": id, "secret": secret, "davUrl": if apps.len() > 1 { format!("{origin}/dav/") } else { format!("{origin}/dav/{kind}/{username}/") } });
            if apps.contains(&"files") {
                value["filesDavUrl"] = json!(format!("{origin}/dav/files/{username}/"));
            }
            if apps.contains(&"news") {
                value["newsUrl"] = json!(format!("{origin}/dav/news"));
            }
            if apps.contains(&"photos") {
                value["immichUrl"] = json!(origin);
            }
            return Ok(json_response(&value, StatusCode::CREATED));
        }
        (Method::POST, "/api/v1/auth/users") => {
            if !r.administrator() {
                return Err(AppError::new("forbidden", 403));
            }
            let p = r.body(body, 16_384).await?;
            let id = runtime.create_account(&string(p.get("username"), 64)?, &string(p.get("displayName"), 100)?, &string(p.get("password"), 1024)?, false).await?;
            return Ok(json_response(&json!({ "id": id }), StatusCode::CREATED));
        }
        (Method::POST, "/api/v1/files/uploads") => {
            let p = r.body(body, 16_384).await?;
            let present = |key: &str| p.get(key).filter(|v| !v.is_null() && *v != &json!("") && *v != &json!(0) && *v != &json!(false));
            let file_id = match present("fileId") {
                Some(v) => string(Some(v), 100)?,
                None => auth::uuid(),
            };
            let parent = match p.get("parentId").filter(|v| !v.is_null()) {
                Some(v) => string(Some(v), 100)?,
                None => String::new(),
            };
            let media = match p.get("mediaType").filter(|v| !v.is_null()) {
                Some(v) => string(Some(v), 150)?.to_lowercase(),
                None => "application/octet-stream".into(),
            };
            let expected = match p.get("expectedRevision").filter(|v| !v.is_null()) {
                Some(v) => integer(Some(v), u32::MAX as f64)?,
                None => 0.0,
            };
            let args = json!([session_id, auth::uuid(), file_id, parent, string(p.get("name"), 1024)?, integer(p.get("size"), MAX_FILE_BYTES)?, media, expected as u32]);
            let upload = db.call("service_begin_upload", args).await?;
            return Ok(json_response(&upload, StatusCode::CREATED));
        }
        _ => {}
    }
    if let Some(id) = path.strip_prefix("/api/v1/auth/sessions/").filter(|id| !id.is_empty() && !id.contains('/'))
        && *method == Method::DELETE
    {
        db.call("service_revoke_owned_session", json!([session_id, id])).await?;
        return ok(json!({ "ok": true }));
    }
    if let Some(id) = path.strip_prefix("/api/v1/auth/app-passwords/").filter(|id| !id.is_empty() && !id.contains('/'))
        && *method == Method::DELETE
    {
        db.call("service_revoke_app_password", json!([session_id, id])).await?;
        return ok(json!({ "ok": true }));
    }
    if path.starts_with("/api/v1/files/history/") {
        return crate::history::handle(&r, body).await;
    }
    if let Some(id) = path.strip_prefix("/api/v1/files/uploads/").filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase() || b == b'-')) {
        if *method == Method::DELETE {
            db.call("service_cancel_upload", json!([session_id, id])).await?;
            return ok(json!({ "ok": true }));
        }
        if *method == Method::PUT {
            let upload = db.call("service_upload", json!([session_id, id])).await?;
            if upload["state"] == "complete" {
                return ok(json!({ "id": upload["fileId"] }));
            }
            if upload["state"] != "uploading" {
                return Err(AppError::new("conflict", 409));
            }
            let size = upload["size"].as_f64().unwrap_or(0.0) as u64;
            let digest = runtime.blobs.receive_named(body.into_data_stream(), size, Some(id)).await?;
            db.call("service_finish_upload", json!([session_id, id, digest, 0])).await?;
            return ok(json!({ "id": upload["fileId"] }));
        }
    }
    let file_route = path.strip_prefix("/api/v1/files/").and_then(|rest| rest.split_once('/')).filter(|(id, what)| !id.is_empty() && !what.contains('/'));
    if let Some((id, what)) = file_route
        && (what == "content" || what == "thumbnail")
        && (*method == Method::GET || *method == Method::HEAD)
    {
        let version = if what == "content" { r.q("version") } else { String::new() };
        let entry: BlobFile = serde_json::from_value(db.read("service_download", json!([session_id, id, version])).await?).map_err(|_| AppError::not_found())?;
        if what == "thumbnail" && entry.media_type.starts_with("image/") {
            let size = if r.q("size") == "m" { "m" } else { "s" };
            if let Some(file) = runtime.photos.thumbnail_file(&entry.blob_id, "image", size).await {
                return thumbnail_response(r.headers, &file, &format!("\"{}-{size}\"", entry.blob_id), entry.locked).await;
            }
        }
        // Without media tools the original stands in for a thumbnail, as in the Node runtime.
        let preview = what == "thumbnail" || r.q("preview") == "1";
        return runtime.blobs.download(method, r.headers, &entry, preview).await;
    }
    Err(AppError::not_found())
}

/// A PNG or WebP logo of at most 256 KiB, in canonical base64.
fn valid_logo(logo: &str, media_type: &str) -> bool {
    use base64::Engine;
    let engine = base64::engine::general_purpose::STANDARD;
    let Ok(bytes) = engine.decode(logo) else { return false };
    if bytes.len() > 256 * 1024 || engine.encode(&bytes) != logo {
        return false;
    }
    media_type == "image/png" && bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]) || media_type == "image/webp" && bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

async fn preferences(runtime: &Runtime, method: &Method, headers: &HeaderMap, body: Body, session_id: &str) -> Result<Response> {
    let old = runtime.db.call("service_pim", json!([session_id, "preferences", "{}"])).await?["value"].take();
    let revision = old["revision"].as_u64().unwrap_or(0);
    let saved: Map<String, Value> = serde_json::from_str(old["value"].as_str().unwrap_or("{}")).unwrap_or_default();
    let etag = format!("\"preferences:{revision}\"");
    let respond = |value: Map<String, Value>, revision: u64| {
        let mut value = value;
        value.insert("revision".into(), json!(revision));
        let mut response = json_response(&Value::Object(value), StatusCode::OK);
        response.headers_mut().insert(header::ETAG, HeaderValue::from_str(&format!("\"preferences:{revision}\"")).expect("etag"));
        response
    };
    match *method {
        Method::GET => Ok(respond(saved, revision)),
        Method::PUT => {
            let supplied = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()).ok_or(AppError::new("preconditionRequired", 428))?;
            if supplied != etag {
                return Err(AppError::new("revisionConflict", 412));
            }
            let p = body_object(body, 3 * 1024 * 1024, headers).await?;
            let merged = crate::preferences::merge(&saved, &p)?;
            let mut stored = saved.clone();
            stored.extend(merged);
            let payload = json!({ "revision": revision, "value": Value::Object(stored.clone()).to_string() });
            let result = runtime.db.call("service_pim", json!([session_id, "savePreferences", payload.to_string()])).await?;
            Ok(respond(stored, result["value"]["revision"].as_u64().unwrap_or(revision + 1)))
        }
        _ => Err(AppError::not_found()),
    }
}

/// The client's static files under `/rust/`, `/chunks/`, `/sdk.*`, `/app.css`;
/// every other page is its `index.html`.
/// Whether a path under `/rust/` names its contents, so it can be cached for good.
fn fingerprinted(path: &str) -> bool {
    // The build (xtask/src/client.rs) names the app and PDF worker folders after their contents.
    if let Some((folder, _)) = path.split_once('/') {
        if let Some(hash) = folder.strip_prefix("app-").or_else(|| folder.strip_prefix("pdf-worker-")) {
            return (8..=64).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit());
        }
    }
    let file = path.rsplit('/').next().unwrap_or(path);
    let mut parts = file.rsplit('.');
    let Some(ext) = parts.next() else { return false };
    if !matches!(ext, "wasm" | "js" | "mjs" | "css") {
        return false;
    }
    let Some(hash) = parts.next() else { return false };
    (8..=64).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit()) && parts.next().is_some()
}

async fn client(runtime: &Runtime, path: &str, headers: &HeaderMap) -> Result<Response> {
    const IMMUTABLE: &str = "public, max-age=31536000, immutable";
    let dist = &runtime.config.client_dist;
    let rel = if let Some(rest) = path.strip_prefix("/rust/") {
        let cache = if fingerprinted(rest) { IMMUTABLE } else { "no-cache" };
        Some((rest, cache))
    } else if path.starts_with("/chunks/") {
        Some((path.trim_start_matches('/'), IMMUTABLE))
    } else if path.starts_with("/sdk-") && (path.ends_with(".js") || path.ends_with(".css")) {
        Some((path.trim_start_matches('/'), IMMUTABLE))
    } else if matches!(path, "/sdk.js" | "/sdk.css" | "/app.css") {
        Some((path.trim_start_matches('/'), "no-cache"))
    } else {
        None
    };
    if let Some((rel_path, cache)) = rel {
        if rel_path.is_empty()
            || rel_path.starts_with('/')
            || rel_path.contains('\\')
            || rel_path.split('/').any(|s| s.is_empty() || s == "." || s == "..")
        {
            return Err(AppError::not_found());
        }
        // As static.ts: a precompressed copy (`.br`, then `.gz`) when the browser accepts it.
        let offered = accepted_encodings(headers);
        let mut encoding = None;
        let mut bytes = None;
        for (name, suffix) in [("br", "br"), ("gzip", "gz")] {
            if offered.contains(name)
                && let Ok(found) = tokio::fs::read(dist.join(format!("{rel_path}.{suffix}"))).await
            {
                encoding = Some(name);
                bytes = Some(found);
                break;
            }
        }
        let bytes = match bytes {
            Some(b) => b,
            None => tokio::fs::read(dist.join(rel_path)).await.map_err(|_| AppError::not_found())?,
        };
        // A validator from the file's size and time, so a revisit costs a 304 instead of the file.
        let modified = tokio::fs::metadata(dist.join(rel_path)).await.ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        let etag = format!("\"{:x}-{modified:x}-{}\"", bytes.len(), encoding.unwrap_or("identity"));
        if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).is_some_and(|v| v.split(',').any(|t| t.trim() == etag)) {
            let mut response = StatusCode::NOT_MODIFIED.into_response();
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
            response.headers_mut().insert(header::VARY, HeaderValue::from_static("accept-encoding"));
            if let Ok(value) = HeaderValue::from_str(&etag) {
                response.headers_mut().insert(header::ETAG, value);
            }
            return Ok(response);
        }
        let kind = match rel_path.rsplit('.').next() {
            Some("js" | "mjs") => "text/javascript; charset=utf-8",
            Some("wasm") => "application/wasm",
            Some("css") => "text/css; charset=utf-8",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            Some("woff2") => "font/woff2",
            Some("ttf") => "font/ttf",
            _ => "application/octet-stream",
        };
        let mut response = ([(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, cache)], Bytes::from(bytes)).into_response();
        if let Ok(value) = HeaderValue::from_str(&etag) {
            response.headers_mut().insert(header::ETAG, value);
        }
        response.headers_mut().insert(header::VARY, HeaderValue::from_static("accept-encoding"));
        if let Some(name) = encoding {
            response.headers_mut().insert(header::CONTENT_ENCODING, HeaderValue::from_static(name));
        }
        return Ok(response);
    }
    let page = tokio::fs::read(dist.join("index.html")).await.map_err(|_| AppError::not_found())?;
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], Bytes::from(page)).into_response())
}

/// `localeCompare` for names: case-insensitive first, then exact.
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b))
}

fn url_path(url: &str) -> String {
    url.split_once("://").map(|(_, rest)| rest.find('/').map(|i| rest[i..].to_owned()).unwrap_or_default()).unwrap_or_default()
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn url_params(query: &str) -> HashMap<String, String> {
    query.split('&').filter(|p| !p.is_empty()).map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (decode(k), decode(v))
    }).collect()
}

fn encode(value: &str) -> String {
    value.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// A cached thumbnail. Its address carries the revision, so browsers may keep it a day, as in
/// Photos; Locked folder files are never kept.
async fn thumbnail_response(headers: &HeaderMap, file: &std::path::Path, etag: &str, locked: bool) -> Result<Response> {
    let cache = if locked { "private, no-store" } else { "private, max-age=86400" };
    if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag) {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag.to_owned()), (header::CACHE_CONTROL, cache.to_owned())]).into_response());
    }
    let bytes = tokio::fs::read(file).await.map_err(|_| AppError::new("contentUnavailable", 503))?;
    Ok(([(header::CONTENT_TYPE, "image/webp".to_owned()), (header::ETAG, etag.to_owned()), (header::CACHE_CONTROL, cache.to_owned())], Bytes::from(bytes)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_files_named_after_their_contents_are_immutable() {
        assert!(fingerprinted("dar_client.0123456789ab.js"));
        assert!(fingerprinted("app.0123456789ab.css"));
        assert!(fingerprinted("app-0123456789ab/split_agent_app_1.wasm"));
        assert!(fingerprinted("app-0123456789ab/snippets/dar-client-1/inline0.js"));
        assert!(fingerprinted("pdf-worker-0123456789ab/worker.js"));
        assert!(fingerprinted("pdf-worker-0123456789ab/dar_pdf_worker_bg.wasm"));
        assert!(!fingerprinted("app.css"));
        assert!(!fingerprinted("mic-worklet.js"));
        assert!(!fingerprinted("app-latest/dar_client.js"));
        assert!(!fingerprinted("pdf-worker-latest/worker.js"));
        assert!(!fingerprinted("snippets/dar-client-86ccece7599fa862/inline0.js"));
    }

    #[test]
    fn query_strings_decode() {
        let params = url_params("q=a%20b+c&tab=1&empty=&bad=%zz");
        assert_eq!(params["q"], "a b c");
        assert_eq!(params["empty"], "");
        assert_eq!(params["bad"], "%zz");
        assert_eq!(url_path("http://127.0.0.1:4311/auth/issuer"), "/auth/issuer");
    }
}

/// Content encodings the browser accepts with q > 0 (`offeredEncodings` in static.ts).
fn accepted_encodings(headers: &HeaderMap) -> std::collections::HashSet<String> {
    let value = headers.get(header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok()).unwrap_or_default().to_ascii_lowercase();
    value
        .split(',')
        .filter_map(|part| {
            let mut pieces = part.trim().split(';');
            let token = pieces.next()?.trim();
            let q = pieces.find_map(|p| p.trim().strip_prefix("q=").and_then(|q| q.trim().parse::<f64>().ok()));
            (!token.is_empty() && q.is_none_or(|q| q > 0.0)).then(|| token.to_owned())
        })
        .collect()
}
