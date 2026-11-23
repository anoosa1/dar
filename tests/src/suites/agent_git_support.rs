//! Helpers and fixtures shared by the agent-git suites (ported from `test/shared/`, `test/support/` and
//! `test/fixtures/`). Only that area's owner edits this file.

#![allow(dead_code)]

use base64::Engine as _;
use dar_acceptance::prelude::*;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};

// =================================================================================================
// Shared helpers for Agent, Git & History suites
// =================================================================================================

/// Percent-encodes a single URI component (`encodeURIComponent`).
pub fn encode_uri_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')');
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Creates a user account via the admin session.
pub async fn create_user(admin: &Session, username: &str, display_name: &str, password: &str) -> Result<()> {
    admin
        .post("/api/v1/auth/users", &json!({ "username": username, "displayName": display_name, "password": password }))
        .await?
        .expect(201)?;
    Ok(())
}

/// Creates an app password for Git over HTTPS and returns the secret.
pub async fn create_app_password(session: &Session, label: &str, scope: &str) -> Result<String> {
    let reply = session.post("/api/v1/auth/app-passwords", &json!({ "label": label, "scope": scope })).await?.expect(201)?;
    reply.json()?["secret"].as_str().map(str::to_owned).context("app password secret")
}

/// Result of running a `git` subprocess.
pub struct GitOutput {
    pub out: String,
    pub err: String,
}

/// Runs `git` with isolated config and Basic auth header.
pub fn run_git(
    cwd: &Path,
    home: &Path,
    author_name: &str,
    author_email: &str,
    user: &str,
    secret: &str,
    args: &[&str],
    expect_ok: bool,
    stdin_data: Option<&str>,
) -> Result<GitOutput> {
    let basic = dar_acceptance::base64(format!("{user}:{secret}").as_bytes());
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", author_name)
        .env("GIT_AUTHOR_EMAIL", author_email)
        .env("GIT_COMMITTER_NAME", author_name)
        .env("GIT_COMMITTER_EMAIL", author_email)
        .arg("-c")
        .arg(format!("http.extraHeader=Authorization: Basic {basic}"))
        .arg("-c")
        .arg("init.defaultBranch=main")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = if let Some(input) = stdin_data {
        let mut child = cmd.stdin(Stdio::piped()).spawn().context("spawning git")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input.as_bytes())?;
        }
        child.wait_with_output().context("waiting for git")?
    } else {
        cmd.stdin(Stdio::null()).output().context("running git")?
    };
    let out = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let err = String::from_utf8_lossy(&output.stderr).into_owned();
    if expect_ok {
        ensure!(output.status.success(), "git {} failed:\nstdout: {out}\nstderr: {err}", args.join(" "));
    } else {
        ensure!(!output.status.success(), "git {} should have failed:\nstdout: {out}\nstderr: {err}", args.join(" "));
    }
    Ok(GitOutput { out, err })
}

/// Runs `git` with no auth header (e.g., cloning a local bundle).
pub fn run_git_local(cwd: &Path, home: &Path, args: &[&str]) -> Result<GitOutput> {
    let output = Command::new("git")
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("running git")?;
    let out = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let err = String::from_utf8_lossy(&output.stderr).into_owned();
    ensure!(output.status.success(), "git {} failed:\n{err}", args.join(" "));
    Ok(GitOutput { out, err })
}

/// Extracts a query parameter from a URL string.
pub fn url_param(url_str: &str, key: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str).ok()?;
    parsed.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
}

/// Signs in to a page at `url` (may be a deep link like `/git?repo=...`).
pub async fn sign_in_at(page: &Page, url: &str, username: &str, password: &str) -> Result<()> {
    page.goto(url).await?;
    page.get_by_label("Username").fill(username).await?;
    page.get_by_label("Password").fill(password).await?;
    page.get_by_role("button", "Sign in").click().await?;
    Ok(())
}

// =================================================================================================
// Codex CLI fixture (`test/fixtures/codex-fixture.mjs`)
// =================================================================================================

fn send_line(writer: &Mutex<std::io::Stdout>, value: &Value) {
    let mut out = writer.lock().unwrap();
    let _ = writeln!(out, "{}", value);
    let _ = out.flush();
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

fn load_sessions(path: &Path) -> HashMap<String, Vec<String>> {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_sessions(path: &Path, sessions: &HashMap<String, Vec<String>>) -> std::io::Result<()> {
    std::fs::write(path, serde_json::to_string(sessions).unwrap())
}

/// The Codex CLI stand-in (`test/fixtures/codex-fixture.mjs`).
pub fn codex_fixture(args: &[String]) -> ExitCode {
    assert!(std::env::var_os("OPENAI_API_KEY").is_none(), "OPENAI_API_KEY must be unset");
    assert!(std::env::var_os("CODEX_API_KEY").is_none(), "CODEX_API_KEY must be unset");
    assert!(std::env::var_os("DAR_DATABASE_TOKEN_FILE").is_none(), "DAR_DATABASE_TOKEN_FILE must be unset");

    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let command = args.first().map(String::as_str).unwrap_or("");
    let subcommand = args.get(1).map(String::as_str).unwrap_or("");

    match (command, subcommand) {
        ("login", "status") => {
            let home = std::env::var("CODEX_HOME").expect("CODEX_HOME");
            let logged_out = Path::new(&home).join("logged-out").exists();
            if logged_out {
                eprintln!("Not logged in");
                ExitCode::from(1)
            } else {
                eprintln!("Logged in using ChatGPT");
                ExitCode::SUCCESS
            }
        }
        ("debug", "models") => {
            send_line(
                &stdout,
                &json!({
                    "models": [{
                        "slug": "fixture-model",
                        "display_name": "Fixture model",
                        "visibility": "list",
                        "priority": 1,
                        "default_reasoning_level": "medium",
                        "supported_reasoning_levels": [{"effort": "low"}, {"effort": "medium"}, {"effort": "high"}]
                    }]
                }),
            );
            ExitCode::SUCCESS
        }
        ("sandbox", _) => {
            assert!(args.iter().any(|a| a == "-P") && args.iter().any(|a| a == "dar"));
            ExitCode::SUCCESS
        }
        ("app-server", _) => {
            assert!(args.iter().any(|a| a == r#"approval_policy="never""#));
            assert!(args.iter().any(|a| a == r#"default_permissions="dar""#));
            assert!(args.iter().any(|a| a == "notify=[]"));
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
            rt.block_on(codex_app_server(stdout))
        }
        ("exec", _) => codex_exec(args, &stdout),
        _ => {
            eprintln!("Unexpected Codex CLI invocation: {command}");
            ExitCode::FAILURE
        }
    }
}

struct ShellState {
    cwd: String,
    exec_id: Value,
    line: String,
}

struct CodexState {
    sessions_file: PathBuf,
    sessions: HashMap<String, Vec<String>>,
    next_id: u64,
    turn: u64,
    cwd: PathBuf,
    current_turn: Option<String>,
    interrupted: Option<oneshot::Sender<()>>,
    steered: Option<oneshot::Sender<String>>,
    replies: HashMap<String, oneshot::Sender<Value>>,
    shells: HashMap<String, ShellState>,
}

async fn codex_app_server(stdout: Arc<Mutex<std::io::Stdout>>) -> ExitCode {
    let home = std::env::var("CODEX_HOME").expect("CODEX_HOME");
    let sessions_file = Path::new(&home).join("fixture-sessions.json");
    let sessions = load_sessions(&sessions_file);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let state = Arc::new(Mutex::new(CodexState {
        sessions_file,
        sessions,
        next_id: 1,
        turn: 0,
        cwd,
        current_turn: None,
        interrupted: None,
        steered: None,
        replies: HashMap::new(),
        shells: HashMap::new(),
    }));

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line).expect("valid JSON-RPC line");
        let id = value.get("id").cloned();
        let method = value.get("method").and_then(Value::as_str).map(str::to_owned);
        let params = value.get("params").cloned().unwrap_or(Value::Null);

        let Some(method) = method else {
            if let Some(id_str) = id.as_ref().and_then(Value::as_str) {
                let tx = state.lock().unwrap().replies.remove(id_str);
                if let Some(tx) = tx {
                    let _ = tx.send(value.get("result").cloned().unwrap_or(Value::Null));
                }
            }
            continue;
        };

        let id = id.unwrap_or(Value::Null);
        match method.as_str() {
            "initialize" => {
                assert_eq!(params["capabilities"]["experimentalApi"], json!(true));
                send_line(&stdout, &json!({ "id": id, "result": { "userAgent": "fixture" } }));
            }
            "initialized" => {}
            "thread/start" => {
                assert_eq!(params["approvalPolicy"], json!("never"));
                assert_eq!(params["model"], json!("fixture-model"));
                let thread_id = {
                    let mut st = state.lock().unwrap();
                    if let Some(c) = params["cwd"].as_str() {
                        st.cwd = PathBuf::from(c);
                    }
                    let tid = format!("fixture-thread-{}", st.sessions.len());
                    st.sessions.insert(tid.clone(), Vec::new());
                    save_sessions(&st.sessions_file, &st.sessions).expect("save sessions");
                    tid
                };
                send_line(&stdout, &json!({ "id": id, "result": { "thread": { "id": thread_id } } }));
            }
            "thread/resume" => {
                let tid = params["threadId"].as_str().unwrap_or_default().to_owned();
                let found = {
                    let mut st = state.lock().unwrap();
                    if st.sessions.contains_key(&tid) {
                        if let Some(c) = params["cwd"].as_str() {
                            st.cwd = PathBuf::from(c);
                        }
                        true
                    } else {
                        false
                    }
                };
                if !found {
                    send_line(&stdout, &json!({ "id": id, "error": { "code": -32600, "message": "no such thread" } }));
                } else {
                    send_line(&stdout, &json!({ "id": id, "result": { "thread": { "id": tid } } }));
                }
            }
            "turn/start" => {
                assert_eq!(params["approvalPolicy"], json!("never"));
                if let Some(effort) = params["effort"].as_str() {
                    assert!(matches!(effort, "low" | "medium" | "high"), "invalid effort: {effort}");
                }
                let turn_id = {
                    let mut st = state.lock().unwrap();
                    st.turn += 1;
                    let tid = format!("turn-{}", st.turn);
                    st.current_turn = Some(tid.clone());
                    tid
                };
                send_line(&stdout, &json!({ "id": id, "result": { "turn": { "id": turn_id, "status": "inProgress", "items": [] } } }));
                let st_clone = state.clone();
                let out_clone = stdout.clone();
                tokio::spawn(async move {
                    if let Err(err) = codex_scenario(st_clone, out_clone, params, turn_id).await {
                        eprintln!("{err}");
                        std::process::exit(3);
                    }
                });
            }
            "turn/steer" => {
                let (current, tx) = {
                    let mut st = state.lock().unwrap();
                    assert_eq!(params["expectedTurnId"].as_str(), st.current_turn.as_deref());
                    (st.current_turn.clone().unwrap_or_default(), st.steered.take())
                };
                send_line(&stdout, &json!({ "id": id, "result": { "turnId": current } }));
                let text = params["input"]
                    .as_array()
                    .and_then(|arr| arr.iter().find(|item| item["type"] == "text"))
                    .and_then(|item| item["text"].as_str())
                    .unwrap_or("")
                    .to_owned();
                if let Some(tx) = tx {
                    let _ = tx.send(text);
                }
            }
            "turn/interrupt" => {
                let tx = state.lock().unwrap().interrupted.take();
                send_line(&stdout, &json!({ "id": id, "result": {} }));
                if let Some(tx) = tx {
                    let _ = tx.send(());
                }
            }
            "review/start" => {
                assert_eq!(params["target"], json!({ "type": "uncommittedChanges" }));
                let thread_id = params["threadId"].as_str().unwrap_or_default().to_owned();
                let turn_id = {
                    let mut st = state.lock().unwrap();
                    st.turn += 1;
                    let tid = format!("review-{}", st.turn);
                    st.current_turn = Some(tid.clone());
                    tid
                };
                send_line(&stdout, &json!({ "id": id, "result": { "turn": { "id": turn_id, "status": "inProgress", "items": [] }, "reviewThreadId": thread_id } }));
                send_line(&stdout, &json!({ "method": "turn/started", "params": { "threadId": thread_id, "turn": { "id": turn_id } } }));
                send_line(&stdout, &json!({ "method": "item/completed", "params": { "threadId": thread_id, "turnId": turn_id, "item": { "type": "agentMessage", "id": "review-1", "text": "Review: greeting.txt changed; no issues found." } } }));
                send_line(&stdout, &json!({ "method": "turn/completed", "params": { "threadId": thread_id, "turn": { "id": turn_id, "status": "completed", "items": [] } } }));
            }
            "thread/compact/start" => {
                let thread_id = params["threadId"].as_str().unwrap_or_default().to_owned();
                send_line(&stdout, &json!({ "id": id, "result": {} }));
                {
                    let mut st = state.lock().unwrap();
                    if let Some(entry) = st.sessions.get_mut(&thread_id) {
                        entry.push("[compacted]".into());
                    }
                    save_sessions(&st.sessions_file, &st.sessions).expect("save sessions");
                }
                send_line(&stdout, &json!({ "method": "thread/compacted", "params": { "threadId": thread_id, "turnId": "compact" } }));
            }
            "account/rateLimits/read" => {
                send_line(
                    &stdout,
                    &json!({
                        "id": id,
                        "result": {
                            "ordinaryUsageAllowed": true,
                            "rateLimits": {
                                "limitId": "codex",
                                "limitName": null,
                                "primary": { "usedPercent": 42, "windowDurationMins": 300, "resetsAt": 1900000000 },
                                "secondary": { "usedPercent": 7, "windowDurationMins": 10080, "resetsAt": 1900500000 },
                                "planType": "pro"
                            },
                            "rateLimitsByLimitId": null
                        }
                    }),
                );
            }
            "command/exec" => {
                assert_eq!(params["tty"], json!(true));
                assert!(params.get("sandboxPolicy").is_none() || params["sandboxPolicy"].is_null());
                let process_id = params["processId"].as_str().unwrap_or_default().to_owned();
                let shell_cwd = params["cwd"].as_str().unwrap_or_default().to_owned();
                state.lock().unwrap().shells.insert(
                    process_id.clone(),
                    ShellState { cwd: shell_cwd, exec_id: id, line: String::new() },
                );
                let delta = base64::engine::general_purpose::STANDARD.encode("$ ");
                send_line(&stdout, &json!({ "method": "command/exec/outputDelta", "params": { "processId": process_id, "stream": "stdout", "deltaBase64": delta, "capReached": false } }));
            }
            "command/exec/write" => {
                let process_id = params["processId"].as_str().unwrap_or_default().to_owned();
                let raw_b64 = params["deltaBase64"].as_str().unwrap_or("");
                let bytes = base64::engine::general_purpose::STANDARD.decode(raw_b64).unwrap_or_default();
                let data = String::from_utf8_lossy(&bytes).into_owned();
                let mut outputs: Vec<String> = Vec::new();
                let mut exit_id: Option<Value> = None;
                {
                    let mut st = state.lock().unwrap();
                    if let Some(shell) = st.shells.get_mut(&process_id) {
                        for ch in data.chars() {
                            if ch == '\r' {
                                outputs.push("\r\n".into());
                                let command = shell.line.trim().to_owned();
                                shell.line.clear();
                                if command == "exit" {
                                    exit_id = Some(shell.exec_id.clone());
                                    break;
                                }
                                if command == "pwd" {
                                    outputs.push(format!("{}\r\n", shell.cwd));
                                } else if let Some(rest) = command.strip_prefix("echo ") {
                                    outputs.push(format!("{rest}\r\n"));
                                } else if !command.is_empty() {
                                    outputs.push(format!("sh: {command}: not found\r\n"));
                                }
                                outputs.push("$ ".into());
                            } else if ch == '\x7f' {
                                if !shell.line.is_empty() {
                                    shell.line.pop();
                                    outputs.push("\x08 \x08".into());
                                }
                            } else {
                                shell.line.push(ch);
                                outputs.push(ch.to_string());
                            }
                        }
                    }
                    if exit_id.is_some() {
                        st.shells.remove(&process_id);
                    }
                }
                for text in outputs {
                    let delta = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
                    send_line(&stdout, &json!({ "method": "command/exec/outputDelta", "params": { "processId": process_id, "stream": "stdout", "deltaBase64": delta, "capReached": false } }));
                }
                if let Some(exec_id) = exit_id {
                    send_line(&stdout, &json!({ "id": exec_id, "result": { "exitCode": 0, "stdout": "", "stderr": "" } }));
                }
                send_line(&stdout, &json!({ "id": id, "result": {} }));
            }
            "command/exec/resize" => {
                let rows = params["size"]["rows"].as_u64().unwrap_or(0);
                let cols = params["size"]["cols"].as_u64().unwrap_or(0);
                assert!(rows > 0 && cols > 0);
                send_line(&stdout, &json!({ "id": id, "result": {} }));
            }
            other => {
                send_line(&stdout, &json!({ "id": id, "error": { "code": -32601, "message": format!("unsupported {other}") } }));
            }
        }
    }
    ExitCode::SUCCESS
}

async fn codex_scenario(state: Arc<Mutex<CodexState>>, stdout: Arc<Mutex<std::io::Stdout>>, params: Value, turn_id: String) -> Result<()> {
    let thread_id = params["threadId"].as_str().unwrap_or_default().to_owned();
    let input = params["input"].as_array().cloned().unwrap_or_default();
    let text = input
        .iter()
        .find(|item| item["type"] == "text")
        .and_then(|item| item["text"].as_str())
        .unwrap_or("")
        .to_owned();
    let images: Vec<Value> = input.into_iter().filter(|item| item["type"] == "localImage").collect();

    let (turn_count, cwd) = {
        let mut st = state.lock().unwrap();
        let entry = st.sessions.entry(thread_id.clone()).or_default();
        entry.push(text.clone());
        let len = entry.len();
        save_sessions(&st.sessions_file, &st.sessions)?;
        (len, st.cwd.clone())
    };

    let notify = |method: &str, p: Value| {
        send_line(&stdout, &json!({ "method": method, "params": p }));
    };
    let complete = |status: &str, error: Option<&str>| {
        let mut turn = json!({ "id": turn_id, "status": status, "items": [] });
        if let Some(msg) = error {
            turn["error"] = json!({ "message": msg });
        }
        send_line(&stdout, &json!({ "method": "turn/completed", "params": { "threadId": thread_id, "turn": turn } }));
    };
    let message = |id: &str, msg: &str| {
        send_line(&stdout, &json!({ "method": "item/completed", "params": { "threadId": thread_id, "turnId": turn_id, "item": { "type": "agentMessage", "id": id, "text": msg } } }));
    };

    if text.contains("WAIT_FOR_CANCEL") {
        let (tx, rx) = oneshot::channel();
        state.lock().unwrap().interrupted = Some(tx);
        notify("item/started", json!({ "threadId": thread_id, "turnId": turn_id, "item": { "type": "commandExecution", "id": "waiting-command", "command": "fixture waiting for cancellation", "status": "inProgress" } }));
        let _ = rx.await;
        complete("interrupted", None);
        return Ok(());
    }
    if text.contains("FAIL_CLI") {
        complete("failed", Some("Fixture failure"));
        return Ok(());
    }
    if text.contains("EXIT_WITHOUT_TURN") {
        message("reply-1", "Unconfirmed response");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        std::process::exit(0);
    }
    if text.contains("ASK_QUESTION") {
        let rx = {
            let mut st = state.lock().unwrap();
            let req_id = format!("server-{}", st.next_id);
            st.next_id += 1;
            let (tx, rx) = oneshot::channel();
            st.replies.insert(req_id.clone(), tx);
            send_line(
                &stdout,
                &json!({
                    "id": req_id,
                    "method": "item/tool/requestUserInput",
                    "params": {
                        "threadId": thread_id,
                        "turnId": turn_id,
                        "itemId": "question-1",
                        "isBlocking": true,
                        "autoResolutionMs": null,
                        "questions": [{
                            "id": "color",
                            "header": "Color",
                            "question": "Which color should the greeting use?",
                            "isOther": true,
                            "isSecret": false,
                            "options": [
                                { "label": "Blue", "description": "Calm" },
                                { "label": "Red", "description": "Bold" }
                            ]
                        }]
                    }
                }),
            );
            rx
        };
        let answer = rx.await.map_err(|_| Error::new("question reply dropped"))?;
        let joined: String = answer["answers"]["color"]["answers"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        message("reply-1", &format!("You chose {joined}."));
        complete("completed", None);
        return Ok(());
    }
    if text.contains("REQUEST_APPROVAL") {
        let rx = {
            let mut st = state.lock().unwrap();
            let req_id = format!("server-{}", st.next_id);
            st.next_id += 1;
            let (tx, rx) = oneshot::channel();
            st.replies.insert(req_id.clone(), tx);
            send_line(
                &stdout,
                &json!({
                    "id": req_id,
                    "method": "item/commandExecution/requestApproval",
                    "params": {
                        "threadId": thread_id,
                        "turnId": turn_id,
                        "itemId": "approval-1",
                        "kind": "command",
                        "startedAtMs": now_ms(),
                        "environmentId": null,
                        "command": "curl https://example.test",
                        "reason": "Needs network access"
                    }
                }),
            );
            rx
        };
        let decision = rx.await.map_err(|_| Error::new("approval reply dropped"))?;
        assert_eq!(decision["decision"], json!("decline"));
        message("reply-1", "Approval was declined.");
        complete("completed", None);
        return Ok(());
    }
    if text.contains("WAIT_FOR_STEER") {
        let (tx, rx) = oneshot::channel();
        state.lock().unwrap().steered = Some(tx);
        notify("item/started", json!({ "threadId": thread_id, "turnId": turn_id, "item": { "type": "commandExecution", "id": "steer-wait", "command": "fixture waiting for guidance", "status": "inProgress" } }));
        let guidance = rx.await.map_err(|_| Error::new("steer channel dropped"))?;
        notify("item/completed", json!({ "threadId": thread_id, "turnId": turn_id, "item": { "type": "commandExecution", "id": "steer-wait", "command": "fixture waiting for guidance", "status": "completed", "exitCode": 0, "aggregatedOutput": "" } }));
        message("reply-1", &format!("Steered: {guidance}"));
        complete("completed", None);
        return Ok(());
    }
    if text.contains("PLAN_ONLY") {
        assert_eq!(params["collaborationMode"]["mode"], json!("plan"));
        message("reply-1", "Planned without editing.");
        complete("completed", None);
        return Ok(());
    }
    if text.contains("DESCRIBE_IMAGE") {
        for image in &images {
            let path = image["path"].as_str().context("image path")?;
            let bytes = std::fs::read(path)?;
            assert_eq!(&bytes[1..4], b"PNG");
        }
        message("reply-1", &format!("Saw {} image{}.", images.len(), if images.len() == 1 { "" } else { "s" }));
        complete("completed", None);
        return Ok(());
    }
    if text.contains("RESUME_CHECK") {
        let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
        message("reply-1", &format!("Turns: {turn_count}. Input: {normalized}"));
        complete("completed", None);
        return Ok(());
    }

    let greeting_path = cwd.join("greeting.txt");
    let content = std::fs::read_to_string(&greeting_path)?;
    let next = if content.contains("Updated") { "Updated again by Codex fixture.\n" } else { "Updated by Codex fixture.\n" };
    std::fs::write(&greeting_path, next)?;
    notify("item/completed", json!({ "threadId": thread_id, "turnId": turn_id, "item": { "type": "reasoning", "id": "reasoning-1", "summary": ["Checking the greeting file before editing."], "content": [] } }));
    notify("turn/plan/updated", json!({ "threadId": thread_id, "turnId": turn_id, "explanation": null, "plan": [{ "step": "Read greeting", "status": "completed" }, { "step": "Update greeting", "status": "pending" }] }));
    notify("item/completed", json!({ "threadId": thread_id, "turnId": turn_id, "item": { "type": "fileChange", "id": "edit-1", "status": "completed", "changes": [{ "path": "greeting.txt", "kind": { "type": "update" }, "diff": "" }] } }));
    notify("item/started", json!({ "threadId": thread_id, "turnId": turn_id, "item": { "type": "commandExecution", "id": "command-1", "command": "fixture test command", "status": "inProgress" } }));
    notify("item/commandExecution/outputDelta", json!({ "threadId": thread_id, "turnId": turn_id, "itemId": "command-1", "delta": "fixture check " }));
    notify("item/completed", json!({ "threadId": thread_id, "turnId": turn_id, "item": { "type": "commandExecution", "id": "command-1", "command": "fixture test command", "status": "completed", "exitCode": 0, "aggregatedOutput": "fixture check passed\n" } }));
    notify("item/agentMessage/delta", json!({ "threadId": thread_id, "turnId": turn_id, "itemId": "reply-1", "delta": "The edit " }));
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    message("reply-1", "The edit is ready.");
    let usage = json!({ "totalTokens": 15, "inputTokens": 10, "cachedInputTokens": 4, "cacheWriteInputTokens": 0, "outputTokens": 5, "reasoningOutputTokens": 0 });
    notify("thread/tokenUsage/updated", json!({ "threadId": thread_id, "turnId": turn_id, "tokenUsage": { "total": usage, "last": usage, "modelContextWindow": 200000 } }));
    complete("completed", None);
    Ok(())
}

fn codex_exec(args: &[String], stdout: &Mutex<std::io::Stdout>) -> ExitCode {
    assert!(args.iter().any(|a| a == "--json"));
    assert!(args.iter().any(|a| a == "--ephemeral"));
    assert!(args.iter().any(|a| a == "--ignore-user-config"));
    assert!(args.iter().any(|a| a == "--ignore-rules"));
    assert_eq!(args.last().map(String::as_str), Some("-"));
    let m_idx = args.iter().position(|a| a == "-m").expect("-m flag");
    assert_eq!(args.get(m_idx + 1).map(String::as_str), Some("fixture-model"));
    assert!(args.iter().any(|a| a == r#"approval_policy="never""#));

    let mut prompt = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut prompt).expect("read stdin");
    send_line(stdout, &json!({ "type": "thread.started", "thread_id": "fixture-thread" }));
    send_line(stdout, &json!({ "type": "turn.started" }));

    if prompt.contains("WAIT_FOR_CANCEL") {
        send_line(stdout, &json!({ "type": "item.started", "item": { "id": "waiting-command", "type": "command_execution", "command": "fixture waiting for cancellation", "status": "in_progress" } }));
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    } else if prompt.contains("FAIL_CLI") {
        send_line(stdout, &json!({ "type": "turn.failed", "error": { "message": "Fixture failure" } }));
        return ExitCode::from(1);
    } else if prompt.contains("EXIT_WITHOUT_TURN") {
        send_line(stdout, &json!({ "type": "item.completed", "item": { "id": "reply-1", "type": "agent_message", "text": "Unconfirmed response" } }));
        return ExitCode::SUCCESS;
    }

    let c_idx = args.iter().position(|a| a == "-C").expect("-C flag");
    let cwd = Path::new(&args[c_idx + 1]);
    let content = std::fs::read_to_string(cwd.join("greeting.txt")).expect("read greeting.txt");
    let next = if content.contains("Updated") { "Updated again by Codex fixture.\n" } else { "Updated by Codex fixture.\n" };
    std::fs::write(cwd.join("greeting.txt"), next).expect("write greeting.txt");
    if let Some(effort) = args.iter().find(|v| v.starts_with("model_reasoning_effort=")) {
        assert!(
            matches!(effort.as_str(), r#"model_reasoning_effort="low""# | r#"model_reasoning_effort="medium""# | r#"model_reasoning_effort="high""#),
            "invalid effort {effort}"
        );
    }
    send_line(stdout, &json!({ "type": "item.completed", "item": { "id": "reasoning-1", "type": "reasoning", "text": "Checking the greeting file before editing." } }));
    send_line(stdout, &json!({ "type": "item.started", "item": { "id": "plan-1", "type": "todo_list", "items": [{ "text": "Read greeting", "completed": true }, { "text": "Update greeting", "completed": false }] } }));
    send_line(stdout, &json!({ "type": "item.completed", "item": { "id": "edit-1", "type": "file_change", "status": "completed", "changes": [{ "path": "greeting.txt", "kind": "update" }] } }));
    send_line(stdout, &json!({ "type": "item.started", "item": { "id": "command-1", "type": "command_execution", "command": "fixture test command", "status": "in_progress" } }));
    send_line(stdout, &json!({ "type": "item.updated", "item": { "id": "command-1", "type": "command_execution", "aggregated_output": "fixture check " } }));
    send_line(stdout, &json!({ "type": "item.completed", "item": { "id": "command-1", "type": "command_execution", "aggregated_output": "fixture check passed\n", "exit_code": 0, "status": "completed" } }));
    send_line(stdout, &json!({ "type": "item.updated", "item": { "id": "reply-1", "type": "agent_message", "text": "The edit " } }));
    std::thread::sleep(std::time::Duration::from_millis(800));
    send_line(stdout, &json!({ "type": "item.completed", "item": { "id": "reply-1", "type": "agent_message", "text": "The edit is ready." } }));
    send_line(stdout, &json!({ "type": "turn.completed", "usage": { "input_tokens": 10, "cached_input_tokens": 4, "output_tokens": 5 } }));
    ExitCode::SUCCESS
}

// =================================================================================================
// Claude Code CLI fixture (`test/fixtures/claude-fixture.mjs`)
// =================================================================================================

struct ClaudeState {
    sessions_file: PathBuf,
    sessions: HashMap<String, Vec<String>>,
    session_id: String,
    announced: bool,
    message_seq: u64,
    running: bool,
    interrupted: Option<oneshot::Sender<()>>,
    steered: Option<oneshot::Sender<String>>,
    replies: HashMap<String, oneshot::Sender<Value>>,
    tools: Vec<String>,
    model: Option<String>,
    permission_mode: Option<String>,
}

/// The Claude Code CLI stand-in (`test/fixtures/claude-fixture.mjs`).
pub fn claude_fixture(args: &[String]) -> ExitCode {
    let home = std::env::var("CLAUDE_CONFIG_DIR").expect("CLAUDE_CONFIG_DIR");
    assert!(!home.is_empty());
    assert!(std::env::var_os("ANTHROPIC_API_KEY").is_none(), "ANTHROPIC_API_KEY must be unset");
    assert!(std::env::var_os("DAR_DATABASE_TOKEN_FILE").is_none(), "DAR_DATABASE_TOKEN_FILE must be unset");
    assert_eq!(std::env::var("HOME").ok().as_deref(), Some(home.as_str()));

    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let option = |name: &str| -> Option<String> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
    };

    if args.first().map(String::as_str) == Some("auth") && args.get(1).map(String::as_str) == Some("status") {
        let logged_out = Path::new(&home).join("logged-out").exists();
        send_line(
            &stdout,
            &json!({
                "loggedIn": !logged_out,
                "authMethod": "claude.ai",
                "apiProvider": "firstParty",
                "email": "fixture@example.test",
                "subscriptionType": "max"
            }),
        );
        return if logged_out { ExitCode::from(1) } else { ExitCode::SUCCESS };
    }

    for flag in ["-p", "--restricted", "--strict-mcp-config", "--disable-slash-commands", "--verbose"] {
        assert!(args.iter().any(|a| a == flag), "missing flag {flag}");
    }
    assert_eq!(option("--input-format").as_deref(), Some("stream-json"));
    assert_eq!(option("--output-format").as_deref(), Some("stream-json"));
    assert_eq!(option("--permission-prompt-tool").as_deref(), Some("stdio"));

    let tools: Vec<String> = option("--tools")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let settings: Value = serde_json::from_str(&option("--settings").unwrap_or_else(|| "{}".into())).expect("valid settings JSON");
    for tool in &tools {
        assert!(
            ["Read", "Edit", "Write", "Glob", "Grep", "TodoWrite", "AskUserQuestion", "Bash"].contains(&tool.as_str()),
            "unexpected tool {tool}"
        );
    }
    if tools.iter().any(|t| t == "Bash") {
        assert_eq!(settings["sandbox"]["failIfUnavailable"], json!(true));
        assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], json!(false));
        assert_eq!(settings["sandbox"]["network"]["allowedDomains"], json!([]));
    }

    let sessions_file = Path::new(&home).join("fixture-sessions.json");
    let mut sessions = load_sessions(&sessions_file);
    let resume = option("--resume");
    if let Some(ref resume_id) = resume
        && !sessions.contains_key(resume_id)
    {
        eprintln!("No conversation found");
        return ExitCode::from(1);
    }
    let session_id = resume.unwrap_or_else(dar_acceptance::uuid);
    sessions.entry(session_id.clone()).or_default();

    let state = Arc::new(Mutex::new(ClaudeState {
        sessions_file,
        sessions,
        session_id,
        announced: false,
        message_seq: 0,
        running: false,
        interrupted: None,
        steered: None,
        replies: HashMap::new(),
        tools,
        model: option("--model"),
        permission_mode: option("--permission-mode"),
    }));

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
    rt.block_on(claude_loop(state, stdout))
}

async fn claude_loop(state: Arc<Mutex<ClaudeState>>, stdout: Arc<Mutex<std::io::Stdout>>) -> ExitCode {
    let (turn_tx, mut turn_rx) = mpsc::unbounded_channel::<Vec<Value>>();
    let worker_state = state.clone();
    let worker_stdout = stdout.clone();
    let worker = tokio::spawn(async move {
        while let Some(content) = turn_rx.recv().await {
            if let Err(err) = claude_turn(worker_state.clone(), worker_stdout.clone(), content).await {
                eprintln!("{err}");
                std::process::exit(3);
            }
            worker_state.lock().unwrap().running = false;
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line).expect("valid stream-json line");
        match value["type"].as_str().unwrap_or("") {
            "control_response" => {
                if let Some(req_id) = value["response"]["request_id"].as_str() {
                    let tx = state.lock().unwrap().replies.remove(req_id);
                    if let Some(tx) = tx {
                        let _ = tx.send(value["response"]["response"].clone());
                    }
                }
            }
            "control_request" => {
                let req_id = value["request_id"].as_str().unwrap_or_default();
                let subtype = value["request"]["subtype"].as_str().unwrap_or_default();
                let ok = |resp: Value| {
                    send_line(&stdout, &json!({ "type": "control_response", "response": { "subtype": "success", "request_id": req_id, "response": resp } }));
                };
                match subtype {
                    "initialize" => ok(json!({
                        "models": [
                            { "value": "default", "resolvedModel": "fixture-model", "displayName": "Default (recommended)" },
                            { "value": "fixture", "resolvedModel": "fixture-model", "displayName": "Fixture Claude", "supportedEffortLevels": ["low", "high"] },
                            { "value": "bad model;", "displayName": "Rejected" }
                        ],
                        "account": { "email": "fixture@example.test" }
                    })),
                    "get_usage" => ok(json!({
                        "subscription_type": "max",
                        "rate_limits_available": true,
                        "rate_limits": {
                            "limits": [
                                { "kind": "session", "group": "session", "percent": 42, "resets_at": "2030-03-17T17:46:40Z" },
                                { "kind": "weekly_all", "group": "weekly", "percent": 7, "resets_at": "2030-03-23T08:00:00Z" }
                            ]
                        }
                    })),
                    "interrupt" => {
                        ok(json!({}));
                        let tx = state.lock().unwrap().interrupted.take();
                        if let Some(tx) = tx {
                            let _ = tx.send(());
                        }
                    }
                    _ => {
                        send_line(&stdout, &json!({ "type": "control_response", "response": { "subtype": "error", "request_id": req_id, "error": "unsupported" } }));
                    }
                }
            }
            "user" => {
                let content = value["message"]["content"].as_array().cloned().unwrap_or_default();
                let steer_tx = {
                    let mut st = state.lock().unwrap();
                    if st.running && st.steered.is_some() {
                        st.steered.take()
                    } else {
                        st.running = true;
                        None
                    }
                };
                if let Some(tx) = steer_tx {
                    let text = content
                        .iter()
                        .find(|item| item["type"] == "text")
                        .and_then(|item| item["text"].as_str())
                        .unwrap_or("")
                        .to_owned();
                    let _ = tx.send(text);
                } else {
                    let _ = turn_tx.send(content);
                }
            }
            _ => {}
        }
    }
    drop(turn_tx);
    let _ = worker.await;
    ExitCode::SUCCESS
}

async fn claude_turn(state: Arc<Mutex<ClaudeState>>, stdout: Arc<Mutex<std::io::Stdout>>, content: Vec<Value>) -> Result<()> {
    let text = content
        .iter()
        .find(|item| item["type"] == "text")
        .and_then(|item| item["text"].as_str())
        .unwrap_or("")
        .to_owned();
    let images: Vec<Value> = content.into_iter().filter(|item| item["type"] == "image").collect();

    let (session_id, turn_count, should_announce, tools, model, permission_mode) = {
        let mut st = state.lock().unwrap();
        let sid = st.session_id.clone();
        let entry = st.sessions.entry(sid.clone()).or_default();
        entry.push(text.clone());
        let len = entry.len();
        save_sessions(&st.sessions_file, &st.sessions)?;
        let announce = !st.announced;
        st.announced = true;
        (sid, len, announce, st.tools.clone(), st.model.clone(), st.permission_mode.clone())
    };

    if should_announce {
        let cwd = std::env::current_dir().unwrap_or_default().to_string_lossy().into_owned();
        send_line(
            &stdout,
            &json!({
                "type": "system",
                "subtype": "init",
                "session_id": session_id,
                "cwd": cwd,
                "tools": tools,
                "model": model,
                "permissionMode": permission_mode
            }),
        );
    }

    let assistant = |blocks: Value| {
        let msg_id = {
            let mut st = state.lock().unwrap();
            st.message_seq += 1;
            format!("msg_{}", st.message_seq)
        };
        send_line(
            &stdout,
            &json!({
                "type": "assistant",
                "message": {
                    "id": msg_id,
                    "role": "assistant",
                    "content": blocks,
                    "usage": { "input_tokens": 3, "cache_read_input_tokens": 4, "cache_creation_input_tokens": 5, "output_tokens": 6 }
                },
                "parent_tool_use_id": null,
                "session_id": session_id
            }),
        );
    };

    let result_msg = |res_text: &str, error: Option<&str>| {
        send_line(
            &stdout,
            &json!({
                "type": "result",
                "subtype": if error.is_some() { "error_during_execution" } else { "success" },
                "is_error": error.is_some(),
                "result": error.unwrap_or(res_text),
                "session_id": session_id,
                "usage": { "input_tokens": 10, "cache_read_input_tokens": 4, "cache_creation_input_tokens": 1, "output_tokens": 5 },
                "modelUsage": { "fixture-model": { "contextWindow": 200000 } }
            }),
        );
    };

    let reply = |reply_text: &str| {
        let next_seq = state.lock().unwrap().message_seq + 1;
        let prefix: String = reply_text.chars().take(4).collect();
        send_line(&stdout, &json!({ "type": "stream_event", "event": { "type": "message_start", "message": { "id": format!("msg_{next_seq}") } }, "parent_tool_use_id": null, "session_id": session_id }));
        send_line(&stdout, &json!({ "type": "stream_event", "event": { "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }, "parent_tool_use_id": null, "session_id": session_id }));
        send_line(&stdout, &json!({ "type": "stream_event", "event": { "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": prefix } }, "parent_tool_use_id": null, "session_id": session_id }));
        assistant(json!([{ "type": "text", "text": reply_text }]));
    };

    let tool_result = |id: &str, content_str: &str, is_error: bool| {
        send_line(
            &stdout,
            &json!({
                "type": "user",
                "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": id, "content": content_str, "is_error": is_error }] },
                "parent_tool_use_id": null,
                "session_id": session_id
            }),
        );
    };

    let control_request = |req: Value| {
        let id = dar_acceptance::uuid();
        let (tx, rx) = oneshot::channel();
        state.lock().unwrap().replies.insert(id.clone(), tx);
        send_line(&stdout, &json!({ "type": "control_request", "request_id": id, "request": req }));
        rx
    };

    if text.contains("WAIT_FOR_CANCEL") {
        let (tx, rx) = oneshot::channel();
        state.lock().unwrap().interrupted = Some(tx);
        assistant(json!([{ "type": "tool_use", "id": "toolu_wait", "name": "Grep", "input": { "pattern": "fixture waiting for cancellation" } }]));
        let _ = rx.await;
        result_msg("", Some("Interrupted"));
        return Ok(());
    }
    if text.contains("FAIL_CLI") {
        result_msg("", Some("Fixture failure"));
        return Ok(());
    }
    if text.contains("EXIT_WITHOUT_RESULT") {
        reply("Unconfirmed response");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        std::process::exit(0);
    }
    if text.contains("ASK_QUESTION") {
        let input = json!({
            "questions": [{
                "question": "Which color should the greeting use?",
                "header": "Color",
                "multiSelect": false,
                "options": [
                    { "label": "Blue", "description": "Calm" },
                    { "label": "Red", "description": "Bold" }
                ]
            }]
        });
        assistant(json!([{ "type": "tool_use", "id": "toolu_ask", "name": "AskUserQuestion", "input": input }]));
        let decision = control_request(json!({ "subtype": "can_use_tool", "tool_name": "AskUserQuestion", "input": input, "tool_use_id": "toolu_ask" }))
            .await
            .map_err(|_| Error::new("ask question reply dropped"))?;
        assert_eq!(decision["behavior"], json!("allow"));
        tool_result("toolu_ask", "answered", false);
        let chosen = decision["updatedInput"]["answers"]["Which color should the greeting use?"].as_str().unwrap_or_default();
        reply(&format!("You chose {chosen}."));
        result_msg("done", None);
        return Ok(());
    }
    if text.contains("REQUEST_NETWORK") {
        let decision = control_request(json!({
            "subtype": "can_use_tool",
            "tool_name": "SandboxNetworkAccess",
            "input": { "host": "example.test" },
            "tool_use_id": dar_acceptance::uuid()
        }))
        .await
        .map_err(|_| Error::new("network reply dropped"))?;
        assert_eq!(decision["behavior"], json!("deny"));
        reply("Network was declined.");
        result_msg("done", None);
        return Ok(());
    }
    if text.contains("WAIT_FOR_STEER") {
        let (tx, rx) = oneshot::channel();
        state.lock().unwrap().steered = Some(tx);
        assistant(json!([{ "type": "tool_use", "id": "toolu_steer", "name": "Grep", "input": { "pattern": "fixture waiting for guidance" } }]));
        let guidance = rx.await.map_err(|_| Error::new("steer reply dropped"))?;
        tool_result("toolu_steer", "found", false);
        reply(&format!("Steered: {guidance}"));
        result_msg("done", None);
        return Ok(());
    }
    if text.contains("PLAN_ONLY") {
        assert_eq!(permission_mode.as_deref(), Some("plan"));
        let decision = control_request(json!({
            "subtype": "can_use_tool",
            "tool_name": "ExitPlanMode",
            "input": { "plan": "1. Read greeting\n2. Update greeting" },
            "tool_use_id": "toolu_plan"
        }))
        .await
        .map_err(|_| Error::new("plan reply dropped"))?;
        assert_eq!(decision["behavior"], json!("deny"));
        reply("Planned without editing.");
        result_msg("done", None);
        return Ok(());
    }
    if text.contains("DESCRIBE_IMAGE") {
        for image in &images {
            let b64 = image["source"]["data"].as_str().context("image data")?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| Error::new(e.to_string()))?;
            assert_eq!(&bytes[1..4], b"PNG");
        }
        reply(&format!("Saw {} image{}.", images.len(), if images.len() == 1 { "" } else { "s" }));
        result_msg("done", None);
        return Ok(());
    }
    if text.contains("Review the uncommitted changes") {
        assert_eq!(permission_mode.as_deref(), Some("plan"));
        assert!(text.contains("+Updated by Claude fixture."));
        reply("Review: greeting.txt changed; no issues found.");
        result_msg("done", None);
        return Ok(());
    }
    if text.contains("RESUME_CHECK") {
        let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
        reply(&format!("Turns: {turn_count}. Input: {normalized}"));
        result_msg("done", None);
        return Ok(());
    }

    assert_eq!(permission_mode.as_deref(), Some("acceptEdits"));
    let file = std::env::current_dir()?.join("greeting.txt");
    let before = std::fs::read_to_string(&file)?;
    assistant(json!([{ "type": "thinking", "thinking": "Checking the greeting file before editing." }]));
    assistant(json!([{ "type": "tool_use", "id": "toolu_todo", "name": "TodoWrite", "input": { "todos": [{ "content": "Read greeting", "status": "completed" }, { "content": "Update greeting", "status": "pending" }] } }]));
    assistant(json!([{ "type": "tool_use", "id": "toolu_edit", "name": "Write", "input": { "file_path": file.to_string_lossy(), "content": "Updated by Claude fixture.\n" } }]));
    let next = if before.contains("Updated") { "Updated again by Claude fixture.\n" } else { "Updated by Claude fixture.\n" };
    std::fs::write(&file, next)?;
    tool_result("toolu_edit", "File written", false);
    assistant(json!([{ "type": "tool_use", "id": "toolu_read", "name": "Read", "input": { "file_path": "/etc/hostname" } }]));
    tool_result("toolu_read", "/etc/hostname is outside the working directory.", true);
    reply("The edit is ready.");
    result_msg("The edit is ready.", None);
    Ok(())
}
