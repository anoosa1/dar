//! The service identity's calls into SpacetimeDB (`runtime/database.ts`). The owner
//! credential stays in this process and is never sent to a browser.

use serde_json::Value;

use crate::error::{AppError, Result};

/// Error codes the module reports and the HTTP status each maps to.
const CODES: [(&str, u16); 13] = [
    ("lockedFolder", 423),
    ("nothingToCommit", 409),
    ("uncommittedChanges", 409),
    ("conflict", 409),
    ("alreadyExists", 409),
    ("notFound", 404),
    ("forbidden", 403),
    ("unauthenticated", 401),
    ("invalidInput", 400),
    ("invalidName", 400),
    ("quotaExceeded", 507),
    ("operationTooLarge", 413),
    ("contentUnavailable", 503),
];

fn coded(detail: &str) -> AppError {
    for (code, status) in CODES {
        if detail.contains(code) {
            let message = if code == "lockedFolder" {
                "Unlock the Locked folder with your password to continue."
            } else {
                code
            };
            return AppError::with(code, status, message);
        }
    }
    AppError::with(
        "databaseUnavailable",
        503,
        "The database request could not be completed.",
    )
}

#[derive(Clone)]
pub struct Database {
    client: reqwest::Client,
    pub url: String,
    pub name: String,
    token: String,
}

impl Database {
    pub fn new(url: &str, name: &str, token: &str) -> Self {
        let client = reqwest::Client::builder()
            .tcp_nodelay(true)
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("HTTP client");
        Self {
            client,
            url: url.trim_end_matches('/').into(),
            name: name.into(),
            token: token.trim().into(),
        }
    }

    /// Runs a procedure or reducer and answers once its changes are durable.
    pub async fn call(&self, function: &str, args: Value) -> Result<Value> {
        self.invoke(function, args, true).await
    }

    /// Runs a procedure that only reads, without waiting for a flush.
    pub async fn read(&self, function: &str, args: Value) -> Result<Value> {
        self.invoke(function, args, false).await
    }

    /// `read` for operations in `reads`, otherwise `call` (`database.ts` `operation`).
    pub async fn operation(
        &self,
        function: &str,
        operation: &str,
        reads: &[&str],
        args: Value,
    ) -> Result<Value> {
        self.invoke(function, args, !reads.contains(&operation))
            .await
    }

    async fn invoke(&self, function: &str, args: Value, confirmed: bool) -> Result<Value> {
        let url = format!(
            "{}/v1/database/{}/call/{function}{}",
            self.url,
            self.name,
            if confirmed { "" } else { "?confirmed=false" }
        );
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.token)
            .json(&args)
            .send()
            .await
            .map_err(|_| coded(""))?;
        let ok = response.status().is_success();
        let text = response.text().await.map_err(|_| coded(""))?;
        if !ok {
            return Err(coded(&text));
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        let value: Value = serde_json::from_str(&text).map_err(|_| coded(""))?;
        let value = match value {
            Value::String(inner) => serde_json::from_str(&inner).map_err(|_| coded(""))?,
            other => other,
        };
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_map_to_statuses() {
        assert_eq!(coded("lockedFolder").status, 423);
        assert_eq!(coded("SenderError: conflict").code, "conflict");
        assert_eq!(coded("trap").code, "databaseUnavailable");
    }
}
