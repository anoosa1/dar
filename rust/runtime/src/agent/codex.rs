//! Codex CLI options, account check, model catalog and sandbox verification
//! (`next/runtime/agent/codex.ts`).

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::agent::{CancelToken, kill_process_group, utf16_len, valid_effort};
use crate::error::{AppError, Result};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexConfig {
    pub executable: String,
    pub home: String,
    pub user_id: String,
    pub git_hosts: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexModel {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

pub const CODEX_INSTRUCTIONS: &str = "Work only in the supplied workspace. Follow repository AGENTS.md instructions within that scope. Do not publish, deploy, access credentials, or request broader permissions. Network and host access are unavailable. Earlier conversation is context; the final user message is the current task.";

pub fn object(value: Value) -> Result<Map<String, Value>> {
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(AppError::new("invalidProviderResponse", 502)),
    }
}

pub fn string_field(value: Option<&Value>, max: usize) -> Result<String> {
    match value.and_then(Value::as_str) {
        Some(s) if utf16_len(s) <= max => Ok(s.to_owned()),
        _ => Err(AppError::new("invalidProviderResponse", 502)),
    }
}

fn toml_inline_table(entries: &[(&str, &str)]) -> String {
    let mut out = String::from("{");
    for (i, (k, v)) in entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(k).unwrap_or_default());
        out.push('=');
        out.push_str(&serde_json::to_string(v).unwrap_or_default());
    }
    out.push('}');
    out
}

pub fn options(config: &CodexConfig, workspace: &Path) -> Vec<String> {
    let git_dir = workspace.join(".git").to_string_lossy().into_owned();
    let codex_dir = workspace.join(".codex").to_string_lossy().into_owned();
    let workspace_str = workspace.to_string_lossy().into_owned();
    let path_env = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());

    let fs_table = toml_inline_table(&[
        (":minimal", "read"),
        (":workspace_roots", "write"),
        ("/nix/store", "read"),
        ("/proc", "deny"),
        ("/sys", "deny"),
        (&config.home, "deny"),
        (&git_dir, "read"),
        (&codex_dir, "deny"),
    ]);
    let env_table = toml_inline_table(&[("PATH", &path_env), ("HOME", &workspace_str)]);

    vec![
        "--disable".into(),
        "apps".into(),
        "--disable".into(),
        "plugins".into(),
        "--disable".into(),
        "hooks".into(),
        "--disable".into(),
        "multi_agent".into(),
        "--disable".into(),
        "shell_snapshot".into(),
        "-c".into(),
        "mcp_servers={}".into(),
        "-c".into(),
        "web_search=\"disabled\"".into(),
        "-c".into(),
        "projects={}".into(),
        "-c".into(),
        "forced_login_method=\"chatgpt\"".into(),
        "-c".into(),
        "approval_policy=\"never\"".into(),
        "-c".into(),
        "default_permissions=\"dar\"".into(),
        "-c".into(),
        format!("permissions.dar.filesystem={fs_table}"),
        "-c".into(),
        "permissions.dar.network.enabled=false".into(),
        "-c".into(),
        "shell_environment_policy.inherit=\"none\"".into(),
        "-c".into(),
        format!("shell_environment_policy.set={env_table}"),
    ]
}

struct CommandOutput {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

async fn command(
    config: &CodexConfig,
    workspace: &Path,
    args: &[String],
    cancel: Option<CancelToken>,
    timeout: Duration,
) -> Result<CommandOutput> {
    if let Some(ref c) = cancel {
        c.throw_if_cancelled()?;
    }
    let mut cmd = Command::new(&config.executable);
    cmd.args(args)
        .current_dir(workspace)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .env("HOME", &config.home)
        .env("CODEX_HOME", &config.home)
        .env("LANG", "C.UTF-8")
        .env("NO_COLOR", "1");
    if let Ok(path_env) = std::env::var("PATH") {
        cmd.env("PATH", path_env);
    }

    let mut child = cmd.spawn().map_err(|_| {
        AppError::with(
            "codexUnavailable",
            503,
            "The Codex CLI could not be started.",
        )
    })?;
    let pid = child.id().unwrap_or(0);
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.shutdown().await;
    }
    let mut stdout = child.stdout.take().expect("codex stdout");
    let mut stderr = child.stderr.take().expect("codex stderr");

    let failure: Arc<Mutex<Option<AppError>>> = Arc::new(Mutex::new(None));

    let fail_out = failure.clone();
    let out_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if collected.len() + n > 4 * 1024 * 1024 {
                        let mut guard = fail_out.lock().await;
                        if guard.is_none() {
                            *guard = Some(AppError::new("invalidProviderResponse", 502));
                        }
                        kill_process_group(pid);
                        break;
                    }
                    collected.extend_from_slice(&buf[..n]);
                }
                Err(_) => {
                    let mut guard = fail_out.lock().await;
                    if guard.is_none() {
                        *guard = Some(AppError::new("codexUnavailable", 503));
                    }
                    kill_process_group(pid);
                    break;
                }
            }
        }
        collected
    });

    let err_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if collected.len() < 16_384 {
                        let take = n.min(16_384 - collected.len());
                        collected.extend_from_slice(&buf[..take]);
                    }
                }
                Err(_) => break,
            }
        }
        collected
    });

    let status = if let Some(c) = cancel {
        tokio::select! {
            res = child.wait() => res.ok(),
            _ = tokio::time::sleep(timeout) => {
                let mut guard = failure.lock().await;
                if guard.is_none() {
                    *guard = Some(AppError::with("codexTimedOut", 504, "The Codex CLI timed out."));
                }
                drop(guard);
                kill_process_group(pid);
                child.wait().await.ok()
            }
            _ = c.cancelled() => {
                let mut guard = failure.lock().await;
                if guard.is_none() {
                    *guard = Some(AppError::new("runStopped", 409));
                }
                drop(guard);
                kill_process_group(pid);
                child.wait().await.ok()
            }
        }
    } else {
        tokio::select! {
            res = child.wait() => res.ok(),
            _ = tokio::time::sleep(timeout) => {
                let mut guard = failure.lock().await;
                if guard.is_none() {
                    *guard = Some(AppError::with("codexTimedOut", 504, "The Codex CLI timed out."));
                }
                drop(guard);
                kill_process_group(pid);
                child.wait().await.ok()
            }
        }
    };

    let out_bytes = out_task.await.unwrap_or_default();
    let err_bytes = err_task.await.unwrap_or_default();
    if let Some(err) = failure.lock().await.clone() {
        return Err(err);
    }
    Ok(CommandOutput {
        code: status.and_then(|s| s.code()),
        stdout: String::from_utf8_lossy(&out_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&err_bytes).into_owned(),
    })
}

pub async fn codex_account(
    config: &CodexConfig,
    workspace: &Path,
    cancel: Option<CancelToken>,
) -> Result<bool> {
    let mut args = vec!["login".into(), "status".into()];
    args.extend(options(config, workspace));
    let result = command(config, workspace, &args, cancel, Duration::from_secs(15)).await?;
    let combined = format!("{}{}", result.stdout, result.stderr).to_ascii_lowercase();
    Ok(result.code == Some(0) && combined.contains("logged in using chatgpt"))
}

pub async fn codex_models(
    config: &CodexConfig,
    workspace: &Path,
    cancel: Option<CancelToken>,
) -> Result<Vec<CodexModel>> {
    let mut args = vec!["debug".into(), "models".into(), "--bundled".into()];
    args.extend(options(config, workspace));
    let result = command(config, workspace, &args, cancel, Duration::from_secs(15)).await?;
    if result.code != Some(0) {
        return Err(AppError::with(
            "codexUnavailable",
            503,
            "The Codex CLI could not list models.",
        ));
    }
    let parsed: Value = serde_json::from_str(&result.stdout)
        .map_err(|_| AppError::new("invalidProviderResponse", 502))?;
    let value = object(parsed)?;
    let raw_models = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::new("invalidProviderResponse", 502))?;

    let mut rows: Vec<Map<String, Value>> = Vec::new();
    for item in raw_models {
        let row = object(item.clone())?;
        if row.get("visibility").and_then(Value::as_str) == Some("list") {
            rows.push(row);
        }
    }
    rows.sort_by(|a, b| {
        let pa = a.get("priority").and_then(Value::as_f64).unwrap_or(1000.0);
        let pb = b.get("priority").and_then(Value::as_f64).unwrap_or(1000.0);
        pa.total_cmp(&pb)
    });

    let mut out = Vec::new();
    for (index, row) in rows.into_iter().take(100).enumerate() {
        let efforts: Vec<String> = row
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .map(|levels| {
                levels
                    .iter()
                    .filter_map(|level| match level {
                        Value::String(s) => Some(s.as_str()),
                        Value::Object(obj) => obj.get("effort").and_then(Value::as_str),
                        _ => None,
                    })
                    .filter(|s| valid_effort(s))
                    .take(10)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let default_effort = row
            .get("default_reasoning_level")
            .and_then(Value::as_str)
            .filter(|s| efforts.iter().any(|e| e == s))
            .map(str::to_owned);
        let id = string_field(row.get("slug"), 120)?;
        let name = string_field(row.get("display_name"), 160)?;
        out.push(CodexModel {
            id,
            name,
            is_default: index == 0,
            efforts: (!efforts.is_empty()).then_some(efforts),
            default_effort,
        });
    }
    Ok(out)
}

pub async fn verify_codex_sandbox(
    config: &CodexConfig,
    workspace: &Path,
    probe: &Path,
    cancel: Option<CancelToken>,
) -> Result<()> {
    let mut args = vec![
        "sandbox".into(),
        "-P".into(),
        "dar".into(),
        "-C".into(),
        workspace.to_string_lossy().into_owned(),
    ];
    args.extend(options(config, workspace));
    args.extend([
        "--".into(),
        "/bin/sh".into(),
        "-c".into(),
        r#"test -d "$1" && test ! -r "$2" && test ! -w "$2""#.into(),
        "dar-sandbox-check".into(),
        workspace.to_string_lossy().into_owned(),
        probe.to_string_lossy().into_owned(),
    ]);
    let result = command(config, workspace, &args, cancel, Duration::from_secs(15)).await?;
    if result.code != Some(0) {
        static RE_NETLINK: OnceLock<Regex> = OnceLock::new();
        static RE_PROC: OnceLock<Regex> = OnceLock::new();
        static RE_METADATA: OnceLock<Regex> = OnceLock::new();

        let re_netlink = RE_NETLINK.get_or_init(|| {
            Regex::new(r"(?i)NETLINK_ROUTE socket|loopback:.*Address family not supported")
                .expect("regex")
        });
        let re_proc = RE_PROC.get_or_init(|| {
            Regex::new(
                r"(?i)bwrap: Can't mount proc on /proc: (?:Operation not permitted|Permission denied)",
            )
            .expect("regex")
        });
        let re_metadata = RE_METADATA.get_or_init(|| {
            Regex::new(r"(?i)bwrap: (?:Can't create file at [^\n]*/\.codex: Is a directory|Destination is not a file [^\n]*/\.codex(?:\r?\n|$))").expect("regex")
        });

        if re_netlink.is_match(&result.stderr) {
            return Err(AppError::with(
                "sandboxAddressFamilyBlocked",
                503,
                "The Dar service blocks the socket family needed by Codex workspace isolation. On NixOS, update the Dar module; Agent needs AF_NETLINK in RestrictAddressFamilies. No model run was started.",
            ));
        }
        if re_proc.is_match(&result.stderr) {
            return Err(AppError::with(
                "sandboxProcMountBlocked",
                503,
                "The service's protected /proc mounts prevent Codex from creating its sandbox. Update the Dar NixOS module; Agent requires ProtectKernelTunables=false while retaining ProtectSystem=strict and Codex's /proc and /sys restrictions. No model run was started.",
            ));
        }
        if re_metadata.is_match(&result.stderr) {
            return Err(AppError::with(
                "sandboxMetadataRootMissing",
                503,
                "This cached workspace is missing a sandbox metadata directory. Update Dar to repair it automatically, or start a new thread. No model run was started.",
            ));
        }
        return Err(AppError::with(
            "sandboxUnavailable",
            503,
            "Codex could not establish workspace isolation. No model run was started.",
        ));
    }
    Ok(())
}
