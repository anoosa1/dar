//! Git tracker (`git-tracker.integration.test.ts`): issues, labels, comments, references,
//! shared-reader permissions, pull requests merged from the web and by push, branch deletion,
//! "Closes #n", Agent branch pull requests, and untracking cleanup.

use crate::suites::agent_git_support::{create_app_password, create_user, run_git};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;

pub const SUITE: Suite = Suite {
    name: "git-tracker",
    cases: &[case!(git_tracker_issues_pulls_and_agent_branches, 600)],
};

pub async fn git_tracker_issues_pulls_and_agent_branches(t: &Ctx) -> Result<()> {
    let fixture = t.fixture().await?;
    let base = fixture.base.clone();
    let session = login(&base).await?;
    let db = session.connect().await?;
    let work = t.temp("dar-git-tracker")?;

    let folder_id = uuid();
    let repo_api = format!("/api/v1/git/repositories/{folder_id}");

    let api = |who: &Session, method: &str, route: &str, body: Option<Value>, expected: u16| {
        let path = format!("{repo_api}{route}");
        let who = who.clone();
        let method = method.to_owned();
        async move {
            let reply = match body {
                Some(b) => who.send_json(&method, &path, &b).await?,
                None => Reply::from(who.request(&method, &path).send().await?).await?,
            };
            let reply = reply.expect(expected)?;
            if reply.body.is_empty() { Ok(Value::Null) } else { reply.json() }
        }
    };

    reducer(|done| db.reducers.create_folder_then(folder_id.clone(), String::new(), format!("Tracker {}", &uuid()[..6]), done)).await?;
    let readme = upload_with(&session, "README.md", "# Tracker\n", Upload { parent_id: &folder_id, ..Upload::default() }).await?;
    session.post(&format!("/api/v1/files/history/{folder_id}"), &json!({ "action": "track" })).await?.expect(200)?;
    let mut state = session.get_json(&format!("/api/v1/files/history/{folder_id}")).await?;
    session.post(&format!("/api/v1/files/history/{folder_id}/commits"), &json!({ "message": "Start", "token": state["status"]["token"] })).await?.expect(201)?;
    let secret = create_app_password(&session, "Tracker", "write").await?;

    let git = |cwd: &std::path::Path, args: &[&str]| -> Result<String> {
        Ok(run_git(cwd, &work, "Ada Lovelace", "ada@example.com", "admin", &secret, args, true, None)?.out)
    };

    // Labels, then issues numbered from 1; a reference records itself on the item it names.
    let bug = api(&session, "POST", "/labels", Some(json!({ "name": "bug", "color": "rose", "description": "Something is wrong" })), 201).await?;
    let bug_id = bug["id"].as_str().context("bug id")?.to_owned();
    api(&session, "POST", "/labels", Some(json!({ "name": "Bug", "color": "blue" })), 409).await?;
    api(&session, "POST", "/labels", Some(json!({ "name": "style", "color": "neon" })), 400).await?;
    let docs = api(&session, "POST", "/labels", Some(json!({ "name": "docs", "color": "teal" })), 201).await?;
    let docs_id = docs["id"].as_str().context("docs id")?.to_owned();

    let first = api(&session, "POST", "/issues", Some(json!({ "kind": "issue", "title": "  Search   is slow ", "body": "Typing lags.", "labels": [bug_id] })), 201).await?;
    assert_eq!(first["number"], json!(1));
    let second = api(&session, "POST", "/issues", Some(json!({ "kind": "issue", "title": "Document search", "body": "Follows #1 and #99." })), 201).await?;
    assert_eq!(second["number"], json!(2));
    let mut issue = api(&session, "GET", "/issues/1", None, 200).await?;
    assert_eq!(issue["title"], json!("Search is slow"));
    assert_eq!(issue["labels"], json!([bug_id]));
    assert_eq!(issue["state"], json!("open"));
    let timeline_refs: Vec<Value> = issue["timeline"]
        .as_array()
        .context("timeline")?
        .iter()
        .map(|item| if item["kind"] == "event" { json!([item["event"], item["number"]]) } else { json!(["comment"]) })
        .collect();
    assert_eq!(timeline_refs, vec![json!(["referenced", 2])]);
    assert_eq!(issue["permissions"], json!({ "edit": true, "close": true, "label": true, "merge": false, "comment": true }));

    // Comments, edits, renames, labels and close/reopen, guarded by the revision.
    let comment = api(&session, "POST", "/issues/1/comments", Some(json!({ "body": "Seen on large folders." })), 201).await?;
    let comment_id = comment["id"].as_str().context("comment id")?.to_owned();
    api(&session, "PATCH", &format!("/comments/{comment_id}"), Some(json!({ "body": "Seen on large folders too." })), 200).await?;
    issue = api(&session, "GET", "/issues/1", None, 200).await?;
    let rev = issue["revision"].as_u64().context("issue revision")?;
    api(&session, "PATCH", "/issues/1", Some(json!({ "revision": rev - 1, "title": "Stale" })), 409).await?;
    api(&session, "PATCH", "/issues/1", Some(json!({ "revision": rev, "title": "Search is slow on large folders", "labels": [bug_id, docs_id], "state": "closed" })), 200).await?;
    issue = api(&session, "GET", "/issues/1", None, 200).await?;
    assert_eq!(issue["state"], json!("closed"));
    assert_eq!(issue["comments"], json!(1));
    let events: Vec<&str> = issue["timeline"]
        .as_array()
        .context("timeline")?
        .iter()
        .map(|item| if item["kind"] == "event" { item["event"].as_str().unwrap_or("") } else { "comment" })
        .collect();
    assert_eq!(events, vec!["referenced", "comment", "renamed", "labeled", "closed"]);
    let edited = issue["timeline"].as_array().unwrap().iter().find(|item| item["kind"] == "comment").context("edited comment")?;
    assert!(edited["editedAt"].as_u64().unwrap_or(0) > 0);
    assert!(edited["body"].as_str().unwrap_or("").ends_with("too."));
    api(&session, "PATCH", "/issues/1", Some(json!({ "revision": issue["revision"], "state": "open" })), 200).await?;
    let mut list = api(&session, "GET", "/issues?kind=issue&state=open", None, 200).await?;
    let nums: Vec<u64> = list["items"].as_array().context("items")?.iter().filter_map(|i| i["number"].as_u64()).collect();
    assert_eq!(nums, vec![2, 1]);
    assert_eq!(list["open"], json!(2));
    assert_eq!(list["closed"], json!(0));
    let docs_list = api(&session, "GET", &format!("/issues?kind=issue&state=open&label={docs_id}"), None, 200).await?;
    let docs_nums: Vec<u64> = docs_list["items"].as_array().context("docs items")?.iter().filter_map(|i| i["number"].as_u64()).collect();
    assert_eq!(docs_nums, vec![1]);
    let label_names: Vec<&str> = list["labels"].as_array().context("labels")?.iter().filter_map(|l| l["name"].as_str()).collect();
    assert_eq!(label_names, vec!["bug", "docs"]);

    // A reader the folder is shared with sees the repository, opens issues and comments, nothing more.
    let username = format!("tracker-{}", &uuid()[..8]);
    create_user(&session, &username, "Tracker Reader", PASSWORD).await?;
    let reader = login_as(&base, &username, PASSWORD).await?;
    reducer(|done| db.reducers.browse_then(String::new(), "files".into(), String::new(), String::new(), done)).await?;
    let folder = db.db.my_files().iter().find(|row| row.id == folder_id).context("folder row")?;
    reducer(|done| db.reducers.grant_access_then(uuid(), folder_id.clone(), username.clone(), "read".into(), folder.revision, done)).await?;
    let reader_repos = reader.get_json("/api/v1/git/repositories").await?;
    let shared = reader_repos.as_array().context("reader repos")?.iter().find(|r| r["id"] == folder_id).context("shared repo")?;
    assert_eq!(shared["role"], json!("read"));
    assert_eq!(shared["owner"], json!("Test Administrator"));
    assert_eq!(shared["clone"]["alias"], json!(format!("{base}/dav/git/{folder_id}.git")));
    assert_eq!(shared["path"], json!(folder.name));
    assert_eq!(shared["openIssues"], json!(2));
    let asked = api(&reader, "POST", "/issues", Some(json!({ "kind": "issue", "title": "Question from a reader" })), 201).await?;
    assert_eq!(asked["number"], json!(3));
    api(&reader, "POST", "/issues", Some(json!({ "kind": "issue", "title": "With a label", "labels": [bug_id] })), 403).await?;
    api(&reader, "POST", "/labels", Some(json!({ "name": "mine", "color": "plum" })), 403).await?;
    let theirs = api(&reader, "POST", "/issues/1/comments", Some(json!({ "body": "Me too" })), 201).await?;
    let theirs_id = theirs["id"].as_str().context("reader comment id")?;
    api(&session, "PATCH", &format!("/comments/{theirs_id}"), Some(json!({ "body": "Not yours" })), 403).await?;
    issue = api(&reader, "GET", "/issues/1", None, 200).await?;
    assert_eq!(issue["permissions"], json!({ "edit": false, "close": false, "label": false, "merge": false, "comment": true }));
    api(&reader, "PATCH", "/issues/1", Some(json!({ "revision": issue["revision"], "state": "closed" })), 403).await?;
    api(&session, "DELETE", &format!("/comments/{theirs_id}"), None, 200).await?;

    // A pull request from a pushed branch; the web merge closes it and the issue it names.
    let git_url = format!("{base}/dav/git/{folder_id}.git");
    git(&work, &["clone", "-q", &git_url, "clone"])?;
    let clone = work.join("clone");
    git(&clone, &["checkout", "-q", "-b", "feature/fast-search"])?;
    std::fs::write(clone.join("search.txt"), "fast\n")?;
    git(&clone, &["add", "-A"])?;
    git(&clone, &["commit", "-qm", "Index names"])?;
    git(&clone, &["push", "-q", "origin", "feature/fast-search"])?;
    api(&reader, "POST", "/issues", Some(json!({ "kind": "pull", "title": "Not allowed", "base": "main", "head": "feature/fast-search" })), 403).await?;
    api(&session, "POST", "/issues", Some(json!({ "kind": "pull", "title": "Missing", "base": "main", "head": "feature/none" })), 404).await?;
    let pull = api(&session, "POST", "/issues", Some(json!({ "kind": "pull", "title": "Faster search", "body": "Closes #1.", "base": "main", "head": "feature/fast-search" })), 201).await?;
    let pull_num = pull["number"].as_u64().context("pull number")?;
    assert_eq!(pull_num, 4);
    api(&session, "POST", "/issues", Some(json!({ "kind": "pull", "title": "Twice", "base": "main", "head": "feature/fast-search" })), 409).await?;
    let open_pulls = api(&session, "GET", "/issues?kind=pull&state=open", None, 200).await?;
    let pull_rows: Vec<Value> = open_pulls["items"].as_array().context("pulls")?.iter().map(|i| json!([i["number"], i["base"], i["head"], i["agent"]])).collect();
    assert_eq!(pull_rows, vec![json!([4, "main", "feature/fast-search", false])]);

    // Another push to the branch is recorded on the pull request.
    std::fs::write(clone.join("search.txt"), "faster\n")?;
    git(&clone, &["commit", "-qam", "Tune index"])?;
    git(&clone, &["push", "-q", "origin", "feature/fast-search"])?;
    let mut changes = api(&session, "GET", &format!("/issues/{pull_num}/changes"), None, 200).await?;
    assert_eq!(changes["compare"]["ahead"].as_array().map(Vec::len), Some(2));
    assert_eq!(changes["compare"]["fastForward"], json!(true));
    let paths: Vec<&str> = changes["compare"]["files"].as_array().context("files")?.iter().filter_map(|f| f["path"].as_str()).collect();
    assert_eq!(paths, vec!["search.txt"]);
    api(&session, "POST", "/merge", Some(json!({ "base": "main", "head": "feature/fast-search", "baseSha": changes["compare"]["baseSha"], "headSha": changes["compare"]["headSha"], "mode": "merge", "deleteBranch": true })), 200).await?;
    let merged = api(&session, "GET", &format!("/issues/{pull_num}"), None, 200).await?;
    assert_eq!(merged["state"], json!("merged"));
    assert_eq!(merged["headSha"], changes["compare"]["headSha"]);
    assert!(merged["mergedSha"].as_str().is_some_and(|s| !s.is_empty()));
    let merged_events: Vec<&str> = merged["timeline"].as_array().context("timeline")?.iter().filter(|i| i["kind"] == "event").filter_map(|i| i["event"].as_str()).collect();
    assert_eq!(merged_events, vec!["pushed", "merged"]);
    issue = api(&session, "GET", "/issues/1", None, 200).await?;
    assert_eq!(issue["state"], json!("closed"));
    assert!(issue["timeline"].as_array().unwrap().iter().any(|i| i["kind"] == "event" && i["event"] == "closed" && i["number"] == pull_num));
    // Its commits stay readable after the branch is gone.
    changes = api(&session, "GET", &format!("/issues/{pull_num}/changes"), None, 200).await?;
    assert_eq!(changes["compare"]["ahead"].as_array().map(Vec::len), Some(2));

    // A plain push that merges a branch closes its pull request as merged too.
    git(&clone, &["checkout", "-q", "main"])?;
    git(&clone, &["pull", "-q", "--ff-only"])?;
    git(&clone, &["checkout", "-q", "-b", "topic"])?;
    std::fs::write(clone.join("topic.txt"), "topic\n")?;
    git(&clone, &["add", "-A"])?;
    git(&clone, &["commit", "-qm", "Topic"])?;
    git(&clone, &["push", "-q", "origin", "topic"])?;
    let by_push = api(&session, "POST", "/issues", Some(json!({ "kind": "pull", "title": "Topic", "base": "main", "head": "topic" })), 201).await?;
    let by_push_num = by_push["number"].as_u64().context("by_push number")?;
    git(&clone, &["checkout", "-q", "main"])?;
    git(&clone, &["merge", "-q", "--no-ff", "-m", "Merge topic", "topic"])?;
    git(&clone, &["push", "-q", "origin", "main"])?;
    assert_eq!(api(&session, "GET", &format!("/issues/{by_push_num}"), None, 200).await?["state"], json!("merged"));

    // Deleting a branch closes its pull request; reopening needs the branch back.
    git(&clone, &["checkout", "-q", "-b", "doomed"])?;
    std::fs::write(clone.join("doomed.txt"), "x\n")?;
    git(&clone, &["add", "-A"])?;
    git(&clone, &["commit", "-qm", "Doomed"])?;
    git(&clone, &["push", "-q", "origin", "doomed"])?;
    let doomed = api(&session, "POST", "/issues", Some(json!({ "kind": "pull", "title": "Doomed", "base": "main", "head": "doomed" })), 201).await?;
    let doomed_num = doomed["number"].as_u64().context("doomed number")?;
    git(&clone, &["push", "-q", "origin", "--delete", "doomed"])?;
    let mut closed = api(&session, "GET", &format!("/issues/{doomed_num}"), None, 200).await?;
    assert_eq!(closed["state"], json!("closed"));
    assert!(closed["timeline"].as_array().unwrap().iter().any(|i| i["kind"] == "event" && i["event"] == "branchDeleted"));
    api(&session, "PATCH", &format!("/issues/{doomed_num}"), Some(json!({ "revision": closed["revision"], "state": "open" })), 409).await?;
    git(&clone, &["push", "-q", "origin", "doomed"])?;
    api(&session, "PATCH", &format!("/issues/{doomed_num}"), Some(json!({ "revision": closed["revision"], "state": "open" })), 200).await?;
    closed = api(&session, "GET", &format!("/issues/{doomed_num}"), None, 200).await?;
    api(&session, "PATCH", &format!("/issues/{doomed_num}"), Some(json!({ "revision": closed["revision"], "state": "closed" })), 200).await?;
    changes = api(&session, "GET", &format!("/issues/{doomed_num}/changes"), None, 200).await?;
    let doomed_subjects: Vec<&str> = changes["compare"]["ahead"].as_array().context("ahead")?.iter().filter_map(|c| c["subject"].as_str()).collect();
    assert_eq!(doomed_subjects, vec!["Doomed"]);

    // A pull request from an Agent branch shows its changes and is merged by Agent's merge.
    let project_id = uuid();
    let thread_id = uuid();
    session.post("/api/v1/agent/projects", &json!({ "id": project_id, "name": "Tracker project", "sourceType": "files", "sourceRef": folder_id })).await?.expect(201)?;
    session.post("/api/v1/agent/threads", &json!({ "id": thread_id, "projectId": project_id, "title": "Tidy" })).await?.expect(201)?;
    session.get(&format!("/api/v1/agent/threads/{thread_id}/branch")).await?.expect(200)?;
    reducer(|done| db.reducers.browse_then(folder_id.clone(), "files".into(), String::new(), String::new(), done)).await?;
    let readme_row = db.db.my_files().iter().find(|row| row.id == readme.file_id).context("readme row")?;
    let me = db.db.me().iter().next().context("me row")?.id;
    let thread_dir = fixture.root().join("agent-workspaces").join(&me).join(&thread_id);
    std::fs::create_dir_all(thread_dir.join("work"))?;
    std::fs::write(thread_dir.join("work").join("README.md"), "# Tidy\n")?;
    std::fs::write(
        thread_dir.join("baseline.json"),
        json!({
            "source": {
                "project": { "id": project_id, "ownerId": me, "name": "Tracker project", "sourceType": "files", "sourceRef": folder_id, "createdAt": 0 },
                "files": [{ "id": readme.file_id, "path": "README.md", "revision": readme_row.revision, "size": 10, "mediaType": "text/plain" }]
            },
            "files": [{ "path": "README.md", "content": dar_acceptance::base64(b"# Tracker\n") }]
        })
        .to_string(),
    )?;
    let staged = session.post(&format!("/api/v1/agent/threads/{thread_id}/branch"), &json!({})).await?.expect(201)?.json()?;
    let branch_name = staged["branch"]["name"].as_str().context("branch name")?.to_owned();
    api(&session, "POST", "/issues", Some(json!({ "kind": "pull", "title": "Into a branch", "base": "topic", "head": branch_name })), 400).await?;
    let from_agent = api(&session, "POST", "/issues", Some(json!({ "kind": "pull", "title": "Tidy the README", "base": "main", "head": branch_name })), 201).await?;
    let from_agent_num = from_agent["number"].as_u64().context("from_agent number")?;
    changes = api(&session, "GET", &format!("/issues/{from_agent_num}/changes"), None, 200).await?;
    let agent_changes: Vec<(&str, &str)> = changes["agent"]["changes"].as_array().context("agent changes")?.iter().map(|c| (c["kind"].as_str().unwrap_or(""), c["path"].as_str().unwrap_or(""))).collect();
    assert_eq!(agent_changes, vec![("modified", "README.md")]);
    assert_eq!(changes["agent"]["threadId"], json!(thread_id));
    session.post(&format!("/api/v1/files/history/{folder_id}/branches/merge"), &json!({ "name": branch_name, "revision": changes["agent"]["revision"] })).await?.expect(200)?;
    assert_eq!(api(&session, "GET", &format!("/issues/{from_agent_num}"), None, 200).await?["state"], json!("merged"));

    // Counts follow; the tracker goes with the folder's history.
    let repo = api(&session, "GET", "", None, 200).await?;
    assert_eq!(repo["openIssues"], json!(2));
    assert_eq!(repo["openPulls"], json!(0));
    state = session.get_json(&format!("/api/v1/files/history/{folder_id}")).await?;
    session.post(&format!("/api/v1/files/history/{folder_id}"), &json!({ "action": "untrack", "revision": state["repo"]["revision"] })).await?.expect(200)?;
    session.post(&format!("/api/v1/files/history/{folder_id}"), &json!({ "action": "track" })).await?.expect(200)?;
    list = api(&session, "GET", "/issues?kind=issue&state=open", None, 200).await?;
    assert_eq!(list["items"].as_array().map(Vec::len), Some(0));
    assert_eq!(list["labels"].as_array().map(Vec::len), Some(0));
    Ok(())
}
