//! Spaced-repetition flashcards for Notes: `/api/v1/notes/cards` discovery, deck grouping and
//! scheduling filter across all supported Markdown card styles, and end-to-end browser review
//! verifying keyboard shortcuts, deck selection, opening the source note, phone layout, and
//! exact `<!--SR:...-->` comment writes leaving the rest of the Markdown untouched.

use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::sync::{Arc, Mutex};

pub const SUITE: Suite = Suite {
    name: "notes-cards",
    cases: &[case!(cards_api, 60), case!(review_in_the_browser, 120)],
};

async fn note(session: &Session, root: &str, name: &str, text: &str) -> Result<String> {
    let upload = Upload {
        parent_id: root,
        media_type: "text/markdown",
        ..Upload::default()
    };
    Ok(upload_with(session, name, text, upload).await?.file_id)
}

pub async fn cards_api(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = admin.get_json("/api/v1/notes/library").await?["rootId"]
        .as_str()
        .context("rootId")?
        .to_owned();
    let tag = &uuid()[..6];

    // Create a subfolder so folder-based decks are exercised alongside `#flashcards/...` tags.
    let db = admin.connect().await?;
    let folder_id = uuid();
    let folder_name = format!("Biology {tag}");
    reducer(|done| {
        db.reducers
            .create_folder_then(folder_id.clone(), root.clone(), folder_name.clone(), done)
    })
    .await?;

    // Note 1 (folder deck): single-line, reversible, multi-line, Logseq `#card`, code block & front-matter decoys,
    // plus one overdue card and one future-scheduled card.
    let bio_id = note(
        &admin,
        &folder_id,
        &format!("Cells {tag}.md"),
        &format!(
            concat!(
                "---\n",
                "title: Cells {tag}\n",
                "summary: Not::A card\n",
                "---\n",
                "# Cells {tag}\n\n",
                "```txt\n",
                "Code::Ignored\n",
                "```\n\n",
                "Powerhouse of the cell::Mitochondria\n",
                "<!--SR:!2026-05-01,2,250-->\n\n",
                "DNA:::Deoxyribonucleic acid\n\n",
                "What carries oxygen in blood?\n",
                "?\n",
                "Hemoglobin\n\n",
                "- Largest organ in the body #card\n",
                "  - Skin\n\n",
                "Already learned::Skip me\n",
                "<!--SR:!2099-01-01,30,250-->\n",
            ),
            tag = tag,
        ),
    )
    .await?;

    // Note 2 (`#flashcards/...` tag deck overrides folder/root).
    let tag_deck = format!("latin-{tag}");
    let latin_id = note(
        &admin,
        &root,
        &format!("Latin {tag}.md"),
        &format!("# Latin {tag}\n#flashcards/{tag_deck}\n\nCarpe diem::Seize the day\n"),
    )
    .await?;

    // Note 3 (root folder, no flashcards tag -> `"Notes"` deck).
    let root_id = note(
        &admin,
        &root,
        &format!("Root Card {tag}.md"),
        &format!("# Root Card {tag}\n\nTwo plus two {tag}::Four\n"),
    )
    .await?;

    // Invalid `?today=` returns 400.
    assert_eq!(
        admin
            .get("/api/v1/notes/cards?today=not-a-date")
            .await?
            .status,
        400
    );

    let payload = admin
        .get_json("/api/v1/notes/cards?today=2026-05-10")
        .await?;
    assert_eq!(payload["today"], "2026-05-10");

    let cards = payload["cards"].as_array().context("cards array")?;
    let bio_cards: Vec<&Value> = cards
        .iter()
        .filter(|c| c["noteId"] == bio_id.as_str())
        .collect();
    assert_eq!(
        bio_cards.len(),
        5,
        "expected 5 due/new cards in bio note (future card excluded), got {bio_cards:?}"
    );

    // Overdue card comes first with its schedule fields populated.
    let overdue = bio_cards[0];
    assert_eq!(overdue["title"], format!("Cells {tag}"));
    assert_eq!(overdue["path"], format!("{folder_name}/Cells {tag}.md"));
    assert_eq!(overdue["deck"], folder_name);
    assert_eq!(overdue["question"], "Powerhouse of the cell");
    assert_eq!(overdue["answer"], "Mitochondria");
    assert_eq!(overdue["due"], "2026-05-01");
    assert_eq!(overdue["interval"], 2);
    assert_eq!(overdue["ease"], 250);
    assert_eq!(overdue["cardIndex"], 0);
    assert!(overdue["revision"].as_u64().unwrap_or(0) >= 1);

    // Remaining 4 bio cards are new: reversible forward, reversible reverse, multi-line, Logseq `#card`.
    let new_qa: Vec<(&str, &str)> = bio_cards[1..]
        .iter()
        .map(|c| {
            (
                c["question"].as_str().unwrap_or(""),
                c["answer"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        new_qa,
        vec![
            ("DNA", "Deoxyribonucleic acid"),
            ("Deoxyribonucleic acid", "DNA"),
            ("What carries oxygen in blood?", "Hemoglobin"),
            ("Largest organ in the body", "- Skin"),
        ]
    );
    assert!(bio_cards[1..].iter().all(|c| c["due"].is_null()));

    // Tag deck and default root deck.
    let latin_card = cards
        .iter()
        .find(|c| c["noteId"] == latin_id.as_str())
        .context("latin card")?;
    assert_eq!(latin_card["deck"], tag_deck);
    assert_eq!(latin_card["question"], "Carpe diem");
    assert_eq!(latin_card["answer"], "Seize the day");

    let root_card = cards
        .iter()
        .find(|c| c["noteId"] == root_id.as_str())
        .context("root card")?;
    assert_eq!(root_card["deck"], "Notes");
    assert_eq!(root_card["question"], format!("Two plus two {tag}"));

    // Per-deck counts include both due/new and total (including future-scheduled cards).
    let decks = payload["decks"].as_array().context("decks array")?;
    let bio_deck = decks
        .iter()
        .find(|d| d["name"] == folder_name.as_str())
        .context("bio deck")?;
    assert_eq!(bio_deck["due"], 1);
    assert_eq!(bio_deck["new"], 4);
    assert_eq!(bio_deck["total"], 6);

    let latin_deck = decks
        .iter()
        .find(|d| d["name"] == tag_deck.as_str())
        .context("latin deck")?;
    assert_eq!(latin_deck["due"], 0);
    assert_eq!(latin_deck["new"], 1);
    assert_eq!(latin_deck["total"], 1);

    Ok(())
}

pub async fn review_in_the_browser(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let root = admin.get_json("/api/v1/notes/library").await?["rootId"]
        .as_str()
        .context("rootId")?
        .to_owned();
    let tag = &uuid()[..6];
    let deck = format!("quiz-{tag}");
    let title = format!("Quiz {tag}");
    let filename = format!("{title}.md");

    let original = format!(
        concat!(
            "---\n",
            "tags:\n",
            "  - flashcards/{deck}\n",
            "---\n",
            "# {title}\n\n",
            "Keep this paragraph untouched.\n\n",
            "```rust\n",
            "// Fake::Not a card\n",
            "```\n\n",
            "Capital of France::Paris\n\n",
            "Atomic symbol for gold?\n",
            "?\n",
            "Au\n\n",
            "- Boiling point of water #card\n",
            "  - 100 C at 1 atm\n\n",
            "Bonjour:::Hello\n\n",
            "Trailing note text stays intact.\n",
        ),
        deck = deck,
        title = title,
    );
    let note_id = note(&admin, &root, &filename, &original).await?;

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

    // Compute browser local date so assertions match the client's `today()` regardless of host TZ.
    let today_str = page
        .evaluate(
            "() => { const d = new Date(); \
             const p = n => String(n).padStart(2, '0'); \
             return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`; }",
        )
        .await?
        .as_str()
        .context("browser local date")?
        .to_owned();
    let today =
        chrono::NaiveDate::parse_from_str(&today_str, "%Y-%m-%d").context("parse browser date")?;
    let due_1d = (today + chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let due_3d = (today + chrono::Duration::days(3))
        .format("%Y-%m-%d")
        .to_string();

    // Sidebar "Review" entry shows the due count and opens the Review view.
    let review_nav = page.get_by_role("button", regex(r"Review, [1-9]\d* due"));
    review_nav.wait_for().await?;
    review_nav.click().await?;
    page.get_by_role("heading", "Review").wait_for().await?;

    // Select our isolated deck so cards from other tests do not interfere.
    let deck_btn = page
        .get_by_role("group", "Decks")
        .get_by_role("button", contains(&deck));
    deck_btn.wait_for().await?;
    deck_btn.click().await?;

    let question = page.get_by_role("region", "Question");
    let answer = page.get_by_role("region", "Answer");

    // Card 1 (single-line): click Show answer, check interval hints, test Open note, grade Good.
    question
        .get_by_text(contains("Capital of France"))
        .wait_for()
        .await?;
    page.get_by_role("button", contains("Show answer"))
        .click()
        .await?;
    answer.get_by_text(contains("Paris")).wait_for().await?;
    let good_btn = page.get_by_role("button", contains("Good"));
    let again_btn = page.get_by_role("button", contains("Again"));
    assert!(
        good_btn.text_content().await?.contains("3d"),
        "expected 3d interval on Good"
    );
    assert!(
        again_btn.text_content().await?.contains("1d"),
        "expected 1d interval on Again"
    );

    page.screenshot(t.screenshot("notes-cards.png"), true)
        .await?;

    // "Open note" opens the source note in the right editor sheet without losing the card.
    page.get_by_role("button", "Open note").click().await?;
    let editor = page.get_by_role("region", format!("Editor for {filename}"));
    editor.wait_for().await?;
    editor.get_by_role("button", "Close editor").click().await?;
    editor.wait_for_state(State::Hidden).await?;

    good_btn.click().await?;

    // Card 2 (multi-line): reveal with Space, grade Again with key 1.
    question
        .get_by_text(contains("Atomic symbol for gold?"))
        .wait_for()
        .await?;
    page.keyboard().press("Space").await?;
    answer.get_by_text(contains("Au")).wait_for().await?;
    page.keyboard().press("1").await?;

    // Card 3 (Logseq #card): reveal with Space, grade Good with key 3.
    question
        .get_by_text(contains("Boiling point of water"))
        .wait_for()
        .await?;
    page.keyboard().press("Space").await?;
    answer
        .get_by_text(contains("100 C at 1 atm"))
        .wait_for()
        .await?;
    page.keyboard().press("3").await?;

    // Card 4 (reversible forward): reveal with Space, grade Good with key 3.
    question.get_by_text(contains("Bonjour")).wait_for().await?;
    page.keyboard().press("Space").await?;
    answer.get_by_text(contains("Hello")).wait_for().await?;
    page.keyboard().press("3").await?;

    // Card 5 (reversible reverse): reveal with Space, grade Again with key 1.
    question.get_by_text(contains("Hello")).wait_for().await?;
    page.keyboard().press("Space").await?;
    answer.get_by_text(contains("Bonjour")).wait_for().await?;
    page.keyboard().press("1").await?;

    // End screen appears once all cards in the deck are graded.
    page.get_by_role("heading", "Review complete")
        .wait_for()
        .await?;

    // Verify saved Markdown has exact `<!--SR:...-->` comments and nothing else changed.
    let saved = admin
        .get(&format!("/api/v1/files/{note_id}/content"))
        .await?
        .expect(200)?
        .text();
    let expected = format!(
        concat!(
            "---\n",
            "tags:\n",
            "  - flashcards/{deck}\n",
            "---\n",
            "# {title}\n\n",
            "Keep this paragraph untouched.\n\n",
            "```rust\n",
            "// Fake::Not a card\n",
            "```\n\n",
            "Capital of France::Paris\n",
            "<!--SR:!{due_3d},3,250-->\n\n",
            "Atomic symbol for gold?\n",
            "?\n",
            "Au\n",
            "<!--SR:!{due_1d},1,230-->\n\n",
            "- Boiling point of water #card\n",
            "  - 100 C at 1 atm\n",
            "<!--SR:!{due_3d},3,250-->\n\n",
            "Bonjour:::Hello\n",
            "<!--SR:!{due_3d},3,250!{due_1d},1,230-->\n\n",
            "Trailing note text stays intact.\n",
        ),
        deck = deck,
        title = title,
        due_3d = due_3d,
        due_1d = due_1d,
    );
    assert_eq!(saved, expected, "saved Markdown mismatch");

    // Re-querying the endpoint confirms none of the 5 graded cards are due today anymore.
    let after = admin
        .get_json(&format!("/api/v1/notes/cards?today={today_str}"))
        .await?;
    let remaining: Vec<&Value> = after["cards"]
        .as_array()
        .context("cards after review")?
        .iter()
        .filter(|c| c["noteId"] == note_id.as_str())
        .collect();
    assert!(
        remaining.is_empty(),
        "expected no cards left due today, got {remaining:?}"
    );

    // Responsive phone layout: no horizontal overflow.
    page.set_viewport_size(390, 844).await?;
    page.get_by_role("heading", "Review complete")
        .wait_for()
        .await?;
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
