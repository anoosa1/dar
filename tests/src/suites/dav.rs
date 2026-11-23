//! Port of `test/dav.integration.test.ts`: scoped app passwords for WebDAV create, upload, list,
//! conditional update, move, copy, delete and revocation; client compatibility for parent folders,
//! modified times, PROPPATCH atomicity, advisory locks, streamed chunked uploads, checksums and
//! Depth infinity/finite listings.

use super::pim_workspace_support::{
    app_password, basic_auth, chunked_put, dav_request, md5_hex, parse_http_date_ms, sha1_hex,
};
use dar_acceptance::prelude::*;
use regex::Regex;
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "dav",
    cases: &[
        case!(scoped_app_passwords_webdav, 45),
        case!(webdav_clients_times_locks_and_streaming, 45),
    ],
};

pub async fn scoped_app_passwords_webdav(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let session = login(&base).await?;
    let write = app_password(&session, "Protocol test", "write").await?;
    let read = app_password(&session, "Protocol test", "read").await?;
    let auth = basic_auth("admin", write["secret"].as_str().context("write secret")?);
    let read_auth = basic_auth("admin", read["secret"].as_str().context("read secret")?);
    let dav_url = write["davUrl"].as_str().context("davUrl")?;
    let folder = format!("{dav_url}Protocol-{}/", uuid());
    let file = format!("{folder}hello.txt");

    assert_eq!(
        dav_request("MKCOL", &folder, &[("authorization", &auth)], None)
            .await?
            .status,
        201
    );
    let initial = dav_request(
        "PUT",
        &file,
        &[("authorization", &auth)],
        Some(b"version one".to_vec()),
    )
    .await?;
    assert_eq!(initial.status, 201, "{}", initial.text());
    let etag = initial.header("etag").context("initial etag")?.to_owned();

    let listing = dav_request(
        "PROPFIND",
        &folder,
        &[("authorization", &auth), ("depth", "1")],
        None,
    )
    .await?;
    assert_eq!(listing.status, 207);
    assert!(listing.text().contains("hello.txt"));

    assert_eq!(
        dav_request("GET", &file, &[("authorization", &read_auth)], None)
            .await?
            .text(),
        "version one"
    );
    assert_eq!(
        dav_request(
            "PUT",
            &file,
            &[("authorization", &read_auth)],
            Some(b"denied".to_vec()),
        )
        .await?
        .status,
        403
    );

    // Without If-Match a write replaces the file (its earlier content stays a version); with it, a stale ETag fails.
    let plain = dav_request(
        "PUT",
        &file,
        &[("authorization", &auth)],
        Some(b"unconditional".to_vec()),
    )
    .await?;
    assert_eq!(plain.status, 204, "{}", plain.text());
    let plain_etag = plain.header("etag").context("plain etag")?.to_owned();
    assert_ne!(plain_etag, etag);

    assert_eq!(
        dav_request(
            "PUT",
            &file,
            &[("authorization", &auth), ("if-match", &etag)],
            Some(b"stale".to_vec()),
        )
        .await?
        .status,
        412
    );

    let update = dav_request(
        "PUT",
        &file,
        &[("authorization", &auth), ("if-match", &plain_etag)],
        Some(b"version two".to_vec()),
    )
    .await?;
    assert_eq!(update.status, 204, "{}", update.text());
    let update_etag = update.header("etag").context("update etag")?.to_owned();

    assert_eq!(
        dav_request("HEAD", &file, &[("authorization", &auth)], None)
            .await?
            .header("etag"),
        Some(update_etag.as_str())
    );
    assert_eq!(
        dav_request(
            "PUT",
            &file,
            &[("authorization", &auth), ("if-none-match", "*")],
            Some(b"exists".to_vec()),
        )
        .await?
        .status,
        412
    );

    let moved = format!("{folder}moved.txt");
    let copy = format!("{folder}copy.txt");
    assert_eq!(
        dav_request(
            "MOVE",
            &file,
            &[
                ("authorization", &auth),
                ("if-match", &update_etag),
                ("destination", &moved),
            ],
            None,
        )
        .await?
        .status,
        201
    );
    let current_etag = dav_request("HEAD", &moved, &[("authorization", &auth)], None)
        .await?
        .header("etag")
        .context("current etag")?
        .to_owned();
    assert_eq!(
        dav_request(
            "COPY",
            &moved,
            &[
                ("authorization", &auth),
                ("if-match", &current_etag),
                ("destination", &copy),
            ],
            None,
        )
        .await?
        .status,
        201
    );
    assert_eq!(
        dav_request("GET", &copy, &[("authorization", &auth)], None)
            .await?
            .text(),
        "version two"
    );
    assert_eq!(
        dav_request(
            "COPY",
            &moved,
            &[
                ("authorization", &auth),
                ("destination", &copy),
                ("overwrite", "F"),
            ],
            None,
        )
        .await?
        .status,
        409
    );
    assert_eq!(
        dav_request(
            "COPY",
            &moved,
            &[("authorization", &auth), ("destination", &copy)],
            None,
        )
        .await?
        .status,
        204
    );
    assert_eq!(
        dav_request(
            "DELETE",
            &moved,
            &[("authorization", &auth), ("if-match", &current_etag)],
            None,
        )
        .await?
        .status,
        204
    );
    assert_eq!(
        dav_request("GET", &moved, &[("authorization", &auth)], None)
            .await?
            .status,
        404
    );
    assert_eq!(
        dav_request(
            "MOVE",
            &copy,
            &[("authorization", &auth), ("destination", &moved)],
            None,
        )
        .await?
        .status,
        201
    );
    assert_eq!(
        dav_request("DELETE", &moved, &[("authorization", &auth)], None)
            .await?
            .status,
        204
    );

    let revoked = session
        .delete(&format!(
            "/api/v1/auth/app-passwords/{}",
            write["id"].as_str().context("write id")?
        ))
        .await?;
    assert_eq!(revoked.status, 200);
    assert_eq!(
        dav_request("GET", &copy, &[("authorization", &auth)], None)
            .await?
            .status,
        401
    );
    let login_basic = basic_auth("admin", PASSWORD);
    assert_eq!(
        dav_request("GET", &copy, &[("authorization", &login_basic)], None)
            .await?
            .status,
        401
    );
    Ok(())
}

pub async fn webdav_clients_times_locks_and_streaming(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let session = login(&base).await?;
    let creds = app_password(&session, "Client test", "write").await?;
    let auth = basic_auth("admin", creds["secret"].as_str().context("secret")?);
    let dav_url = creds["davUrl"].as_str().context("davUrl")?;
    let folder = format!("{dav_url}Clients-{}/", uuid());

    let options = dav_request("OPTIONS", dav_url, &[], None).await?;
    assert_eq!(options.header("dav"), Some("1, 2"));
    assert!(options
        .header("allow")
        .unwrap_or_default()
        .contains("LOCK, UNLOCK"));

    // A missing parent is 409, which clients answer by creating it; an existing folder is 405.
    assert_eq!(
        dav_request(
            "MKCOL",
            &format!("{folder}nested/"),
            &[("authorization", &auth)],
            None,
        )
        .await?
        .status,
        409
    );
    assert_eq!(
        dav_request(
            "PUT",
            &format!("{folder}nested/file.txt"),
            &[("authorization", &auth)],
            Some(b"x".to_vec()),
        )
        .await?
        .status,
        409
    );
    assert_eq!(
        dav_request("MKCOL", &folder, &[("authorization", &auth)], None)
            .await?
            .status,
        201
    );
    assert_eq!(
        dav_request("MKCOL", &folder, &[("authorization", &auth)], None)
            .await?
            .status,
        405
    );

    let last_mod_re = Regex::new(r"<d:getlastmodified>([^<]+)<").unwrap();
    let modified = |url: String| {
        let auth = auth.clone();
        let re = last_mod_re.clone();
        async move {
            let listing = dav_request(
                "PROPFIND",
                &url,
                &[("authorization", &auth), ("depth", "0")],
                None,
            )
            .await?
            .text();
            let raw = re
                .captures(&listing)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str())
                .context("missing getlastmodified")?;
            parse_http_date_ms(raw)
        }
    };

    // rclone, ownCloud and Nextcloud send the file's time with its content.
    let dated = format!("{folder}dated.txt");
    let time = chrono::NaiveDate::from_ymd_opt(2024, 1, 2)
        .unwrap()
        .and_hms_opt(3, 4, 5)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    let time_secs = (time / 1000).to_string();
    let put = dav_request(
        "PUT",
        &dated,
        &[("authorization", &auth), ("x-oc-mtime", &time_secs)],
        Some(b"dated".to_vec()),
    )
    .await?;
    assert_eq!(put.status, 201);
    assert_eq!(put.header("x-oc-mtime"), Some("accepted"));
    assert_eq!(modified(dated.clone()).await?, time);

    // A generic type gives way to the type of the name.
    assert_eq!(
        dav_request(
            "PUT",
            &format!("{folder}notes.md"),
            &[
                ("authorization", &auth),
                ("content-type", "application/octet-stream"),
            ],
            Some(b"# Notes".to_vec()),
        )
        .await?
        .status,
        201
    );
    let md_prop = dav_request(
        "PROPFIND",
        &format!("{folder}notes.md"),
        &[("authorization", &auth), ("depth", "0")],
        None,
    )
    .await?
    .text();
    assert!(md_prop.contains("<d:getcontenttype>text/markdown<"));

    let proppatch = |body: &str| {
        let auth = auth.clone();
        let dated = dated.clone();
        let xml = format!(
            "<?xml version=\"1.0\"?><d:propertyupdate xmlns:d=\"DAV:\" xmlns:z=\"urn:schemas-microsoft-com:\"><d:set><d:prop>{body}</d:prop></d:set></d:propertyupdate>"
        );
        async move {
            dav_request(
                "PROPPATCH",
                &dated,
                &[
                    ("authorization", &auth),
                    ("content-type", "application/xml"),
                ],
                Some(xml.into_bytes()),
            )
            .await
        }
    };

    let patched = proppatch("<d:lastmodified>1600000000</d:lastmodified>").await?;
    assert_eq!(patched.status, 207);
    assert!(Regex::new(r"(?s)lastmodified.*HTTP/1\.1 200 OK")
        .unwrap()
        .is_match(&patched.text()));
    assert_eq!(modified(dated.clone()).await?, 1_600_000_000_000);

    // Windows sets its file times and attributes after each save.
    let win_time = chrono::NaiveDate::from_ymd_opt(2022, 3, 2)
        .unwrap()
        .and_hms_opt(10, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    let patched = proppatch(
        "<z:Win32LastModifiedTime>Wed, 02 Mar 2022 10:00:00 GMT</z:Win32LastModifiedTime><z:Win32FileAttributes>00000020</z:Win32FileAttributes>",
    )
    .await?;
    let err_status_re = Regex::new(r"HTTP/1\.1 (403|424)").unwrap();
    assert!(!err_status_re.is_match(&patched.text()));
    assert_eq!(modified(dated.clone()).await?, win_time);

    // rclone sends back the checksums it read with the time; they are accepted and ignored.
    let patched = proppatch(&format!(
        "<d:lastmodified>1650000000</d:lastmodified><oc:checksums xmlns:oc=\"http://owncloud.org/ns\"><oc:checksum>SHA1:{}</oc:checksum></oc:checksums>",
        "0".repeat(40)
    ))
    .await?;
    assert!(!err_status_re.is_match(&patched.text()));
    assert_eq!(modified(dated.clone()).await?, 1_650_000_000_000);

    proppatch("<z:Win32LastModifiedTime>Wed, 02 Mar 2022 10:00:00 GMT</z:Win32LastModifiedTime>")
        .await?;

    // Properties apply together: an unsupported one fails the request and changes nothing.
    let patched =
        proppatch("<d:lastmodified>1500000000</d:lastmodified><d:author>someone</d:author>").await?;
    let failed = patched.text();
    assert!(Regex::new(r"(?s)author.*403 Forbidden")
        .unwrap()
        .is_match(&failed));
    assert!(Regex::new(r"(?s)lastmodified.*424 Failed Dependency")
        .unwrap()
        .is_match(&failed));
    assert_eq!(modified(dated.clone()).await?, win_time);

    // Locks are advisory: Finder and Windows mount writable, and a lock on a new name creates an empty file.
    let lock_body = "<?xml version=\"1.0\"?><d:lockinfo xmlns:d=\"DAV:\"><d:lockscope><d:exclusive/></d:lockscope><d:locktype><d:write/></d:locktype><d:owner><d:href>test</d:href></d:owner></d:lockinfo>";
    let lock = dav_request(
        "LOCK",
        &dated,
        &[("authorization", &auth), ("timeout", "Second-600")],
        Some(lock_body.as_bytes().to_vec()),
    )
    .await?;
    assert_eq!(lock.status, 200);
    let token = lock.header("lock-token").context("lock-token")?.to_owned();
    assert!(Regex::new(r"^<opaquelocktoken:[0-9a-f-]+>$")
        .unwrap()
        .is_match(&token));
    assert!(lock.text().contains("<d:timeout>Second-600</d:timeout>"));

    let if_header = format!("({token})");
    let refresh = dav_request(
        "LOCK",
        &dated,
        &[("authorization", &auth), ("if", &if_header)],
        None,
    )
    .await?;
    assert_eq!(refresh.status, 200);
    assert_eq!(refresh.header("lock-token"), Some(token.as_str()));

    assert_eq!(
        dav_request(
            "PUT",
            &dated,
            &[("authorization", &auth), ("if", &if_header)],
            Some(b"locked write".to_vec()),
        )
        .await?
        .status,
        204
    );
    assert_eq!(
        dav_request(
            "UNLOCK",
            &dated,
            &[("authorization", &auth), ("lock-token", &token)],
            None,
        )
        .await?
        .status,
        204
    );

    let placeholder = format!("{folder}placeholder.docx");
    assert_eq!(
        dav_request(
            "LOCK",
            &placeholder,
            &[("authorization", &auth)],
            Some(lock_body.as_bytes().to_vec()),
        )
        .await?
        .status,
        201
    );
    let empty = dav_request("GET", &placeholder, &[("authorization", &auth)], None).await?;
    assert_eq!(empty.status, 200);
    assert_eq!(empty.text(), "");
    assert!(dav_request(
        "PROPFIND",
        &folder,
        &[("authorization", &auth), ("depth", "1")],
        None,
    )
    .await?
    .text()
    .contains("<d:supportedlock>"));

    // Finder streams uploads in chunks and states the size in X-Expected-Entity-Length.
    let streamed = format!("{folder}streamed.bin");
    let content = vec![7u8; 200_000];
    let len_str = content.len().to_string();
    let (chunked_status, chunked_body) = chunked_put(
        &streamed,
        &[
            ("authorization", &auth),
            ("x-expected-entity-length", &len_str),
        ],
        &[&content[..100_000], &content[100_000..]],
    )
    .await?;
    assert_eq!(chunked_status, 201, "{chunked_body}");
    let downloaded = dav_request("GET", &streamed, &[("authorization", &auth)], None).await?;
    assert_eq!(downloaded.body, content);

    let (unsized_status, _) = chunked_put(
        &format!("{folder}unsized.bin"),
        &[("authorization", &auth)],
        &[&[0u8; 3]],
    )
    .await?;
    assert_eq!(unsized_status, 411);

    // Renaming and moving keep the modified time.
    assert_eq!(
        dav_request(
            "MKCOL",
            &format!("{folder}deeper/"),
            &[("authorization", &auth)],
            None,
        )
        .await?
        .status,
        201
    );
    let moved = format!("{folder}deeper/renamed.txt");
    let before = modified(dated.clone()).await?;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        dav_request(
            "MOVE",
            &dated,
            &[("authorization", &auth), ("destination", &moved)],
            None,
        )
        .await?
        .status,
        201
    );
    assert_eq!(modified(moved.clone()).await?, before);

    // Listings carry SHA-256 at once, and SHA-1 and MD5 once the runtime has hashed the content (rclone compares those).
    let sha1 = sha1_hex(&content);
    let md5 = md5_hex(&content);
    let sha256 = dar_acceptance::sha256_hex(&content);
    let checksum_re = Regex::new(r"<oc:checksum>([^<]*)<").unwrap();
    let mut checksum = String::new();
    for attempt in 0..50 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let body = dav_request(
            "PROPFIND",
            &streamed,
            &[("authorization", &auth), ("depth", "0")],
            None,
        )
        .await?
        .text();
        checksum = checksum_re
            .captures(&body)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
            .unwrap_or_default();
        if checksum.contains("SHA1:") {
            break;
        }
    }
    assert_eq!(checksum, format!("SHA1:{sha1} MD5:{md5} SHA256:{sha256}"));

    // Depth infinity (also the default) lists the whole tree; finite depths stay as they were.
    let moved_path = url::Url::parse(&moved)?.path().to_owned();
    let deeper_path = url::Url::parse(&format!("{folder}deeper/"))?
        .path()
        .to_owned();
    for depth in [Some("infinity"), None] {
        let mut hdrs = vec![("authorization", auth.as_str())];
        if let Some(d) = depth {
            hdrs.push(("depth", d));
        }
        let tree = dav_request("PROPFIND", &folder, &hdrs, None).await?.text();
        assert!(
            tree.contains(&format!("<d:href>{moved_path}</d:href>")),
            "{}",
            &tree[..tree.len().min(400)]
        );
        assert!(tree.contains(&format!("<d:href>{deeper_path}</d:href>")));
    }
    let depth1 = dav_request(
        "PROPFIND",
        &folder,
        &[("authorization", &auth), ("depth", "1")],
        None,
    )
    .await?
    .text();
    assert!(!depth1.contains("renamed.txt"));
    assert_eq!(
        dav_request(
            "PROPFIND",
            &folder,
            &[("authorization", &auth), ("depth", "2")],
            None,
        )
        .await?
        .status,
        400
    );
    assert_eq!(
        dav_request("DELETE", &folder, &[("authorization", &auth)], None)
            .await?
            .status,
        204
    );
    Ok(())
}
