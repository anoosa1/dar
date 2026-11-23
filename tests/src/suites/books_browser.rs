//! Books in the browser (`books-browser.mjs`): home grid & covers, book details & Want to read,
//! EPUB reader with paging/typography/contents/bookmarks/reload/other-device conflict, CBZ manga
//! right-to-left reader, PDF reader, Files PDF preview, and phone layout.

use super::photos_books_apps_support::{cbz, create_user, epub, pdf};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::sync::{Arc, Mutex};

pub const SUITE: Suite = Suite { name: "books-browser", cases: &[case!(books_journey, 180)] };

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
    let up = Upload { parent_id: &root, ..Upload::default() };
    let voyage = upload_with(
        &reader,
        "the-test-voyage.epub",
        epub("The Test Voyage", "Ishmael Fixture", Some(("Fixture Series", 2)), false),
        up.clone(),
    )
    .await?;
    upload_with(
        &reader,
        "second.epub",
        epub("A Second Voyage", "Ishmael Fixture", Some(("Fixture Series", 3)), false),
        up.clone(),
    )
    .await?;
    let comic = upload_with(&reader, "fixture-comic.cbz", cbz(), up.clone()).await?;
    let paper = upload_with(
        &reader,
        "paper.pdf",
        pdf(),
        Upload { parent_id: &root, media_type: "application/pdf", ..Upload::default() },
    )
    .await?;

    let browser = Browser::shared().await?;
    let context = browser.new_context(ContextOptions::sized(1440, 960)).await?;
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
    page.get_by_role("navigation", "Applications").get_by_role("link", "Books").click().await?;
    page.wait_for_url(|u| u.path().contains("/books")).await?;

    let main = page.get_by_role("main", "Home");
    main.get_by_role("heading", "Recently added").wait_for().await?;
    main.get_by_role("button", regex("The Test Voyage")).first().wait_for().await?;
    // Covers come from the files (EPUB cover image, first PDF page, first comic page).
    page.wait_for_function("() => [...document.querySelectorAll('.books-cover img')].filter(img => img.complete && img.naturalWidth > 0).length >= 4").await?;
    page.screenshot(t.screenshot("books-home-1440.png"), false).await?;

    // Details: series, actions and Want to read.
    main.get_by_role("button", regex("The Test Voyage")).first().click().await?;
    let details = page.get_by_role("complementary", "Book details");
    details.get_by_role("heading", "The Test Voyage").wait_for().await?;
    details.get_by_text("Fixture Series \u{00b7} Book 2 of 2").wait_for().await?;
    details.get_by_role("button", "Want to read").click().await?;
    page.get_by_text("Marked as want to read").wait_for().await?;
    assert_eq!(details.get_by_role("button", "Want to read").get_attribute("aria-pressed").await?, Some("true".into()));
    page.screenshot(t.screenshot("books-details-1440.png"), false).await?;

    // EPUB reader: pages turn, typography applies inside the chapter, contents jump, place is saved.
    details.get_by_role("button", "Read").click().await?;
    let book = page.get_by_role("main", "Reading The Test Voyage");
    book.wait_for().await?;
    let frame = page.frame_locator("iframe.epub-view");
    frame.get_by_role("heading", "Departure").wait_for().await?;
    assert_eq!(
        page.evaluate("() => document.querySelector('iframe.epub-view').contentDocument.title").await?,
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

    book.get_by_role("button", "Text and layout").click().await?;
    let typography = page.get_by_role("dialog", "Text and layout");
    typography.get_by_role("button", regex("Sepia")).click().await?;
    page.wait_for_function("() => getComputedStyle(document.querySelector('iframe.epub-view').contentDocument.documentElement).backgroundColor === 'rgb(244, 236, 216)'").await?;
    typography.get_by_role("button", "Larger text").click().await?;
    page.wait_for_function("() => getComputedStyle(document.querySelector('iframe.epub-view').contentDocument.documentElement).fontSize === '19px'").await?;
    page.screenshot(t.screenshot("books-reader-epub-1440.png"), false).await?;
    page.keyboard().press("Escape").await?;
    typography.wait_for_state(State::Detached).await?;

    let panel = book.get_by_role("complementary", "Contents and bookmarks");
    if !panel.is_visible().await? {
        book.get_by_role("button", "Contents and bookmarks").click().await?;
    }
    panel.get_by_role("button", "The mast breaks").click().await?;
    frame.get_by_text(contains("Paragraph 3 of The Storm.")).wait_for().await?;
    page.wait_for_function("() => document.querySelector('iframe.epub-view').contentDocument.title === 'The Storm'").await?;
    book.get_by_role("button", "Add bookmark").click().await?;
    book.get_by_role("button", "Remove bookmark").wait_for().await?;
    panel.get_by_role("tab", regex("Bookmarks")).click().await?;
    panel.get_by_role("button", regex("^The mast breaks")).wait_for().await?;

    // Internal links stay in the reader.
    page.wait_for_timeout(1600).await;
    let state = reader.get_json(&format!("/api/v1/books/{}", voyage.file_id)).await?;
    assert_eq!(state["state"]["position"]["spine"], 1, "the place is saved to the account");
    assert_eq!(state["state"]["status"], "reading");
    assert!(state["state"]["device"].as_str().unwrap_or_default().contains("on"));
    assert_eq!(state["marks"].as_array().map(Vec::len), Some(1));

    // Reload: the reader reopens at the saved chapter, with the account's typography.
    page.reload().await?;
    page.get_by_role("main", "Reading The Test Voyage").wait_for().await?;
    page.wait_for_function("() => document.querySelector('iframe.epub-view')?.contentDocument?.title === 'The Storm'").await?;
    page.wait_for_function("() => getComputedStyle(document.querySelector('iframe.epub-view').contentDocument.documentElement).backgroundColor === 'rgb(244, 236, 216)'").await?;

    // Another device moved on: saving here offers to go there instead of overwriting.
    let remote = reader
        .put(
            &format!("/api/v1/books/{}/state", voyage.file_id),
            &json!({"revision": state["state"]["revision"], "position": {"spine": 0, "fraction": 0}, "progress": 0.01, "device": "Phone"}),
        )
        .await?;
    assert_eq!(remote.status, 200);
    page.get_by_role("main", "Reading The Test Voyage").focus().await?;
    page.keyboard().press("ArrowRight").await?;
    let conflict = page.get_by_role("alertdialog", "Your place changed on another device");
    conflict.wait_for().await?;
    conflict.get_by_text(regex("on Phone")).wait_for().await?;
    page.screenshot(t.screenshot("books-reader-conflict-1440.png"), false).await?;
    conflict.get_by_role("button", "Go there").click().await?;
    page.wait_for_function("() => document.querySelector('iframe.epub-view')?.contentDocument?.title === 'Departure'").await?;
    page.get_by_role("button", "Back to library").click().await?;
    page.get_by_role("heading", "Continue reading").wait_for().await?;

    // Comic: pages turn right to left from the comic's own metadata.
    page.goto(&format!("{base}/books?book={}&read=1", comic.file_id)).await?;
    let comic_reader = page.get_by_role("main", regex("^Reading "));
    comic_reader.wait_for().await?;
    comic_reader.get_by_role("img", "Page 1 of 3").wait_for().await?;
    comic_reader.focus().await?;
    page.keyboard().press("ArrowLeft").await?;
    comic_reader.get_by_role("img", "Page 2 of 3").wait_for().await?;
    page.screenshot(t.screenshot("books-reader-comic-1440.png"), false).await?;

    // PDF: drawn by pdf.js, with the page tracked.
    page.goto(&format!("{base}/books?book={}&read=1", paper.file_id)).await?;
    let pdf_reader = page.get_by_role("main", "Reading A Fixture Paper");
    pdf_reader.wait_for().await?;
    page.locator(".pdf-view-page.is-drawn").first().wait_for().await?;
    pdf_reader.get_by_text("Page 1 of 2").wait_for().await?;
    page.screenshot(t.screenshot("books-reader-pdf-1440.png"), false).await?;

    // Files' PDF preview uses the same viewer.
    page.goto(&format!("{base}/?folder={root}")).await?;
    page.get_by_role("button", "Actions for paper.pdf").click().await?;
    page.get_by_role("menuitem", "Details").click().await?;
    page.get_by_role("button", "Preview").click().await?;
    page.get_by_role("region", "Preview of paper.pdf").locator(".pdf-view-page.is-drawn").first().wait_for().await?;
    page.screenshot(t.screenshot("books-files-pdf-preview-1440.png"), false).await?;

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
    mobile.get_by_role("navigation", "Applications").wait_for().await?;
    mobile.goto(&format!("{base}/books")).await?;
    if let Err(error) = mobile.get_by_role("heading", "Continue reading").wait_for().await {
        let _ = mobile.screenshot(t.screenshot("books-failure-390.png"), false).await;
        return Err(error);
    }
    assert_eq!(
        mobile.evaluate("() => document.documentElement.scrollWidth <= innerWidth + 1").await?,
        json!(true),
        "no sideways scrolling on phones"
    );
    mobile.screenshot(t.screenshot("books-home-390.png"), false).await?;
    mobile.get_by_role("button", regex("A Second Voyage")).first().click().await?;
    mobile.get_by_role("complementary", "Book details").get_by_role("heading", "A Second Voyage").wait_for().await?;
    mobile.screenshot(t.screenshot("books-details-390.png"), false).await?;
    mobile.get_by_role("button", "Read").click().await?;
    mobile.frame_locator("iframe.epub-view").get_by_role("heading", "Departure").wait_for().await?;
    mobile.screenshot(t.screenshot("books-reader-390.png"), false).await?;

    phone.close().await?;
    context.close().await?;

    let mut all = errors.lock().unwrap().clone();
    all.extend(phone_errors.lock().unwrap().iter().cloned());
    assert_eq!(all, Vec::<String>::new());
    Ok(())
}
