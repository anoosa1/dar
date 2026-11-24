//! Git app API (`git-app.integration.test.ts`): repositories, trees, commits, branches, tags,
//! compare, archives, blobs, activity, aliases and ref deletion.

use crate::suites::agent_git_support::{create_app_password, encode_uri_component, run_git};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;

pub const SUITE: Suite = Suite {
    name: "git-app",
    cases: &[case!(git_app_repositories_trees_and_refs, 240)],
};

pub async fn git_app_repositories_trees_and_refs(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let session = login(&base).await?;
    let db = session.connect().await?;
    let work = t.temp("dar-git-app")?;

    let get_status = |route: &str, expected: u16| {
        let path = format!("/api/v1/git{route}");
        let session = session.clone();
        async move {
            let reply = session.get(&path).await?.expect(expected)?;
            if expected == 200 {
                reply.json()
            } else {
                Ok(Value::Null)
            }
        }
    };
    let history = |folder_id: &str, rest: &str| format!("/api/v1/files/history/{folder_id}{rest}");

    let folder_id = uuid();
    let projects_id = uuid();
    let name = format!("Site {}", &uuid()[..6]);
    let projects_name = format!("Projects {}", &uuid()[..6]);
    reducer(|done| {
        db.reducers.create_folder_then(
            projects_id.clone(),
            String::new(),
            projects_name.clone(),
            done,
        )
    })
    .await?;
    reducer(|done| {
        db.reducers
            .create_folder_then(folder_id.clone(), projects_id.clone(), name.clone(), done)
    })
    .await?;
    upload_with(
        &session,
        "README.md",
        "# Site\n\n[![badge](x.svg)](y) Personal site and **blog**.\n\nMore text.\n",
        Upload {
            parent_id: &folder_id,
            ..Upload::default()
        },
    )
    .await?;
    upload_with(
        &session,
        "index.html",
        "<h1>Hi</h1>\n",
        Upload {
            parent_id: &folder_id,
            ..Upload::default()
        },
    )
    .await?;
    session
        .post(&history(&folder_id, ""), &json!({ "action": "track" }))
        .await?
        .expect(200)?;
    let state = session.get_json(&history(&folder_id, "")).await?;
    session
        .post(
            &history(&folder_id, "/commits"),
            &json!({ "message": "Start the site", "token": state["status"]["token"] }),
        )
        .await?
        .expect(201)?;
    let secret = create_app_password(&session, "Git app", "write").await?;

    let run = |cwd: &std::path::Path, args: &[&str], ok: bool, user: &str| {
        run_git(
            cwd,
            &work,
            "Ada Lovelace",
            "ada@example.com",
            user,
            &secret,
            args,
            ok,
            None,
        )
    };

    // The list: description from the README, counts and both clone addresses.
    let repos = get_status("/repositories", 200).await?;
    let mut repo = repos
        .as_array()
        .context("repositories")?
        .iter()
        .find(|item| item["id"] == folder_id)
        .cloned()
        .context("the tracked folder is a repository")?;
    assert_eq!(repo["path"], json!(format!("{projects_name}/{name}")));
    assert_eq!(repo["description"], json!("Personal site and blog."));
    assert_eq!(
        [
            repo["commits"].as_u64(),
            repo["branches"].as_u64(),
            repo["tags"].as_u64(),
            repo["dirty"].as_i64().map(|v| v as u64)
        ],
        [Some(1), Some(1), Some(0), Some(0)]
    );
    assert_eq!(
        repo["clone"]["id"],
        json!(format!("{base}/dav/git/{folder_id}.git"))
    );
    let expected_alias = format!(
        "{base}/dav/git/admin/{}/{}.git",
        encode_uri_component(&projects_name),
        encode_uri_component(&name)
    );
    assert_eq!(repo["clone"]["alias"], json!(expected_alias));
    assert!(repo["size"].as_u64().unwrap_or(0) > 0);
    let week_sum: u64 = repo["weeks"]
        .as_array()
        .context("weeks")?
        .iter()
        .filter_map(Value::as_u64)
        .sum();
    assert_eq!(week_sum, 1);

    // Clone by alias; the alias belongs to its user only.
    let alias = repo["clone"]["alias"]
        .as_str()
        .context("clone alias")?
        .to_owned();
    run(&work, &["clone", "-q", &alias, "site"], true, "admin")?;
    let site = work.join("site");
    run(&work, &["clone", "-q", &alias, "other"], false, "someone")?;
    let missing_url = format!(
        "{base}/dav/git/admin/{}/missing.git",
        encode_uri_component(&projects_name)
    );
    run(
        &work,
        &["clone", "-q", &missing_url, "missing"],
        false,
        "admin",
    )?;

    // Work on main, a branch and a tag.
    std::fs::write(site.join("index.html"), "<h1>Hello</h1>\n<p>Welcome</p>\n")?;
    run(&site, &["commit", "-qam", "Greet visitors"], true, "admin")?;
    run(
        &site,
        &["tag", "-a", "v1", "-m", "First version"],
        true,
        "admin",
    )?;
    run(
        &site,
        &["checkout", "-q", "-b", "feature/rss"],
        true,
        "admin",
    )?;
    std::fs::write(site.join("rss.xml"), "<rss/>\n")?;
    run(&site, &["add", "-A"], true, "admin")?;
    run(&site, &["commit", "-qm", "Add a feed"], true, "admin")?;
    run(
        &site,
        &["push", "-q", "origin", "main", "feature/rss", "v1"],
        true,
        "admin",
    )?;
    run(&site, &["checkout", "-q", "main"], true, "admin")?;
    std::fs::write(site.join("about.html"), "about\n")?;
    run(&site, &["add", "-A"], true, "admin")?;
    run(&site, &["commit", "-qm", "Add about"], true, "admin")?;
    run(&site, &["push", "-q", "origin", "main"], true, "admin")?;
    let head = run(&site, &["rev-parse", "main"], true, "admin")?.out;
    let greet = run(&site, &["rev-parse", "main~1"], true, "admin")?.out;

    repo = get_status(&format!("/repositories/{folder_id}"), 200).await?;
    assert_eq!(
        [
            repo["commits"].as_u64(),
            repo["branches"].as_u64(),
            repo["tags"].as_u64()
        ],
        [Some(3), Some(2), Some(1)]
    );

    // Tree: entries with their newest commit, and the README.
    let tree = get_status(&format!("/repositories/{folder_id}/tree"), 200).await?;
    assert_eq!(tree["sha"].as_str(), Some(head.as_str()));
    let entries: Vec<(&str, Option<&str>)> = tree["entries"]
        .as_array()
        .context("entries")?
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap_or(""),
                e["last"]["subject"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        entries,
        vec![
            ("about.html", Some("Add about")),
            ("index.html", Some("Greet visitors")),
            ("README.md", Some("Start the site"))
        ]
    );
    assert!(
        tree["readme"]["text"]
            .as_str()
            .unwrap_or("")
            .contains("Personal site")
    );
    let branch_tree = get_status(
        &format!("/repositories/{folder_id}/tree?ref=feature%2Frss"),
        200,
    )
    .await?;
    assert!(
        branch_tree["entries"]
            .as_array()
            .is_some_and(|a| a.iter().any(|e| e["name"] == "rss.xml"))
    );
    get_status(&format!("/repositories/{folder_id}/tree?ref=nope"), 404).await?;
    get_status(&format!("/repositories/{folder_id}/tree?path=..%2Fx"), 400).await?;

    // Commits: where each came from, refs, line counts, and filters.
    let page = get_status(&format!("/repositories/{folder_id}/commits?ref=main"), 200).await?;
    let commit_pairs: Vec<(&str, &str)> = page["commits"]
        .as_array()
        .context("commits")?
        .iter()
        .map(|c| {
            (
                c["subject"].as_str().unwrap_or(""),
                c["source"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        commit_pairs,
        vec![
            ("Add about", "git"),
            ("Greet visitors", "git"),
            ("Start the site", "dar")
        ]
    );
    assert_eq!(
        page["commits"][0]["refs"],
        json!([{ "name": "main", "kind": "branch" }])
    );
    assert_eq!(
        page["commits"][1]["refs"],
        json!([{ "name": "v1", "kind": "tag" }])
    );
    assert_eq!(page["commits"][0]["added"], json!(1));
    assert_eq!(page["total"], json!(3));
    assert_eq!(page["next"], Value::Null);
    assert_eq!(
        page["authors"],
        json!(["Ada Lovelace", "Test Administrator"])
    );
    assert_eq!(
        get_status(&format!("/repositories/{folder_id}/commits"), 200).await?["total"],
        json!(4),
        "all branches"
    );
    assert_eq!(
        get_status(
            &format!("/repositories/{folder_id}/commits?source=dar"),
            200
        )
        .await?["commits"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        get_status(
            &format!("/repositories/{folder_id}/commits?author=Ada%20Lovelace&ref=main"),
            200
        )
        .await?["total"],
        json!(2)
    );
    assert_eq!(
        get_status(
            &format!("/repositories/{folder_id}/commits?ref=main&limit=2"),
            200
        )
        .await?["next"],
        json!(2)
    );

    let detail = get_status(&format!("/repositories/{folder_id}/commits/{greet}"), 200).await?;
    assert_eq!(detail["files"].as_array().map(Vec::len), Some(1));
    assert_eq!(detail["files"][0]["kind"], json!("modified"));
    assert_eq!(
        [
            detail["files"][0]["added"].as_u64(),
            detail["files"][0]["removed"].as_u64()
        ],
        [Some(2), Some(1)]
    );
    let patch = detail["files"][0]["patch"].as_str().context("patch")?;
    assert_eq!(
        patch,
        "@@ -1 +1,2 @@\n-<h1>Hi</h1>\n+<h1>Hello</h1>\n+<p>Welcome</p>"
    );
    get_status(
        &format!("/repositories/{folder_id}/commits/{}", "0".repeat(40)),
        404,
    )
    .await?;

    // Branches with divergence, tags, compare and downloads.
    let branches = get_status(&format!("/repositories/{folder_id}/branches"), 200).await?;
    assert_eq!(branches["main"]["sha"].as_str(), Some(head.as_str()));
    let branch_rows: Vec<(&str, u64, u64)> = branches["branches"]
        .as_array()
        .context("branches")?
        .iter()
        .map(|b| {
            (
                b["name"].as_str().unwrap_or(""),
                b["ahead"].as_u64().unwrap_or(0),
                b["behind"].as_u64().unwrap_or(0),
            )
        })
        .collect();
    assert_eq!(branch_rows, vec![("feature/rss", 1, 1)]);
    let tags = get_status(&format!("/repositories/{folder_id}/tags"), 200).await?;
    let tag_rows: Vec<(&str, &str, &str, bool)> = tags
        .as_array()
        .context("tags")?
        .iter()
        .map(|tag| {
            (
                tag["name"].as_str().unwrap_or(""),
                tag["target"].as_str().unwrap_or(""),
                tag["message"].as_str().unwrap_or(""),
                tag["annotated"].as_bool().unwrap_or(false),
            )
        })
        .collect();
    assert_eq!(
        tag_rows,
        vec![("v1", greet.as_str(), "First version", true)]
    );
    let compared = get_status(
        &format!("/repositories/{folder_id}/compare?base=main&head=feature%2Frss"),
        200,
    )
    .await?;
    let ahead_subjects: Vec<&str> = compared["ahead"]
        .as_array()
        .context("ahead")?
        .iter()
        .filter_map(|c| c["subject"].as_str())
        .collect();
    let file_paths: Vec<&str> = compared["files"]
        .as_array()
        .context("files")?
        .iter()
        .filter_map(|f| f["path"].as_str())
        .collect();
    assert_eq!(ahead_subjects, vec!["Add a feed"]);
    assert_eq!(compared["behind"], json!(1));
    assert_eq!(compared["fastForward"], json!(false));
    assert_eq!(file_paths, vec!["rss.xml"]);

    let zip = session
        .get(&format!(
            "/api/v1/git/repositories/{folder_id}/archive?ref=v1"
        ))
        .await?
        .expect(200)?;
    assert!(
        zip.header("content-disposition")
            .unwrap_or("")
            .ends_with(".zip")
    );
    assert_eq!(&zip.body[..2], b"PK");
    let blob = session
        .get(&format!(
            "/api/v1/git/repositories/{folder_id}/blob?ref=feature%2Frss&path=rss.xml&view"
        ))
        .await?
        .expect(200)?;
    assert_eq!(blob.text(), "<rss/>\n");
    assert!(
        blob.header("content-security-policy")
            .unwrap_or("")
            .contains("sandbox")
    );

    // Activity counts every commit this year.
    let activity = get_status("/activity", 200).await?;
    assert!(activity["events"].as_array().is_some_and(|a| {
        a.iter()
            .any(|e| e["repoId"] == folder_id && e["subject"] == "Add about")
    }));
    assert!(activity["times"].as_array().map(Vec::len).unwrap_or(0) >= 4);

    // Deleting: a stale ID is refused, main never goes, and a branch goes like a push --delete.
    let remove = |body: Value| {
        let path = format!("/api/v1/git/repositories/{folder_id}/refs");
        let session = session.clone();
        async move { session.send_json("DELETE", &path, &body).await }
    };
    assert_eq!(
        remove(json!({ "name": "refs/heads/feature/rss", "old": head }))
            .await?
            .status,
        409
    );
    assert_eq!(
        remove(json!({ "name": "refs/heads/main", "old": head }))
            .await?
            .status,
        403
    );
    assert_eq!(
        remove(json!({ "name": "refs/heads/feature/rss", "old": branches["branches"][0]["sha"] }))
            .await?
            .status,
        200
    );
    assert_eq!(
        get_status(&format!("/repositories/{folder_id}/branches"), 200).await?["branches"],
        json!([])
    );
    assert!(
        !run(&site, &["ls-remote", &alias], true, "admin")?
            .out
            .contains("feature/rss")
    );
    Ok(())
}
