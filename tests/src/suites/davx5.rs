//! Port of `test/davx5.integration.test.ts`: DAVx5 Files mounts discovery from generic `/dav/`
//! root and DAVx5-style CardDAV/CalDAV discovery and ETag synchronization with one scoped
//! credential for both homes.

use super::pim_workspace_support::{app_password, basic_auth, dav_request};
use dar_acceptance::prelude::*;
use regex::Regex;

pub const SUITE: Suite = Suite {
    name: "davx5",
    cases: &[
        case!(files_mounts_discover_from_generic_root, 40),
        case!(discovery_and_etag_sync_one_credential, 45),
    ],
};

pub async fn files_mounts_discover_from_generic_root(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let session = login(&base).await?;

    let files = basic_auth(
        "admin",
        app_password(&session, "Files mount proof", "v2 files:read files:write").await?["secret"]
            .as_str()
            .context("files secret")?,
    );
    let mixed = basic_auth(
        "admin",
        app_password(
            &session,
            "Files mount proof",
            "v2 files:read contacts:read calendar:read",
        )
        .await?["secret"]
            .as_str()
            .context("mixed secret")?,
    );
    let pim = basic_auth(
        "admin",
        app_password(&session, "Files mount proof", "pim:read").await?["secret"]
            .as_str()
            .context("pim secret")?,
    );

    let dav_one = Regex::new(r"\b1\b").unwrap();
    for path in ["/dav/", "/dav/files/admin", "/dav/files/admin/"] {
        let options = dav_request("OPTIONS", &format!("{base}{path}"), &[], None).await?;
        assert_eq!(options.status, 204);
        assert!(dav_one.is_match(options.header("dav").unwrap_or_default()));
    }

    let file = format!("Mount-{}.txt", uuid());
    let content = "Visible through the Files mount";
    assert_eq!(
        dav_request(
            "PUT",
            &format!("{base}/dav/files/admin/{file}"),
            &[("authorization", &files)],
            Some(content.as_bytes().to_vec()),
        )
        .await?
        .status,
        201
    );

    let body = b"<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\"><d:prop><d:displayname/><d:resourcetype/><d:getetag/><d:getcontentlength/><d:getcontenttype/></d:prop></d:propfind>";
    for auth in [&files, &mixed] {
        let root = dav_request(
            "PROPFIND",
            &format!("{base}/dav/"),
            &[("authorization", auth), ("depth", "1")],
            Some(body.to_vec()),
        )
        .await?;
        assert_eq!(root.status, 207, "{}", root.text());
        assert!(root.text().contains("<d:href>/dav/files/admin/</d:href>"));

        let listing = dav_request(
            "PROPFIND",
            &format!("{base}/dav/files/admin"),
            &[("authorization", auth), ("depth", "1")],
            Some(body.to_vec()),
        )
        .await?;
        assert_eq!(listing.status, 207);
        assert!(listing.text().contains(&file));

        assert_eq!(
            dav_request(
                "GET",
                &format!("{base}/dav/files/admin/{file}"),
                &[("authorization", auth)],
                None,
            )
            .await?
            .text(),
            content
        );
    }

    let unrelated = dav_request(
        "PROPFIND",
        &format!("{base}/dav/"),
        &[("authorization", &pim), ("depth", "1")],
        Some(body.to_vec()),
    )
    .await?;
    assert_eq!(unrelated.status, 207);
    assert!(!unrelated.text().contains("<d:href>/dav/files/"));
    assert_eq!(
        dav_request(
            "PROPFIND",
            &format!("{base}/dav/files/admin/"),
            &[("authorization", &pim), ("depth", "1")],
            None,
        )
        .await?
        .status,
        403
    );
    Ok(())
}

pub async fn discovery_and_etag_sync_one_credential(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let session = login(&base).await?;

    let credential = app_password(&session, "DAVx5 proof", "pim:write").await?;
    let auth = basic_auth(
        "admin",
        credential["secret"].as_str().context("credential secret")?,
    );
    assert_eq!(credential["davUrl"], format!("{base}/dav/"));

    for path in ["/dav", "/.well-known/carddav", "/.well-known/caldav"] {
        let response = dav_request("GET", &format!("{base}{path}"), &[], None).await?;
        assert_eq!(response.status, 308);
        assert_eq!(response.header("location"), Some("/dav/"));
    }

    let denied = dav_request("PROPFIND", &format!("{base}/dav/"), &[("depth", "0")], None).await?;
    assert_eq!(denied.status, 401);
    assert!(
        denied
            .header("www-authenticate")
            .unwrap_or_default()
            .contains("Basic")
    );

    let options = dav_request("OPTIONS", &format!("{base}/dav/"), &[], None).await?;
    assert_eq!(options.status, 204);
    let dav_hdr = options.header("dav").unwrap_or_default();
    assert!(dav_hdr.contains("calendar-access"));
    assert!(dav_hdr.contains("addressbook"));

    let discovery = b"<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\" xmlns:a=\"urn:ietf:params:xml:ns:carddav\"><d:prop><d:current-user-principal/><c:calendar-home-set/><a:addressbook-home-set/></d:prop></d:propfind>";
    for path in ["/dav/", "/dav/principals/admin/"] {
        let response = dav_request(
            "PROPFIND",
            &format!("{base}{path}"),
            &[
                ("authorization", &auth),
                ("depth", "0"),
                ("content-type", "application/xml"),
            ],
            Some(discovery.to_vec()),
        )
        .await?;
        assert_eq!(response.status, 207);
        let text = response.text();
        assert!(text.contains("/dav/calendars/admin/"));
        assert!(text.contains("/dav/addressbooks/admin/"));
    }

    let ctag_re = Regex::new(r"<cs:getctag>(.*?)</cs:getctag>").unwrap();
    for kind in ["contacts", "calendar"] {
        let api = format!(
            "/api/v1/{kind}/{}",
            if kind == "contacts" {
                "books"
            } else {
                "calendars"
            }
        );
        let collection = session
            .post(
                &api,
                &json!({"name": format!("DAVx5 {kind}"), "color": "#1e66f5"}),
            )
            .await?
            .expect(201)?
            .json()?;
        let collection_id = collection["id"].as_str().context("collection id")?;
        let home = format!(
            "/dav/{}/admin/",
            if kind == "contacts" {
                "addressbooks"
            } else {
                "calendars"
            }
        );
        let href = format!("{home}{collection_id}/");

        let list = dav_request(
            "PROPFIND",
            &format!("{base}{home}"),
            &[("authorization", &auth), ("depth", "1")],
            None,
        )
        .await?;
        assert_eq!(list.status, 207);
        let listing = list.text();
        assert!(listing.contains(collection_id), "{listing}");

        let before = dav_request(
            "PROPFIND",
            &format!("{base}{href}"),
            &[("authorization", &auth), ("depth", "0")],
            None,
        )
        .await?
        .text();
        let initial_tag = ctag_re
            .captures(&before)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
            .context("initial ctag")?;

        let uid = uuid();
        let content = if kind == "contacts" {
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{uid}\r\nFN:DAVx5 contact\r\nEND:VCARD\r\n")
        } else {
            format!(
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:20261109T120000Z\r\nDTEND:20261109T130000Z\r\nSUMMARY:DAVx5 event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
            )
        };
        let resource = format!(
            "{href}{uid}{}",
            if kind == "contacts" { ".vcf" } else { ".ics" }
        );

        let put = dav_request(
            "PUT",
            &format!("{base}{resource}"),
            &[("authorization", &auth), ("if-none-match", "*")],
            Some(content.as_bytes().to_vec()),
        )
        .await?;
        assert_eq!(put.status, 201, "{}", put.text());
        let etag = put.header("etag").context("put etag")?.to_owned();

        let fetched = dav_request(
            "GET",
            &format!("{base}{resource}"),
            &[("authorization", &auth)],
            None,
        )
        .await?;
        assert_eq!(fetched.status, 200);
        assert!(fetched.text().contains(&uid));

        let changed = dav_request(
            "PROPFIND",
            &format!("{base}{href}"),
            &[("authorization", &auth), ("depth", "0")],
            None,
        )
        .await?
        .text();
        let updated_tag = ctag_re
            .captures(&changed)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .context("updated ctag")?;
        assert_ne!(updated_tag, initial_tag);

        assert_eq!(
            dav_request(
                "PUT",
                &format!("{base}{resource}"),
                &[("authorization", &auth), ("if-match", "\"stale\"")],
                Some(content.as_bytes().to_vec()),
            )
            .await?
            .status,
            412
        );

        let read = app_password(&session, "Read only DAVx5", "pim:read").await?;
        let read_auth = basic_auth("admin", read["secret"].as_str().context("read secret")?);
        assert_eq!(
            dav_request(
                "GET",
                &format!("{base}{resource}"),
                &[("authorization", &read_auth)],
                None,
            )
            .await?
            .status,
            200
        );
        assert_eq!(
            dav_request(
                "DELETE",
                &format!("{base}{resource}"),
                &[("authorization", &read_auth), ("if-match", &etag)],
                None,
            )
            .await?
            .status,
            403
        );
        assert_eq!(
            dav_request(
                "DELETE",
                &format!("{base}{resource}"),
                &[("authorization", &auth), ("if-match", &etag)],
                None,
            )
            .await?
            .status,
            204
        );
        assert_eq!(
            dav_request(
                "GET",
                &format!("{base}{resource}"),
                &[("authorization", &auth)],
                None,
            )
            .await?
            .status,
            404
        );
    }

    let unrelated = dav_request(
        "PROPFIND",
        &format!("{base}/dav/files/admin/"),
        &[("authorization", &auth), ("depth", "0")],
        None,
    )
    .await?;
    assert!(
        unrelated.status == 401 || unrelated.status == 403,
        "expected 401 or 403, got {}",
        unrelated.status
    );

    assert_eq!(
        session
            .delete(&format!(
                "/api/v1/auth/app-passwords/{}",
                credential["id"].as_str().context("credential id")?
            ))
            .await?
            .status,
        200
    );
    assert_eq!(
        dav_request(
            "PROPFIND",
            &format!("{base}/dav/"),
            &[("authorization", &auth), ("depth", "0")],
            None,
        )
        .await?
        .status,
        401
    );
    Ok(())
}
