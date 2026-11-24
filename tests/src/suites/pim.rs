//! Port of `test/pim.integration.test.ts`: Contacts CRUD, imports, private isolation, live
//! sharing revocation and revision conflicts; DAV discovery, conditional native/DAV round trip,
//! namespace filters and credential scoping; Calendar recurrence and durable reminders; Unicode
//! pagination; fair reminder batching.

use super::pim_workspace_support::{
    basic_auth, dav_request, encode_uri_component, etag, json_request, parse_rfc3339_ms,
};
use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};
use dar_acceptance::prelude::*;
use regex::Regex;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "pim",
    cases: &[
        case!(contacts_crud_isolation_and_conflicts, 45),
        case!(dav_discovery_round_trip_and_scoping, 45),
        case!(calendar_recurrence_and_reminders, 45),
        case!(contacts_pagination_unicode, 45),
        case!(reminder_batches_advance_fairly, 45),
    ],
};

pub async fn contacts_crud_isolation_and_conflicts(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let user = format!("pim-{}", &uuid()[..8]);
    json_request(
        &admin,
        "/api/v1/auth/users",
        "POST",
        Some(&json!({"username": user, "displayName": "Other user", "password": "a private contacts password"})),
        None,
        201,
    )
    .await?;
    let other = login_as(&base, &user, "a private contacts password").await?;
    let live = other.connect().await?;
    live.subscribe(&["SELECT * FROM my_pim_changes"]).await?;

    let mut book = json_request(
        &admin,
        "/api/v1/contacts/books",
        "POST",
        Some(&json!({"name": "Personal", "color": "#2563eb"})),
        None,
        201,
    )
    .await?
    .json()?;
    let book_id = book["id"].as_str().context("book id")?.to_owned();
    let path = format!("/api/v1/contacts/books/{book_id}");

    let created = json_request(
        &admin,
        &format!("{path}/items"),
        "POST",
        Some(&json!({"data": {"firstName": "Amira", "lastName": "Lee", "emails": [{"label": "work", "value": "amira@example.com"}], "groups": ["Design team"]}})),
        None,
        201,
    )
    .await?
    .json()?;
    let created_href = created["href"].as_str().context("created href")?.to_owned();

    json_request(&other, &format!("{path}/items"), "GET", None, None, 404).await?;

    let changed = json_request(
        &admin,
        &format!("{path}/items/{created_href}"),
        "PUT",
        Some(&json!({"data": {"displayName": "Amira Lee", "favorite": true}})),
        Some(&etag(&created)),
        200,
    )
    .await?
    .json()?;
    let changed_href = changed["href"].as_str().context("changed href")?.to_owned();

    json_request(
        &admin,
        &format!("{path}/items/{created_href}"),
        "PUT",
        Some(&json!({"data": {"displayName": "Stale"}})),
        Some(&etag(&created)),
        412,
    )
    .await?;
    json_request(
        &admin,
        &format!("{path}/items/{created_href}"),
        "DELETE",
        None,
        None,
        428,
    )
    .await?;

    book = json_request(&admin, &path, "GET", None, None, 200)
        .await?
        .json()?;
    json_request(
        &admin,
        &format!("{path}/grants"),
        "POST",
        Some(&json!({"username": user, "role": "read"})),
        Some(&etag(&book)),
        200,
    )
    .await?;

    let other_items = json_request(&other, &format!("{path}/items"), "GET", None, None, 200)
        .await?
        .json()?;
    assert_eq!(other_items["items"].as_array().map(Vec::len), Some(1));

    json_request(
        &other,
        &format!("{path}/items/{changed_href}"),
        "PUT",
        Some(&json!({"data": {"displayName": "No access"}})),
        Some(&etag(&changed)),
        403,
    )
    .await?;

    let grants = json_request(&admin, &format!("{path}/grants"), "GET", None, None, 200)
        .await?
        .json()?;
    book = json_request(&admin, &path, "GET", None, None, 200)
        .await?
        .json()?;

    // A view row has no primary key: a change arrives as a delete and an insert.
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let tx = Mutex::new(Some(tx));
    live.db.my_pim_changes().on_insert(move |_, _| {
        if let Some(sender) = tx.lock().unwrap().take() {
            let _ = sender.send(());
        }
    });
    json_request(
        &admin,
        &format!("{path}/grants"),
        "DELETE",
        Some(&json!({"id": grants["items"][0]["id"]})),
        Some(&etag(&book)),
        200,
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .context("timed out waiting for live PIM change notification")?
        .ok();

    json_request(&other, &format!("{path}/items"), "GET", None, None, 404).await?;

    let card = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:external-1\r\nFN:External Contact\r\nX-KEEP:unknown\r\nEND:VCARD\r\n";
    let imported = json_request(
        &admin,
        &format!("{path}/import"),
        "POST",
        Some(&json!({"content": format!("{card}{card}")})),
        None,
        200,
    )
    .await?
    .json()?;
    assert_eq!(imported["imported"], 1);
    let items = imported["items"].as_array().context("import items")?;
    assert_eq!(items.len(), 2);
    let errors: Vec<&str> = items
        .iter()
        .filter_map(|item| item["error"].as_str())
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one conflict error in import result, got: {imported}"
    );
    assert!(
        errors[0].contains("conflict"),
        "expected conflict error, got: {}",
        errors[0]
    );

    let exported = json_request(&admin, &format!("{path}/export"), "GET", None, None, 200)
        .await?
        .text();
    assert!(exported.contains("X-KEEP:unknown"), "exported: {exported}");

    json_request(
        &admin,
        &format!("{path}/items/{changed_href}"),
        "DELETE",
        None,
        Some(&etag(&changed)),
        200,
    )
    .await?;
    json_request(
        &admin,
        &format!("{path}/items/{changed_href}"),
        "GET",
        None,
        None,
        404,
    )
    .await?;

    live.disconnect();
    Ok(())
}

pub async fn dav_discovery_round_trip_and_scoping(t: &Ctx) -> Result<()> {
    let origin = t.fixture().await?.base.clone();
    let session = login(&origin).await?;

    for kind in ["contacts", "calendar"] {
        let base = format!(
            "/api/v1/{kind}/{}",
            if kind == "contacts" {
                "books"
            } else {
                "calendars"
            }
        );
        let collection = json_request(
            &session,
            &base,
            "POST",
            Some(&json!({"name": format!("DAV {kind}"), "color": "#2563eb"})),
            None,
            201,
        )
        .await?
        .json()?;
        let collection_id = collection["id"].as_str().context("collection id")?;
        let password = json_request(
            &session,
            "/api/v1/auth/app-passwords",
            "POST",
            Some(&json!({"label": format!("DAV {kind}"), "scope": format!("{kind}:write")})),
            None,
            201,
        )
        .await?
        .json()?;
        let auth = basic_auth("admin", password["secret"].as_str().context("secret")?);
        let dav = format!(
            "{}{}/",
            password["davUrl"].as_str().context("davUrl")?,
            collection_id
        );
        let href = if kind == "contacts" {
            "test.vcf"
        } else {
            "test.ics"
        };
        let content = if kind == "contacts" {
            "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:dav-contact\r\nFN:DAV Contact\r\nEND:VCARD\r\n"
        } else {
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nBEGIN:VEVENT\r\nUID:dav-event\r\nDTSTART:20261109T090000Z\r\nDTEND:20261109T100000Z\r\nSUMMARY:DAV Event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        };

        let discovery = dav_request(
            "PROPFIND",
            &format!("{origin}/dav/"),
            &[("authorization", &auth), ("depth", "0")],
            Some(
                b"<d:propfind xmlns:d=\"DAV:\"><d:prop><d:current-user-principal/></d:prop></d:propfind>"
                    .to_vec(),
            ),
        )
        .await?;
        assert_eq!(discovery.status, 207);
        assert!(discovery.text().contains("principals/admin"));

        let item_url = format!("{dav}{href}");
        let put = dav_request(
            "PUT",
            &item_url,
            &[("authorization", &auth), ("if-none-match", "*")],
            Some(content.as_bytes().to_vec()),
        )
        .await?;
        assert_eq!(put.status, 201, "PUT {item_url}: {}", put.text());
        let tag = put.header("etag").context("missing ETag")?.to_owned();

        let unconditional = dav_request(
            "PUT",
            &item_url,
            &[("authorization", &auth)],
            Some(content.as_bytes().to_vec()),
        )
        .await?;
        assert_eq!(unconditional.status, 428);

        let native = json_request(
            &session,
            &format!("{base}/{collection_id}/items/{href}"),
            "GET",
            None,
            None,
            200,
        )
        .await?
        .json()?;
        assert_eq!(etag(&native), tag);

        let (ns, report, data) = if kind == "contacts" {
            ("carddav", "addressbook-multiget", "address-data")
        } else {
            ("caldav", "calendar-multiget", "calendar-data")
        };
        let dav_path = url::Url::parse(&dav)?.path().to_owned();
        let multiget_body = format!(
            "<q:{report} xmlns:q=\"urn:ietf:params:xml:ns:{ns}\" xmlns:d=\"DAV:\"><d:prop><d:getetag/><q:{data}/></d:prop><d:href>{dav_path}{href}</d:href></q:{report}>"
        );
        let result = dav_request(
            "REPORT",
            &dav,
            &[("authorization", &auth)],
            Some(multiget_body.into_bytes()),
        )
        .await?;
        assert_eq!(result.status, 207);
        let result_text = result.text();
        assert!(
            Regex::new(r"DAV (Contact|Event)")
                .unwrap()
                .is_match(&result_text),
            "unexpected multiget body: {result_text}"
        );

        let filter = if kind == "contacts" {
            "<q:filter><q:prop-filter name=\"FN\"><q:text-match collation=\"i;unicode-casemap\">contact</q:text-match></q:prop-filter></q:filter>"
        } else {
            "<q:filter><q:comp-filter name=\"VCALENDAR\"><q:comp-filter name=\"VEVENT\"><q:time-range start=\"20261109T000000Z\" end=\"20261110T000000Z\"/></q:comp-filter></q:comp-filter></q:filter>"
        };
        let query_tag = if kind == "contacts" {
            "addressbook-query"
        } else {
            "calendar-query"
        };
        let query_body = format!(
            "<q:{query_tag} xmlns:q=\"urn:ietf:params:xml:ns:{ns}\" xmlns:d=\"DAV:\"><d:prop><d:getetag/></d:prop>{filter}</q:{query_tag}>"
        );
        let query = dav_request(
            "REPORT",
            &dav,
            &[("authorization", &auth)],
            Some(query_body.into_bytes()),
        )
        .await?;
        assert_eq!(query.status, 207);
        assert!(query.text().contains(href));

        let forbidden = dav_request(
            "PROPFIND",
            &format!("{origin}/dav/files/admin/"),
            &[("authorization", &auth), ("depth", "0")],
            None,
        )
        .await?;
        assert_eq!(forbidden.status, 403);

        let deleted = dav_request(
            "DELETE",
            &item_url,
            &[("authorization", &auth), ("if-match", &tag)],
            None,
        )
        .await?;
        assert_eq!(deleted.status, 204);

        json_request(
            &session,
            &format!(
                "/api/v1/auth/app-passwords/{}",
                password["id"].as_str().context("password id")?
            ),
            "DELETE",
            None,
            None,
            200,
        )
        .await?;
        let revoked = dav_request(
            "PROPFIND",
            &dav,
            &[("authorization", &auth), ("depth", "0")],
            None,
        )
        .await?;
        assert_eq!(revoked.status, 401);
    }
    Ok(())
}

pub async fn calendar_recurrence_and_reminders(t: &Ctx) -> Result<()> {
    let fixture = t.fixture().await?;
    let session = login(&fixture.base).await?;
    let db = fixture.database()?;

    let collection = json_request(
        &session,
        "/api/v1/calendar/calendars",
        "POST",
        Some(&json!({"name": "Personal calendar", "color": "#24804b"})),
        None,
        201,
    )
    .await?
    .json()?;
    let base = format!(
        "/api/v1/calendar/calendars/{}",
        collection["id"].as_str().context("collection id")?
    );

    let now = Utc::now();
    let start = (now + ChronoDuration::milliseconds(3_603_000))
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();
    let end = (now + ChronoDuration::milliseconds(7_203_000))
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();

    let event = json_request(
        &session,
        &format!("{base}/items"),
        "POST",
        Some(&json!({"data": {"title": "Due reminder", "start": start, "end": end, "timezone": "UTC", "recurrence": "FREQ=DAILY;COUNT=3", "reminderMinutes": 60}})),
        None,
        201,
    )
    .await?
    .json()?;
    let event_id = event["id"].as_str().context("event id")?;
    let event_href = event["href"].as_str().context("event href")?;

    let from = (Utc::now() - ChronoDuration::days(1)).to_rfc3339_opts(SecondsFormat::Millis, true);
    let to = (Utc::now() + ChronoDuration::days(4)).to_rfc3339_opts(SecondsFormat::Millis, true);
    let list = json_request(
        &session,
        &format!("{base}/occurrences?from={from}&to={to}&timezone=UTC"),
        "GET",
        None,
        None,
        200,
    )
    .await?
    .json()?;
    assert_eq!(list["items"].as_array().map(Vec::len), Some(3));

    let pin = uuid();
    let snapshot = db.call("service_backup", json!([pin])).await?;
    assert!(
        snapshot["pim"]["resources"]
            .as_array()
            .context("backup resources")?
            .iter()
            .any(|row| row["id"] == event_id)
    );
    db.call("service_release_backup", json!([pin])).await?;

    // Wait for the actual due boundary, rather than an arbitrary polling sleep.
    let due_at_ms = parse_rfc3339_ms(&format!("{start}Z"))? - 3_600_000;
    let wait_ms = (due_at_ms - Utc::now().timestamp_millis() + 25).max(0) as u64;
    tokio::time::sleep(Duration::from_millis(wait_ms)).await;

    let deliver = |db: Database| async move {
        tokio::task::spawn_blocking(move || support::deliver_reminders(&db))
            .await
            .map_err(|e| Error::new(e.to_string()))?
    };
    deliver(db.clone()).await?;

    let mut due = json_request(&session, "/api/v1/pim/reminders", "GET", None, None, 200)
        .await?
        .json()?;
    let reminder_id = due["items"]
        .as_array()
        .context("reminders array")?
        .iter()
        .find(|row| row["resourceId"] == event_id)
        .and_then(|row| row["id"].as_str())
        .context("expected due reminder")?
        .to_owned();

    deliver(db.clone()).await?;
    due = json_request(&session, "/api/v1/pim/reminders", "GET", None, None, 200)
        .await?
        .json()?;
    let matching = due["items"]
        .as_array()
        .context("reminders array")?
        .iter()
        .filter(|row| row["resourceId"] == event_id)
        .count();
    assert_eq!(matching, 1);

    json_request(
        &session,
        &format!(
            "/api/v1/pim/reminders/{}",
            encode_uri_component(&reminder_id)
        ),
        "DELETE",
        None,
        None,
        200,
    )
    .await?;
    due = json_request(&session, "/api/v1/pim/reminders", "GET", None, None, 200)
        .await?
        .json()?;
    assert!(
        !due["items"]
            .as_array()
            .context("reminders array")?
            .iter()
            .any(|row| row["resourceId"] == event_id)
    );

    json_request(
        &session,
        &format!("{base}/items/{event_href}"),
        "PUT",
        Some(&json!({"data": {"title": "Updated reminder", "start": start, "end": end, "timezone": "UTC", "recurrence": "FREQ=DAILY;COUNT=3", "reminderMinutes": null}})),
        Some(&etag(&event)),
        200,
    )
    .await?;
    deliver(db).await?;
    let inbox = json_request(&session, "/api/v1/pim/reminders", "GET", None, None, 200)
        .await?
        .json()?;
    assert!(
        !inbox["items"]
            .as_array()
            .context("inbox array")?
            .iter()
            .any(|row| row["resourceId"] == event_id)
    );
    Ok(())
}

pub async fn contacts_pagination_unicode(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let session = login(&base).await?;
    let book = json_request(
        &session,
        "/api/v1/contacts/books",
        "POST",
        Some(&json!({"name": "Pagination contacts", "color": "#2563eb"})),
        None,
        201,
    )
    .await?
    .json()?;
    let book_id = book["id"].as_str().context("book id")?;

    let mut content = String::new();
    for i in 0..250 {
        content.push_str(&format!(
            "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:page-{i}\r\nFN:Zoë {i:03}\r\nCATEGORIES:Friends\r\nEND:VCARD\r\n"
        ));
    }
    let imported = json_request(
        &session,
        &format!("/api/v1/contacts/books/{book_id}/import"),
        "POST",
        Some(&json!({"content": content})),
        None,
        200,
    )
    .await?
    .json()?;
    assert_eq!(imported["imported"], 250);

    let q = encode_uri_component("zoë");
    let first = json_request(
        &session,
        &format!("/api/v1/contacts/people?book={book_id}&q={q}&group=Friends"),
        "GET",
        None,
        None,
        200,
    )
    .await?
    .json()?;
    let first_items = first["items"].as_array().context("first items")?;
    assert_eq!(first_items.len(), 200);
    let next_cursor = first["nextCursor"].as_str().unwrap_or_default();
    assert!(!next_cursor.is_empty());

    let second = json_request(
        &session,
        &format!(
            "/api/v1/contacts/people?book={book_id}&q={q}&group=Friends&cursor={}",
            encode_uri_component(next_cursor)
        ),
        "GET",
        None,
        None,
        200,
    )
    .await?
    .json()?;
    let second_items = second["items"].as_array().context("second items")?;
    assert_eq!(second_items.len(), 50);
    assert_eq!(second["nextCursor"], "");

    let ids: HashSet<&str> = first_items
        .iter()
        .chain(second_items.iter())
        .filter_map(|row| row["id"].as_str())
        .collect();
    assert_eq!(ids.len(), 250);
    Ok(())
}

pub async fn reminder_batches_advance_fairly(t: &Ctx) -> Result<()> {
    let fixture = t.fixture().await?;
    let session = login(&fixture.base).await?;
    let db = fixture.database()?;

    let calendar = json_request(
        &session,
        "/api/v1/calendar/calendars",
        "POST",
        Some(&json!({"name": "Reminder batching", "color": "#2563eb"})),
        None,
        201,
    )
    .await?
    .json()?;
    let calendar_id = calendar["id"].as_str().context("calendar id")?;

    let mut content =
        String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Batch proof//EN\r\n");
    for i in 0..205 {
        content.push_str(&format!(
            "BEGIN:VEVENT\r\nUID:batch-{i}\r\nDTSTART:20261109T090000Z\r\nDTEND:20261109T100000Z\r\nSUMMARY:Batch {i}\r\nEND:VEVENT\r\n"
        ));
    }
    content.push_str("END:VCALENDAR\r\n");

    let imported = json_request(
        &session,
        &format!("/api/v1/calendar/calendars/{calendar_id}/import"),
        "POST",
        Some(&json!({"content": content})),
        None,
        200,
    )
    .await?
    .json()?;
    assert_eq!(imported["imported"], 205);

    let last_href = imported["items"]
        .as_array()
        .and_then(|items| items.last())
        .and_then(|item| item["href"].as_str())
        .context("last imported href")?;
    let last = json_request(
        &session,
        &format!("/api/v1/calendar/calendars/{calendar_id}/items/{last_href}"),
        "GET",
        None,
        None,
        200,
    )
    .await?
    .json()?;
    let modified_ms = parse_rfc3339_ms(last["modifiedAt"].as_str().context("modifiedAt")?)?;
    let wait_ms = (modified_ms + 1005 - Utc::now().timestamp_millis()).max(0) as u64;
    tokio::time::sleep(Duration::from_millis(wait_ms)).await;

    let first = db.call("service_pim_reminder_work", json!([])).await?;
    let first_jobs = first.as_array().context("first reminder work")?;
    assert_eq!(first_jobs.len(), 200);

    let db_clone = db.clone();
    tokio::task::spawn_blocking(move || support::deliver_reminders(&db_clone))
        .await
        .map_err(|e| Error::new(e.to_string()))??;

    let next = db.call("service_pim_reminder_work", json!([])).await?;
    let next_jobs = next.as_array().context("next reminder work")?;
    assert!(
        next_jobs
            .iter()
            .any(|job| job["resource"]["collectionId"] == calendar_id)
    );

    let first_ids: HashSet<&str> = first_jobs
        .iter()
        .filter_map(|job| job["resource"]["id"].as_str())
        .collect();
    assert!(!next_jobs.iter().any(|job| {
        job["resource"]["id"]
            .as_str()
            .is_some_and(|id| first_ids.contains(id))
    }));

    let current = json_request(
        &session,
        &format!("/api/v1/calendar/calendars/{calendar_id}"),
        "GET",
        None,
        None,
        200,
    )
    .await?
    .json()?;
    json_request(
        &session,
        &format!("/api/v1/calendar/calendars/{calendar_id}"),
        "DELETE",
        None,
        Some(&etag(&current)),
        200,
    )
    .await?;
    Ok(())
}
