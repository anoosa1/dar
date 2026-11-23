//! Port of `test/calendar-subscriptions.integration.test.ts`: calendar subscriptions sync by
//! link, stay read-only and show dated tasks; Notes live in Files (`docs/notes`).

use axum::http::{StatusCode, header};
use axum::Router;
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "calendar-subscriptions",
    cases: &[case!(subscriptions_dated_tasks_and_notes_library, 120)],
};

fn feed(with_retro: bool) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//Test//EN",
        "BEGIN:VTIMEZONE",
        "TZID:Europe/Berlin",
        "BEGIN:STANDARD",
        "DTSTART:19701025T030000",
        "TZOFFSETFROM:+0200",
        "TZOFFSETTO:+0100",
        "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU",
        "END:STANDARD",
        "END:VTIMEZONE",
        "BEGIN:VEVENT",
        "UID:offsite@test",
        "DTSTAMP:20261101T000000Z",
        "DTSTART;VALUE=DATE:20261117",
        "DTEND;VALUE=DATE:20261118",
        "SUMMARY:Company offsite",
        "END:VEVENT",
        "BEGIN:VEVENT",
        "UID:standup@test",
        "DTSTAMP:20261101T000000Z",
        "DTSTART;TZID=Europe/Berlin:20261116T100000",
        "DTEND;TZID=Europe/Berlin:20261116T103000",
        "RRULE:FREQ=DAILY;COUNT=3",
        "SUMMARY:Standup",
        "END:VEVENT",
        "BEGIN:VEVENT",
        "UID:standup@test",
        "DTSTAMP:20261101T000000Z",
        "RECURRENCE-ID;TZID=Europe/Berlin:20261117T100000",
        "DTSTART;TZID=Europe/Berlin:20261117T110000",
        "DTEND;TZID=Europe/Berlin:20261117T113000",
        "SUMMARY:Standup (moved)",
        "END:VEVENT",
        "BEGIN:VEVENT",
        "DTSTAMP:20261101T000000Z",
        "DTSTART:20261118T090000Z",
        "SUMMARY:No UID",
        "END:VEVENT",
    ];
    if with_retro {
        lines.extend([
            "BEGIN:VEVENT",
            "UID:retro@test",
            "DTSTAMP:20261101T000000Z",
            "DTSTART:20261118T170000Z",
            "DTEND:20261118T180000Z",
            "SUMMARY:Retro",
            "END:VEVENT",
        ]);
    }
    lines.push("END:VCALENDAR");
    lines.push("");
    lines.join("\r\n")
}

pub async fn subscriptions_dated_tasks_and_notes_library(t: &Ctx) -> Result<()> {
    let root = t.temp("dar-subscriptions")?;
    let retro = Arc::new(AtomicBool::new(true));
    let requests = Arc::new(AtomicUsize::new(0));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let feed_port = listener.local_addr()?.port();
    let feed_url = format!("webcal://127.0.0.1:{feed_port}/team.ics");

    let r_flag = retro.clone();
    let r_count = requests.clone();
    let app = Router::new().fallback(move || {
        let r_flag = r_flag.clone();
        let r_count = r_count.clone();
        async move {
            r_count.fetch_add(1, Ordering::Relaxed);
            let body = feed(r_flag.load(Ordering::Relaxed));
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/calendar")],
                body,
            )
        }
    });
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let runtime = Instance::isolated(
        &root,
        "dar-subs",
        InstanceConfig {
            news_allow_private_networks: true,
            ..InstanceConfig::default()
        },
        true,
    )
    .await?;

    let outcome: Result<()> = async {
        let base = runtime.base.clone();
        let token = dar_acceptance::read_to_string(
            &dar_acceptance::stack::stack().bootstrap_token_file,
        )?;
        let setup = Reply::from(
            session::client()
                .post(format!("{base}/api/v1/auth/setup"))
                .header("origin", &base)
                .json(&json!({
                    "username": "admin",
                    "displayName": "Subscriptions",
                    "password": PASSWORD,
                    "bootstrapToken": token.trim(),
                }))
                .send()
                .await?,
        )
        .await?;
        assert_eq!(setup.status, 200, "{}", setup.text());

        let session = login(&base).await?;
        let call = |method: &'static str, route: String, body: Option<Value>| {
            let session = session.clone();
            async move {
                let mut req = session
                    .request(method, &route)
                    .header("content-type", "application/json");
                if let Some(b) = body {
                    req = req.body(b.to_string());
                }
                let reply = Reply::from(req.send().await?).await?;
                ensure!(
                    (200..300).contains(&reply.status),
                    "{method} {route}: {} {}",
                    reply.status,
                    reply.text()
                );
                reply.json()
            }
        };

        let calendars = || async {
            let v = call("GET", "/api/v1/calendar/calendars".into(), None).await?;
            Ok::<Vec<Value>, Error>(v["items"].as_array().cloned().unwrap_or_default())
        };
        let occurrences = |id: &str, include_tasks: bool| {
            let route = format!(
                "/api/v1/calendar/calendars/{id}/occurrences?from=2026-11-15T00%3A00%3A00Z&to=2026-11-22T00%3A00%3A00Z&timezone=UTC&includeTasks={}",
                if include_tasks { "1" } else { "0" }
            );
            async move {
                let v = call("GET", route, None).await?;
                Ok::<Vec<Value>, Error>(v["items"].as_array().cloned().unwrap_or_default())
            }
        };

        // Subscribe: the first refresh runs right away and lists every UID once (overrides included).
        let created = call(
            "POST",
            "/api/v1/calendar/subscriptions".into(),
            Some(json!({"name": "Team", "color": "#2f7d8c", "url": feed_url, "refreshMinutes": 60})),
        )
        .await?;
        let created_id = created["id"].as_str().context("created id")?.to_owned();

        let subscribed = eventually(Duration::from_secs(15), "initial subscription sync", || async {
            let list = calendars().await?;
            Ok(list.into_iter().find(|c| {
                c["id"] == created_id.as_str()
                    && c["subscription"]["lastSyncAt"].as_f64().unwrap_or(0.0) != 0.0
            }))
        })
        .await?;
        assert_eq!(subscribed["subscription"]["url"], feed_url);
        assert_eq!(subscribed["subscription"]["lastError"], "");

        let mut titles: Vec<String> = occurrences(&created_id, false)
            .await?
            .into_iter()
            .map(|o| o["title"].as_str().unwrap_or_default().to_owned())
            .collect();
        titles.sort();
        assert_eq!(
            titles,
            vec![
                "Company offsite",
                "Retro",
                "Standup",
                "Standup",
                "Standup (moved)"
            ]
        );

        // Read-only everywhere; invalid links and intervals are refused.
        let write = session
            .post(
                &format!("/api/v1/calendar/calendars/{created_id}/items"),
                &json!({"data": {"title": "Mine", "start": "2026-11-19T10:00", "end": "2026-11-19T11:00", "timezone": "UTC", "allDay": false}}),
            )
            .await?;
        assert_eq!(write.status, 403);
        assert_eq!(
            session
                .post(
                    "/api/v1/calendar/subscriptions",
                    &json!({"name": "Bad", "url": "ftp://example.test/a.ics", "refreshMinutes": 60}),
                )
                .await?
                .status,
            400
        );
        assert_eq!(
            session
                .post(
                    "/api/v1/calendar/subscriptions",
                    &json!({"name": "Bad", "url": feed_url, "refreshMinutes": 7}),
                )
                .await?
                .status,
            400
        );
        let task_lists = call("GET", "/api/v1/tasks/lists".into(), None).await?;
        assert!(!task_lists["items"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|c| c["id"] == created_id.as_str()));

        // Refresh now: events the source dropped are removed.
        retro.store(false, Ordering::Relaxed);
        let before = requests.load(Ordering::Relaxed);
        call(
            "POST",
            format!("/api/v1/calendar/calendars/{created_id}/refresh"),
            Some(json!({})),
        )
        .await?;
        eventually(Duration::from_secs(15), "refresh dropping Retro", || async {
            if requests.load(Ordering::Relaxed) <= before {
                return Ok(None);
            }
            let items = occurrences(&created_id, false).await?;
            Ok((!items.iter().any(|o| o["title"] == "Retro")).then_some(()))
        })
        .await?;

        // A broken link reports an error on the calendar and keeps the synced events.
        call(
            "PUT",
            format!("/api/v1/calendar/calendars/{created_id}/subscription"),
            Some(json!({"url": "http://127.0.0.1:9/missing.ics"})),
        )
        .await?;
        eventually(Duration::from_secs(15), "broken subscription error", || async {
            let list = calendars().await?;
            let has_error = list.iter().any(|c| {
                c["id"] == created_id.as_str()
                    && !c["subscription"]["lastError"]
                        .as_str()
                        .unwrap_or_default()
                        .is_empty()
            });
            Ok(has_error.then_some(()))
        })
        .await?;
        let kept_titles: Vec<String> = occurrences(&created_id, false)
            .await?
            .into_iter()
            .map(|o| o["title"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert!(kept_titles.iter().any(|t| t == "Company offsite"));

        // Dated tasks appear in Calendar occurrences only when asked for.
        let list = call(
            "POST",
            "/api/v1/tasks/lists".into(),
            Some(json!({"name": "Errands", "color": "#7c3aed"})),
        )
        .await?;
        let list_id = list["id"].as_str().context("list id")?;
        call(
            "POST",
            "/api/v1/tasks/items".into(),
            Some(json!({"collectionId": list_id, "data": {"title": "File taxes", "status": "NEEDS-ACTION", "allDay": true, "start": "", "due": "2026-11-19", "timezone": "UTC"}})),
        )
        .await?;
        call(
            "POST",
            "/api/v1/tasks/items".into(),
            Some(json!({"collectionId": list_id, "data": {"title": "Someday", "status": "NEEDS-ACTION", "allDay": true, "start": "", "due": ""}})),
        )
        .await?;
        assert_eq!(occurrences(list_id, false).await?, Vec::<Value>::new());
        let tasks = occurrences(list_id, true).await?;
        let summary: Vec<(String, bool, String)> = tasks
            .iter()
            .map(|o| {
                (
                    o["title"].as_str().unwrap_or_default().to_owned(),
                    o["task"].as_bool().unwrap_or(false),
                    o["startAt"]
                        .as_str()
                        .unwrap_or_default()
                        .chars()
                        .take(10)
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![("File taxes".to_owned(), true, "2026-11-19".to_owned())]
        );

        // Notes: the library is the Markdown files in Files › docs/notes.
        let library = call("GET", "/api/v1/notes/library".into(), None).await?;
        assert_eq!(library["path"], "docs/notes");
        assert_eq!(library["notes"], json!([]));
        let root_id = library["rootId"].as_str().context("notes rootId")?;
        upload_with(
            &session,
            "Groceries.md",
            "# Groceries\n\nMilk and bread.\n",
            Upload {
                parent_id: root_id,
                ..Upload::default()
            },
        )
        .await?;
        upload_with(
            &session,
            "photo.png",
            "not a note",
            Upload {
                parent_id: root_id,
                ..Upload::default()
            },
        )
        .await?;
        let notes = call("GET", "/api/v1/notes/library".into(), None).await?;
        let rows: Vec<(&str, &str, &str)> = notes["notes"]
            .as_array()
            .context("notes array")?
            .iter()
            .map(|n| {
                (
                    n["name"].as_str().unwrap_or_default(),
                    n["title"].as_str().unwrap_or_default(),
                    n["excerpt"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(rows, vec![("Groceries.md", "Groceries", "Milk and bread.")]);
        assert_eq!(
            call("GET", "/api/v1/notes/library?q=bread".into(), None).await?["notes"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            call("GET", "/api/v1/notes/library?q=absent".into(), None).await?["notes"],
            json!([])
        );
        Ok(())
    }
    .await;

    runtime.close().await;
    server.abort();
    outcome
}
