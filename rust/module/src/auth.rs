//! Accounts, browser and protocol sessions, principals and app passwords
//! (`next/module/src/index.ts`, identity parts).

use serde_json::json;
use spacetimedb::{Identity, ProcedureContext, ReducerContext, ScheduleAt, SpacetimeType, Table, Timestamp, ViewContext};

use crate::scopes::{password_allows, valid_password_scope};
use crate::tables::*;
use crate::{EnvAccess, Res, active_user, fail, from_session, now, opaque_id, run, service, unused_uuid, user_for_view};

/// A browser connects with Dar's short-lived token for one session and tab; anything else
/// except the service identity is refused.
#[spacetimedb::reducer(client_connected)]
pub fn connected(ctx: &ReducerContext) -> Res<()> {
    let config = ctx.db.instance().id().find("instance".to_owned());
    if config.as_ref().is_some_and(|c| c.service == ctx.sender()) {
        return Ok(());
    }
    let auth = ctx.sender_auth();
    let Some(jwt) = auth.jwt() else { return fail("unauthenticated") };
    if config.is_none_or(|c| jwt.issuer() != c.issuer) || !jwt.audience().iter().any(|a| a == "dar-browser") {
        return fail("unauthenticated");
    }
    let payload: serde_json::Value = serde_json::from_str(jwt.raw_payload()).map_err(|_| "unauthenticated")?;
    let Some(session_id) = payload["sid"].as_str() else { return fail("unauthenticated") };
    if !jwt.subject().starts_with(&format!("{session_id}:")) {
        return fail("unauthenticated");
    }
    from_session(ctx, session_id)?;
    if ctx.db.principals().identity().find(ctx.sender()).is_none() {
        ctx.db.principals().insert(PrincipalRow { identity: ctx.sender(), session_id: session_id.into() });
    }
    Ok(())
}

#[spacetimedb::reducer]
pub fn expire_session(ctx: &ReducerContext, expiry: SessionExpiry) -> Res<()> {
    if !ctx.sender_auth().is_internal() {
        return fail("forbidden");
    }
    if let Some(session) = ctx.db.sessions().id().find(expiry.session_id).filter(|s| s.expires_at <= now(ctx)) {
        let id = session.id.clone();
        ctx.db.sessions().id().update(Session { active: false, ..session });
        crate::identity::revoke_provider(ctx, None, Some(&id), None);
    }
    Ok(())
}

#[spacetimedb::procedure]
pub fn service_status(ctx: &mut ProcedureContext) -> String {
    run(ctx, |tx| {
        service(tx)?;
        Ok(json!({ "initialized": tx.db.accounts().count() > 0 }).to_string())
    })
}

/// Start-up check: a changed issuer invalidates every session and app password.
#[spacetimedb::reducer]
pub fn service_startup(ctx: &ReducerContext, issuer: String) -> Res<()> {
    service(ctx)?;
    if ctx.db.identity_migrations().id().find(1).is_none() {
        ctx.db.identity_migrations().insert(IdentityMigration { id: 1, applied_at: now(ctx) });
    }
    if ctx.db.workspace_migrations().id().find(1).is_none() {
        ctx.db.workspace_migrations().insert(WorkspaceMigration { id: 1, applied_at: now(ctx) });
    }
    if issuer != ctx.env.DAR_ISSUER() {
        return fail("invalidInput");
    }
    let config = ctx.db.instance().id().find("instance".to_owned()).ok_or("forbidden")?;
    if config.issuer == issuer {
        return Ok(());
    }
    ctx.db.instance().id().update(Instance { issuer, ..config });
    crate::identity::revoke_provider(ctx, None, None, None);
    for session in ctx.db.sessions().iter().filter(|s| s.active) {
        ctx.db.sessions().id().update(Session { active: false, ..session });
    }
    for password in ctx.db.app_passwords().iter().filter(|p| !p.revoked) {
        ctx.db.app_passwords().id().update(AppPassword { revoked: true, ..password });
    }
    Ok(())
}

fn account_json(a: &Account) -> serde_json::Value {
    json!({ "id": a.id, "username": a.username, "displayName": a.display_name, "verifier": a.verifier, "administrator": a.administrator, "enabled": a.enabled, "createdAt": a.created_at })
}

#[spacetimedb::procedure]
pub fn service_account(ctx: &mut ProcedureContext, username: String) -> String {
    run(ctx, |tx| {
        service(tx)?;
        Ok(tx.db.accounts().username().find(username.clone()).map_or(serde_json::Value::Null, |a| account_json(&a)).to_string())
    })
}

fn valid_username(name: &str) -> bool {
    let bytes = name.as_bytes();
    (3..=64).contains(&bytes.len())
        && matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9')
        && bytes.iter().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-'))
}

#[spacetimedb::reducer]
pub fn service_create_account(ctx: &ReducerContext, id: String, username: String, displayName: String, verifier: String, first: bool) -> Res<()> {
    service(ctx)?;
    let display_name = displayName;
    let count = ctx.db.accounts().count();
    if first != (count == 0) {
        return fail("conflict");
    }
    opaque_id(&id)?;
    let name_length = display_name.encode_utf16().count();
    if !valid_username(&username) || !(1..=100).contains(&name_length) {
        return fail("invalidInput");
    }
    if ctx.db.accounts().username().find(username.clone()).is_some() {
        return fail("conflict");
    }
    if count >= 10_000 {
        return fail("operationTooLarge");
    }
    ctx.db.accounts().insert(Account { id: id.clone(), username, display_name, verifier, administrator: first, enabled: true, created_at: now(ctx) });
    ctx.db.usage().insert(Usage { user_id: id, used: 0.0, reserved: 0.0, quota: 0.0 });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[spacetimedb::reducer]
pub fn service_create_session(ctx: &ReducerContext, id: String, userId: String, digest: String, csrfDigest: String, expiresAt: f64, expectedVerifier: String) -> Res<()> {
    service(ctx)?;
    let (user_id, csrf_digest, expires_at, expected_verifier) = (userId, csrfDigest, expiresAt, expectedVerifier);
    match ctx.db.accounts().id().find(user_id.clone()) {
        Some(a) if a.enabled && a.verifier == expected_verifier => {}
        _ => return fail("unauthenticated"),
    }
    let at = now(ctx);
    if !expires_at.is_finite() || expires_at <= at || expires_at > at + 7.0 * 86_400_000.0 {
        return fail("invalidInput");
    }
    ctx.db.sessions().insert(Session { id: id.clone(), user_id, digest, csrf_digest, expires_at, active: true, created_at: at });
    let when = Timestamp::from_micros_since_unix_epoch(expires_at.ceil() as i64 * 1000);
    ctx.db.session_expiry().insert(SessionExpiry { scheduled_id: 0, scheduled_at: ScheduleAt::Time(when), session_id: id });
    Ok(())
}

#[spacetimedb::procedure]
pub fn service_session(ctx: &mut ProcedureContext, digest: String) -> String {
    run(ctx, |tx| {
        service(tx)?;
        let session = tx.db.sessions().digest().find(digest.clone());
        let user = session.as_ref().and_then(|s| tx.db.accounts().id().find(s.user_id.clone()));
        let value = match (session, user) {
            (Some(s), Some(u)) if s.active && s.expires_at > now(tx) && u.enabled => json!({
                "session": { "id": s.id, "userId": s.user_id, "digest": s.digest, "csrfDigest": s.csrf_digest, "expiresAt": s.expires_at, "active": s.active, "createdAt": s.created_at },
                "user": { "id": u.id, "username": u.username, "displayName": u.display_name, "administrator": u.administrator },
            }),
            _ => serde_json::Value::Null,
        };
        Ok(value.to_string())
    })
}

#[spacetimedb::reducer]
pub fn service_revoke_session(ctx: &ReducerContext, id: String) -> Res<()> {
    service(ctx)?;
    if let Some(session) = ctx.db.sessions().id().find(id.clone()) {
        ctx.db.sessions().id().update(Session { active: false, ..session });
    }
    crate::identity::revoke_provider(ctx, None, Some(&id), None);
    Ok(())
}

/// Every session and app password of an account (`revokeUser`).
pub fn revoke_user(ctx: &ReducerContext, user_id: &str) {
    crate::identity::revoke_user(ctx, user_id);
}

#[spacetimedb::reducer]
pub fn service_change_password(ctx: &ReducerContext, userId: String, verifier: String, expectedVerifier: String) -> Res<()> {
    service(ctx)?;
    let (user_id, expected_verifier) = (userId, expectedVerifier);
    let user = ctx.db.accounts().id().find(user_id.clone()).ok_or("notFound")?;
    if user.verifier != expected_verifier {
        return fail("conflict");
    }
    ctx.db.accounts().id().update(Account { verifier, ..user });
    crate::identity::touch_profile(ctx, &user_id);
    revoke_user(ctx, &user_id);
    Ok(())
}

#[spacetimedb::reducer]
pub fn service_set_enabled(ctx: &ReducerContext, userId: String, enabled: bool) -> Res<()> {
    service(ctx)?;
    let user_id = userId;
    let user = ctx.db.accounts().id().find(user_id.clone()).filter(|u| !u.administrator).ok_or("forbidden")?;
    if enabled && ctx.db.directory_profiles().user_id().find(user_id.clone()).is_some_and(|p| p.deleted) {
        return fail("notFound");
    }
    let changed = user.enabled != enabled;
    ctx.db.accounts().id().update(Account { enabled, ..user });
    if changed {
        crate::identity::touch_profile(ctx, &user_id);
    }
    if !enabled {
        revoke_user(ctx, &user_id);
    }
    Ok(())
}

#[spacetimedb::reducer]
pub fn service_revoke_owned_session(ctx: &ReducerContext, sessionId: String, id: String) -> Res<()> {
    service(ctx)?;
    let user = from_session(ctx, &sessionId)?;
    let session = ctx.db.sessions().id().find(id.clone()).filter(|s| s.user_id == user.id).ok_or("notFound")?;
    ctx.db.sessions().id().update(Session { active: false, ..session });
    crate::identity::revoke_provider(ctx, None, Some(&id), None);
    Ok(())
}

#[derive(SpacetimeType)]
pub struct SafeUser {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub administrator: bool,
}

#[spacetimedb::view(accessor = me, public)]
pub fn me(ctx: &ViewContext) -> Option<SafeUser> {
    user_for_view(ctx).map(|u| SafeUser { id: u.id, username: u.username, display_name: u.display_name, administrator: u.administrator })
}

fn identity_from_uuid(uuid: &str) -> Identity {
    crate::identity_of(uuid)
}

/// An app password and its protocol session; returns the session ID. The protocol principal
/// cannot be authenticated by a browser token: only the service acts on its behalf.
pub fn insert_app_password(ctx: &ReducerContext, user_id: &str, id: &str, digest: &str, label: &str, scope: &str) -> Res<String> {
    let label = label.trim();
    let hex64 = digest.len() == 64 && digest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if label.is_empty() || label.encode_utf16().count() > 100 || !valid_password_scope(scope) || !hex64 {
        return fail("invalidInput");
    }
    let protocol_session = unused_uuid(ctx);
    let identity = identity_from_uuid(&unused_uuid(ctx));
    let expires_at = now(ctx) + 365.0 * 86_400_000.0;
    ctx.db.sessions().insert(Session { id: protocol_session.clone(), user_id: user_id.into(), digest: format!("protocol:{id}"), csrf_digest: String::new(), expires_at, active: true, created_at: now(ctx) });
    ctx.db.principals().insert(PrincipalRow { identity, session_id: protocol_session.clone() });
    ctx.db.app_passwords().insert(AppPassword { id: id.into(), user_id: user_id.into(), digest: digest.into(), label: label.into(), scope: scope.into(), session_id: protocol_session.clone(), principal: identity, expires_at, revoked: false });
    Ok(protocol_session)
}

#[spacetimedb::reducer]
pub fn service_create_app_password(ctx: &ReducerContext, sessionId: String, id: String, digest: String, label: String, scope: String) -> Res<()> {
    let session_id = sessionId;
    service(ctx)?;
    let user = from_session(ctx, &session_id)?;
    insert_app_password(ctx, &user.id, &id, &digest, &label, &scope).map(|_| ())
}

#[spacetimedb::procedure]
pub fn service_app_passwords(ctx: &mut ProcedureContext, sessionId: String) -> String {
    let session_id = sessionId;
    run(ctx, |tx| {
        service(tx)?;
        let user = from_session(tx, &session_id)?;
        let rows: Vec<_> = tx
            .db
            .app_passwords()
            .user_id()
            .filter(user.id.as_str())
            .filter(|p| !p.revoked)
            .map(|p| json!({ "id": p.id, "label": p.label, "scope": p.scope, "expiresAt": p.expires_at }))
            .collect();
        Ok(serde_json::Value::from(rows).to_string())
    })
}

#[spacetimedb::reducer]
pub fn service_revoke_app_password(ctx: &ReducerContext, sessionId: String, id: String) -> Res<()> {
    let session_id = sessionId;
    service(ctx)?;
    let user = from_session(ctx, &session_id)?;
    let password = ctx.db.app_passwords().id().find(id).filter(|p| p.user_id == user.id).ok_or("notFound")?;
    if let Some(session) = ctx.db.sessions().id().find(password.session_id.clone()) {
        ctx.db.sessions().id().update(Session { active: false, ..session });
    }
    ctx.db.app_passwords().id().update(AppPassword { revoked: true, ..password });
    Ok(())
}

/// The live app password with this digest, for `username`.
fn protocol_password(tx: &ReducerContext, username: &str, digest: &str) -> Res<(AppPassword, Account)> {
    let password = tx.db.app_passwords().digest().find(digest.to_owned());
    let user = password.as_ref().and_then(|p| tx.db.accounts().id().find(p.user_id.clone()));
    let session = password.as_ref().and_then(|p| tx.db.sessions().id().find(p.session_id.clone()));
    match (password, user, session) {
        (Some(p), Some(u), Some(s)) if !p.revoked && p.expires_at > now(tx) && u.enabled && u.username == username.to_lowercase() && s.active => Ok((p, u)),
        _ => fail("unauthenticated"),
    }
}

/// Files over WebDAV: returns the protocol session for an app password.
#[spacetimedb::procedure]
pub fn service_protocol_authenticate(ctx: &mut ProcedureContext, username: String, digest: String, write: bool) -> String {
    run(ctx, |tx| {
        service(tx)?;
        let (password, user) = protocol_password(tx, &username, &digest)?;
        if !password_allows(&password.scope, "files", if write { "write" } else { "read" }) {
            return fail("forbidden");
        }
        Ok(json!({ "sessionId": password.session_id, "username": user.username }).to_string())
    })
}

/// Contacts, Calendar and News over their standards protocols.
#[spacetimedb::procedure]
pub fn service_pim_authenticate(ctx: &mut ProcedureContext, username: String, digest: String, app: String, write: bool) -> String {
    run(ctx, |tx| {
        service(tx)?;
        let (password, user) = protocol_password(tx, &username, &digest)?;
        if !["contacts", "calendar", "news"].contains(&app.as_str()) || !password_allows(&password.scope, &app, if write { "write" } else { "read" }) {
            return fail("forbidden");
        }
        Ok(json!({ "sessionId": password.session_id, "username": user.username }).to_string())
    })
}

/// The caller's own account, for reducers browsers call directly.
pub fn caller(ctx: &ReducerContext) -> Res<Account> {
    active_user(ctx)
}
