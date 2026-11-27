//! Dar's build, verify and dev commands. From Dar's dev shell (`nix develop`):
//!
//! ```text
//! cargo xtask setup              prepare the patched crates the workspace builds against
//! cargo xtask build [--release]  module, bindings, SDK bundle, example app, client, runtime
//! cargo xtask verify [--release] [--only <name>…] [--jobs <n>] [--no-unit]
//!                                build, then unit tests, the bridge's Go tests and the
//!                                acceptance suite (tests/), in parallel, on throwaway roots
//! cargo xtask dev [--release]    build and run Dar locally (data in ~/.local/share/dar-rust)
//! ```
//!
//! `build` and `verify` use the `fast` and `wasm-fast` profiles (optimized, incremental, no
//! LTO); `--release` builds what ships (`release` and `wasm-release`, wasm-opt, precompressed
//! assets). Steps whose inputs did not change are skipped.

mod client;
mod hash;
mod patches;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Instant, SystemTime};

pub type Result<T = ()> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let release = args.iter().any(|a| a == "--release");
    let result = match args.first().map(String::as_str) {
        Some("setup") => patches::prepare(),
        Some("module") => patches::prepare().and_then(|()| module()),
        Some("bindings") => bindings(),
        Some("client") => patches::prepare().and_then(|()| client::build(profile(release))),
        Some("runtime") => runtime(profile(release)),
        Some("build") => build(profile(release)),
        Some("verify") => verify(&args[1..]),
        Some("dev") => dev(profile(release)),
        _ => {
            eprintln!(
                "usage: cargo xtask setup | build [--release] | verify [--release] [--only <name>…] [--jobs <n>] [--no-unit] | dev [--release]\n       (steps: module, bindings, client [--release], runtime [--release])"
            );
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Profile {
    Fast,
    Release,
}

fn profile(release: bool) -> Profile {
    if release {
        Profile::Release
    } else {
        Profile::Fast
    }
}

impl Profile {
    /// The native profile (runtime, acceptance suite).
    pub fn native(self) -> &'static str {
        match self {
            Profile::Fast => "fast",
            Profile::Release => "release",
        }
    }
    /// The client's wasm profile.
    pub fn wasm(self) -> &'static str {
        match self {
            Profile::Fast => "wasm-fast",
            Profile::Release => "wasm-release",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Profile::Fast => "fast",
            Profile::Release => "release",
        }
    }
}

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

pub fn target() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo().join("target"))
}

pub fn module_wasm() -> PathBuf {
    std::env::var_os("DAR_MODULE_WASM")
        .map(PathBuf::from)
        .unwrap_or_else(|| target().join("wasm32-unknown-unknown/release/dar_module.wasm"))
}

pub fn runtime_bin(profile: Profile) -> PathBuf {
    target().join(profile.native()).join("dar-runtime")
}

pub fn client_dist(profile: Profile) -> PathBuf {
    target().join(format!("client-{}", profile.name()))
}

/// Runs a command in the repository, failing with its name.
pub fn run(command: &mut Command) -> Result {
    let shown = format!("{command:?}");
    let status = command
        .current_dir_if_unset()
        .status()
        .map_err(|e| format!("{shown}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{shown} failed ({status})"))
    }
}

trait CurrentDir {
    fn current_dir_if_unset(&mut self) -> &mut Self;
}

impl CurrentDir for Command {
    fn current_dir_if_unset(&mut self) -> &mut Self {
        if self.get_current_dir().is_none() {
            self.current_dir(repo());
        }
        self
    }
}

pub fn cargo() -> Command {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(repo());
    command
}

/// Times a step and says how long it took.
pub fn step<T>(name: &str, work: impl FnOnce() -> Result<T>) -> Result<T> {
    let started = Instant::now();
    let value = work().map_err(|e| format!("{name}: {e}"))?;
    eprintln!("xtask: {name} ({:.1}s)", started.elapsed().as_secs_f64());
    Ok(value)
}

/// The newest modification time under `dir`, skipping `dist`, `node_modules` and `target`.
pub fn newest(dir: &Path) -> SystemTime {
    let mut newest = SystemTime::UNIX_EPOCH;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return newest;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if ["dist", "node_modules", "target"].contains(&name.to_string_lossy().as_ref()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let time = if meta.is_dir() {
            newest_of(&path)
        } else {
            meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)
        };
        newest = newest.max(time);
    }
    newest
}

fn newest_of(dir: &Path) -> SystemTime {
    newest(dir)
}

pub fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn module() -> Result {
    step("module (wasm32, release)", || {
        run(cargo().args([
            "build",
            "-q",
            "-p",
            "dar-module",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
        ]))
    })
}

/// Regenerates `crates/bindings/src/generated` when the module changed, rewriting only the
/// files whose contents differ so unchanged bindings do not trigger rebuilds.
fn bindings() -> Result {
    let wasm = module_wasm();
    let out = repo().join("crates/bindings/src/generated");
    let stamp = target().join("bindings.stamp");
    let digest = hash::file(&wasm)
        .map_err(|e| format!("{}: {e} (build the module first)", wasm.display()))?;
    if out.join("mod.rs").exists() && std::fs::read_to_string(&stamp).is_ok_and(|s| s == digest) {
        return Ok(());
    }
    step("bindings", || {
        let scratch = target().join("bindings-tmp");
        let cli = target().join("bindings-cli");
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&cli).map_err(|e| e.to_string())?;
        // The 2.14.1 generator leaves fields named `type` unescaped, so its rustfmt pass fails
        // after the files are written; that is tolerated and fixed below.
        let _ = Command::new(spacetime())
            .arg("--root-dir")
            .arg(&cli)
            .args(["generate", "--lang", "rust", "--bin-path"])
            .arg(&wasm)
            .arg("--out-dir")
            .arg(&scratch)
            .args(["--yes", "--no-config"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !scratch.join("mod.rs").exists() {
            return Err("spacetime generate wrote no bindings".into());
        }
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&scratch)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                let fixed: String = text
                    .lines()
                    .map(|line| {
                        let indent = &line[..line.len() - line.trim_start().len()];
                        let rest = line.trim_start();
                        if let Some(after) = rest.strip_prefix("pub type: ") {
                            format!("{indent}pub r#type: {after}\n")
                        } else if let Some(after) = rest.strip_prefix("type: ") {
                            format!("{indent}r#type: {after}\n")
                        } else {
                            format!("{line}\n")
                        }
                    })
                    .collect();
                std::fs::write(&path, fixed).map_err(|e| e.to_string())?;
                files.push(path);
            }
        }
        if which("rustfmt") {
            let _ = Command::new("rustfmt")
                .args(["--edition", "2024"])
                .args(&files)
                .status();
        }
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        let mut keep = std::collections::HashSet::new();
        for file in &files {
            let name = file.file_name().unwrap().to_owned();
            let target = out.join(&name);
            let new = std::fs::read(file).map_err(|e| e.to_string())?;
            if std::fs::read(&target).ok().as_deref() != Some(&new[..]) {
                std::fs::write(&target, &new).map_err(|e| e.to_string())?;
            }
            keep.insert(name);
        }
        for entry in std::fs::read_dir(&out)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            if !keep.contains(&entry.file_name()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::write(&stamp, digest).map_err(|e| e.to_string())
    })
}

pub fn spacetime() -> PathBuf {
    std::env::var_os("DAR_SPACETIME_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| "spacetime".into())
}

pub fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

/// The installed-app SDK bundle (`sdk/dist`) and the example app, when their sources changed.
fn javascript() -> Result {
    let repo = repo();
    if !repo.join("node_modules").exists() {
        return Err("node_modules is missing: run `npm ci` in Dar's dev shell".into());
    }
    let sdk_out = repo.join("sdk/dist/sdk.js");
    if modified(&sdk_out).is_none_or(|built| newest(&repo.join("sdk")) > built) {
        step("SDK bundle", || {
            run(Command::new("node")
                .args(["sdk/build-bundle.mjs"])
                .stdout(Stdio::null()))
        })?;
    }
    let app = repo.join("apps/sample-app");
    if modified(&app.join("dist/dar-app.json"))
        .is_none_or(|built| newest(&app) > built || newest(&repo.join("sdk")) > built)
    {
        step("example app", || {
            run(Command::new("node")
                .args(["sdk/build.mjs", "apps/sample-app"])
                .stdout(Stdio::null()))
        })?;
    }
    Ok(())
}

fn runtime(profile: Profile) -> Result {
    patches::prepare()?;
    // The fast runtime is built with the acceptance suite: one feature resolution for the
    // dependencies they share, so neither build invalidates the other.
    let packages: &[&str] = match profile {
        Profile::Fast => &["-p", "dar-runtime", "-p", "dar-acceptance"],
        Profile::Release => &["-p", "dar-runtime"],
    };
    step(&format!("runtime ({})", profile.native()), || {
        run(cargo()
            .args(["build", "-q"])
            .args(packages)
            .args(["--profile", profile.native()]))
    })
}

fn build(profile: Profile) -> Result {
    let started = Instant::now();
    patches::prepare()?;
    javascript()?;
    module()?;
    bindings()?;
    client::build(profile)?;
    runtime(profile)?;
    eprintln!("xtask: built ({:.0}s)", started.elapsed().as_secs_f64());
    Ok(())
}

/// The environment that names the built stack for the acceptance suite and `dev`.
fn stack_env(command: &mut Command, profile: Profile) {
    let repo = repo();
    command
        .env("DAR_REPO", &repo)
        .env("DAR_RUST_MODULE", module_wasm())
        .env("DAR_RUST_RUNTIME", runtime_bin(profile))
        .env("DAR_CLIENT_DIST", client_dist(profile))
        .env("DAR_SAMPLE_APP_DIST", repo.join("apps/sample-app/dist"))
        .env("DAR_SPACETIME_BIN", spacetime());
}

fn verify(args: &[String]) -> Result {
    let started = Instant::now();
    let release = args.iter().any(|a| a == "--release");
    let profile = profile(release);
    let only: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, _)| *i > 0 && args[i - 1] == "--only")
        .map(|(_, v)| v)
        .collect();
    let jobs = args
        .iter()
        .position(|a| a == "--jobs")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let unit = !args.iter().any(|a| a == "--no-unit") && only.is_empty();
    build(profile)?;
    step("acceptance suite (build)", || {
        run(cargo().args([
            "build",
            "-q",
            "-p",
            "dar-runtime",
            "-p",
            "dar-acceptance",
            "--profile",
            profile.native(),
        ]))
    })?;
    let mut results: Vec<(String, bool, f64)> = Vec::new();
    std::thread::scope(|scope| {
        let go =
            (only.is_empty() || only.iter().any(|o| "bridge".contains(o.as_str()))).then(|| {
                scope.spawn(|| {
                    let started = Instant::now();
                    let ok = Command::new("go")
                        .args(["test", "./..."])
                        .current_dir(repo().join("bridge"))
                        .env("CGO_ENABLED", "0")
                        .env("GOTOOLCHAIN", "local")
                        .env("GOFLAGS", "-mod=readonly")
                        .status()
                        .is_ok_and(|s| s.success());
                    (
                        "bridge go test".to_owned(),
                        ok,
                        started.elapsed().as_secs_f64(),
                    )
                })
            });
        let unit = unit.then(|| {
            scope.spawn(|| {
                let started = Instant::now();
                // Native unit tests of every crate, then the module's and client's libraries
                // (their cdylib and wasm binaries do not link natively). The test profile keeps their
                // dev-dependency features away from the `fast` build.
                let ok = run(cargo().args([
                    "test",
                    "-q",
                    "-p",
                    "dar-runtime",
                    "-p",
                    "dar-notes",
                    "-p",
                    "dar-pdf",
                    "-p",
                    "dar-oidc",
                    "-p",
                    "dar-dav",
                    "-p",
                    "dar-acceptance",
                    "--lib",
                    "--tests",
                ]))
                .is_ok()
                    && run(cargo().args([
                        "test",
                        "-q",
                        "-p",
                        "dar-module",
                        "-p",
                        "dar-client",
                        "-p",
                        "dar-pdf-worker",
                        "--lib",
                    ]))
                    .is_ok();
                (
                    "unit tests (cargo test)".to_owned(),
                    ok,
                    started.elapsed().as_secs_f64(),
                )
            })
        });
        let acceptance = scope.spawn(|| {
            let started = Instant::now();
            let mut command = Command::new(target().join(profile.native()).join("dar-acceptance"));
            stack_env(&mut command, profile);
            for name in &only {
                command.args(["--only", name]);
            }
            if let Some(jobs) = &jobs {
                command.args(["--jobs", jobs]);
            }
            let ok = command
                .current_dir(repo())
                .status()
                .is_ok_and(|s| s.success());
            (
                "acceptance suite".to_owned(),
                ok,
                started.elapsed().as_secs_f64(),
            )
        });
        for handle in [go, unit].into_iter().flatten().chain([acceptance]) {
            results.push(
                handle
                    .join()
                    .unwrap_or_else(|_| ("a check".into(), false, 0.0)),
            );
        }
    });
    eprintln!("\nverify:");
    for (name, ok, seconds) in &results {
        eprintln!(
            "{} {:>5.0}s {name}",
            if *ok { "PASS" } else { "FAIL" },
            seconds
        );
    }
    let failed = results.iter().filter(|(_, ok, _)| !ok).count();
    eprintln!(
        "{} of {} checks passed in {:.0}s.",
        results.len() - failed,
        results.len(),
        started.elapsed().as_secs_f64()
    );
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} check(s) failed"))
    }
}

/// Runs Dar locally: SpacetimeDB with the module, then the runtime, on a persistent root.
/// Environment: DAR_ROOT (~/.local/share/dar-rust), DAR_PORT (4331), DAR_DATABASE_PORT (4330),
/// DAR_HOST (127.0.0.1), DAR_ORIGIN (http://127.0.0.1:$DAR_PORT; an https:// origin, such as
/// `tailscale serve` in front of the loopback port, runs in production mode). An existing root
/// and database (also a TypeScript instance's) are taken over in place.
fn dev(profile: Profile) -> Result {
    build(profile)?;
    let var = |name: &str, default: String| {
        std::env::var(name)
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or(default)
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let data = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or(format!("{home}/.local/share"));
    let root = PathBuf::from(var("DAR_ROOT", format!("{data}/dar-rust")));
    let port = var("DAR_PORT", "4331".into());
    let database_port = var("DAR_DATABASE_PORT", "4330".into());
    let host = var("DAR_HOST", "127.0.0.1".into());
    let origin = var("DAR_ORIGIN", format!("http://127.0.0.1:{port}"));
    let name = var("DAR_DATABASE_NAME", "dar".into());
    let development = !origin.starts_with("https://");
    let database_url = format!("http://127.0.0.1:{database_port}");
    let cli_root = root.join("cli");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    // A new root is marked as a Dar root before anything is put in it.
    if !root.join("instance.json").exists()
        && std::fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .next()
            .is_none()
    {
        std::fs::write(
            root.join("instance.json"),
            r#"{"format":"dar-spacetime-v1"}"#,
        )
        .map_err(|e| e.to_string())?;
    }
    for dir in ["secrets", "database"] {
        std::fs::create_dir_all(root.join(dir)).map_err(|e| e.to_string())?;
    }
    let bootstrap = root.join("secrets/bootstrap-token");
    if !bootstrap.exists() {
        let mut bytes = [0u8; 32];
        use std::io::Read as _;
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .map_err(|e| e.to_string())?;
        std::fs::write(&bootstrap, hash::base64url(&bytes)).map_err(|e| e.to_string())?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("database.log"))
        .map_err(|e| e.to_string())?;
    let mut database = Command::new(spacetime())
        .arg("--root-dir")
        .arg(&cli_root)
        .args(["start", "--data-dir"])
        .arg(root.join("database"))
        .args([
            "--listen-addr",
            &format!("127.0.0.1:{database_port}"),
            "--non-interactive",
        ])
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("spacetime start: {e}"))?;
    let stop = |child: &mut std::process::Child| {
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
        let _ = child.wait();
    };
    let result = (|| {
        wait_for(&format!("127.0.0.1:{database_port}"), "/v1/ping")?;
        run(Command::new(spacetime())
            .arg("--root-dir")
            .arg(&cli_root)
            .args(["publish", &name, "--bin-path"])
            .arg(module_wasm())
            .args(["--server", &database_url, "--yes", "--no-config"])
            .env("DAR_ISSUER", format!("{origin}/auth/issuer"))
            .stdout(Stdio::null()))?;
        let config =
            std::fs::read_to_string(cli_root.join("config/cli.toml")).map_err(|e| e.to_string())?;
        let token = config
            .lines()
            .find_map(|l| {
                l.trim().strip_prefix("spacetimedb_token").map(|r| {
                    r.trim_start_matches([' ', '='])
                        .trim()
                        .trim_matches('"')
                        .to_owned()
                })
            })
            .filter(|t| !t.is_empty())
            .ok_or("The local database service identity was not initialized.")?;
        let token_file = root.join("secrets/database-token");
        match std::fs::read_to_string(&token_file) {
            Ok(stored) if stored != token => {
                return Err("Stored database identity changed; refusing to overwrite it.".into());
            }
            Ok(_) => {}
            Err(_) => std::fs::write(&token_file, &token).map_err(|e| e.to_string())?,
        }
        let mut runtime = Command::new(runtime_bin(profile))
            .env("DAR_ROOT", &root)
            .env("DAR_HOST", &host)
            .env("DAR_ORIGIN", &origin)
            .env("DAR_PORT", &port)
            .env("DAR_DATABASE_URL", &database_url)
            .env("DAR_DATABASE_NAME", &name)
            .env("DAR_DATABASE_TOKEN_FILE", &token_file)
            .env("DAR_BOOTSTRAP_TOKEN_FILE", &bootstrap)
            .env("DAR_DEVELOPMENT", if development { "1" } else { "" })
            .env("DAR_CLIENT_DIST", client_dist(profile))
            .spawn()
            .map_err(|e| e.to_string())?;
        std::fs::write(
            root.join("service-pids.json"),
            format!(
                r#"{{"database":{},"runtime":{}}}"#,
                database.id(),
                runtime.id()
            ),
        )
        .map_err(|e| e.to_string())?;
        wait_for(&format!("127.0.0.1:{port}"), "/api/v1/auth/status")?;
        eprintln!(
            "Dar is ready at {origin}. The operator setup key is in {}.",
            bootstrap.display()
        );
        let status = runtime.wait().map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("the runtime exited ({status})"))
        }
    })();
    stop(&mut database);
    result
}

/// Waits up to 30 s for an HTTP 200 from `address` + `path` (plain HTTP/1.1 over TCP).
fn wait_for(address: &str, path: &str) -> Result {
    use std::io::{Read as _, Write as _};
    let deadline = Instant::now() + std::time::Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(mut stream) = std::net::TcpStream::connect(address) {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(1)));
            let _ = write!(
                stream,
                "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
            );
            let mut head = [0u8; 12];
            if stream.read_exact(&mut head).is_ok() && head.starts_with(b"HTTP/1.1 200") {
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(format!("timed out waiting for http://{address}{path}"))
}
