//! Graph view for Notes: `/api/v1/notes/graph` nodes and deduplicated edges, and the browser
//! Graph view (canvas, accessible list, filtering, local graph, opening a note, responsive phone).

use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::sync::{Arc, Mutex};

pub const SUITE: Suite = Suite {
    name: "notes-graph",
    cases: &[case!(graph_api, 60), case!(graph_in_the_browser, 120)],
};

async fn note(session: &Session, root: &str, name: &str, text: &str) -> Result<String> {
    let upload = Upload {
        parent_id: root,
        media_type: "text/markdown",
        ..Upload::default()
    };
    Ok(upload_with(session, name, text, upload).await?.file_id)
}

pub async fn graph_api(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = admin.get_json("/api/v1/notes/library").await?["rootId"]
        .as_str()
        .context("rootId")?
        .to_owned();
    let tag = &uuid()[..6];
    let a = note(
        &admin,
        &root,
        &format!("Alpha {tag}.md"),
        &format!("# Alpha {tag}\nSee [[Beta {tag}]] and [again](Beta%20{tag}.md), plus [[Alpha {tag}]] and [[Missing {tag}]]. #cluster{tag}\n"),
    )
    .await?;
    let b = note(
        &admin,
        &root,
        &format!("Beta {tag}.md"),
        &format!("# Beta {tag}\nLeaf note.\n"),
    )
    .await?;
    let c = note(
        &admin,
        &root,
        &format!("Gamma {tag}.md"),
        &format!("# Gamma {tag}\nBack to [[Alpha {tag}]].\n"),
    )
    .await?;

    let graph = admin.get_json("/api/v1/notes/graph").await?;
    let nodes = graph["nodes"].as_array().context("nodes")?;
    let node_a = nodes
        .iter()
        .find(|n| n["id"] == a.as_str())
        .context("node A")?;
    let node_b = nodes
        .iter()
        .find(|n| n["id"] == b.as_str())
        .context("node B")?;
    let node_c = nodes
        .iter()
        .find(|n| n["id"] == c.as_str())
        .context("node C")?;
    assert_eq!(node_a["title"], format!("Alpha {tag}"));
    assert_eq!(node_a["path"], format!("Alpha {tag}.md"));
    assert_eq!(node_a["tags"], json!([format!("cluster{tag}")]));
    assert_eq!(node_b["title"], format!("Beta {tag}"));
    assert_eq!(node_c["title"], format!("Gamma {tag}"));

    let ids = [a.as_str(), b.as_str(), c.as_str()];
    let edges: Vec<(&str, &str)> = graph["edges"]
        .as_array()
        .context("edges")?
        .iter()
        .filter_map(|e| Some((e["from"].as_str()?, e["to"].as_str()?)))
        .filter(|(from, to)| ids.contains(from) || ids.contains(to))
        .collect();
    assert_eq!(
        edges.len(),
        2,
        "expected 2 deduplicated edges, got {edges:?}"
    );
    assert!(
        edges.contains(&(a.as_str(), b.as_str())),
        "missing A -> B in {edges:?}"
    );
    assert!(
        edges.contains(&(c.as_str(), a.as_str())),
        "missing C -> A in {edges:?}"
    );
    assert!(
        !edges.contains(&(a.as_str(), a.as_str())),
        "unexpected self-edge in {edges:?}"
    );
    Ok(())
}

pub async fn graph_in_the_browser(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = admin.get_json("/api/v1/notes/library").await?["rootId"]
        .as_str()
        .context("rootId")?
        .to_owned();
    let tag = &uuid()[..6];
    let atlas = format!("Atlas {tag}");
    let beacon = format!("Beacon {tag}");
    let compass = format!("Compass {tag}");
    note(
        &admin,
        &root,
        &format!("{atlas}.md"),
        &format!("# {atlas}\nLinks to [[{beacon}]] twice: [[{beacon}]]. #orbit{tag}\n"),
    )
    .await?;
    note(
        &admin,
        &root,
        &format!("{beacon}.md"),
        &format!("# {beacon}\nDestination note.\n"),
    )
    .await?;
    note(
        &admin,
        &root,
        &format!("{compass}.md"),
        &format!("# {compass}\nPoints to [[{atlas}]].\n"),
    )
    .await?;

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

    // Open Graph view from the sidebar.
    page.get_by_role("button", "Graph").click().await?;
    page.get_by_role("heading", "Graph").wait_for().await?;

    let canvas = page.locator("canvas.note-graph-canvas");
    canvas.wait_for().await?;
    let box_ = canvas
        .bounding_box()
        .await?
        .context("canvas bounding box")?;
    assert!(
        box_.width > 100.0 && box_.height > 100.0,
        "unexpected canvas box: {box_:?}"
    );

    // Accessible list shows the notes with their link counts.
    let list = page.get_by_role("list", "Graph notes");
    list.get_by_role("button", contains(&atlas))
        .wait_for()
        .await?;
    list.get_by_role("button", contains(&beacon))
        .wait_for()
        .await?;
    list.get_by_role("button", contains(&compass))
        .wait_for()
        .await?;
    let atlas_text = list
        .get_by_role("button", contains(&atlas))
        .text_content()
        .await?;
    assert!(
        atlas_text.contains("2 links"),
        "expected 2 links on Atlas, got {atlas_text:?}"
    );

    page.screenshot(t.screenshot("notes-graph.png"), true)
        .await?;

    // Filter by #tag narrows the accessible list.
    let filter = page.get_by_label("Filter graph");
    filter.fill(&format!("#orbit{tag}")).await?;
    list.get_by_role("button", contains(&atlas))
        .wait_for()
        .await?;
    list.get_by_role("button", contains(&beacon))
        .wait_for_state(State::Hidden)
        .await?;
    filter.fill("").await?;
    list.get_by_role("button", contains(&beacon))
        .wait_for()
        .await?;

    // Clicking a list item opens the note editor sheet.
    list.get_by_role("button", contains(&beacon))
        .click()
        .await?;
    let editor = page.get_by_role("region", format!("Editor for {beacon}.md"));
    editor.wait_for().await?;

    // Local graph narrows to the open note (Beacon) and its direct neighbor (Atlas).
    page.get_by_label("Local graph").check().await?;
    list.get_by_role("button", contains(&beacon))
        .wait_for()
        .await?;
    list.get_by_role("button", contains(&atlas))
        .wait_for()
        .await?;
    list.get_by_role("button", contains(&compass))
        .wait_for_state(State::Hidden)
        .await?;
    page.get_by_label("Local graph").uncheck().await?;
    editor.get_by_role("button", "Close editor").click().await?;

    // Responsive phone layout: canvas stays visible without horizontal page overflow.
    page.set_viewport_size(390, 844).await?;
    canvas.wait_for().await?;
    let overflow = page
        .evaluate("() => document.documentElement.scrollWidth > window.innerWidth")
        .await?
        .as_bool()
        .unwrap_or(true);
    assert!(!overflow, "horizontal overflow on phone viewport");

    let failures = failures.lock().unwrap().clone();
    assert!(failures.is_empty(), "page errors: {failures:?}");
    Ok(())
}
