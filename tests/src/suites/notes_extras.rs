//! Notes extras: Random note, importing a Logseq graph from Notes settings, ` ```base ` blocks
//! (Obsidian Bases) rendered with the query engine, and global search by `#tag` and alias.

use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "notes-extras",
    cases: &[case!(random_note_and_base_blocks, 120), case!(logseq_import, 120), case!(tag_and_alias_search, 90)],
};

async fn note(session: &Session, root: &str, name: &str, text: &str) -> Result<String> {
    let upload = Upload { parent_id: root, media_type: "text/markdown", ..Upload::default() };
    Ok(upload_with(session, name, text, upload).await?.file_id)
}

async fn notes_root(session: &Session) -> Result<String> {
    Ok(session.get_json("/api/v1/notes/library").await?["rootId"].as_str().context("rootId")?.to_owned())
}

/// A signed-in Notes page that records page errors.
async fn notes_page(base: &str, path: &str) -> Result<(BrowserContext, Page, Arc<Mutex<Vec<String>>>)> {
    let browser = Browser::shared().await?;
    let context = browser.new_context(ContextOptions::sized(1440, 950)).await?;
    let page = context.new_page().await?;
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let sink = failures.clone();
        page.on_page_error(move |msg| sink.lock().unwrap().push(msg.clone()));
    }
    page.goto(&format!("{base}{path}")).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.get_by_role("heading", "All notes").wait_for().await?;
    Ok((context, page, failures))
}

pub async fn random_note_and_base_blocks(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = notes_root(&admin).await?;
    let tag = &uuid()[..6];
    let db = admin.connect().await?;
    let templates = uuid();
    reducer(|done| db.reducers.create_folder_then(templates.clone(), root.clone(), "Templates".into(), done)).await?;
    let template = note(&admin, &templates, "Daily.md", "# {{title}}\n").await?;
    let reading = uuid();
    reducer(|done| db.reducers.create_folder_then(reading.clone(), root.clone(), format!("Reading {tag}"), done)).await?;
    note(&admin, &reading, &format!("Dune {tag}.md"), &format!("---\nstatus: reading\nrating: 5\ntags: [book{tag}]\n---\nGood.\n")).await?;
    note(&admin, &reading, &format!("Emma {tag}.md"), &format!("---\nstatus: done\nrating: 3\ntags: [book{tag}]\n---\nFine.\n")).await?;
    note(&admin, &root, &format!("Loose {tag}.md"), &format!("---\nrating: 9\ntags: [book{tag}]\n---\nOutside the folder.\n")).await?;
    let shelf_text = format!(
        "# Shelf {tag}\n\n```base\nfilters:\n  and:\n    - file.hasTag(\"book{tag}\")\n    - file.inFolder(\"Reading {tag}\")\nformulas:\n  double: \"rating * 2\"\nproperties:\n  status:\n    displayName: State\nviews:\n  - type: table\n    order:\n      - file.name\n      - status\n      - rating\n    sort:\n      - property: rating\n        direction: DESC\n```\n"
    );
    let shelf = note(&admin, &root, &format!("Shelf {tag}.md"), &shelf_text).await?;

    let library = admin.get_json("/api/v1/notes/library").await?;
    let candidates: Vec<String> = library["notes"]
        .as_array()
        .context("notes")?
        .iter()
        .filter(|n| n["id"] != shelf.as_str() && !n["path"].as_str().unwrap_or_default().to_lowercase().starts_with("templates/"))
        .filter_map(|n| n["id"].as_str().map(str::to_owned))
        .collect();

    let (_context, page, failures) = notes_page(&base, &format!("/notes?note={shelf}")).await?;
    let editor = page.get_by_role("region", format!("Editor for Shelf {tag}.md"));
    editor.wait_for().await?;

    // The base block lists the books in the folder, with the base's columns, sort and notice.
    editor.get_by_role("button", "Reading").click().await?;
    let results = editor.get_by_role("group", "Query results");
    results.get_by_text("2 notes").wait_for().await?;
    results.get_by_role("columnheader", "State").wait_for().await?;
    let first = results.locator("tbody tr").first();
    first.get_by_role("link", format!("Dune {tag}")).wait_for().await?;
    first.get_by_text("reading").wait_for().await?;
    assert_eq!(results.get_by_role("link", format!("Loose {tag}")).count().await?, 0);
    results.get_by_text("Not supported here, left out: formulas.").wait_for().await?;
    page.screenshot(t.screenshot("notes-base-block.png"), true).await?;

    // Random note opens another note, never the open one or a template.
    let mut previous = shelf.clone();
    for _ in 0..4 {
        page.get_by_role("button", "Random note").click().await?;
        let chosen = page
            .wait_for_function_with("(previous) => { const id = new URLSearchParams(location.search).get('note'); return id && id !== previous ? id : null; }", json!(previous))
            .await?;
        let chosen = chosen.as_str().context("chosen note")?.to_owned();
        assert_ne!(chosen, template, "Random note opened a template");
        assert!(candidates.contains(&chosen) || chosen == shelf, "Random note opened {chosen}, not a library note");
        previous = chosen;
    }

    let failures = failures.lock().unwrap().clone();
    assert!(failures.is_empty(), "page errors: {failures:?}");
    Ok(())
}

pub async fn logseq_import(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = notes_root(&admin).await?;
    let tag = &uuid()[..6];
    // A note already at the page's path is kept; the import takes ` 2`.
    let existing = note(&admin, &root, &format!("Inbox {tag}.md"), "Kept.\n").await?;
    let block = "6563a1b2-0f4e-4b1a-9c3d-112233445566";
    let page_text = format!("tags:: logseq{tag}\nalias:: Alpha {tag}\n\n- TODO Write the plan\n- The key idea {tag}\n  id:: {block}\n");
    let journal_text = format!("- Met about (({block}))\n- {{{{embed [[proj{tag}/alpha]]}}}}\n- DONE Call {tag}\n");

    let (_context, page, failures) = notes_page(&base, "/notes").await?;
    page.get_by_role("button", "Notes settings").first().click().await?;
    let settings = page.get_by_role("dialog", "Notes settings");
    settings.get_by_role("tab", "Import and export").click().await?;
    settings
        .get_by_label("Logseq Markdown files")
        .set_input_files(vec![
            FilePayload::new(&format!("proj{tag}___alpha.md"), "text/markdown", page_text.clone()),
            FilePayload::new("2031_01_09.md", "text/markdown", journal_text.clone()),
            FilePayload::new(&format!("Inbox {tag}.md"), "text/markdown", "- From Logseq\n"),
            FilePayload::new("config.edn", "application/edn", "{}"),
        ])
        .await?;
    settings.get_by_role("button", "Import Logseq graph").click().await?;
    settings.get_by_text("Imported 3 notes into Notes.").timeout(Duration::from_secs(30)).wait_for().await?;
    settings.get_by_text("Linked 1 block reference.").wait_for().await?;
    settings.get_by_text("Skipped 1 file that is not a page or journal.").wait_for().await?;
    page.screenshot(t.screenshot("notes-logseq-import.png"), true).await?;

    let library = admin.get_json("/api/v1/notes/library").await?;
    let notes = library["notes"].as_array().context("notes")?;
    let by_path = |path: String| notes.iter().find(|n| n["path"] == path.as_str()).cloned();
    let alpha = by_path(format!("proj{tag}/alpha.md")).context("namespaced page")?;
    assert_eq!(alpha["tags"], json!([format!("logseq{tag}")]));
    assert_eq!(alpha["aliases"], json!([format!("Alpha {tag}")]));
    let content = |id: &str| {
        let (admin, id) = (admin.clone(), id.to_owned());
        async move { Ok::<String, Error>(admin.get(&format!("/api/v1/files/{id}/content")).await?.expect(200)?.text()) }
    };
    assert_eq!(
        content(alpha["id"].as_str().context("id")?).await?,
        format!("---\ntags:\n  - logseq{tag}\naliases:\n  - Alpha {tag}\n---\n- [ ] Write the plan\n- The key idea {tag} ^6563a1\n")
    );
    let journal = by_path("Journal/2031-01-09.md".into()).context("journal")?;
    assert_eq!(content(journal["id"].as_str().context("id")?).await?, format!("- Met about [[proj{tag}/alpha#^6563a1]]\n- ![[proj{tag}/alpha]]\n- [x] Call {tag}\n"));
    let copy = by_path(format!("Inbox {tag} 2.md")).context("renamed copy")?;
    assert_eq!(content(copy["id"].as_str().context("id")?).await?, "- From Logseq\n");
    assert_eq!(content(&existing).await?, "Kept.\n");

    // The Notes list shows the imported notes once settings close.
    settings.get_by_role("button", "Close settings").click().await?;
    page.get_by_role("button", contains(&format!("Inbox {tag} 2"))).first().wait_for().await?;

    let failures = failures.lock().unwrap().clone();
    assert!(failures.is_empty(), "page errors: {failures:?}");
    Ok(())
}

pub async fn tag_and_alias_search(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = notes_root(&admin).await?;
    let tag = &uuid()[..6];
    // The tag is a property, so the text never holds `#grow…` for the module's text search.
    let tagged = note(&admin, &root, &format!("Garden plan {tag}.md"), &format!("---\ntags: [grow{tag}/spring]\n---\n# Garden plan {tag}\nSeeds and beds.\n")).await?;
    let aliased = note(&admin, &root, &format!("Atlas {tag}.md"), &format!("---\naliases: [Wayfinder{tag}]\n---\n# Atlas {tag}\n")).await?;
    let search = |q: String| {
        let admin = admin.clone();
        async move { admin.get_json(&format!("/api/v1/search?q={}", q.replace('#', "%23"))).await }
    };
    let hrefs = |result: &Value| result["items"].as_array().cloned().unwrap_or_default().iter().filter(|i| i["group"] == "Notes").filter_map(|i| i["href"].as_str().map(str::to_owned)).collect::<Vec<_>>();

    // `#tag` finds notes by tag, nested tags and tag prefixes included.
    let found = search(format!("#grow{tag}")).await?;
    assert_eq!(hrefs(&found), [format!("/notes?note={tagged}")]);
    let item = found["items"].as_array().context("items")?.iter().find(|i| i["href"] == format!("/notes?note={tagged}")).cloned().context("tag hit")?;
    assert_eq!(item["title"], format!("Garden plan {tag}"));
    assert_eq!(item["detail"], format!("#grow{tag}/spring"));
    assert_eq!(hrefs(&search(format!("#grow{tag}/spr")).await?), [format!("/notes?note={tagged}")]);
    assert!(hrefs(&search(format!("#seeds{tag}")).await?).is_empty());
    // An alias finds its note, once.
    let found = search(format!("wayfinder{tag}")).await?;
    assert_eq!(hrefs(&found), [format!("/notes?note={aliased}")]);

    // The spotlight shows the tag hit in the Notes group.
    let (_context, page, failures) = notes_page(&base, "/notes").await?;
    ui::open_search(&page).await?.fill(&format!("#grow{tag}")).await?;
    page.get_by_role("option", contains(&format!("Garden plan {tag}"))).wait_for().await?;
    page.screenshot(t.screenshot("notes-tag-search.png"), true).await?;
    ui::close_search(&page).await?;

    let failures = failures.lock().unwrap().clone();
    assert!(failures.is_empty(), "page errors: {failures:?}");
    Ok(())
}
