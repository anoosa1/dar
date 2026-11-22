//! End-to-end checks over HTTP: a browser, a relying party and Dar's sign-in page (called
//! in-process, as Dar's API would), against an isolated in-memory provider.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD as B64};
use dar_oidc::{Account, Client, ClientInput, Decision, Directory, Error, Keyring, MemoryStore, NativeSession, Provider, ProviderConfig, Revoke};
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, LOCATION, SET_COOKIE};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

#[derive(Default)]
struct TestDirectory {
    accounts: Mutex<HashMap<String, Account>>,
    sessions: Mutex<HashMap<String, NativeSession>>,
}

impl Directory for TestDirectory {
    fn account(&self, id: &str) -> Option<Account> {
        self.accounts.lock().unwrap().get(id).cloned()
    }
    fn native_session(&self, id: &str) -> Option<NativeSession> {
        self.sessions.lock().unwrap().get(id).cloned()
    }
}

const SECRET: &str = "a-confidential-client-secret-of-40-chars!";
const GITEA_CB: &str = "https://git.example/callback";
const SPA_CB: &str = "http://127.0.0.1:9999/cb";

struct Harness {
    base: String,
    provider: Provider,
    directory: Arc<TestDirectory>,
    http: reqwest::Client,
    /// The browser's cookies (paths ignored; names are distinct per interaction anyway).
    jar: Mutex<HashMap<String, String>>,
}

async fn start() -> Harness {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let directory = Arc::new(TestDirectory::default());
    directory.accounts.lock().unwrap().insert(
        "acct-1".into(),
        Account { id: "acct-1".into(), user_name: "amina".into(), display_name: "Amina".into(), email: Some("amina@example.test".into()), groups: vec!["g1".into()], ..Default::default() },
    );
    let config = ProviderConfig::new(&base, true).unwrap();
    let provider = Provider::new(config, Keyring::generate().unwrap(), Box::new(MemoryStore::default()), directory.clone());
    let gitea = ClientInput {
        id: "gitea".into(),
        name: "Gitea".into(),
        kind: "confidential".into(),
        secret: Some(SECRET.into()),
        redirect_uris: vec![GITEA_CB.into()],
        post_logout_redirect_uris: vec!["https://git.example/bye".into()],
        scopes: vec!["openid profile email groups offline_access".into()],
        ..Default::default()
    };
    let spa = ClientInput { id: "spa".into(), name: "SPA".into(), kind: "public".into(), redirect_uris: vec![SPA_CB.into()], scopes: vec!["openid profile offline_access".into()], ..Default::default() };
    provider.set_clients(vec![Client::from_input(&gitea, true).unwrap(), Client::from_input(&spa, true).unwrap()]);
    let app = axum::Router::new().nest("/oidc", provider.router());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    Harness { base, provider, directory, http, jar: Mutex::new(HashMap::new()) }
}

fn pkce() -> (String, String) {
    let verifier = B64.encode(rand_bytes());
    let challenge = B64.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn rand_bytes() -> [u8; 32] {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).unwrap();
    b
}

impl Harness {
    fn native(&self, id: &str, created_at: i64) -> NativeSession {
        let session = NativeSession { id: id.into(), account_id: "acct-1".into(), created_at };
        self.directory.sessions.lock().unwrap().insert(id.into(), session.clone());
        session
    }

    fn remember(&self, headers: &HeaderMap) {
        for value in headers.get_all(SET_COOKIE) {
            let text = value.to_str().unwrap();
            let pair = text.split(';').next().unwrap();
            let (name, value) = pair.split_once('=').unwrap();
            let path = text.split("; ").find_map(|a| a.strip_prefix("Path=")).unwrap_or("/");
            let mut jar = self.jar.lock().unwrap();
            let slot = format!("{name}|{path}");
            if value.is_empty() || text.contains("Max-Age=0") {
                jar.remove(&slot);
            } else {
                jar.insert(slot, value.into());
            }
        }
    }

    /// The cookie a browser would send to `path`.
    fn cookie_at(&self, name: &str, path: &str) -> Option<String> {
        self.jar.lock().unwrap().iter().find(|(slot, _)| slot.split_once('|').is_some_and(|(n, p)| n == name && path.starts_with(p))).map(|(_, v)| v.clone())
    }

    fn cookie(&self, name: &str) -> Option<String> {
        self.cookie_at(name, "/")
            .or_else(|| self.jar.lock().unwrap().iter().find(|(slot, _)| slot.starts_with(&format!("{name}|"))).map(|(_, v)| v.clone()))
    }

    fn cookie_header(&self, path: &str) -> String {
        let jar = self.jar.lock().unwrap();
        jar.iter().filter_map(|(slot, v)| slot.split_once('|').filter(|(_, p)| path.starts_with(p)).map(|(n, _)| format!("{n}={v}"))).collect::<Vec<_>>().join("; ")
    }

    /// A browser GET: send cookies, keep new ones, return the status and Location.
    async fn browse(&self, url: &str) -> (StatusCode, Option<String>, String) {
        let path = Url::parse(url).unwrap().path().to_owned();
        let response = self.http.get(url).header("cookie", self.cookie_header(&path)).send().await.unwrap();
        self.remember(response.headers());
        let status = response.status();
        let location = response.headers().get(LOCATION).map(|v| v.to_str().unwrap().to_owned());
        (status, location, response.text().await.unwrap())
    }

    fn auth_url(&self, client: &str, redirect: &str, scope: &str, extra: &[(&str, &str)]) -> String {
        let mut url = Url::parse(&format!("{}/oidc/auth", self.base)).unwrap();
        url.query_pairs_mut().append_pair("response_type", "code").append_pair("client_id", client).append_pair("redirect_uri", redirect).append_pair("scope", scope).append_pair("state", "st-1");
        url.query_pairs_mut().extend_pairs(extra.iter());
        url.to_string()
    }

    fn uid(location: &str) -> String {
        location.rsplit('/').next().unwrap().to_owned()
    }

    /// Follow the interaction (login, then consent) until the client callback.
    async fn sign_in(&self, start: &str, native: &NativeSession) -> Url {
        let (status, mut location, _) = self.browse(start).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        for _ in 0..4 {
            let target = location.clone().unwrap();
            if !target.contains("/identity/interaction/") {
                return Url::parse(&target).unwrap();
            }
            let uid = Self::uid(&target);
            let cookie = self.cookie_at("dar_oidc_interaction", &format!("/oidc/auth/{uid}"));
            let details = self.provider.interaction_details(&uid, cookie.as_deref(), native).unwrap();
            let decision = if details.prompt == "login" { Decision::Login { password_confirmed: details.requires_password } } else { Decision::Approve };
            let resume = self.provider.decide(&uid, cookie.as_deref(), native, decision).unwrap();
            let (status, next, _) = self.browse(&resume).await;
            assert_eq!(status, StatusCode::SEE_OTHER, "{resume}");
            location = next;
        }
        panic!("sign-in did not finish");
    }

    async fn token(&self, form: &[(&str, &str)], basic: Option<(&str, &str)>) -> (StatusCode, Value) {
        let mut request = self.http.post(format!("{}/oidc/token", self.base)).form(form);
        if let Some((id, secret)) = basic {
            request = request.header("authorization", format!("Basic {}", STANDARD.encode(format!("{id}:{secret}"))));
        }
        let response = request.send().await.unwrap();
        (response.status(), response.json().await.unwrap_or(Value::Null))
    }

    async fn introspect(&self, token: &str) -> Value {
        let auth = format!("Basic {}", STANDARD.encode(format!("gitea:{SECRET}")));
        let response = self.http.post(format!("{}/oidc/token/introspection", self.base)).header("authorization", auth).form(&[("token", token)]).send().await.unwrap();
        response.json().await.unwrap()
    }

    async fn verify_id_token(&self, token: &str, audience: &str) -> Value {
        let jwks: Value = self.http.get(format!("{}/oidc/jwks", self.base)).send().await.unwrap().json().await.unwrap();
        let header = jsonwebtoken::decode_header(token).unwrap();
        let key = jwks["keys"].as_array().unwrap().iter().find(|k| k["kid"].as_str() == header.kid.as_deref()).expect("key published");
        let decoding = jsonwebtoken::DecodingKey::from_rsa_components(key["n"].as_str().unwrap(), key["e"].as_str().unwrap()).unwrap();
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.set_audience(&[audience]);
        validation.set_issuer(&[format!("{}/oidc", self.base)]);
        jsonwebtoken::decode::<Value>(token, &decoding, &validation).unwrap().claims
    }
}

fn query(url: &Url, name: &str) -> Option<String> {
    url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned())
}

#[tokio::test]
async fn discovery_and_jwks() {
    let h = start().await;
    let doc: Value = h.http.get(format!("{}/oidc/.well-known/openid-configuration", h.base)).send().await.unwrap().json().await.unwrap();
    assert_eq!(doc["issuer"], format!("{}/oidc", h.base));
    assert_eq!(doc["code_challenge_methods_supported"][0], "S256");
    let jwks: Value = h.http.get(doc["jwks_uri"].as_str().unwrap()).send().await.unwrap().json().await.unwrap();
    assert_eq!(jwks["keys"][0]["alg"], "RS256");
    assert!(jwks["keys"][0].get("d").is_none(), "private parts never leave the provider");
}

#[tokio::test]
async fn confidential_client_full_flow() {
    let h = start().await;
    let native = h.native("native-1", 0);
    let (verifier, challenge) = pkce();
    let start = h.auth_url("gitea", GITEA_CB, "openid profile email offline_access", &[("nonce", "n-1"), ("code_challenge", &challenge), ("code_challenge_method", "S256"), ("prompt", "consent")]);

    // The first interaction asks for a login; the old native session needs the password.
    let (_, location, _) = h.browse(&start).await;
    let uid = Harness::uid(location.as_deref().unwrap());
    let cookie = h.cookie_at("dar_oidc_interaction", &format!("/oidc/auth/{uid}"));
    let details = h.provider.interaction_details(&uid, cookie.as_deref(), &native).unwrap();
    assert_eq!(details.prompt, "login");
    assert!(details.requires_password);
    assert!(matches!(h.provider.decide(&uid, cookie.as_deref(), &native, Decision::Login { password_confirmed: false }), Err(Error::PasswordRequired)));
    assert!(matches!(h.provider.interaction_details(&uid, None, &native), Err(Error::InteractionExpired)), "another browser cannot use the interaction");
    let resume = h.provider.decide(&uid, cookie.as_deref(), &native, Decision::Login { password_confirmed: true }).unwrap();
    let (_, location, _) = h.browse(&resume).await;
    let location = location.unwrap();
    assert!(location.contains("/identity/interaction/"), "consent follows login");
    let uid = Harness::uid(&location);
    let cookie = h.cookie_at("dar_oidc_interaction", &format!("/oidc/auth/{uid}"));
    let details = h.provider.interaction_details(&uid, cookie.as_deref(), &native).unwrap();
    assert_eq!(details.prompt, "consent");
    let scopes: Vec<_> = details.permissions.iter().map(|p| p.scope.as_str()).collect();
    assert_eq!(scopes, ["openid", "profile", "email", "offline_access"]);
    let resume = h.provider.decide(&uid, cookie.as_deref(), &native, Decision::Approve).unwrap();
    let (_, location, _) = h.browse(&resume).await;
    let callback = Url::parse(&location.unwrap()).unwrap();
    assert!(callback.as_str().starts_with(GITEA_CB));
    assert_eq!(query(&callback, "state").as_deref(), Some("st-1"));
    assert_eq!(query(&callback, "iss"), Some(format!("{}/oidc", h.base)));
    let code = query(&callback, "code").unwrap();

    // Wrong verifier, then wrong secret, are refused without consuming… the verifier one does consume.
    let (status, body) = h.token(&[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", GITEA_CB), ("code_verifier", &verifier)], Some(("gitea", "wrong-secret-wrong-secret-wrong-secret"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let (status, tokens) = h.token(&[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", GITEA_CB), ("code_verifier", &verifier)], Some(("gitea", SECRET))).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    assert_eq!(tokens["token_type"], "Bearer");
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    let refresh = tokens["refresh_token"].as_str().expect("offline_access with prompt=consent gives a refresh token").to_owned();
    let claims = h.verify_id_token(tokens["id_token"].as_str().unwrap(), "gitea").await;
    assert_eq!(claims["sub"], "acct-1");
    assert_eq!(claims["nonce"], "n-1");
    assert_eq!(claims["at_hash"], B64.encode(&Sha256::digest(access.as_bytes())[..16]));

    let info: Value = h.http.get(format!("{}/oidc/me", h.base)).bearer_auth(&access).send().await.unwrap().json().await.unwrap();
    assert_eq!(info["preferred_username"], "amina");
    assert_eq!(info["email_verified"], false);
    assert!(info.get("groups").is_none(), "groups was not requested");
    assert_eq!(h.introspect(&access).await["active"], true);

    // Refresh rotates; presenting the old refresh token again revokes the grant.
    let (status, rotated) = h.token(&[("grant_type", "refresh_token"), ("refresh_token", &refresh)], Some(("gitea", SECRET))).await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    let new_access = rotated["access_token"].as_str().unwrap().to_owned();
    assert!(rotated["refresh_token"].as_str().is_some_and(|t| t != refresh));
    assert!(rotated["id_token"].is_string());
    let (status, _) = h.token(&[("grant_type", "refresh_token"), ("refresh_token", &refresh)], Some(("gitea", SECRET))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(h.introspect(&new_access).await["active"], false, "refresh replay revokes the grant");

    // A replayed code is refused.
    let (status, body) = h.token(&[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", GITEA_CB), ("code_verifier", &verifier)], Some(("gitea", SECRET))).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_grant")));

    // The provider session survives, but the grant is gone: prompt=none needs consent.
    let silent = h.auth_url("gitea", GITEA_CB, "openid", &[("code_challenge", &challenge), ("code_challenge_method", "S256"), ("prompt", "none")]);
    let (_, location, _) = h.browse(&silent).await;
    assert_eq!(query(&Url::parse(&location.unwrap()).unwrap(), "error").as_deref(), Some("consent_required"));
}

#[tokio::test]
async fn code_replay_revokes_issued_tokens() {
    let h = start().await;
    let native = h.native("native-1", i64::MAX);
    let (verifier, challenge) = pkce();
    let callback = h.sign_in(&h.auth_url("gitea", GITEA_CB, "openid", &[("code_challenge", &challenge), ("code_challenge_method", "S256")]), &native).await;
    let code = query(&callback, "code").unwrap();
    let form = [("grant_type", "authorization_code"), ("code", code.as_str()), ("redirect_uri", GITEA_CB), ("code_verifier", verifier.as_str())];
    let (_, tokens) = h.token(&form, Some(("gitea", SECRET))).await;
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    assert!(tokens.get("refresh_token").is_none(), "no offline_access, no refresh token");
    assert_eq!(h.introspect(&access).await["active"], true);
    h.token(&form, Some(("gitea", SECRET))).await;
    assert_eq!(h.introspect(&access).await["active"], false);
}

#[tokio::test]
async fn public_client_rules() {
    let h = start().await;
    let native = h.native("native-1", i64::MAX);
    // PKCE is mandatory.
    let (_, location, _) = h.browse(&h.auth_url("spa", SPA_CB, "openid", &[])).await;
    assert_eq!(query(&Url::parse(&location.unwrap()).unwrap(), "error").as_deref(), Some("invalid_request"));
    // plain PKCE is not accepted.
    let (_, location, _) = h.browse(&h.auth_url("spa", SPA_CB, "openid", &[("code_challenge", "abc"), ("code_challenge_method", "plain")])).await;
    assert_eq!(query(&Url::parse(&location.unwrap()).unwrap(), "error").as_deref(), Some("invalid_request"));

    let (verifier, challenge) = pkce();
    // offline_access without prompt=consent is dropped, so no refresh token.
    let callback = h.sign_in(&h.auth_url("spa", SPA_CB, "openid offline_access", &[("code_challenge", &challenge), ("code_challenge_method", "S256")]), &native).await;
    let code = query(&callback, "code").unwrap();
    let (other_verifier, _) = pkce();
    let (status, _) = h.token(&[("grant_type", "authorization_code"), ("client_id", "spa"), ("code", &code), ("redirect_uri", SPA_CB), ("code_verifier", &other_verifier)], None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a wrong verifier fails");

    let callback = h.sign_in(&h.auth_url("spa", SPA_CB, "openid offline_access", &[("code_challenge", &challenge), ("code_challenge_method", "S256")]), &native).await;
    let code = query(&callback, "code").unwrap();
    let response = h
        .http
        .post(format!("{}/oidc/token", h.base))
        .header("origin", "http://127.0.0.1:9999")
        .form(&[("grant_type", "authorization_code"), ("client_id", "spa"), ("code", &code), ("redirect_uri", SPA_CB), ("code_verifier", &verifier)])
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers().get("access-control-allow-origin").unwrap(), "http://127.0.0.1:9999");
    let tokens: Value = response.json().await.unwrap();
    assert_eq!(tokens["scope"], "openid");
    assert!(tokens.get("refresh_token").is_none());
}

#[tokio::test]
async fn invalid_clients_and_redirects_never_redirect() {
    let h = start().await;
    for url in [h.auth_url("nobody", GITEA_CB, "openid", &[]), h.auth_url("gitea", "https://evil.example/cb", "openid", &[]), h.auth_url("gitea", "https://git.example/callback/", "openid", &[])] {
        let (status, location, body) = h.browse(&url).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(location.is_none());
        assert!(body.contains("Unable to continue"));
    }
    // Repeated parameters are invalid.
    let (status, _, _) = h.browse(&format!("{}&state=again", h.auth_url("gitea", GITEA_CB, "openid", &[]))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn native_logout_revokes_everything() {
    let h = start().await;
    let native = h.native("native-1", i64::MAX);
    let (verifier, challenge) = pkce();
    let callback = h.sign_in(&h.auth_url("gitea", GITEA_CB, "openid profile", &[("code_challenge", &challenge), ("code_challenge_method", "S256")]), &native).await;
    let code = query(&callback, "code").unwrap();
    let (_, tokens) = h.token(&[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", GITEA_CB), ("code_verifier", &verifier)], Some(("gitea", SECRET))).await;
    let access = tokens["access_token"].as_str().unwrap().to_owned();

    // Silent re-authentication works while the native session lives.
    let silent = h.auth_url("gitea", GITEA_CB, "openid profile", &[("code_challenge", &challenge), ("code_challenge_method", "S256"), ("prompt", "none")]);
    let (_, location, _) = h.browse(&silent).await;
    assert!(query(&Url::parse(&location.unwrap()).unwrap(), "code").is_some());

    h.directory.sessions.lock().unwrap().remove("native-1");
    h.provider.revoke(Revoke::NativeSession("native-1".into()));
    let status = h.http.get(format!("{}/oidc/me", h.base)).bearer_auth(&access).send().await.unwrap().status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (_, location, _) = h.browse(&silent).await;
    assert_eq!(query(&Url::parse(&location.unwrap()).unwrap(), "error").as_deref(), Some("login_required"));
}

#[tokio::test]
async fn rp_initiated_logout() {
    let h = start().await;
    let native = h.native("native-1", i64::MAX);
    let (verifier, challenge) = pkce();
    let callback = h.sign_in(&h.auth_url("gitea", GITEA_CB, "openid", &[("code_challenge", &challenge), ("code_challenge_method", "S256")]), &native).await;
    let code = query(&callback, "code").unwrap();
    let (_, tokens) = h.token(&[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", GITEA_CB), ("code_verifier", &verifier)], Some(("gitea", SECRET))).await;
    let id_token = tokens["id_token"].as_str().unwrap();

    let mut url = Url::parse(&format!("{}/oidc/session/end", h.base)).unwrap();
    url.query_pairs_mut().append_pair("id_token_hint", id_token).append_pair("post_logout_redirect_uri", "https://evil.example/bye");
    assert_eq!(h.browse(url.as_str()).await.0, StatusCode::BAD_REQUEST, "unregistered logout redirects are refused");

    let mut url = Url::parse(&format!("{}/oidc/session/end", h.base)).unwrap();
    url.query_pairs_mut().append_pair("id_token_hint", id_token).append_pair("post_logout_redirect_uri", "https://git.example/bye").append_pair("state", "bye-1");
    let (status, _, page) = h.browse(url.as_str()).await;
    assert_eq!(status, StatusCode::OK);
    let xsrf = page.split("name=\"xsrf\" value=\"").nth(1).unwrap().split('"').next().unwrap().to_owned();
    let response = h
        .http
        .post(format!("{}/oidc/session/end/confirm", h.base))
        .header("cookie", h.cookie_header("/oidc/session/end/confirm"))
        .form(&[("xsrf", xsrf.as_str()), ("logout", "yes"), ("post_logout_redirect_uri", "https://git.example/bye"), ("state", "bye-1"), ("client_id", "gitea")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[LOCATION], "https://git.example/bye?state=bye-1");
    h.remember(response.headers());
    assert!(h.cookie("dar_oidc").is_none());
    let silent = h.auth_url("gitea", GITEA_CB, "openid", &[("code_challenge", &challenge), ("code_challenge_method", "S256"), ("prompt", "none")]);
    let (_, location, _) = h.browse(&silent).await;
    assert_eq!(query(&Url::parse(&location.unwrap()).unwrap(), "error").as_deref(), Some("login_required"));
}

#[tokio::test]
async fn built_in_immich_code() {
    let h = start().await;
    let native = h.native("native-1", i64::MAX);
    let (verifier, challenge) = pkce();
    let redirect = "app.immich:///oauth-callback";
    let callback = h.sign_in(&h.auth_url("immich-app", redirect, "openid", &[("code_challenge", &challenge), ("code_challenge_method", "S256")]), &native).await;
    let code = query(&callback, "code").unwrap();
    assert!(h.provider.redeem_built_in_code("immich-app", &code, redirect, "x".repeat(43).as_str()).is_err(), "wrong verifier");
    let callback = h.sign_in(&h.auth_url("immich-app", redirect, "openid", &[("code_challenge", &challenge), ("code_challenge_method", "S256")]), &native).await;
    let code = query(&callback, "code").unwrap();
    assert_eq!(h.provider.redeem_built_in_code("immich-app", &code, redirect, &verifier).unwrap(), "acct-1");
    assert!(h.provider.redeem_built_in_code("immich-app", &code, redirect, &verifier).is_err(), "codes are single-use");
}

#[tokio::test]
async fn key_rotation_keeps_old_tokens_verifiable() {
    let h = start().await;
    let native = h.native("native-1", i64::MAX);
    let (verifier, challenge) = pkce();
    let callback = h.sign_in(&h.auth_url("gitea", GITEA_CB, "openid", &[("code_challenge", &challenge), ("code_challenge_method", "S256")]), &native).await;
    let code = query(&callback, "code").unwrap();
    let (_, tokens) = h.token(&[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", GITEA_CB), ("code_verifier", &verifier)], Some(("gitea", SECRET))).await;
    h.provider.rotate_keys().unwrap();
    let jwks: Value = h.http.get(format!("{}/oidc/jwks", h.base)).send().await.unwrap().json().await.unwrap();
    assert_eq!(jwks["keys"].as_array().unwrap().len(), 2);
    h.verify_id_token(tokens["id_token"].as_str().unwrap(), "gitea").await;
}
