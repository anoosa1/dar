//! What suites use beside the HTTP API (`test/support/`): the service credential's database
//! calls, the runtime's operator commands, and credentials in the formats Dar stores.

use crate::stack::stack;
use crate::{Error, Result, Value, bail, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// The service credential's view of a database: procedures and reducers over SpacetimeDB's
/// HTTP API, as the runtime calls them. The owner token stays in the test process.
#[derive(Debug, Clone)]
pub struct Database {
    pub url: String,
    pub name: String,
    pub token_file: PathBuf,
    token: String,
}

/// An error a database call reported, with Dar's code and HTTP status.
#[derive(Debug, Clone, PartialEq)]
pub struct AppError {
    pub code: String,
    pub status: u16,
}

const CODES: &[(&str, u16)] = &[
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

impl Database {
    pub fn new(url: &str, name: &str, token_file: &Path) -> Result<Database> {
        let token = crate::read_to_string(token_file)?.trim().to_owned();
        if token.is_empty() {
            bail!("a database service credential is required");
        }
        Ok(Database {
            url: url.trim_end_matches('/').to_owned(),
            name: name.to_owned(),
            token_file: token_file.to_path_buf(),
            token,
        })
    }

    async fn invoke(
        &self,
        function: &str,
        args: &Value,
        confirmed: bool,
    ) -> std::result::Result<Value, AppError> {
        let url = format!(
            "{}/v1/database/{}/call/{function}{}",
            self.url,
            self.name,
            if confirmed { "" } else { "?confirmed=false" }
        );
        let unavailable = || AppError {
            code: "databaseUnavailable".into(),
            status: 503,
        };
        let response = crate::session::client()
            .post(url)
            .bearer_auth(&self.token)
            .json(args)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| unavailable())?;
        let ok = response.status().is_success();
        let text = response.text().await.map_err(|_| unavailable())?;
        if !ok {
            for (code, status) in CODES {
                if text.contains(code) {
                    return Err(AppError {
                        code: (*code).into(),
                        status: *status,
                    });
                }
            }
            return Err(unavailable());
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        let value: Value = serde_json::from_str(&text).map_err(|_| unavailable())?;
        // Procedures return their JSON as a string.
        Ok(match value {
            Value::String(inner) => serde_json::from_str(&inner).unwrap_or(Value::String(inner)),
            other => other,
        })
    }

    /// Runs a procedure or reducer and answers once its changes are durable.
    pub async fn call(&self, function: &str, args: Value) -> std::result::Result<Value, AppError> {
        self.invoke(function, &args, true).await
    }

    /// Runs a read-only procedure without waiting for a flush.
    pub async fn read(&self, function: &str, args: Value) -> std::result::Result<Value, AppError> {
        self.invoke(function, &args, false).await
    }

    /// A SQL query as the owner (`/v1/database/<name>/sql`), rows as JSON arrays.
    pub async fn sql(&self, query: &str) -> Result<Value> {
        let response = crate::session::client()
            .post(format!("{}/v1/database/{}/sql", self.url, self.name))
            .bearer_auth(&self.token)
            .body(query.to_owned())
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            bail!("SQL {query}: {status} {text}");
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// The provider's own records (`service_provider`), as its store reads and writes them.
    pub async fn provider(
        &self,
        operation: &str,
        payload: Value,
    ) -> std::result::Result<Value, AppError> {
        self.call("service_provider", json!([operation, payload.to_string()]))
            .await
    }

    /// The environment `dar-runtime` operator commands need for this database.
    pub fn env(&self) -> [(&'static str, String); 3] {
        [
            ("DAR_DATABASE_URL", self.url.clone()),
            ("DAR_DATABASE_NAME", self.name.clone()),
            (
                "DAR_DATABASE_TOKEN_FILE",
                self.token_file.to_string_lossy().into_owned(),
            ),
        ]
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.code, self.status)
    }
}

impl std::error::Error for AppError {}

/// Runs `dar-runtime <args>` for `db` (and `root` when the command needs one) and returns its
/// output (`operator` in `support/operator.ts`).
pub fn operator(db: Option<&Database>, root: Option<&Path>, args: &[&str]) -> Result<String> {
    operator_env(db, root, &[], args)
}

fn operator_json(db: &Database, root: Option<&Path>, args: &[&str]) -> Result<Value> {
    Ok(serde_json::from_str(&operator(Some(db), root, args)?)?)
}

/// Delivers due reminders now, as the runtime's periodic worker does.
pub fn deliver_reminders(db: &Database) -> Result<()> {
    operator(Some(db), None, &["deliver-reminders"]).map(|_| ())
}
pub fn backup(db: &Database, root: &Path, destination: &Path) -> Result<Value> {
    operator_json(db, Some(root), &["backup", &destination.to_string_lossy()])
}
pub fn restore(db: &Database, source: &Path, root: &Path) -> Result<Value> {
    operator_json(db, Some(root), &["restore", &source.to_string_lossy()])
}
pub fn export_files(
    db: &Database,
    root: &Path,
    username: &str,
    destination: &Path,
) -> Result<Value> {
    operator_json(
        db,
        Some(root),
        &["export-files", username, &destination.to_string_lossy()],
    )
}
pub fn reconcile_storage(db: &Database, root: &Path) -> Result<u64> {
    Ok(
        operator_json(db, Some(root), &["reconcile-storage"])?["count"]
            .as_u64()
            .unwrap_or_default(),
    )
}
pub fn sweep_orphans(db: &Database, root: &Path) -> Result<u64> {
    Ok(operator_json(db, Some(root), &["sweep-orphans"])?["count"]
        .as_u64()
        .unwrap_or_default())
}
/// `apps-install|apps-enable|apps-disable|apps-uninstall <root> [argument]`, offline.
pub fn manage_apps(root: &Path, command: &str, argument: Option<&str>) -> Result<String> {
    let root = root.to_string_lossy();
    let mut args = vec![command, root.as_ref()];
    args.extend(argument);
    operator(None, None, &args)
}
/// Runs `dar-runtime <args>` with extra environment variables.
pub fn operator_env(
    db: Option<&Database>,
    root: Option<&Path>,
    env: &[(&str, &str)],
    args: &[&str],
) -> Result<String> {
    let mut command = Command::new(&stack().artifacts.runtime);
    command.args(args).stdin(Stdio::null());
    if let Some(db) = db {
        command.envs(db.env());
    }
    if let Some(root) = root {
        command.env("DAR_ROOT", root);
    }
    for (k, v) in env {
        command.env(k, v);
    }
    let output = command
        .output()
        .map_err(|e| Error::new(format!("dar-runtime {}: {e}", args.first().unwrap_or(&""))))?;
    let (stdout, stderr) = (
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    );
    if !output.status.success() {
        bail!(
            "{}",
            if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                format!("dar-runtime {} failed", args.first().unwrap_or(&""))
            }
        );
    }
    Ok(stdout)
}

/// Applies an OIDC client file to the database, as the runtime does with `DAR_OIDC_FILE`.
pub fn configure_oidc(
    db: &Database,
    root: &Path,
    origin: &str,
    file: &Path,
    optional: bool,
) -> Result<String> {
    let file = file.to_string_lossy();
    let mut args = vec!["configure-oidc", file.as_ref()];
    if optional {
        args.push("optional");
    }
    operator_env(
        Some(db),
        Some(root),
        &[("DAR_ORIGIN", origin), ("DAR_DEVELOPMENT", "1")],
        &args,
    )
}

/// SHA-256 of a token, as Dar stores token digests.
pub fn digest_token(secret: &str) -> String {
    crate::sha256_hex(secret)
}

/// A random secret and the digest that is stored for it.
pub fn create_opaque_token() -> (String, String) {
    let secret = crate::base64url(&crate::random_bytes(32));
    let digest = digest_token(&secret);
    (secret, digest)
}

/// A procedure's JSON result; `{error:{code}}` becomes an error with the code.
pub fn read_procedure(value: &Value) -> Result<Value> {
    if let Some(code) = value
        .get("error")
        .and_then(|e| e.get("code").or(Some(e)))
        .filter(|_| value.get("error").is_some())
    {
        bail!(
            "{}",
            code.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| code.to_string())
        );
    }
    Ok(value.clone())
}

/// A password verifier exactly as the runtime makes one (scrypt N=2^17, r=8, p=1).
pub fn hash_password(password: &str) -> Result<String> {
    if password.chars().count() < 15 || password.len() > 1024 {
        bail!("Use at least 15 characters and no more than 1024 UTF-8 bytes.");
    }
    let salt = crate::random_bytes(16);
    let params = scrypt::Params::new(17, 8, 1, 32).map_err(|e| Error::new(e.to_string()))?;
    let mut digest = [0u8; 32];
    scrypt::scrypt(password.as_bytes(), &salt, &params, &mut digest)
        .map_err(|e| Error::new(e.to_string()))?;
    Ok(format!(
        "scrypt$131072$8$1${}${}",
        crate::base64url(&salt),
        crate::base64url(&digest)
    ))
}

/// The instance's identity signing key, unsealed from `secrets/` (for suites that forge
/// tokens): the private JWK (`kid`, `d`, `x`, `y`).
pub fn signing_jwk(secrets: &Path) -> Result<Value> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use base64::Engine as _;
    let decode = |text: &str| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(text.trim().trim_end_matches('='))
            .map_err(|e| Error::new(e.to_string()))
    };
    let key = decode(&crate::read_to_string(
        secrets.join("identity-keyring.key"),
    )?)?;
    let envelope: Value =
        serde_json::from_str(&crate::read_to_string(secrets.join("signing-key.json"))?)?;
    if envelope["format"] != "dar-keyring-v1" {
        bail!("unrecognized signing key");
    }
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(&key).map_err(|e| Error::new(e.to_string()))?;
    let mut sealed = decode(envelope["ciphertext"].as_str().unwrap_or_default())?;
    sealed.extend(decode(envelope["tag"].as_str().unwrap_or_default())?);
    let iv = decode(envelope["iv"].as_str().unwrap_or_default())?;
    let plain = cipher
        .decrypt(
            aes_gcm::Nonce::from_slice(&iv),
            Payload {
                msg: &sealed,
                aad: b"dar.identity.signing-key.v1",
            },
        )
        .map_err(|_| Error::new("the signing key did not unseal"))?;
    Ok(serde_json::from_slice(&plain)?)
}

/// An ES256 JWT signed with a private JWK, with `kid` in the header.
pub fn sign_es256(jwk: &Value, claims: &Value) -> Result<String> {
    use base64::Engine as _;
    use p256::ecdsa::signature::Signer as _;
    let d = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(jwk["d"].as_str().unwrap_or_default())
        .map_err(|e| Error::new(e.to_string()))?;
    let key = p256::ecdsa::SigningKey::from_slice(&d).map_err(|e| Error::new(e.to_string()))?;
    let header = json!({"alg": "ES256", "kid": jwk["kid"], "typ": "JWT"});
    let input = format!(
        "{}.{}",
        crate::base64url(header.to_string().as_bytes()),
        crate::base64url(claims.to_string().as_bytes())
    );
    let signature: p256::ecdsa::Signature = key.sign(input.as_bytes());
    Ok(format!(
        "{input}.{}",
        crate::base64url(&signature.to_bytes())
    ))
}

/// A JWT's claims, unverified (`decodeJwt`).
pub fn jwt_claims(token: &str) -> Result<Value> {
    use base64::Engine as _;
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| Error::new("not a JWT"))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|e| Error::new(e.to_string()))?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// A browser token for a session, signed as the runtime's issuer signs them (`/auth/issuer`).
pub fn browser_token(
    secrets: &Path,
    issuer: &str,
    session_id: &str,
    expires_at_ms: u64,
) -> Result<String> {
    let jwk = signing_jwk(secrets)?;
    let now = chrono::Utc::now().timestamp() as u64;
    let exp = (expires_at_ms / 1000).min(now + 300);
    sign_es256(
        &jwk,
        &json!({"sid": session_id, "iss": issuer, "aud": "dar-browser", "sub": format!("{session_id}:{}", crate::uuid()), "iat": now, "exp": exp}),
    )
}
