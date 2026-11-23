//! Files in the browser (`browser.mjs`): desktop and phone sign-in, upload and a cancelled
//! upload, preview, rename, favorite, trash and restore, reconnecting during navigation, and
//! signing out; no page or console errors and no horizontal overflow on the phone.

use dar_acceptance::prelude::*;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

pub const SUITE: Suite = Suite { name: "browser", cases: &[case!(files_journey, 180)] };

pub async fn files_journey(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let browser = Browser::shared().await?;
    let context = browser.new_context(ContextOptions::sized(1440, 950)).await?;
    let page = context.new_page().await?;
    page.track_sockets("/v1/database/").await?;
    let failures = page.collect_errors(|text| text.contains("401") && text.contains("Unauthorized"));
    page.goto(&base).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.screenshot(t.screenshot("auth-desktop.png"), true).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.wait_for_function("() => !document.body.textContent.includes('Connecting…')").await?;
    ui::open_menu_for(&page, &page.get_by_role("button", "New"), &page.get_by_role("menuitem", "Folder")).await?;
    page.get_by_role("menuitem", "Folder").click().await?;
    let folder = format!("Browser {}", &uuid()[..6]);
    page.locator("dialog,[data-editor-pane]").get_by_label("Name").fill(&folder).await?;
    page.get_by_role("button", "Create folder").click().await?;
    page.get_by_role("button", &folder).click().await?;
    page.get_by_role("heading", &folder).wait_for().await?;
    page.reload().await?;
    page.get_by_role("heading", &folder).wait_for().await?;
    page.get_by_label("Choose files to upload").set_input_files(vec![FilePayload::new("Welcome.txt", "text/plain", "A real file uploaded through the Files app.")]).await?;
    page.get_by_role("button", "Welcome.txt").wait_for().await?;
    page.get_by_role("button", "Dismiss finished uploads").click().await?;

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
    }).await?;
    page.get_by_label("Choose files to upload").set_input_files(vec![FilePayload::new("Cancelled.txt", "text/plain", vec![0u8; 4096])]).await?;
    intercepted.notified().await;
    page.get_by_role("button", "Cancel upload Cancelled.txt").click().await?;
    page.get_by_text("Upload cancelled.").wait_for().await?;
    gate.notify_one();
    page.get_by_role("button", "Dismiss finished uploads").click().await?;

    ui::file_details(&page, "Welcome.txt").await?;
    page.get_by_role("heading", "Welcome.txt").wait_for().await?;
    page.screenshot(t.screenshot("files-desktop.png"), true).await?;
    ui::item_action(&page, "Welcome.txt", "Rename").await?;
    page.locator("dialog,[data-editor-pane]").get_by_label("Name").fill("A little hello.txt").await?;
    page.locator("dialog,[data-editor-pane]").get_by_role("button", "Save").click().await?;
    page.get_by_role("heading", "A little hello.txt").wait_for().await?;
    page.get_by_role("button", "Preview").click().await?;
    page.get_by_role("region", "Preview of A little hello.txt").get_by_text("A real file uploaded through the Files app.").wait_for().await?;
    page.get_by_role("button", "Close preview").click().await?;
    page.wait_for_function("() => document.activeElement?.textContent?.trim() === 'Preview'").await?;
    page.get_by_role("button", "Add to favorites").click().await?;
    page.get_by_role("button", "Remove favorite").wait_for().await?;
    page.get_by_role("button", "Close details").click().await?;
    page.get_by_role("button", "Favorites").click().await?;
    page.get_by_role("button", "A little hello.txt").wait_for().await?;
    ui::file_details(&page, "A little hello.txt").await?;
    ui::item_action(&page, "A little hello.txt", "Move to deleted files").await?;
    page.get_by_role("button", "Deleted files").click().await?;
    ui::file_details(&page, "A little hello.txt").await?;
    page.get_by_role("button", "Restore").click().await?;
    page.get_by_role("button", "All files").first().click().await?;
    page.get_by_role("button", &folder).click().await?;
    page.get_by_role("button", "A little hello.txt").wait_for().await?;

    // The live connection drops while the person navigates; the app reconnects and refreshes.
    let opened = page.sockets_opened().await?;
    page.drop_sockets(4000, "Acceptance reconnect").await?;
    page.get_by_role("button", "Favorites").click().await?;
    page.wait_for_function_with("(opened) => globalThis.__darSockets.opened() > opened", json!(opened)).await?;
    page.wait_for_function("() => !/Connecting…|Reconnecting…|Refreshing your files…/.test(document.body.textContent)").await?;
    page.get_by_role("button", "A little hello.txt").wait_for().await?;
    page.get_by_role("button", "All files").first().click().await?;
    page.get_by_role("button", &folder).click().await?;
    page.get_by_role("button", "A little hello.txt").wait_for().await?;

    let state = context.storage_state().await?;
    let mobile = browser.new_context(ContextOptions { storage_state: Some(state), ..ContextOptions::phone(390, 844) }).await?;
    let phone = mobile.new_page().await?;
    let phone_failures = Arc::new(Mutex::new(Vec::new()));
    let sink = phone_failures.clone();
    phone.on_page_error(move |message| sink.lock().unwrap().push(message.clone()));
    phone.goto(&base).await?;
    phone.get_by_role("navigation", "Applications").wait_for().await?;
    phone.wait_for_function("() => !document.body.textContent.includes('Connecting…')").await?;
    phone.get_by_role("button", &folder).click().await?;
    phone.get_by_role("button", "A little hello.txt").wait_for().await?;
    assert_eq!(phone.evaluate("() => document.documentElement.scrollWidth <= innerWidth").await?, json!(true));
    phone.screenshot(t.screenshot("files-mobile.png"), true).await?;
    ui::file_details(&phone, "A little hello.txt").await?;
    phone.get_by_role("heading", "A little hello.txt").wait_for().await?;
    assert_eq!(phone.evaluate("() => document.documentElement.scrollWidth <= innerWidth").await?, json!(true));
    phone.screenshot(t.screenshot("files-mobile-details.png"), true).await?;
    phone.get_by_role("button", "Close details").click().await?;
    mobile.close().await?;

    page.get_by_role("button", "Open account menu").click().await?;
    page.get_by_role("menuitem", "Sign out").click().await?;
    page.get_by_role("region", "Sign in").wait_for().await?;
    context.close().await?;
    let mut all = failures.lock().unwrap().clone();
    all.extend(phone_failures.lock().unwrap().iter().cloned());
    assert_eq!(all, Vec::<String>::new());
    Ok(())
}
