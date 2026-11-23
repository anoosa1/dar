//! Folder history in the browser (`history-browser.mjs`): tracking, review and commit pane,
//! Git clone address and pushed tag, snapshots, restore, keyboard dismissal, responsive themes,
//! and stopping history.

use crate::suites::agent_git_support::{create_app_password, create_user, run_git, sign_in_at};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;

pub const SUITE: Suite = Suite {
    name: "history-browser",
    cases: &[case!(folder_history_browser_journey, 180)],
};

pub async fn folder_history_browser_journey(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let username = format!("history-ui-{}", &uuid()[..6]);
    create_user(&admin, &username, "History tester", PASSWORD).await?;
    let native = login_as(&base, &username, PASSWORD).await?;
    let db = native.connect().await?;
    let folder_id = uuid();
    reducer(|done| db.reducers.create_folder_then(folder_id.clone(), String::new(), "Project".into(), done)).await?;
    db.disconnect();
    let readme = upload_with(&native, "README.md", "# Project\n\nFirst line\n", Upload { parent_id: &folder_id, ..Upload::default() }).await?;
    upload_with(&native, "photo.bin", [0u8, 1, 2, 3], Upload { parent_id: &folder_id, ..Upload::default() }).await?;

    let browser = Browser::shared().await?;
    let context = browser.new_context(ContextOptions::sized(1440, 1000)).await?;
    let page = context.new_page().await?;
    let errors = page.collect_errors(|text| text.contains("401") && text.contains("Unauthorized"));

    sign_in_at(&page, &base, &username, PASSWORD).await?;
    ui::file_details(&page, "Project").await?;
    let details = page.get_by_role("complementary", "File details");
    details.get_by_role("tab", "History").click().await?;
    details.get_by_role("button", "Track history").click().await?;
    details.get_by_text("History is on for this folder.").wait_for().await?;
    details.get_by_text("README.md").wait_for().await?;
    page.screenshot(t.screenshot("history-status-1440.png"), false).await?;

    // Review the changes in the editing pane, then commit with a message.
    details.get_by_role("button", "Review and commit").click().await?;
    let pane = page.get_by_role("complementary", "Commit changes");
    assert_eq!(pane.get_by_label("Message").evaluate("(el) => el === document.activeElement", Value::Null).await?, json!(true));
    pane.get_by_role("button", "README.md").click().await?;
    pane.get_by_role("region", "Changes in README.md").get_by_text("First line").wait_for().await?;
    pane.get_by_role("button", "photo.bin").click().await?;
    pane.get_by_text(regex("No text preview")).wait_for().await?;
    pane.get_by_label("Message").fill("Start the project").await?;
    page.screenshot(t.screenshot("history-commit-1440.png"), false).await?;
    pane.get_by_role("button", "Commit").click().await?;
    pane.wait_for_state(State::Detached).await?;
    details.get_by_text("Committed 2 changes.").wait_for().await?;
    details.get_by_text("Everything is committed.").wait_for().await?;
    details.get_by_role("button", regex("^Start the project")).wait_for().await?;

    // Git: the clone address, and a tag pushed with Git listed with the branches and tags.
    let git = details.locator(".history-git");
    assert_eq!(git.get_by_label("Clone address").input_value().await?, format!("{base}/dav/git/{folder_id}.git"));
    let secret = create_app_password(&native, "Git", "write").await?;
    let work = t.temp("git-ui")?;
    let git_url = format!("{base}/dav/git/{folder_id}.git");
    run_git(&work, &work, "Tester", "t@example.com", &username, &secret, &["clone", "-q", &git_url, "repo"], true, None)?;
    run_git(&work, &work, "Tester", "t@example.com", &username, &secret, &["-C", "repo", "tag", "-a", "v1.0", "-m", "First release"], true, None)?;
    run_git(&work, &work, "Tester", "t@example.com", &username, &secret, &["-C", "repo", "push", "-q", "origin", "v1.0"], true, None)?;
    details.get_by_role("tab", "Details").click().await?;
    details.get_by_role("tab", "History").click().await?;
    details.get_by_role("list", "Branches and tags").get_by_text("v1.0").wait_for().await?;

    // A change made elsewhere shows up; a snapshot commits it without a message.
    upload_with(&native, "README.md", "# Project\n\nSecond line\n", Upload { parent_id: &folder_id, file_id: Some(&readme.file_id), expected_revision: Some(1), ..Upload::default() }).await?;
    details.get_by_role("button", "Snapshot").wait_for().await?;
    details.get_by_role("button", "Snapshot").click().await?;
    details.get_by_text("Snapshot of 1 change committed.").wait_for().await?;

    // Restoring the first commit brings back its content and records a restore.
    details.get_by_role("button", regex("^Start the project")).click().await?;
    let changeset = page.get_by_role("complementary", "Commit");
    let sha = changeset.locator(".history-sha").text_content().await?;
    let sha_re = regex::Regex::new("^[0-9a-f]{7}$").unwrap();
    assert!(sha_re.is_match(sha.trim()), "a commit Git has seen shows its ID, got {sha:?}");
    changeset.get_by_role("button", "README.md").click().await?;
    changeset.get_by_role("region", "Changes in README.md").wait_for().await?;
    changeset.get_by_role("button", "Restore folder to this point").click().await?;
    let confirm = page.get_by_role("dialog", "Restore this folder?");
    confirm.get_by_role("button", "Restore folder").click().await?;
    details.get_by_text("Folder restored to “Start the project”.").wait_for().await?;
    details.get_by_role("button", regex("^Restore to “Start the project”")).wait_for().await?;
    assert_eq!(native.get(&format!("/api/v1/files/{}/content", readme.file_id)).await?.expect(200)?.text(), "# Project\n\nFirst line\n");

    // Keyboard: Escape closes an opened commit and returns focus to the history.
    details.get_by_role("button", regex("^Snapshot")).last().click().await?;
    page.get_by_role("complementary", "Snapshot").wait_for().await?;
    page.keyboard().press("Escape").await?;
    page.get_by_role("complementary", "Snapshot").wait_for_state(State::Detached).await?;

    for theme in ["latte", "mocha"] {
        for width in [1440u32, 390] {
            page.evaluate_with("(value) => { document.documentElement.dataset.theme = value; }", json!(theme)).await?;
            page.set_viewport_size(width, 844).await?;
            assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false), "{theme} {width}");
            page.screenshot(t.screenshot(&format!("history-{theme}-{width}.png")), false).await?;
            git.scroll_into_view_if_needed().await?;
            page.screenshot(t.screenshot(&format!("history-git-{theme}-{width}.png")), false).await?;
        }
    }
    page.set_viewport_size(1440, 1000).await?;

    // Stopping history asks first and returns the folder to untracked.
    details.get_by_role("button", "Stop keeping history").click().await?;
    page.get_by_role("dialog", "Stop keeping history?").get_by_role("button", "Delete history").click().await?;
    details.get_by_role("button", "Track history").wait_for().await?;

    context.close().await?;
    assert_eq!(*errors.lock().unwrap(), Vec::<String>::new());
    Ok(())
}
