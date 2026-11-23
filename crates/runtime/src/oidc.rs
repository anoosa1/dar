//! The `dar-oidc` provider at `/oidc`, on Dar's accounts and browser sessions, and the
//! sign-in page's interaction API (`/api/v1/identity/interactions/:uid`).
//!
//! As in the Node runtime (`identity/provider.ts`, `store.ts`, `keys.ts`, `config.ts`): provider
//! records live in the database, sealed (`oidc_store.rs`); clients are administered in the
//! database (metadata public, the full registration sealed) and loaded when a request names
//! them; `oidc.json` registers host-configured clients at startup; the signing keyring is
//! `secrets/oidc-keyring.json`, sealed with the identity keyring.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine;
use dar_oidc::{Account, ClientInput, Decision, Directory, Keyring, NativeSession, Provider, ProviderConfig};
use serde_json::{Value, json};

use crate::auth::KeyBox;
use crate::db::Database;
use crate::error::{AppError, Result};

struct DarDirectory(Database);

impl DarDirectory {
    fn lookup(&self, operation: &str, id: &str) -> Option<Value> {
        let payload = json!({ "id": id }).to_string();
        let value = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.0.read("service_provider", json!([operation, payload])))).ok()?;
        (!value.is_null()).then_some(value)
    }
}

impl Directory for DarDirectory {
    fn account(&self, id: &str) -> Option<Account> {
        // The account as SCIM sees it (identity.rs `profile`); disabled accounts sign in nowhere.
        let a = self.lookup("account", id)?;
        if a["active"] != json!(true) {
            return None;
        }
        let text = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        let emails = a["emails"].as_array().cloned().unwrap_or_default();
        let email = emails.iter().find(|e| e["primary"] == json!(true)).or(emails.first()).and_then(|e| text(&e["value"]));
        let groups = a["groups"].as_array().map(|g| g.iter().filter_map(|g| text(&g["value"])).collect()).unwrap_or_default();
        Some(Account { id: text(&a["id"])?, user_name: text(&a["userName"])?, display_name: text(&a["displayName"]).unwrap_or_default(), given_name: text(&a["name"]["givenName"]), family_name: text(&a["name"]["familyName"]), email, groups })
    }

    fn native_session(&self, id: &str) -> Option<NativeSession> {
        let s = self.lookup("session", id)?;
        Some(NativeSession { id: s["id"].as_str()?.into(), account_id: s["userId"].as_str()?.into(), created_at: s["createdAt"].as_f64()? as i64 })
    }
}

/// The provider with what administration needs: sealed boxes and the keyring file.
pub struct Oidc {
    pub provider: Provider,
    pub db: Database,
    pub clients: KeyBox,
    keys: KeyBox,
    keyring_file: PathBuf,
    pub development: bool,
    pub origin: String,
}

fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

impl Oidc {
    pub fn open(origin: &str, development: bool, secrets: &Path, db: Database) -> std::io::Result<Arc<Oidc>> {
        let keys = KeyBox::open(secrets, "provider-keyring")?;
        let keyring_file = secrets.join("oidc-keyring.json");
        let keyring = match std::fs::symlink_metadata(&keyring_file) {
            Ok(meta) => {
                use std::os::unix::fs::PermissionsExt;
                if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
                    return Err(std::io::Error::other("Unsafe provider keyring."));
                }
                let sealed = std::fs::read_to_string(&keyring_file)?;
                let state = keys.decrypt(&sealed).ok_or_else(|| std::io::Error::other("The provider keyring cannot be decrypted."))?;
                Keyring::from_jwk_state(&state).map_err(|e| std::io::Error::other(e.to_string()))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let keyring = Keyring::generate().map_err(|e| std::io::Error::other(e.to_string()))?;
                let state = keyring.to_jwk_state().map_err(|e| std::io::Error::other(e.to_string()))?;
                crate::auth::create_durable_file(&keyring_file, &keys.encrypt(&state))?;
                keyring
            }
            Err(e) => return Err(e),
        };
        let config = ProviderConfig::new(origin, development).map_err(|e| std::io::Error::other(e.to_string()))?;
        let store = crate::oidc_store::DbStore::new(db.clone(), KeyBox::open(secrets, "provider-model")?);
        let provider = Provider::new(config, keyring, Box::new(store), Arc::new(DarDirectory(db.clone())));
        Ok(Arc::new(Oidc { provider, db, clients: KeyBox::open(secrets, "provider-client")?, keys, keyring_file, development, origin: origin.to_owned() }))
    }

    fn save_keys(&self, state: dar_oidc::KeyState) -> Result<()> {
        let keyring = Keyring::from_state(state).map_err(|_| AppError::internal())?;
        let sealed = self.keys.encrypt(&keyring.to_jwk_state().map_err(|_| AppError::internal())?);
        crate::root::replace_durable_file(&self.keyring_file, sealed.as_bytes()).map_err(|_| AppError::internal())
    }

    fn status_of(state: &dar_oidc::KeyState) -> Value {
        json!({ "activeKid": state.active.kid, "rotatedAt": iso(state.rotated_at), "retained": state.retired.iter().map(|k| json!({ "kid": k.kid, "retireAt": iso(k.retire_at) })).collect::<Vec<_>>() })
    }

    pub fn status(&self) -> Value {
        Self::status_of(&self.provider.key_state())
    }

    /// A new signing key; the old one stays published briefly.
    pub fn rotate(&self) -> Result<Value> {
        let state = self.provider.rotate_keys().map_err(|_| AppError::internal())?;
        self.save_keys(state.clone())?;
        Ok(Self::status_of(&state))
    }

    /// Drops retired keys past their time.
    pub fn maintain(&self) {
        if let Some(state) = self.provider.maintain() {
            let _ = self.save_keys(state);
        }
    }

    /// The administered client `id` as the database has it now, into the provider.
    pub async fn load_client(&self, id: &str) {
        if !dar_oidc::config::valid_client_id(id) {
            return;
        }
        let Ok(row) = self.db.read("service_provider", json!(["client", json!({ "id": id }).to_string()])).await else { return };
        let client = (!row.is_null()).then(|| self.client_of(&row)).flatten();
        self.provider.put_client(id, client);
    }

    fn client_of(&self, row: &Value) -> Option<dar_oidc::Client> {
        let m = self.clients.decrypt(row["encrypted"].as_str()?)?;
        let list = |k: &str| m[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        let public = m["token_endpoint_auth_method"] == "none";
        let input = ClientInput {
            id: row["id"].as_str()?.to_owned(),
            name: m["client_name"].as_str()?.to_owned(),
            kind: if public { "public".into() } else { "confidential".into() },
            secret: if public { None } else { m["client_secret"].as_str().map(str::to_owned) },
            redirect_uris: list("redirect_uris"),
            post_logout_redirect_uris: list("post_logout_redirect_uris"),
            scopes: vec![m["scope"].as_str().unwrap_or_default().to_owned()],
            require_pkce: (m["require_pkce"] == json!(false)).then_some(false),
            enabled: Some(row["enabled"] != json!(false)),
        };
        dar_oidc::Client::from_input(&input, self.development).ok()
    }

    /// Host-configured clients from `oidc.json` (`configureOidc`): the whole file is checked
    /// before any revisioned client update.
    pub async fn configure(&self, filename: &Path, optional: bool) -> std::result::Result<(), String> {
        let source = match std::fs::read_to_string(filename) {
            Ok(s) => s,
            Err(e) if optional && e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        let input: Value = serde_json::from_str(&source).map_err(|_| "Invalid OIDC configuration JSON.".to_owned())?;
        let Some(list) = input.get("clients").and_then(Value::as_array).filter(|l| l.len() <= 200) else { return Err("OIDC configuration requires a clients array (up to 200 clients).".into()) };
        let mut ids: Vec<String> = Vec::new();
        let mut clients = Vec::new();
        for p in list {
            let Some(p) = p.as_object() else { return Err("Invalid OIDC client configuration.".into()) };
            let id = p.get("id").and_then(Value::as_str).filter(|id| dar_oidc::config::valid_client_id(id) && !ids.iter().any(|i| i == id)).ok_or("OIDC client IDs must be unique URL-safe identifiers.")?.to_owned();
            if id == "immich-app" {
                return Err(format!("The OIDC client ID {id} is reserved for the Immich app."));
            }
            ids.push(id.clone());
            if p.get("enabled").is_some_and(|e| !e.is_boolean()) {
                return Err("Invalid OIDC client enabled setting.".into());
            }
            if p.keys().any(|k| !["id", "name", "type", "redirectUris", "postLogoutRedirectUris", "scopes", "enabled", "secretFile", "requirePkce"].contains(&k.as_str())) {
                return Err("Unknown OIDC client setting; use secretFile for secrets.".into());
            }
            let secret = if p.get("type") == Some(&json!("confidential")) {
                let file = p.get("secretFile").and_then(Value::as_str).filter(|f| !f.is_empty()).ok_or("Confidential OIDC clients require secretFile.")?;
                let path = filename.parent().unwrap_or(Path::new(".")).join(file);
                let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                let secret = raw.strip_suffix("\r\n").or_else(|| raw.strip_suffix('\n')).unwrap_or(&raw).to_owned();
                if secret.len() < 32 || secret.contains(['\r', '\n']) {
                    return Err("OIDC client secrets must have at least 32 characters and no embedded newlines.".into());
                }
                Some(secret)
            } else {
                if p.contains_key("secretFile") {
                    return Err("Public OIDC clients cannot have secretFile.".into());
                }
                None
            };
            let metadata = client_metadata(p, &id, self.development, secret.as_deref()).map_err(|e| e.message)?;
            clients.push((id, metadata, p.get("enabled") != Some(&json!(false))));
        }
        for (id, metadata, enabled) in clients {
            let old = self.db.call("service_provider", json!(["client", json!({ "id": id }).to_string()])).await.map_err(|e| e.to_string())?;
            if !old.is_null() && old["enabled"] == json!(enabled) && old["encrypted"].as_str().and_then(|e| self.clients.decrypt(e)) == Some(metadata.clone()) {
                continue;
            }
            let mut public = metadata.clone();
            public.as_object_mut().expect("metadata").remove("client_secret");
            let payload = json!({ "id": id, "revision": old["revision"].as_u64().unwrap_or(0), "enabled": enabled, "metadata": public.to_string(), "encrypted": self.clients.encrypt(&metadata) });
            self.db.call("service_provider", json!(["saveClient", payload.to_string()])).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn invalid_redirects() -> AppError {
    AppError::with("invalidRedirectUris", 400, "Use exact HTTPS redirect URLs; loopback HTTP is allowed only in development.")
}

fn redirects(value: Option<&Value>, development: bool, optional: bool) -> Result<Vec<String>> {
    let list = match value {
        None if optional => return Ok(Vec::new()),
        Some(Value::Array(a)) if a.len() <= if optional { 10 } else { 20 } && (optional || !a.is_empty()) => a,
        _ => return Err(AppError::new("invalidRedirectUris", 400)),
    };
    let mut out: Vec<String> = Vec::new();
    for v in list {
        let raw = v.as_str().filter(|s| s.len() <= 2048).ok_or_else(|| AppError::new("invalidInput", 400))?;
        let url = reqwest::Url::parse(raw).map_err(|_| AppError::new("invalidRedirectUris", 400))?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() || url.as_str() != raw || raw.contains('*') || !(url.scheme() == "https" || development && url.scheme() == "http" && loopback) {
            return Err(invalid_redirects());
        }
        if !out.iter().any(|o| o == raw) {
            out.push(raw.to_owned());
        }
    }
    Ok(out)
}

/// The registration a client is stored as (`clientMetadata`).
pub fn client_metadata(p: &serde_json::Map<String, Value>, id: &str, development: bool, secret: Option<&str>) -> Result<Value> {
    let name = p.get("name").and_then(Value::as_str).filter(|n| n.len() <= 100).ok_or_else(|| AppError::new("invalidInput", 400))?.trim().to_owned();
    let kind = p.get("type").and_then(Value::as_str).unwrap_or_default();
    if name.is_empty() || !["public", "confidential"].contains(&kind) {
        return Err(AppError::new("invalidClient", 400));
    }
    let require_pkce = p.get("requirePkce");
    if require_pkce.is_some_and(|v| !v.is_boolean()) || kind == "public" && require_pkce == Some(&json!(false)) {
        return Err(AppError::with("invalidClient", 400, "PKCE can only be disabled for confidential clients."));
    }
    let scopes: Vec<String> = match p.get("scopes") {
        Some(Value::String(s)) => s.split_whitespace().map(str::to_owned).collect(),
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().map(str::to_owned).ok_or_else(|| AppError::new("invalidScopes", 400))).collect::<Result<_>>()?,
        _ => return Err(AppError::new("invalidScopes", 400)),
    };
    if !scopes.iter().any(|s| s == "openid") || scopes.iter().any(|s| !["openid", "profile", "email", "groups", "offline_access"].contains(&s.as_str())) {
        return Err(AppError::new("invalidScopes", 400));
    }
    let mut unique: Vec<String> = Vec::new();
    for s in scopes {
        if !unique.contains(&s) {
            unique.push(s);
        }
    }
    let mut grants = vec!["authorization_code"];
    if unique.iter().any(|s| s == "offline_access") {
        grants.push("refresh_token");
    }
    let mut m = json!({
        "client_id": id, "client_name": name, "application_type": "web",
        "redirect_uris": redirects(p.get("redirectUris"), development, false)?,
        "post_logout_redirect_uris": redirects(p.get("postLogoutRedirectUris"), development, true)?,
        "response_types": ["code"], "grant_types": grants, "scope": unique.join(" "),
        "token_endpoint_auth_method": if kind == "public" { "none" } else { "client_secret_basic" },
        "id_token_signed_response_alg": "RS256",
    });
    if require_pkce == Some(&json!(false)) {
        m["require_pkce"] = json!(false);
    }
    if kind == "confidential" {
        m["client_secret"] = json!(secret);
        m["client_secret_expires_at"] = json!(0);
    }
    Ok(m)
}

fn origin_host(origin: &str) -> String {
    match reqwest::Url::parse(origin) {
        Ok(u) => match (u.host_str(), u.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_owned(),
            _ => String::new(),
        },
        Err(_) => String::new(),
    }
}

impl Oidc {
    /// Checks the canonical `Host` header and, in production, the local HTTPS reverse proxy
    /// boundary (`validateTransport` in `runtime/identity/provider.ts`).
    pub fn validate_transport(&self, peer: std::net::SocketAddr, headers: &axum::http::HeaderMap) -> Result<()> {
        let expected = origin_host(&self.origin);
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
        if header("host") != Some(expected.as_str()) {
            return Err(AppError::new("invalidHost", 400));
        }
        if !self.development {
            let loopback = match peer.ip() {
                std::net::IpAddr::V4(v4) => v4.is_loopback(),
                std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
            };
            let xfh_ok = header("x-forwarded-host").is_none_or(|h| h == expected);
            if !loopback || header("x-forwarded-proto") != Some("https") || !xfh_ok {
                return Err(AppError::with("untrustedProxy", 400, "OIDC requires the configured local HTTPS reverse proxy."));
            }
        }
        Ok(())
    }
}

/// Before each provider request: validate the proxy/host boundary and URL length, load the
/// client it names fresh from the database, and attach the provider's security headers.
pub async fn load_named_client(State(oidc): State<Arc<Oidc>>, axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>, request: Request, next: Next) -> Response {
    use axum::http::{HeaderValue, header};
    use axum::response::IntoResponse;

    let mut response = if let Err(err) = oidc.validate_transport(peer, request.headers()) {
        err.into_response()
    } else if request.uri().to_string().len() + "/oidc".len() > 8192 {
        AppError::new("requestTooLarge", 413).into_response()
    } else {
        let (parts, body) = request.into_parts();
        let mut names: Vec<String> = Vec::new();
        fn query_of(names: &mut Vec<String>, query: &str) {
            for pair in query.split('&') {
                if let Some(v) = pair.strip_prefix("client_id=") {
                    names.push(crate::git::decode_component(&v.replace('+', " ")).unwrap_or_default());
                }
            }
        }
        if let Some(q) = parts.uri.query() {
            query_of(&mut names, q);
        }
        if let Some(basic) = parts.headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Basic "))
            && let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(basic.trim())
            && let Some((user, _)) = String::from_utf8_lossy(&decoded).split_once(':')
        {
            names.push(crate::git::decode_component(&user.replace('+', " ")).unwrap_or_default());
        }
        let form = parts.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"));
        let body = if form {
            match axum::body::to_bytes(body, 256 * 1024).await {
                Ok(bytes) => {
                    query_of(&mut names, &String::from_utf8_lossy(&bytes));
                    Body::from(bytes)
                }
                Err(_) => Body::empty(),
            }
        } else {
            body
        };
        names.sort();
        names.dedup();
        for name in names.iter().filter(|n| !n.is_empty()) {
            oidc.load_client(name).await;
        }
        next.run(Request::from_parts(parts, body)).await
    };
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    if !oidc.development {
        headers.insert("strict-transport-security", HeaderValue::from_static("max-age=31536000"));
    }
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"));
    }
    response
}

/// The `Set-Cookie` that gives the interaction API its own path-scoped copy of the verified binding.
pub fn page_cookie(provider: &Provider, uid: &str, incoming: Option<&str>) -> Option<String> {
    let valid = !uid.is_empty() && uid.len() <= 100 && uid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    (valid && provider.verify_interaction(uid, incoming)).then(|| provider.interaction_cookie(uid, &format!("/api/v1/identity/interactions/{uid}")))
}

fn failure(error: dar_oidc::Error) -> AppError {
    match error {
        dar_oidc::Error::InteractionExpired => AppError::with("interactionExpired", 410, "This sign-in request expired. Start again from the application."),
        other => AppError::with("invalidRequest", 400, &other.to_string()),
    }
}

/// `GET` shows the request; `POST {action: "deny"|"login"|"approve", password?}` decides it
/// and answers where the browser continues.
pub async fn interaction(provider: &Provider, runtime: &crate::http::Runtime, uid: &str, cookie: Option<&str>, authenticated: &Value, body: Option<serde_json::Map<String, Value>>) -> Result<Value> {
    let session_id = authenticated["session"]["id"].as_str().unwrap_or_default();
    let native = NativeSession {
        id: session_id.into(),
        account_id: authenticated["user"]["id"].as_str().unwrap_or_default().into(),
        created_at: authenticated["session"]["createdAt"].as_f64().unwrap_or(0.0) as i64,
    };
    let details = provider.interaction_details(uid, cookie, &native).map_err(failure)?;
    let Some(body) = body else {
        return serde_json::to_value(&details).map_err(|_| AppError::internal());
    };
    let decision = match body.get("action").and_then(Value::as_str) {
        Some("deny") => Decision::Deny,
        Some("approve") => Decision::Approve,
        Some("login") => {
            let mut confirmed = false;
            if details.requires_password {
                let password = body.get("password").and_then(Value::as_str).filter(|p| p.len() <= 1024).ok_or(AppError::new("invalidCredentials", 401))?;
                runtime.confirm_password(authenticated["user"]["username"].as_str().unwrap_or_default(), password).await?;
                confirmed = true;
            }
            Decision::Login { password_confirmed: confirmed }
        }
        _ => return Err(AppError::invalid()),
    };
    let redirect = provider.decide(uid, cookie, &native, decision).map_err(failure)?;
    Ok(json!({ "redirect": redirect }))
}
