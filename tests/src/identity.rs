//! Identity administration for suites (`identity-helpers.ts`): the admin session with a fresh
//! password proof for writes, SCIM credentials, and a cookie jar that follows Dar's sign-in
//! and consent redirects. The OpenID relying party that exchanges codes lives in [`crate::oidc`].

use crate::session::{Reply, Session, client, login};
use crate::{PASSWORD, Result, Value, ensure, json};
use std::collections::BTreeMap;

/// The administrator's identity API (`/api/v1/identity`), with a password proof for writes.
pub struct IdentityAdmin {
    pub session: Session,
    pub proof: String,
}

impl IdentityAdmin {
    /// Signs in as `admin` and confirms the password (`identityAdmin`).
    pub async fn new(base: &str) -> Result<IdentityAdmin> {
        let session = login(base).await?;
        let reply = Reply::from(session.request("POST", "/api/v1/identity/confirmation").json(&json!({"password": PASSWORD})).send().await?).await?;
        ensure!(reply.status == 200, "identity confirmation: {} {}", reply.status, reply.text());
        let proof = reply.json()?["proof"].as_str().unwrap_or_default().to_owned();
        Ok(IdentityAdmin { session, proof })
    }

    /// A request to `/api/v1/identity{path}` without the proof.
    pub async fn api(&self, path: &str, method: &str, payload: Option<&Value>) -> Result<Reply> {
        self.send(path, method, payload, &[]).await
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        let reply = self.api(path, "GET", None).await?;
        ensure!(reply.status == 200, "GET identity{path}: {} {}", reply.status, reply.text());
        reply.json()
    }

    /// A write with the password proof, and `if-match` when given.
    pub async fn write(&self, path: &str, method: &str, payload: Option<&Value>, if_match: Option<&str>) -> Result<Reply> {
        let mut extra = vec![("x-identity-proof", self.proof.clone())];
        if let Some(tag) = if_match {
            extra.push(("if-match", tag.to_owned()));
        }
        self.send(path, method, payload, &extra).await
    }

    async fn send(&self, path: &str, method: &str, payload: Option<&Value>, extra: &[(&str, String)]) -> Result<Reply> {
        let mut request = self.session.request(method, &format!("/api/v1/identity{path}")).header("content-type", "application/json");
        for (name, value) in extra {
            request = request.header(*name, value);
        }
        if let Some(payload) = payload {
            request = request.body(payload.to_string());
        }
        Reply::from(request.send().await?).await
    }

    /// A SCIM credential with `scope` (`write` or `read`), valid for a day.
    pub async fn credential(&self, scope: &str) -> Result<(String, String)> {
        let reply = self.write("/credentials", "POST", Some(&json!({"label": "Acceptance SCIM", "scope": scope, "days": 1})), None).await?;
        ensure!(reply.status == 201, "creating a SCIM credential: {} {}", reply.status, reply.text());
        let value = reply.json()?;
        Ok((value["id"].as_str().unwrap_or_default().to_owned(), value["secret"].as_str().unwrap_or_default().to_owned()))
    }
}

/// Cookies kept across requests that do not follow redirects (`CookieJar`).
#[derive(Debug, Clone, Default)]
pub struct CookieJar {
    pub cookies: BTreeMap<String, String>,
}

impl CookieJar {
    /// A jar holding a `name=value` cookie (a session's), or nothing.
    pub fn new(cookie: Option<&str>) -> Self {
        let mut jar = Self::default();
        if let Some((name, value)) = cookie.and_then(|c| c.split_once('=')) {
            jar.cookies.insert(name.to_owned(), value.to_owned());
        }
        jar
    }

    pub fn header(&self) -> String {
        self.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
    }

    /// Sends a request with the jar's cookies, without following redirects, and keeps the
    /// cookies the response sets (deleting expired ones).
    pub async fn fetch(&mut self, request: reqwest::RequestBuilder) -> Result<Reply> {
        let request = if self.cookies.is_empty() { request } else { request.header("cookie", self.header()) };
        let reply = Reply::from(request.send().await?).await?;
        for cookie in reply.set_cookies() {
            let pair = cookie.split(';').next().unwrap_or_default();
            let Some((name, value)) = pair.split_once('=') else { continue };
            let lower = cookie.to_ascii_lowercase();
            if lower.contains("max-age=0") || lower.contains("expires=thu, 01 jan 1970") {
                self.cookies.remove(name.trim());
            } else {
                self.cookies.insert(name.trim().to_owned(), value.to_owned());
            }
        }
        Ok(reply)
    }

    pub async fn get(&mut self, url: &str) -> Result<Reply> {
        self.fetch(client().get(url)).await
    }
}

/// Where an authorization request left Dar, and the cookies it collected on the way.
#[derive(Debug, Clone)]
pub struct Followed {
    pub url: url::Url,
    pub jar: CookieJar,
}

/// The sign-in or consent prompt Dar showed (`detail` of the interaction).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interaction {
    pub prompt: String,
    #[serde(default)]
    pub requires_password: bool,
}

/// Follows an authorization request through Dar's sign-in and consent until it leaves Dar
/// (`follow`): interactions are answered with the session's CSRF token and [`PASSWORD`],
/// approving unless `deny`; every prompt is passed to `on_interaction`.
pub async fn follow(base: &str, session: &Session, start: &str, deny: bool, jar: Option<CookieJar>, mut on_interaction: impl FnMut(&Interaction)) -> Result<Followed> {
    let mut jar = jar.unwrap_or_else(|| CookieJar::new(Some(&session.cookie)));
    let base_url = url::Url::parse(base)?;
    let mut next = start.to_owned();
    for _ in 0..12 {
        let target = url::Url::parse(&next)?;
        if target.origin() != base_url.origin() {
            return Ok(Followed { url: target, jar });
        }
        if let Some(id) = target.path().strip_prefix("/identity/interaction/").filter(|id| !id.contains('/')) {
            let endpoint = format!("{base}/api/v1/identity/interactions/{id}");
            let headers = |request: reqwest::RequestBuilder| request.header("origin", base).header("x-csrf-token", &session.csrf_token).header("content-type", "application/json");
            let detail = jar.fetch(headers(client().get(&endpoint))).await?;
            ensure!(detail.status == 200, "interaction {id}: {} {}", detail.status, detail.text());
            let interaction: Interaction = serde_json::from_slice(&detail.body)?;
            on_interaction(&interaction);
            let action = if deny { "deny" } else if interaction.prompt == "login" { "login" } else { "approve" };
            let answered = jar.fetch(headers(client().post(&endpoint)).body(json!({"action": action, "password": PASSWORD}).to_string())).await?;
            ensure!(answered.status == 200, "answering interaction {id}: {} {}", answered.status, answered.text());
            next = answered.json()?["redirect"].as_str().unwrap_or_default().to_owned();
        } else {
            let response = jar.get(&next).await?;
            ensure!([302, 303].contains(&response.status), "{}: {} {}", target.path(), response.status, response.text());
            next = base_url.join(response.header("location").unwrap_or_default())?.to_string();
        }
    }
    crate::bail!("authorization failed to reach the registered callback")
}
