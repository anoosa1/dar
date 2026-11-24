//! JSON Canvas 1.0 in Notes: listing `*.canvas` files alongside notes, rendering text cards,
//! embedded note cards and SVG connections, adding and moving cards, connecting two cards,
//! saving with unknown fields preserved, and the accessible card outline.

use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "notes-canvas",
    cases: &[case!(canvas_in_the_browser, 180)],
};

async fn upload_text(session: &Session, root: &str, name: &str, media_type: &str, text: &str) -> Result<String> {
    let upload = Upload { parent_id: root, media_type, ..Upload::default() };
    Ok(upload_with(session, name, text, upload).await?.file_id)
}

pub async fn canvas_in_the_browser(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = admin.get_json("/api/v1/notes/library").await?["rootId"].as_str().context("rootId")?.to_owned();
    let tag = &uuid()[..6];
    let note_title = format!("Canvas Note {tag}");
    let note_name = format!("{note_title}.md");
    upload_text(&admin, &root, &note_name, "text/markdown", &format!("# {note_title}\n\nEmbedded canvas body {tag}.\n")).await?;

    let canvas_title = format!("Board {tag}");
    let canvas_name = format!("{canvas_title}.canvas");
    let initial_json = json!({
        "nodes": [
            {
                "id": "card-text",
                "type": "text",
                "text": format!("Idea {tag}"),
                "x": 40,
                "y": 40,
                "width": 240,
                "height": 120,
                "color": "4",
                "customNodeField": "keep-node"
            },
            {
                "id": "card-note",
                "type": "file",
                "file": note_name,
                "x": 380,
                "y": 40,
                "width": 280,
                "height": 160
            }
        ],
        "edges": [
            {
                "id": "edge-1",
                "fromNode": "card-text",
                "fromSide": "right",
                "toNode": "card-note",
                "toSide": "left",
                "label": "links",
                "customEdgeField": 42
            }
        ],
        "customRootField": "keep-root"
    });
    let canvas_id = upload_text(&admin, &root, &canvas_name, "application/json", &serde_json::to_string_pretty(&initial_json)?).await?;

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

    // Canvases appear in the Notes list and open in the right sheet.
    page.locator(".workspace-row.is-canvas").filter_has_text(&canvas_title).locator(".workspace-row-content").click().await?;
    let editor = page.get_by_role("region", format!("Editor for {canvas_name}"));
    editor.wait_for().await?;

    // Both initial cards (including the embedded note content) and the SVG edge render.
    let text_card = editor.locator("[data-node-id='card-text']");
    let note_card = editor.locator("[data-node-id='card-note']");
    let initial_edge = editor.locator("[data-edge-id='edge-1']");
    text_card.wait_for().await?;
    note_card.wait_for().await?;
    initial_edge.first().wait_for().await?;
    text_card.get_by_text(format!("Idea {tag}")).wait_for().await?;
    note_card.get_by_text(format!("Embedded canvas body {tag}.")).timeout(Duration::from_secs(15)).wait_for().await?;

    // Add a new text card from the toolbar, type its text, finish editing, then move it via arrow keys.
    editor.get_by_role("button", "Add text card").click().await?;
    let input = editor.get_by_label("Card text");
    input.wait_for().await?;
    input.fill(&format!("Fresh card {tag}")).await?;
    input.press("Escape").await?;
    let added_card = editor.locator(".canvas-card.is-text").filter_has_text(format!("Fresh card {tag}"));
    added_card.wait_for().await?;
    let added_id = added_card.get_attribute("data-node-id").await?.context("added card id")?;

    // Move the original text card with Shift+ArrowRight so its x position advances measurably.
    text_card.click().await?;
    text_card.press("Shift+ArrowRight").await?;

    // Connect the newly added card to the note card (select both, then click Connect selected cards).
    added_card.click().await?;
    note_card.click_with(ClickOptions { modifiers: vec!["Shift"], ..ClickOptions::default() }).await?;
    editor.get_by_role("button", "Connect selected cards").click().await?;

    // Wait for autosave to persist the updated JSON canvas.
    let saved_body = wait_for_saved_canvas(&admin, &canvas_id, &added_id).await?;
    assert_eq!(saved_body["customRootField"], "keep-root", "unknown root fields must be preserved");
    let nodes = saved_body["nodes"].as_array().context("saved nodes")?;
    assert_eq!(nodes.len(), 3, "expected 3 cards after adding one: {saved_body}");
    let saved_text = nodes.iter().find(|n| n["id"] == "card-text").context("card-text")?;
    assert!(saved_text["x"].as_f64().unwrap_or(0.0) > 40.0, "expected card-text to move right: {saved_text}");
    assert_eq!(saved_text["customNodeField"], "keep-node", "unknown node fields must be preserved");
    let saved_added = nodes.iter().find(|n| n["id"] == added_id.as_str()).context("added card")?;
    assert_eq!(saved_added["text"], format!("Fresh card {tag}"));

    let edges = saved_body["edges"].as_array().context("saved edges")?;
    assert_eq!(edges.len(), 2, "expected 2 edges after connecting: {saved_body}");
    let kept_edge = edges.iter().find(|e| e["id"] == "edge-1").context("edge-1")?;
    assert_eq!(kept_edge["customEdgeField"], 42, "unknown edge fields must be preserved");
    let has_new_edge = edges.iter().any(|e| {
        (e["fromNode"] == added_id.as_str() && e["toNode"] == "card-note")
            || (e["fromNode"] == "card-note" && e["toNode"] == added_id.as_str())
    });
    assert!(has_new_edge, "expected an edge between {added_id} and card-note: {edges:?}");

    page.screenshot(t.screenshot("notes-canvas.png"), true).await?;

    let failures = failures.lock().unwrap().clone();
    assert!(failures.is_empty(), "page errors: {failures:?}");
    Ok(())
}

async fn wait_for_saved_canvas(admin: &Session, canvas_id: &str, added_id: &str) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut last = String::new();
    while tokio::time::Instant::now() < deadline {
        last = admin.get(&format!("/api/v1/files/{canvas_id}/content")).await?.text().to_owned();
        if let Ok(parsed) = serde_json::from_str::<Value>(&last) {
            let has_added = parsed["nodes"].as_array().is_some_and(|n| n.iter().any(|c| c["id"] == added_id));
            let has_two_edges = parsed["edges"].as_array().is_some_and(|e| e.len() >= 2);
            if has_added && has_two_edges {
                return Ok(parsed);
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    bail!("timed out waiting for saved canvas with node {added_id} and 2 edges; last content: {last}")
}
