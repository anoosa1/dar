//! Provider records and their storage boundary.
//!
//! In Dar these live in private SpacetimeDB tables reached through the service identity
//! (`service_provider` in the module). Secrets (codes, tokens, session and interaction IDs)
//! are only ever stored by their SHA-256 (`crate::key`), as `modelKey` does today.

use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ProviderSession {
    pub account_id: String,
    pub native_session_id: String,
    /// Seconds since the epoch.
    pub auth_time: i64,
    pub expires_at: i64,
    /// Grants made in this provider session, by client.
    pub grants: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Prompt {
    Login,
    Consent,
}

impl Prompt {
    pub fn as_str(self) -> &'static str {
        match self {
            Prompt::Login => "login",
            Prompt::Consent => "consent",
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum InteractionResult {
    Login { account_id: String, native_session_id: String, at: i64 },
    Consent { grant_id: String },
    Denied,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Interaction {
    /// The validated authorization request, replayed when the browser resumes.
    pub params: Vec<(String, String)>,
    pub client_id: String,
    pub prompt: Prompt,
    pub session_key: Option<String>,
    pub account_id: Option<String>,
    pub missing_scopes: Vec<String>,
    /// Milliseconds; compared with the native session's creation time.
    pub created_at: i64,
    pub expires_at: i64,
    /// Prompts already satisfied earlier in this authorization request.
    pub logged_in: bool,
    pub result: Option<InteractionResult>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GrantRecord {
    pub id: String,
    pub account_id: String,
    pub client_id: String,
    pub scopes: BTreeSet<String>,
    pub native_session_id: String,
    pub session_key: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CodeRecord {
    pub client_id: String,
    pub account_id: String,
    pub redirect_uri: String,
    pub scope: String,
    /// oxide-auth's encoded PKCE challenge.
    pub pkce: Option<String>,
    pub nonce: Option<String>,
    pub auth_time: i64,
    pub grant_id: String,
    pub session_key: String,
    pub native_session_id: String,
    pub expires_at: i64,
    pub consumed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TokenKind {
    Access,
    Refresh,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TokenRecord {
    pub kind: TokenKind,
    pub client_id: String,
    pub account_id: String,
    pub scope: String,
    pub grant_id: String,
    pub session_key: String,
    pub native_session_id: String,
    pub auth_time: i64,
    pub issued_at: i64,
    pub expires_at: i64,
    /// A rotated refresh token; presenting it again revokes the grant.
    pub consumed: bool,
}

pub enum CodeUse {
    Fresh(CodeRecord),
    /// Already redeemed: the caller revokes what the grant issued.
    Replayed(CodeRecord),
    Missing,
}

/// What a revocation applies to.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Revoke {
    Grant(String),
    /// A provider session (by key) and the grants made in it.
    Session(String),
    /// Native logout, password change or session expiry.
    NativeSession(String),
    /// Suspension, deletion or password recovery.
    Account(String),
    /// A client was disabled or deleted.
    Client(String),
}

/// Storage for provider records. Every method is one transaction.
pub trait Store: Send + 'static {
    fn session(&self, key: &str) -> Option<ProviderSession>;
    fn save_session(&mut self, key: &str, session: ProviderSession);
    fn interaction(&self, key: &str) -> Option<Interaction>;
    fn save_interaction(&mut self, key: &str, interaction: Interaction);
    fn delete_interaction(&mut self, key: &str);
    fn grant(&self, id: &str) -> Option<GrantRecord>;
    fn save_grant(&mut self, grant: GrantRecord);
    fn code(&self, key: &str) -> Option<CodeRecord>;
    fn save_code(&mut self, key: &str, code: CodeRecord);
    /// Mark a code used, atomically.
    fn consume_code(&mut self, key: &str) -> CodeUse;
    fn token(&self, key: &str) -> Option<TokenRecord>;
    fn save_token(&mut self, key: &str, token: TokenRecord);
    fn delete_token(&mut self, key: &str);
    fn revoke(&mut self, what: Revoke);
    /// Remove records that expired before `now` (milliseconds).
    fn cleanup(&mut self, now: i64);
}

/// The in-memory store used by tests and the demo server.
#[derive(Default)]
pub struct MemoryStore {
    sessions: HashMap<String, ProviderSession>,
    interactions: HashMap<String, Interaction>,
    grants: HashMap<String, GrantRecord>,
    codes: HashMap<String, CodeRecord>,
    tokens: HashMap<String, TokenRecord>,
}

impl MemoryStore {
    fn revoke_grants(&mut self, matches: impl Fn(&GrantRecord) -> bool) {
        let ids: BTreeSet<String> = self.grants.values().filter(|g| matches(g)).map(|g| g.id.clone()).collect();
        self.grants.retain(|id, _| !ids.contains(id));
        self.tokens.retain(|_, t| !ids.contains(&t.grant_id));
        self.codes.retain(|_, c| !ids.contains(&c.grant_id));
        for session in self.sessions.values_mut() {
            session.grants.retain(|_, id| !ids.contains(id));
        }
    }

    pub fn counts(&self) -> (usize, usize, usize, usize) {
        (self.sessions.len(), self.grants.len(), self.codes.len(), self.tokens.len())
    }
}

impl Store for MemoryStore {
    fn session(&self, key: &str) -> Option<ProviderSession> {
        self.sessions.get(key).filter(|s| s.expires_at > crate::now()).cloned()
    }
    fn save_session(&mut self, key: &str, session: ProviderSession) {
        self.sessions.insert(key.to_owned(), session);
    }
    fn interaction(&self, key: &str) -> Option<Interaction> {
        self.interactions.get(key).filter(|i| i.expires_at > crate::now()).cloned()
    }
    fn save_interaction(&mut self, key: &str, interaction: Interaction) {
        self.interactions.insert(key.to_owned(), interaction);
    }
    fn delete_interaction(&mut self, key: &str) {
        self.interactions.remove(key);
    }
    fn grant(&self, id: &str) -> Option<GrantRecord> {
        self.grants.get(id).filter(|g| g.expires_at > crate::now()).cloned()
    }
    fn save_grant(&mut self, grant: GrantRecord) {
        self.grants.insert(grant.id.clone(), grant);
    }
    fn code(&self, key: &str) -> Option<CodeRecord> {
        self.codes.get(key).cloned()
    }
    fn save_code(&mut self, key: &str, code: CodeRecord) {
        self.codes.insert(key.to_owned(), code);
    }
    fn consume_code(&mut self, key: &str) -> CodeUse {
        match self.codes.get_mut(key) {
            None => CodeUse::Missing,
            Some(code) if code.consumed => CodeUse::Replayed(code.clone()),
            Some(code) => {
                code.consumed = true;
                CodeUse::Fresh(code.clone())
            }
        }
    }
    fn token(&self, key: &str) -> Option<TokenRecord> {
        self.tokens.get(key).filter(|t| t.expires_at > crate::now()).cloned()
    }
    fn save_token(&mut self, key: &str, token: TokenRecord) {
        self.tokens.insert(key.to_owned(), token);
    }
    fn delete_token(&mut self, key: &str) {
        self.tokens.remove(key);
    }
    fn revoke(&mut self, what: Revoke) {
        match what {
            Revoke::Grant(id) => self.revoke_grants(|g| g.id == id),
            Revoke::Session(key) => {
                self.sessions.remove(&key);
                self.revoke_grants(|g| g.session_key == key);
            }
            Revoke::NativeSession(id) => {
                self.sessions.retain(|_, s| s.native_session_id != id);
                self.revoke_grants(|g| g.native_session_id == id);
            }
            Revoke::Account(id) => {
                self.sessions.retain(|_, s| s.account_id != id);
                self.revoke_grants(|g| g.account_id == id);
            }
            Revoke::Client(id) => self.revoke_grants(|g| g.client_id == id),
        }
    }
    fn cleanup(&mut self, now: i64) {
        self.sessions.retain(|_, s| s.expires_at > now);
        self.interactions.retain(|_, i| i.expires_at > now);
        self.codes.retain(|_, c| c.expires_at > now);
        self.tokens.retain(|_, t| t.expires_at > now);
        self.revoke_grants(|g| g.expires_at <= now);
    }
}
