//! Production proxy OIDC (`identity-proxy.integration.test.ts`): checks local proxy headers,
//! secure cookies and canonical HTTPS issuer throughout a real code exchange.

use super::core_support::{form, query_param};
use dar_acceptance::identity::Interaction;
use dar_acceptance::prelude::*;
use dar_acceptance::stack::stack;
use std::collections::BTreeMap;

pub const SUITE: Suite = Suite {
    name: "identity-proxy",
    cases: &[case!(production_proxy_headers_cookies_and_issuer, 60)],
};

struct Proxy {
    origin: String,
    backend: String,
    set_cookies: Vec<String>,
}

impl Proxy {
    async fn send(&mut self, method: &str, url: &str, headers: &[(&str, &str)], body: Option<String>) -> Result<Reply> {
        let parsed = url::Url::parse(url)?;
        ensure!(parsed.origin().ascii_serialization() == self.origin, "expected origin {}, got {url}", self.origin);
        let mut target = format!("{}{}", self.backend, parsed.path());
        if let Some(query) = parsed.query() {
            target.push('?');
            target.push_str(query);
        }
        let mut request = session::client()
            .request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), target)
            .header("host", "identity.example.test")
            .header("x-forwarded-proto", "https");
        for (name, value) in headers {
            if !name.eq_ignore_ascii_case("host") && !name.eq_ignore_ascii_case("x-forwarded-proto") {
                request = request.header(*name, *value);
            }
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        let reply = Reply::from(request.send().await?).await?;
        for cookie in reply.set_cookies() {
            self.set_cookies.push(cookie.to_owned());
        }
        Ok(reply)
    }
}

pub async fn production_proxy_headers_cookies_and_issuer(t: &Ctx) -> Result<()> {
    let root = t.temp("dar-provider-proxy")?;
    let origin = "https://identity.example.test";
    let name = format!("dar-proxy-{}", &uuid()[..8]);
    instance::publish_module(&name, origin)?;
    let runtime = Instance::start(InstanceConfig {
        port: 0,
        origin: Some(origin.into()),
        root,
        database_name: name,
        development: false,
        ..InstanceConfig::default()
    })
    .await?;
    let backend = format!("http://127.0.0.1:{}", runtime.port);

    let result = async {
        let rejected = |headers: Vec<(&'static str, &'static str)>, code: &'static str| {
            let backend = backend.clone();
            async move {
                let mut req = session::client().get(format!("{backend}/oidc/.well-known/openid-configuration"));
                for (k, v) in headers {
                    req = req.header(k, v);
                }
                let reply = Reply::from(req.send().await?).await?;
                assert_eq!(reply.status, 400);
                assert_eq!(reply.json()?["error"]["code"], json!(code));
                Ok::<(), Error>(())
            }
        };

        rejected(vec![("host", "identity.example.test")], "untrustedProxy").await?;
        rejected(vec![("host", "evil.example"), ("x-forwarded-proto", "https")], "invalidHost").await?;
        rejected(vec![("host", "identity.example.test"), ("x-forwarded-proto", "https"), ("x-forwarded-host", "evil.example")], "untrustedProxy").await?;

        let mut proxy = Proxy { origin: origin.into(), backend: backend.clone(), set_cookies: Vec::new() };
        let bootstrap_token = dar_acceptance::read_to_string(&stack().bootstrap_token_file)?;
        let setup = proxy
            .send(
                "POST",
                &format!("{origin}/api/v1/auth/setup"),
                &[("origin", origin), ("content-type", "application/json")],
                Some(json!({ "username": "admin", "displayName": "Production operator", "password": PASSWORD, "bootstrapToken": bootstrap_token.trim() }).to_string()),
            )
            .await?
            .expect(200)?;
        let cookie = setup
            .set_cookies()
            .first()
            .and_then(|c| c.split(';').next())
            .context("session cookie")?
            .to_owned();
        let csrf = setup.json()?["csrfToken"].as_str().context("csrfToken")?.to_owned();

        let confirmation = proxy
            .send(
                "POST",
                &format!("{origin}/api/v1/identity/confirmation"),
                &[("cookie", &cookie), ("origin", origin), ("x-csrf-token", &csrf), ("content-type", "application/json")],
                Some(json!({ "password": PASSWORD }).to_string()),
            )
            .await?
            .expect(200)?;
        let proof = confirmation.json()?["proof"].as_str().context("proof")?.to_owned();

        let payload = json!({
            "name": "HTTPS relying party",
            "type": "confidential",
            "scopes": "openid profile",
            "redirectUris": ["https://rp.example.test/callback"],
            "postLogoutRedirectUris": [],
        });
        let mut invalid_payload = payload.clone();
        invalid_payload["redirectUris"] = json!(["http://127.0.0.1/callback"]);
        let invalid = proxy
            .send(
                "POST",
                &format!("{origin}/api/v1/identity/clients"),
                &[("cookie", &cookie), ("origin", origin), ("x-csrf-token", &csrf), ("content-type", "application/json"), ("x-identity-proof", &proof)],
                Some(invalid_payload.to_string()),
            )
            .await?;
        assert_eq!(invalid.status, 400);

        let registration = proxy
            .send(
                "POST",
                &format!("{origin}/api/v1/identity/clients"),
                &[("cookie", &cookie), ("origin", origin), ("x-csrf-token", &csrf), ("content-type", "application/json"), ("x-identity-proof", &proof)],
                Some(payload.to_string()),
            )
            .await?
            .expect(201)?;
        let client_json = registration.json()?;
        let client_id = client_json["id"].as_str().context("client id")?.to_owned();
        let client_secret = client_json["secret"].as_str().context("client secret")?.to_owned();

        let discovery = proxy.send("GET", &format!("{origin}/oidc/.well-known/openid-configuration"), &[], None).await?.expect(200)?.json()?;
        assert_eq!(discovery["issuer"], json!(format!("{origin}/oidc")));
        assert!(discovery.get("pushed_authorization_request_endpoint").is_none());

        // Real authorization code flow through the HTTPS proxy boundary.
        let verifier = dar_acceptance::base64url(&dar_acceptance::random_bytes(32));
        let challenge = {
            use sha2::Digest as _;
            dar_acceptance::base64url(&sha2::Sha256::digest(verifier.as_bytes()))
        };
        let state = dar_acceptance::base64url(&dar_acceptance::random_bytes(16));
        let nonce = dar_acceptance::base64url(&dar_acceptance::random_bytes(16));
        let redirect = "https://rp.example.test/callback";
        let mut auth_url = url::Url::parse(discovery["authorization_endpoint"].as_str().context("authorization_endpoint")?)?;
        auth_url.query_pairs_mut().extend_pairs([
            ("client_id", client_id.as_str()),
            ("redirect_uri", redirect),
            ("response_type", "code"),
            ("scope", "openid profile"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("nonce", &nonce),
            ("prompt", "consent"),
        ]);

        let mut jar: BTreeMap<String, String> = BTreeMap::new();
        if let Some((k, v)) = cookie.split_once('=') {
            jar.insert(k.to_owned(), v.to_owned());
        }
        let update_jar = |jar: &mut BTreeMap<String, String>, reply: &Reply| {
            for set_cookie in reply.set_cookies() {
                let pair = set_cookie.split(';').next().unwrap_or_default();
                let Some((k, v)) = pair.split_once('=') else { continue };
                let lower = set_cookie.to_ascii_lowercase();
                if lower.contains("max-age=0") || lower.contains("expires=thu, 01 jan 1970") {
                    jar.remove(k.trim());
                } else {
                    jar.insert(k.trim().to_owned(), v.to_owned());
                }
            }
        };
        let jar_header = |jar: &BTreeMap<String, String>| jar.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ");

        let origin_url = url::Url::parse(origin)?;
        let mut next = auth_url.to_string();
        let callback_url = loop {
            let target = url::Url::parse(&next)?;
            if target.origin() != origin_url.origin() {
                break target;
            }
            let cookie_hdr = jar_header(&jar);
            if let Some(id) = target.path().strip_prefix("/identity/interaction/").filter(|id| !id.contains('/')) {
                let endpoint = format!("{origin}/api/v1/identity/interactions/{id}");
                let detail = proxy
                    .send("GET", &endpoint, &[("cookie", &cookie_hdr), ("origin", origin), ("x-csrf-token", &csrf)], None)
                    .await?
                    .expect(200)?;
                update_jar(&mut jar, &detail);
                let interaction: Interaction = serde_json::from_slice(&detail.body)?;
                let action = if interaction.prompt == "login" { "login" } else { "approve" };
                let cookie_hdr = jar_header(&jar);
                let answered = proxy
                    .send(
                        "POST",
                        &endpoint,
                        &[("cookie", &cookie_hdr), ("origin", origin), ("x-csrf-token", &csrf), ("content-type", "application/json")],
                        Some(json!({ "action": action, "password": PASSWORD }).to_string()),
                    )
                    .await?
                    .expect(200)?;
                update_jar(&mut jar, &answered);
                next = answered.json()?["redirect"].as_str().context("interaction redirect")?.to_owned();
            } else {
                let reply = proxy.send("GET", &next, &[("cookie", &cookie_hdr)], None).await?;
                update_jar(&mut jar, &reply);
                ensure!([302, 303].contains(&reply.status), "{}: {} {}", target.path(), reply.status, reply.text());
                next = origin_url.join(reply.header("location").context("redirect location")?)?.to_string();
            }
        };

        assert_eq!(query_param(&callback_url, "state").as_deref(), Some(state.as_str()));
        assert_eq!(query_param(&callback_url, "iss").as_deref(), Some(format!("{origin}/oidc").as_str()));
        let code = query_param(&callback_url, "code").context("authorization code")?;

        let basic = format!("Basic {}", dar_acceptance::base64(format!("{client_id}:{client_secret}").as_bytes()));
        let token_endpoint = discovery["token_endpoint"].as_str().context("token_endpoint")?;
        let token_reply = proxy
            .send(
                "POST",
                token_endpoint,
                &[("authorization", &basic), ("content-type", "application/x-www-form-urlencoded")],
                Some(form(&[
                    ("grant_type", "authorization_code"),
                    ("code", &code),
                    ("redirect_uri", redirect),
                    ("code_verifier", &verifier),
                ])),
            )
            .await?
            .expect(200)?;
        let token_json = token_reply.json()?;
        let id_token = token_json["id_token"].as_str().context("id_token")?;

        let jwks_uri = discovery["jwks_uri"].as_str().context("jwks_uri")?;
        let jwks: jsonwebtoken::jwk::JwkSet = serde_json::from_slice(&proxy.send("GET", jwks_uri, &[], None).await?.expect(200)?.body)?;
        let header = jsonwebtoken::decode_header(id_token)?;
        let jwk = match &header.kid {
            Some(kid) => jwks.find(kid),
            None => jwks.keys.first(),
        }
        .context("JWKS key")?;
        let mut validation = jsonwebtoken::Validation::new(header.alg);
        validation.set_audience(&[&client_id]);
        validation.set_issuer(&[format!("{origin}/oidc")]);
        validation.leeway = 5;
        let claims = jsonwebtoken::decode::<Value>(id_token, &jsonwebtoken::DecodingKey::from_jwk(jwk)?, &validation)?.claims;
        assert_eq!(claims["nonce"], json!(nonce));
        assert_eq!(claims["iss"], json!(format!("{origin}/oidc")));

        let provider_cookies: Vec<&String> = proxy.set_cookies.iter().filter(|c| c.starts_with("dar_oidc")).collect();
        assert!(!provider_cookies.is_empty());
        for c in &provider_cookies {
            let lower = c.to_ascii_lowercase();
            assert!(lower.contains("; secure") && lower.contains("; httponly") && lower.contains("samesite=lax"), "unexpected provider cookie: {c}");
        }
        assert!(cookie.starts_with("__Host-dar_session="), "expected __Host-dar_session cookie, got {cookie}");
        Ok(())
    }
    .await;

    runtime.close().await;
    result
}
