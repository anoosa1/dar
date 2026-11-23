//! The runner's model: a suite is a named list of cases that run in order and share one
//! fixture instance; suites run in parallel, each on its own threads and tokio runtime, with
//! their own instances, so nothing one suite does is visible to another.

use crate::instance::Instance;
use crate::{Error, Result};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;

pub type CaseFuture<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

/// One test (`test(...)` in node:test, or a browser script's journey).
#[derive(Clone, Copy)]
pub struct Case {
    pub name: &'static str,
    pub run: for<'a> fn(&'a Ctx) -> CaseFuture<'a>,
    /// How long the case may take.
    pub timeout: Duration,
}

/// A named group of cases sharing a fixture, run in order.
#[derive(Clone, Copy)]
pub struct Suite {
    pub name: &'static str,
    pub cases: &'static [Case],
}

/// Registers a case: `case!(files::immutable_upload)`, or with a timeout in seconds:
/// `case!(files::immutable_upload, 120)`. The function is `async fn(t: &Ctx) -> Result<()>`.
#[macro_export]
macro_rules! case {
    ($function:path) => { $crate::case!($function, 300) };
    ($function:path, $seconds:expr) => {{
        fn run(t: &$crate::Ctx) -> $crate::harness::CaseFuture<'_> {
            Box::pin($function(t))
        }
        $crate::Case { name: stringify!($function), run, timeout: std::time::Duration::from_secs($seconds) }
    }};
}

/// What a case gets: its suite's fixture and a folder for its artifacts.
pub struct Ctx {
    pub suite: &'static str,
    dir: PathBuf,
    fixture: OnceCell<Arc<Instance>>,
}

impl Ctx {
    pub fn new(suite: &'static str, dir: PathBuf) -> Self {
        Self { suite, dir, fixture: OnceCell::new() }
    }

    /// The suite's standard instance (started on first use; see [`Instance::fixture`]).
    pub async fn fixture(&self) -> Result<Arc<Instance>> {
        self.fixture.get_or_try_init(|| async { Instance::fixture(&self.dir, &self.suite.replace(['_', ' '], "-")).await.map(Arc::new) }).await.cloned()
    }

    /// The suite's folder: logs, screenshots and the roots of its instances.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A new folder in the suite's folder.
    pub fn temp(&self, prefix: &str) -> Result<PathBuf> {
        for _ in 0..16 {
            let path = self.dir.join(format!("{prefix}-{}", crate::random_hex(4)));
            if std::fs::create_dir(&path).is_ok() {
                return Ok(path);
            }
        }
        Err(Error::new("could not create a folder for the case"))
    }

    /// Where a screenshot named `name` goes.
    pub fn screenshot(&self, name: &str) -> PathBuf {
        self.dir.join("screenshots").join(name)
    }

    /// Stops the fixture (the runner calls this after the last case).
    pub async fn close(&self) {
        if let Some(instance) = self.fixture.get() {
            instance.close().await;
        }
    }
}
