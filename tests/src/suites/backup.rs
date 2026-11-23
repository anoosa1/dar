//! Backup & restore (`backup.integration.test.ts`): consistent backup, corruption rejection,
//! empty-root restore, session/token invalidation, app settings, Photos, Notes/trash, PIM, News,
//! Identity keys/clients, file bytes export/recovery and live runtime lock check.

use super::core_support::md5_hex;
use dar_acceptance::oidc::{Authorize, Client, authorize};
use dar_acceptance::prelude::*;
use dar_acceptance::stack::free_port;

pub const SUITE: Suite = Suite {
    name: "backup",
    cases: &[case!(consistent_backup_restore_and_byte_recovery, 90)],
};

pub async fn consistent_backup_restore_and_byte_recovery(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let base = instance.base.clone();
    let source_root = instance.root().to_path_buf();
    let source = instance.database()?;

    let session = login(&base).await?;
    let identity = IdentityAdmin::new(&base).await?;
    let registered_client = identity.default_client().await?;
    let (_, provisioning_secret) = identity.credential("write").await?;
    let before_tokens = registered_client
        .exchange(&authorize(&base, &identity.session, &registered_client, Authorize::default()).await?)
        .await?;
    let before_keys: Value = session::client().get(format!("{base}/oidc/jwks")).send().await?.json().await?;

    let file = upload(&session, &format!("backup-{}.txt", uuid()), "This content must survive a full restore.").await?;

    // Notes: one kept note and one trashed note.
    let trash_book = session.post("/api/v1/notes/notebooks", &json!({ "name": "Backup trash proof" })).await?.expect(201)?.json()?;
    let book_id = trash_book["id"].as_str().context("notebook id")?.to_owned();
    let kept_note_resp = session
        .post("/api/v1/notes/items", &json!({ "collectionId": book_id, "title": "Kept note", "content": "Note body must survive restore." }))
        .await?
        .expect(201)?;
    let _kept_note = kept_note_resp.json()?;
    let trash_resp = session
        .post("/api/v1/notes/items", &json!({ "collectionId": book_id, "title": "Retained in trash", "content": "Trash must survive restore." }))
        .await?
        .expect(201)?;
    let trash_etag = trash_resp.header("etag").context("trash note etag")?.to_owned();
    let trash_note_id = trash_resp.json()?["id"].as_str().context("trash note id")?.to_owned();
    let del = Reply::from(
        session
            .request("DELETE", &format!("/api/v1/notes/items/{trash_note_id}"))
            .header("if-match", &trash_etag)
            .send()
            .await?,
    )
    .await?;
    assert_eq!(del.status, 200);

    let notes = session.get_json("/api/v1/notes/items?view=all").await?;
    let trash = session.get_json("/api/v1/notes/trash").await?;
    let note_proof = if let Some(first_id) = notes["items"][0]["id"].as_str() {
        let detail = session.get_json(&format!("/api/v1/notes/items/{first_id}")).await?;
        Some((first_id.to_owned(), detail["content"].as_str().unwrap_or_default().to_owned()))
    } else {
        None
    };
    assert!(note_proof.is_some());

    // PIM: ensure at least one contacts book + contact and one calendar + event exist so both are verified.
    let book = session
        .post("/api/v1/contacts/books", &json!({ "name": "Backup contacts", "color": "#2563eb" }))
        .await?
        .expect(201)?
        .json()?;
    let book_id = book["id"].as_str().context("contacts book id")?;
    session
        .post(
            &format!("/api/v1/contacts/books/{book_id}/items"),
            &json!({ "data": { "firstName": "Backup", "lastName": "Contact", "emails": [{ "label": "work", "value": "backup@example.test" }] } }),
        )
        .await?
        .expect(201)?;
    let cal = session
        .post("/api/v1/calendar/calendars", &json!({ "name": "Backup calendar", "color": "#2563eb" }))
        .await?
        .expect(201)?
        .json()?;
    let cal_id = cal["id"].as_str().context("calendar id")?;
    session
        .post(
            &format!("/api/v1/calendar/calendars/{cal_id}/items"),
            &json!({ "data": { "title": "Backup event", "start": "2026-11-08T10:00", "end": "2026-11-08T11:00", "timezone": "UTC" } }),
        )
        .await?
        .expect(201)?;

    let before_pim = session.get_json("/api/v1/contacts/books").await?;
    let sample_book_id = before_pim["items"][0]["id"].as_str().context("sample contacts book id")?.to_owned();
    let before_cards = session.get(&format!("/api/v1/contacts/books/{sample_book_id}/export")).await?.expect(200)?.text();
    let before_calendars = session.get_json("/api/v1/calendar/calendars").await?;
    let mut event_proof: Option<(String, String, String)> = None;
    for calendar in before_calendars["items"].as_array().context("calendars")? {
        let cid = calendar["id"].as_str().context("cal id")?;
        let rows = session.get_json(&format!("/api/v1/calendar/calendars/{cid}/items")).await?;
        if let Some(href) = rows["items"][0]["href"].as_str() {
            let event = session.get_json(&format!("/api/v1/calendar/calendars/{cid}/items/{href}")).await?;
            event_proof = Some((cid.to_owned(), href.to_owned(), event["content"].as_str().unwrap_or_default().to_owned()));
            break;
        }
    }
    assert!(event_proof.is_some());

    // News: a folder, an OPML-imported feed and a stored, starred article.
    let news_folder = session
        .post("/api/v1/news/folders", &json!({ "name": format!("Backup {}", &uuid()[..6]) }))
        .await?
        .expect(201)?
        .json()?;
    let news_folder_id = news_folder["id"].as_i64().context("news folder id")?;
    let news_folder_name = news_folder["name"].as_str().context("news folder name")?.to_owned();
    let opml = format!(
        r#"<opml version="2.0"><body><outline text="{news_folder_name}"><outline text="Backup feed" xmlUrl="http://127.0.0.1:9/backup-{}.xml"/></outline></body></opml>"#,
        uuid()
    );
    session.post("/api/v1/news/opml", &json!({ "text": opml })).await?.expect(200)?;
    let news_before = session.get_json("/api/v1/news/overview").await?;
    let backup_feed = news_before["feeds"]
        .as_array()
        .and_then(|arr| arr.iter().find(|r| r["folderId"].as_i64() == Some(news_folder_id)))
        .context("backup feed")?
        .clone();
    let backup_feed_id = backup_feed["id"].as_i64().context("backup feed id")?;
    let backup_feed_url = backup_feed["url"].as_str().context("backup feed url")?.to_owned();

    let guid = uuid();
    let now = chrono::Utc::now().timestamp_millis();
    source
        .call(
            "service_news_worker",
            json!([
                "store",
                json!({
                    "feedId": backup_feed_id,
                    "items": [{
                        "guid": guid,
                        "guidHash": md5_hex(guid.as_bytes()),
                        "url": "https://backup.example/kept",
                        "title": "Kept through restore",
                        "author": "",
                        "pubDate": now,
                        "updatedDate": now,
                        "body": "<p>Kept</p>",
                        "enclosureMime": "",
                        "enclosureLink": "",
                        "mediaThumbnail": "",
                        "mediaDescription": "",
                        "rtl": false,
                        "fingerprint": md5_hex(format!("f{guid}").as_bytes()),
                        "contentHash": md5_hex(format!("c{guid}").as_bytes()),
                        "excerpt": "Kept",
                    }]
                })
                .to_string()
            ]),
        )
        .await?;
    let news_items = session.get_json(&format!("/api/v1/news/items?type=feed&id={backup_feed_id}")).await?;
    let news_item_id = news_items["items"][0]["id"].as_i64().context("news item id")?;
    session.post("/api/v1/news/items/state", &json!({ "ids": [news_item_id], "starred": true })).await?.expect(200)?;

    // Photos: favorite, description and album survive restore.
    let picture = upload(&session, &format!("backup-{}.jpg", uuid()), "image bytes for backup").await?;
    let indexed = session.post(&format!("/api/v1/photos/assets/{}/index", picture.file_id), &json!({})).await?.expect(200)?.json()?;
    assert_eq!(indexed, json!({ "indexed": true }));
    session.patch("/api/v1/photos/assets", &json!({ "ids": [picture.file_id], "favorite": true })).await?.expect(200)?;
    let photo_revision = session.get_json(&format!("/api/v1/photos/assets/{}", picture.file_id)).await?["revision"].as_u64().context("photo revision")?;
    session
        .put(&format!("/api/v1/photos/assets/{}/description", picture.file_id), &json!({ "description": "Kept by backup", "revision": photo_revision }))
        .await?
        .expect(200)?;
    let photo_album = session
        .post("/api/v1/photos/albums", &json!({ "name": format!("Backup album {}", &uuid()[..6]), "ids": [picture.file_id] }))
        .await?
        .expect(201)?
        .json()?;

    let temporary = t.temp("dar-backup-proof")?;
    let directory = temporary.join("backup");

    let app_settings = session.get_json("/api/v1/appSettings").await?;
    let rev = app_settings["revision"].as_u64().context("appSettings revision")?;
    let set_tasks = |enabled: bool, revision: u64| {
        let session = session.clone();
        async move {
            let reply = Reply::from(
                session
                    .request("PUT", "/api/v1/admin/installedApps")
                    .header("if-match", format!("\"apps:{revision}\""))
                    .json(&json!({ "id": "tasks", "enabled": enabled, "revision": revision }))
                    .send()
                    .await?,
            )
            .await?
            .expect(200)?;
            Ok::<u64, Error>(reply.json()?["revision"].as_u64().context("new revision")?)
        }
    };
    let disabled_rev = set_tasks(false, rev).await?;
    let backup_result = support::backup(&source, &source_root, &directory);
    let _ = set_tasks(true, disabled_rev).await?;
    let backup_result = backup_result?;
    assert!(backup_result["blobs"].as_u64().unwrap_or(0) > 0);

    let exported = temporary.join("export");
    support::export_files(&source, &source_root, "admin", &exported)?;
    let manifest_raw = dar_acceptance::read_to_string(exported.join("manifest.json"))?;
    let manifest: Value = serde_json::from_str(&manifest_raw)?;
    let content_file = manifest["files"]
        .as_array()
        .and_then(|arr| arr.iter().find(|item| item["id"] == file.file_id.as_str()))
        .and_then(|item| item["contentFile"].as_str())
        .context("exported file contentFile")?;
    assert_eq!(dar_acceptance::read_to_string(exported.join(content_file))?, "This content must survive a full restore.");
    assert!(!regex::Regex::new("verifier|csrfDigest|signing-key")?.is_match(&manifest_raw));

    let saved = dar_acceptance::read_to_string(directory.join("snapshot.json"))?;
    assert!(saved.contains("dar-backup-v12"));
    assert!(!regex::Regex::new(r#""providerModels"|"provisioningTokens"|"identityProofs""#)?.is_match(&saved));

    let port = free_port()?;
    let new_origin = format!("http://127.0.0.1:{port}");
    let name = format!("dar-restore-{}", &uuid()[..8]);
    instance::publish_module(&name, &new_origin)?;
    let target = Database::new(&source.url, &name, &source.token_file)?;
    let restored_root = temporary.join("restored");

    std::fs::write(directory.join("snapshot.json"), format!("{saved} "))?;
    let corrupted_err = support::restore(&target, &directory, &restored_root).unwrap_err();
    assert!(corrupted_err.to_string().contains("corrupted"), "expected corrupted error, got: {corrupted_err}");
    std::fs::write(directory.join("snapshot.json"), &saved)?;
    support::restore(&target, &directory, &restored_root)?;

    let runtime = Instance::start(InstanceConfig {
        port,
        origin: Some(new_origin.clone()),
        root: restored_root.clone(),
        database_name: name,
        development: true,
        ..InstanceConfig::default()
    })
    .await?;

    let verify = async {
        // Old session cookie must be invalidated after restore.
        let old_check = session::client()
            .get(format!("{new_origin}/api/v1/auth/session"))
            .headers(session.headers())
            .send()
            .await?;
        assert_eq!(old_check.status().as_u16(), 401);

        let recovered = login(&new_origin).await?;
        let app_settings_after = recovered.get_json("/api/v1/appSettings").await?;
        assert!(app_settings_after["disabled"].as_array().is_some_and(|arr| arr.iter().any(|v| v == "tasks")));

        let restored_photo = recovered.get_json(&format!("/api/v1/photos/assets/{}", picture.file_id)).await?;
        assert_eq!(restored_photo["favorite"], json!(true));
        assert_eq!(restored_photo["description"], json!("Kept by backup"));
        assert_eq!(restored_photo["albums"], json!([{ "id": photo_album["id"], "name": photo_album["name"] }]));

        assert_eq!(recovered.get("/api/v1/tasks/items").await?.status, 404);

        if let Some((nid, expected_content)) = &note_proof {
            let after = recovered.get_json(&format!("/api/v1/notes/items/{nid}")).await?;
            assert_eq!(after["content"].as_str(), Some(expected_content.as_str()));
        }
        let restored_trash = recovered.get_json("/api/v1/notes/trash").await?;
        let ids = |v: &Value| {
            let mut list: Vec<String> = v["items"].as_array().map(|a| a.iter().filter_map(|r| r["id"].as_str().map(str::to_owned)).collect()).unwrap_or_default();
            list.sort();
            list
        };
        assert_eq!(ids(&restored_trash), ids(&trash));

        let after_keys: Value = session::client().get(format!("{new_origin}/oidc/jwks")).send().await?.json().await?;
        assert_eq!(after_keys, before_keys);

        assert_eq!(
            session::client().get(format!("{new_origin}/scim/v2/Users")).bearer_auth(&provisioning_secret).send().await?.status().as_u16(),
            401
        );
        assert_eq!(
            session::client().get(format!("{new_origin}/oidc/me")).bearer_auth(before_tokens.access_token()).send().await?.status().as_u16(),
            401
        );

        let registered = recovered.get_json("/api/v1/identity/clients").await?;
        assert!(registered["items"].as_array().is_some_and(|arr| arr.iter().any(|c| c["id"] == registered_client.id.as_str())));

        let after_client = Client::discover(&new_origin, &registered_client.id, registered_client.secret.as_deref(), &registered_client.redirect).await?;
        let after_tokens = after_client.exchange(&authorize(&new_origin, &recovered, &after_client, Authorize::default()).await?).await?;
        assert!(after_tokens.id_token().is_some());

        assert_eq!(recovered.get(&format!("/api/v1/files/{}/content", file.file_id)).await?.expect(200)?.text(), "This content must survive a full restore.");

        let after_pim = recovered.get_json("/api/v1/contacts/books").await?;
        assert_eq!(ids(&after_pim), ids(&before_pim));
        assert_eq!(recovered.get(&format!("/api/v1/contacts/books/{sample_book_id}/export")).await?.expect(200)?.text(), before_cards);

        if let Some((cid, href, expected_ics)) = &event_proof {
            let event = recovered.get_json(&format!("/api/v1/calendar/calendars/{cid}/items/{href}")).await?;
            assert_eq!(event["content"].as_str(), Some(expected_ics.as_str()));
        }

        let news_after = recovered.get_json("/api/v1/news/overview").await?;
        assert_eq!(
            news_after["folders"].as_array().and_then(|a| a.iter().find(|r| r["id"].as_i64() == Some(news_folder_id))).and_then(|r| r["name"].as_str()),
            Some(news_folder_name.as_str())
        );
        assert_eq!(
            news_after["feeds"].as_array().and_then(|a| a.iter().find(|r| r["id"].as_i64() == Some(backup_feed_id))).and_then(|r| r["url"].as_str()),
            Some(backup_feed_url.as_str())
        );
        assert!(news_after["starredCount"].as_u64().unwrap_or(0) >= 1);

        let kept_item = recovered.get_json(&format!("/api/v1/news/items/{news_item_id}")).await?;
        assert_eq!(kept_item["title"], json!("Kept through restore"));
        assert_eq!(kept_item["body"], json!("<p>Kept</p>"));
        assert_eq!(kept_item["starred"], json!(true));

        let later_folder = recovered
            .post("/api/v1/news/folders", &json!({ "name": format!("After {}", &uuid()[..6]) }))
            .await?
            .expect(201)?
            .json()?;
        assert!(later_folder["id"].as_i64().unwrap_or(0) > news_folder_id);

        let lock_err = support::restore(&target, &directory, &restored_root).unwrap_err();
        assert!(lock_err.to_string().contains("Another runtime"), "expected Another runtime error, got: {lock_err}");
        Ok::<(), Error>(())
    }
    .await;

    runtime.close().await;
    verify?;
    assert!(!restored_root.join("runtime.lock").exists(), "runtime.lock must be removed on close");
    Ok(())
}
