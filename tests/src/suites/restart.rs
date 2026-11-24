//! Forced crash & full database/runtime restart (`restart.mjs`): SIGKILL during an in-flight
//! upload, full SpacetimeDB + runtime restart with in-place module republish, stale lock/temp
//! reconciliation, durable bytes, WebDAV app password, Git repository clone, Books, Photos,
//! Notes/trash, Calendar/Tasks reminder catch-up without duplication, Agent thread/message,
//! OIDC unredeemed code/silent login/refresh/SCIM, and offline app disable/uninstall/reinstall.

use super::core_support::{epub, png, query_param};
use dar_acceptance::oidc::{Authorize, authorize};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use dar_acceptance::stack::{SpacetimeServer, free_port, stack, write_private};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

pub const SUITE: Suite = Suite {
    name: "restart",
    cases: &[case!(forced_crash_and_full_restart, 180)],
};

pub async fn forced_crash_and_full_restart(t: &Ctx) -> Result<()> {
    let stack = stack();
    let root = t.temp("dar-restart-proof")?;
    let port = free_port()?;
    let origin = format!("http://127.0.0.1:{port}");
    let db_name = "dar-restart";

    support::manage_apps(
        &root,
        "apps-install",
        Some(&stack.artifacts.prayer_times.to_string_lossy()),
    )?;

    let bootstrap_token_file = root.join("secrets/bootstrap-token");
    write_private(
        &bootstrap_token_file,
        dar_acceptance::base64url(&dar_acceptance::random_bytes(32)).as_bytes(),
    )?;

    let db_server = SpacetimeServer::start(&root.join("spacetime"), &stack.artifacts).await?;
    db_server.publish(&stack.artifacts, db_name, &origin)?;

    let make_config = || InstanceConfig {
        origin: Some(origin.clone()),
        root: root.clone(),
        database_name: db_name.into(),
        port,
        development: true,
        database_url: Some(db_server.url.clone()),
        service_token_file: Some(db_server.token_file.clone()),
        bootstrap_token_file: Some(bootstrap_token_file.clone()),
        ..InstanceConfig::default()
    };

    let mut runtime = Instance::start(make_config()).await?;

    let result = async {
        let bootstrap = dar_acceptance::read_to_string(&bootstrap_token_file)?;
        let setup = session::client()
            .post(format!("{origin}/api/v1/auth/setup"))
            .header("origin", &origin)
            .json(&json!({
                "username": "admin",
                "displayName": "Restart proof",
                "password": PASSWORD,
                "bootstrapToken": bootstrap.trim(),
            }))
            .send()
            .await?;
        assert_eq!(setup.status().as_u16(), 200);

        let session = login(&origin).await?;
        let set_app = |id: &str, enabled: bool| {
            let session = session.clone();
            let id = id.to_owned();
            async move {
                let settings = session.get_json("/api/v1/appSettings").await?;
                let rev = settings["revision"].as_u64().context("appSettings revision")?;
                let reply = Reply::from(
                    session
                        .request("PUT", "/api/v1/admin/installedApps")
                        .header("if-match", format!("\"apps:{rev}\""))
                        .json(&json!({ "id": id, "enabled": enabled, "revision": rev }))
                        .send()
                        .await?,
                )
                .await?;
                assert_eq!(reply.status, 200, "{}", reply.text());
                Ok::<(), Error>(())
            }
        };

        let app_endpoint = "/api/v1/apps/org.dar.prayer-times/userData/restartProof";
        let put_app = Reply::from(
            session
                .request("PUT", app_endpoint)
                .header("if-match", "\"0\"")
                .json(&json!({ "schemaVersion": 1, "value": { "location": "private restart proof" } }))
                .send()
                .await?,
        )
        .await?;
        assert_eq!(put_app.status, 200);

        let identity = IdentityAdmin::new(&origin).await?;
        let client = identity.default_client().await?;
        let (_, scim_secret) = identity.credential("write").await?;
        let active_tokens = client.exchange(&authorize(&origin, &identity.session, &client, Authorize::default()).await?).await?;
        let pending_code = authorize(&origin, &identity.session, &client, Authorize::default()).await?;
        let provider_keys: Value = session::client().get(format!("{origin}/oidc/jwks")).send().await?.json().await?;

        let dir_resp = Reply::from(
            session::client()
                .post(format!("{origin}/scim/v2/Users"))
                .bearer_auth(&scim_secret)
                .header("content-type", "application/scim+json")
                .json(&json!({ "userName": "durable-directory", "displayName": "Durable directory" }))
                .send()
                .await?,
        )
        .await?
        .expect(201)?;
        let dir_user_id = dir_resp.json()?["id"].as_str().context("dir user id")?.to_owned();

        let calendar = session
            .post("/api/v1/calendar/calendars", &json!({ "name": "Durable calendar", "color": "#2563eb" }))
            .await?
            .expect(201)?
            .json()?;
        let calendar_id = calendar["id"].as_str().context("calendar id")?.to_owned();

        let now = chrono::Utc::now();
        let reminder_start_dt = now + chrono::Duration::milliseconds(3_605_000);
        let reminder_end_dt = now + chrono::Duration::milliseconds(7_205_000);
        let reminder_start = reminder_start_dt.format("%Y-%m-%dT%H:%M:%S").to_string();
        let reminder_end = reminder_end_dt.format("%Y-%m-%dT%H:%M:%S").to_string();

        let cal_event = session
            .post(
                &format!("/api/v1/calendar/calendars/{calendar_id}/items"),
                &json!({ "data": { "title": "Reminder after crash", "start": reminder_start, "end": reminder_end, "timezone": "UTC", "reminderMinutes": 60 } }),
            )
            .await?
            .expect(201)?
            .json()?;
        let cal_event_id = cal_event["id"].as_str().context("calendar event id")?.to_owned();

        let notebook = session.post("/api/v1/notes/notebooks", &json!({ "name": "Durable notes" })).await?.expect(201)?.json()?;
        let notebook_id = notebook["id"].as_str().context("notebook id")?.to_owned();

        let agent_project = session
            .post(
                "/api/v1/agent/projects",
                &json!({ "id": uuid(), "name": "Durable Agent project", "sourceType": "git", "sourceRef": "https://example.test/you/repo.git" }),
            )
            .await?
            .expect(201)?
            .json()?;
        let agent_project_id = agent_project["id"].as_str().context("agent project id")?.to_owned();
        let agent_thread = session
            .post("/api/v1/agent/threads", &json!({ "id": uuid(), "projectId": agent_project_id, "title": "Durable conversation" }))
            .await?
            .expect(201)?
            .json()?;
        let agent_thread_id = agent_thread["id"].as_str().context("agent thread id")?.to_owned();
        let agent_msg_id = uuid();
        let agent_msg_content = "Keep this task through a runtime crash.";
        let msg_resp = Reply::from(
            session
                .request("POST", &format!("/api/v1/agent/threads/{agent_thread_id}/messages"))
                .header("if-match", "\"1\"")
                .json(&json!({ "id": agent_msg_id, "content": agent_msg_content }))
                .send()
                .await?,
        )
        .await?;
        assert_eq!(msg_resp.status, 201);

        let exact_markdown = "# Durable source\r\n\r\nUnchanged after a crash.\n";
        let note = session
            .post(
                "/api/v1/notes/items",
                &json!({ "collectionId": notebook_id, "title": "Durable note", "content": exact_markdown, "tags": ["restart"], "favorite": true }),
            )
            .await?
            .expect(201)?
            .json()?;
        let note_id = note["id"].as_str().context("note id")?.to_owned();
        let note_uid = note["uid"].as_str().context("note uid")?.to_owned();

        let trashed_resp = session
            .post("/api/v1/notes/items", &json!({ "collectionId": notebook_id, "title": "Durable trash", "content": exact_markdown }))
            .await?
            .expect(201)?;
        let trashed_etag = trashed_resp.header("etag").context("trashed etag")?.to_owned();
        let trashed_id = trashed_resp.json()?["id"].as_str().context("trashed id")?.to_owned();
        let del = Reply::from(
            session
                .request("DELETE", &format!("/api/v1/notes/items/{trashed_id}"))
                .header("if-match", &trashed_etag)
                .send()
                .await?,
        )
        .await?;
        assert_eq!(del.status, 200);

        let task = session
            .post(
                "/api/v1/tasks/items",
                &json!({
                    "collectionId": calendar_id,
                    "data": { "title": "Task reminder after crash", "start": reminder_start, "due": reminder_end, "timezone": "UTC", "reminderMinutes": 60, "recurrence": "FREQ=DAILY;COUNT=2" }
                }),
            )
            .await?
            .expect(201)?
            .json()?;
        let task_id = task["id"].as_str().context("task id")?.to_owned();
        let task_content = task["content"].as_str().context("task content")?.to_owned();
        let task_uid = task["uid"].as_str().context("task uid")?.to_owned();

        let owner = Database::new(&db_server.url, db_name, &db_server.token_file)?;
        // Do not run deliver_reminders before the reminder is due: if >1s elapsed since creation,
        // commit_reminders advances run_at by 60s and defers delivery past the fast restart.
        let pre_crash_inbox = session.get_json("/api/v1/pim/reminders").await?;
        assert_eq!(pre_crash_inbox["items"].as_array().map_or(0, Vec::len), 0);

        let durable = upload(&session, "Durable.txt", "Committed content survives process failure.").await?;

        // WebDAV app password + PUT
        let app_password = session
            .post("/api/v1/auth/app-passwords", &json!({ "label": "Sync client", "scope": "write" }))
            .await?
            .expect(201)?
            .json()?;
        let app_pw_secret = app_password["secret"].as_str().context("app password secret")?.to_owned();
        let dav_url = app_password["davUrl"].as_str().context("davUrl")?.to_owned();
        let dav_basic = format!("Basic {}", dar_acceptance::base64(format!("admin:{app_pw_secret}").as_bytes()));
        let dav_file = format!("{dav_url}Synced.txt");
        let dav_put = Reply::from(
            session::client().put(&dav_file).header("authorization", &dav_basic).body("Written by a sync client.").send().await?,
        )
        .await?
        .expect(201)?;
        let dav_etag = dav_put.header("etag").context("dav etag")?.to_owned();

        // Tracked folder & Git repository, Book EPUB + reading state, Photo + description
        let repo_folder = uuid();
        {
            let live = session.connect().await?;
            reducer(|cb| live.reducers.create_folder_then(repo_folder.clone(), String::new(), "Durable repository".into(), cb)).await?;
            live.disconnect();
        }
        upload_with(&session, "README.md", "# Durable repository\n", Upload { parent_id: &repo_folder, ..Upload::default() }).await?;
        let history_path = format!("/api/v1/files/history/{repo_folder}");
        session.post(&history_path, &json!({ "action": "track" })).await?.expect(200)?;
        let tracked = session.get_json(&history_path).await?;
        let token = tracked["status"]["token"].as_str().context("history token")?.to_owned();
        session.post(&format!("{history_path}/commits"), &json!({ "message": "Durable start", "token": token })).await?.expect(201)?;

        let book = upload_with(
            &session,
            "durable-voyage.epub",
            epub("Durable Voyage", "Restart Fixture"),
            Upload { media_type: "application/epub+zip", ..Upload::default() },
        )
        .await?;
        session
            .put(
                &format!("/api/v1/books/{}/state", book.file_id),
                &json!({ "revision": 0, "status": "reading", "position": { "spine": 1, "fraction": 0.5 }, "progress": 0.6, "device": "Restart proof" }),
            )
            .await?
            .expect(200)?;

        let photo_target = session.post("/api/v1/photos/upload-target", &json!({ "year": 2024, "month": 4 })).await?.expect(200)?.json()?;
        let photo_parent = photo_target["parentId"].as_str().context("photo parentId")?.to_owned();
        let photo = upload_with(
            &session,
            "durable.png",
            png(32, 20, [20, 90, 160]),
            Upload { parent_id: &photo_parent, media_type: "image/png", ..Upload::default() },
        )
        .await?;
        session.post(&format!("/api/v1/photos/assets/{}/index", photo.file_id), &json!({})).await?.expect(200)?;
        let photo_rev = session.get_json(&format!("/api/v1/photos/assets/{}", photo.file_id)).await?["revision"].as_u64().context("photo revision")?;
        session
            .put(&format!("/api/v1/photos/assets/{}/description", photo.file_id), &json!({ "description": "Durable photo", "revision": photo_rev }))
            .await?
            .expect(200)?;

        // Start a partial in-flight upload (1024 of 4096 bytes) over raw TCP so we can SIGKILL mid-transfer.
        let reservation = session
            .post("/api/v1/files/uploads", &json!({ "name": "Interrupted.txt", "parentId": "", "size": 4096, "mediaType": "text/plain" }))
            .await?
            .expect(201)?
            .json()?;
        let res_id = reservation["id"].as_str().context("reservation id")?.to_owned();
        let res_file_id = reservation["fileId"].as_str().context("reservation fileId")?.to_owned();

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        let partial_req = format!(
            "PUT /api/v1/files/uploads/{res_id} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {}\r\nOrigin: {origin}\r\nX-CSRF-Token: {}\r\nContent-Length: 4096\r\n\r\n",
            session.cookie, session.csrf_token
        );
        stream.write_all(partial_req.as_bytes()).await?;
        stream.write_all(&vec![b'x'; 1024]).await?;
        stream.flush().await?;

        let tmp_path = root.join("uploads").join(format!("{res_id}.tmp"));
        eventually(Duration::from_secs(15), "partial upload temp file >= 1024 bytes", || {
            let tmp_path = tmp_path.clone();
            async move {
                Ok(std::fs::metadata(&tmp_path).ok().filter(|m| m.len() >= 1024).map(|_| ()))
            }
        })
        .await?;

        set_app("notes", false).await?;
        set_app("org.dar.prayer-times", false).await?;

        // Forced runtime SIGKILL + full database server restart + in-place module republish.
        runtime.kill_now().await?;
        drop(stream);

        db_server.restart(&stack.artifacts).await?;
        db_server.publish(&stack.artifacts, db_name, &origin)?;
        runtime = Instance::start(make_config()).await?;

        assert_eq!(session.get("/api/v1/notes/items").await?.status, 404);
        assert_eq!(session.get(app_endpoint).await?.status, 404);
        set_app("notes", true).await?;
        set_app("org.dar.prayer-times", true).await?;

        let keys_after: Value = session::client().get(format!("{origin}/oidc/jwks")).send().await?.json().await?;
        assert_eq!(keys_after, provider_keys);
        assert!(client.exchange(&pending_code).await?.id_token().is_some());
        let silent = authorize(&origin, &identity.session, &client, Authorize { prompt: Some("none".into()), jar: Some(pending_code.jar.clone()), ..Authorize::default() }).await?;
        assert_eq!(query_param(&silent.url, "error"), None, "{}", silent.url);
        assert!(client.exchange(&silent).await?.id_token().is_some());
        assert!(!client.refresh(active_tokens.refresh_token().context("active refresh token")?).await?.access_token().is_empty());

        let restored_dir = Reply::from(
            session::client().get(format!("{origin}/scim/v2/Users/{dir_user_id}")).bearer_auth(&scim_secret).send().await?,
        )
        .await?
        .expect(200)?;
        assert_eq!(restored_dir.json()?["id"], json!(dir_user_id));

        let uploads_entries: Vec<_> = std::fs::read_dir(root.join("uploads"))?.collect::<std::io::Result<Vec<_>>>()?;
        assert!(uploads_entries.is_empty(), "stale temp uploads must be reconciled on startup");

        assert_eq!(session.get(&format!("/api/v1/files/{}/content", durable.file_id)).await?.expect(200)?.text(), "Committed content survives process failure.");

        let dav_get = Reply::from(session::client().get(&dav_file).header("authorization", &dav_basic).send().await?).await?.expect(200)?;
        assert_eq!(dav_get.text(), "Written by a sync client.");
        assert_eq!(dav_get.header("etag"), Some(dav_etag.as_str()));

        let history_after = session.get(&history_path).await?.expect(200)?.text();
        assert!(history_after.contains("Durable start"));

        let repo_alias = session.get_json(&format!("/api/v1/git/repositories/{repo_folder}")).await?["clone"]["alias"].as_str().context("repo alias")?.to_owned();
        let clone_root = t.temp("dar-restart-clone")?;
        let cloned = Command::new("git")
            .args(["-c", &format!("http.extraHeader=Authorization: {dav_basic}"), "clone", "-q", &repo_alias, "repo"])
            .current_dir(&clone_root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &clone_root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .output()?;
        assert!(cloned.status.success(), "git clone failed: {}", String::from_utf8_lossy(&cloned.stderr));
        assert_eq!(dar_acceptance::read_to_string(clone_root.join("repo/README.md"))?, "# Durable repository\n");

        let book_after = session.get_json(&format!("/api/v1/books/{}", book.file_id)).await?["state"].clone();
        assert_eq!(book_after["position"], json!({ "spine": 1, "fraction": 0.5 }));
        assert_eq!(book_after["status"], json!("reading"));

        let photo_after = session.get_json(&format!("/api/v1/photos/assets/{}", photo.file_id)).await?;
        assert_eq!(photo_after["description"], json!("Durable photo"));
        assert_eq!(session.get(&format!("/api/v1/photos/assets/{}/thumbnail", photo.file_id)).await?.status, 200);

        let propfind = session::client()
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &dav_url)
            .header("authorization", &dav_basic)
            .header("depth", "1")
            .send()
            .await?;
        assert_eq!(propfind.status().as_u16(), 207);

        let retry = Reply::from(
            session.request("PUT", &format!("/api/v1/files/uploads/{res_id}")).body(vec![b'r'; 4096]).send().await?,
        )
        .await?;
        assert_eq!(retry.status, 200, "{}", retry.text());
        let repeated = Reply::from(
            session.request("PUT", &format!("/api/v1/files/uploads/{res_id}")).body(vec![b's'; 4096]).send().await?,
        )
        .await?;
        assert_eq!(repeated.status, 200);
        assert_eq!(session.get(&format!("/api/v1/files/{res_file_id}/content")).await?.expect(200)?.text(), "r".repeat(4096));

        {
            let live = session.connect().await?;
            let found = live.db.my_files().iter().any(|entry| entry.id == durable.file_id);
            live.disconnect();
            assert!(found);
        }

        // Wait until the 60-minute reminder is due, then deliver twice and assert single delivery.
        let due_at = reminder_start_dt - chrono::Duration::hours(1);
        let wait_ms = (due_at - chrono::Utc::now()).num_milliseconds() + 50;
        if wait_ms > 0 {
            tokio::time::sleep(Duration::from_millis(wait_ms as u64)).await;
        }
        support::deliver_reminders(&owner)?;
        support::deliver_reminders(&owner)?;
        let inbox = session.get_json("/api/v1/pim/reminders").await?;
        let items = inbox["items"].as_array().context("reminder items")?;
        assert_eq!(items.iter().filter(|r| r["resourceId"] == cal_event_id.as_str()).count(), 1);
        assert_eq!(items.iter().filter(|r| r["resourceId"] == task_id.as_str()).count(), 1);

        let recovered_note = session.get_json(&format!("/api/v1/notes/items/{note_id}")).await?;
        assert_eq!(recovered_note["content"], json!(exact_markdown));
        assert_eq!(recovered_note["uid"], json!(note_uid));
        assert_eq!(recovered_note["data"]["favorite"], json!(true));
        let recovered_trash = session.get_json("/api/v1/notes/trash").await?;
        assert!(recovered_trash["items"].as_array().is_some_and(|a| a.iter().any(|r| r["id"] == trashed_id.as_str())));

        let recovered_task = session.get_json(&format!("/api/v1/tasks/items/{task_id}")).await?;
        assert_eq!(recovered_task["content"], json!(task_content));
        assert_eq!(recovered_task["uid"], json!(task_uid));

        let recovered_agent = session.get_json(&format!("/api/v1/agent/threads/{agent_thread_id}/messages")).await?;
        assert_eq!(recovered_agent["thread"]["projectId"], json!(agent_project_id));
        assert_eq!(recovered_agent["items"][0]["id"], json!(agent_msg_id));
        assert_eq!(recovered_agent["items"][0]["content"], json!(agent_msg_content));

        assert_eq!(session.get_json(app_endpoint).await?["value"]["location"], json!("private restart proof"));

        // Offline app disable -> 404; uninstall + reinstall -> userData survives.
        runtime.close().await;
        assert!(!root.join("runtime.lock").exists());
        support::manage_apps(&root, "apps-disable", Some("org.dar.prayer-times"))?;
        runtime = Instance::start(make_config()).await?;
        assert_eq!(session.get(app_endpoint).await?.status, 404);

        runtime.close().await;
        support::manage_apps(&root, "apps-uninstall", Some("org.dar.prayer-times"))?;
        support::manage_apps(&root, "apps-install", Some(&stack.artifacts.prayer_times.to_string_lossy()))?;
        runtime = Instance::start(make_config()).await?;
        assert_eq!(session.get_json(app_endpoint).await?["value"]["location"], json!("private restart proof"));

        Ok::<(), Error>(())
    }
    .await;

    runtime.close().await;
    db_server.stop();
    result
}
