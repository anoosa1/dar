//! The provider's HTTP endpoints, relative to the issuer path (the host mounts them at `/oidc`).

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use oxide_auth::code_grant::{accesstoken, refresh};
use serde_json::{Value, json};

use crate::oauth::{Clients, Flow, Params, PkceExtension, Records, TokenRequest};
use crate::store::{Revoke, TokenKind};
use crate::{Outcome, Provider, key, random_token};

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'";

impl Provider {
    /// Routes relative to the issuer; mount with `Router::new().nest("/oidc", provider.router())`.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/auth", get(authorize).post(authorize))
            .route("/auth/{uid}", get(resume))
            .route("/token", post(token).options(preflight))
            .route("/me", get(userinfo).post(userinfo).options(preflight))
            .route("/token/introspection", post(introspection))
            .route("/token/revocation", post(revocation).options(preflight))
            .route("/session/end", get(end_session).post(end_session))
            .route("/session/end/confirm", post(confirm_end_session))
            .with_state(self.clone())
    }
}

fn escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            '\'' => "&#39;".into(),
            c => c.to_string(),
        })
        .collect()
}

/// The same frame as Dar's protocol pages (`page` in `provider.ts`).
fn page(status: StatusCode, title: &str, content: &str, csp: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><link rel=\"stylesheet\" href=\"/app.css\"><title>{} · Dar</title></head><body><main class=\"identity-protocol\"><a class=\"brand\" href=\"/\">Dar</a><h1>{}</h1>{content}</main></body></html>",
        escape(title),
        escape(title)
    );
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CONTENT_SECURITY_POLICY, csp),
            (header::CACHE_CONTROL, "no-store"),
        ],
        html,
    )
        .into_response()
}

fn error_page(message: &str) -> Response {
    page(
        StatusCode::BAD_REQUEST,
        "Unable to continue",
        &format!("<p>{}</p><a href=\"/\">Return to Dar</a>", escape(message)),
        CSP,
    )
}

fn json_response(status: StatusCode, body: Value) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        axum::Json(body),
    )
        .into_response()
}

fn pairs(raw: &[u8]) -> Vec<(String, String)> {
    url::form_urlencoded::parse(raw)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// The provider session cookie, also one the TypeScript stack signed before the instance moved to
/// this one: that is re-signed in this provider's form, so the session simply continues.
fn session_cookie(headers: &HeaderMap, p: &Provider) -> Option<String> {
    let value = cookie(headers, "dar_oidc")?;
    if p.verify_cookie(&value).is_some() {
        return Some(value);
    }
    let signature = cookie(headers, "dar_oidc.sig")?;
    p.legacy_cookie_valid("dar_oidc", &value, &signature)
        .then(|| p.sign_cookie(&value))
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

impl Provider {
    fn cookie_attributes(&self, path: &str, max_age: i64) -> String {
        let secure = if self.config().development {
            ""
        } else {
            "; Secure"
        };
        format!("Path={path}; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}")
    }

    fn issuer_path(&self) -> String {
        url::Url::parse(&self.config().issuer())
            .map(|u| u.path().to_owned())
            .unwrap_or_else(|_| "/oidc".into())
    }

    fn render(&self, outcome: Outcome, new_session: Option<String>) -> Response {
        let mut response = match outcome {
            Outcome::Page(message) => error_page(&message),
            Outcome::Interact { uid } => {
                let interaction_url = self.config().interaction_url(&uid);
                let signed = self.sign_cookie(&uid);
                let resume_path = format!("{}/auth/{uid}", self.issuer_path());
                let resume_cookie = format!(
                    "dar_oidc_interaction={signed}; {}",
                    self.cookie_attributes(&resume_path, self.config().lifetimes.interaction)
                );
                let mut res = (
                    StatusCode::SEE_OTHER,
                    [
                        (header::LOCATION, interaction_url.clone()),
                        (header::SET_COOKIE, resume_cookie),
                    ],
                )
                    .into_response();
                if let Ok(parsed) = url::Url::parse(&interaction_url)
                    && parsed.path() != resume_path
                {
                    let page_cookie = format!(
                        "dar_oidc_interaction={signed}; {}",
                        self.cookie_attributes(parsed.path(), self.config().lifetimes.interaction)
                    );
                    if let Ok(value) = HeaderValue::from_str(&page_cookie) {
                        res.headers_mut().append(header::SET_COOKIE, value);
                    }
                }
                res
            }
            Outcome::Respond {
                redirect,
                form_post: false,
                ..
            } => (
                StatusCode::SEE_OTHER,
                [(header::LOCATION, redirect.to_string())],
            )
                .into_response(),
            Outcome::Respond {
                redirect,
                form_post: true,
                params,
            } => {
                let fields: String = params
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
                            escape(k),
                            escape(v)
                        )
                    })
                    .collect();
                let origin = redirect.origin().ascii_serialization();
                let csp = format!(
                    "default-src 'none'; script-src 'self'; style-src 'self'; form-action {origin}; base-uri 'none'; frame-ancestors 'none'"
                );
                let content = format!(
                    "<form id=\"oidc-response\" method=\"post\" action=\"{}\">{fields}<button class=\"primary-button\" type=\"submit\">Continue</button></form><script src=\"/oidc-form.js\" defer></script>",
                    escape(redirect.as_str())
                );
                page(StatusCode::OK, "Continue sign-in", &content, &csp)
            }
        };
        if let Some(uid) = new_session {
            // As oidc-provider sets it (no path given): the whole site, so Dar's pages see the session too.
            let value = format!(
                "dar_oidc={}; {}",
                self.sign_cookie(&uid),
                self.cookie_attributes("/", self.config().lifetimes.session)
            );
            if let Ok(value) = HeaderValue::from_str(&value) {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
        }
        response
    }

    /// Allowed when a public client redirects to this origin (`clientBasedCORS`).
    fn cors_allowed(&self, origin: &str, client_id: Option<&str>) -> bool {
        let state = self.lock();
        state
            .clients
            .values()
            .filter(|c| {
                c.enabled
                    && c.kind == crate::ClientKind::Public
                    && client_id.is_none_or(|id| id == c.id)
            })
            .any(|c| {
                c.redirect_uris.iter().any(|u| {
                    url::Url::parse(u).is_ok_and(|u| u.origin().ascii_serialization() == origin)
                })
            })
    }

    fn with_cors(
        &self,
        mut response: Response,
        headers: &HeaderMap,
        client_id: Option<&str>,
    ) -> Response {
        if let Some(origin) = header_str(headers, header::ORIGIN)
            && self.cors_allowed(origin, client_id)
            && let Ok(value) = HeaderValue::from_str(origin)
        {
            response
                .headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
            response
                .headers_mut()
                .insert(header::VARY, HeaderValue::from_static("Origin"));
        }
        response
    }
}

async fn discovery(State(p): State<Provider>) -> Response {
    let issuer = p.config().issuer();
    let body = json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/auth"),
        "token_endpoint": format!("{issuer}/token"),
        "userinfo_endpoint": format!("{issuer}/me"),
        "jwks_uri": format!("{issuer}/jwks"),
        "end_session_endpoint": format!("{issuer}/session/end"),
        "revocation_endpoint": format!("{issuer}/token/revocation"),
        "introspection_endpoint": format!("{issuer}/token/introspection"),
        "response_types_supported": ["code"],
        "response_modes_supported": ["query", "form_post"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "none"],
        "introspection_endpoint_auth_methods_supported": ["client_secret_basic", "none"],
        "revocation_endpoint_auth_methods_supported": ["client_secret_basic", "none"],
        "code_challenge_methods_supported": ["S256"],
        "scopes_supported": crate::config::SCOPES.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        "claims_supported": ["sub", "name", "preferred_username", "given_name", "family_name", "email", "email_verified", "groups", "iss", "aud", "exp", "iat", "auth_time", "nonce", "at_hash"],
        "claim_types_supported": ["normal"],
        "claims_parameter_supported": false,
        "request_parameter_supported": false,
        "request_uri_parameter_supported": false,
        "authorization_response_iss_parameter_supported": true,
    });
    (
        [(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        axum::Json(body),
    )
        .into_response()
}

async fn jwks(State(p): State<Provider>) -> Response {
    let body = p.lock().keys.jwks();
    (
        [
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (header::CONTENT_TYPE, "application/jwk-set+json"),
        ],
        body.to_string(),
    )
        .into_response()
}

async fn authorize(
    State(p): State<Provider>,
    method: Method,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    let raw = if method == Method::POST {
        body.to_vec()
    } else {
        query.unwrap_or_default().into_bytes()
    };
    if raw.len() > 8192 {
        return error_page("The sign-in request is too large.");
    }
    let session = session_cookie(&headers, &p);
    let (outcome, new_session) = p.authorize(Params::parse(pairs(&raw)), session.as_deref(), None);
    p.render(outcome, new_session)
}

/// The browser returns here from Dar's sign-in page.
async fn resume(
    State(p): State<Provider>,
    headers: HeaderMap,
    Path(uid): Path<String>,
) -> Response {
    let bound = cookie(&headers, "dar_oidc_interaction")
        .and_then(|c| p.verify_cookie(&c))
        .is_some_and(|v| v == uid);
    let interaction = {
        let mut state = p.lock();
        let found = if bound {
            state.store.interaction(&key(&uid))
        } else {
            None
        };
        if found.as_ref().is_some_and(|i| i.result.is_some()) {
            state.store.delete_interaction(&key(&uid));
        }
        found
    };
    let Some(interaction) = interaction else {
        return error_page(
            "This sign-in request expired. Return to the application and try again.",
        );
    };
    if interaction.result.is_none() {
        return (
            StatusCode::SEE_OTHER,
            [(header::LOCATION, p.config().interaction_url(&uid))],
        )
            .into_response();
    }
    let session = session_cookie(&headers, &p);
    let params = Params::parse(interaction.params.clone());
    let (outcome, new_session) = p.authorize(params, session.as_deref(), Some(interaction));
    let mut response = p.render(outcome, new_session);
    // Clear the finished interaction's cookie before any new interaction's Set-Cookie, so
    // cookie jars that key only by name do not let the old clear erase the new binding.
    let clear = format!(
        "dar_oidc_interaction=; {}",
        p.cookie_attributes(&format!("{}/auth/{uid}", p.issuer_path()), 0)
    );
    if let Ok(value) = HeaderValue::from_str(&clear) {
        let existing: Vec<HeaderValue> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .cloned()
            .collect();
        response.headers_mut().remove(header::SET_COOKIE);
        response.headers_mut().append(header::SET_COOKIE, value);
        for v in existing {
            response.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    response
}

fn token_error(error: &str, description: &str, status: StatusCode) -> Response {
    let mut response = json_response(
        status,
        json!({"error": error, "error_description": description}),
    );
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"dar\""),
        );
    }
    response
}

fn bearer_json(raw: &str, id_token: Option<String>) -> Value {
    let mut body: Value = serde_json::from_str(raw).unwrap_or_else(|_| json!({}));
    body["token_type"] = json!("Bearer");
    if let Some(id_token) = id_token {
        body["id_token"] = json!(id_token);
    }
    body
}

async fn token(State(p): State<Provider>, headers: HeaderMap, body: Bytes) -> Response {
    let request = TokenRequest::new(
        Params::parse(pairs(&body)),
        header_str(&headers, header::AUTHORIZATION),
    );
    let client_id = request.client_id().map(str::to_owned);
    if client_id.is_none() {
        return token_error(
            "invalid_client",
            "Client authentication failed.",
            StatusCode::UNAUTHORIZED,
        );
    }
    let response = {
        let mut guard = p.lock();
        let state = &mut *guard;
        let client = client_id
            .as_deref()
            .and_then(|id| state.clients.get(id))
            .cloned();
        let mut flow = Flow {
            clients: Clients(&state.clients),
            records: Records {
                store: state.store.as_mut(),
                directory: p.inner.directory.as_ref(),
                clients: &state.clients,
                keys: &state.keys,
                config: &p.inner.config,
                approving: None,
                redeemed: None,
                rejected_code: false,
                id_token: None,
                replayed_grant: Default::default(),
            },
            pkce: PkceExtension::for_client(client.as_ref()),
        };
        match request.form.get("grant_type") {
            Some("authorization_code") => match accesstoken::access_token(&mut flow, &request) {
                Ok(bearer) => json_response(
                    StatusCode::OK,
                    bearer_json(&bearer.to_json(), flow.records.id_token.take()),
                ),
                Err(accesstoken::Error::Invalid(_)) if flow.records.rejected_code => token_error(
                    "invalid_grant",
                    "The authorization code is invalid, expired or already used.",
                    StatusCode::BAD_REQUEST,
                ),
                Err(accesstoken::Error::Invalid(description)) => json_response(
                    StatusCode::BAD_REQUEST,
                    serde_json::from_str(&description.to_json()).unwrap_or_default(),
                ),
                Err(accesstoken::Error::Unauthorized(..)) => token_error(
                    "invalid_client",
                    "Client authentication failed.",
                    StatusCode::UNAUTHORIZED,
                ),
                Err(accesstoken::Error::Primitive(_)) => token_error(
                    "server_error",
                    "The request could not be completed.",
                    StatusCode::INTERNAL_SERVER_ERROR,
                ),
            },
            Some("refresh_token") => {
                // Refresh tokens are bound to the client that received them.
                let presented = request
                    .form
                    .get("refresh_token")
                    .and_then(|t| flow.records.store.token(&key(t)));
                if presented.is_some_and(|t| Some(t.client_id.as_str()) != client_id.as_deref()) {
                    token_error(
                        "invalid_grant",
                        "The refresh token is invalid.",
                        StatusCode::BAD_REQUEST,
                    )
                } else {
                    let result = refresh::refresh(&mut flow, &request);
                    if let Some(grant_id) = flow.records.replayed_grant.take() {
                        flow.records.store.revoke(Revoke::Grant(grant_id));
                    }
                    match result {
                        Ok(bearer) => json_response(
                            StatusCode::OK,
                            bearer_json(&bearer.to_json(), flow.records.id_token.take()),
                        ),
                        Err(refresh::Error::Invalid(description)) => json_response(
                            StatusCode::BAD_REQUEST,
                            serde_json::from_str(&description.to_json()).unwrap_or_default(),
                        ),
                        Err(refresh::Error::Unauthorized(..)) => token_error(
                            "invalid_client",
                            "Client authentication failed.",
                            StatusCode::UNAUTHORIZED,
                        ),
                        Err(refresh::Error::Primitive) => token_error(
                            "invalid_grant",
                            "The refresh token is invalid.",
                            StatusCode::BAD_REQUEST,
                        ),
                    }
                }
            }
            Some(_) => token_error(
                "unsupported_grant_type",
                "Only authorization_code and refresh_token are supported.",
                StatusCode::BAD_REQUEST,
            ),
            None => token_error(
                "invalid_request",
                "grant_type is required.",
                StatusCode::BAD_REQUEST,
            ),
        }
    };
    p.with_cors(response, &headers, client_id.as_deref())
}

async fn preflight(State(p): State<Provider>, headers: HeaderMap) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Some(origin) = header_str(&headers, header::ORIGIN)
        && p.cors_allowed(origin, None)
        && let Ok(value) = HeaderValue::from_str(origin)
    {
        let h = response.headers_mut();
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("authorization, content-type"),
        );
        h.insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    response
}

async fn userinfo(State(p): State<Provider>, headers: HeaderMap) -> Response {
    let invalid = || {
        let mut r = json_response(StatusCode::UNAUTHORIZED, json!({"error": "invalid_token"}));
        r.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer error=\"invalid_token\""),
        );
        r
    };
    let Some(token) =
        header_str(&headers, header::AUTHORIZATION).and_then(|h| h.strip_prefix("Bearer "))
    else {
        return invalid();
    };
    let record = p
        .lock()
        .store
        .token(&key(token.trim()))
        .filter(|t| t.kind == TokenKind::Access);
    let directory = &p.inner.directory;
    let Some(record) = record.filter(|t| {
        directory
            .native_session(&t.native_session_id)
            .is_some_and(|s| s.account_id == t.account_id)
    }) else {
        return invalid();
    };
    let Some(account) = directory.account(&record.account_id) else {
        return invalid();
    };
    let scopes: Vec<&str> = record.scope.split(' ').collect();
    let mut claims = json!({"sub": account.id});
    if scopes.contains(&"profile") {
        claims["name"] = json!(account.display_name);
        claims["preferred_username"] = json!(account.user_name);
        if let Some(v) = &account.given_name {
            claims["given_name"] = json!(v);
        }
        if let Some(v) = &account.family_name {
            claims["family_name"] = json!(v);
        }
    }
    if scopes.contains(&"email")
        && let Some(email) = &account.email
    {
        // Dar never verifies email addresses.
        claims["email"] = json!(email);
        claims["email_verified"] = json!(false);
    }
    if scopes.contains(&"groups") {
        claims["groups"] = json!(account.groups);
    }
    let response = json_response(StatusCode::OK, claims);
    p.with_cors(response, &headers, Some(&record.client_id))
}

async fn introspection(State(p): State<Provider>, headers: HeaderMap, body: Bytes) -> Response {
    let request = TokenRequest::new(
        Params::parse(pairs(&body)),
        header_str(&headers, header::AUTHORIZATION),
    );
    let state = p.lock();
    let Some(client) = request.client(&Clients(&state.clients)) else {
        return token_error(
            "invalid_client",
            "Client authentication failed.",
            StatusCode::UNAUTHORIZED,
        );
    };
    let Some(token) = request.form.get("token") else {
        return token_error(
            "invalid_request",
            "token is required.",
            StatusCode::BAD_REQUEST,
        );
    };
    let directory = &p.inner.directory;
    let record = state.store.token(&key(token)).filter(|t| {
        t.client_id == client.id
            && !t.consumed
            && state.store.grant(&t.grant_id).is_some()
            && directory
                .native_session(&t.native_session_id)
                .is_some_and(|s| s.account_id == t.account_id)
            && directory.account(&t.account_id).is_some()
    });
    let body = match record {
        None => json!({"active": false}),
        Some(t) => json!({
            "active": true, "client_id": t.client_id, "sub": t.account_id, "scope": t.scope,
            "iat": t.issued_at / 1000, "exp": t.expires_at / 1000, "iss": p.config().issuer(),
            "token_type": if t.kind == TokenKind::Access { "access_token" } else { "refresh_token" },
        }),
    };
    json_response(StatusCode::OK, body)
}

async fn revocation(State(p): State<Provider>, headers: HeaderMap, body: Bytes) -> Response {
    let request = TokenRequest::new(
        Params::parse(pairs(&body)),
        header_str(&headers, header::AUTHORIZATION),
    );
    let client_id = {
        let mut guard = p.lock();
        let state = &mut *guard;
        let Some(client) = request
            .client(&Clients(&state.clients))
            .map(|c| c.id.clone())
        else {
            return token_error(
                "invalid_client",
                "Client authentication failed.",
                StatusCode::UNAUTHORIZED,
            );
        };
        let Some(token) = request.form.get("token") else {
            return token_error(
                "invalid_request",
                "token is required.",
                StatusCode::BAD_REQUEST,
            );
        };
        if let Some(record) = state
            .store
            .token(&key(token))
            .filter(|t| t.client_id == client)
        {
            match record.kind {
                // Revoking a refresh token ends its whole grant (RFC 7009 §2.1).
                TokenKind::Refresh => state.store.revoke(Revoke::Grant(record.grant_id)),
                TokenKind::Access => state.store.delete_token(&key(token)),
            }
        }
        client
    };
    p.with_cors(StatusCode::OK.into_response(), &headers, Some(&client_id))
}

/// RP-initiated logout: confirm, then end the provider session and its grants.
async fn end_session(
    State(p): State<Provider>,
    method: Method,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    let raw = if method == Method::POST {
        body.to_vec()
    } else {
        query.unwrap_or_default().into_bytes()
    };
    let params = Params::parse(pairs(&raw));
    if !params.valid {
        return error_page("The sign-out request is invalid.");
    }
    let issuer = p.config().issuer();
    let hinted = match params.get("id_token_hint") {
        None => None,
        Some(hint) => match p.lock().keys.verify_hint(hint, &issuer) {
            Some(claims) => claims["aud"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| claims["aud"][0].as_str().map(str::to_owned)),
            None => return error_page("The sign-out request has an invalid id_token_hint."),
        },
    };
    let client_id = match (hinted, params.get("client_id")) {
        (Some(a), Some(b)) if a != b => {
            return error_page("The sign-out request names two different applications.");
        }
        (Some(a), _) => Some(a),
        (None, b) => b.map(str::to_owned),
    };
    let target = params.get("post_logout_redirect_uri");
    if let Some(uri) = target {
        let registered = client_id
            .as_deref()
            .and_then(|id| p.lock().clients.get(id).cloned())
            .is_some_and(|c| c.enabled && c.post_logout_redirect_uris.iter().any(|u| u == uri));
        if !registered {
            return error_page("The application's sign-out address is not registered.");
        }
    }
    let xsrf = random_token(24);
    let mut fields = format!("<input type=\"hidden\" name=\"xsrf\" value=\"{xsrf}\">");
    for (name, value) in [
        ("post_logout_redirect_uri", target),
        ("state", params.get("state")),
        ("client_id", client_id.as_deref()),
    ] {
        if let Some(value) = value {
            fields.push_str(&format!(
                "<input type=\"hidden\" name=\"{name}\" value=\"{}\">",
                escape(value)
            ));
        }
    }
    let content = format!(
        "<p>This ends the provider session and its application grants. Your Dar browser session stays signed in.</p><form id=\"op.logoutForm\" method=\"post\" action=\"{}/session/end/confirm\">{fields}</form><div class=\"dialog-actions\"><a class=\"secondary-button\" href=\"/settings\">Cancel</a><button class=\"primary-button\" type=\"submit\" form=\"op.logoutForm\" name=\"logout\" value=\"yes\">Sign out</button></div>",
        escape(&issuer)
    );
    let origin = target
        .and_then(|u| url::Url::parse(u).ok())
        .map(|u| format!(" {}", u.origin().ascii_serialization()))
        .unwrap_or_default();
    let csp = format!(
        "default-src 'none'; script-src 'self'; style-src 'self'; form-action 'self'{origin}; base-uri 'none'; frame-ancestors 'none'"
    );
    let mut response = page(
        StatusCode::OK,
        "Sign out of connected applications?",
        &content,
        &csp,
    );
    let cookie = format!(
        "dar_oidc_logout={}; {}",
        p.sign_cookie(&xsrf),
        p.cookie_attributes(&format!("{}/session/end", p.issuer_path()), 600)
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

async fn confirm_end_session(
    State(p): State<Provider>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let params = Params::parse(pairs(&body));
    let expected = cookie(&headers, "dar_oidc_logout").and_then(|c| p.verify_cookie(&c));
    if !params.valid || expected.is_none() || expected.as_deref() != params.get("xsrf") {
        return error_page("The sign-out request expired. Try again.");
    }
    if params.get("logout") != Some("yes") {
        return error_page("Sign-out was not confirmed.");
    }
    if let Some(uid) = session_cookie(&headers, &p).and_then(|c| p.verify_cookie(&c)) {
        p.revoke(Revoke::Session(key(&uid)));
    }
    let mut response = match params.get("post_logout_redirect_uri") {
        Some(uri) => {
            let registered = params
                .get("client_id")
                .and_then(|id| p.lock().clients.get(id).cloned())
                .is_some_and(|c| c.enabled && c.post_logout_redirect_uris.iter().any(|u| u == uri));
            match url::Url::parse(uri) {
                Ok(mut url) if registered => {
                    if let Some(state) = params.get("state") {
                        url.query_pairs_mut().append_pair("state", state);
                    }
                    (StatusCode::SEE_OTHER, [(header::LOCATION, url.to_string())]).into_response()
                }
                _ => error_page("The application's sign-out address is not registered."),
            }
        }
        None => page(
            StatusCode::OK,
            "Signed out",
            "<p>The provider session has ended.</p><a href=\"/\">Return to Dar</a>",
            CSP,
        ),
    };
    for (name, path) in [
        ("dar_oidc", "/".to_owned()),
        ("dar_oidc.sig", "/".to_owned()),
        ("dar_oidc", p.issuer_path()),
        (
            "dar_oidc_logout",
            format!("{}/session/end", p.issuer_path()),
        ),
    ] {
        if let Ok(value) =
            HeaderValue::from_str(&format!("{name}=; {}", p.cookie_attributes(&path, 0)))
        {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}
