//! A small OpenID Connect relying party for suites (what `openid-client` did): discovery, the
//! authorization URL with PKCE, the code grant with ID token validation against the JWKS,
//! refresh, UserInfo and RP-initiated logout.

use crate::identity::{CookieJar, Followed, IdentityAdmin, follow};
use crate::session::{Reply, Session, client};
use crate::{Error, Result, Value, bail, ensure, json};

/// A registered client and the provider's discovery document.
#[derive(Debug, Clone)]
pub struct Client {
    pub id: String,
    pub secret: Option<String>,
    pub redirect: String,
    /// What the client was registered with (for suites that compare or update it).
    pub payload: Value,
    pub metadata: Value,
}

/// The token response, with the ID token's verified claims.
#[derive(Debug, Clone)]
pub struct Tokens {
    pub raw: Value,
    pub claims: Option<Value>,
}

impl Tokens {
    pub fn access_token(&self) -> &str {
        self.raw["access_token"].as_str().unwrap_or_default()
    }
    pub fn refresh_token(&self) -> Option<&str> {
        self.raw["refresh_token"].as_str()
    }
    pub fn id_token(&self) -> Option<&str> {
        self.raw["id_token"].as_str()
    }
}

/// An authorization in progress: where Dar sent the browser, and the values to check.
#[derive(Debug, Clone)]
pub struct Flow {
    pub url: url::Url,
    pub jar: CookieJar,
    pub verifier: String,
    pub state: String,
    pub nonce: String,
    pub pkce: bool,
}

/// Options for [`authorize`]; the defaults are PKCE, `prompt=consent` and the usual scopes.
#[derive(Debug, Clone, Default)]
pub struct Authorize {
    pub no_pkce: bool,
    pub deny: bool,
    pub scope: Option<String>,
    pub prompt: Option<String>,
    pub params: Vec<(String, String)>,
    pub jar: Option<CookieJar>,
}

/// The discovery document at `<issuer>/.well-known/openid-configuration`.
pub async fn discovery(issuer: &str) -> Result<Value> {
    let response = client()
        .get(format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        ))
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "discovery at {issuer}: {}",
        response.status()
    );
    let metadata: Value = response.json().await?;
    ensure!(
        metadata["issuer"] == issuer.trim_end_matches('/'),
        "discovery issuer {} is not {issuer}",
        metadata["issuer"]
    );
    Ok(metadata)
}

impl Client {
    /// A client for an existing registration at `base`'s provider.
    pub async fn discover(
        base: &str,
        id: &str,
        secret: Option<&str>,
        redirect: &str,
    ) -> Result<Client> {
        Ok(Client {
            id: id.into(),
            secret: secret.map(Into::into),
            redirect: redirect.into(),
            payload: Value::Null,
            metadata: discovery(&format!("{base}/oidc")).await?,
        })
    }

    /// The authorization URL for `params` (state, nonce, PKCE…).
    pub fn authorization_url(&self, params: &[(String, String)]) -> Result<url::Url> {
        let mut url = url::Url::parse(
            self.metadata["authorization_endpoint"]
                .as_str()
                .unwrap_or_default(),
        )?;
        url.query_pairs_mut()
            .append_pair("client_id", &self.id)
            .extend_pairs(params);
        Ok(url)
    }

    fn authenticate(
        &self,
        request: reqwest::RequestBuilder,
        form: &mut Vec<(String, String)>,
    ) -> reqwest::RequestBuilder {
        match &self.secret {
            Some(secret) => request.basic_auth(&self.id, Some(secret)),
            None => {
                form.push(("client_id".into(), self.id.clone()));
                request
            }
        }
    }

    /// A token endpoint request with the client's authentication.
    pub async fn token_request(&self, mut form: Vec<(String, String)>) -> Result<Reply> {
        let endpoint = self.metadata["token_endpoint"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let request = self.authenticate(client().post(&endpoint), &mut form);
        Reply::from(request.form(&form).send().await?).await
    }

    /// Verifies an ID token's signature (JWKS), issuer, audience and expiry.
    pub async fn verify_id_token(&self, token: &str) -> Result<Value> {
        let header = jsonwebtoken::decode_header(token)?;
        let jwks: jsonwebtoken::jwk::JwkSet = client()
            .get(self.metadata["jwks_uri"].as_str().unwrap_or_default())
            .send()
            .await?
            .json()
            .await?;
        let jwk = match &header.kid {
            Some(kid) => jwks.find(kid),
            None => jwks.keys.first(),
        }
        .ok_or_else(|| Error::new("no JWKS key for the ID token"))?;
        let mut validation = jsonwebtoken::Validation::new(header.alg);
        validation.set_audience(&[&self.id]);
        validation.set_issuer(&[self.metadata["issuer"].as_str().unwrap_or_default()]);
        validation.leeway = 5;
        let data = jsonwebtoken::decode::<Value>(
            token,
            &jsonwebtoken::DecodingKey::from_jwk(jwk)?,
            &validation,
        )?;
        Ok(data.claims)
    }

    /// Redeems the code `flow` ended with (`authorizationCodeGrant`): checks `state` and `iss`,
    /// sends the PKCE verifier, and validates the ID token and its `nonce`.
    pub async fn exchange(&self, flow: &Flow) -> Result<Tokens> {
        let query: std::collections::HashMap<String, String> =
            flow.url.query_pairs().into_owned().collect();
        if let Some(error) = query.get("error") {
            bail!(
                "authorization failed: {error} {}",
                query.get("error_description").cloned().unwrap_or_default()
            );
        }
        ensure!(query.get("state") == Some(&flow.state), "state mismatch");
        if let Some(iss) = query.get("iss") {
            ensure!(
                self.metadata["issuer"] == iss.as_str(),
                "iss mismatch: {iss}"
            );
        }
        let code = query
            .get("code")
            .ok_or_else(|| Error::new("no code in the callback"))?;
        let mut form = vec![
            ("grant_type".into(), "authorization_code".into()),
            ("code".into(), code.clone()),
            ("redirect_uri".into(), self.redirect.clone()),
        ];
        if flow.pkce {
            form.push(("code_verifier".into(), flow.verifier.clone()));
        }
        let reply = self.token_request(form).await?;
        ensure!(
            reply.status == 200,
            "code grant: {} {}",
            reply.status,
            reply.text()
        );
        let raw = reply.json()?;
        let claims = match raw["id_token"].as_str() {
            Some(token) => {
                let claims = self.verify_id_token(token).await?;
                ensure!(claims["nonce"] == flow.nonce.as_str(), "nonce mismatch");
                Some(claims)
            }
            None => None,
        };
        Ok(Tokens { raw, claims })
    }

    /// `refreshTokenGrant`.
    pub async fn refresh(&self, refresh_token: &str) -> Result<Tokens> {
        let reply = self
            .token_request(vec![
                ("grant_type".into(), "refresh_token".into()),
                ("refresh_token".into(), refresh_token.into()),
            ])
            .await?;
        ensure!(
            reply.status == 200,
            "refresh grant: {} {}",
            reply.status,
            reply.text()
        );
        let raw = reply.json()?;
        let claims = match raw["id_token"].as_str() {
            Some(token) => Some(self.verify_id_token(token).await?),
            None => None,
        };
        Ok(Tokens { raw, claims })
    }

    /// `fetchUserInfo` with an access token; fails unless 200.
    pub async fn user_info(&self, access_token: &str) -> Result<Value> {
        let reply = Reply::from(
            client()
                .get(
                    self.metadata["userinfo_endpoint"]
                        .as_str()
                        .unwrap_or_default(),
                )
                .bearer_auth(access_token)
                .send()
                .await?,
        )
        .await?;
        ensure!(
            reply.status == 200,
            "userinfo: {} {}",
            reply.status,
            reply.text()
        );
        reply.json()
    }

    /// The RP-initiated logout URL (`buildEndSessionUrl`).
    pub fn end_session_url(&self, params: &[(&str, &str)]) -> Result<url::Url> {
        let mut url = url::Url::parse(
            self.metadata["end_session_endpoint"]
                .as_str()
                .unwrap_or_default(),
        )?;
        url.query_pairs_mut()
            .append_pair("client_id", &self.id)
            .extend_pairs(params);
        Ok(url)
    }
}

impl IdentityAdmin {
    /// Registers a relying party (`client()` in `identity-helpers.ts`) and discovers the provider.
    pub async fn client(&self, kind: &str, scopes: &str, redirect: &str) -> Result<Client> {
        let base = self.session.base.clone();
        let payload = json!({"name": "Acceptance relying party", "type": kind, "redirectUris": [redirect], "postLogoutRedirectUris": ["http://127.0.0.1:49999/signed-out"], "scopes": scopes, "enabled": true});
        let reply = self.write("/clients", "POST", Some(&payload), None).await?;
        ensure!(
            reply.status == 201,
            "registering a client: {} {}",
            reply.status,
            reply.text()
        );
        let value = reply.json()?;
        let mut client = Client::discover(
            &base,
            value["id"].as_str().unwrap_or_default(),
            value["secret"].as_str(),
            redirect,
        )
        .await?;
        client.payload = payload;
        Ok(client)
    }

    /// A confidential client with the usual scopes and callback.
    pub async fn default_client(&self) -> Result<Client> {
        self.client(
            "confidential",
            "openid profile email groups offline_access",
            "http://127.0.0.1:49999/callback",
        )
        .await
    }
}

/// Starts an authorization for `client` as `session` and follows it out of Dar (`authorize`).
pub async fn authorize(
    base: &str,
    session: &Session,
    client: &Client,
    options: Authorize,
) -> Result<Flow> {
    let verifier = crate::base64url(&crate::random_bytes(32));
    let challenge = {
        use sha2::Digest as _;
        crate::base64url(&sha2::Sha256::digest(verifier.as_bytes()))
    };
    let (state, nonce) = (
        crate::base64url(&crate::random_bytes(16)),
        crate::base64url(&crate::random_bytes(16)),
    );
    let mut params = vec![
        ("redirect_uri".to_owned(), client.redirect.clone()),
        ("response_type".to_owned(), "code".to_owned()),
        (
            "scope".to_owned(),
            options
                .scope
                .clone()
                .unwrap_or_else(|| "openid profile email groups offline_access".into()),
        ),
    ];
    if !options.no_pkce {
        params.push(("code_challenge".into(), challenge));
        params.push(("code_challenge_method".into(), "S256".into()));
    }
    params.push(("state".into(), state.clone()));
    params.push(("nonce".into(), nonce.clone()));
    params.push((
        "prompt".into(),
        options.prompt.clone().unwrap_or_else(|| "consent".into()),
    ));
    params.extend(options.params.iter().cloned());
    let start = client.authorization_url(&params)?;
    let Followed { url, jar } = follow(
        base,
        session,
        start.as_str(),
        options.deny,
        options.jar.clone(),
        |_| {},
    )
    .await?;
    Ok(Flow {
        url,
        jar,
        verifier,
        state,
        nonce,
        pkce: !options.no_pkce,
    })
}
