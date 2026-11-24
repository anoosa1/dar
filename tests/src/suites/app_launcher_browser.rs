//! App launcher & installed apps browser journey (`app-launcher-browser.mjs`): desktop rail and
//! phone bottom bar across themes/viewports, keyboard navigation, protected core apps, persisted
//! disabling, API/search enforcement, stale/unauthorized writes and non-admin phone menu.

use dar_acceptance::browser::Cookie;
use dar_acceptance::prelude::*;

pub const SUITE: Suite = Suite {
    name: "app-launcher-browser",
    cases: &[case!(app_launcher_and_installed_apps, 120)],
};

pub async fn app_launcher_and_installed_apps(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let endpoint = "/api/v1/admin/installedApps";

    let read = || {
        let admin = admin.clone();
        async move { admin.get_json(endpoint).await }
    };
    let change = |id: &str, enabled: bool, revision: u64| {
        let admin = admin.clone();
        let id = id.to_owned();
        async move {
            Reply::from(
                admin
                    .request("PUT", endpoint)
                    .header("if-match", format!("\"apps:{revision}\""))
                    .json(&json!({ "id": id, "enabled": enabled, "revision": revision }))
                    .send()
                    .await?,
            )
            .await
        }
    };

    let initial = read().await?;
    let external = initial["items"]
        .as_array()
        .and_then(|arr| {
            arr.iter().find(|app| {
                app["version"].as_str().is_some_and(|v| !v.is_empty())
                    && app["canToggle"] == json!(true)
            })
        })
        .context("external toggleable app")?
        .clone();
    let ext_id = external["id"].as_str().context("external id")?.to_owned();
    let ext_name = external["name"]
        .as_str()
        .context("external name")?
        .to_owned();

    let marker = format!("launcher-{}", &uuid()[..8]);
    let data_url = format!("/api/v1/apps/{ext_id}/userData/{marker}");
    let put_data = Reply::from(
        admin
            .request("PUT", &data_url)
            .header("if-match", "\"0\"")
            .json(&json!({ "schemaVersion": 1, "value": { "marker": marker } }))
            .send()
            .await?,
    )
    .await?;
    assert_eq!(put_data.status, 200);

    let browser = Browser::shared().await?;
    let context = browser.new_context_sized(1440, 950).await?;

    let result = async {
        let page = context.new_page().await?;
        let errors = page.collect_errors(|text| text.contains("401") && text.contains("Unauthorized"));

        page.goto(&format!("{base}/settings?section=installedApps")).await?;
        page.get_by_label("Username").fill("admin").await?;
        page.get_by_label("Password").fill(PASSWORD).await?;
        page.get_by_role("button", "Sign in").click().await?;
        page.get_by_role("heading", "Apps").wait_for().await?;

        for (id, name) in [("files", "Files"), ("contacts", "Contacts"), ("calendar", "Calendar")] {
            let row = page.get_by_role("region", name);
            assert_eq!(row.get_by_role("button", ()).count().await?, 0);
            assert_eq!(row.inner_text().await?.trim(), name);
            let rev = read().await?["revision"].as_u64().context("rev")?;
            assert_eq!(change(id, false, rev).await?.status, 403);
        }

        for theme in ["latte", "mocha"] {
            page.evaluate_with("(value) => { document.documentElement.dataset.theme = value; }", json!(theme)).await?;
            for (width, height) in [(1440u32, 950u32), (390, 568)] {
                page.set_viewport_size(width, height).await?;
                let rail = page.get_by_role("navigation", "Applications");
                let links = rail.locator(".app-rail-item").filter_visible(true);
                rail.wait_for().await?;
                let boxes = links
                    .evaluate_all("items => items.map(item => { const r = item.getBoundingClientRect(); return { x: r.x, y: r.y, bottom: r.bottom }; })", Value::Null)
                    .await?;
                let arr = boxes.as_array().context("boxes")?;
                if width == 390 {
                    assert!(arr.len() <= 4, "{boxes}");
                    let y0 = arr[0]["y"].as_f64().unwrap();
                    for i in 0..arr.len() {
                        assert!(arr[i]["bottom"].as_f64().unwrap() <= height as f64);
                        if i > 0 {
                            assert!(arr[i]["x"].as_f64().unwrap() > arr[i - 1]["x"].as_f64().unwrap());
                            assert!((arr[i]["y"].as_f64().unwrap() - y0).abs() < 1.0);
                        }
                    }
                } else {
                    for name in ["Files", "Contacts", "Calendar", "Notes", "Tasks", ext_name.as_str(), "Settings"] {
                        rail.get_by_role("link", name).wait_for().await?;
                    }
                    let x0 = arr[0]["x"].as_f64().unwrap();
                    for i in 1..arr.len() {
                        assert!(arr[i]["y"].as_f64().unwrap() > arr[i - 1]["y"].as_f64().unwrap());
                        assert!((arr[i]["x"].as_f64().unwrap() - x0).abs() < 1.0);
                    }
                }
                assert_eq!(rail.get_by_role("link", "Settings").filter_visible(true).count().await?, if width == 390 { 0 } else { 1 });

                if width == 390 {
                    let opener = page.get_by_role("button", "More apps");
                    opener.click().await?;
                    let menu = page.get_by_role("menu", "More apps");
                    menu.wait_for().await?;
                    let box_ = menu.bounding_box().await?.context("more apps menu box")?;
                    assert!(
                        box_.x >= 12.0
                            && box_.x + box_.width <= width as f64 - 12.0
                            && box_.y >= 0.0
                            && box_.y + box_.height <= height as f64 - 12.0,
                        "{box_:?}"
                    );
                    menu.get_by_role("menuitem", "Settings").wait_for().await?;
                    page.keyboard().press("End").await?;
                    assert_eq!(menu.get_by_role("menuitem", "Manage apps").evaluate("el => el === document.activeElement", Value::Null).await?, json!(true));
                    page.keyboard().press("Escape").await?;
                    assert_eq!(opener.evaluate("el => el === document.activeElement", Value::Null).await?, json!(true));
                    opener.click().await?;
                    menu.wait_for().await?;
                }
                assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
                page.screenshot(t.screenshot(&format!("app-launcher-{theme}-{width}.png")), false).await?;
                page.keyboard().press("Escape").await?;
            }
        }

        page.set_viewport_size(1440, 950).await?;
        let stale = read().await?["revision"].as_u64().context("stale revision")?;
        for name in ["Notes", "Tasks", ext_name.as_str()] {
            page.get_by_role("button", format!("Disable {name}")).click().await?;
            page.get_by_role("button", format!("Enable {name}")).wait_for().await?;
        }
        assert_eq!(change("notes", true, stale).await?.status, 409);

        page.reload().await?;
        page.get_by_role("button", "Enable Notes").wait_for().await?;
        let rail = page.get_by_role("navigation", "Applications");
        rail.get_by_role("link", "Files").wait_for().await?;
        for name in ["Notes", "Tasks", ext_name.as_str()] {
            rail.get_by_role("link", name).wait_for_state(State::Detached).await?;
        }

        for route in ["/api/v1/notes/notebooks", "/api/v1/tasks/items", data_url.as_str()] {
            assert_eq!(admin.get(route).await?.status, 404, "{route}");
        }
        for q in ["Notes", "Tasks", ext_name.as_str()] {
            let encoded: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
            let r = admin.get_json(&format!("/api/v1/search?q={encoded}")).await?;
            let blocked = ["/notes".to_owned(), "/tasks".to_owned(), format!("/apps/{ext_id}")];
            assert!(!r["items"].as_array().is_some_and(|arr| arr.iter().any(|item| item["href"].as_str().is_some_and(|h| blocked.iter().any(|b| b == h)))));
        }

        page.goto(&format!("{base}/notes")).await?;
        page.get_by_role("heading", "App disabled").wait_for().await?;

        let username = format!("launcher-{}", &uuid()[..6]);
        admin
            .post("/api/v1/auth/users", &json!({ "username": username, "displayName": "Launcher user", "password": PASSWORD }))
            .await?
            .expect(201)?;
        let user = login_as(&base, &username, PASSWORD).await?;
        let rev = read().await?["revision"].as_u64().context("rev")?;
        assert_eq!(user.put(endpoint, &json!({ "id": "notes", "enabled": true, "revision": rev })).await?.status, 403);

        let user_ctx = browser.new_context_sized(390, 568).await?;
        let user_check = async {
            let (cookie_name, cookie_val) = user.cookie.split_once('=').context("user cookie")?;
            user_ctx.add_cookies(vec![Cookie::for_url(cookie_name, cookie_val, &base)]).await?;
            let user_page = user_ctx.new_page().await?;
            user_page.goto(&base).await?;
            assert_eq!(
                user_page.get_by_role("navigation", "Applications").get_by_role("link", "Files").get_attribute("aria-current").await?.as_deref(),
                Some("page")
            );
            user_page.get_by_role("button", "More apps").click().await?;
            let user_menu = user_page.get_by_role("menu", "More apps");
            user_menu.wait_for().await?;
            user_menu.get_by_role("menuitem", "Settings").wait_for().await?;
            assert_eq!(user_menu.get_by_role("menuitem", "Manage apps").count().await?, 0);
            Ok::<(), Error>(())
        }
        .await;
        user_ctx.close().await.ok();
        user_check?;

        let collected = errors.lock().unwrap().clone();
        assert!(collected.is_empty(), "browser errors: {collected:?}");
        Ok::<(), Error>(())
    }
    .await;

    // Re-enable apps and verify persisted userData, then clean up.
    for id in ["notes", "tasks", ext_id.as_str()] {
        let rev = read().await?["revision"].as_u64().context("rev")?;
        assert_eq!(change(id, true, rev).await?.status, 200);
    }
    assert_eq!(
        admin.get_json(&data_url).await?["value"],
        json!({ "marker": marker })
    );
    let _ = Reply::from(
        admin
            .request("DELETE", &data_url)
            .header("if-match", "\"1\"")
            .send()
            .await?,
    )
    .await;

    context.close().await.ok();
    result
}
