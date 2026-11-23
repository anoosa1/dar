//! The run's shared pieces: the built artifacts under test and one SpacetimeDB server that
//! every instance's database lives on (suites that restart the database start their own).

use crate::{Error, Result, ResultExt, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What the suite runs: the module, runtime and client built by `cargo xtask build`.
#[derive(Debug, Clone)]
pub struct Artifacts {
    pub repo: PathBuf,
    pub module: PathBuf,
    pub runtime: PathBuf,
    pub client: PathBuf,
    pub spacetime: PathBuf,
    /// The example installed app, built by the SDK (`apps/prayer-times/dist`).
    pub prayer_times: PathBuf,
}

impl Artifacts {
    /// From `DAR_RUST_MODULE`, `DAR_RUST_RUNTIME`, `DAR_CLIENT_DIST` and `DAR_SPACETIME_BIN`
    /// (set by `cargo xtask verify`), or the repository's usual build outputs.
    pub fn from_env() -> Result<Self> {
        let repo = std::env::var_os("DAR_REPO").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf());
        let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| repo.join("target"));
        let pick = |variable: &str, default: PathBuf| std::env::var_os(variable).map(PathBuf::from).unwrap_or(default);
        let artifacts = Artifacts {
            module: pick("DAR_RUST_MODULE", target.join("wasm32-unknown-unknown/module/dar_module.wasm")),
            runtime: pick("DAR_RUST_RUNTIME", target.join("fast/dar-runtime")),
            client: pick("DAR_CLIENT_DIST", target.join("client-dist")),
            spacetime: pick("DAR_SPACETIME_BIN", crate::which("spacetime").unwrap_or_else(|| "spacetime".into())),
            prayer_times: pick("DAR_PRAYER_TIMES_DIST", repo.join("apps/prayer-times/dist")),
            repo,
        };
        for (what, path) in [("module", &artifacts.module), ("runtime", &artifacts.runtime), ("client", &artifacts.client)] {
            if !path.exists() {
                bail!("the {what} is not built ({}): run `cargo xtask build` first", path.display());
            }
        }
        Ok(artifacts)
    }
}

/// A SpacetimeDB server with its CLI identity (the service credential instances use).
pub struct SpacetimeServer {
    pub url: String,
    pub port: u16,
    pub cli_root: PathBuf,
    pub data_dir: PathBuf,
    /// The owner token the runtime calls service reducers with.
    pub token_file: PathBuf,
    pub log: PathBuf,
    child: Mutex<Option<Child>>,
}

impl SpacetimeServer {
    /// Starts a server in `root` and obtains the CLI identity by publishing a first database.
    pub async fn start(root: &Path, artifacts: &Artifacts) -> Result<SpacetimeServer> {
        std::fs::create_dir_all(root)?;
        let port = free_port()?;
        let server = SpacetimeServer {
            url: format!("http://127.0.0.1:{port}"),
            port,
            cli_root: root.join("cli"),
            data_dir: root.join("database"),
            token_file: root.join("secrets/database-token"),
            log: root.join("database.log"),
            child: Mutex::new(None),
        };
        std::fs::create_dir_all(&server.data_dir)?;
        std::fs::create_dir_all(root.join("secrets"))?;
        server.launch(artifacts)?;
        server.ready().await?;
        server.publish(artifacts, "dar-identity", &server.url.clone())?;
        let config = crate::read_to_string(server.cli_root.join("config/cli.toml"))?;
        let token = config.lines().find_map(|line| line.trim().strip_prefix("spacetimedb_token").map(|rest| rest.trim_start_matches([' ', '=']).trim().trim_matches('"').to_owned()))
            .filter(|token| !token.is_empty()).context("the database's CLI identity was not initialized")?;
        write_private(&server.token_file, token.as_bytes())?;
        Ok(server)
    }

    fn launch(&self, artifacts: &Artifacts) -> Result<()> {
        let log = std::fs::OpenOptions::new().create(true).append(true).open(&self.log)?;
        let child = Command::new(&artifacts.spacetime)
            .args(["--root-dir"]).arg(&self.cli_root)
            .args(["start", "--data-dir"]).arg(&self.data_dir)
            .args(["--listen-addr", &format!("127.0.0.1:{}", self.port), "--non-interactive"])
            .stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log)
            .spawn().context("starting SpacetimeDB")?;
        *self.child.lock().unwrap() = Some(child);
        Ok(())
    }

    async fn ready(&self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(60);
        let client = crate::session::client();
        loop {
            if let Some(child) = self.child.lock().unwrap().as_mut()
                && let Some(status) = child.try_wait()?
            {
                bail!("SpacetimeDB stopped during startup ({status}):\n{}", tail(&self.log));
            }
            if let Ok(response) = client.get(format!("{}/v1/ping", self.url)).timeout(Duration::from_millis(500)).send().await
                && response.status().is_success()
            {
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("SpacetimeDB did not answer at {}:\n{}", self.url, tail(&self.log));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Publishes the module as database `name`, trusting `origin`'s issuer.
    pub fn publish(&self, artifacts: &Artifacts, name: &str, origin: &str) -> Result<()> {
        let output = Command::new(&artifacts.spacetime)
            .arg("--root-dir").arg(&self.cli_root)
            .args(["publish", name, "--bin-path"]).arg(&artifacts.module)
            .args(["--server", &self.url, "--yes", "--no-config"])
            .env("DAR_ISSUER", format!("{origin}/auth/issuer"))
            .stdin(Stdio::null()).output().context("running spacetime publish")?;
        if !output.status.success() {
            bail!("publishing {name} failed:\n{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        Ok(())
    }

    /// Stops the server (SIGTERM, then SIGKILL after 15 s).
    pub fn stop(&self) {
        if let Some(child) = self.child.lock().unwrap().take() {
            terminate(child, Duration::from_secs(15));
        }
    }

    /// Restarts the server on the same port and data (restart suite).
    pub async fn restart(&self, artifacts: &Artifacts) -> Result<()> {
        self.stop();
        self.launch(artifacts)?;
        self.ready().await
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.lock().unwrap().as_ref().map(Child::id)
    }
}

impl Drop for SpacetimeServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The run: artifacts, its root, the shared database server and the bootstrap token file.
pub struct Stack {
    pub artifacts: Artifacts,
    pub root: PathBuf,
    pub database: SpacetimeServer,
    pub bootstrap_token_file: PathBuf,
}

static STACK: OnceLock<Stack> = OnceLock::new();

/// The run's stack. The runner starts it before any suite.
pub fn stack() -> &'static Stack {
    STACK.get().expect("the stack is started by the runner before suites run")
}

pub async fn start(root: PathBuf) -> Result<&'static Stack> {
    let artifacts = Artifacts::from_env()?;
    let database = SpacetimeServer::start(&root.join("spacetime"), &artifacts).await?;
    let bootstrap_token_file = root.join("spacetime/secrets/bootstrap-token");
    write_private(&bootstrap_token_file, crate::base64url(&crate::random_bytes(32)).as_bytes())?;
    STACK.set(Stack { artifacts, root, database, bootstrap_token_file }).map_err(|_| Error::new("the stack started twice"))?;
    Ok(stack())
}

/// Stops the shared database (the runner calls this at exit; statics are not dropped).
pub fn shutdown() {
    if let Some(stack) = STACK.get() {
        stack.database.stop();
    }
}

/// A free local port that no other suite of this run was given.
pub fn free_port() -> Result<u16> {
    static GIVEN: Mutex<Option<HashSet<u16>>> = Mutex::new(None);
    for _ in 0..64 {
        let port = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        if GIVEN.lock().unwrap().get_or_insert_with(HashSet::new).insert(port) {
            return Ok(port);
        }
    }
    bail!("no free port")
}

pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents).context(path.display())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The last 16 KB of a log, for failure messages.
pub fn tail(path: &Path) -> String {
    let text = std::fs::read(path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
    let start = text.len().saturating_sub(16_000);
    let start = (start..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
    text[start..].to_owned()
}

/// SIGTERM, then SIGKILL if the process is still running after `grace`.
pub fn terminate(mut child: Child, grace: Duration) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    let _ = Command::new("kill").args(["-TERM", &child.id().to_string()]).status();
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();
}
