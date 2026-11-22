//! The identity directory and OIDC provider store (`next/module/src/identity.ts`): profiles,
//! groups, provisioning tokens, registered clients and the provider's models.

use serde_json::{Map, Value, json};
use spacetimedb::{ReducerContext, Table};

use crate::tables::*;
use crate::{Res, fail, now, row};

type Doc = Map<String, Value>;

fn str_of(p: &Doc, key: &str) -> String {
    crate::text_of(p.get(key))
}

/// Ends the provider's grants and tokens for an account, a browser session, a client, or all.
pub fn revoke_provider(ctx: &ReducerContext, user_id: Option<&str>, session_id: Option<&str>, client_id: Option<&str>) {
    let rows: Vec<ProviderModel> = if let Some(u) = user_id {
        ctx.db.provider_models().account_id().filter(u).collect()
    } else if let Some(c) = client_id {
        ctx.db.provider_models().client_id().filter(c).collect()
    } else if let Some(s) = session_id {
        ctx.db.provider_models().native_session_id().filter(s).collect()
    } else {
        ctx.db.provider_models().iter().collect()
    };
    for r in rows {
        if session_id.is_none_or(|s| r.native_session_id == s) {
            ctx.db.provider_models().id().delete(r.id);
        }
    }
}

/// Signs an account out everywhere: sessions, app passwords, provisioning tokens and grants.
pub fn revoke_user(ctx: &ReducerContext, user_id: &str) {
    for session in ctx.db.sessions().user_id().filter(user_id).collect::<Vec<_>>() {
        if session.active {
            ctx.db.sessions().id().update(Session { active: false, ..session });
        }
    }
    for password in ctx.db.app_passwords().user_id().filter(user_id).collect::<Vec<_>>() {
        if !password.revoked {
            ctx.db.app_passwords().id().update(AppPassword { revoked: true, ..password });
        }
    }
    for token in ctx.db.provisioning_tokens().owner_id().filter(user_id).collect::<Vec<_>>() {
        if !token.revoked {
            ctx.db.provisioning_tokens().id().update(ProvisioningToken { revoked: true, ..token });
        }
    }
    revoke_provider(ctx, Some(user_id), None, None);
}

#[derive(Clone)]
pub struct Actor {
    pub id: String,
    pub scim: bool,
}

/// A SCIM bearer (provisioning token) or an administrator's browser session; writes from a
/// session need a fresh password proof.
pub fn identity_actor(ctx: &ReducerContext, session_id: &str, bearer: &str, proof: &str, write: bool) -> Res<Actor> {
    if !bearer.is_empty() {
        let token = ctx.db.provisioning_tokens().digest().find(bearer.to_owned());
        let owner = token.as_ref().and_then(|t| ctx.db.accounts().id().find(t.owner_id.clone()));
        let Some(token) = token.filter(|t| !t.revoked && t.expires_at > now(ctx)) else { return fail("unauthenticated") };
        let Some(owner) = owner.filter(|o| o.enabled && o.administrator) else { return fail("unauthenticated") };
        if write && token.scope != "write" {
            return fail("forbidden");
        }
        return Ok(Actor { id: owner.id, scim: true });
    }
    let session = ctx.db.sessions().id().find(session_id.to_owned());
    let account = session.as_ref().and_then(|s| ctx.db.accounts().id().find(s.user_id.clone()));
    let (Some(session), Some(account)) = (session, account) else { return fail("unauthenticated") };
    if !session.active || session.expires_at <= now(ctx) || !account.enabled {
        return fail("unauthenticated");
    }
    if !account.administrator {
        return fail("forbidden");
    }
    if write {
        let ok = ctx.db.identity_proofs().digest().find(proof.to_owned()).is_some_and(|p| p.session_id == session_id && p.expires_at > now(ctx) && p.verifier == account.verifier);
        if !ok {
            return fail("forbidden");
        }
    }
    Ok(Actor { id: account.id, scim: false })
}

fn parse_doc(text: &str) -> Doc {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// An account as SCIM sees it, or `None` once deleted.
pub fn profile(ctx: &ReducerContext, user_id: &str) -> Option<Value> {
    let a = ctx.db.accounts().id().find(user_id.to_owned())?;
    let p = ctx.db.directory_profiles().user_id().find(user_id.to_owned());
    if p.as_ref().is_some_and(|p| p.deleted) {
        return None;
    }
    let mut data = parse_doc(p.as_ref().map_or("{}", |p| p.document.as_str()));
    let groups: Vec<Value> = ctx
        .db
        .directory_members()
        .user_id()
        .filter(&a.id)
        .map(|m| {
            let display = ctx.db.directory_groups().id().find(m.group_id.clone()).map(|g| parse_doc(&g.document).get("displayName").cloned().unwrap_or(Value::Null));
            let mut item = json!({ "value": m.group_id });
            if let Some(d) = display.filter(|d| !d.is_null()) {
                item["display"] = d;
            }
            item
        })
        .collect();
    for (k, v) in [
        ("id", json!(a.id)),
        ("userName", json!(a.username)),
        ("displayName", json!(a.display_name)),
        ("active", json!(a.enabled)),
        ("administrator", json!(a.administrator)),
        ("revision", json!(p.as_ref().map_or(1, |p| p.revision))),
        ("createdAt", json!(a.created_at)),
        ("modifiedAt", json!(p.as_ref().map_or(a.created_at, |p| p.modified_at))),
        ("groups", Value::Array(groups)),
    ] {
        data.insert(k.into(), v);
    }
    Some(Value::Object(data))
}

fn expected(actual: u32, value: Option<&Value>) -> Res<()> {
    if value.and_then(Value::as_f64) == Some(actual as f64) { Ok(()) } else { fail("conflict") }
}

/// A profile changed (SCIM clients see a new revision).
pub fn touch_profile(ctx: &ReducerContext, id: &str) {
    let old = ctx.db.directory_profiles().user_id().find(id.to_owned());
    let next = DirectoryProfile {
        user_id: id.into(),
        document: old.as_ref().map_or("{}".into(), |o| o.document.clone()),
        deleted: old.as_ref().is_some_and(|o| o.deleted),
        revision: old.as_ref().map_or(1, |o| o.revision) + 1,
        modified_at: now(ctx),
    };
    if old.is_some() {
        ctx.db.directory_profiles().user_id().update(next);
    } else {
        ctx.db.directory_profiles().insert(next);
    }
}

fn valid_username(name: &str) -> bool {
    let b = name.as_bytes();
    (3..=64).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'.' | b'-'))
}

fn group(ctx: &ReducerContext, id: &str) -> Res<Value> {
    let Some(g) = ctx.db.directory_groups().id().find(id.to_owned()) else { return fail("notFound") };
    let mut data = parse_doc(&g.document);
    let members: Vec<Value> = ctx
        .db
        .directory_members()
        .group_id()
        .filter(id)
        .map(|m| {
            let mut item = json!({ "value": m.user_id });
            if let Some(a) = ctx.db.accounts().id().find(m.user_id.clone()) {
                item["display"] = json!(a.display_name);
            }
            item
        })
        .collect();
    for (k, v) in [("id", json!(id)), ("revision", json!(g.revision)), ("createdAt", json!(g.created_at)), ("modifiedAt", json!(g.modified_at)), ("members", Value::Array(members))] {
        data.insert(k.into(), v);
    }
    Ok(Value::Object(data))
}

pub fn directory_operation(ctx: &ReducerContext, operation: &str, p: &Doc, actor: &Actor) -> Res<Value> {
    match operation {
        "authorize" | "authorizeWrite" => return Ok(json!({ "ok": true })),
        "users" => return Ok(Value::Array(ctx.db.accounts().iter().filter_map(|a| profile(ctx, &a.id)).collect())),
        "user" => return profile(ctx, &str_of(p, "id")).ok_or_else(|| "notFound".to_owned()),
        "groups" => return ctx.db.directory_groups().iter().map(|g| group(ctx, &g.id)).collect::<Res<Vec<_>>>().map(Value::Array),
        "group" => return group(ctx, &str_of(p, "id")),
        "saveUser" => {
            let id = str_of(p, "id");
            let old = ctx.db.accounts().id().find(id.clone());
            let previous = ctx.db.directory_profiles().user_id().find(id.clone());
            if previous.as_ref().is_some_and(|p| p.deleted) {
                return fail("notFound");
            }
            expected(if old.is_some() { previous.as_ref().map_or(1, |p| p.revision) } else { 0 }, p.get("revision"))?;
            let document = parse_doc(&str_of(p, "document"));
            let username = crate::text_of(document.get("userName")).to_lowercase();
            let active = document.get("active") != Some(&Value::Bool(false));
            if let Some(old) = &old
                && old.administrator
                && (username != old.username || document.get("active") != Some(&Value::Bool(old.enabled)) || p.contains_key("verifier"))
            {
                return fail("forbidden");
            }
            let display = crate::text_of(document.get("displayName"));
            if !valid_username(&username) || display.trim().is_empty() || display.encode_utf16().count() > 100 {
                return fail("invalidInput");
            }
            if ctx.db.accounts().username().find(username.clone()).is_some_and(|d| d.id != id) {
                return fail("conflict");
            }
            if old.is_none() && ctx.db.accounts().count() >= 10000 {
                return fail("operationTooLarge");
            }
            let verifier = match p.get("verifier") {
                Some(Value::String(v)) => v.clone(),
                _ => old.as_ref().map(|o| o.verifier.clone()).unwrap_or_default(),
            };
            let account = Account {
                id: id.clone(),
                username,
                display_name: display,
                verifier,
                administrator: old.as_ref().is_some_and(|o| o.administrator),
                enabled: active,
                created_at: old.as_ref().map_or(now(ctx), |o| o.created_at),
            };
            let mut attributes = document.clone();
            for k in ["userName", "displayName", "active"] {
                attributes.remove(k);
            }
            let next = DirectoryProfile {
                user_id: id.clone(),
                document: Value::Object(attributes).to_string(),
                revision: previous.as_ref().map_or(if old.is_some() { 1 } else { 0 }, |p| p.revision) + 1,
                modified_at: now(ctx),
                deleted: false,
            };
            if old.is_some() {
                ctx.db.accounts().id().update(account.clone());
            } else {
                ctx.db.accounts().insert(account.clone());
                ctx.db.usage().insert(Usage { user_id: id.clone(), used: 0.0, reserved: 0.0, quota: 0.0 });
            }
            if previous.is_some() {
                ctx.db.directory_profiles().user_id().update(next);
            } else {
                ctx.db.directory_profiles().insert(next);
            }
            if let Some(old) = &old
                && old.display_name != account.display_name
            {
                for m in ctx.db.directory_members().user_id().filter(&id).collect::<Vec<_>>() {
                    if let Some(g) = ctx.db.directory_groups().id().find(m.group_id) {
                        ctx.db.directory_groups().id().update(DirectoryGroup { revision: g.revision + 1, modified_at: now(ctx), ..g });
                    }
                }
            }
            if !account.enabled || old.as_ref().is_some_and(|o| o.verifier != account.verifier) {
                revoke_user(ctx, &id);
            }
            return profile(ctx, &id).ok_or_else(|| "notFound".to_owned());
        }
        "deleteUser" => {
            let a = ctx.db.accounts().id().find(str_of(p, "id"));
            let old = a.as_ref().and_then(|a| ctx.db.directory_profiles().user_id().find(a.id.clone()));
            let Some(a) = a.filter(|_| !old.as_ref().is_some_and(|o| o.deleted)) else { return fail("notFound") };
            if a.administrator {
                return fail("forbidden");
            }
            expected(old.as_ref().map_or(1, |o| o.revision), p.get("revision"))?;
            revoke_user(ctx, &a.id);
            ctx.db.accounts().id().update(Account { enabled: false, ..a.clone() });
            for r in ctx.db.app_data().owner_id().filter(&a.id).collect::<Vec<_>>() {
                ctx.db.app_data().id().delete(r.id);
            }
            for r in ctx.db.agent_messages().owner_id().filter(&a.id).collect::<Vec<_>>() {
                ctx.db.agent_messages().id().delete(r.id);
            }
            for r in ctx.db.agent_runs().owner_id().filter(&a.id).collect::<Vec<_>>() {
                ctx.db.agent_runs().id().delete(r.id);
            }
            for r in ctx.db.agent_threads().owner_id().filter(&a.id).collect::<Vec<_>>() {
                ctx.db.agent_threads().id().delete(r.id);
            }
            for r in ctx.db.agent_projects().owner_id().filter(&a.id).collect::<Vec<_>>() {
                ctx.db.agent_projects().id().delete(r.id);
            }
            crate::news::delete_news_account(ctx, &a.id);
            crate::chat::delete_chat_account(ctx, &a.id);
            crate::photos::delete_photos_account(ctx, &a.id);
            let next = DirectoryProfile {
                user_id: a.id.clone(),
                document: old.as_ref().map_or("{}".into(), |o| o.document.clone()),
                revision: old.as_ref().map_or(1, |o| o.revision) + 1,
                modified_at: now(ctx),
                deleted: true,
            };
            if old.is_some() {
                ctx.db.directory_profiles().user_id().update(next);
            } else {
                ctx.db.directory_profiles().insert(next);
            }
            for member in ctx.db.directory_members().user_id().filter(&a.id).collect::<Vec<_>>() {
                ctx.db.directory_members().id().delete(member.id);
                if let Some(g) = ctx.db.directory_groups().id().find(member.group_id) {
                    ctx.db.directory_groups().id().update(DirectoryGroup { revision: g.revision + 1, modified_at: now(ctx), ..g });
                }
            }
            return Ok(json!({ "ok": true }));
        }
        "saveGroup" => {
            let id = str_of(p, "id");
            let old = ctx.db.directory_groups().id().find(id.clone());
            expected(old.as_ref().map_or(0, |o| o.revision), p.get("revision"))?;
            let data = parse_doc(&str_of(p, "document"));
            let members: Vec<Value> = data.get("members").and_then(Value::as_array).cloned().unwrap_or_default();
            if old.is_none() && ctx.db.directory_groups().count() >= 2000 {
                return fail("operationTooLarge");
            }
            if data.get("displayName").map(|v| crate::text_of(Some(v))).unwrap_or_default().trim().is_empty() || members.len() > 2000 {
                return fail("invalidInput");
            }
            let member_ids: Vec<String> = members.iter().map(|m| crate::text_of(m.get("value"))).collect();
            for member in &member_ids {
                if profile(ctx, member).is_none() {
                    return fail("invalidInput");
                }
            }
            let mut attributes = data.clone();
            attributes.remove("members");
            let next = DirectoryGroup {
                id: id.clone(),
                document: Value::Object(attributes).to_string(),
                revision: old.as_ref().map_or(0, |o| o.revision) + 1,
                created_at: old.as_ref().map_or(now(ctx), |o| o.created_at),
                modified_at: now(ctx),
            };
            if old.is_some() {
                ctx.db.directory_groups().id().update(next);
            } else {
                ctx.db.directory_groups().insert(next);
            }
            let mut affected: Vec<String> = Vec::new();
            for m in &member_ids {
                if !affected.contains(m) {
                    affected.push(m.clone());
                }
            }
            for m in ctx.db.directory_members().group_id().filter(&id).collect::<Vec<_>>() {
                if !affected.contains(&m.user_id) {
                    affected.push(m.user_id.clone());
                }
                ctx.db.directory_members().id().delete(m.id);
            }
            let mut inserted: Vec<&String> = Vec::new();
            for user_id in &member_ids {
                if !inserted.contains(&user_id) {
                    inserted.push(user_id);
                    ctx.db.directory_members().insert(DirectoryMember { id: format!("{id}/{user_id}"), group_id: id.clone(), user_id: user_id.clone() });
                }
            }
            for user_id in affected {
                touch_profile(ctx, &user_id);
            }
            return group(ctx, &id);
        }
        "deleteGroup" => {
            let Some(g) = ctx.db.directory_groups().id().find(str_of(p, "id")) else { return fail("notFound") };
            expected(g.revision, p.get("revision"))?;
            for member in ctx.db.directory_members().group_id().filter(&g.id).collect::<Vec<_>>() {
                touch_profile(ctx, &member.user_id);
                ctx.db.directory_members().id().delete(member.id);
            }
            ctx.db.directory_groups().id().delete(g.id);
            return Ok(json!({ "ok": true }));
        }
        _ => {}
    }
    if actor.scim {
        return fail("forbidden");
    }
    match operation {
        "adminWrite" => Ok(json!({ "ok": true })),
        "storage" => Ok(Value::Array(
            ctx.db
                .accounts()
                .iter()
                .map(|a| {
                    let m = ctx.db.usage().user_id().find(a.id.clone());
                    json!({ "userId": a.id, "used": m.as_ref().map_or(0.0, |m| m.used), "quota": m.as_ref().map_or(0.0, |m| m.quota) })
                })
                .collect(),
        )),
        "setStorageQuota" => {
            let id = str_of(p, "id");
            if ctx.db.accounts().id().find(id.clone()).is_none() || ctx.db.directory_profiles().user_id().find(id.clone()).is_some_and(|d| d.deleted) {
                return fail("notFound");
            }
            let Some(quota) = p.get("quota").and_then(Value::as_f64).filter(|q| *q >= 0.0 && q.fract() == 0.0 && *q <= 9_007_199_254_740_991.0) else { return fail("invalidInput") };
            let m = ctx.db.usage().user_id().find(id.clone());
            match &m {
                Some(m) => {
                    ctx.db.usage().user_id().update(Usage { quota, ..m.clone() });
                }
                None => {
                    ctx.db.usage().insert(Usage { user_id: id.clone(), used: 0.0, reserved: 0.0, quota });
                }
            }
            Ok(json!({ "userId": id, "used": m.map_or(0.0, |m| m.used), "quota": quota }))
        }
        "clients" => Ok(Value::Array(
            ctx.db
                .identity_clients()
                .iter()
                .map(|c| {
                    let mut v = row(&c);
                    v.as_object_mut().expect("row").remove("encrypted");
                    v
                })
                .collect(),
        )),
        "saveClient" => {
            let id = str_of(p, "id");
            let old = ctx.db.identity_clients().id().find(id.clone());
            expected(old.as_ref().map_or(0, |o| o.revision), p.get("revision"))?;
            if old.is_none() && ctx.db.identity_clients().count() >= 200 {
                return fail("operationTooLarge");
            }
            let next = IdentityClient {
                id: id.clone(),
                metadata: str_of(p, "metadata"),
                encrypted: str_of(p, "encrypted"),
                enabled: p.get("enabled") != Some(&Value::Bool(false)),
                revision: old.as_ref().map_or(0, |o| o.revision) + 1,
                modified_at: now(ctx),
            };
            if old.is_some() {
                ctx.db.identity_clients().id().update(next.clone());
                revoke_provider(ctx, None, None, Some(&id));
            } else {
                ctx.db.identity_clients().insert(next.clone());
            }
            let mut v = row(&next);
            v.as_object_mut().expect("row").remove("encrypted");
            Ok(v)
        }
        "deleteClient" => {
            let Some(c) = ctx.db.identity_clients().id().find(str_of(p, "id")) else { return fail("notFound") };
            expected(c.revision, p.get("revision"))?;
            revoke_provider(ctx, None, None, Some(&c.id));
            ctx.db.identity_clients().id().delete(c.id);
            Ok(json!({ "ok": true }))
        }
        "tokens" => Ok(Value::Array(
            ctx.db
                .provisioning_tokens()
                .iter()
                .map(|t| {
                    let mut v = row(&t);
                    v.as_object_mut().expect("row").remove("digest");
                    v
                })
                .collect(),
        )),
        "createToken" => {
            if ctx.db.provisioning_tokens().count() >= 1000 {
                for t in ctx.db.provisioning_tokens().iter().collect::<Vec<_>>() {
                    if t.revoked || t.expires_at <= now(ctx) {
                        ctx.db.provisioning_tokens().id().delete(t.id);
                        if ctx.db.provisioning_tokens().count() < 1000 {
                            break;
                        }
                    }
                }
            }
            let scope = str_of(p, "scope");
            let expires_at = p.get("expiresAt").and_then(Value::as_f64).unwrap_or(f64::NAN);
            if !["read", "write"].contains(&scope.as_str()) || !(expires_at > now(ctx)) || expires_at > now(ctx) + 366.0 * 86_400_000.0 || ctx.db.provisioning_tokens().count() >= 1000 {
                return fail("invalidInput");
            }
            ctx.db.provisioning_tokens().insert(ProvisioningToken {
                id: str_of(p, "id"),
                digest: str_of(p, "digest"),
                owner_id: actor.id.clone(),
                label: str_of(p, "label"),
                scope,
                expires_at,
                revoked: false,
            });
            Ok(json!({ "ok": true }))
        }
        "revokeToken" => {
            let Some(t) = ctx.db.provisioning_tokens().id().find(str_of(p, "id")) else { return fail("notFound") };
            ctx.db.provisioning_tokens().id().update(ProvisioningToken { revoked: true, ..t });
            Ok(json!({ "ok": true }))
        }
        _ => fail("invalidInput"),
    }
}

/// The built-in Immich client is on while Photos is; others while enabled.
fn client_enabled(ctx: &ReducerContext, id: &str) -> bool {
    if id == crate::photos::IMMICH_CLIENT_ID {
        crate::app_settings::app_enabled(ctx, "photos")
    } else {
        ctx.db.identity_clients().id().find(id.to_owned()).is_some_and(|c| c.enabled)
    }
}

fn model_alive(ctx: &ReducerContext, r: &ProviderModel) -> bool {
    if r.expires_at <= now(ctx) {
        return false;
    }
    if !r.account_id.is_empty() && profile(ctx, &r.account_id).is_none_or(|p| p["active"] != json!(true)) {
        return false;
    }
    if !r.client_id.is_empty() && !client_enabled(ctx, &r.client_id) {
        return false;
    }
    if !r.grant_key.is_empty() && ctx.db.provider_models().id().find(r.grant_key.clone()).map_or(0.0, |g| g.expires_at) <= now(ctx) {
        return false;
    }
    if !r.native_session_id.is_empty() && ctx.db.sessions().id().find(r.native_session_id.clone()).is_none_or(|s| !s.active || s.expires_at <= now(ctx)) {
        return false;
    }
    true
}

pub fn provider_operation(ctx: &ReducerContext, operation: &str, p: &Doc) -> Res<Value> {
    if operation == "saveClient" {
        return directory_operation(ctx, operation, p, &Actor { id: String::new(), scim: false });
    }
    let id = p.get("id").map(|v| crate::text_of(Some(v))).unwrap_or_default();
    match operation {
        "client" => Ok(ctx.db.identity_clients().id().find(id).map_or(Value::Null, |c| row(&c))),
        "account" => Ok(profile(ctx, &id).unwrap_or(Value::Null)),
        "session" => Ok(ctx
            .db
            .sessions()
            .id()
            .find(id)
            .filter(|s| s.active && s.expires_at > now(ctx) && ctx.db.accounts().id().find(s.user_id.clone()).is_some_and(|a| a.enabled))
            .map_or(Value::Null, |s| row(&s))),
        "find" | "findUid" | "findUserCode" => {
            let found = match operation {
                "find" => ctx.db.provider_models().id().find(id),
                "findUid" => ctx.db.provider_models().uid_key().filter(&id).next(),
                _ => ctx.db.provider_models().user_code_key().filter(&id).next(),
            };
            Ok(found.filter(|r| model_alive(ctx, r)).map_or(Value::Null, |r| row(&r)))
        }
        "upsert" => {
            let r: ProviderModel = serde_json::from_value(Value::Object(p.clone())).map_err(|_| "invalidInput".to_owned())?;
            if !r.grant_key.is_empty() && ctx.db.provider_models().id().find(r.grant_key.clone()).map_or(0.0, |g| g.expires_at) <= now(ctx) {
                return fail("conflict");
            }
            if !r.account_id.is_empty() && profile(ctx, &r.account_id).is_none_or(|p| p["active"] != json!(true)) {
                return fail("unauthenticated");
            }
            if !r.native_session_id.is_empty() {
                let s = ctx.db.sessions().id().find(r.native_session_id.clone());
                if s.is_none_or(|s| !s.active || s.expires_at <= now(ctx) || !r.account_id.is_empty() && r.account_id != s.user_id) {
                    return fail("unauthenticated");
                }
            }
            if !r.client_id.is_empty() && !client_enabled(ctx, &r.client_id) {
                return fail("forbidden");
            }
            let old = ctx.db.provider_models().id().find(id);
            if old.is_none() && ctx.db.provider_models().count() >= 50000 {
                return fail("operationTooLarge");
            }
            match old {
                Some(old) => {
                    let consumed = r.consumed.max(old.consumed);
                    ctx.db.provider_models().id().update(ProviderModel { consumed, ..r });
                }
                None => {
                    ctx.db.provider_models().insert(r);
                }
            }
            Ok(json!({ "ok": true }))
        }
        "consume" => {
            let Some(r) = ctx.db.provider_models().id().find(id).filter(|r| r.consumed == 0.0 && r.expires_at > now(ctx)) else { return fail("conflict") };
            ctx.db.provider_models().id().update(ProviderModel { consumed: (now(ctx) / 1000.0).floor(), ..r });
            Ok(json!({ "ok": true }))
        }
        "destroy" => {
            ctx.db.provider_models().id().delete(id);
            Ok(json!({ "ok": true }))
        }
        "revokeGrant" => {
            for r in ctx.db.provider_models().grant_key().filter(&id).collect::<Vec<_>>() {
                ctx.db.provider_models().id().delete(r.id);
            }
            ctx.db.provider_models().id().delete(id);
            Ok(json!({ "ok": true }))
        }
        "cleanup" => {
            let upper = format!("{:0>16}", crate::text_of(Some(&json!(now(ctx)))));
            let mut count = 0;
            for r in ctx.db.provider_models().expiry_key().filter((std::ops::Bound::Unbounded, std::ops::Bound::Included(upper.as_str()))).collect::<Vec<_>>() {
                count += 1;
                if count > 1000 {
                    break;
                }
                ctx.db.provider_models().id().delete(r.id);
            }
            for r in ctx.db.identity_proofs().iter().collect::<Vec<_>>() {
                if r.expires_at <= now(ctx) {
                    ctx.db.identity_proofs().digest().delete(r.digest);
                }
            }
            Ok(json!({ "ok": true }))
        }
        _ => fail("invalidInput"),
    }
}

pub fn identity_snapshot(ctx: &ReducerContext) -> Value {
    json!({
        "profiles": ctx.db.directory_profiles().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "groups": ctx.db.directory_groups().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "members": ctx.db.directory_members().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "clients": ctx.db.identity_clients().iter().map(|r| row(&r)).collect::<Vec<_>>(),
    })
}

fn rows<T: serde::de::DeserializeOwned>(value: &Value, key: &str) -> Res<Vec<T>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(v) => serde_json::from_value(v.clone()).map_err(|_| "invalidInput".to_owned()),
    }
}

pub fn restore_identity(ctx: &ReducerContext, data: Option<&Value>) -> Res<()> {
    let Some(data) = data else { return Ok(()) };
    for r in rows::<DirectoryProfile>(data, "profiles")? {
        if ctx.db.accounts().id().find(r.user_id.clone()).is_none() {
            return fail("invalidInput");
        }
        ctx.db.directory_profiles().insert(r);
    }
    for r in rows::<DirectoryGroup>(data, "groups")? {
        ctx.db.directory_groups().insert(r);
    }
    for r in rows::<DirectoryMember>(data, "members")? {
        if ctx.db.accounts().id().find(r.user_id.clone()).is_none() || ctx.db.directory_groups().id().find(r.group_id.clone()).is_none() {
            return fail("invalidInput");
        }
        ctx.db.directory_members().insert(r);
    }
    for r in rows::<IdentityClient>(data, "clients")? {
        ctx.db.identity_clients().insert(r);
    }
    Ok(())
}

/// Backup rows of one table (the snapshot helpers of every area share it).
pub fn snapshot_rows<T: serde::de::DeserializeOwned>(value: &Value, key: &str) -> Res<Vec<T>> {
    rows(value, key)
}
