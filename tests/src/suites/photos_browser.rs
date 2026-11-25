//! Photos in the browser (`photos-browser.mjs`): library grid with thumbnails, upload into Files,
//! viewer with details/favorite/description/keyboard navigation/focus return, selection to new
//! album, settings sheet, spotlight search with thumbnails, phone layout, trash restore & Undo,
//! duplicates, Locked folder (Photos & Files) with password unlock & video playback, album
//! sharing with a person & password-protected link, public page on desktop & phone, Shared with
//! me, and Immich import progress in settings & sidebar.

use super::photos_books_apps_support::{
    create_user, ffmpeg, photos_call, photos_media_fixtures, setup_admin_as, sha1_base64,
    upload_raw,
};
use axum::Router;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use dar_acceptance::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

pub const SUITE: Suite = Suite {
    name: "photos-browser",
    cases: &[case!(photos_journey, 240)],
};

#[derive(Clone)]
struct SlowShot {
    id: String,
    name: String,
    bytes: Vec<u8>,
}

async fn start_slow_immich(
    shots: Vec<SlowShot>,
) -> Result<(u16, tokio::sync::oneshot::Sender<()>)> {
    let shots = Arc::new(shots);
    let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
        let shots = shots.clone();
        async move { handle_slow_immich(uri, headers, shots).await }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
    });
    Ok((port, tx))
}

async fn handle_slow_immich(uri: Uri, headers: HeaderMap, shots: Arc<Vec<SlowShot>>) -> Response {
    let path = uri.path();
    let json_resp = |status: StatusCode, value: Value| {
        (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            value.to_string(),
        )
            .into_response()
    };
    if path == "/api/server/ping" {
        return json_resp(StatusCode::OK, json!({"res": "pong"}));
    }
    if headers.get("x-api-key").and_then(|v| v.to_str().ok()) != Some("browser-key") {
        return json_resp(StatusCode::UNAUTHORIZED, json!({}));
    }
    if path == "/api/users/me" {
        return json_resp(
            StatusCode::OK,
            json!({"id": uuid(), "name": "Remote", "email": "remote@example.com"}),
        );
    }
    if path == "/api/search/metadata" {
        let items: Vec<Value> = shots
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "checksum": sha1_base64(&s.bytes),
                    "originalFileName": s.name,
                    "originalMimeType": "image/jpeg",
                    "fileCreatedAt": "2022-06-01T10:00:00.000Z",
                    "fileModifiedAt": "2022-06-01T10:00:00.000Z",
                    "isFavorite": false,
                    "visibility": "timeline",
                    "isTrashed": false,
                    "type": "IMAGE"
                })
            })
            .collect();
        return json_resp(
            StatusCode::OK,
            json!({"assets": {"items": items, "nextPage": null, "total": shots.len()}}),
        );
    }
    if let Some(rest) = path.strip_prefix("/api/assets/")
        && let Some(id) = rest.strip_suffix("/original")
        && !id.contains('/')
        && let Some(shot) = shots.iter().find(|s| s.id == id)
    {
        tokio::time::sleep(Duration::from_millis(1000)).await;
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "image/jpeg")],
            shot.bytes.clone(),
        )
            .into_response();
    }
    if path == "/api/albums" {
        return json_resp(StatusCode::OK, json!([]));
    }
    json_resp(StatusCode::NOT_FOUND, json!({}))
}

pub async fn photos_journey(t: &Ctx) -> Result<()> {
    // Prepare the exact isolated Photos fixture state that `photos-browser.mjs` expects.
    let root = t.temp("dar-photos-browser")?;
    let media = photos_media_fixtures(&root).await?;
    let instance =
        Instance::isolated(&root, "dar-photos-ui", InstanceConfig::default(), true).await?;
    let base = instance.base.clone();

    let result: Result<()> = async {
        setup_admin_as(&base, "Photos fixture").await?;
        let session = login(&base).await?;
        create_user(&session, "other", "Other", PASSWORD).await?;

        let target = photos_call(&session, "/upload-target", "POST", Some(&json!({"year": 2024, "month": 4}))).await?;
        let target_parent = target["parentId"].as_str().context("target parentId")?.to_owned();
        let trip = upload_raw(&session, &target_parent, "trip.jpg", "image/jpeg", &media.trip).await?;
        let clip = upload_raw(&session, &target_parent, "clip.mp4", "video/mp4", &media.clip).await?;
        let broken = upload_raw(&session, &target_parent, "broken.jpg", "image/jpeg", b"not really a jpeg").await?;
        let plain = upload_raw(&session, "", "plain.png", "image/png", &media.plain).await?;
        for id in [&trip, &clip, &broken, &plain] {
            photos_call(&session, &format!("/assets/{id}/index"), "POST", Some(&json!({}))).await?;
        }
        photos_call(&session, "/assets", "PATCH", Some(&json!({"ids": [trip, plain], "favorite": true}))).await?;
        photos_call(&session, "/assets", "PATCH", Some(&json!({"ids": [plain], "archived": true}))).await?;
        let db = session.connect().await?;
        let trash_res = reducer(|done| db.reducers.trash_entry_then(trip.clone(), 1, done)).await;
        db.disconnect();
        trash_res?;

        let browser = Browser::shared().await?;
        let context = browser.new_context(ContextOptions::sized(1440, 950)).await?;
        let page = context.new_page().await?;
        let errors = page.collect_errors(|text| text.contains("Failed to load resource"));
        let extra_errors = Arc::new(Mutex::new(Vec::<String>::new()));

        let journey: Result<()> = async {
            page.goto(&format!("{base}/photos")).await?;
            page.get_by_label("Username").fill("admin").await?;
            page.get_by_label("Password").fill(PASSWORD).await?;
            page.get_by_role("button", "Sign in").click().await?;
            page.get_by_role("heading", "Photos").wait_for().await?;

            let views = page.get_by_role("navigation", "Photos views");
            let photos_view = views.get_by_role("button", regex("^Photos"));
            assert_eq!(photos_view.get_attribute("aria-current").await?, Some("page".into()));
            let grid = page.get_by_role("list", "Photos");
            grid.get_by_role("listitem", ()).nth(1).wait_for().await?;
            assert_eq!(grid.get_by_role("listitem", ()).count().await?, 2);
            assert_eq!(page.get_by_role("link", "Photos").first().get_attribute("aria-current").await?, Some("page".into()));

            // Upload from the create button: the photo lands in Files › Photos and appears at the top.
            page.get_by_label("Upload photos and videos")
                .set_input_files(vec![FilePayload::new("plain.png", "image/png", media.plain.clone())])
                .await?;
            page.get_by_role("region", "Uploads").get_by_text("Done").wait_for().await?;
            grid.get_by_role("listitem", ()).nth(2).wait_for().await?;
            let newest = grid.get_by_role("button", regex("^Photo plain\\.png"));
            newest.wait_for().await?;
            page.wait_for_function("() => [...document.querySelectorAll('.photos-open img')].some(img => img.complete && img.naturalWidth > 0)").await?;
            page.screenshot(t.screenshot("photos-library.png"), false).await?;

            // Layout toggle (justified by default -> square -> justified), Explore, Folders and Albums hub.
            assert_eq!(page.evaluate("() => document.querySelector('.photos-grid')?.classList.contains('is-justified')").await?, json!(true));
            page.get_by_role("button", "Square grid").click().await?;
            assert_eq!(page.evaluate("() => document.querySelector('.photos-grid')?.classList.contains('is-justified')").await?, json!(false));
            page.get_by_role("button", "Justified layout").click().await?;
            assert_eq!(page.evaluate("() => document.querySelector('.photos-grid')?.classList.contains('is-justified')").await?, json!(true));

            views.get_by_role("button", regex("^Explore")).click().await?;
            page.get_by_role("heading", "Explore").wait_for().await?;
            page.get_by_role("region", "Quick collections").wait_for().await?;
            page.screenshot(t.screenshot("photos-explore.png"), false).await?;

            // All photos is every picture in Files, at ?view=library; Photos (the Photos folder) is the default.
            views.get_by_role("button", regex("^All photos")).click().await?;
            page.get_by_role("heading", "All photos").wait_for().await?;
            page.get_by_role("list", "All photos photos").get_by_role("button", regex("^Photo plain\\.png")).wait_for().await?;
            assert!(page.url().ends_with("?view=library"), "url={}", page.url());
            photos_view.click().await?;
            grid.wait_for().await?;
            assert_eq!(url::Url::parse(&page.url())?.query(), None);

            // Viewer in the middle sheet (auto-folding sidebar) with toggleable details, zoom and focus return.
            newest.click().await?;
            let viewer = page.get_by_role("region", "Viewer for plain.png");
            viewer.wait_for().await?;
            let details = page.get_by_role("complementary", "Photo details");
            details.get_by_text(contains("320 \u{00d7} 200")).wait_for().await?;
            viewer.get_by_role("button", "Zoom in").click().await?;
            viewer.get_by_text("150%").wait_for().await?;
            viewer.get_by_role("button", "Reset zoom").click().await?;
            viewer.get_by_text("Fit").wait_for().await?;
            viewer.get_by_role("button", "Toggle details").click().await?;
            assert!(!details.is_visible().await?);
            viewer.get_by_role("button", "Toggle details").click().await?;
            details.wait_for().await?;
            viewer.get_by_role("button", "Add to Favorites").click().await?;
            viewer.get_by_role("button", "Remove from Favorites").wait_for().await?;
            details.get_by_placeholder("Add a description\u{2026}").fill("Test card").await?;
            details.get_by_placeholder("Add a description\u{2026}").blur().await?;
            details.get_by_text("Saved").wait_for().await?;
            page.screenshot(t.screenshot("photos-viewer.png"), false).await?;
            page.keyboard().press("ArrowRight").await?;
            page.get_by_role("region", regex("^Viewer for (clip\\.mp4|broken\\.jpg)$")).wait_for().await?;
            page.keyboard().press("Escape").await?;
            grid.wait_for().await?;
            page.wait_for_function("() => document.activeElement?.classList.contains('photos-open')").await?;
            page.get_by_role("button", regex("^Favorites")).click().await?;
            page.get_by_role("list", "Favorites photos").get_by_role("button", regex("^Photo plain\\.png")).first().wait_for().await?;
            photos_view.click().await?;
            grid.wait_for().await?;

            // Select, then create an album from the selection and verify Albums hub + Album Hero.
            page.get_by_label("Select clip.mp4").check().await?;
            let selection = page.get_by_role("toolbar", "Selection");
            selection.get_by_text("1 selected").wait_for().await?;
            selection.get_by_role("button", "Add to album").click().await?;
            page.get_by_role("menuitem", "New album\u{2026}").click().await?;
            let dialog = page.get_by_role("dialog", "New album");
            dialog.get_by_label("Name").fill("Road trip").await?;
            dialog.get_by_role("button", "Create album").click().await?;
            views.get_by_role("button", regex("^Albums")).click().await?;
            page.get_by_role("heading", "Albums").level(1).wait_for().await?;
            page.screenshot(t.screenshot("photos-albums-hub.png"), false).await?;
            page.get_by_role("navigation", "Albums").get_by_role("button", regex("^Road trip")).click().await?;
            page.get_by_role("heading", "Road trip").wait_for().await?;
            page.get_by_role("region", "Album overview").wait_for().await?;
            page.screenshot(t.screenshot("photos-album-hero.png"), false).await?;
            page.get_by_role("list", "Road trip photos").get_by_role("button", regex("^Video clip\\.mp4")).wait_for().await?;
            assert_eq!(page.get_by_role("list", "Road trip photos").get_by_role("listitem", ()).count().await?, 1);

            // Settings sheet and the spotlight search opening a photo.
            page.get_by_role("button", "Photos settings").first().click().await?;
            let sheet = page.get_by_role("dialog", "Photos settings");
            sheet.get_by_text("Where photos come from").wait_for().await?;
            sheet.get_by_role("tab", "Immich").click().await?;
            sheet.get_by_role("heading", "Immich mobile app").wait_for().await?;
            assert_eq!(sheet.get_by_label("Server URL for the Immich app").input_value().await?, base);
            sheet.get_by_text("admin@dar").wait_for().await?;
            sheet.get_by_text("No devices yet.").wait_for().await?;
            sheet.get_by_label("Server address").fill("http://169.254.169.254").await?;
            sheet.get_by_label("API key").fill("anything").await?;
            sheet.get_by_role("button", "Connect").click().await?;
            sheet.get_by_text(regex("reserved network")).wait_for().await?;
            sheet.get_by_label("Server address").scroll_into_view_if_needed().await?;
            page.screenshot(t.screenshot("photos-immich-settings.png"), false).await?;
            sheet.get_by_role("button", "Close settings").click().await?;
            sheet.wait_for_state(State::Detached).await?;

            ui::open_search(&page).await?.fill("test card").await?;
            let result_opt = page.get_by_role("listbox", "Search results").get_by_role("option", regex("plain\\.png"));
            result_opt.wait_for().await?;
            assert!(result_opt.locator("img").count().await? > 0);
            page.screenshot(t.screenshot("photos-search.png"), false).await?;
            result_opt.click().await?;
            page.get_by_role("region", "Viewer for plain.png").wait_for().await?;

            // Phones: chips for views, the viewer above its details, no horizontal overflow.
            page.set_viewport_size(390, 844).await?;
            page.get_by_role("region", "Viewer for plain.png").wait_for().await?;
            assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
            assert!(!page.get_by_role("complementary", "Photo details").is_visible().await?);
            page.screenshot(t.screenshot("photos-phone-viewer.png"), false).await?;
            page.get_by_role("button", "Photo info").click().await?;
            let back = page.get_by_role("button", "Back to photo");
            back.wait_for().await?;
            assert_eq!(back.evaluate("(el) => el === document.activeElement", Value::Null).await?, json!(true));
            page.screenshot(t.screenshot("photos-phone-info.png"), false).await?;
            back.click().await?;
            page.get_by_role("button", "Photo info").wait_for().await?;
            assert_eq!(page.get_by_role("button", "Photo info").evaluate("(el) => el === document.activeElement", Value::Null).await?, json!(true));
            page.get_by_role("button", "Back to photos").click().await?;
            page.get_by_role("list", regex("photos$")).wait_for().await?;
            assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
            page.screenshot(t.screenshot("photos-phone.png"), false).await?;

            // Trash: the photo trashed in Files is in Photos › Trash and restores; moving to the trash offers Undo.
            page.set_viewport_size(1440, 950).await?;
            page.goto(&format!("{base}/photos?view=trash")).await?;
            let trash_grid = page.get_by_role("list", "Trash photos");
            trash_grid.get_by_role("button", regex("^Photo trip\\.jpg")).wait_for().await?;
            page.get_by_label("Select trip.jpg").check().await?;
            page.get_by_role("toolbar", "Selection").get_by_role("button", "Restore").click().await?;
            page.get_by_text("1 item restored").wait_for().await?;
            page.get_by_text("Trash is empty").wait_for().await?;
            photos_view.click().await?;
            grid.get_by_role("button", regex("^Photo trip\\.jpg")).wait_for().await?;
            page.get_by_label("Select trip.jpg").check().await?;
            page.get_by_role("toolbar", "Selection").get_by_role("button", "Move to trash").click().await?;
            page.get_by_text("1 item moved to the trash").wait_for().await?;
            assert_eq!(grid.get_by_role("button", regex("^Photo trip\\.jpg")).count().await?, 0);
            page.get_by_role("button", "Undo").click().await?;
            grid.get_by_role("button", regex("^Photo trip\\.jpg")).wait_for().await?;

            // Duplicates: plain.png was uploaded twice; keep one and trash the other.
            page.get_by_role("button", regex("^Duplicates")).click().await?;
            page.get_by_role("heading", "Duplicates").wait_for().await?;
            let group = page.get_by_role("region", regex("^2 copies of plain\\.png"));
            group.wait_for().await?;
            group.get_by_text("Identical files").wait_for().await?;
            page.screenshot(t.screenshot("photos-duplicates.png"), false).await?;
            group.get_by_role("button", "Keep 1, trash 1").click().await?;
            page.get_by_text("Kept 1, moved 1 to the trash").wait_for().await?;
            page.get_by_text("No duplicates").wait_for().await?;

            // Locked folder: moving asks for the password, then the photo leaves the library; the video plays there.
            photos_view.click().await?;
            grid.get_by_role("button", regex("^Video clip\\.mp4")).wait_for().await?;
            page.get_by_label("Select clip.mp4").check().await?;
            page.get_by_role("toolbar", "Selection").get_by_role("button", "Move to Locked folder").click().await?;
            let unlock = page.get_by_role("dialog", "Unlock the Locked folder");
            unlock.get_by_label("Password").fill("wrong password here").await?;
            unlock.get_by_role("button", "Unlock").click().await?;
            unlock.get_by_text("That password is not correct.").wait_for().await?;
            unlock.get_by_label("Password").fill(PASSWORD).await?;
            unlock.get_by_role("button", "Unlock").click().await?;
            page.get_by_text("1 item moved to the Locked folder").wait_for().await?;
            assert_eq!(grid.get_by_role("button", regex("^Video clip\\.mp4")).count().await?, 0);
            page.get_by_role("button", regex("^Locked folder")).click().await?;
            let locked_grid = page.get_by_role("list", "Locked folder photos");
            locked_grid.get_by_role("button", regex("^Video clip\\.mp4")).click().await?;
            let video = page.get_by_role("region", "Viewer for clip.mp4").locator("video");
            page.wait_for_function("() => (document.querySelector('.photos-stage video')?.readyState ?? 0) >= 1").await?;
            let video_src = video.get_attribute("src").await?.unwrap_or_default();
            assert!(regex::Regex::new(r"/api/v1/photos/assets/[a-f0-9-]+/video$").unwrap().is_match(&video_src), "src={video_src}");
            page.get_by_role("complementary", "Photo details").get_by_text(regex("In the Locked folder")).wait_for().await?;
            page.screenshot(t.screenshot("photos-locked.png"), false).await?;
            page.keyboard().press("Escape").await?;
            page.get_by_role("button", "Lock").click().await?;
            page.get_by_label("Password").wait_for().await?;
            assert_eq!(locked_grid.count().await?, 0);

            // Files: the same Locked folder, outside All files, opens with the password and deletes permanently.
            page.goto(&format!("{base}/?section=locked")).await?;
            page.get_by_role("heading", "Locked folder").level(2).wait_for().await?;
            page.get_by_label("Password").fill(PASSWORD).await?;
            page.get_by_role("button", "Unlock").click().await?;
            page.get_by_role("button", "clip.mp4").wait_for().await?;
            page.screenshot(t.screenshot("files-locked.png"), false).await?;
            page.get_by_role("button", "Actions for clip.mp4").click().await?;
            page.get_by_role("menuitem", "Delete permanently").wait_for().await?;
            assert_eq!(page.get_by_role("menuitem", "Move to deleted files").count().await?, 0);
            page.keyboard().press("Escape").await?;
            page.get_by_role("button", "Lock").click().await?;
            page.get_by_label("Password").wait_for().await?;
            assert_eq!(page.get_by_role("button", "clip.mp4").count().await?, 0);
            page.get_by_role("button", "All files").click().await?;
            page.get_by_role("heading", "All files").wait_for().await?;
            assert_eq!(page.get_by_role("button", "Locked folder").count().await?, 1);

            // Sharing: an album shared with another account and through a password-protected link.
            page.goto(&format!("{base}/photos")).await?;
            grid.get_by_role("button", regex("^Photo trip\\.jpg")).wait_for().await?;
            page.get_by_label("Select trip.jpg").check().await?;
            page.get_by_role("toolbar", "Selection").get_by_role("button", "Add to album").click().await?;
            page.get_by_role("menuitem", "New album\u{2026}").click().await?;
            let named = page.get_by_role("dialog", "New album");
            named.get_by_label("Name").fill("Shared trip").await?;
            named.get_by_role("button", "Create album").click().await?;
            page.get_by_role("navigation", "Albums").get_by_role("button", regex("^Shared trip")).click().await?;
            page.get_by_role("heading", "Shared trip").wait_for().await?;
            page.get_by_role("button", "Share").click().await?;
            let pane = page.get_by_role("complementary", "Share Shared trip");
            pane.get_by_label("Username").fill("other").await?;
            pane.get_by_role("button", "Share").click().await?;
            pane.get_by_text("@other").wait_for().await?;
            pane.get_by_role("button", "Create link").click().await?;
            let address = pane.get_by_label("Link address");
            address.wait_for().await?;
            let share_url = address.input_value().await?;
            assert!(regex::Regex::new(r"/s/[A-Za-z0-9_-]{43}$").unwrap().is_match(&share_url), "share_url={share_url}");
            pane.get_by_role("button", "Add password").click().await?;
            pane.get_by_label("Link password").fill("open sesame").await?;
            pane.get_by_role("button", "Set password").click().await?;
            pane.get_by_text(regex("Never expires \u{00b7} Password")).wait_for().await?;
            page.screenshot(t.screenshot("photos-share-pane.png"), false).await?;
            pane.get_by_role("button", "Done").click().await?;
            pane.wait_for_state(State::Detached).await?;
            assert_eq!(page.get_by_role("navigation", "Albums").get_by_role("button", regex("^Shared trip.*shared")).count().await?, 1);

            // A visitor without an account: password, grid, viewer with download.
            let guest_context = browser.new_context(ContextOptions::sized(1280, 900)).await?;
            let guest = guest_context.new_page().await?;
            let guest_errors = guest.collect_errors(|text| text.contains("Failed to load resource"));
            let guest_res: Result<()> = async {
                guest.goto(&share_url).await?;
                guest.get_by_role("heading", "Password required").wait_for().await?;
                guest.get_by_label("Password").fill("wrong one").await?;
                guest.get_by_role("button", "Open").click().await?;
                guest.get_by_text("That password is not correct.").wait_for().await?;
                guest.get_by_label("Password").fill("open sesame").await?;
                guest.get_by_role("button", "Open").click().await?;
                guest.get_by_role("heading", "Shared trip").wait_for().await?;
                guest.get_by_text(regex("Shared by Photos fixture \u{00b7} 1 item")).wait_for().await?;
                guest.wait_for_function("() => [...document.querySelectorAll('.photos-open img')].some(img => img.complete && img.naturalWidth > 0)").await?;
                guest.screenshot(t.screenshot("photos-public.png"), false).await?;
                guest.get_by_role("button", regex("^Photo trip\\.jpg")).click().await?;
                guest.get_by_role("region", "Viewer for trip.jpg").wait_for().await?;
                let dl_href = guest.get_by_role("link", "Download original").get_attribute("href").await?.unwrap_or_default();
                assert!(regex::Regex::new(r"/public/photos/.+/original$").unwrap().is_match(&dl_href), "href={dl_href}");
                guest.keyboard().press("Escape").await?;
                guest.get_by_role("list", "Shared trip photos").wait_for().await?;
                guest.set_viewport_size(390, 844).await?;
                assert_eq!(guest.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
                guest.screenshot(t.screenshot("photos-public-phone.png"), false).await?;
                Ok(())
            }
            .await;
            extra_errors.lock().unwrap().extend(guest_errors.lock().unwrap().iter().cloned());
            guest_context.close().await?;
            guest_res?;

            // The other account finds it under Shared with me, view only.
            let other_context = browser.new_context(ContextOptions::sized(1440, 950)).await?;
            let other_page = other_context.new_page().await?;
            {
                let sink = extra_errors.clone();
                other_page.on_page_error(move |msg| sink.lock().unwrap().push(msg.clone()));
            }
            let other_res: Result<()> = async {
                other_page.goto(&format!("{base}/photos")).await?;
                other_page.get_by_label("Username").fill("other").await?;
                other_page.get_by_label("Password").fill(PASSWORD).await?;
                other_page.get_by_role("button", "Sign in").click().await?;
                other_page.get_by_role("button", regex("^Shared with me")).click().await?;
                other_page.get_by_role("list", "Albums shared with you").get_by_role("button", regex("^Shared trip")).click().await?;
                other_page.get_by_text(regex("Shared by Photos fixture \u{00b7} view only")).wait_for().await?;
                other_page.get_by_role("button", regex("^Photo trip\\.jpg")).click().await?;
                let their_viewer = other_page.get_by_role("region", "Viewer for trip.jpg");
                their_viewer.wait_for().await?;
                assert_eq!(their_viewer.get_by_role("button", "Add to Favorites").count().await?, 0);
                other_page.wait_for_function("() => (document.querySelector('.photos-stage img')?.naturalWidth ?? 0) > 0").await?;
                other_page.get_by_role("complementary", "Photo details").get_by_text(regex("Shared by Photos fixture")).wait_for().await?;
                other_page.screenshot(t.screenshot("photos-shared-with-me.png"), false).await?;
                Ok(())
            }
            .await;
            other_context.close().await?;
            other_res?;

            // Immich import with live status: a slow fake Immich (each original takes a second) so the bars can be seen mid-run.
            let mut shots = Vec::new();
            for (n, source) in ["testsrc2", "mandelbrot", "smptehdbars"].iter().enumerate() {
                let file = root.join(format!("immich-{n}.jpg"));
                ffmpeg(&["-f", "lavfi", "-i", &format!("{source}=size=320x240"), "-frames:v", "1", &file.to_string_lossy()]).await?;
                shots.push(SlowShot { id: uuid(), name: format!("immich-{n}.jpg"), bytes: std::fs::read(&file)? });
            }
            let (immich_port, stop_immich) = start_slow_immich(shots).await?;
            let immich_res: Result<()> = async {
                page.goto(&format!("{base}/photos")).await?;
                page.get_by_role("button", "Photos settings").first().click().await?;
                let settings_sheet = page.get_by_role("dialog", "Photos settings");
                settings_sheet.get_by_role("tab", "Immich").click().await?;
                settings_sheet.get_by_label("Server address").fill(&format!("http://127.0.0.1:{immich_port}")).await?;
                settings_sheet.get_by_label("API key").fill("browser-key").await?;
                assert_eq!(settings_sheet.get_by_text("Direction").count().await?, 0);
                settings_sheet.get_by_role("button", "Connect").click().await?;
                settings_sheet.get_by_text("Connected to Immich. Importing has started.").wait_for().await?;
                let settings_bar = settings_sheet.get_by_role("progressbar", "Importing from Immich");
                settings_bar.wait_for().await?;
                settings_sheet.get_by_text(regex("of 3 checked")).wait_for().await?;
                settings_sheet.get_by_text(regex("^immich-\\d\\.jpg$")).wait_for().await?;
                page.screenshot(t.screenshot("photos-immich-progress.png"), false).await?;
                settings_sheet.get_by_role("button", "Close settings").click().await?;
                let sidebar_bar = page.get_by_role("button", "Immich import in progress, open Photos settings");
                sidebar_bar.wait_for().await?;
                page.screenshot(t.screenshot("photos-immich-sidebar.png"), false).await?;
                sidebar_bar.timeout(Duration::from_secs(30)).wait_for_state(State::Detached).await?;
                grid.get_by_role("button", regex("^Photo immich-2\\.jpg")).wait_for().await?;
                page.get_by_role("button", "Photos settings").first().click().await?;
                settings_sheet.get_by_role("tab", "Immich").click().await?;
                settings_sheet.get_by_text(regex("3 imported \u{00b7} 0 albums \u{00b7} 3 on Immich")).wait_for().await?;
                assert_eq!(settings_sheet.get_by_role("progressbar", ()).count().await?, 0);
                settings_sheet.get_by_role("button", "Disconnect").click().await?;
                settings_sheet.get_by_text("Disconnected. Imported photos stay in Dar.").wait_for().await?;
                settings_sheet.get_by_role("button", "Close settings").click().await?;
                Ok(())
            }
            .await;
            let _ = stop_immich.send(());
            immich_res?;
            Ok(())
        }
        .await;

        context.close().await?;
        journey?;

        let mut all = errors.lock().unwrap().clone();
        all.extend(extra_errors.lock().unwrap().iter().cloned());
        assert_eq!(all, Vec::<String>::new());
        Ok(())
    }
    .await;

    instance.close().await;
    result
}
