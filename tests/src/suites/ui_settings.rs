//! UI settings (`ui-settings.integration.test.ts`): app preferences preservation, stale/invalid
//! write rejection, user scoping, live hidden-file window filtering, and personal profile edits.

use dar_acceptance::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "ui-settings",
    cases: &[case!(app_preferences_and_hidden_file_windows, 60), case!(personal_profile_edits, 60)],
};

async fn account(base: &str) -> Result<Session> {
    let admin = login(base).await?;
    let username = format!("ux-{}", &uuid()[..8]);
    admin
        .post("/api/v1/auth/users", &json!({ "username": username, "displayName": "UX settings", "password": PASSWORD }))
        .await?
        .expect(201)?;
    login_as(base, &username, PASSWORD).await
}

pub async fn app_preferences_and_hidden_file_windows(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let user = account(&base).await?;
    let other = account(&base).await?;
    let endpoint = "/api/v1/pim/preferences";

    let initial = user.get(endpoint).await?.expect(200)?;
    let etag = initial.header("etag").context("initial preferences etag")?.to_owned();

    let resp = Reply::from(
        user.request("PUT", endpoint)
            .header("if-match", &etag)
            .json(&json!({
                "filesView": "grid",
                "filesShowHidden": false,
                "notesEditorMode": "preview",
                "tasksView": "all",
                "appearance": "mocha",
                "calendarShowCancelled": true,
            }))
            .send()
            .await?,
    )
    .await?
    .expect(200)?;
    let mut value = resp.json()?;

    let stale = Reply::from(
        user.request("PUT", endpoint)
            .header("if-match", &etag)
            .json(&json!({ "filesView": "list" }))
            .send()
            .await?,
    )
    .await?;
    assert_eq!(stale.status, 412);

    let rev = value["revision"].as_u64().context("revision")?;
    let resp = Reply::from(
        user.request("PUT", endpoint)
            .header("if-match", format!("\"preferences:{rev}\""))
            .json(&json!({ "timezone": "America/Toronto", "weekStart": 0 }))
            .send()
            .await?,
    )
    .await?
    .expect(200)?;
    value = resp.json()?;
    assert_eq!(value["filesView"], json!("grid"));
    assert_eq!(value["appearance"], json!("mocha"));
    assert_eq!(value["calendarShowCancelled"], json!(true));
    assert_eq!(value["notesEditorMode"], json!("preview"));
    assert_eq!(value["tasksView"], json!("all"));

    assert!(other.get_json(endpoint).await?.get("filesView").is_none());

    let rev = value["revision"].as_u64().context("revision")?;
    for invalid in [
        json!({ "calendarShowCancelled": "yes" }),
        json!({ "appearance": "invalid" }),
        json!({ "filesView": "broken" }),
        json!({ "filesShowHidden": "yes" }),
        json!({ "tasksView": "unknown" }),
        json!({ "notesEditorMode": "html" }),
    ] {
        let bad = Reply::from(
            user.request("PUT", endpoint)
                .header("if-match", format!("\"preferences:{rev}\""))
                .json(&invalid)
                .send()
                .await?,
        )
        .await?;
        assert_eq!(bad.status, 400, "invalid payload {invalid}");
    }

    let hidden = upload(&user, ".hidden-proof.txt", "hidden").await?;
    let visible = upload(&user, "visible-proof.txt", "visible").await?;
    let live = user.connect().await?;
    let result = async {
        assert!(!live.db.my_files().iter().any(|r| r.id == hidden.file_id));
        assert!(live.db.my_files().iter().any(|r| r.id == visible.file_id));

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let tx = Arc::new(Mutex::new(Some(tx)));
        let hidden_id = hidden.file_id.clone();
        live.db.my_files().on_insert(move |_ctx, row| {
            if row.id == hidden_id
                && let Some(sender) = tx.lock().unwrap().take()
            {
                let _ = sender.send(());
            }
        });

        let resp = Reply::from(
            user.request("PUT", endpoint)
                .header("if-match", format!("\"preferences:{rev}\""))
                .json(&json!({ "filesShowHidden": true }))
                .send()
                .await?,
        )
        .await?;
        assert_eq!(resp.status, 200);
        tokio::time::timeout(Duration::from_secs(10), rx).await.map_err(|_| Error::new("hidden file insert did not arrive"))?.ok();
        assert!(live.db.my_files().iter().any(|r| r.id == hidden.file_id));
        Ok::<(), Error>(())
    }
    .await;
    live.disconnect();
    result
}

pub async fn personal_profile_edits(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let user = account(&base).await?;
    let other = account(&base).await?;
    let endpoint = "/api/v1/account/profile";

    let resp = user.get(endpoint).await?.expect(200)?;
    let etag = resp.header("etag").context("profile etag")?.to_owned();
    let mut profile = resp.json()?;
    let id = profile["id"].as_str().context("profile id")?.to_owned();
    let username = profile["userName"].as_str().context("userName")?.to_owned();

    let resp = Reply::from(
        user.request("PUT", endpoint)
            .header("if-match", &etag)
            .json(&json!({
                "displayName": "Changed display",
                "givenName": "Test",
                "familyName": "Person",
                "email": "profile@example.com",
            }))
            .send()
            .await?,
    )
    .await?
    .expect(200)?;
    profile = resp.json()?;
    assert_eq!(profile["id"], json!(id));
    assert_eq!(profile["userName"], json!(username));
    assert_eq!(profile["administrator"], json!(false));
    assert_eq!(profile["active"], json!(true));
    assert_eq!(profile["name"]["givenName"], json!("Test"));
    assert_eq!(profile["emails"][0]["value"], json!("profile@example.com"));

    let stale = Reply::from(
        user.request("PUT", endpoint)
            .header("if-match", &etag)
            .json(&json!({ "displayName": "stale" }))
            .send()
            .await?,
    )
    .await?;
    assert_eq!(stale.status, 412);

    let rev = profile["revision"].as_u64().context("profile revision")?;
    for privileged in [
        json!({ "displayName": "Rejected", "administrator": true }),
        json!({ "displayName": "Rejected", "userName": "admin" }),
        json!({ "displayName": "Rejected", "active": false }),
    ] {
        let bad = Reply::from(
            user.request("PUT", endpoint)
                .header("if-match", format!("\"{id}:{rev}\""))
                .json(&privileged)
                .send()
                .await?,
        )
        .await?;
        assert_eq!(bad.status, 400);
    }

    let missing_rev = user.put(endpoint, &json!({ "displayName": "missing revision" })).await?;
    assert_eq!(missing_rev.status, 428);

    assert_ne!(other.get_json(endpoint).await?["id"], json!(id));

    let session_info = user.get_json("/api/v1/auth/session").await?;
    assert_eq!(session_info["user"]["displayName"], json!("Changed display"));

    let invalid = Reply::from(
        user.request("PUT", endpoint)
            .header("if-match", format!("\"{id}:{rev}\""))
            .json(&json!({ "displayName": "", "email": "bad" }))
            .send()
            .await?,
    )
    .await?;
    assert_eq!(invalid.status, 400);
    Ok(())
}
