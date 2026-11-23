//! Appearance & mixed app-password scopes (`appearance-scopes.integration.test.ts`):
//! instance branding (public, admin-only, revisioned, resettable) and per-app read/write
//! DAV scope enforcement.

use dar_acceptance::prelude::*;

pub const SUITE: Suite = Suite {
    name: "appearance-scopes",
    cases: &[case!(instance_branding, 60), case!(mixed_app_permissions_over_dav, 60)],
};

const LOGO_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aX1sAAAAASUVORK5CYII=";

pub async fn instance_branding(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;
    let name = format!("appearance-{}", &uuid()[..8]);
    let pw = "an isolated appearance password";
    admin
        .post("/api/v1/auth/users", &json!({ "username": name, "displayName": "Appearance test", "password": pw }))
        .await?
        .expect(201)?;
    let user = login_as(&base, &name, pw).await?;

    let current: Value = session::client().get(format!("{base}/api/v1/appearance")).send().await?.json().await?;
    let rev = current["revision"].as_u64().context("appearance revision")?;

    let save = |session: &Session, revision: u64, image: &str, media_type: &str| {
        let session = session.clone();
        let image = image.to_owned();
        let media_type = media_type.to_owned();
        async move {
            Reply::from(
                session
                    .request("PUT", "/api/v1/appearance")
                    .header("if-match", format!("\"appearance:{revision}\""))
                    .json(&json!({ "revision": revision, "logo": image, "mediaType": media_type }))
                    .send()
                    .await?,
            )
            .await
        }
    };

    assert_eq!(save(&user, rev, LOGO_B64, "image/png").await?.status, 403);
    assert_eq!(save(&admin, rev, "PHN2Zz4=", "image/svg+xml").await?.status, 400);
    assert_eq!(save(&admin, rev, LOGO_B64, "image/png").await?.status, 200);
    assert_eq!(save(&admin, rev, LOGO_B64, "image/png").await?.status, 409);

    let saved: Value = session::client().get(format!("{base}/api/v1/appearance")).send().await?.json().await?;
    let logo_url = saved["logoUrl"].as_str().context("logoUrl")?;
    let image = Reply::from(session::client().get(format!("{base}{logo_url}")).send().await?).await?;
    assert_eq!(image.header("content-type"), Some("image/png"));
    assert_eq!(dar_acceptance::base64(&image.body), LOGO_B64);

    let saved_rev = saved["revision"].as_u64().context("saved revision")?;
    assert_eq!(save(&admin, saved_rev, "", "").await?.status, 200);
    assert_eq!(session::client().get(format!("{base}/api/v1/appearance/logo")).send().await?.status().as_u16(), 404);
    Ok(())
}

pub async fn mixed_app_permissions_over_dav(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = login(&base).await?;

    for invalid_scope in ["v2 ", "v2 files:admin", "v2 files:read files:read"] {
        let bad = admin.post("/api/v1/auth/app-passwords", &json!({ "label": "Invalid", "scope": invalid_scope })).await?;
        assert_eq!(bad.status, 400, "scope {invalid_scope}");
    }

    let create = |scope: &str| {
        let admin = admin.clone();
        let scope = scope.to_owned();
        async move {
            let value = admin
                .post("/api/v1/auth/app-passwords", &json!({ "label": "Mixed scopes", "scope": scope }))
                .await?
                .expect(201)?
                .json()?;
            let id = value["id"].as_str().context("id")?.to_owned();
            let secret = value["secret"].as_str().context("secret")?.to_owned();
            let dav_url = value["davUrl"].as_str().context("davUrl")?.to_owned();
            let auth = format!("Basic {}", dar_acceptance::base64(format!("admin:{secret}").as_bytes()));
            Ok::<(String, String, String), Error>((id, dav_url, auth))
        }
    };

    let (mixed_id, mixed_dav, mixed_auth) = create("v2 files:write contacts:read calendar:write").await?;
    let (_, _, read_auth) = create("v2 files:read calendar:read").await?;
    assert_eq!(mixed_dav, format!("{base}/dav/"));

    let file = format!("{base}/dav/files/admin/scope-{}.txt", uuid());
    let put = Reply::from(session::client().put(&file).header("authorization", &mixed_auth).body("private contents").send().await?)
        .await?
        .expect(201)?;
    let put_etag = put.header("etag").context("put etag")?.to_owned();

    assert_eq!(session::client().get(&file).header("authorization", &mixed_auth).send().await?.status().as_u16(), 403);
    assert_eq!(
        session::client()
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &file)
            .header("authorization", &mixed_auth)
            .header("depth", "0")
            .send()
            .await?
            .status()
            .as_u16(),
        403
    );
    assert_eq!(
        Reply::from(session::client().get(&file).header("authorization", &read_auth).send().await?).await?.text(),
        "private contents"
    );
    assert_eq!(session::client().put(&file).header("authorization", &read_auth).body("denied").send().await?.status().as_u16(), 403);

    let book = admin
        .post("/api/v1/contacts/books", &json!({ "name": "Scope book", "color": "#2563eb" }))
        .await?
        .expect(201)?
        .json()?;
    let book_id = book["id"].as_str().context("book id")?;
    let contact = format!("{base}/dav/addressbooks/admin/{book_id}/denied.vcf");
    assert_eq!(session::client().put(&contact).header("authorization", &mixed_auth).body("denied").send().await?.status().as_u16(), 403);

    let cal = admin
        .post("/api/v1/calendar/calendars", &json!({ "name": "Scope calendar", "color": "#2563eb" }))
        .await?
        .expect(201)?
        .json()?;
    let cal_id = cal["id"].as_str().context("cal id")?;
    let event = format!("{base}/dav/calendars/admin/{cal_id}/test.ics");
    let content = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Dar//Test//EN\r\nBEGIN:VEVENT\r\nUID:{}\r\nDTSTAMP:20261107T120000Z\r\nDTSTART:20261107T120000Z\r\nDTEND:20261107T130000Z\r\nSUMMARY:Scope test\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        uuid()
    );
    let created = Reply::from(
        session::client()
            .put(&event)
            .header("authorization", &mixed_auth)
            .header("if-none-match", "*")
            .body(content)
            .send()
            .await?,
    )
    .await?
    .expect(201)?;
    let event_etag = created.header("etag").context("event etag")?.to_owned();
    assert_eq!(session::client().get(&event).header("authorization", &mixed_auth).send().await?.status().as_u16(), 403);
    let read_event = Reply::from(session::client().get(&event).header("authorization", &read_auth).send().await?).await?.text();
    assert!(read_event.contains("Scope test"));
    assert_eq!(
        session::client()
            .delete(&event)
            .header("authorization", &mixed_auth)
            .header("if-match", &event_etag)
            .send()
            .await?
            .status()
            .as_u16(),
        204
    );

    admin.delete(&format!("/api/v1/auth/app-passwords/{mixed_id}")).await?;
    assert_eq!(
        session::client()
            .delete(&file)
            .header("authorization", &mixed_auth)
            .header("if-match", &put_etag)
            .send()
            .await?
            .status()
            .as_u16(),
        401
    );
    Ok(())
}
