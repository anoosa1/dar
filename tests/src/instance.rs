//! Isolated Dar instances (`instance.ts`): their own database on the run's SpacetimeDB server,
//! their own root and port, running the runtime under test. [`Instance::fixture`] is the
//! standard instance most suites use: the example app installed, a configured OIDC client, the
//! fake chat bridge, and the administrator account set up.

use crate::stack::{Stack, free_port, stack, tail, terminate, write_private};
use crate::{Result, ResultExt, bail, json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct InstanceConfig {
    /// The public origin; `http://127.0.0.1:<port>` unless set (proxies set their own).
    pub origin: Option<String>,
    pub root: PathBuf,
    pub database_name: String,
    /// 0 picks a free port.
    pub port: u16,
    pub development: bool,
    pub chat_bridge: Option<PathBuf>,
    pub news_allow_private_networks: bool,
    pub immich_allow_local_host: bool,
    pub codex_file: Option<PathBuf>,
    pub claude_file: Option<PathBuf>,
    pub apps_file: Option<PathBuf>,
    pub oidc_file: Option<PathBuf>,
    /// Further environment for the runtime (Mail: `DAR_MAIL_PROVIDERS_FILE`, `SSL_CERT_FILE`).
    pub env: Vec<(String, String)>,
    /// Another database server and its tokens (defaults: the run's).
    pub database_url: Option<String>,
    pub service_token_file: Option<PathBuf>,
    pub bootstrap_token_file: Option<PathBuf>,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            origin: None,
            root: PathBuf::new(),
            database_name: String::new(),
            port: 0,
            development: true,
            chat_bridge: None,
            news_allow_private_networks: false,
            immich_allow_local_host: false,
            codex_file: None,
            claude_file: None,
            apps_file: None,
            oidc_file: None,
            env: Vec::new(),
            database_url: None,
            service_token_file: None,
            bootstrap_token_file: None,
        }
    }
}

pub struct Instance {
    /// `http://127.0.0.1:<port>` or the configured origin.
    pub base: String,
    pub port: u16,
    pub config: InstanceConfig,
    /// Where the runtime's output goes.
    pub log: PathBuf,
    /// The fake chat bridge's control folder (fixture instances).
    pub chat_control: Option<PathBuf>,
    child: Mutex<Option<Child>>,
}

impl Instance {
    /// Starts the runtime on an existing database (`startInstance`).
    pub async fn start(mut config: InstanceConfig) -> Result<Instance> {
        if config.port == 0 {
            config.port = free_port()?;
        }
        let base = config
            .origin
            .clone()
            .unwrap_or_else(|| format!("http://127.0.0.1:{}", config.port));
        std::fs::create_dir_all(&config.root)?;
        let log = config
            .root
            .parent()
            .unwrap_or(&config.root)
            .join(format!("runtime-{}.log", config.database_name));
        let instance = Instance {
            base,
            port: config.port,
            config,
            log,
            chat_control: None,
            child: Mutex::new(None),
        };
        instance.launch().await?;
        Ok(instance)
    }

    /// A fresh database and root: publishes the module, then starts the runtime
    /// (`isolatedInstance`). The root is `<root>/state` unless `state_directory` is false.
    pub async fn isolated(
        root: &Path,
        prefix: &str,
        mut config: InstanceConfig,
        state_directory: bool,
    ) -> Result<Instance> {
        let stack = stack();
        config.database_name = format!("{prefix}-{}", &crate::uuid()[..8]);
        if config.port == 0 {
            config.port = free_port()?;
        }
        let base = config
            .origin
            .clone()
            .unwrap_or_else(|| format!("http://127.0.0.1:{}", config.port));
        config.root = if state_directory {
            root.join("state")
        } else {
            root.to_path_buf()
        };
        publish(stack, &config, &config.database_name, &base)?;
        Instance::start(config).await
    }

    /// The standard instance: the example app, a configured OIDC client (`configured-rp`), the
    /// fake chat bridge and the administrator `admin` with [`crate::PASSWORD`].
    pub async fn fixture(dir: &Path, prefix: &str) -> Result<Instance> {
        let stack = stack();
        let root = dir.join(format!("{prefix}-root"));
        std::fs::create_dir_all(&root)?;
        let installed = Command::new(&stack.artifacts.runtime)
            .arg("apps-install")
            .arg(&root)
            .arg(&stack.artifacts.prayer_times)
            .stdin(Stdio::null())
            .output()
            .context("installing the example app")?;
        if !installed.status.success() {
            bail!(
                "the example app was not installed:\n{}",
                String::from_utf8_lossy(&installed.stderr)
            );
        }
        write_private(
            &root.join("oidc-secret"),
            b"isolated-configured-secret-1234567890\n",
        )?;
        std::fs::write(root.join("oidc.json"), json!({"clients": [{"id": "configured-rp", "name": "Configured RP", "type": "confidential",
            "redirectUris": ["http://127.0.0.1:49999/callback"], "scopes": ["openid", "profile"], "secretFile": "oidc-secret"}]}).to_string())?;
        let control = root.join("chat-control");
        let bridge = fixture_command(
            &root,
            "chat-bridge",
            &[("DAR_FAKE_CHAT_CONTROL", control.to_string_lossy().as_ref())],
        )?;
        std::fs::create_dir_all(&control)?;
        let config = InstanceConfig {
            chat_bridge: Some(bridge),
            ..InstanceConfig::default()
        };
        let mut instance = Instance::isolated(&root, prefix, config, false).await?;
        instance.chat_control = Some(control);
        crate::session::setup_admin(&instance.base).await?;
        Ok(instance)
    }

    fn environment(&self) -> Vec<(String, String)> {
        let stack = stack();
        let c = &self.config;
        let flag = |value: bool| if value { "1" } else { "0" }.to_owned();
        let path = |p: &Path| p.to_string_lossy().into_owned();
        let mut env = vec![
            ("DAR_HOST".into(), "127.0.0.1".into()),
            ("DAR_PORT".into(), c.port.to_string()),
            ("DAR_ORIGIN".into(), self.base.clone()),
            ("DAR_ROOT".into(), path(&c.root)),
            (
                "DAR_DATABASE_URL".into(),
                c.database_url
                    .clone()
                    .unwrap_or_else(|| stack.database.url.clone()),
            ),
            ("DAR_DATABASE_NAME".into(), c.database_name.clone()),
            (
                "DAR_DATABASE_TOKEN_FILE".into(),
                path(
                    c.service_token_file
                        .as_deref()
                        .unwrap_or(&stack.database.token_file),
                ),
            ),
            (
                "DAR_BOOTSTRAP_TOKEN_FILE".into(),
                path(
                    c.bootstrap_token_file
                        .as_deref()
                        .unwrap_or(&stack.bootstrap_token_file),
                ),
            ),
            ("DAR_DEVELOPMENT".into(), flag(c.development)),
            (
                "DAR_NEWS_ALLOW_PRIVATE_NETWORKS".into(),
                flag(c.news_allow_private_networks),
            ),
            (
                "DAR_IMMICH_ALLOW_LOCAL_HOST".into(),
                flag(c.immich_allow_local_host),
            ),
            ("DAR_ISSUER".into(), format!("{}/auth/issuer", self.base)),
            ("DAR_CLIENT_DIST".into(), path(&stack.artifacts.client)),
            ("DAR_SPACETIME_BIN".into(), path(&stack.artifacts.spacetime)),
        ];
        for (name, value) in [
            ("DAR_CHAT_BRIDGE", &c.chat_bridge),
            ("DAR_CODEX_FILE", &c.codex_file),
            ("DAR_CLAUDE_FILE", &c.claude_file),
            ("DAR_APPS_FILE", &c.apps_file),
            ("DAR_OIDC_FILE", &c.oidc_file),
        ] {
            if let Some(value) = value {
                env.push((name.into(), path(value)));
            }
        }
        env.extend(c.env.iter().cloned());
        env
    }

    async fn launch(&self) -> Result<()> {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)?;
        let child = Command::new(&stack().artifacts.runtime)
            .envs(self.environment())
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()
            .context("starting the runtime")?;
        *self.child.lock().unwrap() = Some(child);
        let deadline = Instant::now() + Duration::from_secs(30);
        let client = crate::session::client();
        let status = format!("http://127.0.0.1:{}/api/v1/auth/status", self.port);
        loop {
            if let Some(child) = self.child.lock().unwrap().as_mut()
                && let Some(code) = child.try_wait()?
            {
                bail!(
                    "the runtime stopped during startup ({code}):\n{}",
                    tail(&self.log)
                );
            }
            if let Ok(response) = client
                .get(&status)
                .timeout(Duration::from_millis(500))
                .send()
                .await
                && response.status().is_success()
            {
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("the runtime did not become ready:\n{}", tail(&self.log));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Stops the runtime (SIGTERM, then SIGKILL after 15 s).
    pub async fn close(&self) {
        let child = self.child.lock().unwrap().take();
        if let Some(child) = child {
            tokio::task::spawn_blocking(move || terminate(child, Duration::from_secs(15)))
                .await
                .ok();
        }
    }

    /// Abruptly terminates the runtime with SIGKILL and reaps the child process.
    pub async fn kill_now(&self) -> Result<()> {
        let child = self.child.lock().unwrap().take();
        if let Some(mut child) = child {
            tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                let _ = child.kill();
                let _ = child.wait()?;
                Ok(())
            })
            .await
            .map_err(|e| crate::Error::new(format!("kill join error: {e}")))??;
        }
        Ok(())
    }

    pub async fn restart(&self) -> Result<()> {
        self.close().await;
        self.launch().await
    }

    /// The runtime's process id while it runs.
    pub fn pid(&self) -> Option<u32> {
        self.child.lock().unwrap().as_ref().map(Child::id)
    }

    /// `base` + `path`.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The instance's root (`DAR_ROOT`).
    pub fn root(&self) -> &Path {
        &self.config.root
    }

    /// The service credential's view of this instance's database.
    pub fn database(&self) -> Result<crate::support::Database> {
        let stack = stack();
        crate::support::Database::new(
            self.config
                .database_url
                .as_deref()
                .unwrap_or(&stack.database.url),
            &self.config.database_name,
            self.config
                .service_token_file
                .as_deref()
                .unwrap_or(&stack.database.token_file),
        )
    }

    /// The runtime's output so far (the last 16 KB).
    pub fn output(&self) -> String {
        tail(&self.log)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        if let Some(child) = self.child.get_mut().unwrap().take() {
            terminate(child, Duration::from_secs(5));
        }
    }
}

/// Publishes the module as database `name` on the configured (or the run's) server.
pub fn publish(stack: &Stack, config: &InstanceConfig, name: &str, origin: &str) -> Result<()> {
    if config.database_url.is_some() {
        bail!("publish on another database server through its own SpacetimeServer");
    }
    stack.database.publish(&stack.artifacts, name, origin)
}

/// Publishes the module as a new database `name`, trusting `origin`'s issuer (`publishModule`).
pub fn publish_module(name: &str, origin: &str) -> Result<()> {
    let stack = stack();
    stack.database.publish(&stack.artifacts, name, origin)
}

/// A script in `dir` that runs one of the suite's fixture programs (the fake chat bridge, the
/// Codex and Claude fixtures, the mail server…) from this test binary, with `env`.
pub fn fixture_command(dir: &Path, fixture: &str, env: &[(&str, &str)]) -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let quote = |value: &str| format!("'{}'", value.replace('\'', r"'\''"));
    let exports: String = env
        .iter()
        .map(|(name, value)| format!("export {name}={}\n", quote(value)))
        .collect();
    let script = dir.join(format!("{fixture}.sh"));
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n{exports}exec {} fixture {} \"$@\"\n",
            quote(&exe.to_string_lossy()),
            quote(fixture)
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(script)
}
