//! `/api/v1` requests (`client/api.ts`): JSON bodies, the CSRF header, Dar's error shape.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub administrator: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSession {
    pub user: User,
    pub csrf_token: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RequestError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl RequestError {
    fn unexpected(status: u16) -> Self {
        Self {
            status,
            message: "The instance returned an unexpected response. Try again shortly.".into(),
        }
    }
}

/// One request; `body` is JSON unless `raw` is given (a `File`/`Blob`/bytes body is sent as-is).
#[derive(Default)]
pub struct Req<'a> {
    pub method: &'a str,
    pub body: Option<String>,
    pub raw: Option<JsValue>,
    pub csrf: Option<&'a str>,
    pub headers: &'a [(&'a str, &'a str)],
    pub signal: Option<web_sys::AbortSignal>,
}

pub async fn fetch(url: &str, req: Req<'_>) -> Result<web_sys::Response, RequestError> {
    let init = web_sys::RequestInit::new();
    init.set_method(if req.method.is_empty() {
        "GET"
    } else {
        req.method
    });
    let headers = web_sys::Headers::new().expect("headers");
    if req.raw.is_none() {
        let _ = headers.set("content-type", "application/json");
    }
    if let Some(csrf) = req.csrf {
        let _ = headers.set("x-csrf-token", csrf);
    }
    for (name, value) in req.headers {
        let _ = headers.set(name, value);
    }
    init.set_headers(&headers);
    if let Some(body) = &req.body {
        init.set_body(&JsValue::from_str(body));
    } else if let Some(raw) = &req.raw {
        init.set_body(raw);
    }
    if let Some(signal) = &req.signal {
        init.set_signal(Some(signal));
    }
    let promise = web_sys::window()
        .expect("window")
        .fetch_with_str_and_init(url, &init);
    match JsFuture::from(promise).await {
        Ok(response) => Ok(response.unchecked_into()),
        Err(_) => Err(RequestError {
            status: 0,
            message: "The connection was interrupted. Try again.".into(),
        }),
    }
}

async fn request_text(url: &str, req: Req<'_>) -> Result<(u16, String), RequestError> {
    let response = fetch(url, req).await?;
    let status = response.status();
    note_pim_revision(url, &response);
    let text = match response.text() {
        Ok(promise) => JsFuture::from(promise)
            .await
            .ok()
            .and_then(|v| v.as_string())
            .ok_or_else(|| RequestError::unexpected(status))?,
        Err(_) => return Err(RequestError::unexpected(status)),
    };
    if !response.ok() {
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|_| RequestError::unexpected(status))?;
        let message = value["error"]["message"]
            .as_str()
            .unwrap_or("The request could not be completed.")
            .to_owned();
        return Err(RequestError { status, message });
    }
    Ok((status, text))
}

/// Like `request<T>()`: parses JSON and turns `{error:{message}}` into an error.
pub async fn request<T: DeserializeOwned>(url: &str, req: Req<'_>) -> Result<T, RequestError> {
    let (status, text) = request_text(url, req).await?;
    serde_json::from_str(&text).map_err(|_| RequestError::unexpected(status))
}

pub async fn get<T: DeserializeOwned>(url: &str) -> Result<T, RequestError> {
    request(url, Req::default()).await
}

/// A JSON request with a body and the session's CSRF token.
pub async fn send<T: DeserializeOwned>(
    url: &str,
    method: &str,
    body: Option<serde_json::Value>,
    csrf: &str,
) -> Result<T, RequestError> {
    request(
        url,
        Req {
            method,
            body: body.map(|b| b.to_string()),
            csrf: Some(csrf),
            ..Default::default()
        },
    )
    .await
}

fn check_procedure_error(value: &str) -> Result<(), String> {
    if !value.contains("\"error\"") {
        return Ok(());
    }
    let parsed: serde_json::Value = serde_json::from_str(value)
        .map_err(|_| "The request could not be completed.".to_owned())?;
    if let Some(error) = parsed.get("error").filter(|e| !e.is_null()) {
        return Err(error["code"]
            .as_str()
            .unwrap_or("The request could not be completed.")
            .to_owned());
    }
    Ok(())
}

/// A procedure's JSON result; `{error:{code}}` becomes an error with the code.
pub fn read_procedure<T: DeserializeOwned>(value: &str) -> Result<T, String> {
    check_procedure_error(value)?;
    serde_json::from_str(value).map_err(|e| {
        if serde_json::from_str::<serde::de::IgnoredAny>(value).is_err() {
            "The request could not be completed.".to_owned()
        } else {
            e.to_string()
        }
    })
}

pub fn format_bytes(bytes: f64) -> String {
    if bytes < 1024.0 {
        return format!("{bytes} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let (mut n, mut index) = (bytes / 1024.0, 0);
    while n >= 1024.0 && index < units.len() - 1 {
        n /= 1024.0;
        index += 1;
    }
    format!("{:.*} {}", if n < 10.0 { 1 } else { 0 }, n, units[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_format_like_the_react_client() {
        assert_eq!(format_bytes(5.0), "5 B");
        assert_eq!(format_bytes(1536.0), "1.5 KB");
        assert_eq!(format_bytes(20.0 * 1024.0 * 1024.0), "20 MB");
    }

    #[test]
    fn procedure_errors_carry_the_code() {
        assert_eq!(
            read_procedure::<serde_json::Value>(r#"{"error":{"code":"conflict"}}"#),
            Err("conflict".into())
        );
        assert_eq!(
            read_procedure::<serde_json::Value>(r#"{"ok":true}"#).unwrap()["ok"],
            true
        );
    }
}

thread_local! {
    static PIM_REVISION: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

/// Contacts, Calendar, Notes and Tasks responses carry the account's change revision, so a
/// page can tell whether live updates are newer than what it loaded (`loadedPimRevision`).
fn note_pim_revision(url: &str, response: &web_sys::Response) {
    let pim = [
        "/api/v1/contacts",
        "/api/v1/calendar",
        "/api/v1/notes",
        "/api/v1/tasks",
        "/api/v1/pim",
    ]
    .iter()
    .any(|p| url.starts_with(p));
    if let Some(value) = pim
        .then(|| response.headers().get("x-dar-pim-revision").ok().flatten())
        .flatten()
        .and_then(|v| v.parse::<f64>().ok())
    {
        PIM_REVISION.with(|r| r.set(r.get().max(value)));
    }
}

pub fn loaded_pim_revision() -> f64 {
    PIM_REVISION.with(|r| r.get())
}
