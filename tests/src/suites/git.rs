//! Git over HTTPS (`git.integration.test.ts`): clone, fetch and push a tracked folder,
//! protocol v0/v2, modes, symlinks, renames, cache rebuild, branches, tags, refusals, merges,
//! shallow clones and bundles.

use crate::suites::agent_git_support::{create_app_password, run_git};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "git",
    cases: &[case!(git_https_clone_fetch_and_push, 240)],
};

pub async fn git_https_clone_fetch_and_push(t: &Ctx) -> Result<()> {
    let fixture = t.fixture().await?;
    let base = fixture.base.clone();
    let session = login(&base).await?;
    let db = session.connect().await?;
    let work = t.temp("dar-git")?;

    let history = |folder_id: &str, rest: &str| format!("/api/v1/files/history/{folder_id}{rest}");
    let overview = |folder_id: &str| {
        let path = history(folder_id, "");
        let session = session.clone();
        async move { session.get_json(&path).await }
    };
    let used = || -> u64 {
        db.db
            .my_usage()
            .iter()
            .next()
            .map(|u| u.used as u64)
            .unwrap_or(0)
    };
    let live = |file_id: &str| {
        let path = format!("/api/v1/files/{file_id}/content");
        let session = session.clone();
        async move { Ok::<String, Error>(session.get(&path).await?.expect(200)?.text()) }
    };
    let run = |cwd: &std::path::Path, secret: &str, args: &[&str], ok: bool| {
        run_git(
            cwd,
            &work,
            "Ada Lovelace",
            "ada@example.com",
            "admin",
            secret,
            args,
            ok,
            None,
        )
    };

    let folder_id = uuid();
    let docs_id = uuid();
    let url = format!("{base}/dav/git/{folder_id}.git");
    reducer(|done| {
        db.reducers.create_folder_then(
            folder_id.clone(),
            String::new(),
            format!("Git {}", &uuid()[..6]),
            done,
        )
    })
    .await?;
    reducer(|done| {
        db.reducers
            .create_folder_then(docs_id.clone(), folder_id.clone(), "docs".into(), done)
    })
    .await?;
    let readme = upload_with(
        &session,
        "README.md",
        "# Project\n",
        Upload {
            parent_id: &folder_id,
            ..Upload::default()
        },
    )
    .await?;
    let notes = upload_with(
        &session,
        "a.txt",
        "alpha\n",
        Upload {
            parent_id: &docs_id,
            ..Upload::default()
        },
    )
    .await?;
    session
        .post(&history(&folder_id, ""), &json!({ "action": "track" }))
        .await?
        .expect(200)?;
    let mut state = overview(&folder_id).await?;
    session
        .post(
            &history(&folder_id, "/commits"),
            &json!({ "message": "Initial", "token": state["status"]["token"] }),
        )
        .await?
        .expect(201)?;
    let write_secret = create_app_password(&session, "Git write", "write").await?;
    let read_secret = create_app_password(&session, "Git read", "read").await?;

    // Git signs in with Basic authentication: no credentials asks for them.
    let anonymous = session::client()
        .get(format!("{url}/info/refs?service=git-upload-pack"))
        .send()
        .await?;
    assert_eq!(anonymous.status().as_u16(), 401);
    let www_auth = anonymous
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        www_auth.starts_with("Basic "),
        "expected Basic auth challenge, got {www_auth:?}"
    );
    let basic_read = format!(
        "Basic {}",
        dar_acceptance::base64(format!("admin:{read_secret}").as_bytes())
    );
    let dumb = session::client()
        .get(format!("{url}/info/refs"))
        .header("authorization", &basic_read)
        .send()
        .await?;
    assert_eq!(dumb.status().as_u16(), 403, "dumb HTTP is refused");

    // Clone: Dar's commit becomes a Git commit, the same one every time.
    run(&work, &read_secret, &["clone", "-q", &url, "one"], true)?;
    let one = work.join("one");
    assert_eq!(
        std::fs::read_to_string(one.join("README.md"))?,
        "# Project\n"
    );
    assert_eq!(
        run(&one, &read_secret, &["log", "-1", "--format=%s|%an"], true)?.out,
        "Initial|Test Administrator"
    );
    let initial = run(&one, &read_secret, &["rev-parse", "HEAD"], true)?.out;
    run(
        &work,
        &read_secret,
        &["-c", "protocol.version=0", "clone", "-q", &url, "two"],
        true,
    )?;
    let two = work.join("two");
    assert_eq!(
        run(&two, &read_secret, &["rev-parse", "HEAD"], true)?.out,
        initial,
        "protocol v0 clones the same commit"
    );
    state = overview(&folder_id).await?;
    assert_eq!(
        state["changesets"][0]["sha"].as_str(),
        Some(initial.as_str()),
        "the History tab shows the Git ID"
    );

    // Push: modes, a symbolic link and a rename reach Dar; the folder changes with them.
    std::fs::write(one.join("README.md"), "# Project\n\nFrom Git\n")?;
    std::fs::write(one.join("run.sh"), "#!/bin/sh\necho hi\n")?;
    std::fs::set_permissions(one.join("run.sh"), std::fs::Permissions::from_mode(0o755))?;
    std::os::unix::fs::symlink("README.md", one.join("link"))?;
    run(
        &one,
        &write_secret,
        &["mv", "docs/a.txt", "docs/b.txt"],
        true,
    )?;
    run(&one, &write_secret, &["add", "-A"], true)?;
    run(
        &one,
        &write_secret,
        &["commit", "-q", "-m", "Edit from Git\n\nWith a body."],
        true,
    )?;
    let before = used();
    let pushed = run(&one, &write_secret, &["push", "origin", "main"], true)?;
    assert!(
        pushed.err.contains("the folder now matches main"),
        "stderr: {}",
        pushed.err
    );
    let edited = run(&one, &write_secret, &["rev-parse", "HEAD"], true)?.out;
    assert_eq!(live(&readme.file_id).await?, "# Project\n\nFrom Git\n");
    state = overview(&folder_id).await?;
    assert_eq!(
        state["changesets"][0]["sha"].as_str(),
        Some(edited.as_str())
    );
    assert_eq!(state["changesets"][0]["author"], json!("Ada Lovelace"));
    assert_eq!(
        state["changesets"][0]["message"],
        json!("Edit from Git\n\nWith a body.")
    );
    assert_eq!(
        state["status"]["changes"].as_array().map(Vec::len),
        Some(0),
        "the folder matches main after a push"
    );
    let top_id = state["changesets"][0]["id"].as_str().context("top id")?;
    let detail = session
        .get_json(&history(&folder_id, &format!("/commits/{top_id}")))
        .await?;
    let changes: Vec<(&str, &str)> = detail["changes"]
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
        vec![
            ("modified", "README.md"),
            ("renamed", "docs/b.txt"),
            ("added", "link"),
            ("added", "run.sh")
        ]
    );
    assert_eq!(
        detail["changes"][1]["fileId"].as_str(),
        Some(notes.file_id.as_str()),
        "a renamed file keeps its ID"
    );
    assert_eq!(live(&notes.file_id).await?, "alpha\n");
    eventually(
        Duration::from_secs(5),
        "pushed content counts against the quota",
        || async { Ok((used() > before).then_some(())) },
    )
    .await?;
    run(&one, &read_secret, &["push", "origin", "main"], false)?;

    // A commit made in Dar arrives with a fetch; modes and the link survive the round trip.
    upload_with(
        &session,
        "README.md",
        "# Project\n\nFrom Dar\n",
        Upload {
            parent_id: &folder_id,
            file_id: Some(&readme.file_id),
            expected_revision: Some(2),
            ..Upload::default()
        },
    )
    .await?;
    state = overview(&folder_id).await?;
    session
        .post(
            &history(&folder_id, "/commits"),
            &json!({ "message": "Edit in Dar", "token": state["status"]["token"] }),
        )
        .await?
        .expect(201)?;
    run(&two, &read_secret, &["pull", "-q", "--ff-only"], true)?;
    assert_eq!(
        run(&two, &read_secret, &["log", "-1", "--format=%s"], true)?.out,
        "Edit in Dar"
    );
    assert_eq!(
        run(&two, &read_secret, &["rev-parse", "HEAD~1"], true)?.out,
        edited
    );
    let ls_tree = run(
        &two,
        &read_secret,
        &["ls-tree", "HEAD", "run.sh", "link"],
        true,
    )?
    .out;
    let ls_re = regex::Regex::new(r"^120000 blob .*\tlink\n100755 blob .*\trun\.sh$").unwrap();
    assert!(ls_re.is_match(&ls_tree), "unexpected ls-tree: {ls_tree:?}");
    assert_eq!(
        std::fs::read_link(two.join("link"))?,
        std::path::PathBuf::from("README.md")
    );
    assert_eq!(
        std::fs::metadata(two.join("run.sh"))?.permissions().mode() & 0o111,
        0o111
    );
    let from_dar = run(&two, &read_secret, &["rev-parse", "HEAD"], true)?.out;

    // The repository is only a cache: rebuilt from the database, every ID is the same.
    let cache_dir = fixture.root().join("git").join(format!("{folder_id}.git"));
    if cache_dir.exists() {
        std::fs::remove_dir_all(&cache_dir)?;
    }
    run(
        &work,
        &read_secret,
        &["clone", "-q", "--mirror", &url, "three.git"],
        true,
    )?;
    let three = work.join("three.git");
    assert_eq!(
        run(&three, &read_secret, &["rev-parse", "main"], true)?.out,
        from_dar
    );
    run(
        &three,
        &read_secret,
        &["fsck", "--strict", "--no-dangling"],
        true,
    )?;

    // Branches and tags are kept without touching the folder; deleting one releases its content.
    run(&one, &write_secret, &["pull", "-q", "--ff-only"], true)?;
    run(
        &one,
        &write_secret,
        &["checkout", "-q", "-b", "feature"],
        true,
    )?;
    std::fs::write(one.join("feature.bin"), vec![7u8; 4096])?;
    run(&one, &write_secret, &["add", "-A"], true)?;
    run(
        &one,
        &write_secret,
        &["commit", "-q", "-m", "Feature work"],
        true,
    )?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let quota = used();
    run(
        &one,
        &write_secret,
        &["push", "-q", "origin", "feature"],
        true,
    )?;
    eventually(Duration::from_secs(5), "branch content counts", || async {
        Ok((used() >= quota + 4096).then_some(()))
    })
    .await?;
    run(
        &one,
        &write_secret,
        &["tag", "-a", "v1", "-m", "Version 1", "main"],
        true,
    )?;
    run(&one, &write_secret, &["push", "-q", "origin", "v1"], true)?;
    state = overview(&folder_id).await?;
    let ref_names: Vec<&str> = state["gitRefs"]
        .as_array()
        .context("gitRefs")?
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert_eq!(ref_names, vec!["refs/heads/feature", "refs/tags/v1"]);
    assert_eq!(state["status"]["changes"].as_array().map(Vec::len), Some(0));
    let remote = run(&two, &read_secret, &["ls-remote", &url], true)?.out;
    assert!(remote.contains("refs/heads/feature"));
    assert!(remote.contains("refs/tags/v1^{}"));
    run(
        &one,
        &write_secret,
        &["push", "-q", "origin", "--delete", "feature"],
        true,
    )?;
    eventually(
        Duration::from_secs(5),
        "a deleted branch's content is released",
        || async { Ok((used() <= quota).then_some(())) },
    )
    .await?;
    run(
        &one,
        &write_secret,
        &["push", "origin", "--delete", "main"],
        false,
    )?;

    // Like Git: a stale main is refused, and so is a push over uncommitted Dar changes.
    run(&one, &write_secret, &["checkout", "-q", "main"], true)?;
    std::fs::write(two.join("stale.txt"), "stale\n")?;
    run(&two, &write_secret, &["add", "-A"], true)?;
    run(&two, &write_secret, &["commit", "-q", "-m", "Stale"], true)?;
    run(
        &one,
        &write_secret,
        &["commit", "-q", "--allow-empty", "-m", "Ahead"],
        true,
    )?;
    run(&one, &write_secret, &["push", "-q", "origin", "main"], true)?;
    let stale_push = run(&two, &write_secret, &["push", "origin", "main"], false)?;
    assert!(
        stale_push.err.contains("rejected"),
        "stderr: {}",
        stale_push.err
    );
    upload_with(
        &session,
        "README.md",
        "# Dirty in Dar\n",
        Upload {
            parent_id: &folder_id,
            file_id: Some(&readme.file_id),
            expected_revision: Some(3),
            ..Upload::default()
        },
    )
    .await?;
    std::fs::write(one.join("README.md"), "# Conflicting\n")?;
    run(&one, &write_secret, &["commit", "-qam", "Conflict"], true)?;
    let conflict_push = run(&one, &write_secret, &["push", "origin", "main"], false)?;
    assert!(
        conflict_push.err.contains("uncommitted changes in Dar"),
        "stderr: {}",
        conflict_push.err
    );
    run(
        &one,
        &write_secret,
        &["reset", "-q", "--hard", "origin/main"],
        true,
    )?;
    std::fs::write(one.join("docs").join("b.txt"), "beta\n")?;
    run(&one, &write_secret, &["commit", "-qam", "Elsewhere"], true)?;
    run(&one, &write_secret, &["push", "-q", "origin", "main"], true)?;
    state = overview(&folder_id).await?;
    let dirty: Vec<(&str, &str)> = state["status"]["changes"]
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
        dirty,
        vec![("modified", "README.md")],
        "Dar's own edit stays uncommitted"
    );
    assert_eq!(live(&notes.file_id).await?, "beta\n");

    // A merge commit, and a shallow clone.
    run(&one, &write_secret, &["checkout", "-q", "-b", "side"], true)?;
    std::fs::write(one.join("side.txt"), "side\n")?;
    run(&one, &write_secret, &["add", "-A"], true)?;
    run(&one, &write_secret, &["commit", "-q", "-m", "Side"], true)?;
    run(&one, &write_secret, &["checkout", "-q", "main"], true)?;
    std::fs::write(one.join("main.txt"), "main\n")?;
    run(&one, &write_secret, &["add", "-A"], true)?;
    run(&one, &write_secret, &["commit", "-q", "-m", "Main"], true)?;
    run(
        &one,
        &write_secret,
        &["merge", "-q", "--no-ff", "-m", "Merge side", "side"],
        true,
    )?;
    run(&one, &write_secret, &["push", "-q", "origin", "main"], true)?;
    state = overview(&folder_id).await?;
    assert_eq!(state["changesets"][0]["kind"], json!("merge"));
    let merge_head = run(&one, &read_secret, &["rev-parse", "HEAD"], true)?.out;
    assert_eq!(
        state["changesets"][0]["sha"].as_str(),
        Some(merge_head.as_str())
    );
    run(
        &work,
        &read_secret,
        &["clone", "-q", "--depth", "1", &url, "shallow"],
        true,
    )?;
    assert_eq!(
        run(
            &work.join("shallow"),
            &read_secret,
            &["rev-parse", "HEAD"],
            true
        )?
        .out,
        merge_head
    );

    // The History tab's bundle has the same commits as a clone.
    let bundle = session
        .get(&history(&folder_id, "/bundle"))
        .await?
        .expect(200)?;
    std::fs::write(work.join("history.bundle"), &bundle.body)?;
    run(
        &work,
        &read_secret,
        &[
            "clone",
            "-q",
            work.join("history.bundle").to_str().unwrap(),
            "four",
        ],
        true,
    )?;
    assert_eq!(
        run(
            &work.join("four"),
            &read_secret,
            &["rev-parse", "HEAD"],
            true
        )?
        .out,
        merge_head
    );
    Ok(())
}
