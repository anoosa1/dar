//! Prayer Times in the browser (`prayer-times-browser.mjs`): separately installed React SDK app,
//! settings sheet setup, save-as-you-go with Undo, invalid input never saved, location denial,
//! desktop/tablet/phone viewports, bottom sheet, focus return, conflict reload, next-day Fajr,
//! and admin inventory.

use dar_acceptance::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const SUITE: Suite = Suite {
    name: "prayer-times-browser",
    cases: &[case!(prayer_times_journey, 120)],
};

/// Installs a fake `Date` clock (matching Playwright's `page.clock.install` + `setFixedTime`).
const FAKE_CLOCK_INIT: &str = r#"(() => {
    const NativeDate = globalThis.Date;
    const realNow = () => NativeDate.now();
    let mode = "offset";
    let base = NativeDate.parse("2026-11-08T18:00:00Z");
    let started = realNow();
    let fixed = base;
    const current = () => (mode === "fixed" ? fixed : base + (realNow() - started));
    function FakeDate(...args) {
        if (!(this instanceof FakeDate)) return new NativeDate(current()).toString();
        if (args.length === 0) return new NativeDate(current());
        return new NativeDate(...args);
    }
    FakeDate.prototype = NativeDate.prototype;
    FakeDate.now = () => current();
    FakeDate.parse = NativeDate.parse.bind(NativeDate);
    FakeDate.UTC = NativeDate.UTC.bind(NativeDate);
    Object.setPrototypeOf(FakeDate, NativeDate);
    globalThis.Date = FakeDate;
    globalThis.__darClock = {
        setFixedTime: (ms) => { mode = "fixed"; fixed = ms; },
        now: () => current(),
    };
})();"#;

pub async fn prayer_times_journey(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let bootstrap = login(&base).await?;
    let settings_path = "/api/v1/apps/org.dar.prayer-times/userData/settings";
    let settings_url = format!("{base}{settings_path}");
    let old = bootstrap.get(settings_path).await?.json()?;
    if !old.is_null() {
        let rev = old["revision"].as_u64().context("old revision")?;
        let deleted = Reply::from(
            bootstrap
                .request("DELETE", settings_path)
                .header("if-match", format!("\"{rev}\""))
                .send()
                .await?,
        )
        .await?;
        assert_eq!(deleted.status, 200);
    }

    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 950))
        .await?;
    let page = context.new_page().await?;
    let expected_conflict = Arc::new(AtomicBool::new(false));
    let errors = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = errors.clone();
        page.on_page_error(move |msg| sink.lock().unwrap().push(msg.clone()));
    }
    {
        let sink = errors.clone();
        let conflict_flag = expected_conflict.clone();
        page.on_console(move |msg| {
            if msg.kind == "error"
                && !(msg.text.contains("401") && msg.text.contains("Unauthorized"))
                && !(conflict_flag.load(Ordering::Relaxed) && msg.text.contains("409"))
            {
                sink.lock().unwrap().push(msg.text.clone());
            }
        });
    }

    let outcome: Result<()> = async {
        page.add_init_script(FAKE_CLOCK_INIT).await?;
        page.goto(&base).await?;
        page.get_by_label("Username").fill("admin").await?;
        page.get_by_label("Password").fill(PASSWORD).await?;
        page.get_by_role("button", "Sign in").click().await?;
        page.get_by_role("button", "All apps").click().await?;
        page.get_by_role("dialog", "All apps").get_by_role("link", "Prayer Times").click().await?;

        // Setup uses the settings sheet: nothing saves until the location and method are complete.
        let session = page.request().get(&format!("{base}/api/v1/auth/session")).await?.json()?;
        let csrf = session["csrfToken"].as_str().context("csrfToken")?.to_owned();

        page.get_by_role("button", "Set up Prayer Times").click().await?;
        let mut editor = page.get_by_role("dialog", "Prayer Times settings");
        editor.get_by_label("Location name").fill("Toronto").await?;
        editor.get_by_label("Latitude").fill("43.6532").await?;
        editor.get_by_label("Longitude").fill("-79.3832").await?;
        editor.get_by_label("Timezone").fill("America/Toronto").await?;
        editor.get_by_role("button", "Use my location").click().await?;
        editor.get_by_text("Location was not available. Enter coordinates manually.").wait_for().await?;
        assert_eq!(editor.get_by_label("Latitude").input_value().await?, "43.6532");
        assert_eq!(page.request().get(&settings_url).await?.json()?, Value::Null);

        editor.get_by_role("tab", "Calculation").click().await?;
        editor.get_by_role("radio", regex("^ISNA\\b")).check().await?;
        editor.get_by_text("Method set to ISNA").wait_for().await?;
        let persisted = page.request().get(&settings_url).await?.json()?;
        assert_eq!(persisted["value"]["location"]["latitude"], 43.6532);
        assert_eq!(persisted["revision"], 1);

        // Today band and week table appear behind the sheet as soon as the first save lands.
        page.get_by_role("heading", regex(" at ")).level(1).wait_for().await?;
        assert_eq!(page.locator(".prayer-week tbody tr").count().await?, 7);

        // Each valid change saves, marks the affected times and can be undone.
        editor.get_by_role("radiogroup", "Asr convention").get_by_role("radio", "Hanafi").check().await?;
        editor.get_by_text("Asr set to Hanafi").wait_for().await?;
        assert_eq!(page.locator(".prayer-week td.is-changed").count().await?, 7);
        assert_eq!(page.request().get(&settings_url).await?.json()?["value"]["calculation"]["asrRule"], "hanafi");

        editor.get_by_role("button", "Undo").click().await?;
        editor.get_by_text("Change undone").wait_for().await?;
        assert_eq!(page.locator(".prayer-week td.is-changed").count().await?, 0);
        assert_eq!(page.request().get(&settings_url).await?.json()?["value"]["calculation"]["asrRule"], "standard");

        // Invalid text keeps its error and is never saved.
        editor.get_by_role("tab", "Location").click().await?;
        let longitude = editor.get_by_label("Longitude");
        longitude.fill("-279.3832").await?;
        longitude.press("Enter").await?;
        editor.get_by_text("Between \u{2212}180 and 180. Not saved.").wait_for().await?;
        assert_eq!(longitude.get_attribute("aria-invalid").await?, Some("true".into()));
        assert_eq!(page.request().get(&settings_url).await?.json()?["value"]["location"]["longitude"], -79.3832);

        page.keyboard().press("Escape").await?;
        editor.wait_for_state(State::Hidden).await?;
        page.get_by_role("button", "Today").first().click().await?;
        let details = page.get_by_role("complementary", "Day details");
        details.wait_for().await?;
        details.get_by_label("Prayer timetable").wait_for().await?;
        assert_eq!(details.locator(".prayer-time-row").count().await?, 6);
        assert!(details.text_content().await?.contains("Asr"));

        page.reload().await?;
        page.get_by_role("button", "Today").last().click().await?;
        details.wait_for().await?;
        assert_eq!(details.locator(".prayer-time-row").count().await?, 6);

        for (width, height) in [(1440, 950), (900, 700), (780, 800), (390, 568)] {
            page.set_viewport_size(width, height).await?;
            assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
            assert_eq!(page.evaluate("() => document.documentElement.scrollHeight > innerHeight").await?, json!(false));
            details.get_by_role("button", "Edit prayer settings").scroll_into_view_if_needed().await?;
            let top = page.locator(".topbar").evaluate("(el) => el.getBoundingClientRect().top", Value::Null).await?;
            assert_eq!(top, json!(0));
            page.screenshot(t.screenshot(&format!("prayer-times-{width}.png")), true).await?;
        }

        // Phones show settings as a full sheet below the top bar.
        details.get_by_role("button", "Edit prayer settings").click().await?;
        editor = page.get_by_role("dialog", "Prayer Times settings");
        editor.wait_for().await?;
        editor.evaluate("(el) => Promise.all(el.getAnimations().map(a => a.finished))", Value::Null).await?;
        let sheet = editor.bounding_box().await?.context("settings sheet bounding box")?;
        let usable = page.evaluate("() => document.documentElement.clientWidth").await?.as_f64().context("clientWidth")?;
        ensure!(
            sheet.x <= 1.0 && sheet.x + sheet.width >= usable - 9.0 && sheet.y >= 56.0 && sheet.y + sheet.height <= 569.0,
            "unexpected sheet geometry: {sheet:?}, usable={usable}"
        );
        assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
        page.screenshot(t.screenshot("prayer-times-settings-390.png"), false).await?;
        editor.get_by_role("button", "Close settings").click().await?;
        editor.wait_for_state(State::Hidden).await?;

        details.get_by_role("button", "Back to week").click().await?;
        page.locator(".prayer-day").first().focus().await?;
        page.keyboard().press("Enter").await?;
        details.wait_for().await?;
        assert_eq!(details.get_by_role("button", "Back to week").evaluate("(el) => el === document.activeElement", Value::Null).await?, json!(true));
        details.get_by_role("button", "Back to week").click().await?;
        assert_eq!(page.locator(".prayer-day").first().evaluate("(el) => el === document.activeElement", Value::Null).await?, json!(true));

        // A change saved elsewhere is a conflict with Reload; the sheet then shows the saved version.
        page.locator(".prayer-day").first().click().await?;
        details.get_by_role("button", "Edit prayer settings").click().await?;
        editor = page.get_by_role("dialog", "Prayer Times settings");
        let label = editor.get_by_label("Location name");
        label.fill("Unsaved change").await?;
        let current = page.request().get(&settings_url).await?.json()?;
        let mut changed = current["value"].clone();
        changed["location"]["label"] = json!("Saved elsewhere");
        let rev = current["revision"].as_u64().context("current revision")?;
        let put_reply = page
            .request()
            .fetch(
                "PUT",
                &settings_url,
                &[("origin", &base), ("x-csrf-token", &csrf), ("if-match", &format!("\"{rev}\""))],
                Some(json!({"schemaVersion": 1, "value": changed})),
                None,
            )
            .await?;
        assert_eq!(put_reply.status(), 200);

        expected_conflict.store(true, Ordering::Relaxed);
        label.press("Enter").await?;
        editor.get_by_role("button", "Reload saved settings").click().await?;
        page.wait_for_function("() => document.querySelector('[data-settings-sheet] input')?.value === 'Saved elsewhere'").await?;
        assert_eq!(label.input_value().await?, "Saved elsewhere");
        expected_conflict.store(false, Ordering::Relaxed);

        editor.get_by_role("button", "Close settings").click().await?;
        let next_day_ms = chrono::DateTime::parse_from_rfc3339("2026-11-09T04:30:00Z").unwrap().timestamp_millis();
        page.evaluate_with("(ms) => globalThis.__darClock.setFixedTime(ms)", json!(next_day_ms)).await?;
        page.wait_for_timeout(1100).await;
        details.get_by_text(regex("Next: Fajr")).wait_for().await?;

        page.set_viewport_size(1440, 950).await?;
        page.goto(&format!("{base}/settings?section=installedApps")).await?;
        page.get_by_role("heading", "Apps").wait_for().await?;
        page.get_by_role("heading", "Prayer Times").wait_for().await?;
        Ok(())
    }
    .await;

    if let Err(error) = outcome {
        let _ = page
            .screenshot(t.screenshot("prayer-times-failure.png"), true)
            .await;
        context.close().await?;
        return Err(error);
    }

    context.close().await?;
    assert_eq!(*errors.lock().unwrap(), Vec::<String>::new());
    Ok(())
}
