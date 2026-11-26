//! Books in the browser (`books-browser.mjs`): home grid & covers, book details & Want to read,
//! EPUB reader with paging/typography/contents/bookmarks/reload/other-device conflict, CBZ manga
//! right-to-left reader, PDF reader, Files PDF preview, and phone layout.

use super::photos_books_apps_support::{cbz, create_user, epub};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::sync::{Arc, Mutex};

pub const SUITE: Suite = Suite {
    name: "books-browser",
    cases: &[case!(books_journey, 180)],
};

/// A two-page PDF with title, author, an outline pointing to both pages and lines of text
/// ('lighthouse' on both pages, for Find).
fn pdf_with_outline() -> Vec<u8> {
    let stream = |text: &str| format!("<< /Length {} >>\nstream\n{text}\nendstream", text.len());
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /Outlines 9 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /Font << /F1 7 0 R >> >> >>".to_owned(),
        stream("BT /F1 24 Tf 40 340 Td (Page one) Tj ET BT /F1 12 Tf 40 300 Td (The lighthouse keeper) Tj 0 -16 Td (counted every wave.) Tj ET"),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >>".to_owned(),
        stream("BT /F1 24 Tf 40 340 Td (Page two) Tj ET BT /F1 12 Tf 40 300 Td (A second lighthouse) Tj 0 -16 Td (stood across the bay.) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        "<< /Title (A Fixture Paper) /Author (Grace Columns) >>".to_owned(),
        "<< /Type /Outlines /First 10 0 R /Last 11 0 R /Count 2 >>".to_owned(),
        "<< /Title (First section) /Parent 9 0 R /Next 11 0 R /Dest [3 0 R /Fit] >>".to_owned(),
        "<< /Title (Second section) /Parent 9 0 R /Prev 10 0 R /Dest [5 0 R /Fit] >>".to_owned(),
    ];
    let mut body = String::from("%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(body.len());
        body.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let xref = body.len();
    body.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        body.push_str(&format!("{offset:010} 00000 n \n"));
    }
    body.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R /Info 8 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    body.into_bytes()
}

pub async fn books_journey(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let username = format!("books-ui-{}", &uuid()[..6]);
    create_user(&admin, &username, "Books tester", PASSWORD).await?;
    let reader = login_as(&base, &username, PASSWORD).await?;

    let root = reader.get_json("/api/v1/books/library").await?["rootId"]
        .as_str()
        .context("books rootId")?
        .to_owned();
    let up = Upload {
        parent_id: &root,
        ..Upload::default()
    };
    let voyage = upload_with(
        &reader,
        "the-test-voyage.epub",
        epub(
            "The Test Voyage",
            "Ishmael Fixture",
            Some(("Fixture Series", 2)),
            false,
        ),
        up.clone(),
    )
    .await?;
    upload_with(
        &reader,
        "second.epub",
        epub(
            "A Second Voyage",
            "Ishmael Fixture",
            Some(("Fixture Series", 3)),
            false,
        ),
        up.clone(),
    )
    .await?;
    let comic = upload_with(&reader, "fixture-comic.cbz", cbz(), up.clone()).await?;
    let paper = upload_with(
        &reader,
        "paper.pdf",
        pdf_with_outline(),
        Upload {
            parent_id: &root,
            media_type: "application/pdf",
            ..Upload::default()
        },
    )
    .await?;

    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 960))
        .await?;
    let page = context.new_page().await?;
    // The fixture's own <script> is blocked (expected), and the other-device step answers 409 on purpose.
    let errors = page.collect_errors(|text| {
        (text.contains("401") && text.contains("Unauthorized"))
            || text.contains("status of 404")
            || text.contains("status of 409")
            || text.contains("directive 'script-src 'none''")
    });

    page.goto(&base).await?;
    page.get_by_label("Username").fill(&username).await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.get_by_role("button", "All apps").click().await?;
    page.get_by_role("dialog", "All apps")
        .get_by_role("link", "Books")
        .click()
        .await?;
    page.wait_for_url(|u| u.path().contains("/books")).await?;

    let main = page.get_by_role("main", "Home");
    main.get_by_role("heading", "Recently added")
        .wait_for()
        .await?;
    main.get_by_role("button", regex("The Test Voyage"))
        .first()
        .wait_for()
        .await?;
    // Covers come from the files (EPUB cover image, first PDF page, first comic page).
    page.wait_for_function("() => [...document.querySelectorAll('.books-cover img')].filter(img => img.complete && img.naturalWidth > 0).length >= 4").await?;
    page.screenshot(t.screenshot("books-home-1440.png"), false)
        .await?;

    // Details: series, actions and Want to read.
    main.get_by_role("button", regex("The Test Voyage"))
        .first()
        .click()
        .await?;
    let details = page.get_by_role("complementary", "Book details");
    details
        .get_by_role("heading", "The Test Voyage")
        .wait_for()
        .await?;
    details
        .get_by_text("Fixture Series \u{00b7} Book 2 of 2")
        .wait_for()
        .await?;
    details
        .get_by_role("button", "Want to read")
        .click()
        .await?;
    page.get_by_text("Marked as want to read")
        .wait_for()
        .await?;
    assert_eq!(
        details
            .get_by_role("button", "Want to read")
            .get_attribute("aria-pressed")
            .await?,
        Some("true".into())
    );
    page.screenshot(t.screenshot("books-details-1440.png"), false)
        .await?;

    // EPUB reader: pages turn, typography applies inside the chapter, contents jump, place is saved.
    details.get_by_role("button", "Read").click().await?;
    let book = page.get_by_role("main", "Reading The Test Voyage");
    book.wait_for().await?;
    let frame = page.frame_locator("iframe.epub-view");
    frame.get_by_role("heading", "Departure").wait_for().await?;
    assert_eq!(
        page.evaluate("() => document.querySelector('iframe.epub-view').contentDocument.title")
            .await?,
        json!("Departure"),
        "book scripts do not run"
    );
    let scroll_left = page
        .evaluate("() => { const d = document.querySelector('iframe.epub-view').contentDocument; return (d.scrollingElement ?? d.documentElement).scrollLeft; }")
        .await?;
    assert_eq!(scroll_left, json!(0));
    book.focus().await?;
    page.keyboard().press("ArrowRight").await?;
    page.wait_for_function("() => { const d = document.querySelector('iframe.epub-view').contentDocument; return (d.scrollingElement ?? d.documentElement).scrollLeft > 0; }").await?;

    book.get_by_role("button", "Text and layout")
        .click()
        .await?;
    let typography = page.get_by_role("dialog", "Text and layout");
    typography
        .get_by_role("button", regex("Sepia"))
        .click()
        .await?;
    page.wait_for_function("() => getComputedStyle(document.querySelector('iframe.epub-view').contentDocument.documentElement).backgroundColor === 'rgb(244, 236, 216)'").await?;
    typography
        .get_by_role("button", "Larger text")
        .click()
        .await?;
    page.wait_for_function("() => getComputedStyle(document.querySelector('iframe.epub-view').contentDocument.documentElement).fontSize === '19px'").await?;
    page.screenshot(t.screenshot("books-reader-epub-1440.png"), false)
        .await?;
    page.keyboard().press("Escape").await?;
    typography.wait_for_state(State::Detached).await?;

    let panel = book.get_by_role("complementary", "Contents and bookmarks");
    if !panel.is_visible().await? {
        book.get_by_role("button", "Contents and bookmarks")
            .click()
            .await?;
    }
    panel
        .get_by_role("button", "The mast breaks")
        .click()
        .await?;
    frame
        .get_by_text(contains("Paragraph 3 of The Storm."))
        .wait_for()
        .await?;
    page.wait_for_function(
        "() => document.querySelector('iframe.epub-view').contentDocument.title === 'The Storm'",
    )
    .await?;
    book.get_by_role("button", "Add bookmark").click().await?;
    book.get_by_role("button", "Remove bookmark")
        .wait_for()
        .await?;
    panel.get_by_role("tab", regex("Bookmarks")).click().await?;
    panel
        .get_by_role("button", regex("^The mast breaks"))
        .wait_for()
        .await?;

    // Internal links stay in the reader.
    page.wait_for_timeout(1600).await;
    let state = reader
        .get_json(&format!("/api/v1/books/{}", voyage.file_id))
        .await?;
    assert_eq!(
        state["state"]["position"]["spine"], 1,
        "the place is saved to the account"
    );
    assert_eq!(state["state"]["status"], "reading");
    assert!(
        state["state"]["device"]
            .as_str()
            .unwrap_or_default()
            .contains("on")
    );
    assert_eq!(state["marks"].as_array().map(Vec::len), Some(1));

    // Reload: the reader reopens at the saved chapter, with the account's typography.
    page.reload().await?;
    page.get_by_role("main", "Reading The Test Voyage")
        .wait_for()
        .await?;
    page.wait_for_function(
        "() => document.querySelector('iframe.epub-view')?.contentDocument?.title === 'The Storm'",
    )
    .await?;
    page.wait_for_function("() => getComputedStyle(document.querySelector('iframe.epub-view').contentDocument.documentElement).backgroundColor === 'rgb(244, 236, 216)'").await?;

    // Another device moved on: saving here offers to go there instead of overwriting.
    let remote = reader
        .put(
            &format!("/api/v1/books/{}/state", voyage.file_id),
            &json!({"revision": state["state"]["revision"], "position": {"spine": 0, "fraction": 0}, "progress": 0.01, "device": "Phone"}),
        )
        .await?;
    assert_eq!(remote.status, 200);
    page.get_by_role("main", "Reading The Test Voyage")
        .focus()
        .await?;
    page.keyboard().press("ArrowRight").await?;
    let conflict = page.get_by_role("alertdialog", "Your place changed on another device");
    conflict.wait_for().await?;
    conflict.get_by_text(regex("on Phone")).wait_for().await?;
    page.screenshot(t.screenshot("books-reader-conflict-1440.png"), false)
        .await?;
    conflict.get_by_role("button", "Go there").click().await?;
    page.wait_for_function(
        "() => document.querySelector('iframe.epub-view')?.contentDocument?.title === 'Departure'",
    )
    .await?;
    page.get_by_role("button", "Back to library")
        .click()
        .await?;
    page.get_by_role("heading", "Continue reading")
        .wait_for()
        .await?;

    // Comic: pages turn right to left from the comic's own metadata.
    page.goto(&format!("{base}/books?book={}&read=1", comic.file_id))
        .await?;
    let comic_reader = page.get_by_role("main", regex("^Reading "));
    comic_reader.wait_for().await?;
    comic_reader
        .get_by_role("img", "Page 1 of 3")
        .wait_for()
        .await?;
    comic_reader.focus().await?;
    page.keyboard().press("ArrowLeft").await?;
    comic_reader
        .get_by_role("img", "Page 2 of 3")
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("books-reader-comic-1440.png"), false)
        .await?;

    // PDF: drawn by the hayro Web Worker, with page tracking, zoom and outline navigation.
    page.goto(&format!("{base}/books?book={}&read=1", paper.file_id))
        .await?;
    let pdf_reader = page.get_by_role("main", "Reading A Fixture Paper");
    pdf_reader.wait_for().await?;
    page.locator(".pdf-view-page.is-drawn")
        .first()
        .wait_for()
        .await?;
    assert_eq!(
        page.evaluate("() => { const c = document.querySelector('.pdf-view-page.is-drawn canvas'); return !!c && c.width > 0 && c.height > 0; }").await?,
        json!(true),
        "drawn PDF canvas has non-zero pixel dimensions"
    );
    pdf_reader.get_by_text("Page 1 of 2").wait_for().await?;

    // The text layer: the page's lines as invisible, selectable spans over their ink.
    let first_page = ".pdf-view-page[data-page=\"0\"]";
    page.wait_for_function("() => document.querySelector('.pdf-view-page[data-page=\"0\"] .pdf-view-text')?.textContent.includes('counted every wave.')").await?;
    let layer = page
        .evaluate(
            "() => { const p = document.querySelector('.pdf-view-page[data-page=\"0\"]'); const spans = [...p.querySelectorAll('.pdf-view-text span')]; \
             const s = spans.find(e => e.textContent === 'The lighthouse keeper'); const pb = p.getBoundingClientRect(); const r = s.getBoundingClientRect(); \
             const scale = pb.width / 300; \
             return { texts: spans.map(e => e.textContent), color: getComputedStyle(s).color, select: getComputedStyle(s).userSelect, \
               left: (r.left - pb.left) / scale, top: (r.top - pb.top) / scale, width: r.width / scale, height: r.height / scale, role: p.getAttribute('role') }; }",
        )
        .await?;
    assert_eq!(
        layer["texts"],
        json!(["Page one", "The lighthouse keeper", "counted every wave."]),
        "{layer}"
    );
    assert_eq!(
        layer["color"], "rgba(0, 0, 0, 0)",
        "the text layer is invisible"
    );
    assert_ne!(layer["select"], "none", "the text layer is selectable");
    assert_eq!(layer["role"], "group");
    // "The lighthouse keeper" sits at (40, 400 - 300) points, 12 points high, over its ink.
    let near = |key: &str, want: f64, slack: f64| {
        (layer[key].as_f64().unwrap_or(-1.0) - want).abs() <= slack
    };
    assert!(
        near("left", 40.0, 1.5) && near("top", 100.0 - 9.6, 2.0) && near("height", 12.0, 2.0),
        "{layer}"
    );
    assert!(
        near("width", 118.0, 6.0),
        "the span is stretched to the run's length: {layer}"
    );

    // Selecting two lines offers Highlight; it lands in Notes › Highlights with a link back.
    page.evaluate(&format!(
        "() => {{ const spans = [...document.querySelectorAll('{first_page} .pdf-view-text span')]; \
         const a = spans.find(e => e.textContent === 'The lighthouse keeper').firstChild; \
         const b = spans.find(e => e.textContent === 'counted every wave.').firstChild; \
         const r = document.createRange(); r.setStart(a, 4); r.setEnd(b, 13); \
         getSelection().removeAllRanges(); getSelection().addRange(r); }}"
    ))
    .await?;
    assert_eq!(
        page.evaluate("() => getSelection().toString()").await?,
        json!("lighthouse keeper\ncounted every")
    );
    pdf_reader
        .get_by_role("button", "Highlight")
        .click()
        .await?;
    pdf_reader
        .get_by_text(contains("Highlight saved to Notes"))
        .wait_for()
        .await?;
    page.locator(&format!("{first_page} .pdf-view-highlight"))
        .first()
        .wait_for()
        .await?;
    let notes = reader.get_json("/api/v1/notes/library").await?;
    let folder = notes["folders"]
        .as_array()
        .and_then(|f| {
            f.iter()
                .find(|f| f["name"] == "Highlights" && f["parentId"] == notes["rootId"])
        })
        .context("Highlights folder in Notes")?;
    let note = notes["notes"]
        .as_array()
        .and_then(|n| {
            n.iter()
                .find(|n| n["parentId"] == folder["id"] && n["name"] == "A Fixture Paper.md")
        })
        .context("highlights note")?;
    let text = reader
        .get(&format!(
            "/api/v1/files/{}/content",
            note["id"].as_str().unwrap_or_default()
        ))
        .await?
        .text();
    let link = format!("/books?book={}&page=1", paper.file_id);
    assert!(
        text.starts_with("# Highlights: A Fixture Paper\n\n> lighthouse keeper\n> counted every\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!("> \u{2014} [A Fixture Paper, p. 1]({link}) ^hl-")),
        "{text}"
    );
    assert!(
        text.contains(&format!("]: {link} \"dar-highlight 1:")),
        "{text}"
    );
    let initial_width = page
        .evaluate("() => document.querySelector('.pdf-view-page').getBoundingClientRect().width")
        .await?
        .as_f64()
        .context("initial PDF page width")?;
    pdf_reader.get_by_role("button", "Zoom in").click().await?;
    page.wait_for_function_with(
        "(w) => document.querySelector('.pdf-view-page').getBoundingClientRect().width > w + 10",
        json!(initial_width),
    )
    .await?;
    pdf_reader
        .get_by_role("button", "Fit width")
        .click()
        .await?;
    let pdf_panel = pdf_reader.get_by_role("complementary", "Contents and bookmarks");
    if !pdf_panel.is_visible().await? {
        pdf_reader
            .get_by_role("button", "Contents and bookmarks")
            .click()
            .await?;
    }
    pdf_panel.get_by_role("tab", "Contents").click().await?;
    pdf_panel
        .get_by_role("button", regex("Second section"))
        .click()
        .await?;
    pdf_reader.get_by_text("Page 2 of 2").wait_for().await?;
    page.screenshot(t.screenshot("books-reader-pdf-1440.png"), false)
        .await?;

    // Find (Ctrl+F): counts matches over every page, starts at this page, moves and scrolls.
    let count_is = |want: &'static str| {
        let page = page.clone();
        async move {
            page.wait_for_function_with(
                "(want) => document.querySelector('.books-find-count')?.textContent === want",
                json!(want),
            )
            .await
        }
    };
    pdf_reader.focus().await?;
    page.keyboard().press("Control+f").await?;
    let find = pdf_reader.get_by_role("searchbox", "Find in book");
    find.wait_for().await?;
    page.wait_for_function(
        "() => document.activeElement?.getAttribute('aria-label') === 'Find in book'",
    )
    .await?;
    find.fill("LIGHTHOUSE").await?;
    count_is("2 of 2").await?;
    page.locator(".pdf-view-page[data-page=\"1\"] .pdf-view-match.is-current")
        .wait_for()
        .await?;
    find.press("Enter").await?;
    count_is("1 of 2").await?;
    pdf_reader.get_by_text("Page 1 of 2").wait_for().await?;
    page.locator(&format!("{first_page} .pdf-view-match.is-current"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("books-reader-pdf-find-1440.png"), false)
        .await?;
    pdf_reader
        .get_by_role("button", "Next match")
        .click()
        .await?;
    count_is("2 of 2").await?;
    pdf_reader.get_by_text("Page 2 of 2").wait_for().await?;
    find.fill("no such words").await?;
    count_is("No matches").await?;
    find.press("Escape").await?;
    find.wait_for_state(State::Detached).await?;
    assert_eq!(page.locator(".pdf-view-match").count().await?, 0);
    pdf_reader.wait_for().await?;

    // The note's link opens the book at the highlight's page, with the highlight drawn again.
    page.wait_for_timeout(1600).await;
    let saved = reader
        .get_json(&format!("/api/v1/books/{}", paper.file_id))
        .await?;
    assert_eq!(
        saved["state"]["position"]["page"], 1,
        "the saved place is page 2"
    );
    page.goto(&format!("{base}{link}")).await?;
    let pdf_reader = page.get_by_role("main", "Reading A Fixture Paper");
    pdf_reader.get_by_text("Page 1 of 2").wait_for().await?;
    page.locator(&format!("{first_page} .pdf-view-highlight"))
        .first()
        .wait_for()
        .await?;
    page.locator(&format!("{first_page}.is-drawn"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("books-reader-pdf-highlight-1440.png"), false)
        .await?;
    page.emulate_media(Some("dark"), None, None).await?;
    page.wait_for_timeout(300).await;
    page.screenshot(
        t.screenshot("books-reader-pdf-highlight-dark-1440.png"),
        false,
    )
    .await?;
    page.emulate_media(Some("light"), None, None).await?;

    // Files' PDF preview uses the same viewer.
    page.goto(&format!("{base}/?folder={root}")).await?;
    page.get_by_role("button", "Actions for paper.pdf")
        .click()
        .await?;
    page.get_by_role("menuitem", "Details").click().await?;
    page.get_by_role("button", "Preview").click().await?;
    let preview = page.get_by_role("region", "Preview of paper.pdf");
    preview
        .locator(".pdf-view-page.is-drawn")
        .first()
        .wait_for()
        .await?;
    assert_eq!(
        page.evaluate("() => { const c = document.querySelector('[aria-label=\"Preview of paper.pdf\"] .pdf-view-page.is-drawn canvas'); return !!c && c.width > 0 && c.height > 0; }").await?,
        json!(true),
        "preview PDF canvas has non-zero pixel dimensions"
    );
    page.screenshot(t.screenshot("books-files-pdf-preview-1440.png"), false)
        .await?;

    // Phones: chips, full-screen details and reader.
    let phone = browser.new_context(ContextOptions::phone(390, 844)).await?;
    let mobile = phone.new_page().await?;
    let phone_errors = Arc::new(Mutex::new(Vec::new()));
    let sink = phone_errors.clone();
    mobile.on_page_error(move |message| sink.lock().unwrap().push(message.clone()));
    mobile.goto(&base).await?;
    mobile.get_by_label("Username").fill(&username).await?;
    mobile.get_by_label("Password").fill(PASSWORD).await?;
    mobile.get_by_role("button", "Sign in").click().await?;
    mobile.locator("header.topbar").wait_for().await?;
    mobile.goto(&format!("{base}/books")).await?;
    if let Err(error) = mobile
        .get_by_role("heading", "Continue reading")
        .wait_for()
        .await
    {
        let _ = mobile
            .screenshot(t.screenshot("books-failure-390.png"), false)
            .await;
        return Err(error);
    }
    assert_eq!(
        mobile
            .evaluate("() => document.documentElement.scrollWidth <= innerWidth + 1")
            .await?,
        json!(true),
        "no sideways scrolling on phones"
    );
    mobile
        .screenshot(t.screenshot("books-home-390.png"), false)
        .await?;
    mobile
        .get_by_role("button", regex("A Second Voyage"))
        .first()
        .click()
        .await?;
    mobile
        .get_by_role("complementary", "Book details")
        .get_by_role("heading", "A Second Voyage")
        .wait_for()
        .await?;
    mobile
        .screenshot(t.screenshot("books-details-390.png"), false)
        .await?;
    mobile.get_by_role("button", "Read").click().await?;
    mobile
        .frame_locator("iframe.epub-view")
        .get_by_role("heading", "Departure")
        .wait_for()
        .await?;
    mobile
        .screenshot(t.screenshot("books-reader-390.png"), false)
        .await?;

    phone.close().await?;
    context.close().await?;

    let mut all = errors.lock().unwrap().clone();
    all.extend(phone_errors.lock().unwrap().iter().cloned());
    assert_eq!(all, Vec::<String>::new());
    Ok(())
}
