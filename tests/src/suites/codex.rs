//! Port of `test/codex.integration.test.ts` and `test/agent-codex-browser.mjs`.

use crate::suites::agent_git_support::{create_user, sign_in_at};
use base64::Engine as _;
use dar_acceptance::browser::{FilePayload, State, contains, regex};
use dar_acceptance::instance::fixture_command;
use dar_acceptance::prelude::*;
use dar_acceptance::session::{Reply, Upload, client, upload_with};
use dar_acceptance::stack::stack;
use dar_bindings::create_folder_reducer::create_folder;
use dar_bindings::my_files_table::MyFilesTableAccess;
use dar_bindings::restore_entry_reducer::restore_entry;
use dar_bindings::trash_entry_reducer::trash_entry;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SUITE: Suite = Suite {
    name: "codex",
    cases: &[case!(codex_cli_and_browser)],
};

async fn agent_req(
    session: &Session,
    method: &str,
    route: &str,
    value: Option<&Value>,
    if_match: Option<u64>,
) -> Result<Reply> {
    let mut req = session.request(method, &format!("/api/v1/agent{route}"));
    if let Some(rev) = if_match {
        req = req.header("if-match", format!("\"{rev}\""));
    }
    if let Some(body) = value {
        req = req.json(body);
    }
    Reply::from(req.send().await?).await
}

async fn call_agent(
    session: &Session,
    route: &str,
    value: Option<&Value>,
    if_match: Option<u64>,
) -> Result<Value> {
    let method = if value.is_some() { "POST" } else { "GET" };
    let reply = agent_req(session, method, route, value, if_match).await?;
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
    call_agent(
        session,
        &format!("/threads/{thread_id}/messages"),
        None,
        None,
    )
    .await?["thread"]["revision"]
        .as_u64()
        .context("thread revision")
}

async fn start_run(
    session: &Session,
    thread_id: &str,
    content: &str,
    extra: Value,
) -> Result<Value> {
    let rev = thread_revision(session, thread_id).await?;
    let mut body = json!({
        "id": uuid(),
        "messageId": uuid(),
        "model": "fixture-model",
        "content": content
    });
    if let Some(map) = extra.as_object() {
        for (k, v) in map {
            body[k] = v.clone();
        }
    }
    call_agent(
        session,
        &format!("/threads/{thread_id}/runs"),
        Some(&body),
        Some(rev),
    )
    .await
}

async fn codex_cli_and_browser(ctx: &Ctx) -> Result<()> {
    let root = ctx.dir().join("codex-acceptance");
    std::fs::create_dir_all(&root)?;
    let executable = fixture_command(&root, "codex", &[])?;
    let home = root.join("codex-home");
    std::fs::create_dir_all(&home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    }
    let codex_file = root.join("codex.json");

    let runtime = Instance::isolated(
        &root,
        "dar-codex",
        InstanceConfig {
            codex_file: Some(codex_file.clone()),
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
                    "displayName": "Codex fixture",
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
        let valid_codex_cfg = json!({
            "executable": executable.to_string_lossy(),
            "home": home.to_string_lossy(),
            "userId": account_id,
            "gitHosts": []
        })
        .to_string();

        ensure!(call_agent(&session, "/provider", None, None).await?["configured"] == json!(false));
        std::fs::write(&codex_file, &valid_codex_cfg)?;
        let provider = call_agent(&session, "/provider", None, None).await?;
        ensure!(provider["connected"] == json!(true), "provider not connected: {provider}");
        ensure!(provider["models"][0]["id"] == "fixture-model");
        ensure!(!provider.to_string().contains("fixture@example.test"));

        let logged_out = home.join("logged-out");
        std::fs::write(&logged_out, "")?;
        ensure!(call_agent(&session, "/provider", None, None).await?["connected"] == json!(false));
        std::fs::remove_file(&logged_out)?;
        ensure!(call_agent(&session, "/provider", None, None).await?["connected"] == json!(true));
        std::fs::write(&codex_file, "invalid JSON")?;
        ensure!(call_agent(&session, "/provider", None, None).await?["configured"] == json!(false));
        std::fs::write(&codex_file, &valid_codex_cfg)?;
        ensure!(call_agent(&session, "/provider", None, None).await?["connected"] == json!(true));

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

        let thread_id = make_thread("Edit source").await?;
        let run_request = json!({
            "id": uuid(),
            "messageId": uuid(),
            "model": "fixture-model",
            "content": "Update the greeting."
        });
        let run = call_agent(&session, &format!("/threads/{thread_id}/runs"), Some(&run_request), Some(1)).await?;
        ensure!(run["status"] == "queued");
        let run_id = run["id"].as_str().context("run id")?.to_owned();
        let replay_run = call_agent(&session, &format!("/threads/{thread_id}/runs"), Some(&run_request), Some(1)).await?;
        ensure!(replay_run["id"] == run_id);

        let done = wait_run(&session, &run_id, "completed", |_| true).await?;
        ensure!(done["document"]["output"] == "The edit is ready.");
        ensure!(done["document"]["terminal"].as_str().unwrap_or_default().contains("fixture check passed"));
        let changes = done["document"]["changes"].as_array().context("changes")?;
        ensure!(changes.len() == 1);
        ensure!(changes[0]["fileId"] == file.file_id);
        ensure!(changes[0]["content"] == "Updated by Codex fixture.\n");

        let history = call_agent(&session, &format!("/threads/{thread_id}/messages"), None, None).await?;
        let roles: Vec<&str> = history["items"]
            .as_array()
            .context("history items")?
            .iter()
            .filter_map(|m| m["role"].as_str())
            .collect();
        ensure!(roles == vec!["user", "assistant"]);
        ensure!(history["items"].as_array().unwrap().last().unwrap()["content"] == "The edit is ready.");
        ensure!(provider["models"][0]["efforts"] == json!(["low", "medium", "high"]));
        ensure!(provider["models"][0]["defaultEffort"] == "medium");
        let step_kinds: Vec<&str> = done["document"]["steps"]
            .as_array()
            .context("steps")?
            .iter()
            .filter_map(|s| s["kind"].as_str())
            .collect();
        ensure!(step_kinds == vec!["reasoning", "plan", "edit", "command"]);
        let last_step = done["document"]["steps"].as_array().unwrap().last().unwrap();
        ensure!(last_step["exitCode"] == 0);
        ensure!(last_step["output"] == "fixture check passed\n");
        ensure!(done["document"]["usage"] == json!({ "input": 10, "cached": 4, "output": 5 }));

        let listed = call_agent(&session, &format!("/threads/{thread_id}/files?query=greet"), None, None).await?;
        ensure!(listed["source"] == "workspace");
        ensure!(listed["items"] == json!(["greeting.txt"]));

        let effort_thread = make_thread("Effort").await?;
        let bad_effort = agent_req(
            &session,
            "POST",
            &format!("/threads/{effort_thread}/runs"),
            Some(&json!({ "id": uuid(), "messageId": uuid(), "model": "fixture-model", "effort": "extreme", "content": "Effort check." })),
            Some(1),
        )
        .await?;
        ensure!(bad_effort.status == 400);

        let effort_run = call_agent(
            &session,
            &format!("/threads/{effort_thread}/runs"),
            Some(&json!({ "id": uuid(), "messageId": uuid(), "model": "fixture-model", "effort": "high", "content": "Effort check." })),
            Some(1),
        )
        .await?;
        let effort_run_id = effort_run["id"].as_str().context("effort run id")?.to_owned();
        ensure!(effort_run["document"]["effort"] == "high");
        let effort_done = wait_run(&session, &effort_run_id, "completed", |_| true).await?;
        ensure!(effort_done["document"]["effort"] == "high");

        // App-server runner: resumed sessions receive only unseen messages.
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
        let resumed = wait_run(&session, r2["id"].as_str().context("r2 id")?, "completed", |_| true).await?;
        ensure!(resumed["document"]["output"] == "Turns: 2. Input: User: Saved note User: RESUME_CHECK second");
        ensure!(resumed["document"]["codexThread"].as_str().is_some_and(|s| !s.is_empty()));

        // Blocking questions wait for the owner's answer; stale request IDs are rejected.
        let q_thread = make_thread("Question").await?;
        let q_run = start_run(&session, &q_thread, "ASK_QUESTION", json!({})).await?;
        let q_id = q_run["id"].as_str().context("q id")?.to_owned();
        let asking = wait_run(&session, &q_id, "running", |run| !run["document"]["pending"].is_null()).await?;
        ensure!(asking["document"]["pending"]["questions"][0]["options"][0]["label"] == "Blue");
        ensure!(
            agent_req(
                &session,
                "POST",
                &format!("/runs/{q_id}/answer"),
                Some(&json!({ "requestId": "stale", "answers": { "color": ["Red"] } })),
                None
            )
            .await?
            .status
                == 409
        );
        let pending_id = asking["document"]["pending"]["id"].as_str().context("pending id")?;
        ensure!(
            agent_req(
                &session,
                "POST",
                &format!("/runs/{q_id}/answer"),
                Some(&json!({ "requestId": pending_id, "answers": { "color": ["Blue"] } })),
                None
            )
            .await?
            .status
                == 200
        );
        let answered = wait_run(&session, &q_id, "completed", |_| true).await?;
        ensure!(answered["document"]["output"] == "You chose Blue.");
        ensure!(answered["document"]["pending"].is_null());
        let q_step = answered["document"]["steps"]
            .as_array()
            .context("answered steps")?
            .iter()
            .find(|s| s["kind"] == "question")
            .context("question step")?;
        ensure!(q_step["items"] == json!([{ "text": "Blue", "done": true }]));

        // Escalation requests are declined without user involvement.
        let approval_thread = make_thread("Approval").await?;
        let approval_run = start_run(&session, &approval_thread, "REQUEST_APPROVAL", json!({})).await?;
        let approval = wait_run(&session, approval_run["id"].as_str().context("approval id")?, "completed", |_| true).await?;
        ensure!(approval["document"]["output"] == "Approval was declined.");
        ensure!(
            approval["document"]["steps"]
                .as_array()
                .is_some_and(|steps| steps.iter().any(|s| s["label"].as_str().unwrap_or_default().starts_with("Declined: curl")))
        );

        // Steering reaches the running turn once and is saved as a user message.
        let steer_thread = make_thread("Steer").await?;
        let steer = start_run(&session, &steer_thread, "WAIT_FOR_STEER", json!({})).await?;
        let steer_id = steer["id"].as_str().context("steer id")?.to_owned();
        wait_run(&session, &steer_id, "running", |run| {
            run["document"]["steps"]
                .as_array()
                .is_some_and(|steps| steps.iter().any(|s| s["id"] == "steer-wait"))
        })
        .await?;
        let guidance_id = uuid();
        let guidance = json!({ "id": guidance_id, "content": "Use blue" });
        for _ in 0..2 {
            ensure!(agent_req(&session, "POST", &format!("/runs/{steer_id}/steer"), Some(&guidance), None).await?.status == 201);
        }
        let steered_run = wait_run(&session, &steer_id, "completed", |_| true).await?;
        ensure!(steered_run["document"]["output"] == "Steered: Use blue");
        ensure!(steered_run["document"]["steered"] == json!([guidance_id]));
        let steer_messages = call_agent(&session, &format!("/threads/{steer_thread}/messages"), None, None).await?;
        let steer_contents: Vec<&str> = steer_messages["items"]
            .as_array()
            .context("steer messages")?
            .iter()
            .filter_map(|m| m["content"].as_str())
            .collect();
        ensure!(steer_contents == vec!["WAIT_FOR_STEER", "Use blue", "Steered: Use blue"]);
        ensure!(
            agent_req(
                &session,
                "POST",
                &format!("/runs/{steer_id}/steer"),
                Some(&json!({ "id": uuid(), "content": "Too late" })),
                None
            )
            .await?
            .status
                == 409
        );

        // Plan mode is forwarded as the collaboration mode.
        let plan_thread = make_thread("Plan").await?;
        let plan_run = start_run(&session, &plan_thread, "PLAN_ONLY", json!({ "mode": "plan" })).await?;
        let planned = wait_run(&session, plan_run["id"].as_str().context("plan id")?, "completed", |_| true).await?;
        ensure!(planned["document"]["output"] == "Planned without editing.");
        ensure!(planned["document"]["mode"] == "plan");

        // Images are stored privately per thread, served inert, and passed as local images.
        let image_thread = make_thread("Image").await?;
        let image_id = uuid();
        let png: [u8; 16] = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
        ];
        let image_path = format!("/api/v1/agent/threads/{image_thread}/attachments/{image_id}");
        ensure!(
            Reply::from(
                session
                    .request("PUT", &format!("{image_path}?name=pixel.png"))
                    .header("content-type", "text/html")
                    .body(png.to_vec())
                    .send()
                    .await?
            )
            .await?
            .status
                == 415
        );
        ensure!(
            Reply::from(
                session
                    .request("PUT", &format!("{image_path}?name=pixel.png"))
                    .header("content-type", "image/png")
                    .body(png.to_vec())
                    .send()
                    .await?
            )
            .await?
            .status
                == 201
        );
        let served = session.get(&image_path).await?;
        ensure!(served.header("content-type") == Some("image/png"));
        ensure!(served.header("x-content-type-options") == Some("nosniff"));
        ensure!(served.body == png);
        let img_run = start_run(&session, &image_thread, "DESCRIBE_IMAGE", json!({ "attachments": [image_id] })).await?;
        let pictured = wait_run(&session, img_run["id"].as_str().context("img id")?, "completed", |_| true).await?;
        ensure!(pictured["document"]["output"] == "Saw 1 image.");
        ensure!(pictured["document"]["attachments"][0]["name"] == "pixel.png");
        let missing_img = agent_req(
            &session,
            "POST",
            &format!("/threads/{image_thread}/runs"),
            Some(&json!({ "id": uuid(), "messageId": uuid(), "model": "fixture-model", "content": "Missing", "attachments": [uuid()] })),
            Some(thread_revision(&session, &image_thread).await?),
        )
        .await?;
        ensure!(missing_img.status == 404);

        // Fork copies history up to a message; review and compaction run through the resumed Codex thread.
        let fork_id = uuid();
        ensure!(
            agent_req(
                &session,
                "POST",
                &format!("/threads/{resume_thread}/fork"),
                Some(&json!({ "id": fork_id, "sequence": 3 })),
                None
            )
            .await?
            .status
                == 201
        );
        let forked_messages = call_agent(&session, &format!("/threads/{fork_id}/messages"), None, None).await?;
        let forked_msgs: Vec<(u64, &str)> = forked_messages["items"]
            .as_array()
            .context("forked messages")?
            .iter()
            .map(|m| (m["sequence"].as_u64().unwrap_or_default(), m["content"].as_str().unwrap_or_default()))
            .collect();
        ensure!(
            forked_msgs
                == vec![
                    (2, "RESUME_CHECK first"),
                    (3, "Turns: 1. Input: User: RESUME_CHECK first")
                ]
        );
        ensure!(
            agent_req(
                &session,
                "POST",
                &format!("/threads/{resume_thread}/fork"),
                Some(&json!({ "id": uuid(), "sequence": 999 })),
                None
            )
            .await?
            .status
                == 400
        );

        let rev_run = start_run(&session, &resume_thread, "Review the uncommitted changes.", json!({ "mode": "review" })).await?;
        let reviewed = wait_run(&session, rev_run["id"].as_str().context("rev id")?, "completed", |_| true).await?;
        ensure!(reviewed["document"]["output"] == "Review: greeting.txt changed; no issues found.");

        let cmp_run = start_run(&session, &resume_thread, "Compact the conversation context.", json!({ "mode": "compact" })).await?;
        let compacted = wait_run(&session, cmp_run["id"].as_str().context("cmp id")?, "completed", |_| true).await?;
        ensure!(
            compacted["document"]["steps"]
                .as_array()
                .is_some_and(|steps| steps.iter().any(|s| s["id"] == "compacted"))
        );

        let empty_cmp_thread = make_thread("Nothing to compact").await?;
        let empty_cmp_run = start_run(&session, &empty_cmp_thread, "Compact", json!({ "mode": "compact" })).await?;
        let empty_cmp = wait_run(&session, empty_cmp_run["id"].as_str().context("empty cmp id")?, "failed", |_| true).await?;
        let cmp_err = empty_cmp["document"]["error"].as_str().unwrap_or_default().to_ascii_lowercase();
        ensure!(
            cmp_err.contains("nothing to compact") || cmp_err.contains("no codex context"),
            "unexpected empty compact error: {cmp_err:?}"
        );

        let limits = call_agent(&session, "/provider/limits", None, None).await?;
        ensure!(limits["available"] == json!(true));
        ensure!(limits["plan"] == "pro");
        let percents: Vec<f64> = limits["windows"]
            .as_array()
            .context("limits windows")?
            .iter()
            .filter_map(|w| w["usedPercent"].as_f64())
            .collect();
        ensure!(percents == vec![42.0, 7.0], "unexpected limits percents: {percents:?}");
        ensure!(
            limits["windows"][0]["resetsAt"].as_f64() == Some(1_900_000_000_000.0),
            "unexpected resetsAt: {}",
            limits["windows"][0]["resetsAt"]
        );

        let preferences = session.get_json("/api/v1/pim/preferences").await?;
        let pref_rev = preferences["revision"].as_u64().context("preferences revision")?;
        let pref_etag = format!("\"preferences:{pref_rev}\"");
        let mut bad_prefs = preferences.clone();
        bad_prefs["agentSnoozed"] = json!([{ "id": "bad", "until": 1 }]);
        ensure!(
            Reply::from(
                session
                    .request("PUT", "/api/v1/pim/preferences")
                    .header("if-match", &pref_etag)
                    .json(&bad_prefs)
                    .send()
                    .await?
            )
            .await?
            .status
                == 400
        );
        let until_ms = chrono::Utc::now().timestamp_millis() + 3_600_000;
        let mut good_prefs = preferences.clone();
        good_prefs["agentSnoozed"] = json!([{ "id": resume_thread, "until": until_ms }]);
        ensure!(
            Reply::from(
                session
                    .request("PUT", "/api/v1/pim/preferences")
                    .header("if-match", &pref_etag)
                    .json(&good_prefs)
                    .send()
                    .await?
            )
            .await?
            .status
                == 200
        );

        // Sharing grants another account read-only history; grantees cannot use owner routes.
        let friend = format!("friend-{}", &uuid()[..6]);
        create_user(&session, &friend, "Friend", PASSWORD).await?;
        let grantee = login_as(&base, &friend, PASSWORD).await?;
        let share_route = format!("/threads/{resume_thread}/shares");
        ensure!(
            agent_req(&session, "POST", &share_route, Some(&json!({ "id": uuid(), "username": "admin" })), None)
                .await?
                .status
                == 400
        );
        ensure!(
            agent_req(
                &session,
                "POST",
                &share_route,
                Some(&json!({ "id": uuid(), "username": format!("nobody-{}", &uuid()[..6]) })),
                None
            )
            .await?
            .status
                == 404
        );
        let granted = call_agent(&session, &share_route, Some(&json!({ "id": uuid(), "username": friend })), None).await?;
        ensure!(granted["username"] == friend);
        let granted_id = granted["id"].as_str().context("granted id")?.to_owned();
        let share_list = call_agent(&session, &share_route, None, None).await?;
        let share_users: Vec<&str> = share_list["items"]
            .as_array()
            .context("shares")?
            .iter()
            .filter_map(|r| r["username"].as_str())
            .collect();
        ensure!(share_users == vec![friend.as_str()]);

        let shared_list = call_agent(&grantee, "/shared", None, None).await?;
        let shared_pairs: Vec<(&str, &str)> = shared_list["items"]
            .as_array()
            .context("shared items")?
            .iter()
            .map(|r| (r["thread"]["id"].as_str().unwrap_or_default(), r["owner"].as_str().unwrap_or_default()))
            .collect();
        ensure!(shared_pairs == vec![(resume_thread.as_str(), "Codex fixture")]);
        let shared_msgs = call_agent(&grantee, &format!("/shared/{resume_thread}/messages"), None, None).await?;
        ensure!(shared_msgs["items"][0]["content"] == "RESUME_CHECK first");
        ensure!(shared_msgs["items"][0].get("ownerId").is_none());
        for route in [
            format!("/threads/{resume_thread}/messages"),
            format!("/threads/{resume_thread}/shares"),
            format!("/threads/{resume_thread}/git"),
            format!("/threads/{resume_thread}/files"),
        ] {
            ensure!(
                agent_req(&grantee, "GET", &route, None, None).await?.status == 404,
                "grantee should get 404 for {route}"
            );
        }
        ensure!(
            agent_req(&session, "DELETE", &format!("{share_route}/{granted_id}"), None, None).await?.status == 200
        );
        ensure!(call_agent(&grantee, "/shared", None, None).await?["items"] == json!([]));
        ensure!(agent_req(&grantee, "GET", &format!("/shared/{resume_thread}/messages"), None, None).await?.status == 404);

        // Rule files in the Codex home would act outside Dar's control; runs refuse until removed.
        let rules_dir = home.join("rules");
        std::fs::create_dir_all(&rules_dir)?;
        let default_rules = rules_dir.join("default.rules");
        std::fs::write(&default_rules, "")?;
        let rules_thread = make_thread("Rules").await?;
        let ruled = start_run(&session, &rules_thread, "RESUME_CHECK rules", json!({})).await?;
        let refused = wait_run(&session, ruled["id"].as_str().context("ruled id")?, "failed", |_| true).await?;
        ensure!(refused["document"]["error"].as_str().unwrap_or_default().contains("execution-policy rules"));
        std::fs::remove_file(&default_rules)?;

        // Local Git through Dar: status, branch and commit; no remotes are contacted.
        let git_route = format!("/threads/{thread_id}/git");
        let git_status = call_agent(&session, &git_route, None, None).await?;
        ensure!(git_status["available"] == json!(true));
        ensure!(git_status["branch"] == "agent");
        ensure!(git_status["changed"].as_u64().unwrap_or_default() > 0);
        ensure!(
            agent_req(&session, "POST", &git_route, Some(&json!({ "action": "branch", "name": "-bad" })), None)
                .await?
                .status
                == 400
        );
        let branched = call_agent(
            &session,
            &git_route,
            Some(&json!({ "action": "branch", "name": "feature/greeting" })),
            None,
        )
        .await?;
        ensure!(branched["branch"] == "feature/greeting");
        let committed = call_agent(
            &session,
            &git_route,
            Some(&json!({ "action": "commit", "message": "Update greeting" })),
            None,
        )
        .await?;
        ensure!(committed["changed"] == 0);
        ensure!(committed["commits"][0]["subject"] == "Update greeting");

        // Terminal: a PTY in the working copy through app-server command/exec, read by offset.
        let no_ws_thread = make_thread("No workspace").await?;
        ensure!(
            agent_req(&session, "POST", &format!("/threads/{no_ws_thread}/terminal"), Some(&json!({ "rows": 24, "cols": 80 })), None)
                .await?
                .status
                == 409
        );
        let shell_route = format!("/threads/{thread_id}/terminal");
        ensure!(
            agent_req(&session, "POST", &shell_route, Some(&json!({ "rows": 24, "cols": 80 })), None)
                .await?
                .status
                == 200
        );
        let mut screen = String::new();
        let mut cursor = 0u64;
        async fn poll_terminal<F>(
            session: &Session,
            shell_route: &str,
            screen: &mut String,
            cursor: &mut u64,
            check: F,
        ) -> Result<Value>
        where
            F: Fn(&str, &Value) -> bool,
        {
            for _ in 0..80 {
                let value = call_agent(session, &format!("{shell_route}?after={}", *cursor), None, None).await?;
                if let Some(b64) = value["data"].as_str()
                    && !b64.is_empty()
                {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .map_err(|e| Error::new(format!("terminal base64: {e}")))?;
                    screen.push_str(&String::from_utf8_lossy(&bytes));
                }
                if let Some(end) = value["end"].as_u64() {
                    *cursor = end;
                }
                if check(screen, &value) {
                    return Ok(value);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            bail!("Terminal output did not match: {screen:?}")
        }
        poll_terminal(&session, &shell_route, &mut screen, &mut cursor, |text, _| text.contains("$ ")).await?;
        ensure!(
            agent_req(
                &session,
                "POST",
                &format!("{shell_route}/input"),
                Some(&json!({ "data": "echo terminal works\r" })),
                None
            )
            .await?
            .status
                == 200
        );
        poll_terminal(&session, &shell_route, &mut screen, &mut cursor, |text, _| {
            text.contains("terminal works\r\n$ ")
        })
        .await?;
        ensure!(
            agent_req(&session, "POST", &format!("{shell_route}/resize"), Some(&json!({ "rows": 30, "cols": 100 })), None)
                .await?
                .status
                == 200
        );
        agent_req(&session, "POST", &format!("{shell_route}/input"), Some(&json!({ "data": "exit\r" })), None).await?;
        poll_terminal(&session, &shell_route, &mut screen, &mut cursor, |_, val| val["exited"] == json!(true)).await?;

        // Rename & delete thread (verifying workspace cleanup).
        ensure!(
            agent_req(
                &session,
                "PATCH",
                &format!("/threads/{effort_thread}"),
                Some(&json!({ "title": "Renamed effort thread" })),
                None
            )
            .await?
            .status
                == 200
        );
        let threads_list = call_agent(&session, "/threads", None, None).await?;
        let found_effort = threads_list["items"]
            .as_array()
            .context("threads items")?
            .iter()
            .find(|r| r["id"] == effort_thread)
            .context("renamed effort thread")?;
        ensure!(found_effort["title"] == "Renamed effort thread");
        ensure!(agent_req(&session, "DELETE", &format!("/threads/{effort_thread}"), None, None).await?.status == 200);
        let after_delete = call_agent(&session, "/threads", None, None).await?;
        ensure!(
            !after_delete["items"]
                .as_array()
                .context("after delete items")?
                .iter()
                .any(|r| r["id"] == effort_thread)
        );
        ensure!(agent_req(&session, "GET", &format!("/runs/{effort_run_id}"), None, None).await?.status == 404);
        ensure!(!runtime.root().join("agent-workspaces").join(&account_id).join(&effort_thread).exists());

        // Apply Files change with optimistic concurrency.
        ensure!(session.get(&format!("/api/v1/files/{}/content", file.file_id)).await?.text() == "Original source.\n");
        let change = &done["document"]["changes"][0];
        let change_content = change["content"].as_str().context("change content")?;
        let expected_rev = change["expectedRevision"].as_u64().context("expectedRevision")?;
        ensure!(change["mediaType"] == "text/plain");
        upload_with(
            &session,
            "greeting.txt",
            change_content,
            Upload {
                parent_id: &folder_id,
                file_id: Some(&file.file_id),
                expected_revision: Some(expected_rev),
                media_type: "text/plain",
            },
        )
        .await?;
        ensure!(session.get(&format!("/api/v1/files/{}/content", file.file_id)).await?.text() == "Updated by Codex fixture.\n");
        let stale = session
            .post(
                "/api/v1/files/uploads",
                &json!({ "fileId": file.file_id, "expectedRevision": 1, "name": "greeting.txt", "size": 3, "mediaType": "text/plain" }),
            )
            .await?;
        ensure!(stale.status == 409);

        // Cancellation, concurrent run refusal, and source revocation.
        let waiting_id = make_thread("Cancellation").await?;
        let waiting = call_agent(
            &session,
            &format!("/threads/{waiting_id}/runs"),
            Some(&json!({ "id": uuid(), "messageId": uuid(), "model": "fixture-model", "content": "WAIT_FOR_CANCEL" })),
            Some(1),
        )
        .await?;
        let waiting_run_id = waiting["id"].as_str().context("waiting run id")?.to_owned();
        wait_run(&session, &waiting_run_id, "running", |_| true).await?;
        let history_rev = history["thread"]["revision"].as_u64().context("history rev")?;
        let competing = agent_req(
            &session,
            "POST",
            &format!("/threads/{thread_id}/runs"),
            Some(&json!({ "id": uuid(), "messageId": uuid(), "model": "fixture-model", "content": "Concurrent run" })),
            Some(history_rev),
        )
        .await?;
        ensure!(competing.status == 409);
        call_agent(&session, &format!("/runs/{waiting_run_id}/cancel"), Some(&json!({})), None).await?;
        wait_run(&session, &waiting_run_id, "cancelled", |_| true).await?;

        let revoked_id = make_thread("Source access ended").await?;
        let revoked = call_agent(
            &session,
            &format!("/threads/{revoked_id}/runs"),
            Some(&json!({ "id": uuid(), "messageId": uuid(), "model": "fixture-model", "content": "WAIT_FOR_CANCEL" })),
            Some(1),
        )
        .await?;
        let revoked_run_id = revoked["id"].as_str().context("revoked run id")?.to_owned();
        wait_run(&session, &revoked_run_id, "running", |row| {
            row["document"]["activity"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item == "fixture waiting for cancellation"))
        })
        .await?;
        let src_live = session.connect().await?;
        let src_res: Result<()> = async {
            let folder_rev = src_live
                .db
                .my_files()
                .iter()
                .find(|row| row.id == folder_id)
                .context("source folder row")?
                .revision;
            live::reducer(|done| src_live.reducers.trash_entry_then(folder_id.clone(), folder_rev, done)).await?;
            wait_run(&session, &revoked_run_id, "interrupted", |_| true).await?;
            live::reducer(|done| src_live.reducers.restore_entry_then(folder_id.clone(), folder_rev + 1, done)).await?;
            Ok(())
        }
        .await;
        src_live.disconnect();
        src_res?;

        for content in ["FAIL_CLI", "EXIT_WITHOUT_TURN"] {
            let failed_id = make_thread(content).await?;
            let failed = call_agent(
                &session,
                &format!("/threads/{failed_id}/runs"),
                Some(&json!({ "id": uuid(), "messageId": uuid(), "model": "fixture-model", "content": content })),
                Some(1),
            )
            .await?;
            wait_run(&session, failed["id"].as_str().context("failed id")?, "failed", |_| true).await?;
            let msgs = call_agent(&session, &format!("/threads/{failed_id}/messages"), None, None).await?;
            ensure!(msgs["items"].as_array().map(|a| a.len()) == Some(1));
        }

        // Runtime restart interrupts in-flight runs.
        let restart_id = make_thread("Interrupted").await?;
        let restart_run = call_agent(
            &session,
            &format!("/threads/{restart_id}/runs"),
            Some(&json!({ "id": uuid(), "messageId": uuid(), "model": "fixture-model", "content": "WAIT_FOR_CANCEL" })),
            Some(1),
        )
        .await?;
        let restart_run_id = restart_run["id"].as_str().context("restart run id")?.to_owned();
        wait_run(&session, &restart_run_id, "running", |_| true).await?;
        runtime.restart().await?;
        wait_run(&session, &restart_run_id, "interrupted", |_| true).await?;
        let restart_msgs = call_agent(&session, &format!("/threads/{restart_id}/messages"), None, None).await?;
        ensure!(restart_msgs["items"].as_array().map(|a| a.len()) == Some(1));

        // Service worker conflict & other-user isolation.
        let db = runtime.database()?;
        let conflict_err = db
            .call(
                "service_agent_worker",
                json!([run_id, uuid(), "finish", json!({ "status": "completed", "document": done["document"] }).to_string()]),
            )
            .await
            .expect_err("duplicate finish should conflict");
        ensure!(conflict_err.code == "conflict", "expected conflict, got {conflict_err:?}");

        create_user(&session, "other", "Other", PASSWORD).await?;
        let other = login_as(&base, "other", PASSWORD).await?;
        ensure!(agent_req(&other, "GET", &format!("/runs/{run_id}"), None, None).await?.status == 404);
        ensure!(call_agent(&other, "/provider", None, None).await?["configured"] == json!(false));
        ensure!(agent_req(&other, "POST", "/provider/connect", None, None).await?.status == 403);

        // Full browser verification (ported from test/agent-codex-browser.mjs).
        codex_browser_flow(&base, &root, &project_id).await?;
        Ok(())
    }
    .await;

    runtime.close().await;
    res
}

async fn codex_browser_flow(base: &str, root: &Path, project_id: &str) -> Result<()> {
    let browser = Browser::shared().await?;
    let context = browser.new_context_sized(1440, 950).await?;
    let page = context.new_page().await?;
    let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let errors = errors.clone();
        page.on_page_error(move |msg| {
            errors.lock().unwrap().push(msg.to_owned());
        });
    }

    let result: Result<()> = async {
        sign_in_at(
            &page,
            &format!("{base}/agent?project={project_id}"),
            "admin",
            PASSWORD,
        )
        .await?;
        page.get_by_role("button", "New thread")
            .first()
            .click()
            .await?;
        page.get_by_role("button", "Project: Files code. Change project")
            .click()
            .await?;
        page.get_by_role("complementary", "Choose project")
            .get_by_role("button", "Files code Dar Files")
            .click()
            .await?;
        page.get_by_label("Message").wait_for().await?;
        page.locator("[data-editor-pane]")
            .wait_for_state(State::Hidden)
            .await?;
        page.get_by_label("Message")
            .fill("Update the greeting from the browser.")
            .await?;
        page.get_by_role("button", "Run with Codex").click().await?;

        page.get_by_text("Codex · completed")
            .wait_for_state(State::Attached)
            .await?;
        page.locator(".agent-message-assistant")
            .get_by_text("The edit is ready.")
            .wait_for()
            .await?;
        ensure!(!page.locator(".agent-review").is_visible().await?);
        page.get_by_role("button", "Show changes").click().await?;
        page.get_by_role("region", "File diff").wait_for().await?;
        ensure!(page.locator(".code-diff-line.added").count().await? > 0);
        ensure!(page.locator(".code-diff-line.removed").count().await? > 0);
        page.get_by_role("button", regex("^Worked for"))
            .first()
            .click()
            .await?;
        page.get_by_text("Checking the greeting file before editing.")
            .wait_for()
            .await?;
        page.locator(".agent-plan")
            .get_by_text("Update greeting")
            .wait_for()
            .await?;
        page.get_by_role("button", regex("Ran fixture test command"))
            .click()
            .await?;
        page.locator(".agent-step-output")
            .filter_has_text("fixture check passed")
            .wait_for()
            .await?;

        page.get_by_role(
            "button",
            regex("^Codex model: Fixture model, Medium reasoning"),
        )
        .click()
        .await?;
        page.get_by_role("radio", "High").click().await?;
        page.keyboard().press("Escape").await?;
        page.get_by_role(
            "button",
            regex("^Codex model: Fixture model, High reasoning"),
        )
        .wait_for()
        .await?;

        page.get_by_label("Message").fill("/effort l").await?;
        page.get_by_role("option", regex("Low")).click().await?;
        page.get_by_role(
            "button",
            regex("^Codex model: Fixture model, Low reasoning"),
        )
        .wait_for()
        .await?;

        page.get_by_role("button", "Wrap lines").click().await?;
        ensure!(
            page.get_by_role("button", "Wrap lines")
                .get_attribute("aria-pressed")
                .await?
                .as_deref()
                == Some("true")
        );
        page.get_by_role("button", "Collapse diff").click().await?;
        ensure!(page.get_by_role("region", "File diff").count().await? == 0);
        page.get_by_role("button", "Expand diff").click().await?;
        page.get_by_role("button", "Apply this edit to Files")
            .click()
            .await?;
        page.get_by_role("button", "Applied to Files")
            .wait_for()
            .await?;

        page.reload().await?;
        page.locator(".agent-message-assistant")
            .get_by_text("The edit is ready.")
            .wait_for()
            .await?;

        page.get_by_label("Message")
            .fill("Save context without another model call.")
            .await?;
        page.get_by_role("button", "Message actions")
            .click()
            .await?;
        page.get_by_role("menuitem", "Save only").click().await?;
        page.get_by_text("Message saved. It has not been sent to a model.")
            .wait_for()
            .await?;
        page.reload().await?;
        page.get_by_label("Conversation messages")
            .get_by_text("Saved · not sent to a model")
            .wait_for()
            .await?;

        for theme in ["latte", "mocha"] {
            page.evaluate_with(
                "value => { document.documentElement.dataset.theme = value; }",
                json!(theme),
            )
            .await?;
            for width in [1440u32, 390] {
                page.set_viewport_size(width, 844).await?;
                if width == 390 {
                    page.get_by_role("button", "Changes").click().await?;
                    page.locator(".code-diff")
                        .wait_for_state(State::Visible)
                        .await?;
                    page.get_by_role("button", "Terminal").click().await?;
                    page.get_by_text(contains("fixture check passed"))
                        .wait_for()
                        .await?;
                    page.get_by_role("button", "Conversation").click().await?;
                }
                ensure!(
                    page.evaluate("() => document.documentElement.scrollWidth > innerWidth")
                        .await?
                        == json!(false)
                );
                ensure!(
                    page.evaluate("() => document.documentElement.scrollHeight <= innerHeight + 1")
                        .await?
                        == json!(true)
                );
                page.screenshot(root.join(format!("codex-{theme}-{width}.png")), true)
                    .await?;
            }
        }

        page.set_viewport_size(1440, 950).await?;
        page.get_by_role("button", "Close changes").click().await?;
        page.get_by_role("button", "New thread")
            .first()
            .click()
            .await?;
        page.get_by_role("button", "Project: Files code. Change project")
            .wait_for()
            .await?;
        page.get_by_label("Message").fill("WAIT_FOR_CANCEL").await?;
        page.locator(".agent-send:not(:disabled)")
            .wait_for()
            .await?;
        page.get_by_label("Message").press("Enter").await?;
        page.get_by_role("region", "Work log")
            .get_by_text("fixture waiting for cancellation")
            .wait_for()
            .await?;
        page.get_by_label("Message")
            .fill("Keep this next-turn draft")
            .await?;
        page.get_by_role("button", "Stop").click().await?;
        page.get_by_text("Codex · cancelled")
            .first()
            .wait_for_state(State::Attached)
            .await?;
        page.reload().await?;
        page.get_by_text("Codex · cancelled")
            .first()
            .wait_for_state(State::Attached)
            .await?;
        ensure!(page.get_by_label("Message").input_value().await? == "Keep this next-turn draft");

        // App-server interactions in a fresh thread: steering, a blocking question, plan mode and an image.
        page.get_by_role("button", "New thread")
            .first()
            .click()
            .await?;
        let composer = page.get_by_label("Message");
        let log = page.get_by_role("region", "Work log");
        composer.fill("WAIT_FOR_STEER").await?;
        composer.press("Enter").await?;
        log.get_by_text("fixture waiting for guidance")
            .wait_for()
            .await?;

        composer.fill("Use blue").await?;
        page.get_by_role("button", "Steer the running turn")
            .click()
            .await?;
        page.locator(".agent-message-assistant")
            .get_by_text("Steered: Use blue")
            .wait_for()
            .await?;
        let steered_msg = page
            .locator(".agent-message-user")
            .filter_has_text("Use blue");
        steered_msg.wait_for().await?;
        ensure!(
            steered_msg
                .get_by_text(contains("Saved · not sent to a model"))
                .count()
                .await?
                == 0
        );

        // The reply shows while the turn is still running, and Enter steers a running turn: a new
        // task waits until the turn has ended (Stop gives way to Send).
        page.get_by_role("button", "Stop")
            .wait_for_state(State::Detached)
            .await?;
        composer.fill("ASK_QUESTION").await?;
        composer.press("Enter").await?;
        let card = page.get_by_role("form", "Codex is asking");
        card.get_by_text("Which color should the greeting use?")
            .wait_for()
            .await?;
        card.get_by_role("radio", regex("Blue")).check().await?;
        card.get_by_role("button", "Send answer").click().await?;
        page.locator(".agent-message-assistant")
            .get_by_text("You chose Blue.")
            .wait_for()
            .await?;
        card.wait_for_state(State::Detached).await?;

        page.get_by_role("button", "Stop")
            .wait_for_state(State::Detached)
            .await?;
        let plan_toggle = page.get_by_role("button", "Plan");
        plan_toggle.click().await?;
        ensure!(plan_toggle.get_attribute("aria-pressed").await?.as_deref() == Some("true"));
        composer.fill("PLAN_ONLY").await?;
        composer.press("Enter").await?;
        page.locator(".agent-message-assistant")
            .get_by_text("Planned without editing.")
            .wait_for()
            .await?;
        page.get_by_role("button", "Stop")
            .wait_for_state(State::Detached)
            .await?;
        plan_toggle.click().await?;
        ensure!(plan_toggle.get_attribute("aria-pressed").await?.as_deref() == Some("false"));

        let png: [u8; 16] = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52,
        ];
        page.locator(".agent-composer input[type=file]")
            .set_input_files(vec![FilePayload::new(
                "pixel.png",
                "image/png",
                png.to_vec(),
            )])
            .await?;
        page.get_by_role("list", "Attached images")
            .get_by_role("img", "pixel.png")
            .wait_for()
            .await?;
        page.locator(".agent-pictures li[data-status=ready]")
            .wait_for()
            .await?;
        composer.fill("DESCRIBE_IMAGE").await?;
        composer.press("Enter").await?;
        page.locator(".agent-message-assistant")
            .get_by_text("Saw 1 image.")
            .wait_for()
            .await?;
        page.locator(".agent-message-images")
            .get_by_role("img", "pixel.png")
            .wait_for()
            .await?;
        ensure!(page.get_by_role("list", "Attached images").count().await? == 0);
        page.screenshot(root.join("codex-interactive.png"), true)
            .await?;

        // Fork from a message, Codex review, Markdown export, snooze and usage limits.
        let first = page
            .locator(".agent-message-user")
            .filter_has_text("WAIT_FOR_STEER");
        first.hover().await?;
        first
            .get_by_role("button", "Fork from here")
            .click()
            .await?;
        page.get_by_role("heading", "Fork: WAIT_FOR_STEER")
            .wait_for()
            .await?;
        page.locator(".agent-message-user")
            .get_by_text("WAIT_FOR_STEER")
            .wait_for()
            .await?;
        ensure!(page.locator(".agent-message").count().await? == 1);

        composer.fill("/review").await?;
        composer.press("Enter").await?;
        page.locator(".agent-message-assistant")
            .get_by_text("Review: greeting.txt changed; no issues found.")
            .wait_for()
            .await?;

        let download = page
            .wait_for_download(async {
                page.get_by_role("button", "Thread actions").click().await?;
                page.get_by_role("menuitem", "Export as Markdown")
                    .click()
                    .await
            })
            .await?;
        ensure!(
            download.suggested_filename == "Fork WAIT_FOR_STEER.md",
            "unexpected download filename: {}",
            download.suggested_filename
        );

        page.locator(".agent-thread-row")
            .filter_has_text("Fork: WAIT_FOR_STEER")
            .hover()
            .await?;
        page.get_by_role("button", "Actions for Fork: WAIT_FOR_STEER")
            .click()
            .await?;
        page.get_by_role("menuitem", "Snooze").click().await?;
        page.get_by_role("menuitem", "In 1 hour").click().await?;
        let snoozed_shelf = page
            .locator(".agent-shelves details")
            .filter_has(&page.locator("summary").filter_has_text("Snoozed"));
        snoozed_shelf
            .get_by_text("Fork: WAIT_FOR_STEER")
            .wait_for_state(State::Attached)
            .await?;
        snoozed_shelf.locator("summary").click().await?;
        snoozed_shelf
            .get_by_text("Fork: WAIT_FOR_STEER")
            .wait_for()
            .await?;
        page.locator(".agent-thread-row")
            .filter_has_text("Fork: WAIT_FOR_STEER")
            .hover()
            .await?;
        page.get_by_role("button", "Actions for Fork: WAIT_FOR_STEER")
            .click()
            .await?;
        page.get_by_role("menuitem", "Unsnooze thread")
            .click()
            .await?;
        snoozed_shelf
            .get_by_text("Fork: WAIT_FOR_STEER")
            .wait_for_state(State::Detached)
            .await?;

        page.get_by_role("button", "Agent settings").click().await?;
        page.get_by_role("dialog", "Agent settings")
            .get_by_text("42%")
            .wait_for()
            .await?;
        page.keyboard().press("Escape").await?;

        // Sharing: owner grants read-only access; the grantee sees messages without a composer.
        let friend = format!("reader-{}", &uuid()[..6]);
        let admin_session = page
            .request()
            .get(&format!("{base}/api/v1/auth/session"))
            .await?
            .json()?;
        let csrf = admin_session["csrfToken"]
            .as_str()
            .context("csrfToken")?
            .to_owned();
        let created_friend = page
            .request()
            .post(
                &format!("{base}/api/v1/auth/users"),
                &[("origin", base), ("x-csrf-token", csrf.as_str())],
                json!({ "username": friend, "displayName": "Reader", "password": PASSWORD }),
            )
            .await?;
        ensure!(created_friend.status == 201);

        page.locator(".agent-thread-select")
            .filter_has_text("Fork: WAIT_FOR_STEER")
            .click()
            .await?;
        page.get_by_role("heading", "Fork: WAIT_FOR_STEER")
            .wait_for()
            .await?;
        page.get_by_role("button", "Thread actions").click().await?;
        page.get_by_role("menuitem", "Share…").click().await?;
        let share_dialog = page.get_by_role("dialog", "Share thread");
        share_dialog
            .get_by_placeholder("Dar username")
            .fill(&friend)
            .await?;
        share_dialog.get_by_role("button", "Share").click().await?;
        share_dialog
            .get_by_role("list", "People with access")
            .get_by_text(format!("{friend} · read only"))
            .wait_for()
            .await?;
        page.keyboard().press("Escape").await?;

        let reader_context = browser.new_context_sized(1280, 900).await?;
        let reader_res: Result<()> = async {
            let reader = reader_context.new_page().await?;
            sign_in_at(&reader, &format!("{base}/agent"), &friend, PASSWORD).await?;
            reader
                .locator(".agent-thread-select")
                .filter_has_text("Fork: WAIT_FOR_STEER")
                .click()
                .await?;
            reader
                .get_by_role("note", ())
                .get_by_text("Codex fixture")
                .wait_for()
                .await?;
            reader
                .locator(".agent-message-assistant")
                .get_by_text("Review: greeting.txt changed; no issues found.")
                .wait_for()
                .await?;
            ensure!(reader.get_by_label("Message").count().await? == 0);
            reader
                .screenshot(root.join("codex-shared.png"), true)
                .await?;
            Ok(())
        }
        .await;
        reader_context.close().await.ok();
        reader_res?;

        page.locator(".agent-thread-select")
            .filter_has_text("WAIT_FOR_STEER")
            .filter_has_not_text("Fork")
            .click()
            .await?;
        page.get_by_role("heading", "WAIT_FOR_STEER")
            .wait_for()
            .await?;
        page.get_by_role("button", "Toggle command output")
            .click()
            .await?;
        page.get_by_role("tab", "Terminal").click().await?;
        page.get_by_role("button", "Open terminal").click().await?;
        let shell_screen = page.get_by_role("log", "Terminal");
        shell_screen
            .get_by_text(contains("$"))
            .first()
            .wait_for()
            .await?;
        shell_screen.press("e").await?;
        page.keyboard().type_text("cho from browser").await?;
        page.keyboard().press("Enter").await?;
        shell_screen.get_by_text("from browser").wait_for().await?;
        page.screenshot(root.join("codex-terminal.png"), true)
            .await?;
        page.get_by_role("button", "End session").click().await?;
        page.get_by_role("button", "Restart")
            .or(&page.get_by_role("button", "Start a new terminal"))
            .first()
            .wait_for()
            .await?;
        page.get_by_role("button", "Close command output")
            .click()
            .await?;

        let session_info = page
            .request()
            .get(&format!("{base}/api/v1/auth/session"))
            .await?
            .json()?;
        let projects = page
            .request()
            .get(&format!("{base}/api/v1/agent/projects"))
            .await?
            .json()?;
        let project = projects["items"]
            .as_array()
            .context("projects items")?
            .iter()
            .find(|p| p["id"] == project_id)
            .context("codex project")?;
        let source_ref = project["sourceRef"].as_str().context("project sourceRef")?;
        let folders = page
            .request()
            .get(&format!(
                "{base}/api/v1/agent/folders?parentId={source_ref}"
            ))
            .await?
            .json()?;
        ensure!(folders["folder"]["id"] == source_ref);
        ensure!(
            session_info["user"]["id"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );

        let page_errors = errors.lock().unwrap().clone();
        ensure!(
            page_errors.is_empty(),
            "unexpected page errors: {page_errors:?}"
        );
        Ok(())
    }
    .await;

    if result.is_err() {
        let _ = page.screenshot(root.join("codex-failure.png"), true).await;
    }
    context.close().await.ok();
    result
}
