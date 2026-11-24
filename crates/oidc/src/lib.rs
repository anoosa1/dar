//! Dar's OpenID provider on oxide-auth: a standalone prototype of
//! `next/runtime/identity/provider.ts`, not yet wired into Dar.
//!
//! The host (Dar's runtime) supplies accounts and native sessions through [`Directory`],
//! persists records through [`store::Store`] and the keyring through [`keys::KeyState`],
//! mounts [`Provider::router`] at `/oidc`, and serves the sign-in page that calls
//! [`Provider::interaction_details`] and [`Provider::decide`].

pub mod config;
mod http;
pub mod keys;
mod legacy;
mod oauth;
pub mod store;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use url::Url;

pub use config::{Client, ClientInput, ClientKind, ProviderConfig};
pub use keys::{KeyState, Keyring};
pub use store::{MemoryStore, Revoke, Store};

use oauth::{AuthRequest, Clients, CodeContext, Flow, Params, PkceExtension, Records};
use oxide_auth::code_grant::authorization::{self, authorization_code};
use store::{GrantRecord, Interaction, InteractionResult, Prompt, ProviderSession};

#[derive(Debug)]
pub enum Error {
    Config(String),
    InvalidClient(String),
    /// The interaction is unknown, expired or belongs to another browser.
    InteractionExpired,
    InvalidInput,
    /// The native session is older than the request and the user must confirm their password.
    PasswordRequired,
    /// Consent must come from the account shown in the request.
    AccountChanged,
    InvalidGrant,
    Internal(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Config(m) | Error::InvalidClient(m) => f.write_str(m),
            Error::InteractionExpired => f.write_str(
                "This sign-in request expired. Return to the application and try again.",
            ),
            Error::InvalidInput => f.write_str("Invalid input."),
            Error::PasswordRequired => f.write_str("Confirm your password to continue."),
            Error::AccountChanged => {
                f.write_str("Use the account shown in the request, or switch accounts.")
            }
            Error::InvalidGrant => {
                f.write_str("This sign-in request expired or was already used. Try again.")
            }
            Error::Internal(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

/// A Dar account as a relying party may see it.
#[derive(Clone, Debug, Default)]
pub struct Account {
    pub id: String,
    pub user_name: String,
    pub display_name: String,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub email: Option<String>,
    pub groups: Vec<String>,
}

/// A native Dar browser session.
#[derive(Clone, Debug)]
pub struct NativeSession {
    pub id: String,
    pub account_id: String,
    /// Milliseconds since the epoch.
    pub created_at: i64,
}

/// Dar's directory and native sessions, as the provider needs them.
pub trait Directory: Send + Sync + 'static {
    /// The account, or `None` when it is unknown, suspended or deleted.
    fn account(&self, id: &str) -> Option<Account>;
    /// The native session, or `None` once it ended.
    fn native_session(&self, id: &str) -> Option<NativeSession>;
}

pub(crate) fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(crate) fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the system random source is available");
    B64.encode(buf)
}

pub(crate) fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Storage key for a secret: its SHA-256, hex encoded.
pub(crate) fn key(secret: &str) -> String {
    digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What the sign-in page shows.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionDetails {
    pub uid: String,
    pub prompt: &'static str,
    pub client: ClientSummary,
    pub permissions: Vec<Permission>,
    pub requires_password: bool,
    pub provider_account_id: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientSummary {
    pub id: String,
    pub name: String,
    pub redirect_uri: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Permission {
    pub scope: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug)]
pub enum Decision {
    Deny,
    /// Sign in as the native session's account. The host confirms the password first when
    /// `requires_password` was set and passes `password_confirmed`.
    Login {
        password_confirmed: bool,
    },
    Approve,
}

pub(crate) struct State {
    pub store: Box<dyn Store>,
    pub clients: BTreeMap<String, Client>,
    pub keys: Keyring,
}

pub(crate) struct Inner {
    pub config: ProviderConfig,
    pub directory: Arc<dyn Directory>,
    pub state: Mutex<State>,
}

#[derive(Clone)]
pub struct Provider {
    pub(crate) inner: Arc<Inner>,
}

/// The result of an authorization request.
pub(crate) enum Outcome {
    /// Redirect to the client (`query`) or post to it (`form_post`).
    Respond {
        redirect: Url,
        form_post: bool,
        params: Vec<(String, String)>,
    },
    /// Send the browser to Dar's sign-in page; `cookie` binds the interaction to it.
    Interact { uid: String },
    /// No trustworthy redirect: show an error page.
    Page(String),
}

impl Provider {
    pub fn new(
        config: ProviderConfig,
        keys: Keyring,
        store: Box<dyn Store>,
        directory: Arc<dyn Directory>,
    ) -> Provider {
        let mut clients = BTreeMap::new();
        let immich = Client::immich();
        clients.insert(immich.id.clone(), immich);
        Provider {
            inner: Arc::new(Inner {
                config,
                directory,
                state: Mutex::new(State {
                    store,
                    clients,
                    keys,
                }),
            }),
        }
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.inner.config
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        match self.inner.state.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                // Store and directory hooks may call `block_in_place` + `Handle::block_on` while
                // holding this mutex; blocking a Tokio worker directly here can leave no worker
                // polling the I/O driver when the other workers are parked idle.
                let acquire = || self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                match tokio::runtime::Handle::try_current() {
                    Ok(handle)
                        if handle.runtime_flavor()
                            == tokio::runtime::RuntimeFlavor::MultiThread =>
                    {
                        tokio::task::block_in_place(acquire)
                    }
                    _ => acquire(),
                }
            }
        }
    }

    /// Caches one administered client as the database has it now (`None`: removed). Built-in
    /// clients stay. The host calls this before each request that names a client, as Dar's
    /// adapter looks clients up when they are needed.
    pub fn put_client(&self, id: &str, client: Option<Client>) {
        let mut state = self.lock();
        if state.clients.get(id).is_some_and(|c| c.built_in) {
            return;
        }
        match client {
            Some(client) => {
                state.clients.insert(id.to_owned(), client);
            }
            None => {
                state.clients.remove(id);
            }
        }
    }

    /// Replace the administered clients. Removed or disabled clients lose their grants.
    pub fn set_clients(&self, clients: Vec<Client>) {
        let mut state = self.lock();
        let keep: BTreeSet<String> = clients
            .iter()
            .filter(|c| c.enabled)
            .map(|c| c.id.clone())
            .collect();
        let dropped: Vec<String> = state
            .clients
            .values()
            .filter(|c| !c.built_in && !keep.contains(&c.id))
            .map(|c| c.id.clone())
            .collect();
        for id in dropped {
            state.store.revoke(Revoke::Client(id));
        }
        state.clients.retain(|_, c| c.built_in);
        for client in clients {
            if !state.clients.get(&client.id).is_some_and(|c| c.built_in) {
                state.clients.insert(client.id.clone(), client);
            }
        }
    }

    /// Revoke grants, tokens and provider sessions (logout, password change, suspension…).
    pub fn revoke(&self, what: Revoke) {
        self.lock().store.revoke(what);
    }

    /// The signing keyring as it stands.
    pub fn key_state(&self) -> KeyState {
        self.lock().keys.state().clone()
    }

    /// Rotate the signing key. Returns the state for the host to persist.
    pub fn rotate_keys(&self) -> Result<KeyState, Error> {
        let mut state = self.lock();
        let keep = self.inner.config.lifetimes.retired_key;
        state.keys.rotate(keep)?;
        Ok(state.keys.state().clone())
    }

    /// Periodic cleanup. Returns the key state when retired keys were dropped.
    pub fn maintain(&self) -> Option<KeyState> {
        let mut state = self.lock();
        state.store.cleanup(now());
        state.keys.prune().then(|| state.keys.state().clone())
    }

    pub(crate) fn sign_cookie(&self, value: &str) -> String {
        sign_with(&self.lock().keys, value)
    }

    pub(crate) fn verify_cookie(&self, signed: &str) -> Option<String> {
        verify_with(&self.lock().keys, signed)
    }

    /// Whether `<name>.sig` is the TypeScript stack's signature of `<name>=<value>` (see `legacy`).
    pub(crate) fn legacy_cookie_valid(&self, name: &str, value: &str, signature: &str) -> bool {
        legacy::valid(self.lock().keys.cookie_key(), name, value, signature)
    }

    /// `Set-Cookie` for the sign-in API, scoped to `path` so separate tabs never collide.
    pub fn interaction_cookie(&self, uid: &str, path: &str) -> String {
        let secure = if self.inner.config.development {
            ""
        } else {
            "; Secure"
        };
        let max_age = self.inner.config.lifetimes.interaction;
        format!(
            "dar_oidc_interaction={}; Path={path}; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}",
            self.sign_cookie(uid)
        )
    }

    /// True when `cookie` is a valid signature for `uid` and the interaction still exists.
    pub fn verify_interaction(&self, uid: &str, cookie: Option<&str>) -> bool {
        let state = self.lock();
        self.interaction_for(&state, uid, cookie).is_ok()
    }

    fn interaction_for(
        &self,
        state: &State,
        uid: &str,
        cookie: Option<&str>,
    ) -> Result<Interaction, Error> {
        let bound = cookie
            .and_then(|c| verify_with(&state.keys, c))
            .is_some_and(|v| v == uid);
        if !bound {
            return Err(Error::InteractionExpired);
        }
        state
            .store
            .interaction(&key(uid))
            .ok_or(Error::InteractionExpired)
    }

    /// What Dar's sign-in page shows for an interaction (`GET /api/v1/identity/interactions/:uid`).
    pub fn interaction_details(
        &self,
        uid: &str,
        cookie: Option<&str>,
        native: &NativeSession,
    ) -> Result<InteractionDetails, Error> {
        let state = self.lock();
        let interaction = self.interaction_for(&state, uid, cookie)?;
        let client = state
            .clients
            .get(&interaction.client_id)
            .filter(|c| c.enabled)
            .ok_or(Error::InteractionExpired)?;
        let params = Params::parse(interaction.params.clone());
        let request = AuthRequest::new(&params, Some(client));
        let mut permissions: Vec<Permission> = request
            .scope
            .as_deref()
            .unwrap_or("openid")
            .split(' ')
            .map(|s| Permission {
                scope: s.into(),
                label: config::scope_label(s).into(),
            })
            .collect();
        permissions.extend(client.permissions.iter().map(|(scope, label)| Permission {
            scope: scope.clone(),
            label: label.clone(),
        }));
        Ok(InteractionDetails {
            uid: uid.to_owned(),
            prompt: interaction.prompt.as_str(),
            client: ClientSummary {
                id: client.id.clone(),
                name: client.name.clone(),
                redirect_uri: params.get("redirect_uri").unwrap_or_default().to_owned(),
            },
            permissions,
            requires_password: interaction.prompt == Prompt::Login
                && native.created_at < interaction.created_at,
            provider_account_id: interaction.account_id,
        })
    }

    /// Record the user's decision; returns where the browser continues (`<issuer>/auth/<uid>`).
    pub fn decide(
        &self,
        uid: &str,
        cookie: Option<&str>,
        native: &NativeSession,
        decision: Decision,
    ) -> Result<String, Error> {
        if self
            .inner
            .directory
            .native_session(&native.id)
            .is_none_or(|s| s.account_id != native.account_id)
        {
            return Err(Error::InteractionExpired);
        }
        let mut state = self.lock();
        let mut interaction = self.interaction_for(&state, uid, cookie)?;
        let result = match decision {
            Decision::Deny => InteractionResult::Denied,
            Decision::Login { password_confirmed } => {
                if native.created_at < interaction.created_at && !password_confirmed {
                    return Err(Error::PasswordRequired);
                }
                InteractionResult::Login {
                    account_id: native.account_id.clone(),
                    native_session_id: native.id.clone(),
                    at: now() / 1000,
                }
            }
            Decision::Approve if interaction.prompt == Prompt::Consent => {
                if interaction.account_id.as_deref() != Some(native.account_id.as_str()) {
                    return Err(Error::AccountChanged);
                }
                let session_key = interaction
                    .session_key
                    .clone()
                    .ok_or(Error::InteractionExpired)?;
                let session = state
                    .store
                    .session(&session_key)
                    .ok_or(Error::InteractionExpired)?;
                let existing = session
                    .grants
                    .get(&interaction.client_id)
                    .and_then(|id| state.store.grant(id));
                let mut grant = match existing {
                    Some(g)
                        if g.account_id == native.account_id
                            && g.client_id == interaction.client_id =>
                    {
                        g
                    }
                    Some(_) => return Err(Error::InvalidGrant),
                    None => GrantRecord {
                        id: random_token(24),
                        account_id: native.account_id.clone(),
                        client_id: interaction.client_id.clone(),
                        scopes: BTreeSet::new(),
                        native_session_id: session.native_session_id.clone(),
                        session_key: session_key.clone(),
                        expires_at: session.expires_at,
                    },
                };
                grant
                    .scopes
                    .extend(interaction.missing_scopes.iter().cloned());
                let grant_id = grant.id.clone();
                state.store.save_grant(grant);
                InteractionResult::Consent { grant_id }
            }
            Decision::Approve => return Err(Error::InvalidInput),
        };
        interaction.result = Some(result);
        state.store.save_interaction(&key(uid), interaction);
        Ok(format!("{}/auth/{uid}", self.inner.config.issuer()))
    }

    /// Redeem a code for a built-in client inside Dar (the Immich app's sign-in), with the
    /// token endpoint's checks (client, redirect, S256 PKCE, grant, session, replay) but no
    /// tokens issued. Returns the account ID.
    pub fn redeem_built_in_code(
        &self,
        client_id: &str,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
    ) -> Result<String, Error> {
        use oxide_auth::code_grant::extensions::Pkce;
        use oxide_auth::primitives::grant::Value;
        let mut guard = self.lock();
        let state = &mut *guard;
        if !state
            .clients
            .get(client_id)
            .is_some_and(|c| c.built_in && c.enabled)
        {
            return Err(Error::InvalidClient("not a built-in client".into()));
        }
        let well_formed = (1..=128).contains(&code.len())
            && code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !well_formed {
            return Err(Error::InvalidGrant);
        }
        let code_key = key(code);
        let source = state.store.code(&code_key).ok_or(Error::InvalidGrant)?;
        let redirect_matches = source.redirect_uri == redirect_uri
            || (Url::parse(redirect_uri)
                .ok()
                .zip(Url::parse(&source.redirect_uri).ok())
                .is_some_and(|(a, b)| a == b));
        if source.client_id != client_id
            || source.expires_at <= now()
            || !redirect_matches
            || source.grant_id.is_empty()
            || source.account_id.is_empty()
        {
            return Err(Error::InvalidGrant);
        }
        let pkce = Pkce::required();
        let challenge = source
            .pkce
            .as_ref()
            .map(|p| Value::private(Some(p.clone())));
        if !oauth::valid_verifier(verifier)
            || pkce.verify(challenge, Some(verifier.into())).is_err()
        {
            return Err(Error::InvalidGrant);
        }
        let grant = state
            .store
            .grant(&source.grant_id)
            .ok_or(Error::InvalidGrant)?;
        if grant.expires_at <= now()
            || grant.client_id != client_id
            || grant.account_id != source.account_id
        {
            return Err(Error::InvalidGrant);
        }
        if source.consumed {
            state.store.revoke(Revoke::Grant(source.grant_id));
            return Err(Error::InvalidGrant);
        }
        let directory = self.inner.directory.as_ref();
        if !state
            .store
            .session(&source.session_key)
            .is_some_and(|s| s.account_id == source.account_id)
            || !directory
                .native_session(&source.native_session_id)
                .is_some_and(|s| s.account_id == source.account_id)
        {
            return Err(Error::InvalidGrant);
        }
        match state.store.consume_code(&code_key) {
            store::CodeUse::Fresh(_) => {}
            store::CodeUse::Replayed(r) => {
                state.store.revoke(Revoke::Grant(r.grant_id));
                return Err(Error::InvalidGrant);
            }
            store::CodeUse::Missing => return Err(Error::InvalidGrant),
        }
        if directory.account(&source.account_id).is_none() {
            return Err(Error::InvalidGrant);
        }
        Ok(source.account_id)
    }

    /// Run an authorization request. `resume` carries a finished interaction.
    pub(crate) fn authorize(
        &self,
        params: Params,
        session_cookie: Option<&str>,
        resume: Option<Interaction>,
    ) -> (Outcome, Option<String>) {
        let config = &self.inner.config;
        let directory = self.inner.directory.as_ref();
        let session_key = session_cookie
            .and_then(|c| self.verify_cookie(c))
            .map(|v| key(&v));
        let mut guard = self.lock();
        let state = &mut *guard;
        let client = params
            .get("client_id")
            .and_then(|id| state.clients.get(id))
            .filter(|c| c.enabled)
            .cloned();
        let request = AuthRequest::new(&params, client.as_ref());
        let mut flow = Flow {
            clients: Clients(&state.clients),
            records: Records {
                store: state.store.as_mut(),
                directory,
                clients: &state.clients,
                keys: &state.keys,
                config,
                approving: None,
                redeemed: None,
                rejected_code: false,
                id_token: None,
                replayed_grant: Default::default(),
            },
            pkce: PkceExtension::for_client(client.as_ref()),
        };
        let issuer = config.issuer();
        let form_post = params.get("response_mode") == Some("form_post");
        let respond = |mut url: Url, extra: &[(&str, &str)]| {
            let mut pairs: Vec<(String, String)> = extra
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            if let Some(state) = params.get("state") {
                pairs.push(("state".into(), state.into()));
            }
            pairs.push(("iss".into(), issuer.clone()));
            if form_post {
                Outcome::Respond {
                    redirect: url,
                    form_post: true,
                    params: pairs,
                }
            } else {
                url.query_pairs_mut().extend_pairs(pairs.iter());
                Outcome::Respond {
                    redirect: url,
                    form_post: false,
                    params: Vec::new(),
                }
            }
        };
        let pending = match authorization_code(&mut flow, &request) {
            Ok(pending) => pending,
            Err(authorization::Error::Redirect(error)) => {
                // Keep oxide-auth's error fields; `respond` adds `state` and `iss`.
                let url: Url = error.into();
                let extra: Vec<(String, String)> = url
                    .query_pairs()
                    .filter(|(k, _)| {
                        matches!(k.as_ref(), "error" | "error_description" | "error_uri")
                    })
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                let refs: Vec<(&str, &str)> = extra
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                return match params.get("redirect_uri").and_then(|u| Url::parse(u).ok()) {
                    Some(base) => (respond(base, &refs), None),
                    None => (
                        Outcome::Page("The application sent an invalid sign-in request.".into()),
                        None,
                    ),
                };
            }
            Err(_) => {
                return (
                    Outcome::Page("The application sent an invalid sign-in request.".into()),
                    None,
                );
            }
        };
        let client = client.expect("oxide-auth accepted the client");
        let redirect = pending.pre_grant().redirect_uri.to_url();
        let error = |code: &str, description: &str| {
            respond(
                redirect.clone(),
                &[("error", code), ("error_description", description)],
            )
        };
        if params.get("request").is_some() || params.get("request_uri").is_some() {
            return (
                error(
                    "request_not_supported",
                    "Request objects are not supported.",
                ),
                None,
            );
        }
        if params
            .get("response_mode")
            .is_some_and(|m| m != "query" && m != "form_post")
        {
            return (error("invalid_request", "Unsupported response_mode."), None);
        }
        let prompts: BTreeSet<&str> = params
            .get("prompt")
            .map(|p| p.split(' ').collect())
            .unwrap_or_default();
        if prompts
            .iter()
            .any(|p| !matches!(*p, "none" | "login" | "consent"))
            || (prompts.contains("none") && prompts.len() > 1)
        {
            return (
                error("invalid_request", "Unsupported prompt value requested."),
                None,
            );
        }
        let max_age = match params.get("max_age").map(str::parse::<i64>) {
            None => None,
            Some(Ok(v)) if v >= 0 => Some(v),
            Some(_) => {
                return (
                    error("invalid_request", "max_age must be a non-negative integer."),
                    None,
                );
            }
        };
        let hint_sub = match params.get("id_token_hint") {
            None => None,
            Some(hint) => match state.keys.verify_hint(hint, &issuer) {
                Some(claims) => claims["sub"].as_str().map(str::to_owned),
                None => {
                    return (
                        error("invalid_request", "Could not validate id_token_hint."),
                        None,
                    );
                }
            },
        };
        let interact = |state: &mut State,
                        prompt: Prompt,
                        session_key: Option<String>,
                        account_id: Option<String>,
                        missing: Vec<String>,
                        logged_in: bool| {
            let uid = random_token(24);
            let created_at = now();
            let interaction = Interaction {
                params: params.pairs(),
                client_id: client.id.clone(),
                prompt,
                session_key,
                account_id,
                missing_scopes: missing,
                created_at,
                expires_at: created_at + config.lifetimes.interaction * 1000,
                logged_in,
                result: None,
            };
            state.store.save_interaction(&key(&uid), interaction);
            Outcome::Interact { uid }
        };

        // Resume: apply what the user decided.
        let mut new_cookie = None;
        let mut session_key = session_key;
        let (mut logged_in, mut consented) = (false, false);
        if let Some(done) = &resume {
            logged_in = done.logged_in;
            match &done.result {
                Some(InteractionResult::Denied) => {
                    return (
                        error("access_denied", "The user declined the request."),
                        None,
                    );
                }
                Some(InteractionResult::Login {
                    account_id,
                    native_session_id,
                    at,
                }) => {
                    // Signing in as someone else ends the previous provider session.
                    if let Some(old_key) = session_key.as_ref().filter(|k| {
                        state
                            .store
                            .session(k)
                            .is_some_and(|s| &s.account_id != account_id)
                    }) {
                        state.store.revoke(Revoke::Session(old_key.clone()));
                    }
                    let uid = random_token(24);
                    let session = ProviderSession {
                        account_id: account_id.clone(),
                        native_session_id: native_session_id.clone(),
                        auth_time: *at,
                        expires_at: now() + config.lifetimes.session * 1000,
                        grants: BTreeMap::new(),
                    };
                    state.store.save_session(&key(&uid), session);
                    session_key = Some(key(&uid));
                    new_cookie = Some(uid);
                    logged_in = true;
                }
                Some(InteractionResult::Consent { grant_id }) => {
                    if let Some(k) = &session_key
                        && let Some(mut session) = state.store.session(k)
                    {
                        session.grants.insert(client.id.clone(), grant_id.clone());
                        state.store.save_session(k, session);
                        consented = true;
                    }
                }
                None => {}
            }
        }

        // Login: a live provider session whose native session and account still exist.
        let session = session_key
            .as_ref()
            .and_then(|k| state.store.session(k))
            .filter(|s| {
                directory
                    .native_session(&s.native_session_id)
                    .is_some_and(|n| n.account_id == s.account_id)
                    && directory.account(&s.account_id).is_some()
            });
        let at = now() / 1000;
        // max_age=0 asks for a new sign-in each time, like prompt=login, even within the same second.
        let needs_login = match &session {
            None => true,
            Some(s) => {
                !logged_in
                    && (prompts.contains("login")
                        || max_age.is_some_and(|m| m == 0 || at - s.auth_time > m)
                        || hint_sub.as_ref().is_some_and(|sub| sub != &s.account_id))
            }
        };
        if needs_login {
            if prompts.contains("none") {
                return (
                    error("login_required", "End-User authentication is required."),
                    new_cookie,
                );
            }
            let account = session.as_ref().map(|s| s.account_id.clone());
            return (
                interact(
                    state,
                    Prompt::Login,
                    session_key.clone(),
                    account,
                    Vec::new(),
                    false,
                ),
                new_cookie,
            );
        }
        let session = session.expect("checked above");
        let session_key = session_key.expect("a session has a key");

        // Consent: every requested scope must be granted in this provider session.
        let requested: Vec<String> = pending
            .pre_grant()
            .scope
            .iter()
            .map(str::to_owned)
            .collect();
        let grant = session
            .grants
            .get(&client.id)
            .and_then(|id| state.store.grant(id))
            .filter(|g| g.account_id == session.account_id);
        let missing: Vec<String> = match (&grant, prompts.contains("consent") && !consented) {
            (Some(g), false) => requested
                .iter()
                .filter(|s| !g.scopes.contains(*s))
                .cloned()
                .collect(),
            _ => requested.clone(),
        };
        if !missing.is_empty() || grant.is_none() {
            if prompts.contains("none") {
                return (
                    error("consent_required", "End-User consent is required."),
                    new_cookie,
                );
            }
            return (
                interact(
                    state,
                    Prompt::Consent,
                    Some(session_key),
                    Some(session.account_id.clone()),
                    missing,
                    logged_in,
                ),
                new_cookie,
            );
        }
        let grant = grant.expect("checked above");
        let mut flow = Flow {
            clients: Clients(&state.clients),
            records: Records {
                store: state.store.as_mut(),
                directory,
                clients: &state.clients,
                keys: &state.keys,
                config,
                approving: Some(CodeContext {
                    account_id: session.account_id.clone(),
                    nonce: params.get("nonce").map(str::to_owned),
                    auth_time: session.auth_time,
                    grant_id: grant.id.clone(),
                    session_key: session_key.clone(),
                    native_session_id: session.native_session_id.clone(),
                }),
                redeemed: None,
                rejected_code: false,
                id_token: None,
                replayed_grant: Default::default(),
            },
            pkce: PkceExtension::for_client(Some(&client)),
        };
        match pending.authorize(&mut flow, session.account_id.clone().into()) {
            Ok(url) => {
                let code = url
                    .query_pairs()
                    .find(|(k, _)| k == "code")
                    .map(|(_, v)| v.into_owned())
                    .unwrap_or_default();
                (respond(redirect, &[("code", &code)]), new_cookie)
            }
            Err(_) => (
                Outcome::Page("The sign-in request could not be completed.".into()),
                new_cookie,
            ),
        }
    }
}

fn sign_with(keys: &Keyring, value: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(keys.cookie_key()).expect("HMAC accepts any key");
    mac.update(value.as_bytes());
    format!("{value}.{}", B64.encode(mac.finalize().into_bytes()))
}

fn verify_with(keys: &Keyring, signed: &str) -> Option<String> {
    let (value, signature) = signed.rsplit_once('.')?;
    let signature = B64.decode(signature).ok()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(keys.cookie_key()).ok()?;
    mac.update(value.as_bytes());
    mac.verify_slice(&signature).ok()?;
    Some(value.to_owned())
}
