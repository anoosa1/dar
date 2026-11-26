//! Port of `test/news.integration.test.ts`: subscribe, refresh, state, folders, OPML, proxy,
//! preferences, search, isolation and the Nextcloud News API.

use crate::suites::comms_support::FeedServer;
use dar_acceptance::prelude::*;
use std::time::{SystemTime, UNIX_EPOCH};

pub const SUITE: Suite = Suite {
    name: "news",
    cases: &[case!(news_api_and_nextcloud, 180)],
};

fn encode_uri_component(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

async fn news_call(
    session: &Session,
    method: &str,
    route: &str,
    body: Option<&Value>,
) -> Result<Value> {
    let path = format!("/api/v1/news{route}");
    let reply = match body {
        Some(b) => session.send_json(method, &path, b).await?,
        None => Reply::from(session.request(method, &path).send().await?).await?,
    };
    ensure!(
        (200..300).contains(&reply.status),
        "{method} {route}: {} {}",
        reply.status,
        reply.text()
    );
    reply.json()
}

async fn news_raw(
    session: &Session,
    method: &str,
    route: &str,
    body: Option<&Value>,
) -> Result<Reply> {
    let path = format!("/api/v1/news{route}");
    match body {
        Some(b) => session.send_json(method, &path, b).await,
        None => Reply::from(session.request(method, &path).send().await?).await,
    }
}

async fn nc_req(
    base: &str,
    secret: Option<&str>,
    method: &str,
    route: &str,
    body: Option<&Value>,
) -> Result<Reply> {
    let mut req = session::client().request(
        reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
        format!("{base}/dav/news{route}"),
    );
    if let Some(s) = secret {
        let auth = format!(
            "Basic {}",
            dar_acceptance::base64(format!("admin:{s}").as_bytes())
        );
        req = req.header("authorization", auth);
    }
    if let Some(b) = body {
        req = req.json(b);
    }
    Reply::from(req.send().await?).await
}

pub async fn news_api_and_nextcloud(t: &Ctx) -> Result<()> {
    let root = t.temp("news")?;
    let fixture = FeedServer::start().await?;
    let site = fixture.site.clone();
    let config = InstanceConfig {
        news_allow_private_networks: true,
        ..InstanceConfig::default()
    };
    let runtime = Instance::isolated(&root, "dar-news", config, true).await?;
    let base = runtime.base.clone();

    session::setup_admin(&base).await?;
    let session = login(&base).await?;

    let overview = || news_call(&session, "GET", "/overview", None);
    assert_eq!(
        overview().await?,
        json!({ "folders": [], "feeds": [], "starredCount": 0, "unreadCount": 0, "newestItemId": 0 })
    );

    // Discovery from a site's alternate link and directly from a feed.
    let found = news_call(
        &session,
        "POST",
        "/discover",
        Some(&json!({ "url": format!("{site}/") })),
    )
    .await?;
    let found_items = found["items"].as_array().context("discover items")?;
    let triples: Vec<(String, String, String)> = found_items
        .iter()
        .map(|f| {
            (
                f["url"].as_str().unwrap_or_default().into(),
                f["title"].as_str().unwrap_or_default().into(),
                f["kind"].as_str().unwrap_or_default().into(),
            )
        })
        .collect();
    assert_eq!(
        triples,
        vec![(
            format!("{site}/feed.xml"),
            "Fixture News".into(),
            "RSS".into()
        )]
    );
    assert_eq!(found_items[0]["items"][0]["title"], json!("Episode one"));
    assert_eq!(
        news_raw(
            &session,
            "POST",
            "/discover",
            Some(&json!({ "url": format!("{site}/missing") }))
        )
        .await?
        .status,
        502
    );

    let feed = news_call(
        &session,
        "POST",
        "/feeds",
        Some(&json!({ "url": format!("{site}/feed.xml"), "folderId": 0 })),
    )
    .await?;
    let feed_id = feed["id"].as_i64().context("feed id")?;
    assert_eq!(feed["title"], json!("Fixture News"));
    assert_eq!(feed["unreadCount"], json!(2));

    let duplicate = news_raw(
        &session,
        "POST",
        "/feeds",
        Some(&json!({ "url": format!("{site}/feed.xml"), "folderId": 0 })),
    )
    .await?;
    assert_eq!(duplicate.status, 409);
    assert!(
        duplicate.json()?["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("already subscribed")
    );

    let page = news_call(&session, "GET", "/items?type=unread", None).await?;
    let items = page["items"].as_array().context("unread items")?;
    let titles: Vec<&str> = items.iter().filter_map(|i| i["title"].as_str()).collect();
    assert_eq!(titles, vec!["Episode one", "Welcome to the fixture"]);
    let episode_id = items[0]["id"].as_i64().context("episode id")?;
    let welcome_id = items[1]["id"].as_i64().context("welcome id")?;
    assert_eq!(
        items[0]["enclosureLink"],
        json!("https://media.example/ep1.mp3")
    );

    let detail = news_call(&session, "GET", &format!("/items/{welcome_id}"), None).await?;
    assert_eq!(
        detail["body"],
        json!(format!(
            "<p>Hello <b>reader</b><img src=\"{site}/pixel.png\"></p>"
        ))
    );
    assert_eq!(detail["url"], json!(format!("{site}/posts/1")));

    // Read and star state, counts and mark-all-read with the newest loaded ID.
    news_call(
        &session,
        "POST",
        "/items/state",
        Some(&json!({ "ids": [welcome_id], "unread": false, "starred": true })),
    )
    .await?;
    let mut o = overview().await?;
    assert_eq!(o["unreadCount"], json!(1));
    assert_eq!(o["starredCount"], json!(1));
    assert_eq!(o["feeds"][0]["unreadCount"], json!(1));
    let starred_ids: Vec<i64> =
        news_call(&session, "GET", "/items?type=starred", None).await?["items"]
            .as_array()
            .context("starred items")?
            .iter()
            .filter_map(|i| i["id"].as_i64())
            .collect();
    assert_eq!(starred_ids, vec![welcome_id]);

    let marked = news_call(
        &session,
        "POST",
        "/read",
        Some(&json!({ "type": "all", "newestItemId": o["newestItemId"] })),
    )
    .await?;
    assert_eq!(marked["ids"], json!([episode_id]));
    assert_eq!(overview().await?["unreadCount"], json!(0));
    news_call(
        &session,
        "POST",
        "/items/state",
        Some(&json!({ "ids": marked["ids"], "unread": true })),
    )
    .await?;
    assert_eq!(overview().await?["unreadCount"], json!(1));
    news_call(
        &session,
        "POST",
        &format!("/items/{episode_id}/position"),
        Some(&json!({ "position": 93 })),
    )
    .await?;
    assert_eq!(
        news_call(&session, "GET", &format!("/items/{episode_id}"), None).await?["position"],
        json!(93)
    );

    // Refresh: a 304 keeps everything; new entries arrive unread; failures are recorded and back off.
    news_call(
        &session,
        "PATCH",
        &format!("/feeds/{feed_id}"),
        Some(&json!({ "updateNow": true })),
    )
    .await?;
    assert_eq!(fixture.state.lock().unwrap().conditional, 1);
    fixture.state.lock().unwrap().extra = true;
    let info = news_call(
        &session,
        "PATCH",
        &format!("/feeds/{feed_id}"),
        Some(&json!({ "updateNow": true })),
    )
    .await?;
    assert_eq!(info["unreadCount"], json!(2));
    assert_eq!(
        news_call(&session, "GET", "/items?type=unread", None).await?["items"][0]["title"],
        json!("Fresh arrival")
    );
    assert_eq!(
        news_call(&session, "GET", "/items?type=all", None).await?["items"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    fixture.state.lock().unwrap().failing = true;
    let info = news_call(
        &session,
        "PATCH",
        &format!("/feeds/{feed_id}"),
        Some(&json!({ "updateNow": true })),
    )
    .await?;
    assert_eq!(info["updateErrorCount"], json!(1));
    assert!(
        info["lastUpdateError"]
            .as_str()
            .unwrap_or_default()
            .contains("500")
    );
    assert!(info["nextUpdate"].as_i64().unwrap_or(0) > now_ms());
    fixture.state.lock().unwrap().failing = false;
    let info = news_call(
        &session,
        "PATCH",
        &format!("/feeds/{feed_id}"),
        Some(&json!({ "updateNow": true })),
    )
    .await?;
    assert_eq!(info["updateErrorCount"], json!(0));

    // Folders, moving, renaming and duplicate names.
    let folder = news_call(
        &session,
        "POST",
        "/folders",
        Some(&json!({ "name": "Technology" })),
    )
    .await?;
    let folder_id = folder["id"].as_i64().context("folder id")?;
    assert_eq!(
        news_raw(
            &session,
            "POST",
            "/folders",
            Some(&json!({ "name": "technology" }))
        )
        .await?
        .status,
        409
    );
    news_call(
        &session,
        "PATCH",
        &format!("/feeds/{feed_id}"),
        Some(&json!({ "folderId": folder_id, "title": "Renamed fixture" })),
    )
    .await?;
    o = overview().await?;
    assert_eq!(o["feeds"][0]["folderId"], json!(folder_id));
    assert_eq!(o["feeds"][0]["title"], json!("Renamed fixture"));
    assert_eq!(
        news_call(
            &session,
            "GET",
            &format!("/items?type=folder&id={folder_id}&getRead=1"),
            None
        )
        .await?["items"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    news_call(
        &session,
        "PATCH",
        &format!("/folders/{folder_id}"),
        Some(&json!({ "name": "Tech", "opened": false })),
    )
    .await?;
    assert_eq!(
        overview().await?["folders"],
        json!([{ "id": folder_id, "name": "Tech", "opened": false }])
    );

    // Full text replaces a teaser with the article page's main content.
    let atom = news_call(
        &session,
        "POST",
        "/feeds",
        Some(
            &json!({ "url": format!("{site}/atom.xml"), "folderId": folder_id, "fullText": true }),
        ),
    )
    .await?;
    let atom_id = atom["id"].as_i64().context("atom id")?;
    let long_id = news_call(
        &session,
        "GET",
        &format!("/items?type=feed&id={atom_id}"),
        None,
    )
    .await?["items"][0]["id"]
        .as_i64()
        .context("long id")?;
    let long_detail = news_call(&session, "GET", &format!("/items/{long_id}"), None).await?;
    let long_body = long_detail["body"].as_str().unwrap_or_default();
    assert!(
        long_body.starts_with("<h1>Long read</h1><p>Full article paragraph 0"),
        "unexpected full text body: {long_body}"
    );
    assert!(!long_body.contains("Menu item"));

    // On-demand full-text fetch extracts and returns sanitized article HTML for a single item.
    let extracted = news_call(
        &session,
        "POST",
        &format!("/items/{long_id}/fulltext"),
        None,
    )
    .await?;
    assert_eq!(extracted["id"], json!(long_id));
    let extracted_body = extracted["body"].as_str().unwrap_or_default();
    assert!(
        extracted_body.starts_with("<h1>Long read</h1><p>Full article paragraph 0"),
        "unexpected on-demand full text body: {extracted_body}"
    );
    assert!(!extracted_body.contains("Menu item"));

    // OPML round trip.
    let opml = news_raw(&session, "GET", "/opml", None).await?;
    assert_eq!(
        opml.header("content-disposition"),
        Some("attachment; filename=\"subscriptions.opml\"")
    );
    let exported = opml.text();
    assert!(
        exported.contains(
            "<outline text=\"Tech\" title=\"Tech\"><outline type=\"rss\" text=\"Renamed fixture\""
        ),
        "unexpected OPML: {exported}"
    );
    let opml_input = format!(
        r#"<opml version="2.0"><body><outline text="Blogs"><outline text="Atom again" xmlUrl="{site}/atom.xml"/><outline text="Elsewhere" xmlUrl="{site}/elsewhere.xml"/></outline></body></opml>"#
    );
    let imported = news_call(
        &session,
        "POST",
        "/opml",
        Some(&json!({ "text": opml_input })),
    )
    .await?;
    assert_eq!(imported, json!({ "added": 1, "skipped": 1, "folders": 1 }));

    // The image proxy serves only raster images, inert.
    let image = news_raw(
        &session,
        "GET",
        &format!(
            "/image?url={}",
            encode_uri_component(&format!("{site}/pixel.png"))
        ),
        None,
    )
    .await?;
    assert_eq!(image.status, 200);
    assert_eq!(image.header("content-type"), Some("image/png"));
    assert_eq!(
        image.header("content-security-policy"),
        Some("default-src 'none'; sandbox")
    );
    assert_eq!(
        news_raw(
            &session,
            "GET",
            &format!(
                "/image?url={}",
                encode_uri_component(&format!("{site}/vector.svg"))
            ),
            None
        )
        .await?
        .status,
        404
    );

    // Preferences are validated; search finds articles.
    let prefs = session.get_json("/api/v1/pim/preferences").await?;
    assert_eq!(
        prefs
            .get("newsRefresh")
            .and_then(Value::as_i64)
            .unwrap_or(30),
        30
    );
    let rev = prefs["revision"].as_i64().context("prefs revision")?;
    let if_match = format!("\"preferences:{rev}\"");
    let bad_pref = Reply::from(
        session
            .request("PUT", "/api/v1/pim/preferences")
            .header("if-match", &if_match)
            .json(&json!({ "newsRefresh": 7 }))
            .send()
            .await?,
    )
    .await?;
    assert_eq!(bad_pref.status, 400);
    let ok_pref = Reply::from(
        session
            .request("PUT", "/api/v1/pim/preferences")
            .header("if-match", &if_match)
            .json(&json!({
                "newsRefresh": 60,
                "newsMarkRead": "scroll",
                "newsLayout": "cards",
                "newsPaper": "sepia",
                "newsFont": "sans",
                "newsSize": "lg"
            }))
            .send()
            .await?,
    )
    .await?;
    assert_eq!(ok_pref.status, 200);
    let saved_prefs = ok_pref.json()?;
    assert_eq!(saved_prefs["newsLayout"], json!("cards"));
    assert_eq!(saved_prefs["newsPaper"], json!("sepia"));
    assert_eq!(saved_prefs["newsFont"], json!("sans"));
    assert_eq!(saved_prefs["newsSize"], json!("lg"));
    let search = session.get_json("/api/v1/search?q=fresh").await?;
    let news_hits: Vec<&str> = search["items"]
        .as_array()
        .context("search items")?
        .iter()
        .filter(|i| i["group"] == json!("News"))
        .filter_map(|i| i["title"].as_str())
        .collect();
    assert_eq!(news_hits, vec!["Fresh arrival"]);

    // Nextcloud News API with app passwords.
    let make_pw = |scope: &'static str| {
        let session = session.clone();
        async move {
            session
                .post(
                    "/api/v1/auth/app-passwords",
                    &json!({ "label": "Phone", "scope": scope }),
                )
                .await?
                .json()
        }
    };
    let writer = make_pw("v2 news:read news:write").await?;
    let reader = make_pw("v2 news:read").await?;
    let files_only = make_pw("v2 files:read").await?;
    let writer_secret = writer["secret"].as_str().context("writer secret")?;
    let reader_secret = reader["secret"].as_str().context("reader secret")?;
    let files_secret = files_only["secret"].as_str().context("files secret")?;
    assert_eq!(writer["newsUrl"], json!(format!("{base}/dav/news")));

    let api = "/index.php/apps/news/api/v1-3";
    assert_eq!(
        nc_req(&base, None, "GET", "/status.php", None)
            .await?
            .json()?["productname"],
        json!("Nextcloud")
    );
    let unauth = nc_req(&base, None, "GET", &format!("{api}/feeds"), None).await?;
    assert_eq!(unauth.status, 401);
    assert!(
        unauth
            .header("www-authenticate")
            .unwrap_or_default()
            .contains("Basic")
    );
    assert_eq!(
        nc_req(
            &base,
            Some(files_secret),
            "GET",
            &format!("{api}/feeds"),
            None
        )
        .await?
        .status,
        403
    );
    assert_eq!(
        nc_req(
            &base,
            Some(reader_secret),
            "GET",
            &format!("{api}/version"),
            None
        )
        .await?
        .json()?,
        json!({ "version": "24.0.0" })
    );
    let user = nc_req(
        &base,
        Some(reader_secret),
        "GET",
        "/ocs/v2.php/cloud/user?format=json",
        None,
    )
    .await?
    .json()?;
    assert_eq!(user["ocs"]["data"]["id"], json!("admin"));

    let nc_feeds = nc_req(
        &base,
        Some(reader_secret),
        "GET",
        &format!("{api}/feeds"),
        None,
    )
    .await?
    .json()?;
    let nc_feed_list = nc_feeds["feeds"].as_array().context("nc feeds")?;
    assert_eq!(nc_feed_list.len(), 3);
    assert_eq!(nc_feeds["starredCount"], json!(1));
    assert!(nc_feeds["newestItemId"].as_i64().unwrap_or(0) > 0);
    assert!(
        nc_feed_list
            .iter()
            .all(|f| f["added"].as_i64().unwrap_or(i64::MAX) < 100_000_000_000)
    );
    assert!(
        nc_feed_list
            .iter()
            .all(|f| f["folderId"].as_i64().is_some_and(|id| id > 0))
    );

    let nc_folders = nc_req(
        &base,
        Some(reader_secret),
        "GET",
        &format!("{api}/folders"),
        None,
    )
    .await?
    .json()?;
    let mut nc_folder_names: Vec<&str> = nc_folders["folders"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    nc_folder_names.sort();
    assert_eq!(nc_folder_names, vec!["Blogs", "Tech"]);

    let nc_items = nc_req(
        &base,
        Some(reader_secret),
        "GET",
        &format!("{api}/items?type=3&batchSize=2&getRead=true"),
        None,
    )
    .await?
    .json()?;
    let nc_item_list = nc_items["items"].as_array().context("nc items")?;
    assert_eq!(nc_item_list.len(), 2);
    let id0 = nc_item_list[0]["id"].as_i64().unwrap();
    let id1 = nc_item_list[1]["id"].as_i64().unwrap();
    assert!(id0 > id1);
    assert!(
        nc_item_list.iter().all(
            |i| i["pubDate"].as_i64().unwrap_or(i64::MAX) < 100_000_000_000
                && i["lastModified"].as_i64().unwrap_or(i64::MAX) < 100_000_000_000
        )
    );

    let older = nc_req(
        &base,
        Some(reader_secret),
        "GET",
        &format!("{api}/items?type=3&batchSize=10&offset={id1}"),
        None,
    )
    .await?
    .json()?;
    let older_list = older["items"].as_array().context("older items")?;
    assert!(!older_list.is_empty() && older_list.iter().all(|i| i["id"].as_i64().unwrap() < id1));

    assert_eq!(
        nc_req(
            &base,
            Some(reader_secret),
            "POST",
            &format!("{api}/items/{id0}/read"),
            None
        )
        .await?
        .status,
        403
    );
    let since = now_secs() - 1;
    let item_ids: Vec<i64> = nc_item_list
        .iter()
        .filter_map(|i| i["id"].as_i64())
        .collect();
    assert_eq!(
        nc_req(
            &base,
            Some(writer_secret),
            "POST",
            &format!("{api}/items/read/multiple"),
            Some(&json!({ "itemIds": item_ids }))
        )
        .await?
        .status,
        200
    );
    let target_feed_id = nc_item_list[0]["feedId"].as_i64().unwrap();
    let target_guid_hash = nc_item_list[0]["guidHash"].as_str().unwrap();
    assert_eq!(
        nc_req(
            &base,
            Some(writer_secret),
            "PUT",
            &format!(
                "/index.php/apps/news/api/v1-2/items/{target_feed_id}/{target_guid_hash}/star"
            ),
            None
        )
        .await?
        .status,
        200
    );
    let updated = nc_req(
        &base,
        Some(reader_secret),
        "GET",
        &format!("{api}/items/updated?type=3&lastModified={since}"),
        None,
    )
    .await?
    .json()?;
    assert!(
        updated["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"].as_i64() == Some(id0)
                && i["unread"] == json!(false)
                && i["starred"] == json!(true))
    );

    assert_eq!(
        nc_req(
            &base,
            Some(writer_secret),
            "POST",
            &format!("{api}/folders"),
            Some(&json!({ "name": "Tech" }))
        )
        .await?
        .status,
        409
    );
    let created = nc_req(
        &base,
        Some(writer_secret),
        "POST",
        &format!("{api}/folders"),
        Some(&json!({ "name": "Podcasts" })),
    )
    .await?
    .json()?;
    let podcasts_id = created["folders"][0]["id"]
        .as_i64()
        .context("podcasts id")?;
    assert_eq!(
        nc_req(
            &base,
            Some(writer_secret),
            "POST",
            &format!("{api}/feeds/{feed_id}/move"),
            Some(&json!({ "folderId": podcasts_id }))
        )
        .await?
        .status,
        200
    );
    assert_eq!(
        nc_req(
            &base,
            Some(writer_secret),
            "POST",
            &format!("{api}/feeds/{feed_id}/rename"),
            Some(&json!({ "feedTitle": "Via app" }))
        )
        .await?
        .status,
        200
    );
    o = overview().await?;
    let moved_feed = o["feeds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"].as_i64() == Some(feed_id))
        .context("feed in overview")?;
    assert_eq!(moved_feed["title"], json!("Via app"));
    assert_eq!(moved_feed["folderId"], json!(podcasts_id));
    assert_eq!(
        nc_req(
            &base,
            Some(writer_secret),
            "POST",
            &format!("{api}/feeds"),
            Some(&json!({ "url": format!("{site}/feed.xml"), "folderId": 0 }))
        )
        .await?
        .status,
        409
    );
    assert_eq!(
        nc_req(
            &base,
            Some(writer_secret),
            "POST",
            &format!("{api}/items/read"),
            Some(&json!({ "newestItemId": o["newestItemId"] }))
        )
        .await?
        .status,
        200
    );
    assert_eq!(overview().await?["unreadCount"], json!(0));

    // Another account cannot reach these feeds; disabling News hides both APIs.
    session
        .post(
            "/api/v1/auth/users",
            &json!({ "username": "other", "displayName": "Other", "password": PASSWORD }),
        )
        .await?
        .expect(201)?;
    let other = login_as(&base, "other", PASSWORD).await?;
    assert_eq!(
        news_raw(&other, "GET", &format!("/feeds/{feed_id}"), None)
            .await?
            .status,
        404
    );
    assert_eq!(
        news_raw(&other, "GET", &format!("/items/{welcome_id}"), None)
            .await?
            .status,
        404
    );
    assert_eq!(
        news_raw(&other, "POST", &format!("/items/{long_id}/fulltext"), None)
            .await?
            .status,
        404
    );
    assert_eq!(
        news_call(
            &other,
            "POST",
            "/items/state",
            Some(&json!({ "ids": [welcome_id], "unread": true }))
        )
        .await?["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    let apps = session.get_json("/api/v1/appSettings").await?;
    let apps_rev = apps["revision"].as_i64().context("apps rev")?;
    let dis = Reply::from(
        session
            .request("PUT", "/api/v1/admin/installedApps")
            .header("if-match", format!("\"apps:{apps_rev}\""))
            .json(&json!({ "id": "news", "enabled": false, "revision": apps_rev }))
            .send()
            .await?,
    )
    .await?;
    assert_eq!(dis.status, 200);
    assert_eq!(
        news_raw(&session, "GET", "/overview", None).await?.status,
        404
    );
    assert_eq!(
        nc_req(
            &base,
            Some(reader_secret),
            "GET",
            &format!("{api}/feeds"),
            None
        )
        .await?
        .status,
        404
    );
    let enabled = session.get_json("/api/v1/appSettings").await?;
    let en_rev = enabled["revision"].as_i64().context("enabled rev")?;
    let re_en = Reply::from(
        session
            .request("PUT", "/api/v1/admin/installedApps")
            .header("if-match", format!("\"apps:{en_rev}\""))
            .json(&json!({ "id": "news", "enabled": true, "revision": en_rev }))
            .send()
            .await?,
    )
    .await?;
    assert_eq!(re_en.status, 200);

    // Unsubscribing removes the feed's articles.
    news_call(&session, "DELETE", &format!("/feeds/{atom_id}"), None).await?;
    assert_eq!(
        news_raw(&session, "GET", &format!("/items/{long_id}"), None)
            .await?
            .status,
        404
    );

    runtime.close().await;
    fixture.close();
    Ok(())
}
