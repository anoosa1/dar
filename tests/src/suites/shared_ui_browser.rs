//! Shared UI browser journey (`shared-ui-browser.mjs`): centered login, Ctrl+K search, Files
//! preferences & undo, trash confirmations, Markdown editor (autosave, preview, version history,
//! conflict resolution, 503 recovery draft across beforeunload reload), action menu bounds,
//! copy/cut/paste clipboard, sidebar active/hover states across 5 apps x 2 themes, shared
//! settings sheets, profile save, reduced motion, tablet widths, system theme, and regional
//! timezone propagation to Calendar.

use dar_acceptance::browser::{Browser, ContextOptions, Fulfill, State, regex};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use dar_acceptance::ui::{Choice, choose, close_search, switch_app};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const SUITE: Suite = Suite {
    name: "shared-ui-browser",
    cases: &[case!(shared_ui_journey, 240)],
};

pub async fn shared_ui_journey(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let username = format!("browser-ux-{}", &uuid()[..6]);
    admin
        .post("/api/v1/auth/users", &json!({ "username": username, "displayName": "Shared UI tester", "password": PASSWORD }))
        .await?
        .expect(201)?;
    let native = login_as(&base, &username, PASSWORD).await?;
    let file = upload(&native, "UX source.txt", "Shared UI clipboard proof.").await?;
    let _hidden = upload(&native, ".hidden-ui.txt", "Hidden view proof.").await?;

    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 1000))
        .await?;
    let page = context.new_page().await?;
    let expected_save_failure = Arc::new(AtomicBool::new(false));
    let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = errors.clone();
        page.on_page_error(move |msg| sink.lock().unwrap().push(msg.clone()));
    }
    {
        let sink = errors.clone();
        let expected = expected_save_failure.clone();
        page.on_console(move |msg| {
            if msg.kind != "error" {
                return;
            }
            if msg.text.contains("401") && msg.text.contains("Unauthorized") {
                return;
            }
            if expected.load(Ordering::Relaxed)
                && (msg.text.contains("409") || msg.text.contains("503"))
            {
                return;
            }
            sink.lock().unwrap().push(msg.text.clone());
        });
    }

    let result = async {
        page.goto(&base).await?;
        page.get_by_label("Username").wait_for().await?;
        for (width, height) in [(1440u32, 1000u32), (390, 844)] {
            page.set_viewport_size(width, height).await?;
            let box_rect = page.locator(".auth-card").bounding_box().await?.context("auth-card box")?;
            assert!(
                (box_rect.x + box_rect.width / 2.0 - width as f64 / 2.0).abs() < 12.0
                    && (box_rect.y + box_rect.height / 2.0 - height as f64 / 2.0).abs() < 2.0,
                "auth-card not centered at {width}x{height}: {box_rect:?}"
            );
            assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
            page.screenshot(t.screenshot(&format!("login-{width}.png")), true).await?;
        }

        page.set_viewport_size(1440, 1000).await?;
        page.get_by_label("Username").fill(&username).await?;
        page.get_by_label("Password").fill(PASSWORD).await?;
        page.get_by_role("button", "Sign in").click().await?;
        page.get_by_role("button", "Actions for UX source.txt").wait_for().await?;

        page.keyboard().press("Control+k").await?;
        assert_eq!(page.get_by_label("Search all apps").evaluate("el => el === document.activeElement", Value::Null).await?, json!(true));
        page.get_by_label("Search all apps").fill("UX source").await?;
        page.keyboard().press("Escape").await?;
        assert_eq!(page.get_by_label("Search all apps").input_value().await?, "");
        close_search(&page).await?;

        let storage_box = page.locator(".storage").bounding_box().await?.context("storage box")?;
        let settings_box = page.get_by_role("button", "Files settings").bounding_box().await?.context("settings box")?;
        assert!(settings_box.y + settings_box.height <= 56.0 && storage_box.y + storage_box.height > 800.0);
        assert_eq!(page.get_by_text("Your files. Your place.").count().await?, 0);
        assert_eq!(page.get_by_role("button", "Actions for .hidden-ui.txt").count().await?, 0);

        page.get_by_role("button", "Actions for UX source.txt").click().await?;
        page.get_by_role("menuitem", "Rename").wait_for().await?;
        assert_eq!(page.get_by_role("complementary", "File details").count().await?, 0);
        page.get_by_role("menuitem", "Details").click().await?;
        page.get_by_role("complementary", "File details").wait_for().await?;
        page.get_by_role("button", "Close details").click().await?;

        page.get_by_role("button", "Files settings").click().await?;
        let mut dialog = page.get_by_role("dialog", "Files settings");
        dialog.get_by_role("radiogroup", "Default view").get_by_role("radio", "Grid").check().await?;
        dialog.get_by_text("Default view set to Grid").wait_for().await?;
        page.get_by_role("button", "Grid view").pressed(true).wait_for().await?;
        dialog.get_by_role("switch", "Show hidden files").check().await?;
        dialog.get_by_text("Hidden files shown").wait_for().await?;
        let trash_switch = dialog.get_by_role("switch", "Confirm before moving files to trash");
        trash_switch.check().await?;
        dialog.get_by_text("Trash confirmation on").wait_for().await?;
        dialog.get_by_role("button", "Undo").click().await?;
        dialog.get_by_text("Change undone").wait_for().await?;
        assert!(!trash_switch.is_checked().await?);
        trash_switch.check().await?;
        dialog.get_by_text("Trash confirmation on").wait_for().await?;
        dialog.get_by_role("button", "Close settings").click().await?;
        page.reload().await?;
        page.get_by_role("button", "Grid view").pressed(true).wait_for().await?;
        assert_eq!(page.get_by_role("button", "Grid view").get_attribute("aria-pressed").await?.as_deref(), Some("true"));
        page.get_by_role("button", "Actions for .hidden-ui.txt").wait_for().await?;

        // Trash confirmation applies to menu, bulk toolbar, and details.
        page.get_by_role("button", "Actions for UX source.txt").click().await?;
        page.get_by_role("menuitem", "Move to deleted files").click().await?;
        dialog = page.get_by_role("dialog", "Move to deleted files?");
        dialog.get_by_role("button", "Cancel").click().await?;
        page.get_by_role("button", "Actions for UX source.txt").wait_for().await?;
        page.get_by_role("button", "Move selected items to deleted files").click().await?;
        dialog = page.get_by_role("dialog", "Move to deleted files?");
        page.keyboard().press("Escape").await?;
        dialog.wait_for_state(State::Hidden).await?;

        page.get_by_role("button", "Actions for UX source.txt").click().await?;
        page.get_by_role("menuitem", "Details").click().await?;
        let details = page.get_by_role("complementary", "File details");
        details.get_by_role("heading", "UX source.txt").wait_for().await?;
        for name in ["Rename", "Move to…", "Copy to…", "Move to deleted files"] {
            assert_eq!(details.get_by_role("button", name).count().await?, 0);
        }
        let frame = details.bounding_box().await?.context("details box")?;
        for name in ["Versions", "Sharing", "Details"] {
            details.get_by_role("tab", name).click().await?;
            let next = details.bounding_box().await?.context("details tab box")?;
            assert_eq!(next.width, frame.width);
            assert_eq!(next.height, frame.height);
        }
        page.get_by_role("button", "Close details").click().await?;

        page.set_viewport_size(1440, 1000).await?;
        page.get_by_role("button", "Open account menu").click().await?;
        page.get_by_role("menuitem", "Settings").click().await?;
        page.get_by_role("button", "Appearance").click().await?;
        page.get_by_role("radio", "Dark — Twilight Nightfall").check().await?;
        page.get_by_text("Appearance saved").wait_for().await?;
        page.wait_for_function("() => document.documentElement.dataset.theme === 'mocha'").await?;
        page.reload().await?;
        page.get_by_role("heading", "Appearance").wait_for().await?;
        page.wait_for_function("() => document.documentElement.dataset.theme === 'mocha'").await?;

        switch_app(&page, "files").await?;
        page.get_by_role("button", "Actions for UX source.txt").wait_for().await?;
        page.get_by_role("button", "New").click().await?;
        let new_btn_box = page.get_by_role("button", "New").bounding_box().await?.context("New button box")?;
        let new_menu_box = page.get_by_role("menu", "New").bounding_box().await?.context("New menu box")?;
        assert!(new_menu_box.width >= new_btn_box.width && (new_btn_box.x - new_menu_box.x).abs() < 1.0);
        page.get_by_role("menuitem", "File").click().await?;
        page.get_by_role("menuitem", "Markdown file").click().await?;
        let creation = page.get_by_role("complementary", "New Markdown file");
        creation.get_by_label("Filename").fill("Browser editor.md").await?;
        creation.get_by_role("button", "Create file").click().await?;
        let editor = page.get_by_role("region", "Editor for Browser editor.md");
        let content = "# File editor proof\n\n**Exact bytes** stay in Files.\n";
        editor.get_by_label("File contents").fill(content).await?;
        editor.get_by_role("status", ()).filter_has_text(regex("^All changes saved$")).wait_for().await?;
        editor.get_by_role("button", "Preview").click().await?;
        editor.get_by_role("heading", "File editor proof").wait_for().await?;
        page.screenshot(t.screenshot("files-editor-mocha.png"), true).await?;
        editor.get_by_role("button", "Close editor").click().await?;

        page.get_by_role("button", "Actions for Browser editor.md").click().await?;
        let href = page.get_by_role("menuitem", "Download").get_attribute("href").await?.context("download href")?;
        assert_eq!(page.request().get(&format!("{base}{href}")).await?.text(), content);
        page.get_by_role("menuitem", "Details").click().await?;
        let info = page.get_by_role("complementary", "File details");
        info.get_by_role("tab", "Versions").click().await?;
        assert_eq!(info.locator(".version").count().await?, 2);
        page.get_by_role("button", "Close details").click().await?;

        page.get_by_role("button", "Browser editor.md").click().await?;
        editor.get_by_role("status", ()).filter_has_text(regex("^All changes saved$")).wait_for().await?;
        assert_eq!(editor.get_by_label("File contents").input_value().await?, content);

        expected_save_failure.store(true, Ordering::Relaxed);
        let edited_id = href.split('/').nth(4).context("edited file id")?.to_owned();
        upload_with(
            &native,
            "Browser editor.md",
            "External saved version",
            Upload { file_id: Some(&edited_id), expected_revision: Some(2), ..Upload::default() },
        )
        .await?;
        editor.get_by_label("File contents").fill("Local conflict draft").await?;
        editor.get_by_text("The saved file changed. Your text has been kept.").wait_for().await?;
        assert_eq!(page.request().get(&format!("{base}{href}")).await?.text(), "External saved version");
        editor.get_by_role("button", "Keep my version").click().await?;
        editor.get_by_role("status", ()).filter_has_text(regex("^All changes saved$")).wait_for().await?;
        assert_eq!(page.request().get(&format!("{base}{href}")).await?.text(), "Local conflict draft");

        page.route("**/api/v1/files/uploads", Some(1), |route| async move {
            route.fulfill(Fulfill::json(503, &json!({ "error": { "message": "Simulated file save interruption" } }))).await
        })
        .await?;
        editor.get_by_label("File contents").fill("Recovered file draft").await?;
        editor.get_by_text("Simulated file save interruption").wait_for().await?;
        page.accept_next_dialog();
        page.reload().await?;
        page.get_by_role("button", "Browser editor.md").click().await?;
        editor.get_by_text("A recovery draft is available.").wait_for().await?;
        editor.get_by_role("button", "Restore draft").click().await?;
        editor.get_by_role("status", ()).filter_has_text(regex("^All changes saved$")).wait_for().await?;
        assert_eq!(page.request().get(&format!("{base}{href}")).await?.text(), "Recovered file draft");
        expected_save_failure.store(false, Ordering::Relaxed);
        editor.get_by_role("button", "Close editor").click().await?;

        for height in [520u32, 700, 1000] {
            page.set_viewport_size(1440, height).await?;
            let trigger = page.get_by_role("button", "Actions for UX source.txt");
            trigger.scroll_into_view_if_needed().await?;
            trigger.click().await?;
            let menu = page.get_by_role("menu", "Actions for UX source.txt");
            menu.wait_for().await?;
            page.keyboard().press("End").await?;
            let box_rect = menu.bounding_box().await?.context("action menu box")?;
            assert!(box_rect.y >= 0.0 && box_rect.y + box_rect.height <= height as f64, "{height}: {box_rect:?}");
            page.keyboard().press("Escape").await?;
        }
        page.set_viewport_size(1440, 1000).await?;

        page.get_by_role("button", "New").click().await?;
        page.get_by_role("menuitem", "Folder").click().await?;
        dialog = page.locator("dialog,[data-editor-pane]");
        dialog.get_by_label("Name").fill("UX destination").await?;
        dialog.get_by_role("button", "Create folder").click().await?;
        dialog.wait_for_state(State::Hidden).await?;

        page.get_by_role("button", "Actions for UX source.txt").click().await?;
        page.get_by_role("menuitem", "Copy").click().await?;
        page.get_by_role("button", "UX destination").click().await?;
        page.get_by_role("heading", "UX destination").wait_for().await?;
        page.get_by_role("button", "Paste here").click().await?;
        page.get_by_role("button", "Actions for UX source.txt").wait_for().await?;

        page.get_by_role("button", "Actions for UX source.txt").click().await?;
        page.get_by_role("menuitem", "Cut").click().await?;
        page.get_by_role("button", "All files").first().click().await?;
        page.get_by_role("button", "New").click().await?;
        page.get_by_role("menuitem", "Folder").click().await?;
        dialog = page.locator("dialog,[data-editor-pane]");
        dialog.get_by_label("Name").fill("UX moved").await?;
        dialog.get_by_role("button", "Create folder").click().await?;
        dialog.wait_for_state(State::Hidden).await?;
        page.get_by_role("button", "UX moved").click().await?;
        page.get_by_role("button", "Paste here").click().await?;
        page.get_by_role("button", "Actions for UX source.txt").wait_for().await?;
        assert_eq!(page.get_by_role("button", "Clear clipboard").count().await?, 0);

        page.get_by_role("button", "Actions for UX source.txt").click().await?;
        let copied_href = page.get_by_role("menuitem", "Download").get_attribute("href").await?.context("copied href")?;
        assert_ne!(copied_href, format!("/api/v1/files/{}/content", file.file_id));
        assert_eq!(page.request().get(&format!("{base}{copied_href}")).await?.text(), "Shared UI clipboard proof.");
        assert_eq!(native.get(&format!("/api/v1/files/{}/content", file.file_id)).await?.expect(200)?.text(), "Shared UI clipboard proof.");
        page.keyboard().press("Escape").await?;
        page.screenshot(t.screenshot("shared-files-actions-desktop.png"), true).await?;

        native
            .post("/api/v1/calendar/calendars", &json!({ "name": "Navigation calendar", "color": "#1e66f5" }))
            .await?
            .expect(201)?;
        for (app, title) in [("files", "Files"), ("contacts", "Contacts"), ("calendar", "Calendar"), ("notes", "Notes"), ("tasks", "Tasks")] {
            if app != "files" {
                switch_app(&page, app).await?;
            }
            if app == "calendar" {
                page.get_by_role("button", "Navigation calendar").click().await?;
            }
            let selected = page.locator(".sidebar .nav-item.active").first();
            let other = page.locator(".sidebar .nav-item:not(.active)").first();
            selected.wait_for().await?;
            for theme in ["latte", "mocha"] {
                let mut prefs = native.get_json("/api/v1/pim/preferences").await?;
                let rev = prefs["revision"].as_u64().context("prefs rev")?;
                prefs["appearance"] = json!(theme);
                let saved = Reply::from(
                    native
                        .request("PUT", "/api/v1/pim/preferences")
                        .header("if-match", format!("\"preferences:{rev}\""))
                        .json(&prefs)
                        .send()
                        .await?,
                )
                .await?;
                assert_eq!(saved.status, 200, "{}", saved.text());
                page.evaluate("() => window.dispatchEvent(new Event('dar:preferences'))").await?;
                page.wait_for_function_with("(theme) => document.documentElement.dataset.theme === theme", json!(theme)).await?;
                page.mouse().move_to(1400.0, 900.0, 1).await?;
                let fill = selected.evaluate("el => getComputedStyle(el).backgroundColor", Value::Null).await?;
                let sidebar_bg = page.locator(".sidebar").evaluate("el => getComputedStyle(el).backgroundColor", Value::Null).await?;
                assert_ne!(fill, sidebar_bg);
                let normal = other.evaluate("el => getComputedStyle(el).backgroundColor", Value::Null).await?;
                other.hover().await?;
                assert_ne!(other.evaluate("el => getComputedStyle(el).backgroundColor", Value::Null).await?, normal);
                assert_eq!(selected.evaluate("el => getComputedStyle(el).backgroundColor", Value::Null).await?, fill);
                selected.hover().await?;
                assert_ne!(selected.evaluate("el => getComputedStyle(el).backgroundColor", Value::Null).await?, fill);
                page.screenshot(t.screenshot(&format!("navigation-{app}-{theme}.png")), true).await?;
            }
            let creation_btn = if app == "files" { page.get_by_role("button", "New") } else { page.locator(".ui-new") };
            assert_eq!(creation_btn.evaluate("el => getComputedStyle(el).borderTopLeftRadius", Value::Null).await?, json!("16px"));
            assert_eq!(creation_btn.evaluate("el => getComputedStyle(el).height", Value::Null).await?, json!("48px"));
            page.get_by_role("button", format!("{title} settings")).wait_for().await?;
            page.get_by_role("button", "Open account menu").click().await?;
            assert_eq!(page.get_by_role("menuitem", "Settings").count().await?, 1);
            assert_eq!(page.get_by_role("menuitem", "Administration").count().await?, 0);
            page.keyboard().press("Escape").await?;
            // A click while the account menu is still closing only dismisses it.
            page.get_by_role("menuitem", "Settings").wait_for_state(State::Detached).await?;
            dialog = page.get_by_role("dialog", format!("{title} settings"));
            ui::open_dialog(&page.get_by_role("button", format!("{title} settings")), &dialog).await?;
            dialog.get_by_role("tab", "General").selected(true).wait_for().await?;
            dialog.locator(".settings-row").first().wait_for().await?;
            page.screenshot(t.screenshot(&format!("shared-{app}-settings.png")), true).await?;
            assert_eq!(dialog.evaluate("el => getComputedStyle(el).animationName", Value::Null).await?, json!("ui-pane-in"));
            page.keyboard().press("Escape").await?;
            dialog.wait_for_state(State::Hidden).await?;
        }

        page.get_by_role("button", "Open account menu").click().await?;
        page.get_by_role("menuitem", "Settings").click().await?;
        page.get_by_role("heading", "Overview").wait_for().await?;
        page.locator(".settings-overview-card").first().wait_for().await?;
        page.screenshot(t.screenshot("shared-settings-overview.png"), true).await?;
        page.get_by_role("navigation", "Personal settings").wait_for().await?;
        assert_eq!(page.get_by_role("navigation", "Administration settings").count().await?, 0);
        page.get_by_role("button", "All apps").click().await?;
        page.get_by_role("dialog", "All apps").get_by_role("link", "Files").wait_for().await?;
        assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
        page.keyboard().press("Escape").await?;
        page.get_by_role("button", "Edit profile").click().await?;
        page.get_by_label("Display name").fill("Consistent profile").await?;
        page.wait_for_next_load(page.get_by_role("button", "Save changes").click()).await?;
        page.get_by_label("Display name").wait_for().await?;
        assert_eq!(page.get_by_label("Display name").input_value().await?, "Consistent profile");
        page.screenshot(t.screenshot("shared-personal-settings-desktop.png"), true).await?;

        page.set_viewport_size(390, 844).await?;
        page.get_by_role("button", "Open navigation").click().await?;
        page.get_by_role("button", "Security").click().await?;
        page.get_by_role("heading", "Account security").wait_for().await?;
        assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
        page.screenshot(t.screenshot("shared-personal-settings-phone.png"), true).await?;
        switch_app(&page, "files").await?;
        page.get_by_role("button", "Actions for UX source.txt").wait_for().await?;
        page.get_by_role("button", "Actions for UX source.txt").click().await?;
        page.get_by_role("menuitem", "Rename").wait_for().await?;
        assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
        page.screenshot(t.screenshot("shared-files-actions-phone.png"), true).await?;
        page.keyboard().press("Escape").await?;

        page.emulate_media(None, Some("reduce"), None).await?;
        page.get_by_role("button", "Files settings").click().await?;
        dialog = page.get_by_role("dialog", "Files settings");
        assert_eq!(dialog.evaluate("el => getComputedStyle(el).animationName", Value::Null).await?, json!("none"));
        page.keyboard().press("Escape").await?;
        dialog.wait_for_state(State::Hidden).await?;
        assert_eq!(page.get_by_role("button", "Files settings").evaluate("el => el === document.activeElement", Value::Null).await?, json!(true));

        // Settings shares app surfaces and keeps long forms inside its own viewport.
        page.goto(&format!("{base}/settings?section=profile")).await?;
        page.get_by_label("Display name").wait_for().await?;
        for (width, height) in [(1280u32, 700u32), (780, 700), (390, 568)] {
            page.set_viewport_size(width, height).await?;
            let frame = page
                .locator(".identity-main")
                .evaluate(
                    "el => ({
                        box: el.getBoundingClientRect().toJSON(),
                        background: getComputedStyle(el).backgroundColor,
                        surface: getComputedStyle(document.querySelector('.ui-settings-nav')).backgroundColor,
                        shell: getComputedStyle(document.querySelector('.identity-settings')).backgroundColor,
                        doc: document.documentElement.scrollHeight,
                        overflow: document.documentElement.scrollWidth > innerWidth
                    })",
                    Value::Null,
                )
                .await?;
            assert_ne!(frame["background"], frame["shell"]);
            if width > 800 {
                assert_eq!(frame["surface"], json!("rgba(0, 0, 0, 0)"));
            }
            assert_eq!(frame["overflow"], json!(false));
            assert!(frame["doc"].as_f64().unwrap_or(9999.0) <= height as f64 + 1.0);
            assert!(frame["box"]["bottom"].as_f64().unwrap_or(9999.0) <= height as f64 + 1.0);
            page.get_by_role("button", "Save changes").scroll_into_view_if_needed().await?;
            assert_eq!(
                page.locator(".topbar").evaluate("el => el.getBoundingClientRect().top", Value::Null).await?.as_f64().unwrap_or(-1.0),
                0.0
            );
            if width > 800 {
                let nav = page.locator(".ui-settings-nav").bounding_box().await?.context("settings nav box")?;
                assert_eq!(nav.width, 230.0);
                assert_eq!(nav.y, 56.0);
            }
        }

        switch_app(&page, "files").await?;
        // Tablet layout across all five apps.
        page.set_viewport_size(700, 900).await?;
        for (app, title) in [("files", "Files"), ("contacts", "Contacts"), ("calendar", "Calendar"), ("notes", "Notes"), ("tasks", "Tasks")] {
            if app != "files" {
                switch_app(&page, app).await?;
            }
            let heading = format!("{title} settings");
            dialog = page.get_by_role("dialog", heading.as_str());
            ui::open_dialog(&page.get_by_role("button", heading.as_str()), &dialog).await?;
            dialog.get_by_role("tab", "General").selected(true).wait_for().await?;
            assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
            page.keyboard().press("Escape").await?;
            dialog.wait_for_state(State::Hidden).await?;
        }

        page.get_by_role("button", "Open account menu").click().await?;
        page.get_by_role("dialog", "Apps and account").get_by_role("link", "Settings").click().await?;
        page.get_by_role("button", "Open navigation").click().await?;
        page.get_by_role("button", "Security").click().await?;
        page.get_by_role("heading", "Account security").wait_for().await?;
        assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));

        page.get_by_role("button", "Open account menu").click().await?;
        page.get_by_role("dialog", "Apps and account").get_by_role("link", "Settings").click().await?;
        page.get_by_role("button", "Open navigation").click().await?;
        page.get_by_role("button", "Appearance").click().await?;
        page.get_by_role("radio", "Light — Twilight Afterglow").check().await?;
        page.wait_for_function("() => document.documentElement.dataset.theme === 'latte'").await?;
        page.emulate_media(Some("dark"), None, None).await?;
        assert_eq!(page.evaluate("() => document.documentElement.dataset.theme").await?, json!("latte"));
        page.get_by_role("radio", "Follow system").check().await?;
        page.wait_for_function("() => document.documentElement.dataset.theme === 'mocha'").await?;
        page.emulate_media(Some("light"), None, None).await?;
        page.wait_for_function("() => document.documentElement.dataset.theme === 'latte'").await?;
        page.screenshot(t.screenshot("appearance-latte-phone.png"), true).await?;

        page.get_by_role("button", "Open navigation").click().await?;
        page.get_by_role("button", "Regional settings").click().await?;
        choose(&page.get_by_role("combobox", "Timezone"), Choice::Label("Asia/Tokyo")).await?;
        page.get_by_text("Regional settings saved").wait_for().await?;
        page.reload().await?;
        page.get_by_role("combobox", "Timezone").wait_for().await?;
        assert!(page.get_by_role("combobox", "Timezone").text_content().await?.contains("Asia/Tokyo"));
        native
            .post("/api/v1/calendar/calendars", &json!({ "name": "Regional calendar", "color": "#1e66f5" }))
            .await?
            .expect(201)?;
        switch_app(&page, "calendar").await?;
        page.get_by_role("button", "Open navigation").click().await?;
        page.get_by_role("button", "New event").click().await?;
        let new_event = page.get_by_role("complementary", "New event");
        new_event.get_by_label("Timezone").wait_for().await?;
        assert_eq!(new_event.get_by_label("Timezone").input_value().await?, "Asia/Tokyo");
        new_event.get_by_role("button", "Close editor").click().await?;

        assert_eq!(*errors.lock().unwrap(), Vec::<String>::new());
        Ok::<(), Error>(())
    }
    .await;

    if result.is_err() && !page.is_closed() {
        let _ = page
            .screenshot(t.screenshot("shared-ui-failure.png"), true)
            .await;
    }
    context.close().await.ok();
    result
}
