//! Cross-app refinement browser journey (`refinement-browser.mjs`): themed Calendar date
//! popover/keyboard navigation, Ctrl+K spotlight search across Notes/Files/shared files/Calendar,
//! compact Notes editor & theme/viewport overflow checks, search pointer hit-testing above an
//! open editor, and Prayer Times settings save-on-close behavior.

use dar_acceptance::browser::{Browser, ClickOptions, ContextOptions, State, regex};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use dar_acceptance::ui::{calendar_date, close_search, open_search, switch_app};
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "refinement-browser",
    cases: &[case!(refinement_journey, 180)],
};

pub async fn refinement_journey(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let marker = format!("Refinement {}", &uuid()[..6]);

    let notes_root = admin.get_json("/api/v1/notes/library").await?["rootId"]
        .as_str()
        .context("notes rootId")?
        .to_owned();
    upload_with(
        &admin,
        &format!("{marker} note.md"),
        "# A quieter week\n\nKeep room for the important things.\n\n- Plant herbs\n- Finish the chapter\n- Take a long walk\n",
        Upload { parent_id: &notes_root, ..Upload::default() },
    )
    .await?;
    upload(&admin, &format!("{marker} file.txt"), "Global file result").await?;

    let calendar = admin
        .post(
            "/api/v1/calendar/calendars",
            &json!({ "name": format!("{marker} calendar"), "color": "#1e66f5" }),
        )
        .await?
        .expect(201)?
        .json()?;
    let calendar_id = calendar["id"].as_str().context("calendar id")?;
    admin
        .post(
            &format!("/api/v1/calendar/calendars/{calendar_id}/items"),
            &json!({ "data": { "title": format!("{marker} meeting"), "start": "2026-11-08T10:00", "end": "2026-11-08T11:00", "timezone": "UTC", "allDay": false } }),
        )
        .await?
        .expect(201)?;

    let prayer_path = "/api/v1/apps/org.dar.prayer-times/userData/settings";
    if admin.get_json(prayer_path).await?.is_null() {
        let put = Reply::from(
            admin
                .request("PUT", prayer_path)
                .header("if-match", "\"0\"")
                .json(&json!({
                    "schemaVersion": 1,
                    "value": {
                        "schemaVersion": 1,
                        "location": { "label": "Toronto", "latitude": 43.6532, "longitude": -79.3832, "timezone": "America/Toronto" },
                        "calculation": {
                            "method": "ISNA",
                            "asrRule": "standard",
                            "highLatitudeRule": "angle",
                            "offsetMinutes": { "fajr": 0, "sunrise": 0, "dhuhr": 0, "asr": 0, "maghrib": 0, "isha": 0 }
                        }
                    }
                }))
                .send()
                .await?,
        )
        .await?;
        assert_eq!(put.status, 200, "{}", put.text());
    }

    let owner_name = format!("refinement-owner-{}", &uuid()[..6]);
    admin
        .post(
            "/api/v1/auth/users",
            &json!({ "username": owner_name, "displayName": "Shared search owner", "password": PASSWORD }),
        )
        .await?
        .expect(201)?;
    let other = login_as(&base, &owner_name, PASSWORD).await?;
    let owner = other.connect().await?;
    let folder = uuid();
    let grant = uuid();
    reducer(|cb| {
        owner.reducers.create_folder_then(
            folder.clone(),
            String::new(),
            "Shared search folder".into(),
            cb,
        )
    })
    .await?;
    upload_with(
        &other,
        &format!("{marker} shared descendant.txt"),
        "Shared result",
        Upload {
            parent_id: &folder,
            ..Upload::default()
        },
    )
    .await?;
    let details = procedure(|cb| owner.procedures.file_details_then(folder.clone(), cb)).await?;
    let revision = details["entry"]["revision"]
        .as_u64()
        .context("folder revision")? as u32;
    reducer(|cb| {
        owner.reducers.grant_access_then(
            grant.clone(),
            folder.clone(),
            "admin".into(),
            "read".into(),
            revision,
            cb,
        )
    })
    .await?;

    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 950))
        .await?;
    let page = context.new_page().await?;
    let errors = page.collect_errors(|text| text.contains("401") && text.contains("Unauthorized"));

    let result = async {
        page.goto(&format!("{base}/calendar")).await?;
        page.get_by_label("Username").fill("admin").await?;
        page.get_by_label("Password").fill(PASSWORD).await?;
        page.get_by_role("button", "Sign in").click().await?;
        page.get_by_role("button", "Choose date").wait_for().await?;
        assert_eq!(page.locator(".calendar-jump").count().await?, 0);

        calendar_date(&page, "2026-12-15").await?;
        assert_eq!(page.get_by_role("button", "Choose date").get_attribute("data-date").await?.as_deref(), Some("2026-12-15"));
        page.get_by_role("button", "Choose date").click().await?;
        let picker = page.get_by_role("dialog", "Choose calendar date");
        picker.wait_for().await?;
        page.screenshot(t.screenshot("calendar-themed-date-picker.png"), true).await?;
        page.keyboard().press("ArrowRight").await?;
        page.keyboard().press("Enter").await?;
        assert_eq!(page.get_by_role("button", "Choose date").get_attribute("data-date").await?.as_deref(), Some("2026-12-16"));
        assert_eq!(page.get_by_role("button", "Choose date").evaluate("el => el === document.activeElement", Value::Null).await?, json!(true));

        page.get_by_role("button", "Choose date").click().await?;
        page.keyboard().press("Escape").await?;
        picker.wait_for_state(State::Hidden).await?;

        page.keyboard().press("Control+k").await?;
        let search = page.get_by_role("combobox", "Search all apps");
        assert_eq!(search.evaluate("el => el === document.activeElement", Value::Null).await?, json!(true));
        search.fill(&marker).await?;
        let results = page.get_by_role("listbox", "Search results");
        results.get_by_role("option", regex(&format!("{marker} note"))).wait_for().await?;
        results.get_by_role("option", regex(&format!("{marker} file"))).wait_for().await?;
        page.screenshot(t.screenshot("global-search-results.png"), true).await?;
        results.get_by_role("option", regex(&format!("{marker} note"))).click().await?;

        let note_editor = page.get_by_role("region", format!("Editor for {marker} note.md"));
        note_editor.wait_for().await?;
        let note_source = ui::note_source(&note_editor).await?;
        page.wait_for_function("() => /A quieter week/.test(document.querySelector('.cm-content')?.darEditor?.text() ?? '')").await?;
        assert!(ui::note_text(&note_source).await?.contains("A quieter week"));
        assert_eq!(page.get_by_label("Filter by tag").count().await?, 0);
        note_editor.get_by_label("File contents").press("End").await?;
        note_editor.get_by_label("File contents").press_sequentially(" #weekly").await?;
        note_editor.get_by_text("All changes saved").wait_for().await?;

        for theme in ["latte", "mocha"] {
            let mut prefs = page.request().get(&format!("{base}/api/v1/pim/preferences")).await?.json()?;
            let rev = prefs["revision"].as_u64().context("prefs revision")?;
            let csrf = page.request().get(&format!("{base}/api/v1/auth/session")).await?.json()?["csrfToken"]
                .as_str()
                .context("csrfToken")?
                .to_owned();
            prefs["appearance"] = json!(theme);
            let put = page
                .request()
                .put(
                    &format!("{base}/api/v1/pim/preferences"),
                    &[("origin", base.as_str()), ("x-csrf-token", csrf.as_str()), ("if-match", &format!("\"preferences:{rev}\""))],
                    prefs,
                )
                .await?;
            assert_eq!(put.status, 200, "{}", put.text());
            page.evaluate("() => window.dispatchEvent(new Event('dar:preferences'))").await?;
            page.wait_for_function_with("(t) => document.documentElement.dataset.theme === t", json!(theme)).await?;
            for (width, height) in [(1440u32, 950u32), (900, 700), (390, 568)] {
                page.set_viewport_size(width, height).await?;
                assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
                assert_eq!(page.evaluate("() => document.documentElement.scrollHeight > innerHeight").await?, json!(false));
                page.screenshot(t.screenshot(&format!("notes-refined-{theme}-{width}.png")), true).await?;
            }
        }
        page.set_viewport_size(1440, 950).await?;

        open_search(&page).await?.fill(&marker).await?;
        page.get_by_role("listbox", "Search results").get_by_role("option", regex(&format!("{marker} file"))).click().await?;
        page.get_by_role("complementary", "File details").get_by_role("heading", format!("{marker} file.txt")).wait_for().await?;

        open_search(&page).await?.fill(&marker).await?;
        page.get_by_role("listbox", "Search results").get_by_role("option", regex(&format!("{marker} shared descendant"))).click().await?;
        page.get_by_role("complementary", "File details")
            .get_by_role("heading", format!("{marker} shared descendant.txt"))
            .wait_for()
            .await?;
        reducer(|cb| owner.reducers.revoke_grant_then(grant.clone(), cb)).await?;
        page.get_by_role("complementary", "File details").wait_for_state(State::Hidden).await?;

        open_search(&page).await?.fill(&marker).await?;
        page.get_by_role("listbox", "Search results").get_by_role("option", regex(&format!("{marker} meeting"))).click().await?;
        page.get_by_role("complementary", "Event details").get_by_role("heading", format!("{marker} meeting")).wait_for().await?;

        // Search results must receive pointer events above an open Files editor.
        switch_app(&page, "files").await?;
        page.get_by_role("button", format!("{marker} file.txt")).click().await?;
        let editor = page.get_by_role("region", format!("Editor for {marker} file.txt"));
        editor.wait_for().await?;
        for (width, height) in [(1440u32, 950u32), (390, 568)] {
            page.set_viewport_size(width, height).await?;
            open_search(&page).await?;
            search.fill(&marker).await?;
            let option = page.get_by_role("listbox", "Search results").get_by_role("option", regex(&format!("{marker} file")));
            option.click_with(ClickOptions { trial: true, timeout: Some(Duration::from_secs(5)), ..ClickOptions::default() }).await?;
            close_search(&page).await?;
            assert!(editor.is_visible().await?);
        }
        editor.get_by_role("button", "Close editor").click().await?;
        editor.wait_for_state(State::Hidden).await?;
        page.set_viewport_size(1440, 950).await?;

        switch_app(&page, "tasks").await?;
        assert_eq!(page.get_by_label("Filter by category").count().await?, 0);
        page.get_by_role("navigation", "Applications").get_by_role("link", "Prayer Times").click().await?;
        page.get_by_role("button", "Prayer Times settings").click().await?;
        let settings = page.get_by_role("dialog", "Prayer Times settings");
        settings.wait_for().await?;
        assert_eq!(page.locator("[data-editor-pane]").count().await?, 0);

        let prayer_url = format!("{base}/api/v1/apps/org.dar.prayer-times/userData/settings");
        let before = page.request().get(&prayer_url).await?.json()?["value"]["location"]["label"]
            .as_str()
            .context("before location label")?
            .to_owned();
        settings.get_by_label("Location name").fill("").await?;
        page.keyboard().press("Escape").await?;
        settings.wait_for_state(State::Hidden).await?;
        let after_invalid = page.request().get(&prayer_url).await?.json()?["value"]["location"]["label"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert_eq!(after_invalid, before);

        page.get_by_role("button", "Prayer Times settings").click().await?;
        settings.get_by_label("Location name").fill("Finished change").await?;
        page.keyboard().press("Escape").await?;
        settings.wait_for_state(State::Hidden).await?;
        page.wait_for_function_with(
            "async (url) => (await (await fetch(url)).json()).value.location.label === 'Finished change'",
            json!(prayer_url),
        )
        .await?;

        let notes = page.request().get(&format!("{base}/api/v1/notes/library")).await?.json()?;
        let saved = notes["notes"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["name"] == format!("{marker} note.md")))
            .context("saved note in library")?;
        assert!(saved["revision"].as_u64().unwrap_or(0) > 1);
        assert_eq!(*errors.lock().unwrap(), Vec::<String>::new());
        Ok::<(), Error>(())
    }
    .await;

    if result.is_err() {
        let _ = page
            .screenshot(t.screenshot("refinement-failure.png"), true)
            .await;
    }
    owner.disconnect();
    context.close().await.ok();
    result
}
