//! Git interop (`git-interop.integration.test.ts`): force pushes, lightweight tags, notes,
//! partial clones, atomic pushes, pruning, signed commits, unusual names and content, refusals,
//! web merges, restore, source filters and disabling the Git app.

use crate::suites::agent_git_support::{create_app_password, encode_uri_component, run_git};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::process::{Command, Stdio};
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "git-interop",
    cases: &[case!(git_interop_and_web_merges, 600)],
};

pub async fn git_interop_and_web_merges(t: &Ctx) -> Result<()> {
    let fixture = t.fixture().await?;
    let base = fixture.base.clone();
    let session = login(&base).await?;
    let db = session.connect().await?;
    let work = t.temp("dar-git-interop")?;

    let history = |folder_id: &str, rest: &str| format!("/api/v1/files/history/{folder_id}{rest}");
    let api = |method: &str, route: &str, body: Option<Value>, expected: u16| {
        let path = format!("/api/v1/git{route}");
        let session = session.clone();
        let method = method.to_owned();
        async move {
            let reply = match body {
                Some(b) => session.send_json(&method, &path, &b).await?,
                None => Reply::from(session.request(&method, &path).send().await?).await?,
            };
            let reply = reply.expect(expected)?;
            if reply.body.is_empty() { Ok(Value::Null) } else { reply.json() }
        }
    };

    let folder_id = uuid();
    let name = format!("Interop {}", &uuid()[..6]);
    let repo_api = format!("/repositories/{folder_id}");
    reducer(|done| db.reducers.create_folder_then(folder_id.clone(), String::new(), name.clone(), done)).await?;
    let readme = upload_with(&session, "README.md", "# Interop\n", Upload { parent_id: &folder_id, ..Upload::default() }).await?;
    session.post(&history(&folder_id, ""), &json!({ "action": "track" })).await?.expect(200)?;
    let mut state = session.get_json(&history(&folder_id, "")).await?;
    session.post(&history(&folder_id, "/commits"), &json!({ "message": "Start", "token": state["status"]["token"] })).await?.expect(201)?;
    let secret = create_app_password(&session, "Interop", "write").await?;

    let git = |cwd: &std::path::Path, args: &[&str], ok: bool, input: Option<&str>| {
        let mut full = vec!["-c", "protocol.version=2"];
        full.extend_from_slice(args);
        run_git(cwd, &work, "Ada Lovelace", "ada@example.com", "admin", &secret, &full, ok, input)
    };

    let children = |parent_id: &str| {
        let session = session.clone();
        let parent_id = parent_id.to_owned();
        async move {
            let mut items: Vec<Value> = Vec::new();
            let mut cursor = String::new();
            loop {
                let query = format!("/api/v1/files/picker?parentId={}&cursor={}&kind=file", encode_uri_component(&parent_id), encode_uri_component(&cursor));
                let page = session.get_json(&query).await?;
                if let Some(arr) = page["items"].as_array() {
                    items.extend(arr.iter().cloned());
                }
                match page["nextCursor"].as_str() {
                    Some(next) if !next.is_empty() => cursor = next.to_owned(),
                    _ => break,
                }
            }
            Ok::<Vec<Value>, Error>(items)
        }
    };
    let child = |parent_id: &str, target: &str| {
        let fut = children(parent_id);
        let target = target.to_owned();
        async move { Ok::<Option<Value>, Error>(fut.await?.into_iter().find(|item| item["name"] == target.as_str())) }
    };

    let url = api("GET", &repo_api, None, 200).await?["clone"]["alias"].as_str().context("clone alias")?.to_owned();
    git(&work, &["clone", "-q", &url, "a"], true, None)?;
    let a = work.join("a");
    git(&work, &["clone", "-q", &url, "b"], true, None)?;
    let b = work.join("b");
    let remote = || -> Result<String> { Ok(git(&a, &["ls-remote", &url], true, None)?.out) };

    // Force pushes: refused without --force, accepted with it.
    git(&a, &["checkout", "-q", "-b", "topic"], true, None)?;
    std::fs::write(a.join("topic.txt"), "one\n")?;
    git(&a, &["add", "-A"], true, None)?;
    git(&a, &["commit", "-qm", "Topic one"], true, None)?;
    git(&a, &["push", "-q", "origin", "topic"], true, None)?;
    git(&a, &["commit", "-q", "--amend", "-m", "Topic rewritten"], true, None)?;
    let non_ff = git(&a, &["push", "origin", "topic"], false, None)?;
    assert!(non_ff.err.contains("rejected") || non_ff.err.contains("non-fast-forward"), "stderr: {}", non_ff.err);
    git(&a, &["push", "-q", "--force", "origin", "topic"], true, None)?;
    let topic_sha = git(&a, &["rev-parse", "topic"], true, None)?.out;
    let topic_re = regex::Regex::new(&format!(r"{topic_sha}\s+refs/heads/topic")).unwrap();
    assert!(topic_re.is_match(&remote()?));

    // Lightweight tags push and delete.
    git(&a, &["tag", "light", "topic"], true, None)?;
    git(&a, &["push", "-q", "origin", "light"], true, None)?;
    let tags = api("GET", &format!("{repo_api}/tags"), None, 200).await?;
    let tag_pairs: Vec<(&str, bool)> = tags.as_array().context("tags")?.iter().map(|t| (t["name"].as_str().unwrap_or(""), t["annotated"].as_bool().unwrap_or(false))).collect();
    assert_eq!(tag_pairs, vec![("light", false)]);
    git(&a, &["push", "-q", "origin", "--delete", "light"], true, None)?;
    assert!(!remote()?.contains("refs/tags/light"));

    // Notes travel with refs/notes.
    git(&a, &["notes", "add", "-m", "Reviewed", "topic"], true, None)?;
    git(&a, &["push", "-q", "origin", "refs/notes/commits"], true, None)?;
    git(&b, &["fetch", "-q", "origin", "refs/notes/commits:refs/notes/commits", "topic:topic"], true, None)?;
    assert_eq!(git(&b, &["notes", "show", "topic"], true, None)?.out, "Reviewed");

    // Partial clone: blobs arrive when checked out or diffed.
    git(&work, &["clone", "-q", "--filter=blob:none", &url, "partial"], true, None)?;
    assert_eq!(std::fs::read_to_string(work.join("partial").join("README.md"))?, "# Interop\n");
    let partial_log = git(&work.join("partial"), &["log", "-p", "--all"], true, None)?.out;
    assert!(partial_log.contains("Topic one") || partial_log.contains("one"));

    // Atomic pushes: one bad ref stops them all.
    git(&a, &["checkout", "-q", "main"], true, None)?;
    git(&a, &["checkout", "-q", "-b", "atomic-new"], true, None)?;
    git(&a, &["checkout", "-q", "main"], true, None)?;
    git(&a, &["commit", "-q", "--allow-empty", "-m", "Diverge"], true, None)?;
    git(&b, &["commit", "-q", "--allow-empty", "-m", "Ahead elsewhere"], true, None)?;
    git(&b, &["push", "-q", "origin", "main"], true, None)?;
    git(&a, &["push", "--atomic", "origin", "main", "atomic-new"], false, None)?;
    assert!(!remote()?.contains("atomic-new"));
    git(&a, &["reset", "-q", "--hard", "origin/main"], true, None)?;
    git(&a, &["pull", "-q", "--ff-only"], true, None)?;

    // fetch --prune drops branches deleted on Dar.
    git(&b, &["fetch", "-q", "origin"], true, None)?;
    assert!(git(&b, &["branch", "-r"], true, None)?.out.contains("origin/topic"));
    git(&a, &["push", "-q", "origin", "--delete", "topic"], true, None)?;
    git(&b, &["fetch", "-q", "--prune", "origin"], true, None)?;
    assert!(!git(&b, &["branch", "-r"], true, None)?.out.contains("origin/topic"));

    // Refusals: submodules, names differing only by case, shallow pushes, oversized pushes.
    git(&a, &["checkout", "-q", "-B", "refused", "main"], true, None)?;
    let head_sha = git(&a, &["rev-parse", "HEAD"], true, None)?.out;
    let cacheinfo = format!("160000,{head_sha},lib");
    git(&a, &["update-index", "--add", "--cacheinfo", &cacheinfo], true, None)?;
    git(&a, &["commit", "-qm", "Submodule"], true, None)?;
    let sub_err = git(&a, &["push", "origin", "refused:main"], false, None)?.err;
    assert!(sub_err.to_ascii_lowercase().contains("submodule"), "stderr: {sub_err}");
    git(&a, &["checkout", "-q", "main"], true, None)?;
    git(&a, &["branch", "-q", "-D", "refused"], true, None)?;

    git(&a, &["checkout", "-q", "-B", "refused", "main"], true, None)?;
    std::fs::write(a.join("Notes.txt"), "1\n")?;
    std::fs::write(a.join("notes.txt"), "2\n")?;
    git(&a, &["add", "-A"], true, None)?;
    git(&a, &["commit", "-qm", "Case"], true, None)?;
    let case_err = git(&a, &["push", "origin", "refused:main"], false, None)?.err;
    assert!(case_err.contains("only by case"), "stderr: {case_err}");
    git(&a, &["checkout", "-q", "main"], true, None)?;
    git(&a, &["branch", "-q", "-D", "refused"], true, None)?;
    let _ = std::fs::remove_file(a.join("Notes.txt"));
    let _ = std::fs::remove_file(a.join("notes.txt"));

    git(&work, &["clone", "-q", "--depth", "1", &url, "shallow"], true, None)?;
    let shallow = work.join("shallow");
    std::fs::write(shallow.join("from-shallow.txt"), "x\n")?;
    git(&shallow, &["add", "-A"], true, None)?;
    git(&shallow, &["commit", "-qm", "From a shallow clone"], true, None)?;
    git(&shallow, &["push", "origin", "main"], false, None)?;

    let mut stream = String::from("blob\nmark :1\ndata 2\nx\n\n");
    for i in 0..5001u64 {
        stream.push_str("commit refs/heads/huge\n");
        stream.push_str(&format!("committer Ada <ada@example.com> {} +0000\n", 1_700_000_000 + i));
        stream.push_str("data 2\nc\n");
        if i == 0 {
            stream.push_str("from refs/heads/main\n");
        }
        stream.push_str(&format!("M 100644 :1 n{}.txt\n\n", i % 7));
    }
    git(&a, &["fast-import", "--quiet"], true, Some(&stream))?;
    let huge_err = git(&a, &["push", "origin", "huge"], false, None)?.err;
    assert!(huge_err.contains("at most 5000"), "stderr: {huge_err}");
    git(&a, &["branch", "-q", "-D", "huge"], true, None)?;
    assert_eq!(api("GET", &repo_api, None, 200).await?["branches"], json!(1), "nothing refused was kept");

    // Signed commits keep their exact bytes: the signature still verifies after a fresh clone.
    let keys_dir = work.join("keys");
    std::fs::create_dir_all(&keys_dir)?;
    let key_path = keys_dir.join("id");
    let keygen = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "ada", "-f", key_path.to_str().unwrap()])
        .stdin(Stdio::null())
        .status()?;
    assert!(keygen.success(), "ssh-keygen failed");
    let pub_key = std::fs::read_to_string(keys_dir.join("id.pub"))?;
    let allowed_path = keys_dir.join("allowed");
    std::fs::write(&allowed_path, format!("ada@example.com {}\n", pub_key.trim()))?;
    std::fs::write(a.join("signed.txt"), "signed\n")?;
    git(&a, &["add", "-A"], true, None)?;
    let signing_arg = format!("user.signingkey={}", key_path.display());
    git(&a, &["-c", "gpg.format=ssh", "-c", &signing_arg, "commit", "-q", "-S", "-m", "Signed change"], true, None)?;
    git(&a, &["push", "-q", "origin", "main"], true, None)?;
    let signed = git(&a, &["rev-parse", "HEAD"], true, None)?.out;
    let cache_dir = fixture.root().join("git").join(format!("{folder_id}.git"));
    if cache_dir.exists() {
        std::fs::remove_dir_all(&cache_dir)?;
    }
    git(&work, &["clone", "-q", &url, "c"], true, None)?;
    let c = work.join("c");
    assert_eq!(git(&c, &["rev-parse", "HEAD"], true, None)?.out, signed);
    let allowed_arg = format!("gpg.ssh.allowedSignersFile={}", allowed_path.display());
    git(&c, &["-c", &allowed_arg, "verify-commit", "HEAD"], true, None)?;

    // Names and content: accents, binary files and diffs too large to show.
    std::fs::create_dir_all(a.join("café"))?;
    std::fs::write(a.join("café").join("naïve résumé.txt"), "bonjour\n")?;
    let binary: Vec<u8> = (0..3000).map(|i| ((i * 37) % 256) as u8).collect();
    std::fs::write(a.join("image.bin"), &binary)?;
    let big: String = (0..6000).map(|i| format!("line {i}\n")).collect();
    std::fs::write(a.join("big.txt"), big)?;
    git(&a, &["add", "-A"], true, None)?;
    git(&a, &["commit", "-qm", "Unusual content"], true, None)?;
    git(&a, &["push", "-q", "origin", "main"], true, None)?;
    eventually(Duration::from_secs(5), "accented names reach Files", || async {
        let Some(cafe) = child(&folder_id, "café").await? else { return Ok(None) };
        let cafe_id = cafe["id"].as_str().unwrap_or("");
        Ok(child(cafe_id, "naïve résumé.txt").await?.map(|_| ()))
    })
    .await?;
    let cafe_tree = api("GET", &format!("{repo_api}/tree?path=caf%C3%A9"), None, 200).await?;
    let cafe_names: Vec<&str> = cafe_tree["entries"].as_array().context("entries")?.iter().filter_map(|e| e["name"].as_str()).collect();
    assert_eq!(cafe_names, vec!["naïve résumé.txt"]);
    let unusual_sha = git(&a, &["rev-parse", "HEAD"], true, None)?.out;
    let detail = api("GET", &format!("{repo_api}/commits/{unusual_sha}"), None, 200).await?;
    let files = detail["files"].as_array().context("files")?;
    assert_eq!(files.iter().find(|f| f["path"] == "image.bin").and_then(|f| f["binary"].as_bool()), Some(true));
    assert_eq!(files.iter().find(|f| f["path"] == "big.txt").and_then(|f| f["truncated"].as_bool()), Some(true));
    assert_eq!(files.iter().find(|f| f["path"] == "café/naïve résumé.txt").and_then(|f| f["patch"].as_str()), Some("@@ -0,0 +1 @@\n+bonjour"));
    let fetched_bin = session.get(&format!("/api/v1/git{repo_api}/blob?path=image.bin")).await?.expect(200)?.body;
    assert_eq!(fetched_bin, binary);

    // A branch pushed to main from the command line fast-forwards it and changes the folder.
    git(&a, &["checkout", "-q", "-b", "promote"], true, None)?;
    std::fs::write(a.join("promoted.txt"), "promoted\n")?;
    git(&a, &["add", "-A"], true, None)?;
    git(&a, &["commit", "-qm", "Promote me"], true, None)?;
    git(&a, &["push", "-q", "origin", "promote"], true, None)?;
    git(&a, &["push", "-q", "origin", "promote:main"], true, None)?;
    eventually(Duration::from_secs(5), "the promoted commit changes the folder", || async {
        Ok(child(&folder_id, "promoted.txt").await?.map(|_| ()))
    })
    .await?;
    git(&a, &["checkout", "-q", "main"], true, None)?;
    git(&a, &["pull", "-q", "--ff-only"], true, None)?;
    git(&a, &["push", "-q", "origin", "--delete", "promote"], true, None)?;

    // Web merges: fast-forward, a merge commit, conflicts, stale views and uncommitted Dar edits.
    let merge = |body: Value, expected: u16| api("POST", &format!("{repo_api}/merge"), Some(body), expected);
    let compare = |head: &str| api("GET", &format!("{repo_api}/compare?base=main&head={}", encode_uri_component(head)), None, 200);

    git(&a, &["checkout", "-q", "-b", "ff"], true, None)?;
    std::fs::write(a.join("ff.txt"), "fast\n")?;
    git(&a, &["add", "-A"], true, None)?;
    git(&a, &["commit", "-qm", "Fast-forward me"], true, None)?;
    git(&a, &["push", "-q", "origin", "ff"], true, None)?;
    let mut view = compare("ff").await?;
    assert_eq!([view["fastForward"].as_bool(), view["mergeable"].as_bool()], [Some(true), Some(true)]);
    let mut result = merge(json!({ "base": "main", "head": "ff", "baseSha": view["baseSha"], "headSha": view["headSha"], "mode": "fast-forward", "deleteBranch": true }), 200).await?;
    assert_eq!(result["sha"], view["headSha"]);
    assert_eq!(result["fastForward"], json!(true));
    assert_eq!(result["deleted"], json!(true));
    eventually(Duration::from_secs(5), "a fast-forward changes the folder", || async {
        Ok(child(&folder_id, "ff.txt").await?.map(|_| ()))
    })
    .await?;
    assert!(!remote()?.contains("refs/heads/ff"));

    git(&a, &["checkout", "-q", "main"], true, None)?;
    git(&a, &["pull", "-q", "--ff-only"], true, None)?;
    std::fs::write(a.join("shared.txt"), "top\nmiddle\nbottom\n")?;
    git(&a, &["add", "-A"], true, None)?;
    git(&a, &["commit", "-qm", "Shared file"], true, None)?;
    git(&a, &["push", "-q", "origin", "main"], true, None)?;
    git(&a, &["checkout", "-q", "-b", "side"], true, None)?;
    std::fs::write(a.join("shared.txt"), "TOP\nmiddle\nbottom\n")?;
    git(&a, &["commit", "-qam", "Side edits the top"], true, None)?;
    git(&a, &["push", "-q", "origin", "side"], true, None)?;
    git(&a, &["checkout", "-q", "main"], true, None)?;
    std::fs::write(a.join("shared.txt"), "top\nmiddle\nBOTTOM\n")?;
    git(&a, &["commit", "-qam", "Main edits the bottom"], true, None)?;
    git(&a, &["push", "-q", "origin", "main"], true, None)?;
    view = compare("side").await?;
    assert_eq!([view["fastForward"].as_bool(), view["mergeable"].as_bool()], [Some(false), Some(true)]);
    assert_eq!(view["behind"], json!(1));
    merge(json!({ "base": "main", "head": "side", "baseSha": view["baseSha"], "headSha": view["headSha"], "mode": "fast-forward" }), 409).await?;
    merge(json!({ "base": "main", "head": "side", "baseSha": "0".repeat(40), "headSha": view["headSha"], "mode": "merge" }), 409).await?;
    result = merge(json!({ "base": "main", "head": "side", "baseSha": view["baseSha"], "headSha": view["headSha"], "mode": "merge", "message": "Merge side from the web" }), 200).await?;
    assert_eq!(result["fastForward"], json!(false));
    git(&a, &["pull", "-q", "--ff-only"], true, None)?;
    let expected_log = format!("Merge side from the web|{} {}|Test Administrator|admin@dar.invalid", view["baseSha"].as_str().unwrap(), view["headSha"].as_str().unwrap());
    assert_eq!(git(&a, &["log", "-1", "--format=%s|%P|%an|%ae"], true, None)?.out, expected_log);
    let shared_file = child(&folder_id, "shared.txt").await?.context("shared.txt")?;
    let shared_id = shared_file["id"].as_str().context("shared.txt id")?.to_owned();
    eventually(Duration::from_secs(5), "a merged file holds both sides", || async {
        let text = session.get(&format!("/api/v1/files/{shared_id}/content")).await?.expect(200)?.text();
        Ok((text == "TOP\nmiddle\nBOTTOM\n").then_some(()))
    })
    .await?;

    git(&a, &["checkout", "-q", "-B", "clash", "main"], true, None)?;
    std::fs::write(a.join("shared.txt"), "CLASH\nmiddle\nBOTTOM\n")?;
    git(&a, &["commit", "-qam", "Clash"], true, None)?;
    git(&a, &["push", "-q", "origin", "clash"], true, None)?;
    git(&a, &["checkout", "-q", "main"], true, None)?;
    std::fs::write(a.join("shared.txt"), "OTHER\nmiddle\nBOTTOM\n")?;
    git(&a, &["commit", "-qam", "Other"], true, None)?;
    git(&a, &["push", "-q", "origin", "main"], true, None)?;
    view = compare("clash").await?;
    assert_eq!(view["mergeable"], json!(false));
    let clash = session.post(&format!("/api/v1/git{repo_api}/merge"), &json!({ "base": "main", "head": "clash", "baseSha": view["baseSha"], "headSha": view["headSha"], "mode": "merge" })).await?;
    assert_eq!(clash.status, 409);
    assert!(clash.text().contains("shared.txt"));
    git(&a, &["push", "-q", "origin", "--delete", "clash"], true, None)?;

    git(&a, &["checkout", "-q", "-b", "dirty"], true, None)?;
    std::fs::write(a.join("README.md"), "# From the branch\n")?;
    git(&a, &["commit", "-qam", "Branch README"], true, None)?;
    git(&a, &["push", "-q", "origin", "dirty"], true, None)?;
    git(&a, &["checkout", "-q", "main"], true, None)?;
    upload_with(&session, "README.md", "# Edited in Dar\n", Upload { parent_id: &folder_id, file_id: Some(&readme.file_id), expected_revision: Some(1), ..Upload::default() }).await?;
    view = compare("dirty").await?;
    let dirty = session.post(&format!("/api/v1/git{repo_api}/merge"), &json!({ "base": "main", "head": "dirty", "baseSha": view["baseSha"], "headSha": view["headSha"], "mode": "fast-forward" })).await?;
    assert_eq!(dirty.status, 409);
    assert!(dirty.text().contains("uncommitted"));
    assert_eq!(session.get(&format!("/api/v1/files/{}/content", readme.file_id)).await?.expect(200)?.text(), "# Edited in Dar\n", "the Dar edit is untouched");

    // Restore from a commit's details: refused while Dar has edits, then a new restore commit.
    let main_dar = api("GET", &format!("{repo_api}/commits?ref=main&source=dar"), None, 200).await?;
    let start_sha = main_dar["commits"].as_array().and_then(|arr| arr.last()).and_then(|c| c["sha"].as_str()).context("start commit sha")?.to_owned();
    let start_detail = api("GET", &format!("{repo_api}/commits/{start_sha}"), None, 200).await?;
    assert!(start_detail["restore"].is_object(), "main commits can be restored");
    let bad_restore = session.post(&history(&folder_id, "/restore"), &json!({ "id": start_detail["restore"]["changesetId"], "revision": start_detail["restore"]["revision"] })).await?;
    assert!(bad_restore.status >= 400, "restoring over uncommitted edits is refused");
    state = session.get_json(&history(&folder_id, "")).await?;
    session.post(&history(&folder_id, "/commits"), &json!({ "message": "Keep the Dar edit", "token": state["status"]["token"] })).await?.expect(201)?;
    let fresh = api("GET", &format!("{repo_api}/commits/{start_sha}"), None, 200).await?;
    session.post(&history(&folder_id, "/restore"), &json!({ "id": fresh["restore"]["changesetId"], "revision": fresh["restore"]["revision"] })).await?.expect(201)?;
    assert_eq!(session.get(&format!("/api/v1/files/{}/content", readme.file_id)).await?.expect(200)?.text(), "# Interop\n");
    let dirty_sha = git(&a, &["rev-parse", "origin/dirty"], true, None)?.out;
    assert!(api("GET", &format!("{repo_api}/commits/{dirty_sha}"), None, 200).await?["restore"].is_null(), "branch commits are not restore points");

    // The source filter covers the whole history and the three sources add up.
    let count = |query: &str| {
        let route = format!("{repo_api}/commits?ref=all{query}");
        let fut = api("GET", &route, None, 200);
        async move { Ok::<u64, Error>(fut.await?["total"].as_u64().unwrap_or(0)) }
    };
    let all = count("").await?;
    let pushed = count("&source=git").await?;
    let dar = count("&source=dar").await?;
    let agent = count("&source=agent").await?;
    assert_eq!(pushed + dar + agent, all);
    assert!(dar >= 3 && pushed >= 8);
    let dar_page = api("GET", &format!("{repo_api}/commits?ref=all&source=dar&limit=1&skip=1"), None, 200).await?;
    assert_eq!(dar_page["commits"].as_array().map(Vec::len), Some(1));
    assert_eq!(dar_page["commits"][0]["source"], json!("dar"));
    assert_eq!(dar_page["next"], json!(2));

    // Turning the app off hides the Git app; Git over HTTPS belongs to Files and keeps working.
    let toggle = |enabled: bool| {
        let session = session.clone();
        async move {
            let rev = session.get_json("/api/v1/admin/installedApps").await?["revision"].as_u64().context("apps revision")?;
            let reply = Reply::from(
                session
                    .request("PUT", "/api/v1/admin/installedApps")
                    .header("if-match", format!("\"apps:{rev}\""))
                    .json(&json!({ "id": "git", "enabled": enabled, "revision": rev }))
                    .send()
                    .await?,
            )
            .await?;
            reply.expect(200)?;
            Ok::<(), Error>(())
        }
    };
    toggle(false).await?;
    let off_check = async {
        api("GET", "/repositories", None, 404).await?;
        git(&work, &["clone", "-q", &url, "while-off"], true, None)?;
        Ok::<(), Error>(())
    }
    .await;
    toggle(true).await?;
    off_check?;
    assert!(api("GET", &format!("{repo_api}/branches"), None, 200).await?["main"].is_object());
    Ok(())
}
