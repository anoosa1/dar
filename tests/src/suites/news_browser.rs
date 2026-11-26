//! Port of `test/news-browser.mjs`: views, settings sheet, sanitized reader with proxied images,
//! keyboard star/navigation, podcast enclosure, subscribe by discovery, confirmed unsubscribe,
//! folders, phone reader and both themes.

use crate::suites::comms_support::FeedServer;
use dar_acceptance::prelude::*;

pub const SUITE: Suite = Suite {
    name: "news-browser",
    cases: &[case!(news_browser_journey, 180)],
};

pub async fn news_browser_journey(t: &Ctx) -> Result<()> {
    let root = t.temp("news-browser")?;
    let fixture = FeedServer::start().await?;
    fixture.state.lock().unwrap().extra = true;
    let site = fixture.site.clone();
    let config = InstanceConfig {
        news_allow_private_networks: true,
        ..InstanceConfig::default()
    };
    let runtime = Instance::isolated(&root, "dar-news-ui", config, true).await?;
    let base = runtime.base.clone();

    // Seed the state that news-browser.mjs expects after the integration test:
    // - Fixture News feed subscribed with all 3 items marked read (so Unread is "All caught up")
    // - Blogs folder containing a failing feed "Elsewhere"
    // - Preference newsMarkRead = "scroll"
    session::setup_admin(&base).await?;
    let session = login(&base).await?;
    session
        .post(
            "/api/v1/news/feeds",
            &json!({ "url": format!("{site}/feed.xml"), "folderId": 0 }),
        )
        .await?
        .expect(201)?;
    let overview = session.get_json("/api/v1/news/overview").await?;
    session
        .post(
            "/api/v1/news/read",
            &json!({ "type": "all", "newestItemId": overview["newestItemId"] }),
        )
        .await?
        .expect(200)?;
    let opml = format!(
        r#"<opml version="2.0"><body><outline text="Blogs"><outline text="Elsewhere" xmlUrl="{site}/elsewhere.xml"/></outline></body></opml>"#
    );
    session
        .post("/api/v1/news/opml", &json!({ "text": opml }))
        .await?
        .expect(200)?;
    let overview = session.get_json("/api/v1/news/overview").await?;
    if let Some(elsewhere) = overview["feeds"]
        .as_array()
        .and_then(|arr| arr.iter().find(|f| f["title"] == json!("Elsewhere")))
        && elsewhere["updateErrorCount"].as_i64().unwrap_or(0) == 0
    {
        let id = elsewhere["id"].as_i64().unwrap();
        session
            .patch(
                &format!("/api/v1/news/feeds/{id}"),
                &json!({ "updateNow": true }),
            )
            .await?
            .expect(200)?;
    }
    let prefs = session.get_json("/api/v1/pim/preferences").await?;
    let rev = prefs["revision"].as_i64().context("prefs revision")?;
    Reply::from(
        session
            .request("PUT", "/api/v1/pim/preferences")
            .header("if-match", format!("\"preferences:{rev}\""))
            .json(&json!({ "newsRefresh": 60, "newsMarkRead": "scroll" }))
            .send()
            .await?,
    )
    .await?
    .expect(200)?;

    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 950))
        .await?;
    let page = context.new_page().await?;
    let errors = page.collect_errors(|text| text.contains("Failed to load resource"));

    page.goto(&format!("{base}/news")).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.locator("header.topbar").wait_for().await?;
    page.get_by_role("heading", "Unread").wait_for().await?;
    page.get_by_role("heading", "All caught up")
        .wait_for()
        .await?;

    // Settings sheet: the API set "On scroll"; switch back and see it saved.
    page.get_by_role("button", "News settings")
        .first()
        .click()
        .await?;
    let sheet = page.get_by_role("dialog", contains("News settings"));
    assert!(
        sheet
            .get_by_role("radio", contains("On scroll"))
            .is_checked()
            .await?
    );
    sheet
        .get_by_role("radio", contains("On open"))
        .check()
        .await?;
    sheet
        .get_by_text(contains("Saved"))
        .first()
        .wait_for()
        .await?;
    sheet
        .get_by_role("tab", contains("Keyboard shortcuts"))
        .click()
        .await?;
    sheet
        .get_by_text(contains("Next unread article"))
        .wait_for()
        .await?;
    sheet
        .get_by_role("button", contains("Close settings"))
        .click()
        .await?;
    sheet.wait_for_state(State::Detached).await?;

    // All articles, grouped by day; opening one shows the sanitized body with proxied images.
    page.get_by_role("button", contains("All articles"))
        .click()
        .await?;
    let list = page.get_by_role("list", contains("All articles articles"));
    list.get_by_role("button", regex("Welcome to the fixture"))
        .click()
        .await?;
    let reader = page.get_by_role("complementary", contains("Article"));
    reader
        .get_by_role("heading", contains("Welcome to the fixture"))
        .wait_for()
        .await?;
    reader
        .get_by_text(contains("Hello reader"))
        .wait_for()
        .await?;
    let img_src = reader
        .locator(".news-body img")
        .get_attribute("src")
        .await?
        .unwrap_or_default();
    assert!(
        img_src.starts_with("/api/v1/news/image?url="),
        "unexpected img src: {img_src}"
    );
    assert_eq!(reader.locator(".news-body script").count().await?, 0);

    let star = reader.get_by_role("button", "Star");
    let was_starred = star.get_attribute("aria-pressed").await?;
    page.locator("body").press("s").await?;
    page.wait_for_function_with(
        "([before]) => document.querySelector('.news-reader-tools [aria-label=Star]')?.getAttribute('aria-pressed') !== before",
        json!([was_starred]),
    )
    .await?;
    page.locator("body").press("k").await?;
    reader
        .get_by_role("heading", contains("Episode one"))
        .wait_for()
        .await?;
    reader
        .locator("audio[src='https://media.example/ep1.mp3']")
        .wait_for_state(State::Attached)
        .await?;
    reader.locator(".news-player").wait_for().await?;

    // Aa reading typography popover: switch to Sepia + Clean Sans and verify classes.
    reader
        .get_by_role("button", "Reading typography")
        .click()
        .await?;
    let aa = reader.locator(".news-aa-popover");
    aa.wait_for().await?;
    aa.get_by_role("button", "Sepia").click().await?;
    aa.get_by_role("button", "Clean Sans").click().await?;
    page.locator(".news-reader.news-paper-sepia.news-font-sans")
        .wait_for()
        .await?;
    // Reset to Dar + Editorial Serif and close popover.
    aa.get_by_role("button", "Dar").click().await?;
    aa.get_by_role("button", "Editorial Serif").click().await?;
    reader
        .get_by_role("button", "Reading typography")
        .click()
        .await?;

    // Focus reading mode toggles via 'f'.
    page.locator("body").press("f").await?;
    page.locator(".news-shell.is-news-focus").wait_for().await?;
    page.locator("body").press("f").await?;
    page.locator(".news-shell.is-news-focus")
        .wait_for_state(State::Detached)
        .await?;

    // Filter chips: Media filters list to Episode one, then All restores it.
    page.locator(".news-filter-bar")
        .get_by_role("button", "Media")
        .click()
        .await?;
    list.get_by_role("button", regex("Episode one"))
        .wait_for()
        .await?;
    page.locator(".news-filter-bar")
        .get_by_role("button", "All")
        .click()
        .await?;

    // Magazine Cards layout switch renders cards grid, then return to Split reading.
    page.get_by_role("button", "Magazine cards").click().await?;
    page.locator(".news-list.is-cards").wait_for().await?;
    page.get_by_role("button", "Split reading").click().await?;
    page.locator(".news-list.is-cards")
        .wait_for_state(State::Detached)
        .await?;

    // Single-feed view shows the 14-day Feed Pulse sparkline card.
    page.get_by_role("button", regex("^Fixture News"))
        .first()
        .click()
        .await?;
    page.locator(".news-feed-pulse svg").wait_for().await?;

    // Subscribe through discovery, then unsubscribe through the feed menu with confirmation.
    page.get_by_role("button", "Subscribe")
        .first()
        .click()
        .await?;
    let pane = page.locator("[data-editor-pane]");
    pane.get_by_label(contains("Site or feed address"))
        .fill(&format!("{site}/atom.xml"))
        .await?;
    pane.get_by_role("button", contains("Find feeds"))
        .first()
        .click()
        .await?;
    pane.get_by_text(contains("Found 1 feed"))
        .wait_for()
        .await?;
    pane.get_by_text(contains("Long read")).wait_for().await?;
    pane.get_by_role("button", "Subscribe").click().await?;
    page.get_by_text(contains("Subscribed to Atom Blog"))
        .wait_for()
        .await?;
    page.get_by_role("heading", "Atom Blog").wait_for().await?;
    page.get_by_role("list", contains("Atom Blog articles"))
        .get_by_role("button", regex("Long read"))
        .wait_for()
        .await?;
    page.get_by_role("button", contains("Actions for Atom Blog"))
        .first()
        .click()
        .await?;
    page.get_by_role("menuitem", contains("Unsubscribe…"))
        .click()
        .await?;
    let confirm = page.get_by_role("dialog", contains("Unsubscribe from Atom Blog?"));
    confirm
        .get_by_role("button", contains("Unsubscribe"))
        .click()
        .await?;
    confirm.wait_for_state(State::Detached).await?;
    page.get_by_role("heading", "Unread").wait_for().await?;
    assert_eq!(
        page.get_by_role("button", contains("Actions for Atom Blog"))
            .count()
            .await?,
        0
    );

    // Folder with a failing imported feed shows its banner.
    page.get_by_role("button", regex("^Blogs"))
        .first()
        .click()
        .await?;
    page.get_by_role("heading", "Blogs").wait_for().await?;
    page.get_by_text(regex(r"Elsewhere: last \d+ updates? failed"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("news-desktop.png"), false)
        .await?;

    // Phones: chips, the list, a full-screen article with Back.
    for theme in ["latte", "mocha"] {
        page.evaluate_with(
            "(value) => { document.documentElement.dataset.theme = value; }",
            json!(theme),
        )
        .await?;
        for width in [1440u32, 390u32] {
            page.set_viewport_size(width, 844).await?;
            if width == 390 {
                page.get_by_role("button", "Open navigation").click().await?;
            }
            page.get_by_role("button", contains("All articles"))
                .click()
                .await?;
            page.wait_for_timeout(300).await;
            let overflow = page
                .evaluate(
                    "() => document.documentElement.scrollWidth > innerWidth ? [...document.querySelectorAll('body *')].filter(e => e.getBoundingClientRect().right > innerWidth + 1 && !e.closest('.sidebar')).slice(0, 8).map(e => e.tagName + '.' + [...e.classList].join('.') + ' ' + Math.round(e.getBoundingClientRect().right)) : []",
                )
                .await?;
            assert_eq!(overflow, json!([]), "{theme} {width} overflow");
            if width == 390 {
                page.get_by_role("list", contains("All articles articles"))
                    .get_by_role("button", regex("Fresh arrival"))
                    .click()
                    .await?;
                let back = page.get_by_role("button", contains("Back to articles"));
                back.wait_for().await?;
                page.get_by_role("heading", contains("Fresh arrival"))
                    .wait_for()
                    .await?;
                assert!(!page.locator(".news-main").is_visible().await?);
                page.screenshot(t.screenshot(&format!("news-phone-{theme}.png")), false)
                    .await?;
                back.click().await?;
                page.locator(".news-main").wait_for().await?;
            }
        }
    }

    assert_eq!(*errors.lock().unwrap(), Vec::<String>::new());
    context.close().await?;
    runtime.close().await;
    fixture.close();
    Ok(())
}
