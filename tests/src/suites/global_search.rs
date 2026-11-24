//! Global search (`global-search.integration.test.ts`): results across apps, private and
//! granted scopes, revocation, text found inside notes and text files, and bounded results.

use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;

pub const SUITE: Suite = Suite {
    name: "global-search",
    cases: &[case!(spans_apps_scopes_and_bounds, 60)],
};

async fn search(session: &Session, query: &str) -> Result<Value> {
    session.get_json(&format!("/api/v1/search?q={query}")).await
}

fn items(result: &Value) -> Vec<Value> {
    result["items"].as_array().cloned().unwrap_or_default()
}

pub async fn spans_apps_scopes_and_bounds(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let marker = format!("search-{}", &uuid()[..8]);
    let other_name = format!("search-owner-{}", &uuid()[..6]);
    let post = |session: &Session, url: &str, value: Value| {
        let (session, url) = (session.clone(), url.to_owned());
        async move { session.post(&url, &value).await?.expect(201)?.json() }
    };
    post(&admin, "/api/v1/auth/users", json!({"username": other_name, "displayName": "Private search owner", "password": PASSWORD})).await?;
    let other = login_as(&base, &other_name, PASSWORD).await?;
    upload(&admin, &format!("{marker} plan.txt"), "A file").await?;
    upload(&admin, &format!(".{marker} hidden.txt"), "Hidden").await?;
    // Notes are Markdown files in the Notes folder (Files › docs/notes), found by name.
    let notes = admin.get_json("/api/v1/notes/library").await?;
    let notes_root = notes["rootId"].as_str().context("notes root")?.to_owned();
    let in_notes = Upload {
        parent_id: &notes_root,
        ..Upload::default()
    };
    let note = upload_with(
        &admin,
        &format!("{marker} body match.md"),
        "# Body match\n",
        in_notes.clone(),
    )
    .await?;
    let calendar = post(
        &admin,
        "/api/v1/calendar/calendars",
        json!({"name": "Search calendar", "color": "#1e66f5"}),
    )
    .await?;
    let calendar_id = calendar["id"].as_str().context("calendar id")?;
    post(&admin, &format!("/api/v1/calendar/calendars/{calendar_id}/items"), json!({"data": {"title": format!("{marker} event"), "start": "2026-11-08T10:00", "end": "2026-11-08T11:00", "timezone": "UTC", "allDay": false}})).await?;
    post(&admin, "/api/v1/tasks/items", json!({"collectionId": calendar_id, "data": {"title": format!("{marker} task"), "status": "NEEDS-ACTION", "allDay": true, "start": "", "due": ""}})).await?;
    let book = post(
        &admin,
        "/api/v1/contacts/books",
        json!({"name": "Search book", "color": "#1e66f5"}),
    )
    .await?;
    post(&admin, &format!("/api/v1/contacts/books/{}/items", book["id"].as_str().unwrap_or_default()), json!({"content": format!("BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{}\r\nFN:{marker} Contact\r\nEND:VCARD\r\n", uuid())})).await?;
    let key = format!("search-{}", uuid());
    let stored = admin
        .request(
            "PUT",
            &format!("/api/v1/apps/org.dar.prayer-times/userData/{key}"),
        )
        .header("content-type", "application/json")
        .header("if-match", "\"0\"")
        .body(json!({"schemaVersion": 1, "value": {"label": marker}}).to_string())
        .send()
        .await?;
    assert_eq!(stored.status().as_u16(), 200);
    let result = items(&search(&admin, &marker).await?);
    for group in [
        "Files",
        "Contacts",
        "Calendar",
        "Notes",
        "Tasks",
        "Applications",
    ] {
        assert!(result.iter().any(|item| item["group"] == group), "{group}");
    }
    assert!(result.iter().any(
        |item| item["href"] == format!("/notes?note={}", note.file_id) && item["group"] == "Notes"
    ));
    assert!(
        !result
            .iter()
            .any(|item| item["title"].as_str().unwrap_or_default().starts_with('.'))
    );
    assert_eq!(items(&search(&other, &marker).await?), Vec::<Value>::new());
    // Notes and text files are also found by their text (indexed in the background), with an
    // excerpt; binary content is not.
    let inside = format!("inside-{}", &uuid()[..8]);
    let text_note = upload_with(
        &admin,
        "Weekly review.md",
        format!("# Weekly review\n\nIdeas for the {inside} garden.\n"),
        in_notes.clone(),
    )
    .await?;
    let text_file = upload(&admin, "shopping.txt", format!("eggs, {inside} flour")).await?;
    upload_with(
        &admin,
        "photo.png",
        inside.clone(),
        Upload {
            media_type: "image/png",
            ..Upload::default()
        },
    )
    .await?;
    upload(
        &admin,
        "blob.txt",
        [inside.as_bytes(), b"\0\x01\x02"].concat(),
    )
    .await?;
    let mut inner = items(&search(&admin, &inside).await?);
    for _ in 0..100 {
        if inner.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        inner = items(&search(&admin, &inside).await?);
    }
    let mut found: Vec<(String, String)> = inner
        .iter()
        .map(|item| {
            (
                item["group"].as_str().unwrap_or_default().to_owned(),
                item["href"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    found.sort();
    let mut expected = vec![
        (
            "Files".to_owned(),
            format!("/?folder=&file={}", text_file.file_id),
        ),
        (
            "Notes".to_owned(),
            format!("/notes?note={}", text_note.file_id),
        ),
    ];
    expected.sort();
    assert_eq!(found, expected);
    let excerpt = inner
        .iter()
        .find(|item| item["group"] == "Notes")
        .and_then(|item| item["detail"].as_str())
        .unwrap_or_default()
        .to_owned();
    assert!(
        excerpt.contains(&format!("Ideas for the {inside} garden")),
        "{excerpt}"
    );
    assert_eq!(
        items(&search(&other, &inside).await?),
        Vec::<Value>::new(),
        "text stays private"
    );
    let owner = other.connect().await?;
    let (folder, grant) = (uuid(), uuid());
    reducer(|done| {
        owner.reducers.create_folder_then(
            folder.clone(),
            String::new(),
            format!("{marker} shared"),
            done,
        )
    })
    .await?;
    upload_with(
        &other,
        &format!("{marker} nested.txt"),
        "Shared file",
        Upload {
            parent_id: &folder,
            ..Upload::default()
        },
    )
    .await?;
    let details =
        procedure(|done| owner.procedures.file_details_then(folder.clone(), done)).await?;
    let revision = details["entry"]["revision"].as_u64().context("revision")? as u32;
    reducer(|done| {
        owner.reducers.grant_access_then(
            grant.clone(),
            folder.clone(),
            "admin".into(),
            "read".into(),
            revision,
            done,
        )
    })
    .await?;
    let nested = format!("{marker} nested.txt");
    assert!(
        items(&search(&admin, &marker).await?)
            .iter()
            .any(|item| item["title"] == nested.as_str())
    );
    reducer(|done| owner.reducers.revoke_grant_then(grant.clone(), done)).await?;
    assert!(
        !items(&search(&admin, &marker).await?)
            .iter()
            .any(|item| item["title"] == nested.as_str())
    );
    drop(owner);
    for i in 0..10 {
        upload_with(
            &admin,
            &format!("{marker} note {i}.md"),
            "",
            in_notes.clone(),
        )
        .await?;
    }
    let limited = search(&admin, &marker).await?;
    assert_eq!(
        items(&limited)
            .iter()
            .filter(|item| item["group"] == "Notes")
            .count(),
        8
    );
    assert_eq!(limited["truncated"], json!(true));
    assert_eq!(
        session::client()
            .get(format!("{base}/api/v1/search?q=test"))
            .send()
            .await?
            .status()
            .as_u16(),
        401
    );
    assert_eq!(
        admin
            .get(&format!("/api/v1/search?q={}", "x".repeat(201)))
            .await?
            .status,
        400
    );
    Ok(())
}
