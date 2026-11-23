//! Port of `test/pim-browser.mjs`: Contacts and Calendar desktop/phone CRUD, deep links,
//! export/import, filters, recurring occurrence edit/drag/resize/delete/cancel restore, views,
//! collection paging, partial calendar failure recovery and request concurrency, and console check.

use dar_acceptance::browser::{Rect, RouteMatch};
use dar_acceptance::prelude::*;
use dar_acceptance::ui::Choice;
use regex::Regex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "pim-browser",
    cases: &[case!(contacts_and_calendar_journey, 180)],
};

/// Initiates native dragging on `source` before scrolling `target_child`'s parent into view.
async fn drag_to_parent(source: &Locator, target_child: &Locator) -> Result<()> {
    let mouse = source.page().mouse();
    source.scroll_into_view_if_needed().await?;
    let start = source.bounding_box().await?.context("drag source box")?;
    mouse.move_to(start.x + 12.0, start.y + 12.0, 1).await?;
    mouse.down().await?;
    let outcome: Result<()> = async {
        mouse.move_to(start.x + 24.0, start.y + 24.0, 4).await?;
        let box_val = target_child
            .evaluate(
                "el => { const p = el.parentElement; if (p.scrollIntoViewIfNeeded) p.scrollIntoViewIfNeeded(true); else p.scrollIntoView({block: 'center', inline: 'center'}); return globalThis.__darEngine.box(p); }",
                Value::Null,
            )
            .await?;
        let end: Rect = serde_json::from_value(box_val)?;
        mouse
            .move_to(end.x + end.width / 2.0, end.y + end.height / 2.0, 10)
            .await?;
        Ok(())
    }
    .await;
    let up = mouse.up().await;
    outcome?;
    up
}

async fn cancelled_events(page: &Page, value: bool) -> Result<()> {
    page.get_by_role("button", "Calendar settings")
        .click()
        .await?;
    let settings = page.get_by_role("dialog", "Calendar settings");
    let toggle = settings.get_by_role("switch", "Show cancelled events");
    if toggle.is_checked().await? != value {
        toggle.set_checked(value).await?;
        settings.get_by_text("Saved").wait_for().await?;
    }
    settings
        .get_by_role("button", "Close settings")
        .click()
        .await
}

async fn calendar_actions(page: &Page, name: &str) -> Result<()> {
    let action = page.get_by_role("button", format!("Actions for calendar {name}"));
    let previous = page.get_by_role("button", "Previous calendars");
    for _ in 0..100 {
        if previous.count().await? == 0 || !previous.is_enabled().await? {
            break;
        }
        previous.click().await?;
    }
    for _ in 0..100 {
        if action.is_visible().await? {
            action.click().await?;
            return Ok(());
        }
        let next = page.get_by_role("button", "Next calendars");
        if next.count().await? == 0 || !next.is_enabled().await? {
            break;
        }
        next.click().await?;
    }
    bail!("Calendar action unavailable: {name}")
}

pub async fn contacts_and_calendar_journey(t: &Ctx) -> Result<()> {
    let origin = t.fixture().await?.base.clone();
    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 950))
        .await?;
    let page = context.new_page().await?;

    let failures = Arc::new(Mutex::new(Vec::<String>::new()));
    let expected_failure = Arc::new(AtomicBool::new(false));
    {
        let sink = failures.clone();
        page.on_page_error(move |msg| sink.lock().unwrap().push(msg.clone()));
    }
    {
        let sink = failures.clone();
        let expected = expected_failure.clone();
        page.on_console(move |msg| {
            if msg.kind == "error"
                && !(msg.text.contains("401") && msg.text.contains("Unauthorized"))
                && !(expected.load(Ordering::Relaxed) && msg.text.contains("503"))
            {
                sink.lock().unwrap().push(msg.text.clone());
            }
        });
    }

    page.goto(&format!("{origin}/contacts")).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.get_by_role("heading", "All contacts")
        .wait_for()
        .await?;

    page.get_by_role("button", "New address book")
        .click()
        .await?;
    let book = format!("Browser people {}", &uuid()[..6]);
    let editor = page.locator("dialog,[data-editor-pane]");
    editor.get_by_label("Name").fill(&book).await?;
    editor.get_by_role("button", "Save").click().await?;
    page.get_by_role("heading", &book).wait_for().await?;
    page.get_by_role("button", &book).wait_for().await?;

    page.get_by_role("button", "New contact").click().await?;
    editor.get_by_label("First name").fill("Amira").await?;
    editor.get_by_label("Last name").fill("Browser").await?;
    editor.get_by_role("button", "Add email").click().await?;
    editor
        .get_by_label("Email 1")
        .fill("amira@example.com")
        .await?;
    editor
        .get_by_label("Groups (one per line)")
        .fill("Design team")
        .await?;
    editor
        .get_by_label("Notes")
        .fill("A real contact, saved in SpacetimeDB.")
        .await?;
    editor.get_by_role("button", "Save contact").click().await?;
    page.get_by_role("heading", "Amira Browser")
        .wait_for()
        .await?;
    assert_eq!(
        page.get_by_role("link", "amira@example.com")
            .get_attribute("href")
            .await?,
        Some("mailto:amira@example.com".to_owned())
    );

    let deep = page.url();
    page.reload().await?;
    page.get_by_role("heading", "Amira Browser")
        .wait_for()
        .await?;
    assert_eq!(page.url(), deep);

    page.get_by_role("button", "Edit contact").click().await?;
    editor
        .get_by_label("Job title")
        .fill("Product designer")
        .await?;
    editor.get_by_label("Favorite").check().await?;
    editor.get_by_role("button", "Save contact").click().await?;
    page.get_by_role("complementary", "Contact details")
        .get_by_text("Product designer")
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("contacts-desktop.png"), true)
        .await?;

    page.get_by_role("button", "Edit contact").click().await?;
    editor
        .get_by_role("button", "Remove Email 1")
        .click()
        .await?;
    editor.get_by_role("button", "Cancel").click().await?;
    editor
        .get_by_text("Discard unsaved changes?")
        .wait_for()
        .await?;
    editor.get_by_role("button", "Keep editing").click().await?;
    assert_eq!(editor.get_by_label("Email 1").count().await?, 0);
    editor.get_by_role("button", "Cancel").click().await?;
    editor
        .get_by_role("button", "Discard changes")
        .click()
        .await?;

    page.get_by_role("button", "Edit contact").click().await?;
    assert_eq!(
        editor.get_by_label("Email 1").input_value().await?,
        "amira@example.com"
    );
    editor.get_by_role("button", "Cancel").click().await?;
    page.wait_for_function(
        "() => document.activeElement?.getAttribute('aria-label') === 'Edit contact'",
    )
    .await?;

    let download = page
        .wait_for_download(page.get_by_role("link", "Export contact").click())
        .await?;
    assert!(String::from_utf8_lossy(&download.read()?).contains("FN:Amira Browser"));

    page.get_by_role("button", "Close contact details")
        .click()
        .await?;
    page.get_by_role("button", "Favorites").click().await?;
    page.get_by_role("button", regex("Amira Browser"))
        .wait_for()
        .await?;
    page.get_by_role("button", regex("Amira Browser"))
        .click()
        .await?;
    page.get_by_role("button", "Close contact details")
        .click()
        .await?;

    ui::open_search(&page)
        .await?
        .fill("no contact has this name")
        .await?;
    page.get_by_text("No matches across your apps.")
        .wait_for()
        .await?;
    page.get_by_role("button", "Clear search").click().await?;
    ui::close_search(&page).await?;

    // Switch to Calendar
    ui::switch_app(&page, "calendar").await?;
    page.get_by_role("heading", regex("2026")).wait_for().await?;
    page.get_by_role("button", "Add calendar").click().await?;
    page.get_by_role("menuitem", "New calendar").click().await?;
    let calendar = format!("Browser time {}", &uuid()[..6]);
    editor.get_by_label("Name").fill(&calendar).await?;
    editor.get_by_role("button", "Save").click().await?;
    page.get_by_role("button", &calendar).wait_for().await?;

    assert_eq!(
        page.get_by_role("radio", "Month")
            .get_attribute("aria-checked")
            .await?,
        Some("true".to_owned())
    );
    page.get_by_role("button", "Choose date").click().await?;
    page.get_by_role("button", "Go to today").click().await?;
    let today = page
        .get_by_role("button", "Choose date")
        .get_attribute("data-date")
        .await?
        .context("today data-date")?;
    assert_eq!(
        page.get_by_role("button", "Choose date")
            .text_content()
            .await?
            .trim(),
        "Today"
    );
    assert_eq!(
        page.locator(".sidebar")
            .get_by_role("button", "Today")
            .count()
            .await?,
        0
    );

    ui::calendar_view(&page, "day").await?;
    page.get_by_role("button", "Next period").click().await?;
    let tomorrow = (chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d")
        .map_err(|e| Error::new(e.to_string()))?
        + chrono::Duration::days(1))
    .format("%Y-%m-%d")
    .to_string();
    assert_eq!(
        page.get_by_role("button", "Choose date")
            .get_attribute("data-date")
            .await?,
        Some(tomorrow)
    );
    assert_ne!(
        page.get_by_role("button", "Choose date")
            .text_content()
            .await?
            .trim(),
        "Today"
    );
    page.get_by_role("button", "Choose date").click().await?;
    page.get_by_role("button", "Go to today").click().await?;
    assert_eq!(
        page.get_by_role("button", "Choose date")
            .get_attribute("data-date")
            .await?,
        Some(today)
    );
    ui::calendar_view(&page, "month").await?;

    page.evaluate(
        r##"async () => {
            const session = await (await fetch("/api/v1/auth/session")).json();
            for (let i = 0; i < 10; i++) {
                const response = await fetch("/api/v1/calendar/calendars", {
                    method: "POST",
                    headers: {"content-type": "application/json", "x-csrf-token": session.csrfToken},
                    body: JSON.stringify({name: "Sidebar page " + i, color: "#1e66f5"}),
                });
                if (!response.ok) throw Error("Sidebar fixture creation failed");
            }
        }"##,
    )
    .await?;
    page.reload().await?;
    page.locator(".calendar-month").wait_for().await?;
    page.get_by_role("button", "Next calendars").click().await?;
    assert!(
        page.get_by_role("button", "Previous calendars")
            .is_enabled()
            .await?
    );

    for (width, height) in [(1440, 950), (1000, 700), (390, 844)] {
        page.set_viewport_size(width, height).await?;
        page.locator(".calendar-month").wait_for().await?;
        assert_eq!(
            page.evaluate("() => document.documentElement.scrollHeight > innerHeight + 1")
                .await?,
            json!(false)
        );
        // Collection paging follows the resize; check the settled layout.
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            page.wait_for_function(
                "() => { const el = document.querySelector('.sidebar'); return el && el.scrollHeight <= el.clientHeight + 1; }",
            ),
        )
        .await;
        assert_eq!(
            page.locator(".sidebar")
                .evaluate("el => el.scrollHeight > el.clientHeight + 1", Value::Null)
                .await?,
            json!(false)
        );
        page.screenshot(t.screenshot(&format!("calendar-month-{width}.png")), true)
            .await?;
    }
    page.set_viewport_size(1440, 950).await?;

    page.get_by_role("button", "Add calendar").click().await?;
    page.get_by_role("menuitem", "New calendar").click().await?;
    let other_calendar = format!("{calendar} other");
    page.locator("[data-editor-pane]")
        .get_by_label("Name")
        .fill(&other_calendar)
        .await?;
    page.locator("[data-editor-pane]")
        .get_by_role("button", "Save")
        .click()
        .await?;
    page.get_by_role("button", &other_calendar)
        .wait_for()
        .await?;

    calendar_actions(&page, &calendar).await?;
    page.get_by_role("menuitem", "Edit calendar")
        .click()
        .await?;
    assert_eq!(
        page.locator("[data-editor-pane]")
            .get_by_label("Name")
            .input_value()
            .await?,
        calendar
    );
    page.get_by_role("button", "Close editor").click().await?;

    calendar_actions(&page, &calendar).await?;
    page.get_by_role("menuitem", "Delete calendar")
        .click()
        .await?;
    let deletion = page.get_by_role("dialog", "Delete calendar?");
    deletion
        .get_by_text(regex(&regex::escape(&calendar)))
        .wait_for()
        .await?;
    deletion.get_by_role("button", "Cancel").click().await?;

    page.get_by_role("button", "Calendar settings")
        .click()
        .await?;
    let settings = page.get_by_role("dialog", "Calendar settings");
    assert_eq!(settings.get_by_label("Display timezone").count().await?, 0);
    settings
        .get_by_role("tab", "Import and export")
        .click()
        .await?;
    ui::choose(
        &settings.get_by_role("combobox", "Calendar for import or export"),
        Choice::Label(&calendar),
    )
    .await?;
    let ics = page
        .wait_for_download(settings.get_by_role("link", "Export ICS").click())
        .await?;
    assert!(String::from_utf8_lossy(&ics.read()?).contains("BEGIN:VCALENDAR"));

    let imported_ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Dar UI proof//EN\r\nBEGIN:VEVENT\r\nUID:{}\r\nDTSTAMP:20261107T000000Z\r\nDTSTART:20261109T120000Z\r\nDTEND:20261109T123000Z\r\nSUMMARY:Settings import proof\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        uuid()
    );
    settings
        .get_by_label("ICS file")
        .set_input_files(vec![FilePayload::new(
            "settings.ics",
            "text/calendar",
            imported_ics,
        )])
        .await?;
    settings
        .get_by_role("button", "Import ICS")
        .click()
        .await?;
    settings
        .get_by_text("1 imported; 0 failed.")
        .wait_for()
        .await?;
    let exported_import = page
        .wait_for_download(settings.get_by_role("link", "Export ICS").click())
        .await?;
    assert!(String::from_utf8_lossy(&exported_import.read()?).contains("Settings import proof"));
    settings
        .get_by_role("button", "Close settings")
        .click()
        .await?;

    ui::calendar_view(&page, "week").await?;
    ui::calendar_date(&page, "2026-11-09").await?;
    page.get_by_role("button", "New event").click().await?;
    let event_title = format!("Browser design review {}", &uuid()[..6]);
    editor.get_by_label("Title").fill(&event_title).await?;
    editor.get_by_label("Starts").fill("2026-11-09").await?;
    editor.get_by_label("Start time").fill("09:00").await?;
    editor.get_by_label("Ends").fill("2026-11-09").await?;
    editor.get_by_label("End time").fill("10:00").await?;
    editor
        .get_by_label("Timezone")
        .fill("America/Toronto")
        .await?;
    ui::choose(&editor.get_by_label("Repeats"), Choice::Value("weekly")).await?;
    assert_eq!(editor.get_by_label("Recurrence rule").count().await?, 0);
    ui::choose(&editor.get_by_label("Repeats"), Choice::Value("custom")).await?;
    editor
        .get_by_label("Recurrence rule")
        .fill("FREQ=WEEKLY;COUNT=3")
        .await?;
    editor.get_by_role("button", "Save event").click().await?;

    let event_pat = regex::escape(&event_title);
    let moved_title = format!("{event_title} moved");
    let moved_pat = regex::escape(&moved_title);

    page.get_by_role("button", regex(&event_pat))
        .first()
        .click()
        .await?;
    page.get_by_role("heading", &event_title).wait_for().await?;
    page.get_by_role("button", "Edit event").click().await?;
    page.get_by_role("button", "This event only")
        .click()
        .await?;
    editor.get_by_label("Title").fill(&moved_title).await?;
    editor.get_by_role("button", "Save event").click().await?;
    page.get_by_role("button", regex(&moved_pat))
        .first()
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("calendar-desktop.png"), true)
        .await?;

    drag_to_parent(
        &page.get_by_role("button", regex(&moved_pat)).first(),
        &page.get_by_role("button", "New event 2026-11-09 15:00"),
    )
    .await?;
    editor.get_by_label("Starts").wait_for().await?;
    assert_eq!(
        editor.get_by_label("Starts").input_value().await?,
        "2026-11-09"
    );
    assert_eq!(
        editor.get_by_label("Start time").input_value().await?,
        "15:00"
    );
    editor.get_by_role("button", "Save event").click().await?;
    editor.wait_for_state(State::Hidden).await?;
    page.get_by_role("button", regex(&format!("15:00.*{moved_pat}")))
        .wait_for()
        .await?;

    page.get_by_label("Calendar time grid")
        .evaluate("element => { element.scrollTop = 12 * 64; }", Value::Null)
        .await?;
    drag_to_parent(
        &page.get_by_role("button", format!("Resize {moved_title}")),
        &page.get_by_role("button", "New event 2026-11-09 18:00"),
    )
    .await?;
    editor.get_by_label("Ends").wait_for().await?;
    assert_eq!(
        editor.get_by_label("Ends").input_value().await?,
        "2026-11-09"
    );
    assert_eq!(
        editor.get_by_label("End time").input_value().await?,
        "18:00"
    );
    editor.get_by_role("button", "Save event").click().await?;
    editor.wait_for_state(State::Hidden).await?;

    for view in ["day", "month", "agenda", "week"] {
        ui::calendar_view(&page, view).await?;
        if view == "month" {
            page.locator(".calendar-month").wait_for().await?;
            assert_eq!(
                page.evaluate("() => document.documentElement.scrollHeight > innerHeight + 1")
                    .await?,
                json!(false)
            );
            // Events fill the month grid after it renders; wait briefly before looking for the overflow link.
            let _ = page
                .get_by_role("button", regex(&moved_pat))
                .first()
                .timeout(Duration::from_secs(5))
                .wait_for()
                .await;
            if !page
                .get_by_role("button", regex(&moved_pat))
                .first()
                .is_visible()
                .await?
            {
                page.get_by_role("button", "Show all events for 2026-11-09")
                    .click()
                    .await?;
            }
        }
        page.get_by_role("button", regex(&moved_pat))
            .first()
            .wait_for()
            .await?;
    }

    ui::calendar_view(&page, "agenda").await?;
    page.get_by_role("button", regex(&moved_pat))
        .first()
        .click()
        .await?;
    page.get_by_role("button", "Delete event").click().await?;
    page.get_by_role("button", "This event only")
        .click()
        .await?;
    page.get_by_role("button", regex(&moved_pat))
        .wait_for_state(State::Hidden)
        .await?;

    cancelled_events(&page, true).await?;
    page.get_by_role("button", regex(&moved_pat))
        .first()
        .click()
        .await?;
    page.get_by_role("button", "Edit event").click().await?;
    page.get_by_role("button", "This event only")
        .click()
        .await?;
    ui::choose(&editor.get_by_label("Status"), Choice::Value("CONFIRMED")).await?;
    editor.get_by_role("button", "Save event").click().await?;
    editor.wait_for_state(State::Hidden).await?;
    cancelled_events(&page, false).await?;
    page.get_by_role("button", regex(&moved_pat))
        .first()
        .wait_for()
        .await?;

    // Inclusive all-day range: intercept the POST response to verify the saved exclusive end date.
    page.get_by_role("button", "New event").click().await?;
    let all_day_editor = page.locator("[data-editor-pane]");
    all_day_editor
        .get_by_label("Title")
        .fill("Inclusive all-day range")
        .await?;
    all_day_editor.get_by_label("All day").check().await?;
    all_day_editor
        .get_by_label("Starts")
        .fill("2026-11-06")
        .await?;
    all_day_editor
        .get_by_label("Ends")
        .fill("2026-11-07")
        .await?;

    let (saved_tx, saved_rx) = tokio::sync::oneshot::channel::<Value>();
    let saved_tx = Arc::new(Mutex::new(Some(saved_tx)));
    let items_re = Regex::new(r"/calendar/calendars/[^/]+/items$").unwrap();
    let route_items_re = items_re.clone();
    page.route(
        RouteMatch::predicate(move |u| {
            url::Url::parse(u).is_ok_and(|parsed| route_items_re.is_match(parsed.path()))
        }),
        None,
        move |route| {
            let saved_tx = saved_tx.clone();
            async move {
                if route.request().method != "POST" {
                    return route.continue_().await;
                }
                let response = route.fetch().await?;
                if let Ok(json_body) = response.json()
                    && let Some(tx) = saved_tx.lock().unwrap().take()
                {
                    let _ = tx.send(json_body);
                }
                let headers: Vec<(String, String)> = response
                    .headers
                    .iter()
                    .filter(|(k, _)| {
                        !matches!(
                            k.to_ascii_lowercase().as_str(),
                            "content-length"
                                | "content-type"
                                | "content-encoding"
                                | "transfer-encoding"
                        )
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                route
                    .fulfill(Fulfill {
                        status: response.status,
                        content_type: response.header("content-type").map(str::to_owned),
                        headers,
                        body: response.body,
                    })
                    .await
            }
        },
    )
    .await?;
    all_day_editor
        .get_by_role("button", "Save event")
        .click()
        .await?;
    let saved_range = tokio::time::timeout(Duration::from_secs(30), saved_rx)
        .await
        .context("all-day event POST response")?
        .map_err(|_| Error::new("all-day event channel closed"))?;
    page.unroute_all().await?;
    assert_eq!(saved_range["data"]["start"], "2026-11-06");
    assert_eq!(saved_range["data"]["end"], "2026-11-08");
    all_day_editor.wait_for_state(State::Hidden).await?;

    // Partial failure recovery and bounded concurrency for occurrence fetches.
    let api = page.request();
    let native_session = api
        .get(&format!("{origin}/api/v1/auth/session"))
        .await?
        .json()?;
    let csrf = native_session["csrfToken"]
        .as_str()
        .context("csrfToken")?
        .to_owned();
    let api_headers = [("origin", origin.as_str()), ("x-csrf-token", csrf.as_str())];
    let preferences = api
        .get(&format!("{origin}/api/v1/pim/preferences"))
        .await?
        .json()?;
    let tz = preferences["timezone"]
        .as_str()
        .unwrap_or("UTC")
        .to_owned();

    let good = api
        .post(
            &format!("{origin}/api/v1/calendar/calendars"),
            &api_headers,
            json!({"name": "Healthy calendar", "color": "#2563eb"}),
        )
        .await?
        .json()?;
    let bad = api
        .post(
            &format!("{origin}/api/v1/calendar/calendars"),
            &api_headers,
            json!({"name": "Retry calendar", "color": "#2563eb"}),
        )
        .await?
        .json()?;
    let good_id = good["id"].as_str().context("good id")?.to_owned();
    let bad_id = bad["id"].as_str().context("bad id")?.to_owned();

    for (cal_id, title) in [
        (&good_id, "Midnight boundary"),
        (&bad_id, "Recovered calendar event"),
    ] {
        let response = api
            .post(
                &format!("{origin}/api/v1/calendar/calendars/{cal_id}/items"),
                &api_headers,
                json!({"data": {"title": title, "start": "2026-11-07T23:00", "end": "2026-11-08T00:00", "timezone": tz, "allDay": false, "recurrence": "", "attendees": [], "reminderMinutes": null}}),
            )
            .await?;
        assert_eq!(response.status(), 201);
    }

    page.goto(&format!("{origin}/calendar?date=2026-11-07&view=day"))
        .await?;
    page.locator(".calendar-event")
        .filter_has_text("Recovered calendar event")
        .wait_for()
        .await?;

    let occurrence_re = Regex::new(r"/api/v1/calendar/calendars/[^/]+/occurrences\?").unwrap();
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_requests = Arc::new(AtomicUsize::new(0));

    let started_id = {
        let re = occurrence_re.clone();
        let in_flight = in_flight.clone();
        let max_requests = max_requests.clone();
        page.on_request(move |req| {
            if re.is_match(&req.url) {
                let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_requests.fetch_max(current, Ordering::SeqCst);
            }
        })
    };
    let finished_id = {
        let re = occurrence_re.clone();
        let in_flight = in_flight.clone();
        page.on_request_finished(move |req| {
            if re.is_match(&req.url) {
                let _ = in_flight.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
                    Some(v.saturating_sub(1))
                });
            }
        })
    };
    let failed_id = {
        let re = occurrence_re.clone();
        let in_flight = in_flight.clone();
        page.on_request_failed(move |req| {
            if re.is_match(&req.url) {
                let _ = in_flight.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
                    Some(v.saturating_sub(1))
                });
            }
        })
    };

    expected_failure.store(true, Ordering::Relaxed);
    let route_re = occurrence_re.clone();
    let bad_needle = format!("/{bad_id}/");
    page.route(
        RouteMatch::predicate(move |u| route_re.is_match(u)),
        None,
        move |route| {
            let bad_needle = bad_needle.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(30)).await;
                if route.failure().is_some() {
                    return Ok(());
                }
                let result = if route.request().url.contains(&bad_needle) {
                    route
                        .fulfill(Fulfill::json(
                            503,
                            &json!({"error": {"message": "Temporary calendar failure"}}),
                        ))
                        .await
                } else {
                    route.continue_().await
                };
                if let Err(err) = result
                    && route.failure().is_none()
                {
                    return Err(err);
                }
                Ok(())
            }
        },
    )
    .await?;

    ui::calendar_view(&page, "agenda").await?;
    page.get_by_text(regex("Could not refresh Retry calendar"))
        .wait_for()
        .await?;
    assert_eq!(
        page.locator(".calendar-event")
            .filter_has_text("Midnight boundary")
            .count()
            .await?,
        1
    );
    let observed_max = max_requests.load(Ordering::SeqCst);
    assert!(
        observed_max > 1 && observed_max <= 4,
        "Expected 2–4 active calendar requests, observed {observed_max}"
    );

    page.unroute_all().await?;
    page.off(started_id);
    page.off(finished_id);
    page.off(failed_id);
    expected_failure.store(false, Ordering::Relaxed);

    page.get_by_role("button", "Refresh").click().await?;
    page.locator(".calendar-event")
        .filter_has_text("Recovered calendar event")
        .wait_for()
        .await?;
    assert_eq!(
        page.locator(".calendar-event")
            .filter_has_text("Midnight boundary")
            .count()
            .await?,
        1
    );

    // Phone viewport checks in a second page sharing the context session.
    let phone = context.new_page().await?;
    phone.set_viewport_size(390, 844).await?;
    {
        let sink = failures.clone();
        phone.on_page_error(move |msg| sink.lock().unwrap().push(msg.clone()));
    }
    phone.goto(&format!("{origin}/contacts")).await?;
    phone
        .get_by_role("button", regex("Amira Browser"))
        .click()
        .await?;
    phone
        .get_by_role("heading", "Amira Browser")
        .wait_for()
        .await?;
    assert_eq!(
        phone
            .evaluate("() => document.documentElement.scrollWidth <= innerWidth")
            .await?,
        json!(true)
    );
    phone
        .screenshot(t.screenshot("contacts-mobile.png"), true)
        .await?;

    phone
        .goto(&format!("{origin}/calendar?date=2026-11-16&view=week"))
        .await?;
    phone
        .get_by_role("button", regex(&event_pat))
        .first()
        .wait_for()
        .await?;
    assert_eq!(
        phone
            .evaluate("() => document.documentElement.scrollWidth <= innerWidth")
            .await?,
        json!(true)
    );
    phone
        .screenshot(t.screenshot("calendar-mobile.png"), true)
        .await?;
    phone
        .get_by_role("button", regex(&event_pat))
        .first()
        .click()
        .await?;
    phone
        .get_by_role("heading", &event_title)
        .wait_for()
        .await?;
    assert_eq!(
        phone
            .evaluate("() => document.documentElement.scrollWidth <= innerWidth")
            .await?,
        json!(true)
    );

    let all_failures = failures.lock().unwrap().clone();
    context.close().await?;
    assert_eq!(all_failures, Vec::<String>::new());
    Ok(())
}
