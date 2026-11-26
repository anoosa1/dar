//! Port of `test/workspace-browser.mjs`: Notes (files in `docs/notes`: folders, heading names,
//! Markdown preview XSS safety, favorites, deep links, external Files edits, delete) and Tasks
//! (desktop and phone viewports, recurrence completion/advance/reopen, task dates/status,
//! keyboard focus and console errors).

use dar_acceptance::prelude::*;
use dar_acceptance::ui::Choice;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "workspace-browser",
    cases: &[case!(notes_and_tasks_journey, 180)],
};

pub async fn notes_and_tasks_journey(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 950))
        .await?;
    let page = context.new_page().await?;

    let failures = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let sink = failures.clone();
        page.on_page_error(move |msg| sink.lock().unwrap().push(msg.clone()));
    }
    {
        let sink = failures.clone();
        page.on_console(move |msg| {
            if msg.kind == "error"
                && !(msg.text.contains("401") && msg.text.contains("Unauthorized"))
            {
                sink.lock().unwrap().push(msg.text.clone());
            }
        });
    }

    page.goto(&format!("{base}/notes")).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.get_by_role("heading", "All notes").wait_for().await?;

    let api = page.request();
    let native_session = api
        .get(&format!("{base}/api/v1/auth/session"))
        .await?
        .json()?;
    let csrf = native_session["csrfToken"]
        .as_str()
        .context("csrfToken")?
        .to_owned();
    let api_headers = [("origin", base.as_str()), ("x-csrf-token", csrf.as_str())];

    // Notes are Markdown files in Files › docs/notes, created on first use.
    let library = || async {
        api.get(&format!("{base}/api/v1/notes/library"))
            .await?
            .json()
    };
    assert_eq!(library().await?["path"], "docs/notes");

    let name = format!("Browser notebook {}", &uuid()[..6]);
    page.get_by_role("button", "New folder").click().await?;
    let mut dialog = page.locator("[data-editor-pane]");
    dialog.get_by_label("Name").fill(&name).await?;
    dialog
        .get_by_role("button", "Create folder")
        .click()
        .await?;
    dialog.wait_for_state(State::Hidden).await?;

    page.get_by_role("list", "Folders")
        .get_by_role("button", &name)
        .click()
        .await?;
    page.get_by_role("heading", &name).wait_for().await?;
    page.get_by_role("button", "New note")
        .first()
        .click()
        .await?;
    let editor = page.get_by_role("region", "Editor for Untitled.md");
    editor.wait_for().await?;

    let markdown_input = ui::note_source(&editor).await?;
    markdown_input.fill("selected words").await?;
    ui::note_select(&markdown_input, Some(0), None).await?;
    editor.get_by_role("button", "Bold").click().await?;
    assert_eq!(ui::note_text(&markdown_input).await?, "**selected words**");

    let title = format!("Browser writing {}", &uuid()[..6]);
    let source = format!(
        "# {title}\n\n**Keep this safe**\n\n- [ ] Plant herbs\n\n<script>window.noteInjected=true</script>\n\n[unsafe](javascript:alert(1))"
    );
    markdown_input.fill(&source).await?;
    editor
        .get_by_text("All changes saved")
        .timeout(Duration::from_secs(10))
        .wait_for()
        .await?;
    // Live preview styles the Markdown in place; source HTML stays text and unsafe links are inert.
    editor
        .locator(".cm-dar-strong")
        .filter_has_text("Keep this safe")
        .wait_for()
        .await?;
    editor
        .locator(".cm-dar-link.is-plain")
        .filter_has_text("unsafe")
        .wait_for()
        .await?;
    editor
        .locator(".cm-line")
        .filter_has_text("<script>window.noteInjected=true</script>")
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("notes-editor-live.png"), true)
        .await?;
    editor.get_by_role("button", "Reading").click().await?;
    editor
        .locator(".markdown-preview")
        .get_by_text("Keep this safe")
        .wait_for()
        .await?;
    assert_eq!(
        page.evaluate("() => window.noteInjected").await?,
        Value::Null
    );
    assert_eq!(
        editor.locator(r#"a[href^="javascript:"]"#).count().await?,
        0
    );
    page.screenshot(t.screenshot("notes-editor-desktop.png"), true)
        .await?;

    // Closing names the new file after its heading; the row shows the heading and an excerpt.
    editor.get_by_role("button", "Close editor").click().await?;
    let row = page.locator(".workspace-row").filter_has_text(&title);
    row.get_by_text(contains("Keep this safe Plant herbs"))
        .wait_for()
        .await?;

    let expected_name = format!("{title}.md");
    let note = eventually(
        Duration::from_secs(5),
        "note renamed after heading",
        || async {
            let lib = library().await?;
            Ok(lib["notes"].as_array().and_then(|notes| {
                notes
                    .iter()
                    .find(|n| n["name"] == expected_name.as_str())
                    .cloned()
            }))
        },
    )
    .await?;
    assert_eq!(note["path"], format!("{name}/{title}.md"));
    let note_id = note["id"].as_str().context("note id")?.to_owned();
    let note_parent = note["parentId"]
        .as_str()
        .context("note parentId")?
        .to_owned();

    ui::item_action(&page, &title, "Add to favorites").await?;
    page.get_by_role("button", "Favorites").click().await?;
    page.locator(".workspace-row")
        .filter_has_text(&title)
        .wait_for()
        .await?;

    // Deep links open the note; an edit made through Files shows after a refresh.
    page.goto(&format!("{base}/notes?note={note_id}")).await?;
    let deep_editor = page.get_by_role("region", format!("Editor for {title}.md"));
    deep_editor.wait_for().await?;
    // On a phone the live editor fills the sheet without sideways scrolling.
    page.set_viewport_size(390, 844).await?;
    ui::note_source(&deep_editor).await?;
    deep_editor
        .locator(".cm-line.cm-dar-h1")
        .filter_has_text(&title)
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("notes-editor-phone.png"), true)
        .await?;
    assert_eq!(
        page.evaluate("() => document.documentElement.scrollWidth > innerWidth")
            .await?,
        json!(false)
    );
    page.set_viewport_size(1440, 950).await?;
    page.get_by_role("button", "Close editor").click().await?;

    let latest = library().await?["notes"]
        .as_array()
        .and_then(|notes| notes.iter().find(|n| n["id"] == note_id.as_str()).cloned())
        .context("latest note")?;
    let edited_body = format!("# {title}\n\nEdited elsewhere.\n");
    let reserve = api
        .post(
            &format!("{base}/api/v1/files/uploads"),
            &api_headers,
            json!({
                "name": format!("{title}.md"),
                "size": edited_body.len(),
                "mediaType": "text/markdown",
                "parentId": note_parent,
                "fileId": note_id,
                "expectedRevision": latest["revision"],
            }),
        )
        .await?;
    assert_eq!(reserve.status(), 201);
    let upload_id = reserve.json()?["id"]
        .as_str()
        .context("upload id")?
        .to_owned();
    let uploaded = api
        .put_bytes(
            &format!("{base}/api/v1/files/uploads/{upload_id}"),
            &api_headers,
            edited_body.into_bytes(),
        )
        .await?;
    assert_eq!(uploaded.status(), 200);

    page.reload().await?;
    page.locator(".workspace-row")
        .filter_has_text(&title)
        .get_by_text("Edited elsewhere.")
        .wait_for()
        .await?;

    ui::item_action(&page, &title, "Move to deleted files").await?;
    page.locator("dialog")
        .get_by_role("button", "Move to deleted files")
        .click()
        .await?;
    page.locator(".workspace-row")
        .filter_has_text(&title)
        .wait_for_state(State::Hidden)
        .await?;
    assert!(
        !library().await?["notes"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|n| n["id"] == note_id.as_str())
    );

    page.set_viewport_size(390, 844).await?;
    page.screenshot(t.screenshot("notes-phone.png"), true)
        .await?;
    assert_eq!(
        page.evaluate("() => document.documentElement.scrollWidth > innerWidth")
            .await?,
        json!(false)
    );
    ui::open_search(&page).await?.fill("does-not-exist").await?;
    page.get_by_text("No matches across your apps.")
        .wait_for()
        .await?;
    page.get_by_role("button", "Clear search").click().await?;
    ui::close_search(&page).await?;

    // Switch to Tasks
    page.set_viewport_size(1440, 950).await?;
    ui::switch_app(&page, "tasks").await?;
    page.get_by_role("heading", "Today").wait_for().await?;
    page.get_by_role("button", "New task list").click().await?;
    dialog = page.locator("dialog,[data-editor-pane]");
    let list_name = format!("Browser tasks {}", &uuid()[..6]);
    dialog.get_by_label("Name").fill(&list_name).await?;
    dialog.get_by_role("button", "Save").click().await?;
    dialog.wait_for_state(State::Hidden).await?;
    page.get_by_role("button", &list_name).click().await?;

    page.get_by_role("button", "New task").click().await?;
    let task_title = format!("Browser task {}", &uuid()[..6]);
    dialog.get_by_label("Title").fill(&task_title).await?;
    dialog
        .get_by_label("Description")
        .fill("Finish one useful thing.")
        .await?;
    dialog.get_by_label("Date only").check().await?;
    dialog.get_by_label("Starts").fill("2026-11-06").await?;
    dialog.get_by_label("Due").fill("2026-11-07").await?;
    let priority = dialog.get_by_label("Priority");
    priority
        .evaluate("el => el.scrollIntoView({block: 'center'})", Value::Null)
        .await?;
    ui::choose(&priority, Choice::Value("1")).await?;
    dialog.get_by_role("button", "Save task").click().await?;
    page.get_by_role("heading", &task_title).wait_for().await?;
    page.screenshot(t.screenshot("tasks-desktop.png"), true)
        .await?;

    page.get_by_role("button", "Edit task").click().await?;
    assert_eq!(
        dialog.get_by_label("Due").input_value().await?,
        "2026-11-07"
    );
    page.screenshot(t.screenshot("tasks-editor-desktop.png"), true)
        .await?;
    page.locator(".editing-pane-content")
        .evaluate("el => el.scrollTop = el.scrollHeight", Value::Null)
        .await?;
    page.screenshot(t.screenshot("tasks-editor-scheduling-desktop.png"), true)
        .await?;
    ui::choose(&dialog.get_by_label("Status"), Choice::Value("IN-PROCESS")).await?;
    dialog.get_by_label("Progress (%)").fill("50").await?;
    dialog.get_by_role("button", "Save task").click().await?;
    page.get_by_role("complementary", "Task details")
        .get_by_text("50%")
        .wait_for()
        .await?;
    page.get_by_role("button", "Complete task").click().await?;
    page.get_by_role("complementary", "Task details")
        .get_by_text("100%")
        .wait_for()
        .await?;
    page.get_by_role("button", "Reopen task").click().await?;
    page.get_by_role("complementary", "Task details")
        .get_by_text("0%")
        .wait_for()
        .await?;

    page.set_viewport_size(390, 844).await?;
    page.screenshot(t.screenshot("tasks-phone.png"), true)
        .await?;
    assert_eq!(
        page.evaluate("() => document.documentElement.scrollWidth > innerWidth")
            .await?,
        json!(false)
    );
    page.get_by_role("button", "Edit task").click().await?;
    page.set_viewport_size(390, 568).await?;
    page.locator(".editing-pane-content")
        .evaluate("el => el.scrollTop = el.scrollHeight", Value::Null)
        .await?;
    page.screenshot(t.screenshot("tasks-editor-short-phone.png"), true)
        .await?;
    assert_eq!(
        page.evaluate("() => document.documentElement.scrollWidth > innerWidth")
            .await?,
        json!(false)
    );
    let phone_save = page
        .locator("[data-editor-pane]")
        .get_by_role("button", "Save task")
        .bounding_box()
        .await?
        .context("phone Save task button")?;
    assert!(phone_save.y + phone_save.height <= 568.0);

    page.set_viewport_size(390, 844).await?;
    page.locator("[data-editor-pane]")
        .get_by_role("button", "Cancel")
        .click()
        .await?;
    page.get_by_role("button", "Close task details")
        .click()
        .await?;
    let task_row = page
        .locator(".workspace-row-content")
        .filter_has_text(&task_title);
    task_row.focus().await?;
    page.keyboard().press("Enter").await?;
    page.wait_for_function(
        "() => document.activeElement?.getAttribute('aria-label') === 'Close task details'",
    )
    .await?;
    page.keyboard().press("Enter").await?;
    page.wait_for_function(
        "() => document.activeElement?.classList.contains('workspace-row-content')",
    )
    .await?;

    // Phones slide task views and lists out of the hamburger navigation drawer.
    page.get_by_role("button", "Open navigation").click().await?;
    assert_eq!(
        page.locator(".sidebar")
            .evaluate("el => getComputedStyle(el).flexDirection", Value::Null)
            .await?,
        json!("column")
    );
    page.get_by_role("button", "Today").wait_for().await?;
    page.keyboard().press("Escape").await?;

    ui::item_action(&page, &task_title, "Delete task").await?;
    dialog.get_by_role("button", "Delete task").click().await?;
    dialog.wait_for_state(State::Hidden).await?;
    let task_pat = regex::escape(&task_title);
    page.get_by_role("button", regex(&task_pat))
        .first()
        .wait_for_state(State::Hidden)
        .await?;
    assert_eq!(
        page.get_by_role("button", regex(&task_pat)).count().await?,
        0
    );

    // Repeating task completion, advancement, and single-occurrence reopen.
    page.set_viewport_size(1440, 950).await?;
    let lists = api
        .get(&format!("{base}/api/v1/tasks/lists"))
        .await?
        .json()?;
    let list_id = lists["items"]
        .as_array()
        .and_then(|items| items.iter().find(|c| c["name"] == list_name.as_str()))
        .and_then(|c| c["id"].as_str())
        .context("task list id")?
        .to_owned();
    let series_response = api
        .post(
            &format!("{base}/api/v1/tasks/items"),
            &api_headers,
            json!({
                "collectionId": list_id,
                "data": {
                    "title": "Browser repeating task",
                    "start": "2026-11-06",
                    "due": "2026-11-06",
                    "allDay": true,
                    "recurrence": "FREQ=DAILY;COUNT=2",
                }
            }),
        )
        .await?;
    assert_eq!(series_response.status(), 201);
    let series = series_response.json()?;
    let series_id = series["id"].as_str().context("series id")?;

    page.goto(&format!("{base}/tasks?item={series_id}")).await?;
    page.get_by_role("heading", "Browser repeating task")
        .wait_for()
        .await?;
    page.get_by_role("button", "Complete task").click().await?;
    dialog.get_by_label("Edit scope").wait_for().await?;
    assert!(
        dialog
            .get_by_label("Edit scope")
            .text_content()
            .await?
            .contains("2026-11-06")
    );
    dialog.get_by_role("button", "Save task").click().await?;
    page.get_by_text("Next pending occurrence: Nov 7, 2026")
        .wait_for()
        .await?;

    page.get_by_role("button", "Complete task").click().await?;
    dialog.get_by_role("button", "Save task").click().await?;
    page.get_by_role("complementary", "Task details")
        .get_by_text("100%")
        .wait_for()
        .await?;

    page.get_by_role("button", "Edit task").click().await?;
    ui::choose(
        &dialog.get_by_label("Edit scope"),
        Choice::Value("2026-11-06"),
    )
    .await?;
    ui::choose(
        &dialog.get_by_label("Status"),
        Choice::Value("NEEDS-ACTION"),
    )
    .await?;
    dialog.get_by_label("Progress (%)").fill("0").await?;
    dialog.get_by_role("button", "Save task").click().await?;
    page.get_by_text("Next pending occurrence: Nov 6, 2026")
        .wait_for()
        .await?;

    // Verify Tasks redesign: smart views return tasks (collection= empty-string bug fix),
    // inline star, Board & Status layouts, direct inspector subtask add, and Tasks settings.
    let smart_all = api
        .get(&format!(
            "{base}/api/v1/tasks/items?view=all&collection=&timezone=UTC"
        ))
        .await?
        .json()?;
    assert!(
        smart_all["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "smart view with empty collection= must return tasks"
    );

    // Quick-add subtask directly inside Task details inspector.
    page.get_by_role("complementary", "Task details")
        .get_by_label("Quick add subtask")
        .fill("Inline subtask check")
        .await?;
    page.keyboard().press("Enter").await?;
    page.get_by_role("complementary", "Task details")
        .get_by_text("Inline subtask check")
        .wait_for()
        .await?;

    // Switch layouts: Board -> Status -> List.
    page.get_by_role("radio", "Board layout").click().await?;
    page.get_by_role("region", "Task lists board")
        .wait_for()
        .await?;
    page.get_by_role("radio", "Status board layout")
        .click()
        .await?;
    page.get_by_role("region", "Status board")
        .wait_for()
        .await?;
    page.get_by_role("radio", "List layout").click().await?;

    // Open Tasks settings and confirm redesigned controls are present.
    page.get_by_role("button", "Tasks settings").click().await?;
    let settings = page.locator("[data-settings-sheet]");
    settings.get_by_text("Default layout").wait_for().await?;
    settings
        .get_by_text("Sidebar smart views")
        .wait_for()
        .await?;
    settings.get_by_text("CalDAV sync").wait_for().await?;
    settings
        .get_by_role("button", "Close settings")
        .click()
        .await?;
    settings.wait_for_state(State::Hidden).await?;

    let all_failures = failures.lock().unwrap().clone();
    context.close().await?;
    assert_eq!(all_failures, Vec::<String>::new());
    Ok(())
}
