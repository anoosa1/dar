//! Identity provider (`identity.integration.test.ts`): the SCIM directory (CRUD, filters,
//! conditional PATCH, membership, credential isolation, tombstones), an independent OpenID
//! relying party (PKCE, nonce, UserInfo, rotation, replay, consent, revocation), and signing
//! key rotation with encrypted persistence and client changes.

use super::core_support::{ENTERPRISE, GROUP, PATCH, USER, form, scim};
use dar_acceptance::oidc::{Authorize, Flow, authorize};
use dar_acceptance::prelude::*;

pub const SUITE: Suite = Suite {
    name: "identity",
    cases: &[case!(scim_directory, 60), case!(relying_party, 90), case!(signing_rotation_and_client_changes, 60)],
};

pub async fn scim_directory(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let base = instance.base.clone();
    let admin = IdentityAdmin::new(&base).await?;
    let (credential_id, secret) = admin.credential("write").await?;
    let (_, read_secret) = admin.credential("read").await?;
    let scim = |path: &str, method: &str, payload: Option<Value>, headers: Vec<(&'static str, String)>| {
        let (base, secret, path, method) = (base.clone(), secret.clone(), path.to_owned(), method.to_owned());
        async move {
            let headers: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
            scim(&base, &secret, &path, &method, payload.as_ref(), &headers).await
        }
    };
    assert_eq!(admin.session.get(&format!("{base}/scim/v2/Users")).await?.status, 401);
    let config = scim("/ServiceProviderConfig", "GET", None, vec![]).await?;
    assert_eq!(config.status, 200);
    assert_eq!(config.json()?["bulk"]["supported"], json!(false));
    let enterprise_schema = format!("/Schemas/{}", url::form_urlencoded::byte_serialize(ENTERPRISE.as_bytes()).collect::<String>());
    for path in ["/Schemas", "/ResourceTypes", enterprise_schema.as_str(), "/ResourceTypes/User"] {
        assert_eq!(scim(path, "GET", None, vec![]).await?.status, 200, "{path}");
    }
    let username = format!("scim-{}", &uuid()[..8]);
    let password = "a long provisioned user password";
    let response = scim("/Users", "POST", Some(json!({"schemas": [USER, ENTERPRISE], "userName": username, "displayName": "SCIM Person", "password": password,
        "emails": [{"value": "work@example.com", "type": "work", "primary": true}], ENTERPRISE: {"department": "Engineering", "employeeNumber": "42"}})), vec![]).await?.expect(201)?;
    assert!(response.header("content-type").unwrap_or_default().starts_with("application/scim+json"));
    let mut user = response.json()?;
    let id = user["id"].as_str().context("user id")?.to_owned();
    let etag = response.header("etag").unwrap_or_default().to_owned();
    assert!(user.get("password").is_none() && user.get("verifier").is_none() && user.get("administrator").is_none());
    assert_eq!(user[ENTERPRISE]["department"], "Engineering");
    assert_eq!(response.header("location"), Some(format!("{base}/scim/v2/Users/{id}").as_str()));
    assert_eq!(scim("/Users", "POST", Some(json!({"userName": username.to_uppercase()})), vec![]).await?.status, 409);
    assert_eq!(scim("/Users", "POST", Some(json!({"userName": "escalation", "administrator": true})), vec![]).await?.status, 400);
    let filter: String = url::form_urlencoded::byte_serialize(format!("userName eq \"{}\" and active eq true", username.to_uppercase()).as_bytes()).collect();
    let list = scim(&format!("/Users?filter={filter}&attributes=userName,{ENTERPRISE}:department&count=1&startIndex=0"), "GET", None, vec![]).await?.json()?;
    assert_eq!(list["totalResults"], 1);
    assert_eq!(list["startIndex"], 1);
    assert!(list["Resources"][0].get("emails").is_none());
    assert_eq!(list["Resources"][0][ENTERPRISE]["department"], "Engineering");
    assert_eq!(scim("/Users?filter=userName%20bad%20%22x%22", "GET", None, vec![]).await?.status, 400);
    assert_eq!(scim("/Users?count=0", "GET", None, vec![]).await?.json()?["Resources"].as_array().map(Vec::len), Some(0));
    let stale = json!({"schemas": [PATCH], "Operations": [{"op": "replace", "path": "displayName", "value": "stale"}]});
    assert_eq!(scim(&format!("/Users/{id}"), "PATCH", Some(stale), vec![("if-match", "W/\"0\"".into())]).await?.status, 412);
    let member = login_as(&base, &username, password).await?;
    let mut group = scim("/Groups", "POST", Some(json!({"schemas": [GROUP], "displayName": "Acceptance group", "members": [{"value": id}, {"value": id}]})), vec![]).await?.expect(201)?.json()?;
    assert_eq!(group["members"].as_array().map(Vec::len), Some(1));
    user = scim(&format!("/Users/{id}"), "GET", None, vec![]).await?.json()?;
    assert_eq!(user["groups"][0]["value"], group["id"]);
    assert_ne!(user["meta"]["version"], json!(etag));
    let group_id = group["id"].as_str().context("group id")?.to_owned();
    let version = group["meta"]["version"].as_str().unwrap_or_default().to_owned();
    group = scim(&format!("/Groups/{group_id}"), "PATCH", Some(json!({"schemas": [PATCH], "Operations": [{"op": "remove", "path": format!("members[value eq \"{id}\"]")}]})), vec![("if-match", version)]).await?.expect(200)?.json()?;
    assert_eq!(group["members"].as_array().map(Vec::len).unwrap_or(0), 0);
    scim(&format!("/Groups/{group_id}"), "PATCH", Some(json!({"schemas": [PATCH], "Operations": [{"op": "add", "path": "members", "value": [{"value": id}]}]})), vec![]).await?.expect(200)?;
    assert_eq!(scim(&format!("/Groups/{group_id}"), "PATCH", Some(json!({"schemas": [PATCH], "Operations": [{"op": "add", "path": "members", "value": [{"value": "missing"}]}]})), vec![]).await?.status, 400);
    user = scim(&format!("/Users/{id}"), "PATCH", Some(json!({"schemas": [PATCH], "Operations": [{"op": "replace", "path": "userName", "value": format!("{username}-renamed")},
        {"op": "replace", "path": "emails[type eq \"work\"].value", "value": "new@example.com"}, {"op": "replace", "path": format!("{ENTERPRISE}:department"), "value": "Design"}]})), vec![]).await?.expect(200)?.json()?;
    assert_eq!(user["id"], id.as_str());
    assert_eq!(user[ENTERPRISE]["department"], "Design");
    user = scim(&format!("/Users/{id}"), "PUT", Some(json!({"schemas": [USER], "userName": format!("{username}-renamed"), "displayName": "Replaced profile", "active": true})), vec![]).await?.expect(200)?.json()?;
    assert!(user.get("emails").is_none() && user.get(ENTERPRISE).is_none());
    assert_eq!(user["id"], id.as_str());
    scim(&format!("/Users/{id}"), "PATCH", Some(json!({"schemas": [PATCH], "Operations": [{"op": "replace", "path": "active", "value": false}]})), vec![]).await?.expect(200)?;
    assert_eq!(member.get("/api/v1/auth/session").await?.status, 401);
    scim(&format!("/Users/{id}"), "PATCH", Some(json!({"schemas": [PATCH], "Operations": [{"op": "replace", "path": "active", "value": true}]})), vec![]).await?.expect(200)?;
    login_as(&base, &format!("{username}-renamed"), password).await?;
    assert_eq!(scim("/Users", "POST", Some(json!({"userName": "readonly-create"})), vec![("authorization", format!("Bearer {read_secret}"))]).await?.status, 403);
    let operator = scim("/Users?filter=userName%20eq%20%22admin%22", "GET", None, vec![]).await?.json()?["Resources"][0].clone();
    let operator_id = operator["id"].as_str().context("operator id")?.to_owned();
    assert_eq!(scim(&format!("/Users/{operator_id}"), "DELETE", None, vec![]).await?.status, 403);
    assert_eq!(scim(&format!("/Users/{operator_id}"), "PATCH", Some(json!({"schemas": [PATCH], "Operations": [{"op": "replace", "path": "active", "value": false}]})), vec![]).await?.status, 403);
    assert_eq!(scim(&format!("/Users/{operator_id}"), "PATCH", Some(json!({"schemas": [PATCH], "Operations": [{"op": "replace", "path": "userName", "value": "operator-renamed"}]})), vec![]).await?.status, 403);
    assert_eq!(scim(&format!("/Users/{operator_id}"), "PATCH", Some(json!({"schemas": [PATCH], "Operations": [{"op": "replace", "path": "displayName", "value": operator["displayName"]}]})), vec![]).await?.status, 200);
    assert_eq!(admin.session.get_json("/api/v1/auth/session").await?["user"]["administrator"], json!(true));
    assert_eq!(scim(&format!("/Users/{id}"), "DELETE", None, vec![]).await?.status, 204);
    assert_eq!(scim(&format!("/Users/{id}"), "GET", None, vec![]).await?.status, 404);
    assert_eq!(scim(&format!("/Users/{id}"), "DELETE", None, vec![]).await?.status, 404);
    assert_eq!(scim("/Users", "POST", Some(json!({"userName": format!("{username}-renamed")})), vec![]).await?.status, 409);
    let owner = instance.database()?;
    assert!(owner.call("service_set_enabled", json!([id, true])).await.is_err());
    group = scim(&format!("/Groups/{group_id}"), "GET", None, vec![]).await?.json()?;
    assert_eq!(group["members"].as_array().map(Vec::len).unwrap_or(0), 0);
    let version = group["meta"]["version"].as_str().unwrap_or_default().to_owned();
    assert_eq!(scim(&format!("/Groups/{group_id}"), "DELETE", None, vec![("if-match", version)]).await?.status, 204);
    let no_password = scim("/Users", "POST", Some(json!({"userName": format!("passwordless-{}", &uuid()[..6])})), vec![]).await?.expect(201)?.json()?;
    let attempt = session::client().post(format!("{base}/api/v1/auth/login")).header("origin", &base).json(&json!({"username": no_password["userName"], "password": ""})).send().await?;
    assert_eq!(attempt.status().as_u16(), 401);
    let proof = admin.api("/confirmation", "POST", Some(&json!({"password": PASSWORD}))).await?.json()?["proof"].as_str().unwrap_or_default().to_owned();
    let cookie_secret = admin.session.cookie.split_once('=').map(|(_, v)| v).unwrap_or_default();
    let native = owner.call("service_session", json!([support::digest_token(cookie_secret)])).await?;
    let (short_secret, short_digest) = support::create_opaque_token();
    let expires_at = chrono::Utc::now().timestamp_millis() + 1000;
    owner.call("service_identity", json!([native["session"]["id"], "", support::digest_token(&proof), "createToken",
        json!({"id": uuid(), "digest": short_digest, "label": "Expiry boundary proof", "scope": "read", "expiresAt": expires_at}).to_string()])).await?;
    assert_eq!(scim("/ServiceProviderConfig", "GET", None, vec![("authorization", format!("Bearer {short_secret}"))]).await?.status, 200);
    let wait = (expires_at - chrono::Utc::now().timestamp_millis() + 10).max(0) as u64;
    tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
    assert_eq!(scim("/ServiceProviderConfig", "GET", None, vec![("authorization", format!("Bearer {short_secret}"))]).await?.status, 401);
    assert_eq!(admin.write(&format!("/credentials/{credential_id}"), "DELETE", None, None).await?.status, 200);
    assert_eq!(scim("/Users", "GET", None, vec![]).await?.status, 401);
    Ok(())
}

async fn introspect(base: &str, client: &dar_acceptance::oidc::Client, token: &str) -> Result<Value> {
    let response = session::client().post(format!("{base}/oidc/token/introspection")).basic_auth(&client.id, client.secret.as_deref())
        .header("content-type", "application/x-www-form-urlencoded").body(form(&[("token", token)])).send().await?;
    Ok(response.json().await?)
}

pub async fn relying_party(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = IdentityAdmin::new(&base).await?;
    let client = admin.default_client().await?;
    let discovery = &client.metadata;
    assert_eq!(discovery["issuer"], format!("{base}/oidc"));
    assert_eq!(discovery["response_types_supported"], json!(["code"]));
    assert!(discovery.get("registration_endpoint").is_none());
    let flow = authorize(&base, &admin.session, &client, Authorize::default()).await?;
    let tokens = client.exchange(&flow).await?;
    assert!(!tokens.access_token().is_empty());
    assert!(tokens.refresh_token().is_some());
    let claims = tokens.claims.clone().context("ID token claims")?;
    assert_eq!(claims["nonce"], flow.nonce.as_str());
    assert_eq!(claims["aud"], client.id.as_str());
    let native = admin.session.get_json("/api/v1/auth/session").await?;
    assert_eq!(claims["sub"], native["user"]["id"]);
    let profile = client.user_info(tokens.access_token()).await?;
    assert_eq!(profile["sub"], claims["sub"]);
    assert_eq!(profile["preferred_username"], "admin");
    assert!(profile.get("verifier").is_none() && profile.get("email_verified").is_none());
    assert_eq!(session::client().get(format!("{base}/scim/v2/Users")).bearer_auth(tokens.access_token()).send().await?.status().as_u16(), 401);
    assert_eq!(introspect(&base, &client, tokens.access_token()).await?["active"], json!(true));
    let rotated = client.refresh(tokens.refresh_token().unwrap()).await?;
    assert_ne!(rotated.refresh_token(), tokens.refresh_token());
    assert!(client.refresh(tokens.refresh_token().unwrap()).await.is_err());
    assert_eq!(introspect(&base, &client, rotated.access_token()).await?["active"], json!(false));
    let denied = authorize(&base, &admin.session, &client, Authorize { deny: true, ..Authorize::default() }).await?;
    let query = |flow: &Flow, key: &str| flow.url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned());
    assert_eq!(query(&denied, "error").as_deref(), Some("access_denied"));
    assert_eq!(query(&denied, "state"), Some(denied.state.clone()));
    let wrong = authorize(&base, &admin.session, &client, Authorize::default()).await?;
    assert!(client.exchange(&Flow { verifier: dar_acceptance::base64url(&dar_acceptance::random_bytes(32)), ..wrong }).await.is_err());
    let concurrent = authorize(&base, &admin.session, &client, Authorize::default()).await?;
    let (one, two) = tokio::join!(client.exchange(&concurrent), client.exchange(&concurrent));
    assert!([one.is_ok(), two.is_ok()].iter().filter(|ok| **ok).count() <= 1);
    assert!(one.is_err() || two.is_err());
    let again = authorize(&base, &admin.session, &client, Authorize::default()).await?;
    let active = client.exchange(&again).await?;
    let connections = admin.get("/connections").await?;
    let grant = connections["items"].as_array().and_then(|items| items.iter().find(|v| v["clientId"] == client.id.as_str())).context("connection")?.clone();
    let grant_id: String = url::form_urlencoded::byte_serialize(grant["id"].as_str().unwrap_or_default().as_bytes()).collect();
    assert_eq!(admin.api(&format!("/connections/{grant_id}"), "DELETE", None).await?.status, 200);
    assert_eq!(introspect(&base, &client, active.access_token()).await?["active"], json!(false));
    let public_client = admin.client("public", "openid profile", "http://127.0.0.1:49999/callback").await?;
    let mut public_flow = authorize(&base, &admin.session, &public_client, Authorize { scope: Some("openid profile".into()), ..Authorize::default() }).await?;
    let public_tokens = public_client.exchange(&public_flow).await?;
    assert!(public_tokens.id_token().is_some());
    let cors = session::client().request(reqwest::Method::OPTIONS, format!("{base}/oidc/token")).header("origin", "http://127.0.0.1:49999").header("access-control-request-method", "POST").send().await?;
    assert!([200, 204].contains(&cors.status().as_u16()));
    let no_pkce = public_client.authorization_url(&[("redirect_uri".into(), public_client.redirect.clone()), ("response_type".into(), "code".into()), ("scope".into(), "openid".into())])?;
    let rejected = session::client().get(no_pkce).send().await?;
    let location = url::Url::parse(rejected.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or_default())?;
    assert_eq!(location.query_pairs().find(|(k, _)| k == "error").map(|(_, v)| v.into_owned()).as_deref(), Some("invalid_request"));
    let unknown = session::client().get(client.authorization_url(&[("redirect_uri".into(), "http://127.0.0.1:49999/evil".into()), ("response_type".into(), "code".into()), ("scope".into(), "openid".into())])?).send().await?;
    assert_eq!(unknown.status().as_u16(), 400);
    assert!(unknown.headers().get("location").is_none());
    let mut logout = url::Url::parse(discovery["end_session_endpoint"].as_str().unwrap_or_default())?;
    logout.query_pairs_mut().append_pair("id_token_hint", public_tokens.id_token().unwrap()).append_pair("post_logout_redirect_uri", "https://evil.example/");
    assert_eq!(public_flow.jar.get(logout.as_str()).await?.status, 400);
    Ok(())
}

fn kid(token: &str) -> Option<String> {
    jsonwebtoken::decode_header(token).ok().and_then(|header| header.kid)
}

pub async fn signing_rotation_and_client_changes(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let base = instance.base.clone();
    let admin = IdentityAdmin::new(&base).await?;
    let client = admin.default_client().await?;
    let tokens = client.exchange(&authorize(&base, &admin.session, &client, Authorize::default()).await?).await?;
    let jwks_json = |path: String| async move { Ok::<Value, Error>(session::client().get(path).send().await?.json().await?) };
    let internal_before = jwks_json(format!("{base}/auth/issuer/jwks")).await?;
    let old_kid = kid(tokens.id_token().context("ID token")?);
    let status = admin.write("/keys/rotate", "POST", None, None).await?.expect(200)?.json()?;
    assert_ne!(status["activeKid"].as_str().map(str::to_owned), old_kid);
    let jwks = jwks_json(format!("{base}/oidc/jwks")).await?;
    let keys = jwks["keys"].as_array().context("keys")?;
    assert!(keys.iter().any(|k| k["kid"].as_str().map(str::to_owned) == old_kid));
    assert!(keys.iter().all(|k| k.get("d").is_none()));
    assert_eq!(jwks_json(format!("{base}/auth/issuer/jwks")).await?, internal_before);
    let fresh = client.exchange(&authorize(&base, &admin.session, &client, Authorize::default()).await?).await?;
    assert_eq!(kid(fresh.id_token().context("ID token")?).as_deref(), status["activeKid"].as_str());
    let keyring = dar_acceptance::read_to_string(instance.root().join("secrets/oidc-keyring.json"))?;
    assert!(!regex::Regex::new(r#""d"|"cookieKey"|RSA"#)?.is_match(&keyring));
    let db = instance.database()?;
    assert!(db.call("service_identity", json!([admin.session.cookie, "", "", "clients", "{}"])).await.is_err());
    let list = admin.get("/clients").await?;
    let registered = list["items"].as_array().and_then(|items| items.iter().find(|v| v["id"] == client.id.as_str())).context("registered client")?.clone();
    assert_eq!(registered["name"], client.payload["name"]);
    assert!(registered.get("secret").is_none() && registered.get("encrypted").is_none() && registered.get("metadata").is_none());
    assert!(registered["modifiedAt"].is_string());
    let detail = admin.api(&format!("/clients/{}", client.id), "GET", None).await?;
    assert_eq!(detail.status, 200);
    let tag = format!("W/\"{}\"", registered["revision"]);
    assert_eq!(detail.header("etag"), Some(tag.as_str()));
    let mut disabled = client.payload.clone();
    disabled["enabled"] = json!(false);
    assert_eq!(admin.write(&format!("/clients/{}", client.id), "PUT", Some(&disabled), Some(&tag)).await?.status, 200);
    assert_eq!(session::client().get(format!("{base}/oidc/me")).bearer_auth(fresh.access_token()).send().await?.status().as_u16(), 401);
    assert!(client.refresh(fresh.refresh_token().context("refresh token")?).await.is_err());
    Ok(())
}
