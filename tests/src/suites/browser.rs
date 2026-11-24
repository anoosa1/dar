//! Files in the browser (`browser.mjs`): desktop and phone sign-in, upload and a cancelled
//! upload, preview, rename, favorite, trash and restore, reconnecting during navigation, and
//! signing out; no page or console errors and no horizontal overflow on the phone.

use dar_acceptance::prelude::*;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

pub const SUITE: Suite = Suite {
    name: "browser",
    cases: &[case!(files_journey, 180), case!(folder_upload, 90)],
};

pub async fn files_journey(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 950))
        .await?;
    let page = context.new_page().await?;
    page.track_sockets("/v1/database/").await?;
    let failures =
        page.collect_errors(|text| text.contains("401") && text.contains("Unauthorized"));
    page.goto(&base).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.screenshot(t.screenshot("auth-desktop.png"), true)
        .await?;
    page.get_by_role("button", "Sign in").click().await?;
    ui::signed_in(&page).await?;
    ui::open_menu_for(
        &page,
        &page.get_by_role("button", "New"),
        &page.get_by_role("menuitem", "Folder"),
    )
    .await?;
    page.get_by_role("menuitem", "Folder").click().await?;
    let folder = format!("Browser {}", &uuid()[..6]);
    page.locator("dialog,[data-editor-pane]")
        .get_by_label("Name")
        .fill(&folder)
        .await?;
    page.get_by_role("button", "Create folder").click().await?;
    page.get_by_role("button", &folder).click().await?;
    page.get_by_role("heading", &folder).wait_for().await?;
    page.reload().await?;
    page.get_by_role("heading", &folder).wait_for().await?;
    page.get_by_label("Choose files to upload")
        .set_input_files(vec![FilePayload::new(
            "Welcome.txt",
            "text/plain",
            "A real file uploaded through the Files app.",
        )])
        .await?;
    page.get_by_role("button", "Welcome.txt").wait_for().await?;
    page.get_by_role("button", "Dismiss finished uploads")
        .click()
        .await?;

    // A reservation held at the network while the upload is cancelled.
    let (gate, intercepted) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let (route_gate, route_intercepted) = (gate.clone(), intercepted.clone());
    page.route("**/api/v1/files/uploads", Some(1), move |route| {
        let (gate, intercepted) = (route_gate.clone(), route_intercepted.clone());
        async move {
            intercepted.notify_one();
            gate.notified().await;
            route.abort().await.ok();
            Ok(())
        }
    })
    .await?;
    page.get_by_label("Choose files to upload")
        .set_input_files(vec![FilePayload::new(
            "Cancelled.txt",
            "text/plain",
            vec![0u8; 4096],
        )])
        .await?;
    intercepted.notified().await;
    page.get_by_role("button", "Cancel upload Cancelled.txt")
        .click()
        .await?;
    page.get_by_text("Upload cancelled.").wait_for().await?;
    gate.notify_one();
    page.get_by_role("button", "Dismiss finished uploads")
        .click()
        .await?;

    ui::file_details(&page, "Welcome.txt").await?;
    page.get_by_role("heading", "Welcome.txt")
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("files-desktop.png"), true)
        .await?;
    ui::item_action(&page, "Welcome.txt", "Rename").await?;
    page.locator("dialog,[data-editor-pane]")
        .get_by_label("Name")
        .fill("A little hello.txt")
        .await?;
    page.locator("dialog,[data-editor-pane]")
        .get_by_role("button", "Save")
        .click()
        .await?;
    page.get_by_role("heading", "A little hello.txt")
        .wait_for()
        .await?;
    page.get_by_role("button", "Preview").click().await?;
    page.get_by_role("region", "Preview of A little hello.txt")
        .get_by_text("A real file uploaded through the Files app.")
        .wait_for()
        .await?;
    page.get_by_role("button", "Close preview").click().await?;
    page.wait_for_function("() => document.activeElement?.textContent?.trim() === 'Preview'")
        .await?;
    page.get_by_role("button", "Add to favorites")
        .click()
        .await?;
    page.get_by_role("button", "Remove favorite")
        .wait_for()
        .await?;
    page.get_by_role("button", "Close details").click().await?;
    page.get_by_role("button", "Favorites").click().await?;
    page.get_by_role("button", "A little hello.txt")
        .wait_for()
        .await?;
    ui::file_details(&page, "A little hello.txt").await?;
    ui::item_action(&page, "A little hello.txt", "Move to deleted files").await?;
    page.get_by_role("button", "Deleted files").click().await?;
    ui::file_details(&page, "A little hello.txt").await?;
    page.get_by_role("button", "Restore").click().await?;
    page.get_by_role("button", "All files")
        .first()
        .click()
        .await?;
    page.get_by_role("button", &folder).click().await?;
    page.get_by_role("button", "A little hello.txt")
        .wait_for()
        .await?;

    // The live connection drops while the person navigates; the app reconnects and refreshes.
    let opened = page.sockets_opened().await?;
    page.drop_sockets(4000, "Acceptance reconnect").await?;
    page.get_by_role("button", "Favorites").click().await?;
    page.wait_for_function_with(
        "(opened) => globalThis.__darSockets.opened() > opened",
        json!(opened),
    )
    .await?;
    page.wait_for_function(
        "() => !/Connecting…|Reconnecting…|Refreshing your files…/.test(document.body.textContent)",
    )
    .await?;
    page.get_by_role("button", "A little hello.txt")
        .wait_for()
        .await?;
    page.get_by_role("button", "All files")
        .first()
        .click()
        .await?;
    page.get_by_role("button", &folder).click().await?;
    page.get_by_role("button", "A little hello.txt")
        .wait_for()
        .await?;

    let state = context.storage_state().await?;
    let mobile = browser
        .new_context(ContextOptions {
            storage_state: Some(state),
            ..ContextOptions::phone(390, 844)
        })
        .await?;
    let phone = mobile.new_page().await?;
    let phone_failures = Arc::new(Mutex::new(Vec::new()));
    let sink = phone_failures.clone();
    phone.on_page_error(move |message| sink.lock().unwrap().push(message.clone()));
    phone.goto(&base).await?;
    phone
        .get_by_role("navigation", "Applications")
        .wait_for()
        .await?;
    ui::signed_in(&phone).await?;
    phone.get_by_role("button", &folder).click().await?;
    phone
        .get_by_role("button", "A little hello.txt")
        .wait_for()
        .await?;
    assert_eq!(
        phone
            .evaluate("() => document.documentElement.scrollWidth <= innerWidth")
            .await?,
        json!(true)
    );
    phone
        .screenshot(t.screenshot("files-mobile.png"), true)
        .await?;
    ui::file_details(&phone, "A little hello.txt").await?;
    phone
        .get_by_role("heading", "A little hello.txt")
        .wait_for()
        .await?;
    assert_eq!(
        phone
            .evaluate("() => document.documentElement.scrollWidth <= innerWidth")
            .await?,
        json!(true)
    );
    phone
        .screenshot(t.screenshot("files-mobile-details.png"), true)
        .await?;
    phone.get_by_role("button", "Close details").click().await?;
    mobile.close().await?;

    page.get_by_role("button", "Open account menu")
        .click()
        .await?;
    page.get_by_role("menuitem", "Sign out").click().await?;
    page.get_by_role("region", "Sign in").wait_for().await?;
    context.close().await?;
    let mut all = failures.lock().unwrap().clone();
    all.extend(phone_failures.lock().unwrap().iter().cloned());
    assert_eq!(all, Vec::<String>::new());
    Ok(())
}

/// Dropping a folder (the entries API, faked here as Chromium cannot drop real folders under
/// automation) recreates it with its subfolders and files; a name already used gets a number.
pub async fn folder_upload(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 950))
        .await?;
    let page = context.new_page().await?;
    let failures =
        page.collect_errors(|text| text.contains("401") && text.contains("Unauthorized"));
    page.goto(&base).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.get_by_role("button", "New").wait_for().await?;
    ui::signed_in(&page).await?;
    ui::open_menu_for(
        &page,
        &page.get_by_role("button", "New"),
        &page.get_by_role("menuitem", "Upload folder"),
    )
    .await?;
    page.get_by_role("menuitem", "Upload files")
        .wait_for()
        .await?;
    page.keyboard().press("Escape").await?;
    page.get_by_label("Choose a folder to upload")
        .wait_for_state(State::Attached)
        .await?;
    assert_eq!(
        page.get_by_label("Choose a folder to upload")
            .get_attribute("webkitdirectory")
            .await?,
        Some(String::new())
    );

    let name = format!("Trip {}", &uuid()[..6]);
    let drop = r#"name => {
        const file = (n, text) => ({ isDirectory: false, name: n, file: ok => ok(new File([text], n, { type: 'text/plain' })) });
        const dir = (n, children) => ({ isDirectory: true, name: n, createReader: () => { let done = false; return { readEntries: ok => { ok(done ? [] : children); done = true; } }; } });
        const tree = dir(name, [file('b.txt', 'bee'), dir('Photos', [file('a.txt', 'ay')]), dir('Empty', [])]);
        const event = new Event('drop', { bubbles: true, cancelable: true });
        Object.defineProperty(event, 'dataTransfer', { value: { items: [{ webkitGetAsEntry: () => tree }], types: ['Files'], files: [] } });
        document.querySelector('.files-main').dispatchEvent(event);
    }"#;
    page.evaluate_with(drop, json!(name)).await?;
    let children = |parent: String| {
        let admin = &admin;
        async move {
            Ok::<Value, Error>(
                admin
                    .get_json(&format!("/api/v1/files/picker?parentId={parent}&kind=file"))
                    .await?["items"]
                    .clone(),
            )
        }
    };
    let find = |items: &Value, n: &str| {
        items
            .as_array()
            .and_then(|a| a.iter().find(|i| i["name"] == n).cloned())
    };
    let top = eventually(
        std::time::Duration::from_secs(15),
        "folder uploaded",
        || async {
            let root = children(String::new()).await?;
            let Some(top) = find(&root, &name) else {
                return Ok(None);
            };
            let inside = children(top["id"].as_str().unwrap_or_default().to_owned()).await?;
            let photos = find(&inside, "Photos");
            let ready = find(&inside, "b.txt").is_some()
                && find(&inside, "Empty").is_some()
                && photos.is_some();
            Ok(ready.then(|| (top, photos.unwrap())))
        },
    )
    .await?;
    eventually(
        std::time::Duration::from_secs(15),
        "nested file uploaded",
        || async {
            let photos = children(top.1["id"].as_str().unwrap_or_default().to_owned()).await?;
            Ok(find(&photos, "a.txt").map(|_| ()))
        },
    )
    .await?;
    page.get_by_role("button", &name).wait_for().await?;
    let uploads = page.get_by_role("region", "Uploads");
    uploads
        .get_by_text(&format!("{name}/Photos/a.txt"))
        .wait_for()
        .await?;
    uploads
        .get_by_text(&format!("{name}/b.txt"))
        .wait_for()
        .await?;

    // The same folder again lands beside it under a new name.
    page.evaluate_with(drop, json!(name)).await?;
    eventually(
        std::time::Duration::from_secs(15),
        "second copy",
        || async { Ok(find(&children(String::new()).await?, &format!("{name} 2")).map(|_| ())) },
    )
    .await?;
    uploads
        .get_by_text(&format!("{name} 2/Photos/a.txt"))
        .wait_for()
        .await?;
    uploads
        .get_by_text(&format!("{name} 2/b.txt"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("files-folder-upload.png"), true)
        .await?;
    let errors = failures.lock().unwrap().clone();
    assert!(errors.is_empty(), "page errors: {errors:?}");
    Ok(())
}
