//! oxide-auth adapters: Dar's clients as a `Registrar`, provider records as the code
//! `Authorizer` and token `Issuer`, and PKCE as the grant extension.
//! oxide-auth drives request validation, client authentication, code redemption and
//! refresh; the OpenID layer (ID tokens, prompts, sessions) is in `lib.rs` and `http.rs`.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD as B64};
use chrono::{DateTime, Utc};
use oxide_auth::code_grant::{accesstoken, authorization, extensions::Pkce, refresh};
use oxide_auth::primitives::grant::{Extensions, Grant, Value};
use oxide_auth::primitives::issuer::{IssuedToken, RefreshedToken, TokenType};
use oxide_auth::primitives::prelude::{Authorizer, Issuer, PreGrant, Registrar, Scope};
use oxide_auth::primitives::registrar::{BoundClient, ClientUrl, RegisteredUrl, RegistrarError};
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::Url;

use crate::config::{Client, ClientKind, ProviderConfig};
use crate::keys::Keyring;
use crate::store::{CodeRecord, CodeUse, Revoke, Store, TokenKind, TokenRecord};
use crate::{Directory, digest, key, now, random_token};

/// Request parameters, rejected when any is repeated (RFC 6749 §3.1).
#[derive(Clone, Debug, Default)]
pub struct Params {
    pub map: BTreeMap<String, String>,
    pub valid: bool,
}

impl Params {
    pub fn parse(pairs: Vec<(String, String)>) -> Params {
        let mut map = BTreeMap::new();
        let mut valid = true;
        for (k, v) in pairs {
            if map.insert(k, v).is_some() {
                valid = false;
            }
        }
        Params { map, valid }
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str).filter(|v| !v.is_empty())
    }
    pub fn pairs(&self) -> Vec<(String, String)> {
        self.map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

// ---------- Registrar ----------

pub struct Clients<'a>(pub &'a BTreeMap<String, Client>);

impl<'a> Clients<'a> {
    pub fn get(&self, id: &str) -> Option<&'a Client> {
        self.0.get(id).filter(|c| c.enabled)
    }
    /// Constant-time client authentication: `client_secret_basic` or `none`.
    pub fn authenticate(&self, id: &str, secret: Option<&[u8]>) -> Option<&'a Client> {
        let client = self.get(id)?;
        match (&client.kind, secret) {
            (ClientKind::Public, None) => Some(client),
            (ClientKind::Confidential { secret_digest }, Some(secret)) => {
                bool::from(digest(secret).ct_eq(secret_digest)).then_some(client)
            }
            _ => None,
        }
    }
}

impl Registrar for Clients<'_> {
    fn bound_redirect<'a>(&self, bound: ClientUrl<'a>) -> Result<BoundClient<'a>, RegistrarError> {
        let client = self.get(&bound.client_id).ok_or(RegistrarError::Unspecified)?;
        // OpenID Connect requires redirect_uri, compared as an exact string.
        let requested = bound.redirect_uri.as_ref().ok_or(RegistrarError::Unspecified)?;
        if !client.redirect_uris.iter().any(|uri| uri == requested.as_str()) {
            return Err(RegistrarError::Unspecified);
        }
        let redirect = RegisteredUrl::Exact(requested.as_ref().clone());
        Ok(BoundClient { client_id: bound.client_id, redirect_uri: Cow::Owned(redirect) })
    }

    fn negotiate(&self, bound: BoundClient, scope: Option<Scope>) -> Result<PreGrant, RegistrarError> {
        // `AuthRequest::scope` already dropped scopes the client may not use.
        let scope = scope.ok_or(RegistrarError::Unspecified)?;
        if !scope.iter().any(|s| s == "openid") {
            return Err(RegistrarError::Unspecified);
        }
        Ok(PreGrant { client_id: bound.client_id.into_owned(), redirect_uri: bound.redirect_uri.into_owned(), scope })
    }

    fn check(&self, client_id: &str, passphrase: Option<&[u8]>) -> Result<(), RegistrarError> {
        self.authenticate(client_id, passphrase).map(|_| ()).ok_or(RegistrarError::Unspecified)
    }
}

// ---------- Requests ----------

pub struct AuthRequest<'a> {
    pub params: &'a Params,
    /// Requested scopes this client may use; unknown scopes are ignored, and
    /// `offline_access` needs `prompt=consent` (OpenID Connect Core §11).
    pub scope: Option<String>,
}

impl<'a> AuthRequest<'a> {
    pub fn new(params: &'a Params, client: Option<&Client>) -> Self {
        let consent = params.get("prompt").is_some_and(|p| p.split(' ').any(|v| v == "consent"));
        let scope = params.get("scope").map(|requested| {
            let mut kept: Vec<&str> = Vec::new();
            for s in requested.split(' ') {
                let allowed = client.is_some_and(|c| c.scopes.contains(s)) && (s != "offline_access" || consent);
                if allowed && !kept.contains(&s) {
                    kept.push(s);
                }
            }
            kept.join(" ")
        });
        AuthRequest { params, scope }
    }
}

impl authorization::Request for AuthRequest<'_> {
    fn valid(&self) -> bool {
        self.params.valid
    }
    fn client_id(&self) -> Option<Cow<'_, str>> {
        self.params.get("client_id").map(Cow::Borrowed)
    }
    fn scope(&self) -> Option<Cow<'_, str>> {
        self.scope.as_deref().filter(|s| !s.is_empty()).map(Cow::Borrowed)
    }
    fn redirect_uri(&self) -> Option<Cow<'_, str>> {
        self.params.get("redirect_uri").map(Cow::Borrowed)
    }
    fn state(&self) -> Option<Cow<'_, str>> {
        self.params.get("state").map(Cow::Borrowed)
    }
    fn response_type(&self) -> Option<Cow<'_, str>> {
        self.params.get("response_type").map(Cow::Borrowed)
    }
    fn extension(&self, key: &str) -> Option<Cow<'_, str>> {
        self.params.get(key).map(Cow::Borrowed)
    }
}

/// A token, introspection or revocation request.
pub struct TokenRequest {
    pub form: Params,
    /// Credentials from `Authorization: Basic`, form-decoded (RFC 6749 §2.3.1).
    pub basic: Option<(String, Vec<u8>)>,
    pub malformed_auth: bool,
}

fn form_decode(value: &str) -> Option<String> {
    url::form_urlencoded::parse(format!("v={value}").as_bytes()).next().map(|(_, v)| v.into_owned())
}

impl TokenRequest {
    pub fn new(form: Params, authorization: Option<&str>) -> TokenRequest {
        let mut request = TokenRequest { form, basic: None, malformed_auth: false };
        if let Some(header) = authorization {
            let parsed = header
                .strip_prefix("Basic ")
                .and_then(|b| STANDARD.decode(b.trim()).ok())
                .and_then(|raw| String::from_utf8(raw).ok())
                .and_then(|text| {
                    let (id, secret) = text.split_once(':')?;
                    Some((form_decode(id)?, form_decode(secret)?.into_bytes()))
                });
            match parsed {
                Some(credentials) => request.basic = Some(credentials),
                None => request.malformed_auth = true,
            }
        }
        request
    }

    /// The client this request claims to come from.
    pub fn client_id(&self) -> Option<&str> {
        self.basic.as_ref().map(|(id, _)| id.as_str()).or(self.form.get("client_id"))
    }

    /// Authenticate the client the way the token endpoint does.
    pub fn client<'a>(&self, clients: &Clients<'a>) -> Option<&'a Client> {
        if self.malformed_auth {
            return None;
        }
        match (&self.basic, self.form.get("client_id")) {
            (Some((id, secret)), body) if body.is_none_or(|b| b == id) => clients.authenticate(id, Some(secret)),
            (None, Some(id)) => clients.authenticate(id, None),
            _ => None,
        }
    }
}

impl accesstoken::Request for TokenRequest {
    fn valid(&self) -> bool {
        self.form.valid && !self.malformed_auth
    }
    fn code(&self) -> Option<Cow<'_, str>> {
        self.form.get("code").map(Cow::Borrowed)
    }
    fn authorization(&self) -> accesstoken::Authorization<'_> {
        match &self.basic {
            Some((id, secret)) => accesstoken::Authorization::UsernamePassword(Cow::Borrowed(id), Cow::Borrowed(secret)),
            None => accesstoken::Authorization::None,
        }
    }
    fn client_id(&self) -> Option<Cow<'_, str>> {
        self.form.get("client_id").map(Cow::Borrowed)
    }
    fn redirect_uri(&self) -> Option<Cow<'_, str>> {
        self.form.get("redirect_uri").map(Cow::Borrowed)
    }
    fn grant_type(&self) -> Option<Cow<'_, str>> {
        self.form.get("grant_type").map(Cow::Borrowed)
    }
    fn extension(&self, key: &str) -> Option<Cow<'_, str>> {
        self.form.get(key).map(Cow::Borrowed)
    }
}

impl refresh::Request for TokenRequest {
    fn valid(&self) -> bool {
        self.form.valid && !self.malformed_auth
    }
    fn refresh_token(&self) -> Option<Cow<'_, str>> {
        self.form.get("refresh_token").map(Cow::Borrowed)
    }
    fn scope(&self) -> Option<Cow<'_, str>> {
        self.form.get("scope").map(Cow::Borrowed)
    }
    fn grant_type(&self) -> Option<Cow<'_, str>> {
        self.form.get("grant_type").map(Cow::Borrowed)
    }
    fn authorization(&self) -> Option<(Cow<'_, str>, Cow<'_, [u8]>)> {
        self.basic.as_ref().map(|(id, secret)| (Cow::Borrowed(id.as_str()), Cow::Borrowed(secret.as_slice())))
    }
    fn extension(&self, key: &str) -> Option<Cow<'_, str>> {
        self.form.get(key).map(Cow::Borrowed)
    }
}

// ---------- PKCE ----------

/// S256 only; required unless a confidential client opted out.
pub struct PkceExtension(pub Pkce);

impl PkceExtension {
    pub fn for_client(client: Option<&Client>) -> Self {
        PkceExtension(if client.is_some_and(|c| !c.require_pkce) { Pkce::optional() } else { Pkce::required() })
    }
}

pub(crate) fn valid_verifier(v: &str) -> bool {
    (43..=128).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
}

impl authorization::Extension for PkceExtension {
    fn extend(&mut self, request: &dyn authorization::Request) -> Result<Extensions, ()> {
        let mut data = Extensions::new();
        if let Some(value) = self.0.challenge(request.extension("code_challenge_method"), request.extension("code_challenge"))? {
            data.set(&self.0, value);
        }
        Ok(data)
    }
}

impl accesstoken::Extension for PkceExtension {
    fn extend(&mut self, request: &dyn accesstoken::Request, mut data: Extensions) -> Result<Extensions, ()> {
        let verifier = request.extension("code_verifier");
        if verifier.as_deref().is_some_and(|v| !valid_verifier(v)) {
            return Err(());
        }
        self.0.verify(data.remove(&self.0), verifier)?;
        Ok(Extensions::new())
    }
}

// ---------- Codes and tokens ----------

/// Who approved an authorization request; attached to the code it creates.
#[derive(Clone)]
pub struct CodeContext {
    pub account_id: String,
    pub nonce: Option<String>,
    pub auth_time: i64,
    pub grant_id: String,
    pub session_key: String,
    pub native_session_id: String,
}

pub struct Records<'a> {
    pub store: &'a mut dyn Store,
    pub directory: &'a dyn Directory,
    pub clients: &'a BTreeMap<String, Client>,
    pub keys: &'a Keyring,
    pub config: &'a ProviderConfig,
    /// Set before `Pending::authorize`.
    pub approving: Option<CodeContext>,
    /// The code redeemed by this request.
    pub redeemed: Option<CodeRecord>,
    /// The presented code was unknown, expired, replayed or no longer backed by a session.
    pub rejected_code: bool,
    /// The ID token minted with the last issued tokens.
    pub id_token: Option<String>,
    /// A rotated refresh token was presented again; its grant is revoked afterwards.
    pub replayed_grant: RefCell<Option<String>>,
}

fn time(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_default()
}

impl Records<'_> {
    /// A record stays usable while its native Dar session and account are.
    fn alive(&self, native_session_id: &str, account_id: &str) -> bool {
        self.directory.native_session(native_session_id).is_some_and(|s| s.account_id == account_id)
            && self.directory.account(account_id).is_some()
    }

    fn id_token(&self, client_id: &str, account_id: &str, auth_time: i64, nonce: Option<&str>, access: &str) -> Option<String> {
        let at = now() / 1000;
        let hash = Sha256::digest(access.as_bytes());
        let mut claims = json!({
            "iss": self.config.issuer(), "sub": account_id, "aud": client_id,
            "iat": at, "exp": at + self.config.lifetimes.id_token, "auth_time": auth_time,
            "at_hash": B64.encode(&hash[..16]),
        });
        if let Some(nonce) = nonce {
            claims["nonce"] = json!(nonce);
        }
        self.keys.sign(&claims).ok()
    }

    fn mint(&mut self, client_id: &str, scope: &Scope, from: &TokenRecord) -> (String, Option<String>, i64) {
        let at = now();
        let access = random_token(32);
        let access_expiry = at + self.config.lifetimes.access_token * 1000;
        let record = TokenRecord { kind: TokenKind::Access, scope: scope.to_string(), issued_at: at, expires_at: access_expiry, consumed: false, ..from.clone() };
        self.store.save_token(&key(&access), record);
        let refreshable = scope.iter().any(|s| s == "offline_access") && self.clients.get(client_id).is_some_and(Client::allows_refresh);
        let refresh = refreshable.then(|| {
            let token = random_token(32);
            // Refresh tokens rotate and never outlive their grant (and so their session).
            let grant_expiry = self.store.grant(&from.grant_id).map_or(at, |g| g.expires_at);
            let record = TokenRecord { kind: TokenKind::Refresh, scope: scope.to_string(), issued_at: at, expires_at: grant_expiry, consumed: false, ..from.clone() };
            self.store.save_token(&key(&token), record);
            token
        });
        (access, refresh, access_expiry)
    }

    fn grant_for(&self, record: &TokenRecord) -> Grant {
        Grant {
            owner_id: record.account_id.clone(),
            client_id: record.client_id.clone(),
            scope: record.scope.parse().unwrap_or_else(|_| "openid".parse().unwrap()),
            redirect_uri: Url::parse(&self.config.issuer()).expect("issuer is a URL"),
            until: time(record.expires_at),
            extensions: Extensions::new(),
        }
    }
}

impl Authorizer for Records<'_> {
    fn authorize(&mut self, grant: Grant) -> Result<String, ()> {
        let context = self.approving.take().ok_or(())?;
        let mut extensions = grant.extensions;
        let pkce = match extensions.remove(&Pkce::optional()) {
            Some(value) => value.into_private_value()?,
            None => None,
        };
        let code = random_token(32);
        let record = CodeRecord {
            client_id: grant.client_id,
            account_id: context.account_id,
            redirect_uri: grant.redirect_uri.to_string(),
            scope: grant.scope.to_string(),
            pkce,
            nonce: context.nonce,
            auth_time: context.auth_time,
            grant_id: context.grant_id,
            session_key: context.session_key,
            native_session_id: context.native_session_id,
            expires_at: now() + self.config.lifetimes.code * 1000,
            consumed: false,
        };
        self.store.save_code(&key(&code), record);
        Ok(code)
    }

    fn extract(&mut self, code: &str) -> Result<Option<Grant>, ()> {
        let record = match self.store.consume_code(&key(code)) {
            CodeUse::Fresh(record) => record,
            CodeUse::Replayed(record) => {
                // A replayed code revokes everything its grant issued.
                self.store.revoke(Revoke::Grant(record.grant_id));
                self.rejected_code = true;
                return Ok(None);
            }
            CodeUse::Missing => {
                self.rejected_code = true;
                return Ok(None);
            }
        };
        if record.expires_at <= now() || self.store.grant(&record.grant_id).is_none() || !self.alive(&record.native_session_id, &record.account_id) {
            self.rejected_code = true;
            return Ok(None);
        }
        let mut extensions = Extensions::new();
        if let Some(pkce) = &record.pkce {
            extensions.set(&Pkce::optional(), Value::private(Some(pkce.clone())));
        }
        let grant = Grant {
            owner_id: record.account_id.clone(),
            client_id: record.client_id.clone(),
            scope: record.scope.parse().map_err(|_| ())?,
            // oxide-auth compares this with the token request's redirect_uri.
            redirect_uri: Url::parse(&record.redirect_uri).map_err(|_| ())?,
            until: time(record.expires_at),
            extensions,
        };
        self.redeemed = Some(record);
        Ok(Some(grant))
    }
}

impl Issuer for Records<'_> {
    fn issue(&mut self, grant: Grant) -> Result<IssuedToken, ()> {
        let code = self.redeemed.clone().ok_or(())?;
        let base = TokenRecord {
            kind: TokenKind::Access,
            client_id: grant.client_id.clone(),
            account_id: code.account_id.clone(),
            scope: String::new(),
            grant_id: code.grant_id.clone(),
            session_key: code.session_key.clone(),
            native_session_id: code.native_session_id.clone(),
            auth_time: code.auth_time,
            issued_at: 0,
            expires_at: 0,
            consumed: false,
        };
        let (access, refresh, until) = self.mint(&grant.client_id, &grant.scope, &base);
        self.id_token = self.id_token(&grant.client_id, &code.account_id, code.auth_time, code.nonce.as_deref(), &access);
        Ok(IssuedToken { token: access, refresh, until: time(until), token_type: TokenType::Bearer })
    }

    fn refresh(&mut self, refresh: &str, grant: Grant) -> Result<RefreshedToken, ()> {
        let refresh_key = key(refresh);
        let mut old = self.store.token(&refresh_key).filter(|t| t.kind == TokenKind::Refresh && !t.consumed).ok_or(())?;
        old.consumed = true;
        self.store.save_token(&refresh_key, old.clone());
        let (access, next, until) = self.mint(&grant.client_id, &grant.scope, &old);
        self.id_token = grant.scope.iter().any(|s| s == "openid").then(|| self.id_token(&grant.client_id, &old.account_id, old.auth_time, None, &access)).flatten();
        Ok(RefreshedToken { token: access, refresh: next, until: time(until), token_type: TokenType::Bearer })
    }

    fn recover_token<'a>(&'a self, token: &'a str) -> Result<Option<Grant>, ()> {
        Ok(self
            .store
            .token(&key(token))
            .filter(|t| t.kind == TokenKind::Access && self.alive(&t.native_session_id, &t.account_id))
            .map(|t| self.grant_for(&t)))
    }

    fn recover_refresh<'a>(&'a self, token: &'a str) -> Result<Option<Grant>, ()> {
        let Some(record) = self.store.token(&key(token)).filter(|t| t.kind == TokenKind::Refresh) else {
            return Ok(None);
        };
        if record.consumed {
            *self.replayed_grant.borrow_mut() = Some(record.grant_id);
            return Ok(None);
        }
        if self.store.grant(&record.grant_id).is_none() || !self.alive(&record.native_session_id, &record.account_id) {
            return Ok(None);
        }
        Ok(Some(self.grant_for(&record)))
    }
}

/// Bundles the primitives for oxide-auth's flows.
pub struct Flow<'a> {
    pub clients: Clients<'a>,
    pub records: Records<'a>,
    pub pkce: PkceExtension,
}

impl authorization::Endpoint for Flow<'_> {
    fn registrar(&self) -> &dyn Registrar {
        &self.clients
    }
    fn authorizer(&mut self) -> &mut dyn Authorizer {
        &mut self.records
    }
    fn extension(&mut self) -> &mut dyn authorization::Extension {
        &mut self.pkce
    }
}

impl accesstoken::Endpoint for Flow<'_> {
    fn registrar(&self) -> &dyn Registrar {
        &self.clients
    }
    fn authorizer(&mut self) -> &mut dyn Authorizer {
        &mut self.records
    }
    fn issuer(&mut self) -> &mut dyn Issuer {
        &mut self.records
    }
    fn extension(&mut self) -> &mut dyn accesstoken::Extension {
        &mut self.pkce
    }
}

impl refresh::Endpoint for Flow<'_> {
    fn registrar(&self) -> &dyn Registrar {
        &self.clients
    }
    fn issuer(&mut self) -> &mut dyn Issuer {
        &mut self.records
    }
}

