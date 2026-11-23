//! Mail's network effects, owned by `MailRuntime`: the providers from the host configuration,
//! sealed account credentials (and OAuth refreshes), the background pass that keeps each
//! account's Files mirror and its server in step, and the actions people take in Mail.
//!
//! A pass first carries changes made in Files to the server (an `.eml` deleted, moved to another
//! mailbox's folder, edited or added; a folder added or renamed), then brings the server's state
//! into Files and the index (new messages written as `.eml` files, removed ones moved to deleted
//! files, flags). Message content is always read from the Files copy.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::imap::{Imap, encode_name};
use super::jmap::Jmap;
use super::message::{self, Attachment, Draft};
use super::net::{NetError, b64, http};
use crate::auth::KeyBox;
use crate::blobs::Blobs;
use crate::db::Database;
use crate::error::{AppError, Result};

const FETCH_BATCH: usize = 25;
const FETCH_BYTES: f64 = 24.0 * 1024.0 * 1024.0;

fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_millis() as f64
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkMailbox {
    pub id: String,
    pub remote: String,
    pub role: String,
    #[serde(default)]
    pub parent_id: String,
    #[serde(default)]
    pub validity: String,
    #[serde(default)]
    pub folder_id: String,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WorkProvider {
    pub id: String,
    pub source: String,
    pub enabled: bool,
    #[serde(default)]
    pub secret: String,
    pub document: Value,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Work {
    pub id: String,
    pub address: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub username: String,
    pub credential: String,
    pub provider: Option<WorkProvider>,
    #[serde(default)]
    pub mailboxes: Vec<WorkMailbox>,
    #[serde(default)]
    pub messages: Vec<WorkMessage>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WorkMessage {
    pub id: String,
    pub remote: String,
    pub mailbox_id: String,
}

/// An open connection to an account's server.
pub enum Server {
    Imap(Box<Imap>),
    Jmap(Box<Jmap>),
}

impl Server {
    async fn close(self) {
        if let Server::Imap(i) = self {
            i.logout().await;
        }
    }
}

struct OAuthState {
    session_id: String,
    provider_id: String,
    verifier: String,
    name: String,
    address: String,
    display_name: String,
    account_id: String,
    created: f64,
}

struct Inner {
    db: Database,
    blobs: Blobs,
    origin: String,
    credentials: KeyBox,
    provider_secrets: KeyBox,
    nix_secrets: Mutex<HashMap<String, String>>,
    oauth: Mutex<HashMap<String, OAuthState>>,
    /// One pass at a time per account.
    passes: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    closed: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    ticking: Mutex<bool>,
}

#[derive(Clone)]
pub struct MailRuntime {
    inner: Arc<Inner>,
}

fn app(e: NetError) -> AppError {
    let message = e.0.strip_prefix("signIn:").unwrap_or(&e.0).to_owned();
    AppError::with("mailServer", 502, &message)
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("randomness");
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn server_text(doc: &Value, key: &str) -> (String, u16, String) {
    (doc[key]["host"].as_str().unwrap_or_default().to_owned(), doc[key]["port"].as_u64().unwrap_or(0) as u16, doc[key]["security"].as_str().unwrap_or("tls").to_owned())
}

impl MailRuntime {
    pub fn open(db: Database, blobs: Blobs, secrets: &Path, origin: &str) -> std::io::Result<MailRuntime> {
        let (closed, _) = watch::channel(false);
        let runtime = MailRuntime {
            inner: Arc::new(Inner {
                db,
                blobs,
                origin: origin.to_owned(),
                credentials: KeyBox::open(secrets, "mail-credential")?,
                provider_secrets: KeyBox::open(secrets, "mail-provider")?,
                nix_secrets: Mutex::new(HashMap::new()),
                oauth: Mutex::new(HashMap::new()),
                passes: Mutex::new(HashMap::new()),
                closed,
                tasks: Mutex::new(Vec::new()),
                ticking: Mutex::new(false),
            }),
        };
        let ticker = runtime.clone();
        let handle = tokio::spawn(async move {
            let mut closed = ticker.inner.closed.subscribe();
            let mut wait = Duration::from_secs(3);
            loop {
                tokio::select! {
                    biased;
                    _ = closed.changed() => break,
                    _ = tokio::time::sleep(wait) => {}
                }
                if *closed.borrow() {
                    break;
                }
                ticker.poke();
                wait = Duration::from_secs(20);
            }
        });
        runtime.inner.tasks.lock().expect("tasks").push(handle);
        Ok(runtime)
    }

    /// Records the host's providers (`DAR_MAIL_PROVIDERS_FILE`): a JSON list of
    /// `{id, name, document, secretFile?}`. Secrets stay in memory, never in the database.
    pub async fn publish_providers(&self, file: Option<&Path>) -> std::result::Result<usize, String> {
        let list: Vec<Value> = match file {
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?
            }
            None => Vec::new(),
        };
        let mut secrets = HashMap::new();
        let mut published = Vec::new();
        for p in &list {
            let id = p["id"].as_str().ok_or("a mail provider has no id")?.to_owned();
            if let Some(file) = p["secretFile"].as_str().filter(|f| !f.is_empty()) {
                let secret = std::fs::read_to_string(file).map_err(|e| format!("the secret of mail provider {id} ({file}): {e}"))?;
                secrets.insert(id.clone(), secret.trim().to_owned());
            }
            published.push(json!({ "id": id, "name": p["name"], "document": p["document"] }));
        }
        *self.inner.nix_secrets.lock().expect("secrets") = secrets;
        self.inner.db.call("service_mail_worker", json!(["publishProviders", json!({ "providers": published }).to_string()])).await.map_err(|e| e.message)?;
        Ok(published.len())
    }

    fn closing(&self) -> bool {
        *self.inner.closed.borrow()
    }

    async fn worker(&self, operation: &str, value: Value) -> Result<Value> {
        self.inner.db.call("service_mail_worker", json!([operation, value.to_string()])).await
    }

    pub async fn user(&self, session_id: &str, operation: &str, value: Value) -> Result<Value> {
        let read = crate::mail::http::READ_OPERATIONS.contains(&operation);
        let args = json!([session_id, operation, value.to_string()]);
        if read { self.inner.db.read("service_mail", args).await } else { self.inner.db.call("service_mail", args).await }
    }

    pub fn seal_credential(&self, value: &Value) -> String {
        self.inner.credentials.encrypt(value)
    }

    pub fn seal_provider_secret(&self, secret: &str) -> String {
        self.inner.provider_secrets.encrypt(&json!(secret))
    }

    fn provider_secret(&self, p: &WorkProvider) -> String {
        if p.source == "nix" {
            return self.inner.nix_secrets.lock().expect("secrets").get(&p.id).cloned().unwrap_or_default();
        }
        if p.secret.is_empty() {
            return String::new();
        }
        self.inner.provider_secrets.decrypt(&p.secret).and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
    }

    // ---------- Signing in ----------

    /// A usable access token for an OAuth credential, refreshed when it is about to expire.
    async fn access_token(&self, account_id: Option<&str>, provider: &WorkProvider, credential: &mut Value) -> std::result::Result<String, NetError> {
        let expires = credential["expires"].as_f64().unwrap_or(0.0);
        if credential["access"].as_str().is_some_and(|a| !a.is_empty()) && expires > now_ms() + 60_000.0 {
            return Ok(credential["access"].as_str().unwrap_or_default().to_owned());
        }
        let refresh = credential["refresh"].as_str().unwrap_or_default().to_owned();
        if refresh.is_empty() {
            return Err(NetError("signIn:The sign-in has expired; sign in again".into()));
        }
        let tokens = self.token_request(provider, &[("grant_type", "refresh_token"), ("refresh_token", &refresh)]).await?;
        credential["access"] = tokens["access_token"].clone();
        credential["expires"] = json!(now_ms() + tokens["expires_in"].as_f64().unwrap_or(3600.0) * 1000.0);
        if let Some(r) = tokens["refresh_token"].as_str().filter(|r| !r.is_empty()) {
            credential["refresh"] = json!(r);
        }
        if let Some(id) = account_id {
            let _ = self.worker("credential", json!({ "accountId": id, "credential": self.seal_credential(credential) })).await;
        }
        Ok(credential["access"].as_str().unwrap_or_default().to_owned())
    }

    async fn token_request(&self, provider: &WorkProvider, fields: &[(&str, &str)]) -> std::result::Result<Value, NetError> {
        let oauth = &provider.document["oauth"];
        let secret = self.provider_secret(provider);
        let mut form: Vec<(&str, String)> = fields.iter().map(|(k, v)| (*k, (*v).to_owned())).collect();
        form.push(("client_id", oauth["clientId"].as_str().unwrap_or_default().to_owned()));
        if !secret.is_empty() {
            form.push(("client_secret", secret));
        }
        let body = form.iter().map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v))).collect::<Vec<_>>().join("&");
        let response = http("POST", oauth["tokenUrl"].as_str().unwrap_or_default(), &[("content-type", "application/x-www-form-urlencoded".into()), ("accept", "application/json".into())], body.into_bytes(), 1024 * 1024).await?;
        let value = response.json().unwrap_or(Value::Null);
        if response.status != 200 || value["access_token"].as_str().is_none_or(str::is_empty) {
            let reason = value["error_description"].as_str().or(value["error"].as_str()).unwrap_or("no access token");
            return Err(NetError(format!("signIn:The provider refused the sign-in: {reason}")));
        }
        Ok(value)
    }

    /// Opens and signs in to the account's server.
    async fn connect(&self, work: &Work) -> std::result::Result<Server, NetError> {
        let provider = work.provider.clone().ok_or_else(|| NetError("The administrator removed this account's mail provider".into()))?;
        if !provider.enabled {
            return Err(NetError("The administrator turned off this account's mail provider".into()));
        }
        let mut credential = self.inner.credentials.decrypt(&work.credential).ok_or_else(|| NetError("signIn:The saved sign-in can no longer be read; sign in again".into()))?;
        self.connect_with(Some(&work.id), &provider, work, &mut credential).await
    }

    async fn connect_with(&self, account_id: Option<&str>, provider: &WorkProvider, work: &Work, credential: &mut Value) -> std::result::Result<Server, NetError> {
        let doc = &provider.document;
        let user = if work.username.is_empty() { work.address.clone() } else { work.username.clone() };
        let kind = credential["kind"].as_str().unwrap_or_default().to_owned();
        if doc["protocol"] == "jmap" {
            let authorization = match kind.as_str() {
                "password" => format!("Basic {}", b64(format!("{user}:{}", credential["password"].as_str().unwrap_or_default()).as_bytes())),
                "token" => format!("Bearer {}", credential["token"].as_str().unwrap_or_default()),
                _ => format!("Bearer {}", self.access_token(account_id, provider, credential).await?),
            };
            return Ok(Server::Jmap(Box::new(Jmap::connect(doc["jmap"]["url"].as_str().unwrap_or_default(), authorization).await?)));
        }
        let (host, port, security) = server_text(doc, "imap");
        let mut imap = Imap::connect(&host, port, &security).await?;
        if kind == "oauth" {
            let token = self.access_token(account_id, provider, credential).await?;
            imap.xoauth2(&user, &token).await?;
        } else {
            imap.login(&user, credential["password"].as_str().unwrap_or_default()).await?;
        }
        Ok(Server::Imap(Box::new(imap)))
    }

    /// Checks a new sign-in against the server before it is saved.
    pub async fn verify(&self, provider: &WorkProvider, address: &str, username: &str, credential: &mut Value) -> Result<()> {
        let work = Work { id: String::new(), address: address.into(), display_name: String::new(), username: username.into(), credential: String::new(), provider: None, mailboxes: Vec::new(), messages: Vec::new() };
        let server = self.connect_with(None, provider, &work, credential).await.map_err(app)?;
        server.close().await;
        Ok(())
    }

    pub async fn provider(&self, id: &str) -> Result<WorkProvider> {
        let value = self.worker("provider", json!({ "id": id })).await?;
        serde_json::from_value(value).map_err(|_| AppError::not_found())
    }

    // ---------- OAuth ----------

    pub async fn oauth_start(&self, session_id: &str, provider_id: &str, name: &str, address: &str, display_name: &str, account_id: &str) -> Result<String> {
        let provider = self.provider(provider_id).await?;
        if !provider.enabled || provider.document["auth"] != "oauth" {
            return Err(AppError::invalid());
        }
        let oauth = &provider.document["oauth"];
        let state = random_token();
        let verifier = random_token();
        use sha2::Digest;
        let challenge = {
            use base64::Engine;
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
        };
        let scopes: Vec<String> = oauth["scopes"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        let mut params: Vec<(String, String)> = vec![
            ("response_type".into(), "code".into()),
            ("client_id".into(), oauth["clientId"].as_str().unwrap_or_default().into()),
            ("redirect_uri".into(), self.redirect_uri()),
            ("scope".into(), scopes.join(" ")),
            ("state".into(), state.clone()),
            ("code_challenge".into(), challenge),
            ("code_challenge_method".into(), "S256".into()),
            ("login_hint".into(), address.into()),
        ];
        if let Some(extra) = oauth["params"].as_object() {
            for (k, v) in extra {
                if let Some(v) = v.as_str() {
                    params.push((k.clone(), v.into()));
                }
            }
        }
        let base = oauth["authorizeUrl"].as_str().unwrap_or_default();
        let url = format!("{base}{}{}", if base.contains('?') { "&" } else { "?" }, params.iter().map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v))).collect::<Vec<_>>().join("&"));
        let mut states = self.inner.oauth.lock().expect("oauth");
        let cutoff = now_ms() - 15.0 * 60_000.0;
        states.retain(|_, s| s.created > cutoff);
        states.insert(state, OAuthState {
            session_id: session_id.into(), provider_id: provider_id.into(), verifier, name: name.into(), address: address.into(),
            display_name: display_name.into(), account_id: account_id.into(), created: now_ms(),
        });
        Ok(url)
    }

    fn redirect_uri(&self) -> String {
        format!("{}/api/v1/mail/oauth/callback", self.inner.origin)
    }

    /// The provider's redirect back: exchanges the code, checks the server accepts the token,
    /// and adds the account (or renews an existing account's sign-in). Returns where to go.
    pub async fn oauth_callback(&self, query: &HashMap<String, String>) -> String {
        let state = query.get("state").cloned().unwrap_or_default();
        let Some(pending) = self.inner.oauth.lock().expect("oauth").remove(&state) else {
            return "/mail?oauthError=expired".into();
        };
        if pending.created < now_ms() - 15.0 * 60_000.0 {
            return "/mail?oauthError=expired".into();
        }
        if let Some(e) = query.get("error") {
            return format!("/mail?oauthError={}", url_encode(e));
        }
        let code = query.get("code").cloned().unwrap_or_default();
        let result: Result<String> = async {
            let provider = self.provider(&pending.provider_id).await?;
            let tokens = self.token_request(&provider, &[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", &self.redirect_uri()), ("code_verifier", &pending.verifier)]).await.map_err(app)?;
            let mut credential = json!({
                "kind": "oauth", "access": tokens["access_token"], "refresh": tokens["refresh_token"].as_str().unwrap_or_default(),
                "expires": now_ms() + tokens["expires_in"].as_f64().unwrap_or(3600.0) * 1000.0,
            });
            self.verify(&provider, &pending.address, "", &mut credential).await?;
            let sealed = self.seal_credential(&credential);
            if !pending.account_id.is_empty() {
                self.user(&pending.session_id, "updateAccount", json!({ "id": pending.account_id, "credential": sealed })).await?;
                return Ok(pending.account_id.clone());
            }
            let account = self.user(&pending.session_id, "createAccount", json!({
                "providerId": pending.provider_id, "name": pending.name, "address": pending.address, "displayName": pending.display_name, "username": "", "credential": sealed,
            })).await?;
            Ok(account["id"].as_str().unwrap_or_default().to_owned())
        }
        .await;
        match result {
            Ok(id) => {
                self.poke();
                format!("/mail?account={}", url_encode(&id))
            }
            Err(e) => format!("/mail?oauthError={}", url_encode(&e.message)),
        }
    }

    // ---------- The background pass ----------

    pub fn poke(&self) {
        if self.closing() {
            return;
        }
        {
            let mut ticking = self.inner.ticking.lock().expect("ticking");
            if *ticking {
                return;
            }
            *ticking = true;
        }
        let rt = self.clone();
        let handle = tokio::spawn(async move {
            for _ in 0..10 {
                if rt.closing() {
                    break;
                }
                let Ok(value) = rt.worker("due", json!({ "limit": 4 })).await else { break };
                let accounts: Vec<Work> = value["accounts"].as_array().map(|a| a.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect()).unwrap_or_default();
                if accounts.is_empty() {
                    break;
                }
                use futures_util::stream::{self, StreamExt};
                stream::iter(accounts).map(|w| rt.sync(w)).buffer_unordered(2).collect::<Vec<()>>().await;
            }
            *rt.inner.ticking.lock().expect("ticking") = false;
        });
        let mut tasks = self.inner.tasks.lock().expect("tasks");
        tasks.retain(|t| !t.is_finished());
        tasks.push(handle);
    }

    /// Runs a pass for one account now and waits for it (tests and "Sync now").
    /// A pass already running for the account is waited for, then this one runs with the
    /// account as it is now.
    pub async fn sync_now(&self, account_id: &str) -> Result<()> {
        let lock = self.pass_lock(account_id);
        let _guard = lock.lock().await;
        let value = self.worker("account", json!({ "accountId": account_id })).await?;
        let work: Work = serde_json::from_value(value).map_err(|_| AppError::not_found())?;
        self.run_pass(work).await;
        Ok(())
    }

    fn pass_lock(&self, account_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.inner.passes.lock().expect("passes").entry(account_id.to_owned()).or_default().clone()
    }

    /// A background pass; skipped when one is already running for the account.
    async fn sync(&self, work: Work) {
        let lock = self.pass_lock(&work.id);
        let Ok(_guard) = lock.try_lock() else { return };
        self.run_pass(work).await;
    }

    async fn run_pass(&self, work: Work) {
        let result = tokio::select! {
            r = self.pass(&work) => r,
            _ = async { let mut c = self.inner.closed.subscribe(); let _ = c.wait_for(|v| *v).await; } => Err(NetError("Dar is stopping".into())),
        };
        match result {
            Ok(()) => {
                let _ = self.worker("finish", json!({ "accountId": work.id })).await;
            }
            Err(e) => {
                let sign_in = e.0.starts_with("signIn:");
                let message: String = e.0.strip_prefix("signIn:").unwrap_or(&e.0).chars().take(480).collect();
                let _ = self.worker("fail", json!({ "accountId": work.id, "error": if message.is_empty() { "The mail server could not be reached".into() } else { message }, "signIn": sign_in })).await;
            }
        }
    }

    async fn w(&self, operation: &str, value: Value) -> std::result::Result<Value, NetError> {
        self.worker(operation, value).await.map_err(|e| NetError(format!("Dar could not record mail ({})", e.code)))
    }

    async fn read_blob(&self, blob_id: &str) -> std::result::Result<Vec<u8>, NetError> {
        let path = self.inner.blobs.path(blob_id).map_err(|_| NetError("A mirrored message is missing".into()))?;
        tokio::fs::read(path).await.map_err(|_| NetError("A mirrored message could not be read".into()))
    }

    pub async fn read_blob_app(&self, blob_id: &str) -> Result<Vec<u8>> {
        self.read_blob(blob_id).await.map_err(|_| AppError::with("contentUnavailable", 404, "This message's file is missing."))
    }

    async fn pass(&self, work: &Work) -> std::result::Result<(), NetError> {
        let mut server = self.connect(work).await?;
        let result = self.pass_with(work, &mut server).await;
        server.close().await;
        result
    }

    async fn pass_with(&self, work: &Work, server: &mut Server) -> std::result::Result<(), NetError> {
        let account = work.id.as_str();
        let delimiter = match server {
            Server::Imap(i) => i.delimiter().await.unwrap_or_else(|_| "/".into()),
            Server::Jmap(_) => String::new(),
        };
        let by_id: HashMap<String, WorkMailbox> = work.mailboxes.iter().map(|m| (m.id.clone(), m.clone())).collect();
        let trash = work.mailboxes.iter().find(|m| m.role == "trash").cloned();
        // 1. Files → server.
        let plan = self.w("plan", json!({ "accountId": account })).await?;
        let mut bound: Vec<Value> = Vec::new();
        let mut folder_remote: HashMap<String, String> = HashMap::new();
        for r in plan["renamed"].as_array().cloned().unwrap_or_default() {
            let (Some(m), Some(name)) = (by_id.get(r["mailboxId"].as_str().unwrap_or_default()), r["name"].as_str()) else { continue };
            match server {
                Server::Imap(i) => {
                    let parent = by_id.get(&m.parent_id).map(|p| p.remote.clone());
                    let remote = match parent {
                        Some(p) => format!("{p}{delimiter}{}", encode_name(name)),
                        None => encode_name(name),
                    };
                    if i.rename(&m.remote, &remote).await.is_ok() {
                        bound.push(json!({ "remote": remote, "mailboxId": m.id }));
                    }
                }
                Server::Jmap(j) => {
                    let _ = j.rename_mailbox(&m.remote, name).await;
                }
            }
        }
        for f in plan["newFolders"].as_array().cloned().unwrap_or_default() {
            let name = f["name"].as_str().unwrap_or("Mailbox");
            let folder = f["folderId"].as_str().unwrap_or_default().to_owned();
            let parent = by_id.get(f["parentMailboxId"].as_str().unwrap_or_default()).map(|p| p.remote.clone())
                .or_else(|| folder_remote.get(f["parentFolderId"].as_str().unwrap_or_default()).cloned());
            let remote = match server {
                Server::Imap(i) => {
                    let remote = match &parent {
                        Some(p) => format!("{p}{delimiter}{}", encode_name(name)),
                        None => encode_name(name),
                    };
                    i.create(&remote).await?;
                    remote
                }
                Server::Jmap(j) => j.create_mailbox(name, parent.as_deref().unwrap_or_default()).await?,
            };
            folder_remote.insert(folder.clone(), remote.clone());
            bound.push(json!({ "remote": remote, "folderId": folder }));
        }
        // Mailboxes as the server has them now (with folders made in Files bound to theirs).
        let mailboxes = self.remote_mailboxes(server).await?;
        let stored = self.w("mailboxes", json!({ "accountId": account, "mailboxes": mailboxes, "bound": bound })).await?;
        let rows: Vec<WorkMailbox> = serde_json::from_value(stored["mailboxes"].clone()).unwrap_or_default();
        let row_by_id: HashMap<String, WorkMailbox> = rows.iter().map(|m| (m.id.clone(), m.clone())).collect();
        let row_by_folder: HashMap<String, WorkMailbox> = rows.iter().map(|m| (m.folder_id.clone(), m.clone())).collect();
        let target_of = |v: &Value| -> Option<WorkMailbox> {
            v["targetMailboxId"].as_str().and_then(|id| row_by_id.get(id)).or_else(|| v["targetFolderId"].as_str().and_then(|f| row_by_folder.get(f))).or_else(|| v["mailboxId"].as_str().and_then(|id| row_by_id.get(id))).cloned()
        };
        // Deleted (or moved out of the mirror) in Files: to Trash on the server, or for good from Trash.
        let mut removed_ids = Vec::new();
        let mut by_mailbox: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for g in plan["gone"].as_array().into_iter().flatten().chain(plan["detached"].as_array().into_iter().flatten()) {
            by_mailbox.entry(g["mailboxId"].as_str().unwrap_or_default().to_owned()).or_default().push((g["id"].as_str().unwrap_or_default().to_owned(), g["remote"].as_str().unwrap_or_default().to_owned()));
        }
        for (mailbox_id, list) in by_mailbox {
            let Some(mailbox) = row_by_id.get(&mailbox_id) else { continue };
            let remotes: Vec<String> = list.iter().map(|(_, r)| r.clone()).collect();
            let to_trash = trash.as_ref().filter(|t| t.id != mailbox.id).and_then(|t| row_by_id.get(&t.id));
            self.remove_remote(server, mailbox, &remotes, to_trash).await?;
            removed_ids.extend(list.into_iter().map(|(id, _)| id));
        }
        // Mailbox folders deleted in Files: their messages are written again on this pass.
        removed_ids.extend(plan["remirror"].as_array().into_iter().flatten().filter_map(|v| v["id"].as_str().map(str::to_owned)));
        if !removed_ids.is_empty() {
            self.w("remove", json!({ "accountId": account, "ids": removed_ids, "trashFiles": false })).await?;
        }
        // Moved between mailbox folders in Files.
        let mut moves = Vec::new();
        for m in plan["moved"].as_array().cloned().unwrap_or_default() {
            let (Some(from), Some(to)) = (row_by_id.get(m["mailboxId"].as_str().unwrap_or_default()), target_of(&m)) else { continue };
            let remote = m["remote"].as_str().unwrap_or_default().to_owned();
            let new_remote = self.move_remote(server, from, &to, std::slice::from_ref(&remote)).await?.remove(&remote).unwrap_or_default();
            moves.push(json!({ "id": m["id"], "mailboxId": to.id, "remote": new_remote }));
        }
        if !moves.is_empty() {
            self.w("moved", json!({ "accountId": account, "moves": moves, "moveFiles": false })).await?;
        }
        // Edited in Files: the new content replaces the old on the server.
        for c in plan["changed"].as_array().cloned().unwrap_or_default() {
            let Some(mailbox) = row_by_id.get(c["mailboxId"].as_str().unwrap_or_default()) else { continue };
            let data = self.read_blob(c["blobId"].as_str().unwrap_or_default()).await?;
            let remote = c["remote"].as_str().unwrap_or_default().to_owned();
            let new_remote = self.append(server, mailbox, &["\\Seen"], &data).await?;
            self.remove_remote(server, mailbox, std::slice::from_ref(&remote), None).await?;
            self.w("moved", json!({ "accountId": account, "moves": [{ "id": c["id"], "mailboxId": mailbox.id, "remote": new_remote, "blobId": c["blobId"] }], "moveFiles": false })).await?;
        }
        // `.eml` files added in Files: appended to their mailbox.
        for a in plan["added"].as_array().cloned().unwrap_or_default() {
            let Some(mailbox) = target_of(&a) else { continue };
            let blob = a["blobId"].as_str().unwrap_or_default();
            let data = self.read_blob(blob).await?;
            let mut meta = message::meta(&data, a["modifiedAt"].as_f64().unwrap_or_else(now_ms));
            let remote = self.append(server, &mailbox, &["\\Seen"], &data).await?;
            meta["blobId"] = json!(blob);
            meta["size"] = json!(data.len());
            meta["fileId"] = a["fileId"].clone();
            meta["seen"] = json!(true);
            meta["remote"] = json!(remote);
            self.w("pending", json!({ "accountId": account, "mailboxId": mailbox.id, "message": meta })).await?;
        }
        // 2. Server → Files.
        for mailbox in rows.iter().filter(|m| m.role != "folder") {
            if self.closing() {
                break;
            }
            self.pull_mailbox(account, server, mailbox).await?;
        }
        Ok(())
    }

    async fn remote_mailboxes(&self, server: &mut Server) -> std::result::Result<Vec<Value>, NetError> {
        Ok(match server {
            Server::Imap(i) => i.mailboxes().await?.into_iter().map(|m| json!({ "remote": m.remote, "name": m.name, "role": m.role, "parentRemote": m.parent_remote })).collect(),
            Server::Jmap(j) => j.mailboxes().await?.into_iter().map(|m| json!({ "remote": m.id, "name": m.name, "role": m.role, "parentRemote": m.parent_id })).collect(),
        })
    }

    async fn pull_mailbox(&self, account: &str, server: &mut Server, mailbox: &WorkMailbox) -> std::result::Result<(), NetError> {
        struct Remote {
            remote: String,
            seen: bool,
            flagged: bool,
            answered: bool,
            draft: bool,
            size: f64,
            blob: String,
        }
        let list: Vec<Remote> = match server {
            Server::Imap(i) => {
                let (validity, messages) = i.list(&mailbox.remote).await?;
                if validity != mailbox.validity {
                    self.w("validity", json!({ "accountId": account, "mailboxId": mailbox.id, "validity": validity })).await?;
                }
                messages.into_iter().map(|m| {
                    let has = |f: &str| m.flags.iter().any(|x| x.eq_ignore_ascii_case(f));
                    Remote { remote: m.uid.to_string(), seen: has("\\Seen"), flagged: has("\\Flagged"), answered: has("\\Answered"), draft: has("\\Draft"), size: m.size, blob: String::new() }
                }).collect()
            }
            Server::Jmap(j) => j.list(&mailbox.remote).await?.into_iter().map(|e| {
                let has = |k: &str| e.keywords.iter().any(|x| x == k);
                Remote { remote: e.id, seen: has("$seen"), flagged: has("$flagged"), answered: has("$answered"), draft: has("$draft"), size: e.size, blob: e.blob_id }
            }).collect(),
        };
        let known = self.w("known", json!({ "accountId": account, "mailboxId": mailbox.id })).await?;
        let mut known_by_remote: HashMap<String, (String, [bool; 4])> = HashMap::new();
        for k in known["messages"].as_array().cloned().unwrap_or_default() {
            let remote = k[1].as_str().unwrap_or_default().to_owned();
            if !remote.is_empty() {
                known_by_remote.insert(remote, (k[0].as_str().unwrap_or_default().to_owned(), [k[2] == true, k[3] == true, k[4] == true, k[5] == true]));
            }
        }
        let present: HashSet<&str> = list.iter().map(|r| r.remote.as_str()).collect();
        let removed: Vec<String> = known_by_remote.iter().filter(|(r, _)| !present.contains(r.as_str())).map(|(_, (id, _))| id.clone()).collect();
        if !removed.is_empty() {
            self.w("remove", json!({ "accountId": account, "ids": removed, "trashFiles": true })).await?;
        }
        let changes: Vec<Value> = list.iter().filter_map(|r| {
            let (id, flags) = known_by_remote.get(&r.remote)?;
            (*flags != [r.seen, r.flagged, r.answered, r.draft]).then(|| json!({ "id": id, "seen": r.seen, "flagged": r.flagged, "answered": r.answered, "draft": r.draft }))
        }).collect();
        if !changes.is_empty() {
            self.w("flags", json!({ "accountId": account, "changes": changes })).await?;
        }
        let fresh: Vec<&Remote> = list.iter().filter(|r| !known_by_remote.contains_key(&r.remote)).collect();
        // Newest last on the server; take newest first so recent mail shows early.
        let mut batch: Vec<&Remote> = Vec::new();
        let mut bytes = 0.0;
        for (n, r) in fresh.iter().rev().enumerate() {
            batch.push(r);
            bytes += r.size;
            if batch.len() >= FETCH_BATCH || bytes >= FETCH_BYTES || n + 1 == fresh.len() {
                if self.closing() {
                    return Ok(());
                }
                let fetched: Vec<(String, Vec<u8>)> = match server {
                    Server::Imap(i) => {
                        let uids: Vec<u32> = batch.iter().filter_map(|r| r.remote.parse().ok()).collect();
                        i.fetch(&mailbox.remote, &uids).await?.into_iter().map(|(u, d)| (u.to_string(), d)).collect()
                    }
                    Server::Jmap(j) => {
                        let mut out = Vec::new();
                        for r in &batch {
                            out.push((r.remote.clone(), j.download(&r.blob).await?));
                        }
                        out
                    }
                };
                let mut messages = Vec::new();
                for (remote, data) in fetched {
                    let Some(r) = batch.iter().find(|r| r.remote == remote) else { continue };
                    let blob = self.inner.blobs.store_bytes(data.clone()).await.map_err(|_| NetError("A message could not be stored".into()))?;
                    let mut meta = message::meta(&data, now_ms());
                    meta["remote"] = json!(remote);
                    meta["blobId"] = json!(blob);
                    meta["size"] = json!(data.len());
                    meta["seen"] = json!(r.seen);
                    meta["flagged"] = json!(r.flagged);
                    meta["answered"] = json!(r.answered);
                    meta["draft"] = json!(r.draft);
                    messages.push(meta);
                }
                if !messages.is_empty() {
                    let stored = self.worker("store", json!({ "accountId": account, "mailboxId": mailbox.id, "messages": messages })).await;
                    if let Err(e) = stored {
                        return Err(NetError(if e.code == "quotaExceeded" { "Your Files storage is full, so new mail is not being mirrored".into() } else { format!("Dar could not record mail ({})", e.code) }));
                    }
                }
                batch.clear();
                bytes = 0.0;
            }
        }
        Ok(())
    }

    // ---------- Server operations shared by the pass and actions ----------

    async fn move_remote(&self, server: &mut Server, from: &WorkMailbox, to: &WorkMailbox, remotes: &[String]) -> std::result::Result<HashMap<String, String>, NetError> {
        let mut out = HashMap::new();
        match server {
            Server::Imap(i) => {
                let uids: Vec<u32> = remotes.iter().filter_map(|r| r.parse().ok()).collect();
                for (a, b) in i.move_to(&from.remote, &uids, &to.remote).await? {
                    out.insert(a.to_string(), b.to_string());
                }
            }
            Server::Jmap(j) => {
                j.update(remotes.iter().map(|r| (r.clone(), json!({ format!("mailboxIds/{}", from.remote): null, format!("mailboxIds/{}", to.remote): true }))).collect()).await?;
                for r in remotes {
                    out.insert(r.clone(), r.clone());
                }
            }
        }
        Ok(out)
    }

    /// Moves to `trash` when given, otherwise removes for good.
    async fn remove_remote(&self, server: &mut Server, from: &WorkMailbox, remotes: &[String], trash: Option<&WorkMailbox>) -> std::result::Result<(), NetError> {
        let remotes: Vec<String> = remotes.iter().filter(|r| !r.is_empty()).cloned().collect();
        if remotes.is_empty() {
            return Ok(());
        }
        if let Some(t) = trash {
            self.move_remote(server, from, t, &remotes).await?;
            return Ok(());
        }
        match server {
            Server::Imap(i) => {
                let uids: Vec<u32> = remotes.iter().filter_map(|r| r.parse().ok()).collect();
                i.expunge(&from.remote, &uids).await
            }
            Server::Jmap(j) => j.destroy(&remotes).await,
        }
    }

    async fn append(&self, server: &mut Server, mailbox: &WorkMailbox, flags: &[&str], data: &[u8]) -> std::result::Result<String, NetError> {
        match server {
            Server::Imap(i) => Ok(i.append(&mailbox.remote, flags, data).await?.map(|u| u.to_string()).unwrap_or_default()),
            Server::Jmap(j) => {
                let keywords: Vec<&str> = flags.iter().map(|f| match *f {
                    "\\Seen" => "$seen",
                    "\\Flagged" => "$flagged",
                    "\\Answered" => "$answered",
                    _ => "$draft",
                }).collect();
                j.import(&mailbox.remote, &keywords, data).await
            }
        }
    }

    async fn prepare(&self, session_id: &str, value: Value) -> Result<Vec<Work>> {
        let prepared = self.user(session_id, "prepare", value).await?;
        serde_json::from_value(prepared["groups"].clone()).map_err(|_| AppError::internal())
    }

    // ---------- Actions ----------

    pub async fn set_flags(&self, session_id: &str, ids: Vec<String>, seen: Option<bool>, flagged: Option<bool>) -> Result<()> {
        for work in self.prepare(session_id, json!({ "ids": ids })).await? {
            let mut server = self.connect(&work).await.map_err(app)?;
            let result: std::result::Result<(), NetError> = async {
                let mut by_mailbox: HashMap<String, Vec<String>> = HashMap::new();
                for m in &work.messages {
                    by_mailbox.entry(m.mailbox_id.clone()).or_default().push(m.remote.clone());
                }
                for (mailbox_id, remotes) in by_mailbox {
                    let Some(mailbox) = work.mailboxes.iter().find(|m| m.id == mailbox_id) else { continue };
                    let remotes: Vec<String> = remotes.into_iter().filter(|r| !r.is_empty()).collect();
                    match &mut server {
                        Server::Imap(i) => {
                            let uids: Vec<u32> = remotes.iter().filter_map(|r| r.parse().ok()).collect();
                            if let Some(v) = seen {
                                i.store(&mailbox.remote, &uids, v, &["\\Seen"]).await?;
                            }
                            if let Some(v) = flagged {
                                i.store(&mailbox.remote, &uids, v, &["\\Flagged"]).await?;
                            }
                        }
                        Server::Jmap(j) => {
                            let mut patch = serde_json::Map::new();
                            if let Some(v) = seen {
                                patch.insert("keywords/$seen".into(), if v { json!(true) } else { Value::Null });
                            }
                            if let Some(v) = flagged {
                                patch.insert("keywords/$flagged".into(), if v { json!(true) } else { Value::Null });
                            }
                            j.update(remotes.iter().map(|r| (r.clone(), Value::Object(patch.clone()))).collect()).await?;
                        }
                    }
                }
                Ok(())
            }
            .await;
            server.close().await;
            result.map_err(app)?;
            let changes: Vec<Value> = work.messages.iter().map(|m| {
                let mut c = json!({ "id": m.id });
                if let Some(v) = seen {
                    c["seen"] = json!(v);
                }
                if let Some(v) = flagged {
                    c["flagged"] = json!(v);
                }
                c
            }).collect();
            self.worker("flags", json!({ "accountId": work.id, "changes": changes })).await?;
        }
        Ok(())
    }

    /// Moves messages to a mailbox by role (`archive`, `trash`, `junk`, `inbox`) or ID; moving to
    /// Trash what is already there deletes it for good (its file moves to deleted files).
    pub async fn move_messages(&self, session_id: &str, ids: Vec<String>, role: Option<&str>, mailbox_id: Option<&str>) -> Result<Value> {
        let mut moved = 0;
        for work in self.prepare(session_id, json!({ "ids": ids })).await? {
            let target = match (role, mailbox_id) {
                (Some(r), _) => work.mailboxes.iter().find(|m| m.role == r),
                (_, Some(id)) => work.mailboxes.iter().find(|m| m.id == id),
                _ => None,
            }
            .cloned()
            .ok_or_else(|| AppError::with("notFound", 404, &format!("This account has no {} mailbox.", role.unwrap_or("such"))))?;
            let mut server = self.connect(&work).await.map_err(app)?;
            let mut moves = Vec::new();
            let mut removed = Vec::new();
            let result: std::result::Result<(), NetError> = async {
                let mut by_mailbox: HashMap<String, Vec<&WorkMessage>> = HashMap::new();
                for m in &work.messages {
                    by_mailbox.entry(m.mailbox_id.clone()).or_default().push(m);
                }
                for (from_id, list) in by_mailbox {
                    let Some(from) = work.mailboxes.iter().find(|m| m.id == from_id) else { continue };
                    let remotes: Vec<String> = list.iter().map(|m| m.remote.clone()).collect();
                    if from.id == target.id {
                        if target.role == "trash" {
                            self.remove_remote(&mut server, from, &remotes, None).await?;
                            removed.extend(list.iter().map(|m| m.id.clone()));
                        }
                        continue;
                    }
                    let map = self.move_remote(&mut server, from, &target, &remotes).await?;
                    for m in list {
                        moves.push(json!({ "id": m.id, "mailboxId": target.id, "remote": map.get(&m.remote).cloned().unwrap_or_default() }));
                    }
                }
                Ok(())
            }
            .await;
            server.close().await;
            result.map_err(app)?;
            moved += moves.len() + removed.len();
            if !moves.is_empty() {
                self.worker("moved", json!({ "accountId": work.id, "moves": moves, "moveFiles": true })).await?;
            }
            if !removed.is_empty() {
                self.worker("remove", json!({ "accountId": work.id, "ids": removed, "trashFiles": true })).await?;
            }
        }
        Ok(json!({ "moved": moved }))
    }

    /// Sends (or with `draft`, saves to Drafts) a message from an account. A saved draft that
    /// this replaces is removed. Returns the new message's index ID.
    pub async fn send(&self, session_id: &str, input: &Value, attachments: Vec<Attachment>, draft: bool) -> Result<Value> {
        let account_id = input["accountId"].as_str().unwrap_or_default();
        let work = self.prepare(session_id, json!({ "accountId": account_id })).await?.into_iter().next().ok_or_else(AppError::not_found)?;
        let reply = match input["inReplyTo"].as_str().filter(|s| !s.is_empty()) {
            Some(id) => Some(self.user(session_id, "message", json!({ "id": id })).await?),
            None => None,
        };
        let list = |key: &str| -> Vec<String> { input[key].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.trim().to_owned())).filter(|s| !s.is_empty()).collect()).unwrap_or_default() };
        let (to, cc, bcc) = (list("to"), list("cc"), list("bcc"));
        for address in to.iter().chain(&cc).chain(&bcc) {
            if !address.contains('@') || address.contains(char::is_whitespace) || address.contains(['<', '>', ',', ';']) {
                return Err(AppError::with("invalidInput", 400, &format!("{address} is not an email address.")));
            }
        }
        if !draft && to.is_empty() && cc.is_empty() && bcc.is_empty() {
            return Err(AppError::with("invalidInput", 400, "Add at least one recipient."));
        }
        let mut references: Vec<String> = Vec::new();
        let mut in_reply_to = None;
        if let Some(r) = &reply {
            let parent_id = r["messageId"].as_str().unwrap_or_default().to_owned();
            if let Ok(data) = self.read_blob_app(r["blobId"].as_str().unwrap_or_default()).await {
                references = message::render(&data, "", "", false)["references"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
            }
            if !parent_id.is_empty() {
                references.push(parent_id.clone());
                in_reply_to = Some(parent_id);
            }
        }
        let seed = random_token();
        let outgoing = message::build(Draft {
            from_name: &work.display_name, from: &work.address, to, cc, bcc, subject: input["subject"].as_str().unwrap_or_default(), text: input["text"].as_str().unwrap_or_default(),
            in_reply_to: in_reply_to.as_deref(), references, attachments, date_ms: now_ms() as i64, id_seed: &seed[..24], markdown: input["format"] == "markdown",
        })
        .map_err(|_| AppError::internal())?;
        let role = if draft { "drafts" } else { "sent" };
        let folder = work.mailboxes.iter().find(|m| m.role == role).cloned();
        if draft && folder.is_none() {
            return Err(AppError::with("notFound", 404, "This account has no Drafts mailbox."));
        }
        let provider = work.provider.clone().ok_or_else(AppError::not_found)?;
        let save_sent = provider.document["saveSent"] != false;
        let mut server = self.connect(&work).await.map_err(app)?;
        let result: std::result::Result<String, NetError> = async {
            match (&mut server, draft) {
                (Server::Jmap(j), false) => {
                    let sent = folder.as_ref().ok_or_else(|| NetError("This account has no Sent mailbox".into()))?;
                    j.submit(&sent.remote, &work.address, &outgoing.recipients, &outgoing.data).await
                }
                (_, false) => {
                    let mut credential = self.inner.credentials.decrypt(&work.credential).ok_or_else(|| NetError("signIn:The saved sign-in can no longer be read".into()))?;
                    let (host, port, security) = server_text(&provider.document, "smtp");
                    let user = if work.username.is_empty() { work.address.clone() } else { work.username.clone() };
                    if credential["kind"] == "oauth" {
                        let token = self.access_token(Some(&work.id), &provider, &mut credential).await?;
                        super::smtp::send(&host, port, &security, super::smtp::Auth::OAuth(&user, &token), &work.address, &outgoing.recipients, &outgoing.data).await?;
                    } else {
                        super::smtp::send(&host, port, &security, super::smtp::Auth::Password(&user, credential["password"].as_str().unwrap_or_default()), &work.address, &outgoing.recipients, &outgoing.data).await?;
                    }
                    match (&folder, save_sent) {
                        (Some(sent), true) => self.append(&mut server, sent, &["\\Seen"], &outgoing.data).await,
                        _ => Ok(String::new()),
                    }
                }
                (_, true) => self.append(&mut server, folder.as_ref().expect("drafts"), &["\\Seen", "\\Draft"], &outgoing.data).await,
            }
        }
        .await;
        // A replaced draft goes; a reply marks its parent answered.
        let replaced = input["replaces"].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        if result.is_ok() {
            if let Some(old) = &replaced
                && let Ok(groups) = self.prepare(session_id, json!({ "ids": [old] })).await
                && let Some(g) = groups.into_iter().next()
                && let Some(m) = g.messages.first()
                && let Some(mailbox) = g.mailboxes.iter().find(|b| b.id == m.mailbox_id)
                && self.remove_remote(&mut server, mailbox, std::slice::from_ref(&m.remote), None).await.is_ok()
            {
                let _ = self.worker("remove", json!({ "accountId": g.id, "ids": [m.id], "trashFiles": true })).await;
            }
            if !draft && let Some(r) = &reply {
                if let Ok(groups) = self.prepare(session_id, json!({ "ids": [r["id"]] })).await
                    && let Some(g) = groups.into_iter().next()
                    && let Some(m) = g.messages.first()
                    && let Some(mailbox) = g.mailboxes.iter().find(|b| b.id == m.mailbox_id)
                {
                    let ok = match &mut server {
                        Server::Imap(i) => i.store(&mailbox.remote, &m.remote.parse::<u32>().into_iter().collect::<Vec<_>>(), true, &["\\Answered"]).await.is_ok(),
                        Server::Jmap(j) => j.update(vec![(m.remote.clone(), json!({ "keywords/$answered": true }))]).await.is_ok(),
                    };
                    if ok {
                        let _ = self.worker("flags", json!({ "accountId": g.id, "changes": [{ "id": m.id, "answered": true }] })).await;
                    }
                }
            }
        }
        server.close().await;
        let remote = result.map_err(app)?;
        // Shown at once; the next pass gives a message without a UID its server identity.
        let mut id = Value::Null;
        if let Some(target) = folder.filter(|_| draft || save_sent || matches!(provider.document["protocol"].as_str(), Some("jmap"))) {
            let blob = self.inner.blobs.store_bytes(outgoing.data.clone()).await.map_err(|_| AppError::internal())?;
            let mut meta = message::meta(&outgoing.data, now_ms());
            meta["blobId"] = json!(blob);
            meta["size"] = json!(outgoing.data.len());
            meta["seen"] = json!(true);
            meta["draft"] = json!(draft);
            meta["remote"] = json!(remote);
            if let Ok(v) = self.worker("pending", json!({ "accountId": work.id, "mailboxId": target.id, "message": meta })).await {
                id = v["id"].clone();
            }
        }
        Ok(json!({ "id": id, "messageId": outgoing.message_id }))
    }

    pub async fn close(&self) {
        let _ = self.inner.closed.send(true);
        let tasks: Vec<JoinHandle<()>> = std::mem::take(&mut *self.inner.tasks.lock().expect("tasks"));
        for t in tasks {
            let _ = tokio::time::timeout(Duration::from_secs(10), t).await;
        }
    }
}

pub fn url_encode(value: &str) -> String {
    value.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}
