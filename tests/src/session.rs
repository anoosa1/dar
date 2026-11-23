//! Signing in and the HTTP helpers suites share (`integration-helpers.ts`).

use crate::{Result, ResultExt, Value, bail, ensure, json};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::sync::OnceLock;
use std::time::Duration;

/// The administrator's password in every fixture.
pub const PASSWORD: &str = "a long isolated test password";

/// One HTTP client for the run: no cookie store and no redirects followed, like `fetch` with
/// explicit cookie headers (suites that follow redirects use [`following`]).
pub fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(120)).build().expect("HTTP client")).clone()
}

/// A client that follows redirects, as `fetch` does by default.
pub fn following() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(120)).build().expect("HTTP client")).clone()
}

/// A signed-in browser session: the cookie, CSRF token and SpacetimeDB token.
#[derive(Debug, Clone)]
pub struct Session {
    pub base: String,
    pub cookie: String,
    pub csrf_token: String,
    /// The SpacetimeDB token, database URL and name from `/api/v1/auth/token`.
    pub token: String,
    pub database_url: String,
    pub database_name: String,
}

impl Session {
    /// `cookie`, `origin` and `x-csrf-token`: what every write sends.
    pub fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("cookie", HeaderValue::from_str(&self.cookie).unwrap());
        headers.insert("origin", HeaderValue::from_str(&self.base).unwrap());
        headers.insert("x-csrf-token", HeaderValue::from_str(&self.csrf_token).unwrap());
        headers
    }

    /// The headers as pairs (for the browser driver's request helpers).
    pub fn header_pairs(&self) -> Vec<(&'static str, String)> {
        vec![("cookie", self.cookie.clone()), ("origin", self.base.clone()), ("x-csrf-token", self.csrf_token.clone())]
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// A request to `base` + `path` with the session's headers.
    pub fn request(&self, method: &str, path: &str) -> reqwest::RequestBuilder {
        let url = if path.starts_with("http") { path.to_owned() } else { self.url(path) };
        client().request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), url).headers(self.headers())
    }

    pub async fn get(&self, path: &str) -> Result<Reply> {
        Reply::from(self.request("GET", path).send().await?).await
    }

    /// Sends JSON with the session's headers.
    pub async fn send_json(&self, method: &str, path: &str, body: &Value) -> Result<Reply> {
        Reply::from(self.request(method, path).json(body).send().await?).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Reply> {
        self.send_json("POST", path, body).await
    }

    pub async fn put(&self, path: &str, body: &Value) -> Result<Reply> {
        self.send_json("PUT", path, body).await
    }

    pub async fn patch(&self, path: &str, body: &Value) -> Result<Reply> {
        self.send_json("PATCH", path, body).await
    }

    pub async fn delete(&self, path: &str) -> Result<Reply> {
        Reply::from(self.request("DELETE", path).send().await?).await
    }

    /// GETs JSON and checks the status is 200.
    pub async fn get_json(&self, path: &str) -> Result<Value> {
        let reply = self.get(path).await?;
        ensure!(reply.status == 200, "GET {path}: {} {}", reply.status, reply.text());
        reply.json()
    }

    /// Opens a SpacetimeDB connection as this person, subscribed to `me`, `my_files` and
    /// `my_usage` (`connect`).
    pub async fn connect(&self) -> Result<crate::live::Live> {
        crate::live::Live::connect(self, &["SELECT * FROM me", "SELECT * FROM my_files", "SELECT * FROM my_usage"]).await
    }
}

/// A buffered HTTP response.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Reply {
    pub async fn from(response: reqwest::Response) -> Result<Reply> {
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.bytes().await?.to_vec();
        Ok(Reply { status, headers, body })
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn json(&self) -> Result<Value> {
        serde_json::from_slice(&self.body).map_err(|error| crate::Error::new(format!("not JSON ({error}): {}", self.text())))
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
    /// Every `set-cookie` header.
    pub fn set_cookies(&self) -> Vec<&str> {
        self.headers.get_all("set-cookie").iter().filter_map(|value| value.to_str().ok()).collect()
    }
    /// Fails with the body unless the status is `expected`.
    pub fn expect(self, expected: u16) -> Result<Reply> {
        ensure!(self.status == expected, "expected HTTP {expected}, got {}: {}", self.status, self.text());
        Ok(self)
    }
}

/// Sets up the first account (`admin`) with the run's bootstrap token, if not done yet.
pub async fn setup_admin(base: &str) -> Result<()> {
    let status: Value = client().get(format!("{base}/api/v1/auth/status")).send().await?.json().await?;
    if status["initialized"] == json!(true) {
        return Ok(());
    }
    let token = crate::read_to_string(&crate::stack::stack().bootstrap_token_file)?;
    let response = client().post(format!("{base}/api/v1/auth/setup")).header("origin", base)
        .json(&json!({"username": "admin", "displayName": "Test Administrator", "password": PASSWORD, "bootstrapToken": token.trim()}))
        .send().await?;
    let status = response.status().as_u16();
    ensure!(status == 200, "setting up the administrator: {status} {}", response.text().await.unwrap_or_default());
    Ok(())
}

/// Signs in as `admin` and fetches the SpacetimeDB token (`login`).
pub async fn login(base: &str) -> Result<Session> {
    login_as(base, "admin", PASSWORD).await
}

pub async fn login_as(base: &str, username: &str, password: &str) -> Result<Session> {
    let response = client().post(format!("{base}/api/v1/auth/login")).header("origin", base)
        .json(&json!({"username": username, "password": password})).send().await.context("signing in")?;
    let reply = Reply::from(response).await?;
    ensure!(reply.status == 200, "signing in as {username}: {} {}", reply.status, reply.text());
    let cookie = reply.header("set-cookie").context("no session cookie")?.split(';').next().unwrap_or_default().to_owned();
    let csrf_token = reply.json()?["csrfToken"].as_str().context("no CSRF token")?.to_owned();
    let mut session = Session { base: base.to_owned(), cookie, csrf_token, token: String::new(), database_url: String::new(), database_name: String::new() };
    let reply = Reply::from(session.request("POST", "/api/v1/auth/token").send().await?).await?;
    ensure!(reply.status == 200, "fetching the database token: {} {}", reply.status, reply.text());
    let token = reply.json()?;
    session.token = token["token"].as_str().unwrap_or_default().to_owned();
    session.database_url = token["databaseUrl"].as_str().unwrap_or_default().to_owned();
    session.database_name = token["databaseName"].as_str().unwrap_or_default().to_owned();
    Ok(session)
}

/// A reservation from [`upload`].
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reservation {
    pub id: String,
    pub file_id: String,
}

/// Options for [`upload_with`]; the defaults upload a new text file at the top level.
#[derive(Debug, Clone)]
pub struct Upload<'a> {
    pub parent_id: &'a str,
    pub file_id: Option<&'a str>,
    pub expected_revision: Option<u64>,
    pub media_type: &'a str,
}

impl Default for Upload<'_> {
    fn default() -> Self {
        Self { parent_id: "", file_id: None, expected_revision: None, media_type: "text/plain" }
    }
}

/// Reserves and sends a file through the Files API (`upload`).
pub async fn upload(session: &Session, name: &str, contents: impl AsRef<[u8]>) -> Result<Reservation> {
    upload_with(session, name, contents, Upload::default()).await
}

pub async fn upload_with(session: &Session, name: &str, contents: impl AsRef<[u8]>, options: Upload<'_>) -> Result<Reservation> {
    let contents = contents.as_ref();
    let mut body = json!({"name": name, "size": contents.len(), "mediaType": options.media_type, "parentId": options.parent_id});
    if let Some(file_id) = options.file_id {
        body["fileId"] = json!(file_id);
    }
    if let Some(revision) = options.expected_revision {
        body["expectedRevision"] = json!(revision);
    }
    let reply = session.post("/api/v1/files/uploads", &body).await?;
    if reply.status != 201 {
        bail!("reserving {name}: {} {}", reply.status, reply.text());
    }
    let reservation: Reservation = serde_json::from_value(reply.json()?)?;
    let transfer = session.request("PUT", &format!("/api/v1/files/uploads/{}", reservation.id)).body(contents.to_vec()).timeout(Duration::from_secs(10)).send().await?;
    let transfer = Reply::from(transfer).await?;
    ensure!(transfer.status == 200, "sending {name}: {} {}", transfer.status, transfer.text());
    Ok(reservation)
}

/// A header name from a string (for building `HeaderMap`s in suites).
pub fn header(name: &str) -> HeaderName {
    HeaderName::from_bytes(name.as_bytes()).expect("header name")
}
