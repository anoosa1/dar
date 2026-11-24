//! Port of `test/agent.integration.test.ts`.

use crate::suites::agent_git_support::{create_user, encode_uri_component};
use dar_acceptance::prelude::*;
use dar_acceptance::session::{Reply, client};
use dar_acceptance::stack::stack;
use dar_acceptance::support::{self, Database};
use dar_bindings::create_folder_reducer::create_folder;

pub const SUITE: Suite = Suite {
    name: "agent",
    cases: &[case!(agent_sources_and_lifecycle)],
};

async fn post_agent(
    session: &Session,
    route: &str,
    value: &Value,
    if_match: Option<&str>,
) -> Result<Reply> {
    let mut req = session
        .request("POST", &format!("/api/v1/agent{route}"))
        .json(value);
    if let Some(etag) = if_match {
        req = req.header("if-match", etag);
    }
    Reply::from(req.send().await?).await
}

fn publish_fresh_db(origin: &str) -> Result<Database> {
    let name = format!("dar-agent-restore-{}", &uuid()[..8]);
    dar_acceptance::instance::publish_module(&name, origin)?;
    let st = stack();
    Database::new(&st.database.url, &name, &st.database.token_file)
}

async fn agent_sources_and_lifecycle(ctx: &Ctx) -> Result<()> {
    let instance = ctx.fixture().await?;
    let base = instance.base.clone();
    let admin = login(&base).await?;
    let db = instance.database()?;

    let project_id = uuid();
    let project_name = format!("Source proof {}", uuid());
    let project = json!({
        "id": project_id,
        "name": project_name,
        "sourceType": "git",
        "sourceRef": "https://example.test/you/project.git"
    });

    ensure!(
        post_agent(&admin, "/projects", &project, None)
            .await?
            .status
            == 201
    );
    ensure!(
        post_agent(&admin, "/projects", &project, None)
            .await?
            .status
            == 201
    );

    let mut bad_cred = project.clone();
    bad_cred["id"] = json!(uuid());
    bad_cred["sourceRef"] = json!("https://secret@example.test/repo.git");
    ensure!(
        post_agent(&admin, "/projects", &bad_cred, None)
            .await?
            .status
            == 400
    );

    let mut bad_host = project.clone();
    bad_host["id"] = json!(uuid());
    bad_host["sourceType"] = json!("host");
    bad_host["sourceRef"] = json!("/var/lib/dar");
    ensure!(
        post_agent(&admin, "/projects", &bad_host, None)
            .await?
            .status
            == 400
    );

    let mut bad_owner = project.clone();
    bad_owner["id"] = json!(uuid());
    bad_owner["ownerId"] = json!(uuid());
    ensure!(
        post_agent(&admin, "/projects", &bad_owner, None)
            .await?
            .status
            == 400
    );

    let unauth = Reply::from(
        client()
            .get(format!("{base}/api/v1/agent/projects"))
            .send()
            .await?,
    )
    .await?;
    ensure!(unauth.status == 401);

    let no_csrf = Reply::from(
        client()
            .post(format!("{base}/api/v1/agent/projects"))
            .header("cookie", &admin.cookie)
            .header("origin", &base)
            .json(&project)
            .send()
            .await?,
    )
    .await?;
    ensure!(no_csrf.status == 403);

    let folder_id = uuid();
    let folder_name = format!("Agent files {}", uuid());
    let conn = admin.connect().await?;
    let create_res = live::reducer(|done| {
        conn.reducers.create_folder_then(
            folder_id.clone(),
            String::new(),
            folder_name.clone(),
            done,
        )
    })
    .await;
    conn.disconnect();
    create_res?;

    let files_project = json!({
        "id": uuid(),
        "name": "Files project",
        "sourceType": "files",
        "sourceRef": folder_id
    });
    ensure!(
        post_agent(&admin, "/projects", &files_project, None)
            .await?
            .status
            == 201
    );

    let folders = admin.get_json("/api/v1/agent/folders").await?;
    ensure!(
        folders["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|f| f["id"] == folder_id)),
        "expected folder {folder_id} in agent folders: {folders}"
    );

    let username = format!("agent-{}", &uuid()[..8]);
    create_user(&admin, &username, "Agent isolation", PASSWORD).await?;
    let other = login_as(&base, &username, PASSWORD).await?;

    let other_projects = other.get_json("/api/v1/agent/projects").await?;
    ensure!(
        other_projects["items"] == json!([]),
        "expected empty projects for other user: {other_projects}"
    );
    ensure!(
        other
            .get(&format!("/api/v1/agent/folders?parentId={folder_id}"))
            .await?
            .status
            == 404
    );

    let picker = admin
        .get_json(&format!("/api/v1/files/picker?parentId={folder_id}"))
        .await?;
    ensure!(
        picker["folder"]["id"] == folder_id,
        "picker folder mismatch: {picker}"
    );
    ensure!(
        other
            .get(&format!("/api/v1/files/picker?parentId={folder_id}"))
            .await?
            .status
            == 404
    );
    ensure!(admin.get("/api/v1/files/picker?kind=invalid").await?.status == 400);

    let auto_id = uuid();
    ensure!(
        post_agent(
            &admin,
            "/threads",
            &json!({ "id": auto_id, "projectId": project_id }),
            None
        )
        .await?
        .status
            == 201
    );
    let auto_reply = post_agent(
        &admin,
        &format!("/threads/{auto_id}/messages"),
        &json!({ "id": uuid(), "content": "Fix the project chooser\nKeep it keyboard accessible." }),
        Some("\"1\""),
    )
    .await?
    .expect(201)?;
    let expected_auto_title = &"Fix the project chooser Keep it keyboard accessible."[..50];
    ensure!(auto_reply.json()?["thread"]["title"] == expected_auto_title);
    ensure!(
        post_agent(
            &admin,
            "/threads",
            &json!({ "id": auto_id, "projectId": project_id }),
            None
        )
        .await?
        .status
            == 201
    );

    let unicode_id = uuid();
    ensure!(
        post_agent(
            &admin,
            "/threads",
            &json!({ "id": unicode_id, "projectId": project_id }),
            None
        )
        .await?
        .status
            == 201
    );
    let unicode_reply = post_agent(
        &admin,
        &format!("/threads/{unicode_id}/messages"),
        &json!({ "id": uuid(), "content": "🌸".repeat(80) }),
        Some("\"1\""),
    )
    .await?
    .expect(201)?;
    let unicode_title = unicode_reply.json()?["thread"]["title"]
        .as_str()
        .context("unicode thread title")?
        .to_owned();
    ensure!(
        unicode_title.len() == 200,
        "expected 200 UTF-8 bytes, got {}",
        unicode_title.len()
    );

    let other_threads = other.get_json("/api/v1/agent/threads").await?;
    ensure!(other_threads["items"] == json!([]));

    let mut other_files_project = files_project.clone();
    other_files_project["id"] = json!(uuid());
    ensure!(
        post_agent(&other, "/projects", &other_files_project, None)
            .await?
            .status
            == 404
    );

    let thread_id = uuid();
    post_agent(
        &admin,
        "/threads",
        &json!({ "id": thread_id, "projectId": project_id, "title": "Revision proof" }),
        None,
    )
    .await?
    .expect(201)?;
    ensure!(
        other
            .get(&format!("/api/v1/agent/threads?projectId={project_id}"))
            .await?
            .status
            == 404
    );
    ensure!(
        other
            .get(&format!("/api/v1/agent/threads/{thread_id}/messages"))
            .await?
            .status
            == 404
    );

    let message = json!({
        "id": uuid(),
        "content": "Preserve exact Markdown.\n\n<script>untrusted</script>"
    });
    let route = format!("/threads/{thread_id}/messages");
    ensure!(post_agent(&admin, &route, &message, None).await?.status == 428);

    let competing = json!({ "id": uuid(), "content": "Competing write" });
    let (w1, w2) = tokio::join!(
        post_agent(&admin, &route, &message, Some("\"1\"")),
        post_agent(&admin, &route, &competing, Some("\"1\""))
    );
    let mut statuses = vec![w1?.status, w2?.status];
    statuses.sort();
    ensure!(
        statuses == vec![201, 409],
        "expected [201, 409], got {statuses:?}"
    );

    let current = admin.get_json(&format!("/api/v1/agent{route}")).await?;
    let current_items = current["items"].as_array().context("current items")?;
    ensure!(
        current_items.len() == 1,
        "expected 1 item, got {}",
        current_items.len()
    );
    let thread = current["thread"].clone();
    let saved = current_items[0].clone();
    let saved_id = saved["id"].as_str().context("saved id")?.to_owned();
    let saved_content = saved["content"]
        .as_str()
        .context("saved content")?
        .to_owned();

    let replay = post_agent(
        &admin,
        &route,
        &json!({ "id": saved_id, "content": saved_content }),
        Some("\"1\""),
    )
    .await?
    .expect(201)?;
    ensure!(replay.json()?["message"]["id"] == saved_id);
    ensure!(
        post_agent(
            &admin,
            &route,
            &json!({ "id": saved_id, "content": "Changed payload" }),
            Some("\"1\"")
        )
        .await?
        .status
            == 409
    );

    let rev = thread["revision"].as_u64().context("thread revision")?;
    ensure!(
        post_agent(
            &admin,
            &route,
            &json!({ "id": uuid(), "content": "x".repeat(16385) }),
            Some(&format!("\"{rev}\""))
        )
        .await?
        .status
            == 400
    );

    let mut current_rev = rev;
    for i in 0..52 {
        let reply = post_agent(
            &admin,
            &route,
            &json!({ "id": uuid(), "content": format!("History {i}") }),
            Some(&format!("\"{current_rev}\"")),
        )
        .await?
        .expect(201)?;
        current_rev = reply.json()?["thread"]["revision"]
            .as_u64()
            .context("updated thread revision")?;
    }

    let latest = admin.get_json(&format!("/api/v1/agent{route}")).await?;
    ensure!(latest["items"].as_array().map(|a| a.len()) == Some(50));
    let next_before = latest["nextBefore"]
        .as_u64()
        .context("expected nextBefore")?;

    let earlier = admin
        .get_json(&format!("/api/v1/agent{route}?before={next_before}"))
        .await?;
    let earlier_items = earlier["items"].as_array().context("earlier items")?;
    ensure!(
        earlier_items.len() == 3,
        "expected 3 earlier items, got {}",
        earlier_items.len()
    );
    ensure!(earlier["nextBefore"].is_null());
    ensure!(earlier_items[0]["id"] == saved_id);
    ensure!(
        admin
            .get(&format!("/api/v1/agent{route}?before=NaN"))
            .await?
            .status
            == 400
    );

    let escaped_thread = post_agent(
        &admin,
        "/threads",
        &json!({ "id": uuid(), "projectId": project_id, "title": "JSON expansion boundary" }),
        None,
    )
    .await?
    .expect(201)?
    .json()?;
    let escaped_id = escaped_thread["id"].as_str().context("escaped thread id")?;
    ensure!(
        post_agent(
            &admin,
            &format!("/threads/{escaped_id}/messages"),
            &json!({ "id": uuid(), "content": format!("{}{}", "\t".repeat(16383), "x") }),
            Some("\"1\"")
        )
        .await?
        .status
            == 201
    );

    let cookie_value = admin
        .cookie
        .split_once('=')
        .context("session cookie value")?
        .1;
    let sid = db
        .call(
            "service_session",
            json!([support::digest_token(cookie_value)]),
        )
        .await?["session"]["id"]
        .as_str()
        .context("session id")?
        .to_owned();
    let settings = db
        .call("service_app_settings", json!(["", "", false, 0]))
        .await?;
    let disabled = db
        .call(
            "service_app_settings",
            json!([sid, "agent", false, settings["revision"]]),
        )
        .await?;
    let disabled_check: Result<()> = async {
        ensure!(admin.get("/api/v1/agent/projects").await?.status == 404);
        let search = admin
            .get_json(&format!(
                "/api/v1/search?q={}",
                encode_uri_component(&project_name)
            ))
            .await?;
        let found = search["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|r| r["id"] == project_id));
        ensure!(
            !found,
            "disabled agent project should not appear in search: {search}"
        );
        Ok(())
    }
    .await;
    let reenable_res = db
        .call(
            "service_app_settings",
            json!([sid, "agent", true, disabled["revision"]]),
        )
        .await;
    disabled_check?;
    reenable_res?;

    let pin = uuid();
    let snapshot = db.call("service_backup", json!([pin])).await?;
    db.call("service_release_backup", json!([pin])).await?;
    ensure!(
        snapshot["format"] == "dar-backup-v12",
        "unexpected backup format: {}",
        snapshot["format"]
    );

    let restored = publish_fresh_db(&base)?;
    restored
        .call("service_restore", json!([snapshot.to_string()]))
        .await?;
    let restored_pin = uuid();
    let recovered = restored
        .call("service_backup", json!([restored_pin]))
        .await?;
    restored
        .call("service_release_backup", json!([restored_pin]))
        .await?;
    ensure!(
        recovered["agent"] == snapshot["agent"],
        "restored agent state mismatch"
    );

    let malformed = publish_fresh_db(&base)?;
    let mut bad_snapshot = snapshot.clone();
    bad_snapshot["agent"]["messages"][0]["ownerId"] = json!(uuid());
    let err = malformed
        .call("service_restore", json!([bad_snapshot.to_string()]))
        .await
        .expect_err("malformed restore should fail");
    ensure!(
        err.code == "invalidInput",
        "expected invalidInput, got {err:?}"
    );

    let legacy = publish_fresh_db(&base)?;
    let mut old_snapshot = snapshot.clone();
    old_snapshot
        .as_object_mut()
        .context("snapshot object")?
        .remove("agent");
    old_snapshot["format"] = json!("dar-backup-v5");
    legacy
        .call("service_restore", json!([old_snapshot.to_string()]))
        .await?;
    let old_pin = uuid();
    let old_recovered = legacy.call("service_backup", json!([old_pin])).await?;
    legacy
        .call("service_release_backup", json!([old_pin]))
        .await?;
    ensure!(
        old_recovered["agent"]
            == json!({ "projects": [], "threads": [], "messages": [], "runs": [], "shares": [] }),
        "legacy restore agent section mismatch: {}",
        old_recovered["agent"]
    );

    Ok(())
}
