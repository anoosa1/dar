//! Errors as Dar's API reports them (`runtime/errors.ts`): `{error:{code,message}}`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, Clone)]
pub struct AppError {
    pub code: String,
    pub status: u16,
    pub message: String,
    /// SCIM's error type (`ScimError.scimType`), shown only by the SCIM endpoints.
    pub scim_type: Option<String>,
}

impl AppError {
    pub fn new(code: &str, status: u16) -> Self {
        Self { code: code.into(), status, message: code.into(), scim_type: None }
    }
    pub fn with(code: &str, status: u16, message: &str) -> Self {
        Self { code: code.into(), status, message: message.into(), scim_type: None }
    }
    pub fn not_found() -> Self {
        Self::new("notFound", 404)
    }
    pub fn invalid() -> Self {
        Self::new("invalidInput", 400)
    }
    pub fn internal() -> Self {
        Self::with("internalError", 500, "The request could not be completed.")
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.code, self.status)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::json!({ "error": { "code": self.code, "message": self.message } });
        let mut response = (status, [("content-type", "application/json; charset=utf-8")], body.to_string()).into_response();
        if self.status == 413 {
            response.headers_mut().insert("connection", "close".parse().expect("header"));
        }
        response
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

/// A SCIM error (`ScimError`): code `scimError`, the detail as the message.
pub fn scim_error(status: u16, detail: &str, scim_type: Option<&str>) -> AppError {
    AppError { code: "scimError".into(), status, message: detail.into(), scim_type: scim_type.map(str::to_owned) }
}
