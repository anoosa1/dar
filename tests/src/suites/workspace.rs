//! Port of `test/workspace.integration.test.ts`: task pagination & collection scoping, notebook
//! export/import & Unicode search pagination, due-only task alarms & completion invalidation,
//! VTODO conditional DAV edits/filters/scoping/child-first import, Notes atomic revisions/fidelity/
//! move/copy/private trash/shared revocation, and Tasks hierarchy/cycles/completion/subtree copy &
//! move/CalDAV VTODO.

use super::pim_workspace_support::{
    app_password, basic_auth, dav_request, encode_uri_component, etag, parse_rfc3339_ms,
};
use chrono::{Duration as ChronoDuration, Utc};
use dar_acceptance::prelude::*;
use regex::Regex;
use std::collections::HashSet;
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "workspace",
    cases: &[
        case!(task_windows_page_and_keep_collection_scope, 45),
        case!(notebook_export_import_and_unicode_search, 45),
        case!(due_only_task_alarms_and_completion, 30),
        case!(vtodo_dav_edits_filters_scoping_and_child_first_import, 45),
        case!(notes_revisions_fidelity_move_copy_trash_and_sharing, 45),
        case!(tasks_hierarchy_cycles_completion_copy_move_and_vtodo, 45),
    ],
};

async fn api_request(
    session: &Session,
    path: &str,
    method: &str,
    body: Option<&Value>,
    if_match: Option<&str>,
) -> Result<Reply> {
    let mut req = session
        .request(method, path)
        .header("content-type", "application/json");
    if let Some(tag) = if_match {
        req = req.header("if-match", tag);
    }
    if let Some(b) = body {
        req = req.body(b.to_string());
    }
    Reply::from(req.send().await?).await
}

pub async fn task_windows_page_and_keep_collection_scope(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let session = login(&base).await?;

    let post_ok = |path: &str, body: Value| {
        let session = session.clone();
        let path = path.to_owned();
        async move {
            let r = session.post(&path, &body).await?;
            ensure!((200..300).contains(&r.status), "POST {path}: {} {}", r.status, r.text());
            r.json()
        }
    };

    let list = post_ok("/api/v1/tasks/lists", json!({"name": "Paged task window"})).await?;
    let other = post_ok("/api/v1/tasks/lists", json!({"name": "Other task scope"})).await?;
    let list_id = list["id"].as_str().context("list id")?;
    let other_id = other["id"].as_str().context("other id")?;

    let mut content = String::new();
    for i in 0..205 {
        content.push_str(&format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:page-task-{i}\r\nSUMMARY:Paged {i:03}\r\nEND:VTODO\r\nEND:VCALENDAR\r\n"
        ));
    }
    post_ok(
        &format!("/api/v1/tasks/lists/{list_id}/import"),
        json!({"content": content}),
    )
    .await?;
    let unrelated = post_ok(
        "/api/v1/tasks/items",
        json!({"collectionId": other_id, "data": {"title": "Paged outside scope"}}),
    )
    .await?;
    let unrelated_id = unrelated["id"].as_str().context("unrelated id")?;

    let endpoint = format!("/api/v1/tasks/items?view=all&sort=title&collection={list_id}");
    let first = session.get(&endpoint).await?.expect(200)?.json()?;
    let next_cursor = first["nextCursor"].as_str().context("nextCursor")?;
    let second = session
        .get(&format!(
            "{endpoint}&cursor={}",
            encode_uri_component(next_cursor)
        ))
        .await?
        .expect(200)?
        .json()?;

    let first_items = first["items"].as_array().context("first items")?;
    let second_items = second["items"].as_array().context("second items")?;
    assert_eq!(first_items.len(), 200);
    assert_eq!(second_items.len(), 5);
    assert_eq!(second["nextCursor"], "");
    let all_ids: HashSet<&str> = first_items
        .iter()
        .chain(second_items.iter())
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert_eq!(all_ids.len(), 205);
    assert_eq!(first_items[0]["data"]["title"], "Paged 000");
    assert!(!all_ids.contains(unrelated_id));
    assert_eq!(
        session
            .get(&format!("/api/v1/tasks/items/{unrelated_id}"))
            .await?
            .status,
        200
    );
    Ok(())
}

pub async fn notebook_export_import_and_unicode_search(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let session = login(&base).await?;

    let book = session
        .post("/api/v1/notes/notebooks", &json!({"name": "Notebook pagination"}))
        .await?
        .json()?;
    let book_id = book["id"].as_str().context("book id")?;

    let items: Vec<Value> = (0..205)
        .map(|i| {
            json!({
                "title": format!("Zoë {i:03}"),
                "content": format!("# Exact {i}\r\n\r\nSource with Unicode é.\n"),
                "tags": ["proof"],
                "favorite": true,
            })
        })
        .collect();

    let imported = session
        .post(
            &format!("/api/v1/notes/notebooks/{book_id}/import"),
            &json!({"items": items}),
        )
        .await?;
    assert_eq!(imported.status, 200);
    assert_eq!(
        imported.json()?["items"]
            .as_array()
            .context("imported items")?
            .iter()
            .filter(|i| !i["id"].is_null())
            .count(),
        205
    );

    let q = encode_uri_component("zoë");
    let endpoint = format!("/api/v1/notes/items?collection={book_id}&q={q}&tag=proof&view=favorites&sort=title");
    let first = session.get(&endpoint).await?.json()?;
    let next_cursor = first["nextCursor"].as_str().context("nextCursor")?;
    let second = session
        .get(&format!(
            "{endpoint}&cursor={}",
            encode_uri_component(next_cursor)
        ))
        .await?
        .json()?;

    let first_items = first["items"].as_array().context("first items")?;
    let second_items = second["items"].as_array().context("second items")?;
    assert_eq!(first_items.len(), 200);
    assert_eq!(second_items.len(), 5);
    assert_eq!(second["nextCursor"], "");
    let ids: HashSet<&str> = first_items
        .iter()
        .chain(second_items.iter())
        .filter_map(|i| i["id"].as_str())
        .collect();
    assert_eq!(ids.len(), 205);

    let exported = session
        .get(&format!("/api/v1/notes/notebooks/{book_id}/export"))
        .await?
        .json()?;
    assert_eq!(exported["format"], "dar-notebook-v1");
    let mut exported_items = exported["items"].as_array().context("exported items")?.clone();
    exported_items.sort_by(|a, b| {
        a["title"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["title"].as_str().unwrap_or_default())
    });
    let expected_items: Vec<Value> = items
        .iter()
        .map(|i| {
            json!({
                "kind": "note",
                "title": i["title"],
                "content": i["content"],
                "tags": i["tags"],
                "favorite": i["favorite"],
            })
        })
        .collect();
    assert_eq!(exported_items, expected_items);

    let target = session
        .post(
            "/api/v1/notes/notebooks",
            &json!({"name": "Notebook imported copy"}),
        )
        .await?
        .json()?;
    let target_id = target["id"].as_str().context("target id")?;
    let copied = session
        .post(
            &format!("/api/v1/notes/notebooks/{target_id}/import"),
            &exported,
        )
        .await?;
    assert_eq!(copied.status, 200);
    assert_eq!(
        copied.json()?["items"]
            .as_array()
            .context("copied items")?
            .iter()
            .filter(|i| !i["id"].is_null())
            .count(),
        205
    );
    Ok(())
}

pub async fn due_only_task_alarms_and_completion(t: &Ctx) -> Result<()> {
    let fixture = t.fixture().await?;
    let session = login(&fixture.base).await?;
    let db = fixture.database()?;

    let list = session
        .post("/api/v1/tasks/lists", &json!({"name": "Task alarm proof"}))
        .await?
        .json()?;
    let list_id = list["id"].as_str().context("list id")?;
    let due = (Utc::now() + ChronoDuration::milliseconds(3000))
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();

    let created = session
        .post(
            "/api/v1/tasks/items",
            &json!({"collectionId": list_id, "data": {"title": "Due task alarm", "due": due, "timezone": "UTC", "reminderMinutes": 0}}),
        )
        .await?;
    assert_eq!(created.status, 201, "{}", created.text());
    let created_etag = created.header("etag").context("created etag")?.to_owned();
    let task = created.json()?;
    let task_id = task["id"].as_str().context("task id")?;

    let due_ms = parse_rfc3339_ms(&format!("{due}Z"))?;
    let wait_ms = (due_ms - Utc::now().timestamp_millis() + 25).max(0) as u64;
    tokio::time::sleep(Duration::from_millis(wait_ms)).await;

    let deliver = |db: Database| async move {
        tokio::task::spawn_blocking(move || support::deliver_reminders(&db))
            .await
            .map_err(|e| Error::new(e.to_string()))?
    };
    deliver(db.clone()).await?;
    deliver(db.clone()).await?;

    let mut inbox = session.get("/api/v1/pim/reminders").await?.json()?;
    assert_eq!(
        inbox["items"]
            .as_array()
            .context("inbox items")?
            .iter()
            .filter(|i| i["resourceId"] == task_id)
            .count(),
        1
    );

    let completed = api_request(
        &session,
        &format!("/api/v1/tasks/items/{task_id}"),
        "PUT",
        Some(&json!({"data": {"status": "COMPLETED"}})),
        Some(&created_etag),
    )
    .await?;
    assert_eq!(completed.status, 200, "{}", completed.text());
    deliver(db).await?;

    inbox = session.get("/api/v1/pim/reminders").await?.json()?;
    assert!(!inbox["items"]
        .as_array()
        .context("inbox items")?
        .iter()
        .any(|i| i["resourceId"] == task_id));
    Ok(())
}

pub async fn vtodo_dav_edits_filters_scoping_and_child_first_import(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let session = login(&base).await?;

    let list = session
        .post("/api/v1/tasks/lists", &json!({"name": "DAV tasks"}))
        .await?
        .expect(201)?
        .json()?;
    let list_id = list["id"].as_str().context("list id")?;
    let password = app_password(&session, "Task DAV", "calendar:write").await?;
    let read = app_password(&session, "Task read", "calendar:read").await?;
    let files = app_password(&session, "Files isolation", "write").await?;

    let write_auth = basic_auth("admin", password["secret"].as_str().context("secret")?);
    let read_auth = basic_auth("admin", read["secret"].as_str().context("read secret")?);
    let files_auth = basic_auth("admin", files["secret"].as_str().context("files secret")?);

    let home = format!("{base}/dav/calendars/admin/{list_id}/");
    let href = format!("{home}external.ics");
    let content = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:dav-task-proof\r\nDUE:20261107T010000Z\r\nSUMMARY:External task\r\nSTATUS:NEEDS-ACTION\r\nX-PRIVATE:retain\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    let r = dav_request(
        "PUT",
        &href,
        &[("authorization", &write_auth), ("if-none-match", "*")],
        Some(content.as_bytes().to_vec()),
    )
    .await?;
    assert_eq!(r.status, 201, "{}", r.text());

    let r = dav_request("GET", &href, &[("authorization", &read_auth)], None).await?;
    assert_eq!(r.status, 200);
    let tag = r.header("etag").context("etag")?.to_owned();
    assert!(r.text().contains("X-PRIVATE:retain"));

    assert_eq!(
        dav_request(
            "PUT",
            &href,
            &[("authorization", &read_auth), ("if-match", &tag)],
            Some(content.as_bytes().to_vec()),
        )
        .await?
        .status,
        403
    );
    assert_eq!(
        dav_request("GET", &href, &[("authorization", &files_auth)], None)
            .await?
            .status,
        403
    );

    let r = dav_request(
        "PUT",
        &href,
        &[("authorization", &write_auth), ("if-match", &tag)],
        Some(content.replace("NEEDS-ACTION", "COMPLETED").into_bytes()),
    )
    .await?;
    assert_eq!(r.status, 204, "{}", r.text());

    assert_eq!(
        dav_request(
            "DELETE",
            &href,
            &[("authorization", &write_auth), ("if-match", &tag)],
            None,
        )
        .await?
        .status,
        412
    );

    let report = |body: String| {
        let home = home.clone();
        let read_auth = read_auth.clone();
        async move {
            dav_request(
                "REPORT",
                &home,
                &[
                    ("authorization", &read_auth),
                    ("content-type", "application/xml"),
                ],
                Some(body.into_bytes()),
            )
            .await
        }
    };

    let r = report(format!(
        "<c:calendar-multiget xmlns:c=\"urn:ietf:params:xml:ns:caldav\" xmlns:d=\"DAV:\"><d:prop><d:getetag/><c:calendar-data/></d:prop><d:href>/dav/calendars/admin/{list_id}/external.ics</d:href></c:calendar-multiget>"
    ))
    .await?;
    assert_eq!(r.status, 207);
    assert!(r.text().contains("STATUS:COMPLETED"));

    let query = |start: &str, end: &str| {
        format!(
            "<c:calendar-query xmlns:c=\"urn:ietf:params:xml:ns:caldav\" xmlns:d=\"DAV:\"><d:prop><c:calendar-data/></d:prop><c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VTODO\"><c:time-range start=\"{start}\" end=\"{end}\"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"
        )
    };
    let r = report(query("20261107T000000Z", "20261107T020000Z")).await?;
    assert_eq!(r.status, 207);
    assert!(r.text().contains("External task"));

    let r = report(query("20261108T000000Z", "20261108T020000Z")).await?;
    assert_eq!(r.status, 207);
    assert!(!r.text().contains("External task"));

    let vtodo_re = Regex::new(r"(?s)BEGIN:VTODO.*END:VTODO").unwrap();
    let imported_content = vtodo_re
        .replace(
            content,
            "BEGIN:VTODO\r\nUID:import-child\r\nSUMMARY:Imported child\r\nRELATED-TO;RELTYPE=PARENT:import-parent\r\nEND:VTODO\r\nBEGIN:VTODO\r\nUID:import-parent\r\nSUMMARY:Imported parent\r\nEND:VTODO",
        )
        .into_owned();
    let r = session
        .post(
            &format!("/api/v1/tasks/lists/{list_id}/import"),
            &json!({"content": imported_content}),
        )
        .await?;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()?["imported"], 2);

    let current = dav_request("GET", &href, &[("authorization", &write_auth)], None).await?;
    let current_etag = current.header("etag").context("current etag")?;
    assert_eq!(
        dav_request(
            "DELETE",
            &href,
            &[("authorization", &write_auth), ("if-match", current_etag)],
            None,
        )
        .await?
        .status,
        204
    );
    Ok(())
}

pub async fn notes_revisions_fidelity_move_copy_trash_and_sharing(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;

    async fn api(
        session: &Session,
        path: &str,
        method: &str,
        body: Option<&Value>,
        tag: Option<&str>,
    ) -> Result<Reply> {
        api_request(session, &format!("/api/v1/notes{path}"), method, body, tag).await
    }

    let r = api(&admin, "/notebooks", "POST", Some(&json!({"name": "Notes proof"})), None).await?;
    assert_eq!(r.status, 201);
    let book = r.json()?;
    let book_id = book["id"].as_str().context("book id")?;

    let other = api(
        &admin,
        "/notebooks",
        "POST",
        Some(&json!({"name": "Other notebook"})),
        None,
    )
    .await?
    .json()?;
    let other_id = other["id"].as_str().context("other id")?;

    let content = "# Exact source\r\n\r\n<script>not executed</script>\n";
    let r = api(
        &admin,
        "/items",
        "POST",
        Some(&json!({"collectionId": book_id, "title": "A note", "content": ""})),
        None,
    )
    .await?;
    assert_eq!(r.status, 201, "{}", r.text());
    let tag = r.header("etag").context("note etag")?.to_owned();
    let mut note = r.json()?;
    let uid = note["uid"].as_str().context("note uid")?.to_owned();
    let id = note["id"].as_str().context("note id")?.to_owned();

    let r = api(
        &admin,
        &format!("/items/{id}"),
        "PUT",
        Some(&json!({"title": "Edited", "content": content, "tags": ["project"], "favorite": true})),
        Some(&tag),
    )
    .await?;
    assert_eq!(r.status, 200, "{}", r.text());
    note = r.json()?;
    assert_eq!(note["uid"], uid);
    assert_eq!(note["content"], content);
    assert_eq!(
        api(&admin, &format!("/items/{id}/export"), "GET", None, None)
            .await?
            .text(),
        content
    );

    assert_eq!(
        api(
            &admin,
            &format!("/items/{id}"),
            "PUT",
            Some(&json!({"title": "lost update", "content": "wrong"})),
            Some(&tag),
        )
        .await?
        .status,
        412
    );

    let search = api(
        &admin,
        "/items?q=not%20executed&view=favorites",
        "GET",
        None,
        None,
    )
    .await?
    .json()?;
    assert!(search["items"]
        .as_array()
        .context("search items")?
        .iter()
        .any(|n| n["id"] == id.as_str()));

    let r = api(
        &admin,
        &format!("/items/{id}/copy"),
        "POST",
        Some(&json!({"collectionId": other_id})),
        Some(&etag(&note)),
    )
    .await?;
    assert_eq!(r.status, 201);
    let copy = r.json()?;
    assert_ne!(copy["id"], id);
    assert_ne!(copy["uid"], uid);

    let r = api(
        &admin,
        &format!("/items/{id}/move"),
        "POST",
        Some(&json!({"collectionId": other_id})),
        Some(&etag(&note)),
    )
    .await?;
    assert_eq!(r.status, 200, "{}", r.text());
    note = api(&admin, &format!("/items/{id}"), "GET", None, None)
        .await?
        .json()?;
    assert_eq!(note["collectionId"], other_id);
    assert_eq!(note["uid"], uid);

    let username = format!("notes-reader-{}", &uuid()[..6]);
    let r = admin
        .post(
            "/api/v1/auth/users",
            &json!({"username": username, "displayName": "Notes reader", "password": PASSWORD}),
        )
        .await?;
    assert_eq!(r.status, 201);
    let reader = login_as(&base, &username, PASSWORD).await?;

    let c = api(&admin, &format!("/notebooks/{other_id}"), "GET", None, None)
        .await?
        .json()?;
    assert_eq!(
        api(
            &admin,
            &format!("/notebooks/{other_id}/grants"),
            "POST",
            Some(&json!({"username": username, "role": "read"})),
            Some(&etag(&c)),
        )
        .await?
        .status,
        200
    );
    assert_eq!(
        api(&reader, &format!("/items/{id}"), "GET", None, None)
            .await?
            .status,
        200
    );
    assert_eq!(
        api(
            &reader,
            &format!("/items/{id}"),
            "PUT",
            Some(&json!({"title": "blocked", "content": "x"})),
            Some(&etag(&note)),
        )
        .await?
        .status,
        403
    );

    let grants = api(
        &admin,
        &format!("/notebooks/{other_id}/grants"),
        "GET",
        None,
        None,
    )
    .await?
    .json()?;
    let current_book = api(&admin, &format!("/notebooks/{other_id}"), "GET", None, None)
        .await?
        .json()?;
    assert_eq!(
        api(
            &admin,
            &format!("/notebooks/{other_id}/grants"),
            "DELETE",
            Some(&json!({"id": grants["items"][0]["id"]})),
            Some(&etag(&current_book)),
        )
        .await?
        .status,
        200
    );
    assert_eq!(
        api(&reader, &format!("/items/{id}"), "GET", None, None)
            .await?
            .status,
        404
    );

    let revoked = api(&admin, &format!("/notebooks/{other_id}"), "GET", None, None)
        .await?
        .json()?;
    assert_eq!(
        api(
            &admin,
            &format!("/notebooks/{other_id}/grants"),
            "POST",
            Some(&json!({"username": username, "role": "read"})),
            Some(&etag(&revoked)),
        )
        .await?
        .status,
        200
    );
    assert_eq!(
        api(&admin, &format!("/items/{id}"), "DELETE", None, Some(&etag(&note)))
            .await?
            .status,
        200
    );
    assert_eq!(
        api(&reader, &format!("/items/{id}"), "GET", None, None)
            .await?
            .status,
        404
    );
    assert_eq!(
        api(&reader, "/trash", "GET", None, None).await?.json()?["items"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    let r = api(
        &admin,
        &format!("/trash/{id}/restore"),
        "POST",
        Some(&json!({"collectionId": book_id})),
        Some(&etag(&note)),
    )
    .await?;
    assert_eq!(r.status, 200, "{}", r.text());
    note = api(&admin, &format!("/items/{id}"), "GET", None, None)
        .await?
        .json()?;
    assert_eq!(note["content"], content);
    assert_eq!(note["uid"], uid);
    assert_eq!(note["collectionId"], book_id);

    assert_eq!(
        api(&admin, &format!("/items/{id}"), "DELETE", None, Some(&etag(&note)))
            .await?
            .status,
        200
    );
    assert_eq!(
        api(&admin, &format!("/trash/{id}"), "DELETE", None, Some(&etag(&note)))
            .await?
            .status,
        200
    );
    assert_eq!(
        api(&admin, &format!("/trash/{id}"), "GET", None, None)
            .await?
            .status,
        404
    );
    Ok(())
}

pub async fn tasks_hierarchy_cycles_completion_copy_move_and_vtodo(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;

    async fn api(
        session: &Session,
        path: &str,
        method: &str,
        body: Option<&Value>,
        tag: Option<&str>,
    ) -> Result<Reply> {
        api_request(session, &format!("/api/v1/tasks{path}"), method, body, tag).await
    }

    let r = api(&admin, "/lists", "POST", Some(&json!({"name": "Tasks proof"})), None).await?;
    assert_eq!(r.status, 201);
    let list = r.json()?;
    let list_id = list["id"].as_str().context("list id")?;

    let dest = api(
        &admin,
        "/lists",
        "POST",
        Some(&json!({"name": "Tasks destination"})),
        None,
    )
    .await?
    .json()?;
    let dest_id = dest["id"].as_str().context("dest id")?;

    let r = api(
        &admin,
        "/items",
        "POST",
        Some(&json!({"collectionId": list_id, "data": {"title": "Parent", "start": "2026-11-06", "due": "2026-11-07", "allDay": true, "priority": 1}})),
        None,
    )
    .await?;
    assert_eq!(r.status, 201, "{}", r.text());
    let mut parent = r.json()?;
    let parent_id = parent["id"].as_str().context("parent id")?.to_owned();
    let parent_uid = parent["uid"].as_str().context("parent uid")?.to_owned();

    let r = api(
        &admin,
        "/items",
        "POST",
        Some(&json!({"collectionId": list_id, "data": {"title": "Child", "parentUid": parent_uid}})),
        None,
    )
    .await?;
    assert_eq!(r.status, 201, "{}", r.text());
    let child = r.json()?;
    let child_id = child["id"].as_str().context("child id")?.to_owned();
    let child_uid = child["uid"].as_str().context("child uid")?.to_owned();

    let mut cycle_data = parent["data"].clone();
    cycle_data["parentUid"] = json!(child_uid);
    assert_eq!(
        api(
            &admin,
            &format!("/items/{parent_id}"),
            "PUT",
            Some(&json!({"data": cycle_data})),
            Some(&etag(&parent)),
        )
        .await?
        .status,
        400
    );
    assert_eq!(
        api(
            &admin,
            &format!("/items/{parent_id}"),
            "DELETE",
            None,
            Some(&etag(&parent)),
        )
        .await?
        .status,
        409
    );

    let r = api(
        &admin,
        &format!("/items/{parent_id}/copy"),
        "POST",
        Some(&json!({"collectionId": dest_id})),
        Some(&etag(&parent)),
    )
    .await?;
    assert_eq!(r.status, 200, "{}", r.text());
    let copies = r.json()?;
    let copy_items = copies["items"].as_array().context("copy items")?;
    assert_eq!(copy_items.len(), 2);
    assert_ne!(copy_items[0]["uid"], parent_uid);
    let child_projection: Value = serde_json::from_str(
        copy_items[1]["projection"]
            .as_str()
            .context("projection str")?,
    )?;
    assert_eq!(child_projection["parentUid"], copy_items[0]["uid"]);

    let r = api(
        &admin,
        &format!("/items/{parent_id}/move"),
        "POST",
        Some(&json!({"collectionId": dest_id})),
        Some(&etag(&parent)),
    )
    .await?;
    assert_eq!(r.status, 200, "{}", r.text());
    parent = api(&admin, &format!("/items/{parent_id}"), "GET", None, None)
        .await?
        .json()?;
    assert_eq!(parent["collectionId"], dest_id);
    assert_eq!(
        api(&admin, &format!("/items/{child_id}"), "GET", None, None)
            .await?
            .json()?["collectionId"],
        dest_id
    );

    let r = api(&admin, &format!("/items/{child_id}"), "GET", None, None).await?;
    let child_etag = r.header("etag").context("child etag")?.to_owned();
    let current = r.json()?;
    let mut completed_data = current["data"].clone();
    completed_data["status"] = json!("COMPLETED");
    let r = api(
        &admin,
        &format!("/items/{child_id}"),
        "PUT",
        Some(&json!({"data": completed_data})),
        Some(&child_etag),
    )
    .await?;
    assert_eq!(r.status, 200);
    let current = r.json()?;
    assert_eq!(current["data"]["progress"], 100);
    assert!(
        !current["data"]["completedAt"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );

    let exported = api(&admin, &format!("/lists/{dest_id}/export"), "GET", None, None)
        .await?
        .text();
    assert!(exported.contains("BEGIN:VTODO"));

    let events = admin
        .get(&format!(
            "/api/v1/calendar/calendars/{dest_id}/occurrences?from=2026-11-01&to=2026-11-15&timezone=UTC"
        ))
        .await?;
    assert_eq!(events.status, 200);
    assert_eq!(events.json()?["items"], json!([]));

    let password = app_password(&admin, "VTODO proof", "calendar:write").await?;
    let auth = basic_auth("admin", password["secret"].as_str().context("secret")?);
    let query = b"<?xml version=\"1.0\"?><c:calendar-query xmlns:c=\"urn:ietf:params:xml:ns:caldav\" xmlns:d=\"DAV:\"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VTODO\"/></c:comp-filter></c:filter></c:calendar-query>";
    let r = dav_request(
        "REPORT",
        &format!("{base}/dav/calendars/admin/{dest_id}/"),
        &[
            ("authorization", &auth),
            ("content-type", "application/xml"),
        ],
        Some(query.to_vec()),
    )
    .await?;
    assert_eq!(r.status, 207, "{}", r.text());
    assert!(r.text().contains("BEGIN:VTODO"));
    Ok(())
}
