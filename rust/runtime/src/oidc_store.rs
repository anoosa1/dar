//! The OIDC provider's records in the database (`runtime/identity/store.ts`): each is a
//! `provider_models` row named like Dar's adapter models (`<Model>/<SHA-256>`), sealed with
//! the "provider-model" box, carrying the account, client, grant and native session it belongs
//! to. The module expires, consumes and revokes rows; logout, password changes, suspension and
//! client changes revoke there in the same transaction.
//!
//! The provider's `Store` is synchronous; calls block the current worker briefly, as directory
//! lookups do.

use dar_oidc::store::{CodeRecord, CodeUse, GrantRecord, Interaction, InteractionResult, ProviderSession, TokenKind, TokenRecord};
use dar_oidc::{Revoke, Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::auth::KeyBox;
use crate::db::Database;

pub struct DbStore {
    db: Database,
    seal: KeyBox,
}

fn hex(value: &str) -> String {
    Sha256::digest(value.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// `modelKey(model, id)`: the row ID for a record known by its plain ID.
fn model_key(model: &str, id: &str) -> String {
    format!("{model}/{}", hex(id))
}

/// The row ID for a record the provider already keys by the SHA-256 of its secret.
fn hashed_key(model: &str, key: &str) -> String {
    format!("{model}/{key}")
}

#[derive(Default)]
struct Row<'a> {
    account_id: &'a str,
    client_id: &'a str,
    grant_key: String,
    session_key: String,
    uid_key: String,
    native_session_id: &'a str,
    /// Milliseconds.
    expires_at: i64,
    consumed: i64,
}

impl DbStore {
    pub fn new(db: Database, seal: KeyBox) -> Self {
        DbStore { db, seal }
    }

    fn call(&self, operation: &str, payload: Value) -> Option<Value> {
        let db = self.db.clone();
        let operation = operation.to_owned();
        let shown = operation.clone();
        let result = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(async move { db.call("service_provider", json!([operation, payload.to_string()])).await }));
        // A refused write (an expired grant, a signed-out session, a disabled client) is expected;
        // anything else is worth a line in the log.
        if let Err(e) = &result
            && !matches!(e.code.as_str(), "conflict" | "unauthenticated" | "forbidden")
        {
            eprintln!("Provider storage {shown} failed: {e}");
        }
        result.ok()
    }

    fn find<T: serde::de::DeserializeOwned>(&self, id: &str) -> Option<(T, Value)> {
        let row = self.call("find", json!({ "id": id })).filter(|r| !r.is_null())?;
        let value = self.seal.decrypt(row["encrypted"].as_str()?)?;
        Some((serde_json::from_value(value).ok()?, row))
    }

    fn upsert<T: serde::Serialize>(&self, id: &str, model: &str, record: &T, row: Row<'_>) {
        let expires_at = row.expires_at.max(0);
        let payload = json!({
            "id": id, "model": model, "encrypted": self.seal.encrypt(&serde_json::to_value(record).expect("record")),
            "accountId": row.account_id, "clientId": row.client_id, "grantKey": row.grant_key, "sessionKey": row.session_key,
            "uidKey": row.uid_key, "userCodeKey": "", "nativeSessionId": row.native_session_id,
            "expiresAt": expires_at, "expiryKey": format!("{expires_at:016}"), "consumed": row.consumed,
        });
        self.call("upsert", payload);
    }
}

fn token_model(kind: TokenKind) -> &'static str {
    match kind {
        TokenKind::Access => "AccessToken",
        TokenKind::Refresh => "RefreshToken",
    }
}

impl Store for DbStore {
    fn session(&self, key: &str) -> Option<ProviderSession> {
        self.find(&hashed_key("Session", key)).map(|(s, _)| s)
    }

    fn save_session(&mut self, key: &str, session: ProviderSession) {
        let row = Row { account_id: &session.account_id, native_session_id: &session.native_session_id, expires_at: session.expires_at, ..Default::default() };
        self.upsert(&hashed_key("Session", key), "Session", &session, row);
    }

    fn interaction(&self, key: &str) -> Option<Interaction> {
        self.find(&hashed_key("Interaction", key)).map(|(i, _)| i)
    }

    fn save_interaction(&mut self, key: &str, interaction: Interaction) {
        let account = match &interaction.result {
            Some(InteractionResult::Login { account_id, .. }) => account_id.clone(),
            _ => interaction.account_id.clone().unwrap_or_default(),
        };
        let native = match &interaction.result {
            Some(InteractionResult::Login { native_session_id, .. }) => native_session_id.clone(),
            _ => String::new(),
        };
        // Permission authority stays with grant and token records, not the pending interaction.
        let row = Row { account_id: if native.is_empty() { "" } else { &account }, client_id: &interaction.client_id, native_session_id: &native, uid_key: hashed_key("uid", key), expires_at: interaction.expires_at, ..Default::default() };
        self.upsert(&hashed_key("Interaction", key), "Interaction", &interaction, row);
    }

    fn delete_interaction(&mut self, key: &str) {
        self.call("destroy", json!({ "id": hashed_key("Interaction", key) }));
    }

    fn grant(&self, id: &str) -> Option<GrantRecord> {
        self.find(&model_key("Grant", id)).map(|(g, _)| g)
    }

    fn save_grant(&mut self, grant: GrantRecord) {
        let row = Row { account_id: &grant.account_id, client_id: &grant.client_id, native_session_id: &grant.native_session_id, session_key: grant.session_key.clone(), expires_at: grant.expires_at, ..Default::default() };
        self.upsert(&model_key("Grant", &grant.id), "Grant", &grant, row);
    }

    fn code(&self, key: &str) -> Option<CodeRecord> {
        self.find::<CodeRecord>(&hashed_key("AuthorizationCode", key)).map(|(mut code, row)| {
            if row["consumed"].as_f64().unwrap_or(0.0) > 0.0 {
                code.consumed = true;
            }
            code
        })
    }

    fn save_code(&mut self, key: &str, code: CodeRecord) {
        let row = Row { account_id: &code.account_id, client_id: &code.client_id, native_session_id: &code.native_session_id, grant_key: model_key("Grant", &code.grant_id), session_key: code.session_key.clone(), expires_at: code.expires_at, ..Default::default() };
        self.upsert(&hashed_key("AuthorizationCode", key), "AuthorizationCode", &code, row);
    }

    fn consume_code(&mut self, key: &str) -> CodeUse {
        let id = hashed_key("AuthorizationCode", key);
        if self.call("consume", json!({ "id": id })).is_some() {
            return match self.find::<CodeRecord>(&id) {
                Some((code, _)) => CodeUse::Fresh(code),
                None => CodeUse::Missing,
            };
        }
        match self.find::<CodeRecord>(&id) {
            Some((mut code, row)) if row["consumed"].as_f64().unwrap_or(0.0) > 0.0 => {
                code.consumed = true;
                CodeUse::Replayed(code)
            }
            _ => CodeUse::Missing,
        }
    }

    fn token(&self, key: &str) -> Option<TokenRecord> {
        for model in ["AccessToken", "RefreshToken"] {
            if let Some((mut token, row)) = self.find::<TokenRecord>(&hashed_key(model, key)) {
                if row["consumed"].as_f64().unwrap_or(0.0) > 0.0 {
                    token.consumed = true;
                }
                return Some(token);
            }
        }
        None
    }

    fn save_token(&mut self, key: &str, token: TokenRecord) {
        let row = Row {
            account_id: &token.account_id,
            client_id: &token.client_id,
            native_session_id: &token.native_session_id,
            grant_key: model_key("Grant", &token.grant_id),
            session_key: token.session_key.clone(),
            expires_at: token.expires_at,
            consumed: if token.consumed { chrono::Utc::now().timestamp() } else { 0 },
            ..Default::default()
        };
        self.upsert(&hashed_key(token_model(token.kind), key), token_model(token.kind), &token, row);
    }

    fn delete_token(&mut self, key: &str) {
        for model in ["AccessToken", "RefreshToken"] {
            self.call("destroy", json!({ "id": hashed_key(model, key) }));
        }
    }

    fn revoke(&mut self, what: Revoke) {
        match what {
            Revoke::Grant(id) => {
                self.call("revokeGrant", json!({ "id": model_key("Grant", &id) }));
            }
            Revoke::Session(key) => {
                if let Some(session) = self.session(&key) {
                    for grant in session.grants.values() {
                        self.call("revokeGrant", json!({ "id": model_key("Grant", grant) }));
                    }
                }
                self.call("destroy", json!({ "id": hashed_key("Session", &key) }));
            }
            // The module revokes these itself, in the transaction that signs out, suspends or
            // changes the client (`revoke_provider`).
            Revoke::NativeSession(_) | Revoke::Account(_) | Revoke::Client(_) => {}
        }
    }

    fn cleanup(&mut self, _now: i64) {
        self.call("cleanup", json!({}));
    }
}
