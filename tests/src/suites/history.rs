//! Folder history (`history.integration.test.ts`): track, commit, rename, restore, retention,
//! quota, export, sharing and agent branches.

use crate::suites::agent_git_support::{create_user, run_git_local};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::process::{Command, Stdio};

pub const SUITE: Suite = Suite {
    name: "history",
    cases: &[case!(folder_history_end_to_end, 90)],
};

pub async fn folder_history_end_to_end(t: &Ctx) -> Result<()> {
    let fixture = t.fixture().await?;
    let base = fixture.base.clone();
    let session = login(&base).await?;
    let db = session.connect().await?;

    let route = |folder_id: &str, rest: &str| format!("/api/v1/files/history/{folder_id}{rest}");
    let overview = |folder_id: &str, who: &Session| {
        let path = route(folder_id, "");
        let who = who.clone();
        async move { who.get(&path).await?.expect(200)?.json() }
    };
    let used = || -> u64 {
        db.db
            .my_usage()
            .iter()
            .next()
            .map(|u| u.used as u64)
            .unwrap_or(0)
    };

    let folder_id = uuid();
    let child_id = uuid();
    reducer(|done| {
        db.reducers.create_folder_then(
            folder_id.clone(),
            String::new(),
            format!("History {}", uuid()),
            done,
        )
    })
    .await?;
    reducer(|done| {
        db.reducers
            .create_folder_then(child_id.clone(), folder_id.clone(), "docs".into(), done)
    })
    .await?;
    let readme = upload_with(
        &session,
        "README.md",
        "# One\n",
        Upload {
            parent_id: &folder_id,
            ..Upload::default()
        },
    )
    .await?;
    let notes = upload_with(
        &session,
        "notes.txt",
        "first\n",
        Upload {
            parent_id: &child_id,
            ..Upload::default()
        },
    )
    .await?;

    let mut state = overview(&folder_id, &session).await?;
    assert_eq!(state["tracked"], json!(false));
    assert_eq!(state["owner"], json!(true));
    session
        .post(&route(&folder_id, ""), &json!({ "action": "track" }))
        .await?
        .expect(200)?;
    assert_eq!(
        session
            .post(&route(&child_id, ""), &json!({ "action": "track" }))
            .await?
            .status,
        403,
        "a folder inside a tracked folder cannot be tracked"
    );
    state = overview(&folder_id, &session).await?;
    let changes: Vec<(&str, &str)> = state["status"]["changes"]
        .as_array()
        .context("changes")?
        .iter()
        .map(|c| {
            (
                c["kind"].as_str().unwrap_or(""),
                c["path"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        changes,
        vec![("added", "README.md"), ("added", "docs/notes.txt")]
    );

    let before = used();
    let token = state["status"]["token"]
        .as_str()
        .context("status token")?
        .to_owned();
    assert_eq!(
        session
            .post(
                &route(&folder_id, "/commits"),
                &json!({ "message": "", "token": token })
            )
            .await?
            .status,
        400,
        "a commit needs a message"
    );
    assert_eq!(
        session
            .post(
                &route(&folder_id, "/commits"),
                &json!({ "message": "Initial", "token": "0000000000000000" })
            )
            .await?
            .status,
        409,
        "a stale status is a conflict"
    );
    session
        .post(
            &route(&folder_id, "/commits"),
            &json!({ "message": "Initial", "token": token }),
        )
        .await?
        .expect(201)?;
    assert_eq!(used(), before, "committing adds no bytes");
    state = overview(&folder_id, &session).await?;
    assert_eq!(state["status"]["changes"].as_array().map(Vec::len), Some(0));
    assert_eq!(state["changesets"][0]["message"], json!("Initial"));
    assert_eq!(
        session
            .post(
                &route(&folder_id, "/commits"),
                &json!({ "snapshot": true, "token": state["status"]["token"] })
            )
            .await?
            .status,
        409,
        "nothing to commit"
    );

    // Modify, rename and add; commit only part of it.
    upload_with(
        &session,
        "README.md",
        "# Two\n",
        Upload {
            parent_id: &folder_id,
            file_id: Some(&readme.file_id),
            expected_revision: Some(1),
            ..Upload::default()
        },
    )
    .await?;
    reducer(|done| {
        db.reducers.move_entry_then(
            notes.file_id.clone(),
            folder_id.clone(),
            1,
            Some("notes.md".into()),
            done,
        )
    })
    .await?;
    upload_with(
        &session,
        "extra.txt",
        "extra\n",
        Upload {
            parent_id: &folder_id,
            ..Upload::default()
        },
    )
    .await?;
    state = overview(&folder_id, &session).await?;
    let changes: Vec<(&str, &str, &str)> = state["status"]["changes"]
        .as_array()
        .context("changes")?
        .iter()
        .map(|c| {
            (
                c["kind"].as_str().unwrap_or(""),
                c["path"].as_str().unwrap_or(""),
                c["previousPath"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        changes,
        vec![
            ("modified", "README.md", ""),
            ("added", "extra.txt", ""),
            ("renamed", "notes.md", "docs/notes.txt")
        ]
    );

    session.post(&route(&folder_id, "/commits"), &json!({ "message": "Edit readme", "token": state["status"]["token"], "paths": ["README.md", "notes.md"] })).await?.expect(201)?;
    state = overview(&folder_id, &session).await?;
    let remaining: Vec<&str> = state["status"]["changes"]
        .as_array()
        .context("changes")?
        .iter()
        .filter_map(|c| c["path"].as_str())
        .collect();
    assert_eq!(remaining, vec!["extra.txt"]);

    let top_id = state["changesets"][0]["id"]
        .as_str()
        .context("top changeset id")?;
    let detail = session
        .get_json(&route(&folder_id, &format!("/commits/{top_id}")))
        .await?;
    let modified = detail["changes"]
        .as_array()
        .context("detail changes")?
        .iter()
        .find(|c| c["kind"] == "modified")
        .context("modified change")?;
    let prev_ver = modified["previousVersionId"]
        .as_str()
        .context("previousVersionId")?;
    let cur_ver = modified["versionId"].as_str().context("versionId")?;
    assert_eq!(
        session
            .get(&route(&folder_id, &format!("/versions/{prev_ver}/content")))
            .await?
            .expect(200)?
            .text(),
        "# One\n"
    );
    assert_eq!(
        session
            .get(&route(&folder_id, &format!("/versions/{cur_ver}/content")))
            .await?
            .expect(200)?
            .text(),
        "# Two\n"
    );
    assert_eq!(
        session
            .get(&route(&folder_id, &format!("/versions/{}/content", uuid())))
            .await?
            .status,
        404
    );

    // Restore needs a clean folder, then brings back the first state and commits it.
    let initial_id = state["changesets"]
        .as_array()
        .and_then(|a| a.last())
        .and_then(|c| c["id"].as_str())
        .context("initial id")?
        .to_owned();
    assert_eq!(
        session
            .post(
                &route(&folder_id, "/restore"),
                &json!({ "id": initial_id, "revision": state["repo"]["revision"] })
            )
            .await?
            .status,
        409
    );
    session
        .post(
            &route(&folder_id, "/commits"),
            &json!({ "snapshot": true, "token": state["status"]["token"] }),
        )
        .await?
        .expect(201)?;
    state = overview(&folder_id, &session).await?;
    session
        .post(
            &route(&folder_id, "/restore"),
            &json!({ "id": initial_id, "revision": state["repo"]["revision"] }),
        )
        .await?
        .expect(201)?;
    state = overview(&folder_id, &session).await?;
    assert_eq!(state["changesets"][0]["kind"], json!("restore"));
    assert_eq!(state["status"]["changes"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        session
            .get(&format!("/api/v1/files/{}/content", readme.file_id))
            .await?
            .expect(200)?
            .text(),
        "# One\n"
    );
    reducer(|done| {
        db.reducers.browse_then(
            child_id.clone(),
            "files".into(),
            String::new(),
            String::new(),
            done,
        )
    })
    .await?;
    let back = db
        .db
        .my_files()
        .iter()
        .find(|row| row.name == "notes.txt" && row.parent_id == child_id)
        .context("restored notes.txt")?;
    assert_eq!(
        session
            .get(&format!("/api/v1/files/{}/content", back.id))
            .await?
            .expect(200)?
            .text(),
        "first\n"
    );

    // Deleting a file for good keeps the versions history uses, still counted.
    reducer(|done| {
        db.reducers.browse_then(
            folder_id.clone(),
            "files".into(),
            String::new(),
            String::new(),
            done,
        )
    })
    .await?;
    assert!(
        db.db
            .my_files()
            .iter()
            .find(|row| row.name == "extra.txt" && row.parent_id == folder_id)
            .is_none(),
        "restore moved the later file to deleted files"
    );
    reducer(|done| {
        db.reducers.browse_then(
            String::new(),
            "trash".into(),
            "extra.txt".into(),
            String::new(),
            done,
        )
    })
    .await?;
    let trashed = db
        .db
        .my_files()
        .iter()
        .find(|row| row.name == "extra.txt")
        .context("trashed extra.txt")?;
    let kept = used();
    reducer(|done| {
        db.reducers
            .delete_entry_then(trashed.id.clone(), trashed.revision, done)
    })
    .await?;
    assert_eq!(used(), kept);
    let snap_id = state["changesets"][1]["id"]
        .as_str()
        .context("snapshot id")?;
    let snapshot_detail = session
        .get_json(&route(&folder_id, &format!("/commits/{snap_id}")))
        .await?;
    let extra_ver = snapshot_detail["changes"]
        .as_array()
        .context("snapshot changes")?
        .iter()
        .find(|c| c["path"] == "extra.txt")
        .and_then(|c| c["versionId"].as_str())
        .context("extra versionId")?;
    assert_eq!(
        session
            .get(&route(
                &folder_id,
                &format!("/versions/{extra_ver}/content")
            ))
            .await?
            .expect(200)?
            .text(),
        "extra\n"
    );

    // The Git bundle clones into the same history.
    let bundle = session
        .get(&route(&folder_id, "/bundle"))
        .await?
        .expect(200)?;
    let work = t.temp("history-bundle")?;
    std::fs::write(work.join("history.bundle"), &bundle.body)?;
    run_git_local(&work, &work, &["clone", "-q", "history.bundle", "clone"])?;
    let log = run_git_local(&work.join("clone"), &work, &["log", "--format=%s"])?.out;
    let subjects: Vec<&str> = log.lines().collect();
    assert_eq!(
        subjects,
        vec!["Restore to “Initial”", "Snapshot", "Edit readme", "Initial"]
    );
    let show = Command::new("git")
        .current_dir(work.join("clone"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &work)
        .args(["show", "HEAD~1:extra.txt"])
        .stdin(Stdio::null())
        .output()?;
    assert!(show.status.success());
    assert_eq!(String::from_utf8_lossy(&show.stdout), "extra\n");

    // Readers see history but cannot change it.
    let username = format!("history-{}", &uuid()[..8]);
    create_user(&session, &username, "History reader", PASSWORD).await?;
    let reader = login_as(&base, &username, PASSWORD).await?;
    assert_eq!(reader.get(&route(&folder_id, "")).await?.status, 404);
    let folder = match db.db.my_files().iter().find(|row| row.id == folder_id) {
        Some(row) => row,
        None => {
            reducer(|done| {
                db.reducers.browse_then(
                    String::new(),
                    "files".into(),
                    String::new(),
                    String::new(),
                    done,
                )
            })
            .await?;
            db.db
                .my_files()
                .iter()
                .find(|row| row.id == folder_id)
                .context("folder row")?
        }
    };
    reducer(|done| {
        db.reducers.grant_access_then(
            uuid(),
            folder_id.clone(),
            username.clone(),
            "read".into(),
            folder.revision,
            done,
        )
    })
    .await?;
    let seen = overview(&folder_id, &reader).await?;
    assert_eq!(seen["canWrite"], json!(false));
    assert_eq!(seen["changesets"].as_array().map(Vec::len), Some(4));
    assert_eq!(
        reader
            .post(
                &route(&folder_id, "/commits"),
                &json!({ "snapshot": true, "token": seen["status"]["token"] })
            )
            .await?
            .status,
        403
    );

    // An Agent thread on the folder proposes a branch; merge applies and commits it.
    let project_id = uuid();
    let thread_id = uuid();
    session.post("/api/v1/agent/projects", &json!({ "id": project_id, "name": "History project", "sourceType": "files", "sourceRef": folder_id })).await?.expect(201)?;
    session
        .post(
            "/api/v1/agent/threads",
            &json!({ "id": thread_id, "projectId": project_id, "title": "Tidy notes" }),
        )
        .await?
        .expect(201)?;
    session
        .get(&format!("/api/v1/agent/threads/{thread_id}/branch"))
        .await?
        .expect(200)?;
    let me = db.db.me().iter().next().context("me row")?.id;
    let thread_dir = fixture
        .root()
        .join("agent-workspaces")
        .join(&me)
        .join(&thread_id);
    let current = overview(&folder_id, &session).await?;
    reducer(|done| {
        db.reducers.browse_then(
            folder_id.clone(),
            "files".into(),
            String::new(),
            String::new(),
            done,
        )
    })
    .await?;
    let readme_row = db
        .db
        .my_files()
        .iter()
        .find(|row| row.id == readme.file_id)
        .context("readme row")?;
    std::fs::create_dir_all(thread_dir.join("work").join("docs"))?;
    std::fs::write(thread_dir.join("work").join("README.md"), "# Agent\n")?;
    std::fs::write(
        thread_dir.join("work").join("docs").join("new.md"),
        "added by agent\n",
    )?;
    std::fs::write(
        thread_dir.join("work").join("docs").join("notes.txt"),
        "first\n",
    )?;
    std::fs::write(
        thread_dir.join("baseline.json"),
        json!({
            "source": {
                "project": { "id": project_id, "ownerId": me, "name": "History project", "sourceType": "files", "sourceRef": folder_id, "createdAt": 0 },
                "files": [
                    { "id": readme.file_id, "path": "README.md", "revision": readme_row.revision, "size": 6, "mediaType": "text/plain" },
                    { "id": back.id, "path": "docs/notes.txt", "revision": back.revision, "size": 6, "mediaType": "text/plain" }
                ]
            },
            "files": [
                { "path": "README.md", "content": dar_acceptance::base64(b"# One\n") },
                { "path": "docs/notes.txt", "content": dar_acceptance::base64(b"first\n") }
            ]
        })
        .to_string(),
    )?;
    let staged = session
        .post(
            &format!("/api/v1/agent/threads/{thread_id}/branch"),
            &json!({}),
        )
        .await?
        .expect(201)?
        .json()?;
    let branch = &staged["branch"];
    let branch_name = branch["name"].as_str().context("branch name")?.to_owned();
    let branch_changes: Vec<(&str, &str)> = branch["changes"]
        .as_array()
        .context("branch changes")?
        .iter()
        .map(|c| {
            (
                c["kind"].as_str().unwrap_or(""),
                c["path"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        branch_changes,
        vec![("modified", "README.md"), ("added", "docs/new.md")]
    );
    eventually(
        std::time::Duration::from_secs(5),
        "staged content counts toward quota",
        || async { Ok((used() > kept).then_some(())) },
    )
    .await?;
    state = overview(&folder_id, &session).await?;
    assert_eq!(state["branches"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        state["status"]["changes"].as_array().map(Vec::len),
        Some(0),
        "a branch leaves the folder unchanged"
    );
    assert_eq!(
        state["repo"]["revision"].as_u64(),
        Some(current["repo"]["revision"].as_u64().unwrap_or(0) + 1)
    );

    let merged = session
        .post(
            &route(&folder_id, "/branches/merge"),
            &json!({ "name": branch_name, "revision": state["repo"]["revision"] }),
        )
        .await?
        .expect(200)?
        .json()?;
    assert_eq!(merged["merged"], json!(true));
    assert_eq!(
        session
            .get(&format!("/api/v1/files/{}/content", readme.file_id))
            .await?
            .expect(200)?
            .text(),
        "# Agent\n"
    );
    state = overview(&folder_id, &session).await?;
    assert_eq!(state["branches"].as_array().map(Vec::len), Some(0));
    assert_eq!(state["changesets"][0]["kind"], json!("merge"));
    assert_eq!(state["status"]["changes"].as_array().map(Vec::len), Some(0));

    // A second proposal against a changed file conflicts; discarding releases its staged bytes.
    let before_second = used();
    session
        .post(
            &format!("/api/v1/agent/threads/{thread_id}/branch"),
            &json!({}),
        )
        .await?
        .expect(201)?;
    eventually(
        std::time::Duration::from_secs(5),
        "second branch staged content counts",
        || async { Ok((used() > before_second).then_some(())) },
    )
    .await?;
    state = overview(&folder_id, &session).await?;
    let conflict = session
        .post(
            &route(&folder_id, "/branches/merge"),
            &json!({ "name": branch_name, "revision": state["repo"]["revision"] }),
        )
        .await?
        .json()?;
    assert_eq!(conflict["merged"], json!(false));
    assert!(
        conflict["conflicts"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == "README.md"))
    );
    let with_branch = used();
    session
        .post(
            &route(&folder_id, "/branches/discard"),
            &json!({ "name": branch_name, "revision": state["repo"]["revision"] }),
        )
        .await?
        .expect(200)?;
    eventually(
        std::time::Duration::from_secs(5),
        "discarding branch releases staged bytes",
        || async { Ok((used() < with_branch).then_some(())) },
    )
    .await?;

    // Stopping history keeps live files' versions and releases what only history held.
    state = overview(&folder_id, &session).await?;
    let tracked_used = used();
    assert_eq!(
        reader
            .post(
                &route(&folder_id, ""),
                &json!({ "action": "untrack", "revision": state["repo"]["revision"] })
            )
            .await?
            .status,
        403
    );
    session
        .post(
            &route(&folder_id, ""),
            &json!({ "action": "untrack", "revision": state["repo"]["revision"] }),
        )
        .await?
        .expect(200)?;
    let expected_after_untrack = tracked_used - "extra\n".len() as u64;
    eventually(
        std::time::Duration::from_secs(5),
        "untracking releases history-only bytes",
        || async { Ok((used() == expected_after_untrack).then_some(())) },
    )
    .await?;
    assert_eq!(
        overview(&folder_id, &session).await?["tracked"],
        json!(false)
    );
    Ok(())
}
