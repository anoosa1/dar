//! Git issues and pull requests in the browser (`git-tracker-browser.mjs`): labels, creating
//! and commenting on issues, pull request comparison and merge closing an issue, deep-link
//! reload, phone layout, and shared reader permissions.

use crate::suites::agent_git_support::{
    create_app_password, create_user, run_git, sign_in_at, url_param,
};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::sync::{Arc, Mutex};

pub const SUITE: Suite = Suite {
    name: "git-tracker-browser",
    cases: &[case!(git_tracker_browser_journey, 180)],
};

pub async fn git_tracker_browser_journey(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let owner = format!("tracker-ui-{}", &uuid()[..6]);
    let reader = format!("reader-ui-{}", &uuid()[..6]);
    create_user(&admin, &owner, "Grace Hopper", PASSWORD).await?;
    create_user(&admin, &reader, "Katherine Johnson", PASSWORD).await?;

    let user = login_as(&base, &owner, PASSWORD).await?;
    let db = user.connect().await?;
    let folder_id = uuid();
    reducer(|done| {
        db.reducers
            .create_folder_then(folder_id.clone(), String::new(), "Compiler".into(), done)
    })
    .await?;
    upload_with(
        &user,
        "README.md",
        "# Compiler\n",
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
        &json!({ "message": "Start", "token": overview["status"]["token"] }),
    )
    .await?
    .expect(201)?;
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
    let folder = db
        .db
        .my_files()
        .iter()
        .find(|row| row.id == folder_id)
        .context("folder row")?;
    reducer(|done| {
        db.reducers.grant_access_then(
            uuid(),
            folder_id.clone(),
            reader.clone(),
            "read".into(),
            folder.revision,
            done,
        )
    })
    .await?;
    db.disconnect();

    let secret = create_app_password(&user, "Git", "write").await?;
    let work = t.temp("dar-tracker-ui")?;
    let git = |cwd: &std::path::Path, args: &[&str]| -> Result<String> {
        Ok(run_git(
            cwd,
            &work,
            "Grace Hopper",
            "grace@example.com",
            &owner,
            &secret,
            args,
            true,
            None,
        )?
        .out)
    };
    let repo_url = format!("{base}/dav/git/{owner}/Compiler.git");
    git(&work, &["clone", "-q", &repo_url, "compiler"])?;
    let clone = work.join("compiler");
    git(&clone, &["checkout", "-q", "-b", "fix/overflow"])?;
    std::fs::write(
        clone.join("lexer.c"),
        "int read(void){\n  return checked();\n}\n",
    )?;
    git(&clone, &["add", "-A"])?;
    git(&clone, &["commit", "-qm", "Check token length"])?;
    git(&clone, &["push", "-q", "origin", "fix/overflow"])?;

    let browser = Browser::shared().await?;
    let all_errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let issues_url = format!("{base}/git?repo={folder_id}&tab=issues");

    let sign_in_page = |context: BrowserContext,
                        username: String,
                        sink: Arc<Mutex<Vec<String>>>,
                        target: String| async move {
        let page = context.new_page().await?;
        let s1 = sink.clone();
        page.on_page_error(move |msg| s1.lock().unwrap().push(msg.clone()));
        let s2 = sink;
        page.on_console(move |msg| {
            if msg.kind == "error"
                && !(msg.text.contains("401") && msg.text.contains("Unauthorized"))
            {
                s2.lock().unwrap().push(msg.text.clone());
            }
        });
        sign_in_at(&page, &target, &username, PASSWORD).await?;
        Ok::<Page, Error>(page)
    };

    let context = browser
        .new_context(ContextOptions::sized(1440, 1000))
        .await?;
    let page = sign_in_page(
        context.clone(),
        owner.clone(),
        all_errors.clone(),
        issues_url.clone(),
    )
    .await?;
    let repo = page.get_by_role("main", "Compiler");
    repo.get_by_role("tab", regex("^Issues")).wait_for().await?;
    repo.get_by_role("heading", contains("No open issues"))
        .wait_for()
        .await?;

    // Labels, in the right editing pane.
    repo.get_by_role("button", "Labels").click().await?;
    let labels = page.get_by_role("complementary", "Labels");
    labels.get_by_label("Name").fill("bug").await?;
    labels
        .get_by_label("Description")
        .fill("Something is wrong")
        .await?;
    labels
        .get_by_role("radio", contains("Rose"))
        .check()
        .await?;
    labels
        .get_by_role("button", contains("Create label"))
        .click()
        .await?;
    labels
        .get_by_role("list", ())
        .get_by_text("bug")
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("git-labels-1440.png"), false)
        .await?;
    labels
        .get_by_role("button", contains("Close"))
        .first()
        .click()
        .await?;
    labels.wait_for_state(State::Detached).await?;

    // A new issue, with a label and a Markdown description; it opens in the right sheet.
    repo.get_by_role("button", "New issue").click().await?;
    let editor = page.get_by_role("complementary", "New issue");
    editor
        .get_by_label("Title")
        .fill("Long identifiers overflow the buffer")
        .await?;
    editor
        .get_by_label("Description")
        .fill("Names over **255** characters crash the lexer.")
        .await?;
    editor.get_by_role("checkbox", regex("bug")).check().await?;
    editor
        .get_by_role("radio", contains("Preview"))
        .click()
        .await?;
    editor.get_by_text("255").wait_for().await?;
    editor
        .get_by_role("button", contains("Open issue"))
        .click()
        .await?;
    editor.wait_for_state(State::Detached).await?;
    let sheet = page.get_by_role("complementary", "Conversation");
    sheet
        .get_by_role("heading", contains("Issue #1"))
        .wait_for()
        .await?;
    sheet
        .get_by_text(contains("Long identifiers overflow the buffer"))
        .wait_for()
        .await?;
    assert_eq!(url_param(&page.url(), "issue").as_deref(), Some("1"));
    repo.get_by_role("list", contains("Issues"))
        .get_by_role("button", regex("Long identifiers overflow the buffer"))
        .wait_for()
        .await?;

    // Comment, then close with a comment and reopen.
    sheet
        .get_by_label("Add a comment")
        .fill("Reproduced with a 300 character name.")
        .await?;
    sheet.get_by_role("button", "Comment").click().await?;
    sheet
        .get_by_role("article", contains("Comment by Grace Hopper"))
        .get_by_text(contains("Reproduced with a 300 character name."))
        .wait_for()
        .await?;
    sheet
        .get_by_role("button", contains("Close issue"))
        .click()
        .await?;
    sheet
        .get_by_text(contains("closed this"))
        .wait_for()
        .await?;
    sheet
        .get_by_role("button", contains("Reopen"))
        .click()
        .await?;
    sheet
        .get_by_text(contains("reopened this"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("git-issue-1440.png"), false)
        .await?;

    // A pull request from the pushed branch: the pane compares it; "Closes #1" is in the description.
    repo.get_by_role("tab", regex("^Pull requests"))
        .click()
        .await?;
    repo.get_by_role("button", "New pull request")
        .click()
        .await?;
    let pull_editor = page.get_by_role("complementary", "New pull request");
    pull_editor
        .get_by_text(contains("1 commit · 1 file · Can fast-forward"))
        .wait_for()
        .await?;
    assert_eq!(
        pull_editor.get_by_label("Title").input_value().await?,
        "Check token length"
    );
    pull_editor
        .get_by_label("Description")
        .fill("Limits names to 255 characters. Closes #1.")
        .await?;
    page.screenshot(t.screenshot("git-new-pull-1440.png"), false)
        .await?;
    pull_editor
        .get_by_role("button", contains("Open pull request"))
        .click()
        .await?;
    pull_editor.wait_for_state(State::Detached).await?;
    sheet
        .get_by_role("heading", contains("Pull request #2"))
        .wait_for()
        .await?;
    sheet
        .get_by_role("radio", regex("^Commits"))
        .click()
        .await?;
    sheet
        .get_by_role("button", regex("Check token length"))
        .wait_for()
        .await?;
    sheet.get_by_role("radio", regex("^Files")).click().await?;
    sheet
        .get_by_role("region", contains("Changes to lexer.c"))
        .get_by_text(contains("return checked();"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("git-pull-files-1440.png"), false)
        .await?;
    sheet.get_by_role("radio", "Unified").click().await?;
    sheet
        .get_by_role("region", contains("Changes to lexer.c"))
        .get_by_text(contains("return checked();"))
        .wait_for()
        .await?;
    sheet.get_by_role("radio", "Split").click().await?;
    sheet
        .get_by_role("region", contains("Changes to lexer.c"))
        .get_by_text(contains("return checked();"))
        .wait_for()
        .await?;
    sheet.get_by_role("button", "Viewed").click().await?;
    sheet.get_by_role("button", "✓ Viewed").wait_for().await?;
    sheet
        .get_by_role("radio", contains("Conversation"))
        .click()
        .await?;
    sheet
        .get_by_role("region", contains("Merge fix/overflow into main"))
        .get_by_role("radio", contains("Merge commit"))
        .click()
        .await?;
    sheet
        .get_by_role("button", contains("Merge into main"))
        .click()
        .await?;
    sheet.get_by_text("Merged").wait_for().await?;
    sheet
        .get_by_text(regex("merged this as"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("git-pull-merged-1440.png"), false)
        .await?;
    // The issue it named is closed, and #1 in the description opens it.
    sheet.get_by_role("button", "#1").click().await?;
    sheet
        .get_by_role("heading", contains("Issue #1"))
        .wait_for()
        .await?;
    sheet.get_by_text("Closed").wait_for().await?;
    sheet
        .get_by_text(regex("closed this with"))
        .wait_for()
        .await?;

    // Saved: a reload reopens the same issue from the address.
    page.reload().await?;
    page.get_by_role("complementary", "Conversation")
        .get_by_role("heading", contains("Issue #1"))
        .wait_for()
        .await?;
    page.get_by_role("complementary", "Conversation")
        .get_by_text(contains("Reproduced with a 300 character name."))
        .wait_for()
        .await?;

    // Phones: the list fills the screen; an issue opens full screen with Back.
    let phone = browser.new_context(ContextOptions::phone(390, 844)).await?;
    let small = sign_in_page(
        phone.clone(),
        owner.clone(),
        all_errors.clone(),
        issues_url.clone(),
    )
    .await?;
    small.get_by_role("tab", regex("^Issues")).click().await?;
    small.get_by_role("radio", regex("Closed")).click().await?;
    small
        .get_by_role("list", contains("Issues"))
        .get_by_role("button", regex("Long identifiers"))
        .wait_for()
        .await?;
    small
        .screenshot(t.screenshot("git-issues-390.png"), false)
        .await?;
    small
        .get_by_role("list", contains("Issues"))
        .get_by_role("button", regex("Long identifiers"))
        .click()
        .await?;
    let small_sheet = small.get_by_role("complementary", "Conversation");
    small_sheet
        .get_by_role("heading", contains("Issue #1"))
        .wait_for()
        .await?;
    assert_eq!(
        small
            .evaluate("() => document.documentElement.scrollWidth <= innerWidth + 1")
            .await?,
        json!(true),
        "no horizontal scroll on phones"
    );
    small
        .screenshot(t.screenshot("git-issue-390.png"), false)
        .await?;
    phone.close().await?;

    // A reader sees the shared repository, comments, and has no label, close or merge controls.
    let other = browser
        .new_context(ContextOptions::sized(1440, 1000))
        .await?;
    let page2 = sign_in_page(
        other.clone(),
        reader.clone(),
        all_errors.clone(),
        issues_url.clone(),
    )
    .await?;
    page2
        .get_by_role("navigation", contains("Shared repositories"))
        .get_by_role("button", regex("Compiler"))
        .wait_for()
        .await?;
    let shared = page2.get_by_role("main", "Compiler");
    shared
        .get_by_text(regex("Shared by Grace Hopper"))
        .wait_for()
        .await?;
    assert_eq!(shared.get_by_role("button", "Labels").count().await?, 0);
    shared.get_by_role("tab", regex("^Issues")).click().await?;
    shared.get_by_role("radio", regex("Closed")).click().await?;
    shared
        .get_by_role("list", contains("Issues"))
        .get_by_role("button", regex("Long identifiers"))
        .click()
        .await?;
    let reader_sheet = page2.get_by_role("complementary", "Conversation");
    reader_sheet
        .get_by_label("Add a comment")
        .fill("Thanks for the fix.")
        .await?;
    reader_sheet
        .get_by_role("button", "Comment")
        .click()
        .await?;
    reader_sheet
        .get_by_text(contains("Thanks for the fix."))
        .wait_for()
        .await?;
    assert_eq!(
        reader_sheet
            .get_by_role("button", contains("Reopen"))
            .count()
            .await?,
        0
    );
    assert_eq!(
        reader_sheet
            .get_by_role("button", contains("Labels"))
            .count()
            .await?,
        0
    );
    other.close().await?;

    context.close().await?;
    assert_eq!(*all_errors.lock().unwrap(), Vec::<String>::new());
    Ok(())
}
