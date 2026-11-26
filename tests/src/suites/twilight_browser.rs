//! Twilight theme contrast & viewport overflow audit (`twilight-browser.mjs`): exercises every
//! app surface, overlay, settings section, long-name deletion dialog, and account appearance
//! persistence across Latte/Mocha themes and desktop (1440px) / phone (390px) viewports, writing
//! `twilight/manifest.json` alongside every screenshot.

use dar_acceptance::browser::{Browser, ContextOptions, Locator, Page};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use dar_acceptance::ui::{calendar_date, file_details};
use std::path::Path;

pub const SUITE: Suite = Suite {
    name: "twilight-browser",
    cases: &[case!(twilight_contrast_and_overflow, 300)],
};

const CONTRAST_AUDIT_JS: &str = r#"() => {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 1;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  function rgba(value) { ctx.clearRect(0, 0, 1, 1); ctx.fillStyle = value; ctx.fillRect(0, 0, 1, 1); return [...ctx.getImageData(0, 0, 1, 1).data]; }
  function over(top, bottom) { const alpha = top[3] / 255; return [...top.slice(0, 3).map((v, i) => v * alpha + bottom[i] * (1 - alpha)), 255]; }
  function background(el) { const c = rgba(getComputedStyle(el).backgroundColor); return c[3] === 255 ? c : over(c, el.parentElement ? background(el.parentElement) : [255, 255, 255, 255]); }
  function luminance(c) { return c.slice(0, 3).map(v => { v /= 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }).reduce((s, v, i) => s + v * [0.2126, 0.7152, 0.0722][i], 0); }
  const issues = [];
  for (const el of document.querySelectorAll("button,a,label,input,textarea,h1,h2,h3,p,td,th,strong,small,span")) {
    if (!el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true }) || el.closest(":disabled,[aria-disabled=true],.sr-only") || getComputedStyle(el).opacity !== "1") continue;
    if (![...el.childNodes].some(n => n.nodeType === 3 && n.textContent.trim()) && !el.matches("input:not([type=checkbox],[type=radio],[type=color]),textarea")) continue;
    const rect = el.getBoundingClientRect();
    if (rect.bottom <= 0 || rect.top >= innerHeight || rect.right <= 0 || rect.left >= innerWidth) continue;
    const style = getComputedStyle(el), bg = background(el), fg = over(rgba(style.color), bg), a = luminance(fg), b = luminance(bg), ratio = (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
    const large = parseFloat(style.fontSize) >= 24 || (parseFloat(style.fontSize) >= 18.66 && parseFloat(style.fontWeight) >= 700), min = large ? 3 : 4.5;
    if (ratio + 0.02 < min) issues.push({ text: (el.textContent || el.getAttribute("aria-label") || el.tagName).trim().slice(0, 70), class: el.className, ratio: +ratio.toFixed(2), min, color: style.color, bg });
  }
  return { contrast: issues, overflow: document.documentElement.scrollWidth > innerWidth, verticalOverflow: document.documentElement.scrollHeight > innerHeight + 1 };
}"#;

async fn capture(
    page: &Page,
    output: &Path,
    theme: &str,
    width: u32,
    name: &str,
    manifest: &mut Vec<Value>,
    failures: &mut Vec<Value>,
) -> Result<()> {
    let issues = page.evaluate(CONTRAST_AUDIT_JS).await?;
    let filename = format!("{theme}-{width}-{name}.png");
    page.screenshot(output.join(&filename), true).await?;
    let mut entry = issues.clone();
    entry["filename"] = json!(filename);
    let has_overflow = issues["overflow"].as_bool().unwrap_or(false)
        || issues["verticalOverflow"].as_bool().unwrap_or(false)
        || !issues["contrast"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true);
    if has_overflow {
        failures.push(entry.clone());
    }
    manifest.push(entry);
    Ok(())
}

async fn navigate(page: &Page, base: &str, path: &str, ready: Locator) -> Result<()> {
    page.goto(&format!("{base}{path}")).await?;
    ready.wait_for().await?;
    page.wait_for_network_idle().await?;
    Ok(())
}

async fn open_settings(page: &Page, app: &str) -> Result<()> {
    let btn = page.get_by_role("button", format!("{app} settings"));
    if !btn.is_visible().await? {
        let nav = page.get_by_role("button", "Open navigation");
        if nav.is_visible().await? {
            nav.click().await?;
        }
    }
    btn.click().await?;
    page.get_by_role("dialog", format!("{app} settings"))
        .wait_for()
        .await
}

async fn close_settings(page: &Page) -> Result<()> {
    page.get_by_role("button", "Close settings").click().await
}

pub async fn twilight_contrast_and_overflow(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let output = t.dir().join("twilight");
    std::fs::create_dir_all(&output)?;
    let admin = login(&base).await?;
    let marker = format!("Twilight {}", &uuid()[..6]);

    upload(
        &admin,
        &format!("{marker}.md"),
        "# A quieter week\n\nKeep room for the important things.\n",
    )
    .await?;
    {
        let connection = admin.connect().await?;
        for i in 0..24 {
            reducer(|cb| {
                connection.reducers.create_folder_then(
                    uuid(),
                    String::new(),
                    format!("{marker} folder {:02}", i + 1),
                    cb,
                )
            })
            .await?;
        }
        connection.disconnect();
    }
    for i in 0..14 {
        upload(
            &admin,
            &format!("{marker} file {:02}.txt", i + 1),
            format!("Scroll check file {}\n", i + 1),
        )
        .await?;
    }

    let notes_root = admin.get_json("/api/v1/notes/library").await?["rootId"]
        .as_str()
        .context("notes rootId")?
        .to_owned();
    let note = upload_with(
        &admin,
        &format!("{marker} note.md"),
        "# A quieter week\n\n- Plant herbs\n- Finish the chapter\n- Take a long walk\n",
        Upload {
            parent_id: &notes_root,
            ..Upload::default()
        },
    )
    .await?;
    let long_notebook_name = format!("{}{}", "N".repeat(223), uuid().replace('-', ""));
    let long_task_list_name = format!("{}{}", "T".repeat(223), uuid().replace('-', ""));
    {
        let folders = admin.connect().await?;
        reducer(|cb| {
            folders.reducers.create_folder_then(
                uuid(),
                notes_root.clone(),
                long_notebook_name.clone(),
                cb,
            )
        })
        .await?;
        folders.disconnect();
    }
    admin
        .post(
            "/api/v1/tasks/lists",
            &json!({ "name": long_task_list_name, "color": "#73396f" }),
        )
        .await?
        .expect(201)?;
    let tasklist = admin
        .post(
            "/api/v1/tasks/lists",
            &json!({ "name": marker, "color": "#73396f" }),
        )
        .await?
        .expect(201)?
        .json()?;
    let task = admin
        .post(
            "/api/v1/tasks/items",
            &json!({ "collectionId": tasklist["id"], "title": marker, "description": "Make room for the important things.", "priority": 5 }),
        )
        .await?
        .expect(201)?
        .json()?;
    let calendar = admin
        .post(
            "/api/v1/calendar/calendars",
            &json!({ "name": marker, "color": "#73396f" }),
        )
        .await?
        .expect(201)?
        .json()?;
    admin
        .post(
            &format!("/api/v1/calendar/calendars/{}/items", calendar["id"].as_str().context("cal id")?),
            &json!({ "data": { "title": marker, "start": "2026-11-08T10:00", "end": "2026-11-08T11:00", "timezone": "UTC", "allDay": false } }),
        )
        .await?
        .expect(201)?;
    let addressbook = admin
        .post(
            "/api/v1/contacts/books",
            &json!({ "name": marker, "color": "#73396f" }),
        )
        .await?
        .expect(201)?
        .json()?;
    let contact = admin
        .post(
            &format!("/api/v1/contacts/books/{}/items", addressbook["id"].as_str().context("book id")?),
            &json!({ "data": { "firstName": "Amira", "lastName": marker, "emails": [{ "label": "work", "value": "amira@example.test" }], "organization": "Design team" } }),
        )
        .await?
        .expect(201)?
        .json()?;

    let prayer_path = "/api/v1/apps/org.dar.prayer-times/userData/settings";
    let prayer = admin.get_json(prayer_path).await?;
    if prayer.is_null() {
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

    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 950))
        .await?;
    let page = context.new_page().await?;
    let errors: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    {
        let sink = errors.clone();
        page.on_page_error(move |msg| sink.lock().unwrap().push(msg.clone()));
    }

    let mut manifest = Vec::new();
    let mut failures = Vec::new();

    let result = async {
        for theme in ["latte", "mocha"] {
            page.emulate_media(Some(if theme == "latte" { "light" } else { "dark" }), Some("reduce"), None).await?;
            for width in [1440u32, 390] {
                let height = if width == 390 { 844 } else { 950 };
                page.set_viewport_size(width, height).await?;
                context.clear_cookies().await?;
                navigate(&page, &base, "/", page.get_by_label("Username")).await?;
                capture(&page, &output, theme, width, "signin", &mut manifest, &mut failures).await?;

                page.get_by_label("Username").fill("admin").await?;
                page.get_by_label("Password").fill(PASSWORD).await?;
                page.get_by_role("button", "Sign in").click().await?;
                page.locator("header.topbar").wait_for().await?;

                navigate(&page, &base, "/settings?section=appearance", page.get_by_role("radio", "Follow system")).await?;
                let choice = page.get_by_role(
                    "radio",
                    if theme == "latte" { "Light — Twilight Afterglow" } else { "Dark — Twilight Nightfall" },
                );
                if !choice.is_checked().await? {
                    choice.check().await?;
                    page.get_by_text("Appearance saved").wait_for().await?;
                }
                page.reload().await?;
                page.wait_for_function_with("(t) => document.documentElement.dataset.theme === t", json!(theme)).await?;
                page.get_by_role("radio", "Follow system").wait_for().await?;
                capture(&page, &output, theme, width, "appearance", &mut manifest, &mut failures).await?;

                navigate(&page, &base, "/", page.get_by_role("button", format!("Actions for {marker}.md"))).await?;
                capture(&page, &output, theme, width, "files", &mut manifest, &mut failures).await?;

                let file_scroll = page
                    .locator(".files-main")
                    .evaluate(
                        "el => { el.scrollTop = 0; const before = el.scrollTop; el.scrollTop = el.scrollHeight; return { before, after: el.scrollTop, height: el.clientHeight, total: el.scrollHeight }; }",
                        Value::Null,
                    )
                    .await?;
                assert!(
                    file_scroll["total"].as_f64().unwrap_or(0.0) > file_scroll["height"].as_f64().unwrap_or(0.0)
                        && file_scroll["after"].as_f64().unwrap_or(0.0) > file_scroll["before"].as_f64().unwrap_or(0.0),
                    "{file_scroll}"
                );
                assert_eq!(
                    page.locator(".file-table tbody tr")
                        .last()
                        .evaluate(
                            "el => { const row = el.getBoundingClientRect(), pane = el.closest('.files-main').getBoundingClientRect(); return row.top >= pane.top && row.bottom <= pane.bottom + 1; }",
                            Value::Null,
                        )
                        .await?,
                    json!(true)
                );
                assert_eq!(
                    page.locator(".topbar").evaluate("el => el.getBoundingClientRect().top", Value::Null).await?.as_f64().unwrap_or(-1.0),
                    0.0
                );
                if width > 800 {
                    assert_eq!(page.locator(".sidebar").evaluate("el => el.getBoundingClientRect().top", Value::Null).await?.as_f64().unwrap_or(-1.0), 56.0);
                }
                assert_eq!(page.evaluate("() => document.documentElement.scrollHeight > innerHeight + 1").await?, json!(false));
                page.locator(".files-main").evaluate("el => { el.scrollTop = 0; }", Value::Null).await?;

                if width < 801 {
                    page.get_by_role("button", "Open navigation").click().await?;
                    capture(&page, &output, theme, width, "launcher", &mut manifest, &mut failures).await?;
                    page.keyboard().press("Escape").await?;
                } else {
                    page.get_by_role("button", "All apps").click().await?;
                    capture(&page, &output, theme, width, "launcher", &mut manifest, &mut failures).await?;
                    page.keyboard().press("Escape").await?;
                }

                page.get_by_role("button", "Open account menu").click().await?;
                capture(&page, &output, theme, width, "account-menu", &mut manifest, &mut failures).await?;
                page.keyboard().press("Escape").await?;

                page.get_by_role("button", format!("Actions for {marker}.md")).click().await?;
                capture(&page, &output, theme, width, "file-actions", &mut manifest, &mut failures).await?;
                page.get_by_role("menuitem", "Rename").click().await?;
                page.get_by_label("Name").wait_for().await?;
                capture(&page, &output, theme, width, "file-rename", &mut manifest, &mut failures).await?;
                page.get_by_role("button", "Cancel").click().await?;

                file_details(&page, &format!("{marker}.md")).await?;
                let details = page.get_by_role("complementary", "File details");
                details.wait_for().await?;
                details.get_by_role("heading", format!("{marker}.md")).wait_for().await?;
                let bounds = details.bounding_box().await?.context("details bounds")?;
                assert!(
                    bounds.x >= 0.0
                        && bounds.y >= 0.0
                        && bounds.x + bounds.width <= width as f64 + 1.0
                        && bounds.y + bounds.height <= height as f64 + 1.0,
                    "{bounds:?}"
                );
                if width > 900 {
                    let list = page.locator(".files-main");
                    list.evaluate("el => { el.scrollTop = 0; }", Value::Null).await?;
                    let frame = details.bounding_box().await?.context("details frame")?;
                    let before = list.evaluate("el => el.scrollTop", Value::Null).await?.as_f64().unwrap_or(0.0);
                    list.evaluate("el => { el.scrollTop = el.scrollHeight; }", Value::Null).await?;
                    assert!(list.evaluate("el => el.scrollTop", Value::Null).await?.as_f64().unwrap_or(0.0) > before);
                    assert_eq!(details.bounding_box().await?.context("details frame after")?, frame);
                }
                capture(&page, &output, theme, width, "file-details", &mut manifest, &mut failures).await?;
                page.get_by_role("button", "Close details").click().await?;

                open_settings(&page, "Files").await?;
                capture(&page, &output, theme, width, "files-settings", &mut manifest, &mut failures).await?;
                close_settings(&page).await?;

                if width < 801 {
                    page.get_by_role("button", "Open navigation").click().await?;
                }
                page.get_by_role("button", "New").click().await?;
                capture(&page, &output, theme, width, "files-new-menu", &mut manifest, &mut failures).await?;
                page.get_by_role("menuitem", "Folder").click().await?;
                page.get_by_label("Name").wait_for().await?;
                capture(&page, &output, theme, width, "new-folder", &mut manifest, &mut failures).await?;
                page.get_by_role("button", "Cancel").click().await?;

                navigate(&page, &base, "/contacts", page.get_by_role("heading", "All contacts")).await?;
                capture(&page, &output, theme, width, "contacts", &mut manifest, &mut failures).await?;
                let book_id = addressbook["id"].as_str().context("book id")?;
                let contact_href = contact["href"].as_str().context("contact href")?;
                let encoded_href: String = url::form_urlencoded::byte_serialize(contact_href.as_bytes()).collect();
                navigate(
                    &page,
                    &base,
                    &format!("/contacts?book={book_id}&contact={encoded_href}"),
                    page.get_by_role("button", "Edit contact"),
                )
                .await?;
                capture(&page, &output, theme, width, "contact-details", &mut manifest, &mut failures).await?;
                page.get_by_role("button", "Edit contact").click().await?;
                page.get_by_label("First name").wait_for().await?;
                capture(&page, &output, theme, width, "contact-editor", &mut manifest, &mut failures).await?;
                page.get_by_role("button", "Cancel").click().await?;
                page.get_by_role("button", "Delete contact").click().await?;
                capture(&page, &output, theme, width, "contact-delete", &mut manifest, &mut failures).await?;
                page.get_by_role("button", "Cancel").click().await?;
                page.get_by_role("button", "Close contact details").click().await?;
                open_settings(&page, "Contacts").await?;
                capture(&page, &output, theme, width, "contacts-settings", &mut manifest, &mut failures).await?;
                close_settings(&page).await?;
                if width < 801 {
                    page.get_by_role("button", "Open navigation").click().await?;
                }
                page.get_by_role("button", "New address book").click().await?;
                page.get_by_label("Name").wait_for().await?;
                capture(&page, &output, theme, width, "new-address-book", &mut manifest, &mut failures).await?;
                page.get_by_role("button", "Cancel").click().await?;

                navigate(&page, &base, "/calendar", page.get_by_role("button", "Choose date")).await?;
                calendar_date(&page, "2026-11-08").await?;
                capture(&page, &output, theme, width, "calendar", &mut manifest, &mut failures).await?;
                page.get_by_role("button", "Choose date").click().await?;
                capture(&page, &output, theme, width, "calendar-date", &mut manifest, &mut failures).await?;
                page.keyboard().press("Escape").await?;
                open_settings(&page, "Calendar").await?;
                capture(&page, &output, theme, width, "calendar-settings", &mut manifest, &mut failures).await?;
                close_settings(&page).await?;
                if width < 801 {
                    page.get_by_role("button", "Open navigation").click().await?;
                }
                page.get_by_role("button", "New event").click().await?;
                page.get_by_label("Title").wait_for().await?;
                capture(&page, &output, theme, width, "new-event", &mut manifest, &mut failures).await?;
                page.get_by_role("button", "Cancel").click().await?;

                navigate(&page, &base, &format!("/notes?note={}", note.file_id), page.get_by_role("region", format!("Editor for {marker} note.md")))
                    .await?;
                capture(&page, &output, theme, width, "notes-live", &mut manifest, &mut failures).await?;
                page.get_by_role("region", format!("Editor for {marker} note.md")).get_by_role("button", "Reading").click().await?;
                capture(&page, &output, theme, width, "notes-reading", &mut manifest, &mut failures).await?;
                navigate(&page, &base, "/notes", page.get_by_role("heading", "All notes")).await?;
                open_settings(&page, "Notes").await?;
                capture(&page, &output, theme, width, "notes-settings", &mut manifest, &mut failures).await?;
                close_settings(&page).await?;

                let task_id = task["id"].as_str().context("task id")?;
                navigate(&page, &base, &format!("/tasks?item={task_id}"), page.get_by_role("heading", marker.as_str())).await?;
                capture(&page, &output, theme, width, "task-details", &mut manifest, &mut failures).await?;
                navigate(&page, &base, "/tasks", page.get_by_role("heading", "Today")).await?;
                open_settings(&page, "Tasks").await?;
                capture(&page, &output, theme, width, "tasks-settings", &mut manifest, &mut failures).await?;
                close_settings(&page).await?;
                if width < 801 {
                    page.get_by_role("button", "Open navigation").click().await?;
                }
                page.get_by_role("button", "New task").click().await?;
                page.get_by_label("Title").wait_for().await?;
                capture(&page, &output, theme, width, "new-task", &mut manifest, &mut failures).await?;
                page.get_by_role("button", "Cancel").click().await?;

                for (path_seg, noun, long_name, menu, label, title) in [
                    ("notes", "folder", long_notebook_name.as_str(), "Move to deleted files", "Move to deleted files", "Move folder to deleted files?"),
                    ("tasks", "task list", long_task_list_name.as_str(), "Delete task list", "Delete task list", "Delete task list?"),
                ] {
                    navigate(&page, &base, &format!("/{path_seg}"), page.locator("header.topbar")).await?;
                    if width < 801 {
                        page.get_by_role("button", "Open navigation").click().await?;
                    }
                    page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))").await?;
                    let target_action = page.get_by_role("button", format!("Actions for {noun} {long_name}"));
                    for _ in 0..100 {
                        if target_action.is_visible().await? {
                            break;
                        }
                        let next_page = page.get_by_role("button", format!("Next {noun}s"));
                        assert!(next_page.is_enabled().await?);
                        next_page.click().await?;
                    }
                    target_action.click().await?;
                    page.get_by_role("menuitem", menu).click().await?;
                    let dialog = page.get_by_role("dialog", title);
                    dialog.wait_for().await?;
                    assert!(dialog.text_content().await?.contains(long_name));
                    assert_eq!(dialog.get_by_role("button", label).count().await?, 1);
                    assert_eq!(dialog.evaluate("el => el.scrollWidth <= el.clientWidth + 1", Value::Null).await?, json!(true));
                    let action_box = dialog.get_by_role("button", label).bounding_box().await?.context("action box")?;
                    let dialog_box = dialog.bounding_box().await?.context("dialog box")?;
                    assert!(
                        action_box.x >= dialog_box.x
                            && action_box.x + action_box.width <= dialog_box.x + dialog_box.width + 1.0
                            && action_box.y + action_box.height <= height as f64 + 1.0
                    );
                    capture(&page, &output, theme, width, &format!("delete-{}-long-name", noun.replace(' ', "-")), &mut manifest, &mut failures).await?;
                    dialog.get_by_role("button", "Cancel").click().await?;
                }

                navigate(&page, &base, "/apps/org.dar.prayer-times", page.locator(".prayer-week")).await?;
                page.locator(".prayer-day").first().wait_for().await?;
                capture(&page, &output, theme, width, "prayer-times", &mut manifest, &mut failures).await?;
                page.locator(".prayer-day").first().click().await?;
                page.get_by_label("Prayer timetable").wait_for().await?;
                capture(&page, &output, theme, width, "prayer-timetable", &mut manifest, &mut failures).await?;
                page.get_by_role("button", "Back to week").click().await?;
                open_settings(&page, "Prayer Times").await?;
                capture(&page, &output, theme, width, "prayer-settings", &mut manifest, &mut failures).await?;
                close_settings(&page).await?;

                for section in ["profile", "security", "regional", "connected", "instanceAppearance", "installedApps", "applications", "directory", "scim", "keys"] {
                    navigate(&page, &base, &format!("/settings?section={section}"), page.locator(".identity-main:not([aria-busy=true])")).await?;
                    capture(&page, &output, theme, width, &format!("settings-{section}"), &mut manifest, &mut failures).await?;
                }
                page.get_by_role("button", "Rotate signing key").click().await?;
                capture(&page, &output, theme, width, "admin-confirmation", &mut manifest, &mut failures).await?;
                page.get_by_role("button", "Cancel").click().await?;
            }
        }
        assert_eq!(*errors.lock().unwrap(), Vec::<String>::new());
        Ok::<(), Error>(())
    }
    .await;

    std::fs::write(
        output.join("manifest.json"),
        serde_json::to_string_pretty(&json!({ "screenshots": manifest, "failures": failures }))?,
    )?;
    context.close().await.ok();
    result?;
    assert!(
        failures.is_empty(),
        "Visible text contrast and viewport overflow failures ({}): {}",
        failures.len(),
        json!(failures)
    );
    Ok(())
}
