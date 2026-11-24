//! Git app in the browser (`git-browser.mjs`): overview, repository tree and README, clone dialog,
//! file sheet, commit graph and list, branch compare/merge/delete, restore from commit, tag ZIP,
//! new repository creation, and responsive latte/mocha checks.

use crate::suites::agent_git_support::{
    create_app_password, create_user, run_git, sign_in_at, url_param,
};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;

pub const SUITE: Suite = Suite {
    name: "git-browser",
    cases: &[case!(git_app_browser_journey, 180)],
};

pub async fn git_app_browser_journey(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let username = format!("git-ui-{}", &uuid()[..6]);
    create_user(&admin, &username, "Git Tester", PASSWORD).await?;
    let user = login_as(&base, &username, PASSWORD).await?;
    let db = user.connect().await?;
    let folder_id = uuid();
    reducer(|done| {
        db.reducers
            .create_folder_then(folder_id.clone(), String::new(), "Site".into(), done)
    })
    .await?;
    db.disconnect();
    upload_with(
        &user,
        "README.md",
        "# Site\n\nPersonal site and blog.\n\n## Develop\n\nRun the build.\n",
        Upload {
            parent_id: &folder_id,
            ..Upload::default()
        },
    )
    .await?;
    upload_with(
        &user,
        "index.html",
        "<h1>Hi</h1>\n",
        Upload {
            parent_id: &folder_id,
            ..Upload::default()
        },
    )
    .await?;
    user.post(
        &format!("/api/v1/files/history/{folder_id}"),
        &json!({ "action": "track" }),
    )
    .await?
    .expect(200)?;
    let overview = user
        .get_json(&format!("/api/v1/files/history/{folder_id}"))
        .await?;
    user.post(
        &format!("/api/v1/files/history/{folder_id}/commits"),
        &json!({ "message": "Start the site", "token": overview["status"]["token"] }),
    )
    .await?
    .expect(201)?;
    let secret = create_app_password(&user, "Git", "write").await?;

    // A history with a merged branch, an open branch and a tag, pushed with Git.
    let work = t.temp("dar-git-ui")?;
    let alias = format!("{base}/dav/git/{username}/Site.git");
    let git = |cwd: &std::path::Path, args: &[&str]| -> Result<String> {
        Ok(run_git(
            cwd,
            &work,
            "Ada Lovelace",
            "ada@example.com",
            &username,
            &secret,
            args,
            true,
            None,
        )?
        .out)
    };
    git(&work, &["clone", "-q", &alias, "site"])?;
    let site = work.join("site");
    git(&site, &["checkout", "-q", "-b", "feature/search"])?;
    std::fs::write(site.join("search.js"), "export const search=()=>[];\n")?;
    git(&site, &["add", "-A"])?;
    git(&site, &["commit", "-qm", "Add search"])?;
    git(&site, &["checkout", "-q", "main"])?;
    std::fs::write(site.join("index.html"), "<h1>Hello</h1>\n<p>Welcome</p>\n")?;
    git(&site, &["commit", "-qam", "Greet visitors"])?;
    git(
        &site,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "Merge branch 'feature/search'",
            "feature/search",
        ],
    )?;
    git(&site, &["tag", "-a", "v1.0", "-m", "First version"])?;
    git(&site, &["checkout", "-q", "-b", "feature/rss"])?;
    std::fs::write(site.join("rss.xml"), "<rss/>\n")?;
    git(&site, &["add", "-A"])?;
    git(&site, &["commit", "-qm", "Add a feed"])?;
    git(&site, &["checkout", "-q", "-b", "feature/old", "main"])?;
    std::fs::write(site.join("old.txt"), "old\n")?;
    git(&site, &["add", "-A"])?;
    git(&site, &["commit", "-qm", "Old work"])?;
    git(
        &site,
        &[
            "push",
            "-q",
            "origin",
            "main",
            "feature/rss",
            "feature/old",
            "v1.0",
        ],
    )?;

    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 1000))
        .await?;
    context
        .grant_permissions(&["clipboard-read", "clipboard-write"], Some(&base))
        .await?;
    let page = context.new_page().await?;
    let errors = page.collect_errors(|text| text.contains("401") && text.contains("Unauthorized"));

    sign_in_at(&page, &format!("{base}/git"), &username, PASSWORD).await?;

    // Overview: a year of activity, the repository card and recent commits.
    let main = page.get_by_role("main", "Overview");
    main.get_by_text("6 commits in the last year")
        .wait_for()
        .await?;
    main.get_by_role("region", "Recent activity")
        .get_by_text("Add a feed")
        .wait_for()
        .await?;
    page.get_by_role("navigation", contains("Git views"))
        .get_by_role("button", contains("Repositories"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("git-overview-1440.png"), false)
        .await?;

    // A repository opens on Code: files with their last commit, then the README.
    main.get_by_role("region", "Repositories")
        .get_by_role("button", regex("^Site"))
        .click()
        .await?;
    let repo = page.get_by_role("main", "Site");
    repo.get_by_role("heading", contains("Site"))
        .level(1)
        .wait_for()
        .await?;
    repo.get_by_role("button", "index.html").wait_for().await?;
    repo.get_by_role("region", "README.md")
        .get_by_role("heading", contains("Develop"))
        .wait_for()
        .await?;
    assert_eq!(
        url_param(&page.url(), "repo").as_deref(),
        Some(folder_id.as_str())
    );

    // Clone: the alias and the permanent address; Escape closes and returns focus.
    repo.get_by_role("button", "Clone").click().await?;
    let clone_dialog = page.get_by_role("dialog", "Clone");
    assert_eq!(
        clone_dialog
            .get_by_label(contains("Clone with HTTPS"))
            .input_value()
            .await?,
        alias
    );
    assert_eq!(
        clone_dialog
            .get_by_label(regex("^Permanent address"))
            .input_value()
            .await?,
        format!("{base}/dav/git/{folder_id}.git")
    );
    page.screenshot(t.screenshot("git-clone-1440.png"), false)
        .await?;
    clone_dialog
        .get_by_role("button", "Copy clone address")
        .click()
        .await?;
    page.get_by_text("Copied clone address").wait_for().await?;
    page.keyboard().press("Escape").await?;
    clone_dialog.wait_for_state(State::Detached).await?;
    assert_eq!(
        repo.get_by_role("button", "Clone")
            .evaluate("(el) => el === document.activeElement", Value::Null)
            .await?,
        json!(true)
    );

    // A file opens in the right sheet.
    repo.get_by_role("button", "index.html").click().await?;
    let file = page.get_by_role("complementary", "File");
    file.get_by_label(contains("Contents of index.html"))
        .filter_has_text("<p>Welcome</p>")
        .wait_for()
        .await?;
    page.keyboard().press("Escape").await?;
    file.wait_for_state(State::Detached).await?;

    // Commits: the graph; a merge commit opens with its parents and diff.
    repo.get_by_role("tab", regex("^Commits")).click().await?;
    let commits = repo.get_by_role("list", "Commits");
    commits
        .get_by_role("button", regex("Merge branch 'feature/search'"))
        .click()
        .await?;
    let commit = page.get_by_role("complementary", "Commit");
    commit.get_by_text("Parents").wait_for().await?;
    commit
        .get_by_role("region", "Changes to search.js")
        .get_by_text(contains("export const search=()=>[];"))
        .wait_for()
        .await?;
    assert!(commits.locator(".git-lanes").count().await? >= 5);
    let sha_re = regex::Regex::new("^[0-9a-f]{40}$").unwrap();
    let commit_param = url_param(&page.url(), "commit").unwrap_or_default();
    assert!(
        sha_re.is_match(&commit_param),
        "unexpected commit query param: {commit_param:?}"
    );
    page.screenshot(t.screenshot("git-graph-1440.png"), false)
        .await?;
    repo.get_by_role("radio", "List").check().await?;
    repo.get_by_role("heading", ())
        .level(3)
        .first()
        .wait_for()
        .await?;
    page.keyboard().press("Escape").await?;

    // Branches: divergence, then merging from the web (fast-forward, deleting the branch) and deleting another branch.
    repo.get_by_role("tab", regex("^Branches")).click().await?;
    let branches = repo.get_by_role("region", "Branches");
    branches
        .get_by_role("listitem", ())
        .filter_has_text("feature/rss")
        .get_by_text("1 ahead")
        .wait_for()
        .await?;
    branches
        .get_by_role("listitem", ())
        .filter_has_text("feature/rss")
        .get_by_role("button", "Compare")
        .click()
        .await?;
    let compare = page.get_by_role("complementary", "Compare");
    let merge_box = compare.get_by_role("region", "Merge feature/rss into main");
    compare.get_by_text("rss.xml").wait_for().await?;
    assert!(
        merge_box
            .get_by_role("radio", "Fast-forward")
            .is_checked()
            .await?
    );
    assert!(
        merge_box
            .get_by_label(contains("Delete feature/rss after merging"))
            .is_checked()
            .await?
    );
    page.screenshot(t.screenshot("git-branches-1440.png"), false)
        .await?;
    merge_box
        .get_by_role("button", "Fast-forward main")
        .click()
        .await?;
    page.get_by_text("main now points at feature/rss")
        .wait_for()
        .await?;
    compare.wait_for_state(State::Detached).await?;
    branches
        .get_by_role("listitem", ())
        .filter_has_text("feature/rss")
        .wait_for_state(State::Detached)
        .await?;
    let tree_after = user
        .get_json(&format!("/api/v1/git/repositories/{folder_id}/tree"))
        .await?;
    assert!(
        tree_after["entries"]
            .as_array()
            .is_some_and(|a| a.iter().any(|e| e["name"] == "rss.xml")),
        "main has the merged file"
    );
    branches
        .get_by_role("button", "Actions for feature/old")
        .click()
        .await?;
    page.get_by_role("menuitem", contains("Delete branch"))
        .click()
        .await?;
    let confirm = page.get_by_role("dialog", "Delete feature/old?");
    confirm
        .get_by_text(regex("1 commit is only on feature/old"))
        .wait_for()
        .await?;
    confirm
        .get_by_role("button", "Delete branch")
        .click()
        .await?;
    page.get_by_text("Branch deleted").wait_for().await?;
    branches
        .get_by_text(regex("Push a branch with Git"))
        .wait_for()
        .await?;
    let ls_remote = git(&site, &["ls-remote", &alias])?;
    assert!(!ls_remote.contains("feature/rss") && !ls_remote.contains("feature/old"));

    // Restore from a commit's details: the folder goes back and a restore commit is recorded.
    repo.get_by_role("tab", regex("^Commits")).click().await?;
    repo.get_by_role("button", regex("^Start the site"))
        .click()
        .await?;
    page.get_by_role("complementary", "Commit")
        .get_by_role("button", "Restore folder to this point")
        .click()
        .await?;
    page.get_by_role("dialog", "Restore this folder?")
        .get_by_role("button", "Restore folder")
        .click()
        .await?;
    page.get_by_text("Folder restored to “Start the site”")
        .wait_for()
        .await?;
    repo.get_by_role("button", regex("^Restore to “Start the site”"))
        .wait_for()
        .await?;
    let restored_tree = user
        .get_json(&format!("/api/v1/git/repositories/{folder_id}/tree"))
        .await?;
    let restored_names: Vec<&str> = restored_tree["entries"]
        .as_array()
        .context("entries")?
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(restored_names, vec!["index.html", "README.md"]);
    page.keyboard().press("Escape").await?;

    // Tags: a ZIP of the tag.
    repo.get_by_role("tab", regex("^Tags")).click().await?;
    let zip = repo.get_by_role("link", "ZIP");
    let href = zip.get_attribute("href").await?.context("zip href")?;
    let response = page.request().get(&format!("{base}{href}")).await?;
    assert_eq!(response.status(), 200);
    assert_eq!(&response.body[..2], b"PK");

    // A new repository: a folder with a README and its first commit.
    page.get_by_role("button", "New repository").click().await?;
    let pane = page.get_by_role("complementary", "New repository");
    pane.get_by_label("Name").fill("Notes repo").await?;
    pane.get_by_role("button", "Create").click().await?;
    pane.wait_for_state(State::Detached).await?;
    let created = page.get_by_role("main", "Notes repo");
    created
        .get_by_role("region", "README.md")
        .get_by_role("heading", contains("Notes repo"))
        .wait_for()
        .await?;
    page.get_by_role("navigation", "Repositories")
        .get_by_role("button", contains("Notes repo"))
        .wait_for()
        .await?;

    // Themes and widths: nothing scrolls sideways.
    page.get_by_role("navigation", "Repositories")
        .get_by_role("button", contains("Site"))
        .click()
        .await?;
    page.get_by_role("main", "Site")
        .get_by_role("tab", regex("^Commits"))
        .click()
        .await?;
    page.get_by_role("main", "Site")
        .get_by_role("radio", "Graph")
        .check()
        .await?;
    for theme in ["latte", "mocha"] {
        for width in [1440u32, 390] {
            page.evaluate_with(
                "(value) => { document.documentElement.dataset.theme = value; }",
                json!(theme),
            )
            .await?;
            page.set_viewport_size(width, 844).await?;
            assert_eq!(
                page.evaluate("() => document.documentElement.scrollWidth > innerWidth")
                    .await?,
                json!(false),
                "{theme} {width}"
            );
            page.screenshot(
                t.screenshot(&format!("git-commits-{theme}-{width}.png")),
                false,
            )
            .await?;
        }
    }
    page.goto(&format!("{base}/git")).await?;
    page.get_by_text("7 commits in the last year")
        .wait_for()
        .await?;
    page.evaluate("() => { document.documentElement.dataset.theme = 'mocha'; }")
        .await?;
    for width in [1440u32, 390] {
        page.set_viewport_size(width, 844).await?;
        assert_eq!(
            page.evaluate("() => document.documentElement.scrollWidth > innerWidth")
                .await?,
            json!(false)
        );
        page.screenshot(
            t.screenshot(&format!("git-overview-mocha-{width}.png")),
            false,
        )
        .await?;
    }

    context.close().await?;
    assert_eq!(*errors.lock().unwrap(), Vec::<String>::new());
    Ok(())
}
