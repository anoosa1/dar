//! Linked notes: the library's tags and aliases, backlinks and unlinked mentions over the API,
//! and in the browser `[[`/`#` suggestions, following and creating links, linked references,
//! link updates after a rename, the tag list and checkbox toggling in the preview.

use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "notes-links",
    cases: &[
        case!(backlinks_mentions_and_tags, 60),
        case!(linked_writing_in_the_browser, 180),
        case!(daily_notes_templates_and_embeds, 120),
        case!(outliner_keys_and_query_blocks, 120),
        case!(hover_preview_block_link_and_folder_rename, 180),
    ],
};

async fn note(session: &Session, root: &str, name: &str, text: &str) -> Result<String> {
    let upload = Upload { parent_id: root, media_type: "text/markdown", ..Upload::default() };
    Ok(upload_with(session, name, text, upload).await?.file_id)
}

fn find<'a>(list: &'a Value, key: &str, id: &str) -> Option<&'a Value> {
    list[key].as_array()?.iter().find(|n| n["id"] == id)
}

pub async fn backlinks_mentions_and_tags(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = admin.get_json("/api/v1/notes/library").await?["rootId"].as_str().context("rootId")?.to_owned();
    let tag = &uuid()[..6];
    let apollo = note(&admin, &root, &format!("Apollo {tag}.md"), &format!("---\naliases: [Moonshot {tag}]\ntags: [space]\n---\n# Apollo {tag}\n\n## Launch\nPlan. #mission/alpha\n")).await?;
    let log = note(&admin, &root, &format!("Log {tag}.md"), &format!("Met about [[Apollo {tag}#Launch|the launch]] today.\n\n- [ ] follow up ^task1\n")).await?;
    let refs = note(&admin, &root, &format!("Refs {tag}.md"), &format!("See [the plan](Apollo%20{tag}.md).\n")).await?;
    let diary = note(&admin, &root, &format!("Diary {tag}.md"), &format!("Thinking about apollo {tag} and moonshot {tag} again.\n\n```\nApollo {tag}\n```\n")).await?;

    let library = admin.get_json("/api/v1/notes/library").await?;
    let row = find(&library, "notes", &apollo).context("apollo in the library")?;
    assert_eq!(row["tags"], json!(["space", "mission/alpha"]));
    assert_eq!(row["aliases"], json!([format!("Moonshot {tag}")]));
    // Aliases are searchable.
    let found = admin.get_json(&format!("/api/v1/notes/library?q=moonshot%20{tag}")).await?;
    assert!(find(&found, "notes", &apollo).is_some());

    let backlinks = admin.get_json(&format!("/api/v1/notes/backlinks?id={apollo}")).await?;
    let from_log = find(&backlinks, "backlinks", &log).context("log links to apollo")?;
    assert_eq!(from_log["title"], format!("Log {tag}"));
    assert_eq!(from_log["contexts"][0]["line"], 0);
    assert!(from_log["contexts"][0]["text"].as_str().unwrap_or_default().contains("the launch"));
    assert!(find(&backlinks, "backlinks", &refs).is_some(), "Markdown links count as links");
    assert!(find(&backlinks, "backlinks", &diary).is_none());

    let mentions = admin.get_json(&format!("/api/v1/notes/mentions?id={apollo}")).await?;
    assert_eq!(mentions["target"], format!("Apollo {tag}"));
    let from_diary = find(&mentions, "mentions", &diary).context("diary mentions apollo")?;
    let matched: Vec<&str> = from_diary["contexts"].as_array().context("contexts")?.iter().filter_map(|c| c["match"].as_str()).collect();
    assert_eq!(matched, [format!("apollo {tag}"), format!("moonshot {tag}")], "code is not a mention");
    let start = from_diary["contexts"][0]["start"].as_u64().context("start")? as usize;
    assert_eq!(start, "Thinking about ".len());
    assert!(find(&mentions, "mentions", &log).is_none(), "linking notes are not unlinked mentions");

    // The other direction has nothing; unknown and malformed IDs fail.
    assert_eq!(admin.get_json(&format!("/api/v1/notes/backlinks?id={diary}")).await?["backlinks"], json!([]));
    assert_eq!(admin.get(&format!("/api/v1/notes/backlinks?id={}", uuid())).await?.status, 404);
    assert_eq!(admin.get("/api/v1/notes/mentions?id=").await?.status, 400);
    Ok(())
}

pub async fn linked_writing_in_the_browser(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = admin.get_json("/api/v1/notes/library").await?["rootId"].as_str().context("rootId")?.to_owned();
    let tag = &uuid()[..6];
    let target = format!("Apollo {tag}");
    let apollo = note(&admin, &root, &format!("{target}.md"), &format!("---\ntags: [orbit{tag}]\n---\n# {target}\n\n## Launch\nCountdown.\n")).await?;

    let browser = Browser::shared().await?;
    let context = browser.new_context(ContextOptions::sized(1440, 950)).await?;
    let page = context.new_page().await?;
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let sink = failures.clone();
        page.on_page_error(move |msg| sink.lock().unwrap().push(msg.clone()));
    }
    {
        let sink = failures.clone();
        page.on_console(move |msg| {
            if msg.kind == "error" && !(msg.text.contains("401") && msg.text.contains("Unauthorized")) {
                sink.lock().unwrap().push(msg.text.clone());
            }
        });
    }
    page.goto(&format!("{base}/notes")).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.get_by_role("heading", "All notes").wait_for().await?;

    // `[[` suggests notes and `#` suggests tags; Enter accepts.
    page.get_by_role("button", "New note").first().click().await?;
    let editor = page.get_by_role("region", "Editor for Untitled.md");
    let source = ui::note_source(&editor).await?;
    assert_eq!(editor.get_by_role("button", "Live").get_attribute("aria-pressed").await?.as_deref(), Some("true"), "Live is the default mode");
    let trip = format!("Trip {tag}");
    source.fill(&format!("# {trip}\n\n- [ ] pack\n\nVisit ")).await?;
    source.press_sequentially("[[Apol").await?;
    page.get_by_role("option", contains(&target)).wait_for().await?;
    page.screenshot(t.screenshot("notes-links-suggestions.png"), true).await?;
    source.press("Enter").await?;
    source.press_sequentially(" soon #orbit").await?;
    page.get_by_role("option", contains(&format!("#orbit{tag}"))).wait_for().await?;
    source.press("Enter").await?;
    let written = ui::note_text(&source).await?;
    assert!(written.ends_with(&format!("Visit [[{target}]] soon #orbit{tag} ")), "{written}");
    source.press_sequentially("and [[Fresh idea]]").await?;
    editor.get_by_text("All changes saved").timeout(Duration::from_secs(10)).wait_for().await?;

    // Live preview: away from the caret's line the heading drops its `#`, the wikilink its
    // brackets (dashed while its note is missing), the tag is a chip and the task a checkbox.
    // The caret's line (here the last) shows its Markdown as written.
    let heading = editor.locator(".cm-line.cm-dar-h1");
    assert_eq!(heading.text_content().await?, trip);
    let link = editor.locator(&format!(r#".cm-dar-link[data-href="{target}"]"#));
    assert_eq!(link.text_content().await?, format!("[[{target}]]"));
    source.press("Control+Home").await?;
    eventually(Duration::from_secs(5), "the heading shows its marker", || async { Ok((heading.text_content().await? == format!("# {trip}")).then_some(())) }).await?;
    eventually(Duration::from_secs(5), "the link hides its brackets", || async { Ok((link.text_content().await? == target).then_some(())) }).await?;
    assert!(!link.matches("el => el.classList.contains('is-unresolved')").await?);
    assert!(editor.locator(r#".cm-dar-link[data-href="Fresh idea"]"#).matches("el => el.classList.contains('is-unresolved')").await?);
    editor.locator(".cm-dar-tag").filter_has_text(&format!("#orbit{tag}")).wait_for().await?;
    page.screenshot(t.screenshot("notes-live-preview.png"), true).await?;
    source.press("Control+End").await?;
    editor.get_by_role("checkbox", "Open task").click().await?;
    editor.get_by_role("checkbox", "Completed task").wait_for().await?;
    assert!(ui::note_text(&source).await?.contains("- [x] pack"));
    // Undo (the toolbar's, CodeMirror's history) reopens it.
    editor.get_by_role("button", "Undo").click().await?;
    editor.get_by_role("checkbox", "Open task").wait_for().await?;
    assert!(ui::note_text(&source).await?.contains("- [ ] pack"));
    editor.get_by_text("All changes saved").timeout(Duration::from_secs(10)).wait_for().await?;

    // Reading links to the note, shows the tag and toggles the checkbox in the source.
    editor.get_by_role("button", "Reading").click().await?;
    assert_eq!(editor.get_by_role("textbox", "File contents").count().await?, 0, "Reading hides the editor");
    editor.get_by_role("checkbox", "Open task").click().await?;
    editor.get_by_role("checkbox", "Completed task").wait_for().await?;
    editor.get_by_text("All changes saved").timeout(Duration::from_secs(10)).wait_for().await?;
    editor.get_by_role("button", "Live").click().await?;
    assert!(ui::note_text(&source).await?.contains("- [x] pack"));
    editor.get_by_role("button", "Reading").click().await?;
    editor.get_by_role("button", format!("#orbit{tag}")).wait_for().await?;
    page.screenshot(t.screenshot("notes-links-preview.png"), true).await?;

    // Following the link opens the note; its linked references list the new note.
    editor.get_by_role("link", target.as_str()).click().await?;
    let apollo_editor = page.get_by_role("region", format!("Editor for {target}.md"));
    apollo_editor.wait_for().await?;
    let references = apollo_editor.get_by_role("region", "References");
    references.get_by_role("button", trip.as_str()).wait_for().await?;
    references.get_by_text(contains("Visit")).wait_for().await?;
    page.screenshot(t.screenshot("notes-links-references.png"), true).await?;

    // The outline jumps to a heading.
    apollo_editor.get_by_role("button", "Outline").click().await?;
    page.get_by_role("menuitem", contains("Launch")).click().await?;

    // Renaming the note updates the link in the other note.
    let renamed = format!("Artemis {tag}");
    ui::item_action(&page, &target, "Rename").await?;
    let pane = page.locator("[data-editor-pane]");
    pane.get_by_role("textbox", ()).fill(&renamed).await?;
    pane.get_by_role("button", "Save").click().await?;
    page.get_by_text(contains("Updated links in 1 note")).wait_for().await?;
    let library = admin.get_json("/api/v1/notes/library").await?;
    let trip_id = library["notes"].as_array().context("notes")?.iter().find(|n| n["title"] == trip.as_str()).and_then(|n| n["id"].as_str()).context("trip note")?.to_owned();
    let text = admin.get(&format!("/api/v1/files/{trip_id}/content")).await?.text();
    assert!(text.contains(&format!("[[{renamed}]]")), "{text}");

    // The tag list filters notes by tag.
    page.get_by_role("list", "Tags").get_by_role("button", contains(&format!("#orbit{tag}"))).click().await?;
    page.get_by_role("heading", format!("#orbit{tag}")).wait_for().await?;
    page.locator(".workspace-row").filter_has_text(&renamed).wait_for().await?;
    page.locator(".workspace-row").filter_has_text(&trip).wait_for().await?;

    // A link to a missing note creates it: Ctrl+click in Live preview follows it.
    page.locator(".workspace-row").filter_has_text(&trip).locator(".workspace-row-content").click().await?;
    let trip_editor = page.get_by_role("region", regex("^Editor for "));
    trip_editor.get_by_role("link", "Fresh idea").wait_for().await?;
    trip_editor.get_by_role("button", "Live").click().await?;
    ui::note_source(&trip_editor).await?;
    let fresh = trip_editor.locator(r#".cm-dar-link[data-href="Fresh idea"]"#);
    // A plain click edits the link's text instead.
    fresh.click().await?;
    trip_editor.locator(".cm-line").filter_has_text("[[Fresh idea]]").wait_for().await?;
    trip_editor.get_by_role("textbox", "File contents").press("Control+Home").await?;
    fresh.click_with(ClickOptions { modifiers: vec!["Control"], ..ClickOptions::default() }).await?;
    page.get_by_role("region", "Editor for Fresh idea.md").wait_for().await?;
    let library = admin.get_json("/api/v1/notes/library").await?;
    assert!(library["notes"].as_array().context("notes")?.iter().any(|n| n["path"] == "Fresh idea.md"));
    assert!(library["notes"].as_array().context("notes")?.iter().any(|n| n["id"] == apollo.as_str() && n["path"] == format!("{renamed}.md")));

    let failures = failures.lock().unwrap().clone();
    assert!(failures.is_empty(), "page errors: {failures:?}");
    Ok(())
}

pub async fn daily_notes_templates_and_embeds(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = admin.get_json("/api/v1/notes/library").await?["rootId"].as_str().context("rootId")?.to_owned();
    let tag = &uuid()[..6];
    let db = admin.connect().await?;
    let templates = uuid();
    reducer(|done| db.reducers.create_folder_then(templates.clone(), root.clone(), "Templates".into(), done)).await?;
    note(&admin, &templates, "Daily.md", &format!("---\ntags: [template]\n---\n# {{{{title}}}}\n\nTemplate line {tag}\n")).await?;
    note(&admin, &root, &format!("Source {tag}.md"), &format!("# Source\n\n## Part\nEmbedded words {tag}.\n\n## Other\nNot shown {tag}.\n")).await?;

    let browser = Browser::shared().await?;
    let context = browser.new_context(ContextOptions::sized(1440, 950)).await?;
    let page = context.new_page().await?;
    page.goto(&format!("{base}/notes")).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.get_by_role("heading", "All notes").wait_for().await?;
    let date = page.evaluate("() => { const d = new Date(); return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`; }").await?;
    let date = date.as_str().context("date")?.to_owned();

    // Today opens Journal/<date>.md, made from Templates/Daily.
    page.get_by_role("button", "Today").click().await?;
    let editor = page.get_by_role("region", format!("Editor for {date}.md"));
    editor.wait_for().await?;
    let source = ui::note_source(&editor).await?;
    let filled = ui::note_text(&source).await?;
    assert_eq!(filled.trim(), format!("# {date}\n\nTemplate line {tag}"));

    // Slash commands insert blocks; templates are listed with them.
    source.press("End").await?;
    source.press_sequentially("\n/tab").await?;
    page.get_by_role("option", contains("Table")).wait_for().await?;
    source.press("Enter").await?;
    assert!(ui::note_text(&source).await?.contains("| --- | --- |"));
    source.press_sequentially("\n\n/dai").await?;
    page.get_by_role("option", contains("Template: Daily")).wait_for().await?;
    source.press("Escape").await?;

    // A note section embeds in place; a dropped image goes to attachments and embeds.
    let text = format!("{filled}\n![[Source {tag}#Part]]\n");
    source.fill(&text).await?;
    source.evaluate(
        "el => { const bytes = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='), c => c.charCodeAt(0));
                const data = new DataTransfer(); data.items.add(new File([bytes], 'dot.png', { type: 'image/png' }));
                el.darEditor.focus(); el.darEditor.select(el.darEditor.text().length);
                el.dispatchEvent(new DragEvent('drop', { dataTransfer: data, bubbles: true, cancelable: true })); }",
        Value::Null,
    ).await?;
    eventually(Duration::from_secs(10), "image embed inserted", || async { Ok(ui::note_text(&source).await?.contains("![[dot.png]]").then_some(())) }).await?;
    editor.get_by_text("All changes saved").timeout(Duration::from_secs(10)).wait_for().await?;
    let files = admin.get_json("/api/v1/notes/attachments").await?;
    assert!(files["files"].as_array().context("files")?.iter().any(|f| f["path"] == "attachments/dot.png" && f["mediaType"] == "image/png"));

    editor.get_by_role("button", "Reading").click().await?;
    let embed = editor.get_by_role("group", format!("Embedded Source {tag} \u{203a} Part"));
    embed.get_by_text(format!("Embedded words {tag}.")).wait_for().await?;
    assert_eq!(embed.get_by_text(format!("Not shown {tag}.")).count().await?, 0);
    editor.locator(r#"img.note-image[src^="/api/v1/files/"]"#).wait_for().await?;
    page.screenshot(t.screenshot("notes-daily-embeds.png"), true).await?;

    // Today opens the same note again.
    editor.get_by_role("button", "Close editor").click().await?;
    page.get_by_role("button", "Today").click().await?;
    editor.wait_for().await?;
    let library = admin.get_json("/api/v1/notes/library").await?;
    assert_eq!(library["notes"].as_array().context("notes")?.iter().filter(|n| n["path"] == format!("Journal/{date}.md")).count(), 1);
    Ok(())
}

pub async fn outliner_keys_and_query_blocks(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = admin.get_json("/api/v1/notes/library").await?["rootId"].as_str().context("rootId")?.to_owned();
    let tag = &uuid()[..6];
    note(&admin, &root, &format!("Book A {tag}.md"), &format!("---\nstatus: reading\nrating: 5\ntags: [book{tag}]\n---\nGood.\n")).await?;
    note(&admin, &root, &format!("Book B {tag}.md"), &format!("---\nstatus: done\nrating: 3\ntags: [book{tag}]\n---\nFine.\n")).await?;
    let library = admin.get_json("/api/v1/notes/library").await?;
    let book = library["notes"].as_array().context("notes")?.iter().find(|n| n["title"] == format!("Book A {tag}")).context("book")?;
    assert_eq!(book["properties"], json!({ "status": "reading", "rating": "5", "tags": [format!("book{tag}")] }));

    let browser = Browser::shared().await?;
    let context = browser.new_context(ContextOptions::sized(1440, 950)).await?;
    let page = context.new_page().await?;
    page.goto(&format!("{base}/notes")).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.get_by_role("heading", "All notes").wait_for().await?;
    page.get_by_role("button", "New note").first().click().await?;
    let editor = page.get_by_role("region", regex("^Editor for Untitled"));
    let source = ui::note_source(&editor).await?;

    // Enter continues the list, Tab and Shift+Tab change the level, Alt+Shift+Up moves the line.
    source.fill("- one").await?;
    source.press("End").await?;
    source.press("Enter").await?;
    source.press_sequentially("two").await?;
    source.press("Tab").await?;
    assert_eq!(ui::note_text(&source).await?, "- one\n\t- two");
    source.press("Shift+Tab").await?;
    source.press("Alt+Shift+ArrowUp").await?;
    assert_eq!(ui::note_text(&source).await?, "- two\n- one");
    source.press("End").await?;
    source.press("Enter").await?;
    source.press("Enter").await?;
    assert_eq!(ui::note_text(&source).await?, "- two\n\n- one", "Enter on an empty item ends the list");

    // A query block lists matching notes with their properties.
    source.fill(&format!("# Shelf {tag}\n\n```query\ntag: #book{tag}\nsort: rating desc\nshow: status, rating\n```\n")).await?;
    editor.get_by_text("All changes saved").timeout(Duration::from_secs(10)).wait_for().await?;
    editor.get_by_role("button", "Reading").click().await?;
    let results = editor.get_by_role("group", "Query results");
    results.get_by_text("2 notes").wait_for().await?;
    let first = results.locator("tbody tr").first();
    first.get_by_role("link", format!("Book A {tag}")).wait_for().await?;
    first.get_by_text("reading").wait_for().await?;
    page.screenshot(t.screenshot("notes-query.png"), true).await?;
    results.get_by_role("link", format!("Book B {tag}")).click().await?;
    page.get_by_role("region", format!("Editor for Book B {tag}.md")).wait_for().await?;
    Ok(())
}

pub async fn hover_preview_block_link_and_folder_rename(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = admin.get_json("/api/v1/notes/library").await?["rootId"].as_str().context("rootId")?.to_owned();
    let tag = &uuid()[..6];
    let db = admin.connect().await?;
    let proj = uuid();
    reducer(|done| db.reducers.create_folder_then(proj.clone(), root.clone(), format!("Proj {tag}"), done)).await?;
    note(
        &admin,
        &proj,
        "Plan.md",
        &format!("# Plan {tag}\n\n## Details\nPreview body {tag}.\n\n## Other\nHidden part {tag}.\n"),
    )
    .await?;
    let host_id = note(
        &admin,
        &root,
        &format!("Host {tag}.md"),
        &format!("# Host {tag}\n\nAnchor line {tag}\n\nSee [[Proj {tag}/Plan#Details|Plan details]], [[Proj {tag}/Plan]] and [p](Proj%20{tag}/Plan.md).\n"),
    )
    .await?;

    let browser = Browser::shared().await?;
    let context = browser.new_context(ContextOptions::sized(1440, 950)).await?;
    let page = context.new_page().await?;
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let sink = failures.clone();
        page.on_page_error(move |msg| sink.lock().unwrap().push(msg.clone()));
    }
    {
        let sink = failures.clone();
        page.on_console(move |msg| {
            if msg.kind == "error" && !(msg.text.contains("401") && msg.text.contains("Unauthorized")) {
                sink.lock().unwrap().push(msg.text.clone());
            }
        });
    }
    page.goto(&format!("{base}/notes")).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.get_by_role("heading", "All notes").wait_for().await?;

    page.locator(".workspace-row").filter_has_text(&format!("Host {tag}")).locator(".workspace-row-content").click().await?;
    let editor = page.get_by_role("region", format!("Editor for Host {tag}.md"));
    editor.wait_for().await?;
    let source = ui::note_source(&editor).await?;

    // Blank line shows the cursor error; non-blank line gets a `^xxxxxx` id and copies the block link.
    source.evaluate(
        "el => { const i = el.darEditor.text().indexOf('\\n\\n'); el.darEditor.focus(); el.darEditor.select(i + 1); }",
        Value::Null,
    )
    .await?;
    editor.get_by_role("button", "File options").click().await?;
    page.get_by_role("menuitem", "Copy block link").click().await?;
    editor.get_by_text("Put the cursor on a line to link to it.").wait_for().await?;

    source.evaluate(
        "el => { const i = el.darEditor.text().indexOf('Anchor line'); el.darEditor.focus(); el.darEditor.select(i + 4); }",
        Value::Null,
    )
    .await?;
    editor.get_by_role("button", "File options").click().await?;
    page.get_by_role("menuitem", "Copy block link").click().await?;
    let updated_source = ui::note_text(&source).await?;
    let anchor_line = updated_source
        .lines()
        .find(|l| l.starts_with(&format!("Anchor line {tag}")))
        .context("anchor line")?;
    let block_id = anchor_line
        .strip_prefix(&format!("Anchor line {tag} ^"))
        .context("appended ^block id")?
        .to_owned();
    assert_eq!(block_id.len(), 6, "{anchor_line}");
    assert!(block_id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()), "{block_id}");

    let status_notice = editor.locator(r#".note-editor-notice[role="status"]"#);
    status_notice.wait_for().await?;
    let notice_text = status_notice.text_content().await?;
    let expected_fallback = format!("[[Host {tag}#^{block_id}]]");
    assert!(
        notice_text.contains("Block link copied.") || notice_text.contains(&expected_fallback),
        "unexpected status notice: {notice_text}"
    );
    editor.get_by_text("All changes saved").timeout(Duration::from_secs(10)).wait_for().await?;

    // Hovering a resolved link in preview opens a top-layer preview card for the target fragment;
    // moving away hides it.
    editor.get_by_role("button", "Reading").click().await?;
    editor.get_by_role("link", "Plan details").hover().await?;
    let card = page.get_by_role("group", format!("Preview of Plan {tag} \u{203a} Details"));
    card.get_by_text(format!("Preview body {tag}.")).wait_for().await?;
    assert_eq!(card.get_by_text(format!("Hidden part {tag}.")).count().await?, 0);
    page.screenshot(t.screenshot("notes-hover-preview.png"), true).await?;
    page.mouse().move_to(10.0, 10.0, 2).await?;
    card.wait_for_state(State::Hidden).await?;

    // Renaming a folder rewrites both wikilinks and Markdown links pointing into it.
    page.get_by_role("button", format!("Actions for folder Proj {tag}")).click().await?;
    page.get_by_role("menuitem", "Rename folder").click().await?;
    let pane = page.locator("[data-editor-pane]");
    pane.get_by_role("textbox", ()).fill(&format!("Work {tag}")).await?;
    pane.get_by_role("button", "Save").click().await?;
    page.get_by_text(contains("Updated links in 1 note")).wait_for().await?;

    let content = admin.get(&format!("/api/v1/files/{host_id}/content")).await?.text();
    assert!(content.contains(&format!("[[Work {tag}/Plan]]")), "{content}");
    assert!(content.contains(&format!("[p](Work%20{tag}/Plan.md)")), "{content}");
    assert!(content.contains(&format!("Anchor line {tag} ^{block_id}")), "{content}");

    let failures = failures.lock().unwrap().clone();
    assert!(failures.is_empty(), "page errors: {failures:?}");
    Ok(())
}
