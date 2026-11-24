//! Dar's acceptance suite: real SpacetimeDB, the runtime, the client in Chromium, on throwaway
//! roots, databases and ports. `cargo xtask verify` builds the stack and runs it; `src/main.rs`
//! is the runner and `src/suites/` holds the suites.
//!
//! - [`stack`]: the run's SpacetimeDB server and the built artifacts it tests.
//! - [`instance`]: isolated Dar instances (database, root, runtime) and the standard fixture.
//! - [`session`]: signing in, uploads and HTTP helpers (`integration-helpers.ts`).
//! - [`live`]: a SpacetimeDB connection as a signed-in person, with reducer and table helpers.
//! - [`support`]: the service credential's database calls, operator commands, credentials.
//! - [`browser`]: Chromium over the DevTools Protocol with Playwright-style locators.
//! - [`ui`]: shared browser steps (`ui-helpers.mjs`).

pub mod browser;
pub mod harness;
pub mod identity;
pub mod instance;
pub mod live;
pub mod oidc;
pub mod session;
pub mod stack;
pub mod support;
pub mod ui;

pub use harness::{Case, Ctx};
pub use instance::{Instance, InstanceConfig};
pub use serde_json::{Value, json};
pub use session::{PASSWORD, Session};

use std::fmt;
use std::path::{Path, PathBuf};

/// A test failure with its context. Any error converts into it, so `?` works throughout.
pub struct Error(String);

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }
    /// Prefixes what was being done: "logging in: connection refused".
    pub fn context(self, context: impl fmt::Display) -> Self {
        let context = context.to_string();
        if context.is_empty() {
            self
        } else {
            Error(format!("{context}: {}", self.0))
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
    fn from(error: E) -> Self {
        let mut message = error.to_string();
        let mut source = error.source();
        while let Some(cause) = source {
            message.push_str(&format!(": {cause}"));
            source = cause.source();
        }
        Error(message)
    }
}

/// Adds context to results: `fetch(...).await.context("loading the session")?`.
pub trait ResultExt<T> {
    fn context(self, context: impl fmt::Display) -> Result<T>;
}

impl<T, E: Into<Error>> ResultExt<T> for std::result::Result<T, E> {
    fn context(self, context: impl fmt::Display) -> Result<T> {
        self.map_err(|error| error.into().context(context))
    }
}

impl<T> ResultExt<T> for Option<T> {
    fn context(self, context: impl fmt::Display) -> Result<T> {
        self.ok_or_else(|| Error::new(context.to_string()))
    }
}

/// Returns early with an error: `bail!("expected {x}")`.
#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => { return Err($crate::Error::new(format!($($arg)*))) };
}

/// Fails unless the condition holds: `ensure!(rows.len() == 2, "rows: {rows:?}")`.
#[macro_export]
macro_rules! ensure {
    ($condition:expr $(,)?) => { if !$condition { $crate::bail!("assertion failed: {}", stringify!($condition)) } };
    ($condition:expr, $($arg:tt)*) => { if !$condition { $crate::bail!($($arg)*) } };
}

/// A new private directory under the system temporary directory.
pub fn temp_dir(prefix: &str) -> Result<PathBuf> {
    let base = std::env::temp_dir();
    for _ in 0..16 {
        let path = base.join(format!("{prefix}{}", random_hex(6)));
        match std::fs::create_dir(&path) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("could not create a temporary directory")
}

/// `n` random bytes as lowercase hex.
pub fn random_hex(n: usize) -> String {
    random_bytes(n).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).expect("system randomness");
    bytes
}

/// A random UUID v4, like `crypto.randomUUID()`.
pub fn uuid() -> String {
    let mut b = random_bytes(16);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Base64url without padding, as Node's `toString("base64url")`.
pub fn base64url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn base64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes.as_ref())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// An executable on PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
    })
}

/// Waits until `check` returns `Some`, polling every 50 ms, or fails after `timeout`.
pub async fn eventually<T, F, Fut>(
    timeout: std::time::Duration,
    what: &str,
    mut check: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Option<T>>>,
{
    let deadline = std::time::Instant::now() + timeout;
    let mut last = None;
    loop {
        match check().await {
            Ok(Some(value)) => return Ok(value),
            Ok(None) => {}
            Err(error) => last = Some(error),
        }
        if std::time::Instant::now() > deadline {
            bail!(
                "{what} did not happen within {timeout:?}{}",
                last.map(|e| format!(" (last error: {e})"))
                    .unwrap_or_default()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Reads a file to a string with the path in the error.
pub fn read_to_string(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    std::fs::read_to_string(path).context(path.display())
}

/// What a suite file usually needs: `use dar_acceptance::prelude::*;`. It includes the generated
/// bindings' table and reducer traits, so `db.db.my_files()` and `db.reducers.browse_then(…)`
/// resolve.
pub mod prelude {
    pub use crate::browser::{
        Browser, ClickOptions, Context as BrowserContext, ContextOptions, FilePayload, Fulfill,
        Locator, Page, State, contains, regex,
    };
    pub use crate::case;
    pub use crate::harness::Suite;
    pub use crate::identity::{CookieJar, IdentityAdmin, follow};
    pub use crate::instance::{self, Instance, InstanceConfig};
    pub use crate::live::{self, Live, procedure, reducer};
    pub use crate::session::{
        self, PASSWORD, Reply, Session, login, login_as, upload, upload_with,
    };
    pub use crate::support::{self, Database};
    pub use crate::{
        Ctx, Error, Result, ResultExt, Value, bail, ensure, eventually, json, ui, uuid,
    };
    pub use dar_bindings::*;
    pub use spacetimedb_sdk::{DbContext as _, Table as _, TableWithPrimaryKey as _};
}
