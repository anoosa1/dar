//! Session security (`auth-security.integration.test.ts`): password changes, suspension and
//! scheduled expiry revoke active sessions; tokens for another audience are rejected.

use dar_acceptance::prelude::*;
use std::time::Duration;

pub const SUITE: Suite = Suite { name: "auth-security", cases: &[case!(revocation_and_audiences, 30)] };

/// Resolves when this connection's `me` row is deleted.
fn me_deleted(live: &Live) -> tokio::sync::oneshot::Receiver<()> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let sender = std::sync::Mutex::new(Some(sender));
    live.db.me().on_delete(move |_, _| if let Some(sender) = sender.lock().unwrap().take() { let _ = sender.send(()); });
    receiver
}

pub async fn revocation_and_audiences(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let base = instance.base.clone();
    let admin = login(&base).await?;
    let username = format!("security-{}", &uuid()[..8]);
    let created = admin.post("/api/v1/auth/users", &json!({"username": username, "displayName": "Security proof", "password": "initial security proof password"})).await?.expect(201)?;
    let id = created.json()?["id"].as_str().context("user id")?.to_owned();
    let first = login_as(&base, &username, "initial security proof password").await?;
    let second = login_as(&base, &username, "initial security proof password").await?;
    let (a, b) = (first.connect().await?, second.connect().await?);
    let owner = instance.database()?;
    let jwk = support::signing_jwk(&instance.root().join("secrets"))?;
    let original = support::jwt_claims(&first.token)?;
    let now = chrono::Utc::now().timestamp();
    let invalid = support::sign_es256(&jwk, &json!({"sid": original["sid"], "iss": original["iss"], "aud": "unrelated-application", "sub": original["sub"], "iat": now, "exp": now + 300}))?;
    assert!(Session { token: invalid, ..first.clone() }.connect().await.is_err());
    let removed = (me_deleted(&a), me_deleted(&b));
    first.post("/api/v1/auth/password", &json!({"currentPassword": "initial security proof password", "password": "replacement security proof password"})).await?.expect(200)?;
    let (one, two) = tokio::join!(removed.0, removed.1);
    one.ok();
    two.ok();
    assert_eq!(second.get("/api/v1/auth/session").await?.status, 401);
    let active = login_as(&base, &username, "replacement security proof password").await?;
    let c = active.connect().await?;
    let revoked = me_deleted(&c);
    owner.call("service_set_enabled", json!([id, false])).await?;
    tokio::time::timeout(Duration::from_secs(10), revoked).await.context("suspension did not revoke the session")?.ok();
    assert_eq!(active.get("/api/v1/auth/session").await?.status, 401);
    owner.call("service_set_enabled", json!([id, true])).await?;
    assert_eq!(active.get("/api/v1/auth/session").await?.status, 401);
    login_as(&base, &username, "replacement security proof password").await?;
    drop(c);
    let account = owner.call("service_account", json!([username])).await?;
    let expiring_id = uuid();
    let expires_at = chrono::Utc::now().timestamp_millis() + 5000;
    owner.call("service_create_session", json!([expiring_id, id, format!("expiry:{expiring_id}"), "", expires_at, account["verifier"]])).await?;
    let token = support::sign_es256(&jwk, &json!({"sid": expiring_id, "iss": original["iss"], "aud": "dar-browser", "sub": format!("{expiring_id}:{}", uuid()), "iat": now, "exp": now + 300}))?;
    let expiring = Session { token, ..first.clone() }.connect().await?;
    assert_eq!(expiring.db.me().count(), 1);
    tokio::time::timeout(Duration::from_secs(8), me_deleted(&expiring)).await.context("scheduled session expiry did not revoke the live view")?.ok();
    assert_eq!(expiring.db.me().count(), 0);
    assert!(reducer(|done| expiring.reducers.create_folder_then(uuid(), String::new(), "Expired session must not write".into(), done)).await.is_err());
    Ok(())
}
