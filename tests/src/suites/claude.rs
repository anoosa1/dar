//! Port of `test/claude.integration.test.ts`.

use dar_acceptance::instance::fixture_command;
use dar_acceptance::prelude::*;
use dar_acceptance::session::{Reply, Upload, client, upload_with};
use dar_acceptance::stack::stack;
use dar_bindings::create_folder_reducer::create_folder;
use std::time::{Duration, Instant};

pub const SUITE: Suite = Suite {
    name: "claude",
    cases: &[case!(claude_cli_runs)],
};

async fn agent_req(session: &Session, route: &str, value: Option<&Value>, if_match: Option<u64>) -> Result<Reply> {
    let method = if value.is_some() { "POST" } else { "GET" };
    let mut req = session.request(method, &format!("/api/v1/agent{route}"));
    if let Some(rev) = if_match {
        req = req.header("if-match", format!("\"{rev}\""));
    }
    if let Some(body) = value {
        req = req.json(body);
    }
    Reply::from(req.send().await?).await
}

async fn call_agent(session: &Session, route: &str, value: Option<&Value>, if_match: Option<u64>) -> Result<Value> {
    let reply = agent_req(session, route, value, if_match).await?;
    ensure!(
        (200..300).contains(&reply.status),
        "agent {route} failed: {} {}",
        reply.status,
        reply.text()
    );
    reply.json()
}

async fn wait_run<F>(session: &Session, id: &str, expected: &str, ready: F) -> Result<Value>
where
    F: Fn(&Value) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let result = call_agent(session, &format!("/runs/{id}"), None, None).await?;
        let status = result["status"].as_str().unwrap_or_default();
        if status == expected && ready(&result) {
            return Ok(result);
        }
        if matches!(status, "failed" | "interrupted" | "cancelled") && status != expected {
            bail!("run {id} ended unexpectedly as {status}: {result}");
        }
        if Instant::now() >= deadline {
            bail!("run {id} did not become {expected}: {result}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn thread_revision(session: &Session, thread_id: &str) -> Result<u64> {
    call_agent(session, &format!("/threads/{thread_id}/messages"), None, None).await?["thread"]["revision"]
        .as_u64()
        .context("thread revision")
}

async fn start_run(session: &Session, thread_id: &str, content: &str, extra: Value) -> Result<Value> {
    let rev = thread_revision(session, thread_id).await?;
    let mut body = json!({
        "id": uuid(),
        "messageId": uuid(),
        "model": "claude/fixture",
        "content": content
    });
    if let Some(map) = extra.as_object() {
        for (k, v) in map {
            body[k] = v.clone();
        }
    }
    call_agent(session, &format!("/threads/{thread_id}/runs"), Some(&body), Some(rev)).await
}

async fn claude_cli_runs(ctx: &Ctx) -> Result<()> {
    let root = ctx.dir().join("claude-acceptance");
    std::fs::create_dir_all(&root)?;
    let executable = fixture_command(&root, "claude", &[])?;
    let home = root.join("claude-home");
    std::fs::create_dir_all(&home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    }
    let claude_file = root.join("claude.json");
    let codex_file = root.join("codex.json");

    let runtime = Instance::isolated(
        &root,
        "dar-claude",
        InstanceConfig {
            codex_file: Some(codex_file),
            claude_file: Some(claude_file.clone()),
            ..InstanceConfig::default()
        },
        true,
    )
    .await?;

    let res: Result<()> = async {
        let base = runtime.base.clone();
        let bootstrap = dar_acceptance::read_to_string(&stack().bootstrap_token_file)?;
        let setup_reply = Reply::from(
            client()
                .post(format!("{base}/api/v1/auth/setup"))
                .header("origin", &base)
                .json(&json!({
                    "username": "admin",
                    "displayName": "Claude fixture",
                    "password": PASSWORD,
                    "bootstrapToken": bootstrap.trim()
                }))
                .send()
                .await?,
        )
        .await?
        .expect(200)?;
        let account_id = setup_reply.json()?["user"]["id"].as_str().context("account id")?.to_owned();

        let session = login(&base).await?;

        let before = call_agent(&session, "/provider", None, None).await?;
        ensure!(before["configured"] == json!(false));
        let before_providers: Vec<(&str, bool)> = before["providers"]
            .as_array()
            .context("providers")?
            .iter()
            .map(|p| (p["id"].as_str().unwrap_or_default(), p["configured"].as_bool().unwrap_or(false)))
            .collect();
        ensure!(before_providers == vec![("codex", false), ("claude", false)]);

        std::fs::write(
            &claude_file,
            json!({
                "executable": executable.to_string_lossy(),
                "home": home.to_string_lossy(),
                "userId": uuid(),
                "gitHosts": []
            })
            .to_string(),
        )?;
        let wrong_user = call_agent(&session, "/provider", None, None).await?;
        ensure!(
            wrong_user["providers"][1]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("different Dar account"),
            "unexpected wrong-user message: {wrong_user}"
        );

        std::fs::write(
            &claude_file,
            json!({
                "executable": executable.to_string_lossy(),
                "home": home.to_string_lossy(),
                "userId": account_id,
                "gitHosts": []
            })
            .to_string(),
        )?;
        let provider = call_agent(&session, "/provider", None, None).await?;
        ensure!(provider["connected"] == json!(true), "provider not connected: {provider}");
        ensure!(!provider.to_string().contains("fixture@example.test"));
        let models: Vec<(&str, &str, bool)> = provider["models"]
            .as_array()
            .context("models")?
            .iter()
            .map(|m| {
                (
                    m["id"].as_str().unwrap_or_default(),
                    m["provider"].as_str().unwrap_or_default(),
                    m["isDefault"].as_bool().unwrap_or(false),
                )
            })
            .collect();
        ensure!(models == vec![("claude/fixture", "claude", true)], "unexpected models: {models:?}");
        ensure!(provider["models"][0]["efforts"] == json!(["low", "high"]));
        ensure!(provider["providers"][1]["connected"] == json!(true));
        ensure!(
            provider["providers"][1]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("Claude Code login (max)")
        );

        let logged_out = home.join("logged-out");
        std::fs::write(&logged_out, "")?;
        ensure!(call_agent(&session, "/provider", None, None).await?["connected"] == json!(false));
        std::fs::remove_file(&logged_out)?;

        let folder_id = uuid();
        let conn = session.connect().await?;
        let folder_res = live::reducer(|done| conn.reducers.create_folder_then(folder_id.clone(), String::new(), "Source".into(), done)).await;
        conn.disconnect();
        folder_res?;

        let file = upload_with(
            &session,
            "greeting.txt",
            "Original source.\n",
            Upload { parent_id: &folder_id, ..Upload::default() },
        )
        .await?;
        let project_id = uuid();
        call_agent(
            &session,
            "/projects",
            Some(&json!({ "id": project_id, "name": "Files code", "sourceType": "files", "sourceRef": folder_id })),
            None,
        )
        .await?;

        let make_thread = |title: &str| {
            let session = session.clone();
            let project_id = project_id.clone();
            let title = title.to_owned();
            async move {
                let id = uuid();
                call_agent(&session, "/threads", Some(&json!({ "id": id, "projectId": project_id, "title": title })), None).await?;
                Ok::<String, Error>(id)
            }
        };

        // An edit run: typed steps, usage, context window and a reviewable Files change.
        let edit_thread = make_thread("Edit source").await?;
        let edit_run = start_run(&session, &edit_thread, "Update the greeting.", json!({ "effort": "high" })).await?;
        let done = wait_run(&session, edit_run["id"].as_str().context("edit run id")?, "completed", |_| true).await?;
        ensure!(done["document"]["output"] == "The edit is ready.");
        ensure!(done["document"]["effort"] == "high");
        ensure!(done["document"]["claudeSession"].as_str().is_some_and(|s| !s.is_empty()));
        let steps = done["document"]["steps"].as_array().context("steps")?;
        let step_summary: Vec<(&str, &str)> = steps
            .iter()
            .map(|s| (s["kind"].as_str().unwrap_or_default(), s["status"].as_str().unwrap_or_default()))
            .collect();
        ensure!(
            step_summary == vec![("reasoning", "completed"), ("plan", "running"), ("edit", "completed"), ("tool", "failed")],
            "unexpected steps: {step_summary:?}"
        );
        ensure!(steps[2]["label"] == "greeting.txt");
        ensure!(
            steps[1]["items"] == json!([{ "text": "Read greeting", "done": true }, { "text": "Update greeting", "done": false }])
        );
        ensure!(done["document"]["usage"] == json!({ "input": 15, "cached": 4, "output": 5 }));
        ensure!(done["document"]["contextWindow"] == 200000);
        ensure!(done["document"]["contextUsed"] == 18);
        let changes = done["document"]["changes"].as_array().context("changes")?;
        ensure!(changes.len() == 1);
        ensure!(changes[0]["fileId"] == file.file_id);
        ensure!(changes[0]["content"] == "Updated by Claude fixture.\n");
        let edit_messages = call_agent(&session, &format!("/threads/{edit_thread}/messages"), None, None).await?;
        let roles: Vec<&str> = edit_messages["items"]
            .as_array()
            .context("messages")?
            .iter()
            .filter_map(|m| m["role"].as_str())
            .collect();
        ensure!(roles == vec!["user", "assistant"]);

        let bad_effort = agent_req(
            &session,
            &format!("/threads/{edit_thread}/runs"),
            Some(&json!({ "id": uuid(), "messageId": uuid(), "model": "claude/fixture", "effort": "max", "content": "Effort check." })),
            Some(thread_revision(&session, &edit_thread).await?),
        )
        .await?;
        ensure!(bad_effort.status == 400);

        // Review is read-only and receives Dar's diff; compaction is left to Claude Code.
        let review_run = start_run(
            &session,
            &edit_thread,
            "Review the uncommitted changes in this working copy.",
            json!({ "mode": "review" }),
        )
        .await?;
        let review_done = wait_run(&session, review_run["id"].as_str().context("review id")?, "completed", |_| true).await?;
        ensure!(review_done["document"]["output"] == "Review: greeting.txt changed; no issues found.");

        let compact = agent_req(
            &session,
            &format!("/threads/{edit_thread}/runs"),
            Some(&json!({ "id": uuid(), "messageId": uuid(), "model": "claude/fixture", "mode": "compact", "content": "Compact." })),
            Some(thread_revision(&session, &edit_thread).await?),
        )
        .await?;
        ensure!(compact.status == 409);

        // Resumed sessions receive only unseen messages; a missing session starts over with the transcript.
        let resume_thread = make_thread("Resume").await?;
        let r1 = start_run(&session, &resume_thread, "RESUME_CHECK first", json!({})).await?;
        let d1 = wait_run(&session, r1["id"].as_str().context("r1 id")?, "completed", |_| true).await?;
        ensure!(d1["document"]["output"] == "Turns: 1. Input: User: RESUME_CHECK first");

        let rev = thread_revision(&session, &resume_thread).await?;
        call_agent(
            &session,
            &format!("/threads/{resume_thread}/messages"),
            Some(&json!({ "id": uuid(), "content": "Saved note" })),
            Some(rev),
        )
        .await?;

        let r2 = start_run(&session, &resume_thread, "RESUME_CHECK second", json!({})).await?;
        let d2 = wait_run(&session, r2["id"].as_str().context("r2 id")?, "completed", |_| true).await?;
        ensure!(d2["document"]["output"] == "Turns: 2. Input: User: Saved note User: RESUME_CHECK second");

        std::fs::write(home.join("fixture-sessions.json"), "{}")?;
        let r3 = start_run(&session, &resume_thread, "RESUME_CHECK third", json!({})).await?;
        let d3 = wait_run(&session, r3["id"].as_str().context("r3 id")?, "completed", |_| true).await?;
        let d3_out = d3["document"]["output"].as_str().context("d3 output")?;
        let resume_re = regex::Regex::new(r"^Turns: 1\. Input: User: RESUME_CHECK first .*User: RESUME_CHECK third$").unwrap();
        ensure!(resume_re.is_match(d3_out), "unexpected fallback transcript output: {d3_out:?}");

        // Questions use the answer card; escalations are declined; plan mode never edits.
        let q_thread = make_thread("Question").await?;
        let q_run = start_run(&session, &q_thread, "ASK_QUESTION", json!({})).await?;
        let q_id = q_run["id"].as_str().context("q id")?.to_owned();
        let asking = wait_run(&session, &q_id, "running", |run| !run["document"]["pending"].is_null()).await?;
        ensure!(asking["document"]["pending"]["questions"][0]["options"][0]["label"] == "Blue");
        let pending_id = asking["document"]["pending"]["id"].as_str().context("pending id")?;
        ensure!(
            agent_req(
                &session,
                &format!("/runs/{q_id}/answer"),
                Some(&json!({ "requestId": pending_id, "answers": { "0": ["Blue"] } })),
                None
            )
            .await?
            .status
                == 200
        );
        let answered = wait_run(&session, &q_id, "completed", |_| true).await?;
        ensure!(answered["document"]["output"] == "You chose Blue.");
        let q_step = answered["document"]["steps"]
            .as_array()
            .context("answered steps")?
            .iter()
            .find(|s| s["kind"] == "question")
            .context("question step")?;
        ensure!(q_step["items"] == json!([{ "text": "Blue", "done": true }]));

        let net_thread = make_thread("Network").await?;
        let net_run = start_run(&session, &net_thread, "REQUEST_NETWORK", json!({})).await?;
        let net_done = wait_run(&session, net_run["id"].as_str().context("net id")?, "completed", |_| true).await?;
        ensure!(
            net_done["document"]["steps"]
                .as_array()
                .is_some_and(|steps| steps.iter().any(|s| s["label"] == "Declined: network access to example.test")),
            "expected declined network step: {net_done}"
        );

        let plan_thread = make_thread("Plan").await?;
        let plan_run = start_run(&session, &plan_thread, "PLAN_ONLY", json!({ "mode": "plan" })).await?;
        let planned = wait_run(&session, plan_run["id"].as_str().context("plan id")?, "completed", |_| true).await?;
        ensure!(planned["document"]["output"] == "1. Read greeting\n2. Update greeting\n\nPlanned without editing.");

        // Steering is written into the running turn once and saved as a user message.
        let steer_thread = make_thread("Steer").await?;
        let steer_run = start_run(&session, &steer_thread, "WAIT_FOR_STEER", json!({})).await?;
        let steer_id = steer_run["id"].as_str().context("steer id")?.to_owned();
        wait_run(&session, &steer_id, "running", |run| {
            run["document"]["steps"].as_array().is_some_and(|s| !s.is_empty())
        })
        .await?;
        let guidance_id = uuid();
        let guidance = json!({ "id": guidance_id, "content": "Use blue" });
        for _ in 0..2 {
            ensure!(agent_req(&session, &format!("/runs/{steer_id}/steer"), Some(&guidance), None).await?.status == 201);
        }
        let steered = wait_run(&session, &steer_id, "completed", |_| true).await?;
        ensure!(steered["document"]["output"] == "Steered: Use blue");
        ensure!(steered["document"]["steered"] == json!([guidance_id]));

        // Images are sent as base64 content blocks from the private store.
        let image_thread = make_thread("Image").await?;
        let image_id = uuid();
        let png: [u8; 16] = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
        ];
        let upload_img = Reply::from(
            session
                .request("PUT", &format!("/api/v1/agent/threads/{image_thread}/attachments/{image_id}?name=pixel.png"))
                .header("content-type", "image/png")
                .body(png.to_vec())
                .send()
                .await?,
        )
        .await?;
        ensure!(upload_img.status == 201, "image upload failed: {} {}", upload_img.status, upload_img.text());
        let img_run = start_run(&session, &image_thread, "DESCRIBE_IMAGE", json!({ "attachments": [image_id] })).await?;
        let img_done = wait_run(&session, img_run["id"].as_str().context("img id")?, "completed", |_| true).await?;
        ensure!(img_done["document"]["output"] == "Saw 1 image.");

        // Cancellation interrupts the CLI; failures keep the provider's reason.
        let cancel_thread = make_thread("Cancel").await?;
        let waiting = start_run(&session, &cancel_thread, "WAIT_FOR_CANCEL", json!({})).await?;
        let waiting_id = waiting["id"].as_str().context("waiting id")?.to_owned();
        wait_run(&session, &waiting_id, "running", |run| {
            run["document"]["steps"].as_array().is_some_and(|s| !s.is_empty())
        })
        .await?;
        call_agent(&session, &format!("/runs/{waiting_id}/cancel"), Some(&json!({})), None).await?;
        wait_run(&session, &waiting_id, "cancelled", |_| true).await?;

        let fail_thread = make_thread("Fail").await?;
        let fail_run = start_run(&session, &fail_thread, "FAIL_CLI", json!({})).await?;
        let fail_done = wait_run(&session, fail_run["id"].as_str().context("fail id")?, "failed", |_| true).await?;
        ensure!(fail_done["document"]["error"].as_str().unwrap_or_default().contains("Fixture failure"));

        let exit_thread = make_thread("Exit").await?;
        let exit_run = start_run(&session, &exit_thread, "EXIT_WITHOUT_RESULT", json!({})).await?;
        let exit_done = wait_run(&session, exit_run["id"].as_str().context("exit id")?, "failed", |_| true).await?;
        ensure!(exit_done["document"]["error"].as_str().unwrap_or_default().contains("exited before completing"));

        // Usage windows come from the CLI's own report.
        let limits = call_agent(&session, "/provider/limits", None, None).await?;
        ensure!(limits["available"] == json!(true));
        ensure!(limits["plan"] == "max");
        let windows: Vec<(&str, f64, u64)> = limits["windows"]
            .as_array()
            .context("windows")?
            .iter()
            .map(|w| {
                (
                    w["label"].as_str().unwrap_or_default(),
                    w["usedPercent"].as_f64().unwrap_or_default(),
                    w["windowMinutes"].as_u64().unwrap_or_default(),
                )
            })
            .collect();
        ensure!(
            windows == vec![("Claude Code · 5 hour", 42.0, 300), ("Claude Code · Weekly", 7.0, 10080)],
            "unexpected limits windows: {windows:?}"
        );

        // Codex models are refused while only Claude Code is connected.
        let codex_run = agent_req(
            &session,
            &format!("/threads/{image_thread}/runs"),
            Some(&json!({ "id": uuid(), "messageId": uuid(), "model": "fixture-model", "content": "Codex" })),
            Some(thread_revision(&session, &image_thread).await?),
        )
        .await?;
        ensure!(codex_run.status == 409);

        Ok(())
    }
    .await;

    runtime.close().await;
    res
}
