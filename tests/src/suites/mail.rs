//! Port of `test/mail.integration.test.ts`: providers, IMAP, OAuth and JMAP accounts, the two-way
//! Files mirror, sending and isolation.

use crate::suites::comms_support::{MailAttachment, MailMessageSpec, MailServer, build_mail_message, https_get_location};
use chrono::{DateTime, Utc};
use dar_acceptance::prelude::*;

pub const SUITE: Suite = Suite { name: "mail", cases: &[case!(mail_providers_imap_oauth_jmap_mirror_and_isolation, 300)] };

fn parse_rfc3339(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

/// Percent-encodes a URI component matching JavaScript's `encodeURIComponent`.
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')');
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode_uri_component(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
                && let Ok(v) = u8::from_str_radix(hex, 16)
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn extract_dav_listing(xml: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<d:href>") {
        let after = &rest[start + "<d:href>".len()..];
        let Some(end) = after.find("</d:href>") else { break };
        let href = &after[..end];
        let decoded = decode_uri_component(href);
        if let Some(last) = decoded.split('/').filter(|p| !p.is_empty()).last() {
            items.push(last.to_owned());
        }
        rest = &after[end + "</d:href>".len()..];
    }
    if !items.is_empty() {
        items.remove(0);
    }
    items.sort();
    items
}

async fn mail_raw(session: &Session, method: &str, route: &str, body: Option<&Value>) -> Result<Reply> {
    let path = format!("/api/v1/mail{route}");
    match body {
        Some(b) => session.send_json(method, &path, b).await,
        None => Reply::from(session.request(method, &path).send().await?).await,
    }
}

async fn mail_call(session: &Session, method: &str, route: &str, body: Option<&Value>) -> Result<Value> {
    let reply = mail_raw(session, method, route, body).await?;
    ensure!((200..300).contains(&reply.status), "{method} {route}: {} {}", reply.status, reply.text());
    reply.json()
}

async fn overview(session: &Session) -> Result<Value> {
    mail_call(session, "GET", "/overview", None).await
}

async fn list_messages(session: &Session, view: &str, id: Option<&str>) -> Result<Vec<Value>> {
    let route = match id {
        Some(id) => format!("/messages?view={view}&id={id}"),
        None => format!("/messages?view={view}"),
    };
    let val = mail_call(session, "GET", &route, None).await?;
    Ok(val["items"].as_array().cloned().unwrap_or_default())
}

async fn sync_account(session: &Session, id: &str) -> Result<Value> {
    mail_call(session, "POST", "/sync", Some(&json!({ "id": id }))).await
}

async fn find_mailbox(session: &Session, account_id: &str, role: &str, name_is: Option<&str>) -> Result<Value> {
    let o = overview(session).await?;
    let account = o["accounts"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["id"].as_str() == Some(account_id)))
        .context("account not found")?;
    let mb = account["mailboxes"]
        .as_array()
        .and_then(|m| {
            m.iter().find(|x| match name_is {
                Some(n) => x["name"].as_str() == Some(n),
                None => x["role"].as_str() == Some(role),
            })
        })
        .context("mailbox not found")?;
    Ok(mb.clone())
}

async fn mail_providers_imap_oauth_jmap_mirror_and_isolation(t: &Ctx) -> Result<()> {
    let root = t.temp("mail")?;
    let mut server = MailServer::start(&root).await?;
    let oauth_secret = {
        let mut s = server.state.lock().unwrap();
        // Server mail before the account exists: a thread of two, an HTML message with a remote
        // image and an attachment, plus another inbox message, archive and nested mailbox.
        s.mail.put(
            "INBOX",
            build_mail_message(MailMessageSpec {
                subject: "Q4 venue shortlist",
                id: Some("venue-1@example.test"),
                body: "Two venues so far.",
                ..Default::default()
            }),
            &["\\Seen"],
        );
        s.mail.put(
            "INBOX",
            build_mail_message(MailMessageSpec {
                subject: "Re: Q4 venue shortlist",
                id: Some("venue-2@example.test"),
                references: &["venue-1@example.test"],
                date: parse_rfc3339("2026-11-21T09:00:00Z"),
                html: Some(r#"<p onclick="x()">Revised quote attached <img src="https://tracker.example/p.gif"></p><script>bad()</script>"#),
                attachment: Some(MailAttachment {
                    name: "quote.pdf",
                    content_type: "application/pdf",
                    data: b"%PDF-1.4 quote",
                }),
                ..Default::default()
            }),
            &[],
        );
        s.mail.put(
            "INBOX",
            build_mail_message(MailMessageSpec {
                subject: "Dinner on Friday?",
                from: "Omar Saleh <omar@example.test>",
                body: "Around 8?",
                date: parse_rfc3339("2026-11-22T09:58:00Z"),
                ..Default::default()
            }),
            &[],
        );
        s.mail.put(
            "Archive",
            build_mail_message(MailMessageSpec {
                subject: "Old receipt",
                from: "Shop <shop@example.test>",
                ..Default::default()
            }),
            &["\\Seen"],
        );
        s.mail.add("Projects/Offsite", "");
        s.mail.put(
            "Projects/Offsite",
            build_mail_message(MailMessageSpec {
                subject: "Bus times",
                from: "Rami <rami@example.test>",
                ..Default::default()
            }),
            &["\\Seen"],
        );
        s.oauth.secret.clone()
    };

    let secret_file = root.join("oauth-secret");
    let providers_file = root.join("mail-providers.json");
    std::fs::write(&secret_file, format!("{oauth_secret}\n"))?;
    let host = "127.0.0.1";
    let providers = json!([
        {
            "id": "test-imap",
            "name": "Test IMAP",
            "document": {
                "protocol": "imap",
                "auth": "password",
                "domains": ["example.test"],
                "saveSent": true,
                "imap": { "host": host, "port": server.imap_port, "security": "tls" },
                "smtp": { "host": host, "port": server.smtp_port, "security": "tls" },
            }
        },
        {
            "id": "test-oauth",
            "name": "Test OAuth",
            "secretFile": secret_file.to_string_lossy(),
            "document": {
                "protocol": "imap",
                "auth": "oauth",
                "domains": ["oauth.example.test"],
                "saveSent": false,
                "imap": { "host": host, "port": server.imap_port, "security": "tls" },
                "smtp": { "host": host, "port": server.smtp_port, "security": "tls" },
                "oauth": {
                    "authorizeUrl": format!("{}/oauth/authorize", server.https),
                    "tokenUrl": format!("{}/oauth/token", server.https),
                    "clientId": "dar-test-client",
                    "scopes": ["mail"],
                    "params": { "access_type": "offline" },
                }
            }
        },
        {
            "id": "test-jmap",
            "name": "Test JMAP",
            "document": {
                "protocol": "jmap",
                "auth": "token",
                "domains": [],
                "jmap": { "url": format!("{}/jmap/session", server.https) },
            }
        },
    ]);
    std::fs::write(&providers_file, serde_json::to_string(&providers)?)?;

    let mut cfg = InstanceConfig::default();
    cfg.env.push(("DAR_MAIL_PROVIDERS_FILE".into(), providers_file.to_string_lossy().into_owned()));
    cfg.env.push(("SSL_CERT_FILE".into(), server.ca_file.to_string_lossy().into_owned()));
    let runtime = Instance::isolated(&root, "dar-mail", cfg, true).await?;
    let base = runtime.base.clone();
    session::setup_admin(&base).await?;
    let session = login(&base).await?;

    // Providers from the host configuration; Administration adds and removes its own.
    let o = overview(&session).await?;
    let provider_ids: Vec<&str> = o["providers"].as_array().unwrap().iter().filter_map(|p| p["id"].as_str()).collect();
    assert_eq!(provider_ids, vec!["test-imap", "test-oauth", "test-jmap"]);
    assert_eq!(o["mailFolder"], json!("docs/mail"));
    assert_eq!(o["administrator"], json!(true));

    let admin = mail_call(&session, "GET", "/admin/providers", None).await?;
    let admin_pairs: Vec<(&str, &str)> = admin["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p["source"].as_str().unwrap()))
        .collect();
    assert_eq!(admin_pairs, vec![("test-imap", "nix"), ("test-oauth", "nix"), ("test-jmap", "nix")]);
    assert!(!admin.to_string().contains(&oauth_secret), "secrets never leave the server");

    let edit_nix = mail_raw(
        &session,
        "PUT",
        "/admin/providers/test-imap",
        Some(&json!({ "name": "Changed", "document": { "protocol": "jmap", "auth": "token", "jmap": { "url": "https://x.test/" } } })),
    )
    .await?;
    assert_eq!(edit_nix.status, 403);

    let lan = mail_call(
        &session,
        "PUT",
        "/admin/providers/lan",
        Some(&json!({
            "name": "LAN",
            "create": true,
            "secret": "lan-secret",
            "document": {
                "protocol": "imap",
                "auth": "password",
                "domains": ["lan.test"],
                "imap": { "host": "mail.lan.test", "port": 993, "security": "tls" },
                "smtp": { "host": "mail.lan.test", "port": 587, "security": "starttls" },
            }
        })),
    )
    .await?;
    assert_eq!(lan["hasSecret"], json!(true));

    let bad_prov = mail_raw(
        &session,
        "PUT",
        "/admin/providers/bad",
        Some(&json!({
            "name": "Bad",
            "document": { "protocol": "imap", "auth": "password", "imap": { "host": "x", "port": 99999, "security": "tls" } }
        })),
    )
    .await?;
    assert_eq!(bad_prov.status, 400);

    assert!(overview(&session).await?["providers"].as_array().unwrap().iter().any(|p| p["id"] == json!("lan")));
    mail_call(&session, "DELETE", "/admin/providers/lan", None).await?;

    // An IMAP account: a wrong password is refused before anything is saved.
    let wrong = mail_raw(
        &session,
        "POST",
        "/accounts",
        Some(&json!({ "providerId": "test-imap", "name": "Work", "address": "anas@example.test", "password": "nope" })),
    )
    .await?;
    assert_eq!(wrong.status, 502);
    assert!(wrong.json()?["error"]["message"].as_str().unwrap_or_default().contains("rejected the sign-in"));
    assert_eq!(overview(&session).await?["accounts"].as_array().unwrap().len(), 0);

    let work = mail_call(
        &session,
        "POST",
        "/accounts",
        Some(&json!({
            "providerId": "test-imap",
            "name": "Work",
            "address": "anas@example.test",
            "displayName": "Anas",
            "password": "imap secret password",
        })),
    )
    .await?;
    let work_id = work["id"].as_str().context("work id")?.to_owned();
    sync_account(&session, &work_id).await?;

    let o = overview(&session).await?;
    let account = &o["accounts"][0];
    assert_eq!(account["status"], json!("ready"), "lastError: {}", account["lastError"]);
    let mut mb_pairs: Vec<(&str, &str)> = account["mailboxes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["name"].as_str().unwrap(), m["role"].as_str().unwrap()))
        .collect();
    mb_pairs.sort();
    assert_eq!(
        mb_pairs,
        vec![
            ("Archive", "archive"),
            ("Drafts", "drafts"),
            ("Inbox", "inbox"),
            ("Junk", "junk"),
            ("Offsite", ""),
            ("Projects", ""),
            ("Sent", "sent"),
            ("Trash", "trash"),
        ]
    );
    let offsite = account["mailboxes"].as_array().unwrap().iter().find(|m| m["name"] == json!("Offsite")).unwrap();
    let projects = account["mailboxes"].as_array().unwrap().iter().find(|m| m["name"] == json!("Projects")).unwrap();
    assert_eq!(offsite["parentId"], projects["id"]);
    assert_eq!(o["counts"]["inbox"], json!(2));

    let mut inbox = list_messages(&session, "inbox", None).await?;
    let inbox_subjects: Vec<&str> = inbox.iter().filter_map(|m| m["subject"].as_str()).collect();
    assert_eq!(inbox_subjects, vec!["Dinner on Friday?", "Re: Q4 venue shortlist", "Q4 venue shortlist"]);
    let reply = inbox[1].clone();
    let reply_id = reply["id"].as_str().unwrap().to_owned();
    assert_eq!(reply["threadCount"], json!(2));
    assert_eq!(reply["attachments"], json!(true));
    assert_eq!(reply["seen"], json!(false));
    let offsite_msgs = list_messages(&session, "mailbox", offsite["id"].as_str()).await?;
    assert_eq!(
        offsite_msgs.iter().filter_map(|m| m["subject"].as_str()).collect::<Vec<_>>(),
        vec!["Bus times"]
    );

    // Reading comes from the .eml file: sanitized, remote images blocked until asked, attachments served.
    let detail = mail_call(&session, "GET", &format!("/messages/{reply_id}"), None).await?;
    assert_eq!(detail["body"]["blockedImages"], json!(1));
    let html = detail["body"]["html"].as_str().unwrap_or_default();
    assert!(!html.contains("script") && !html.contains("onclick") && !html.contains("tracker.example"), "{html}");
    let thread_subjects: Vec<&str> = detail["thread"].as_array().unwrap().iter().filter_map(|m| m["subject"].as_str()).collect();
    assert_eq!(thread_subjects, vec!["Q4 venue shortlist", "Re: Q4 venue shortlist"]);
    assert_eq!(detail["fileName"], json!("2026-11-21 0900 Re: Q4 venue shortlist.eml"));

    let with_images = mail_call(&session, "GET", &format!("/messages/{reply_id}?images=1"), None).await?;
    assert!(with_images["body"]["html"]
        .as_str()
        .unwrap_or_default()
        .contains("/api/v1/mail/image?url=https%3A%2F%2Ftracker.example%2Fp.gif"));

    let pdf = mail_raw(&session, "GET", &format!("/messages/{reply_id}/attachments/0"), None).await?;
    assert_eq!(pdf.status, 200);
    assert_eq!(pdf.text(), "%PDF-1.4 quote");
    let cd = pdf.headers.get("content-disposition").and_then(|v| v.to_str().ok()).unwrap_or_default();
    assert!(cd.starts_with(r#"attachment; filename="quote.pdf""#), "cd={cd}");
    assert_eq!(pdf.headers.get("content-type").and_then(|v| v.to_str().ok()), Some("application/octet-stream"));
    assert_eq!(mail_raw(&session, "GET", &format!("/messages/{reply_id}/raw"), None).await?.status, 200);

    // The mirror in Files, through WebDAV.
    let cred = session
        .post("/api/v1/auth/app-passwords", &json!({ "label": "Mail mirror", "scope": "v2 files:read files:write" }))
        .await?
        .expect(201)?
        .json()?;
    let dav_auth = format!("Basic {}", dar_acceptance::base64(format!("admin:{}", cred["secret"].as_str().unwrap()).as_bytes()));
    let dav_root = format!("{base}/dav/files/admin/docs/mail/Work");
    let dav_listing = |folder: String| {
        let dav_root = dav_root.clone();
        let dav_auth = dav_auth.clone();
        async move {
            let reply = Reply::from(
                session::client()
                    .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), format!("{dav_root}/{folder}/"))
                    .header("authorization", &dav_auth)
                    .header("depth", "1")
                    .send()
                    .await?,
            )
            .await?;
            ensure!(reply.status == 207, "PROPFIND {folder}: {} {}", reply.status, reply.text());
            Ok::<Vec<String>, Error>(extract_dav_listing(&reply.text()))
        }
    };
    assert_eq!(
        dav_listing("Inbox".into()).await?,
        vec![
            "2026-11-20 1024 Q4 venue shortlist.eml",
            "2026-11-21 0900 Re: Q4 venue shortlist.eml",
            "2026-11-22 0958 Dinner on Friday?.eml",
        ]
    );
    assert_eq!(dav_listing("Projects".into()).await?, vec!["Offsite"]);

    let dinner_file = format!("{dav_root}/Inbox/{}", encode_uri_component("2026-11-22 0958 Dinner on Friday?.eml"));
    let dinner_get = Reply::from(session::client().get(&dinner_file).header("authorization", &dav_auth).send().await?).await?;
    assert!(dinner_get.text().contains("Subject: Dinner on Friday?"));

    // Flags and moves made in Mail reach the server; the file follows the message.
    mail_call(&session, "POST", "/messages/flags", Some(&json!({ "ids": [reply_id], "seen": true, "flagged": true }))).await?;
    {
        let s = server.state.lock().unwrap();
        let (_, m) = s.mail.find_by_text("Message-ID: <venue-2@").context("venue-2 on server")?;
        assert!(m.flags.contains("\\Flagged"));
    }
    assert_eq!(overview(&session).await?["counts"]["flagged"], json!(1));
    let dinner = inbox[0].clone();
    let dinner_id = dinner["id"].as_str().unwrap().to_owned();
    assert_eq!(
        mail_call(&session, "POST", "/messages/move", Some(&json!({ "ids": [dinner_id], "role": "archive" }))).await?,
        json!({ "moved": 1 })
    );
    {
        let s = server.state.lock().unwrap();
        let (box_name, _) = s.mail.find_by_text("Dinner on Friday").context("dinner on server")?;
        assert_eq!(box_name, "Archive");
    }
    assert!(dav_listing("Archive".into()).await?.iter().any(|n| n == "2026-11-22 0958 Dinner on Friday?.eml"));
    assert!(!dav_listing("Inbox".into()).await?.iter().any(|n| n == "2026-11-22 0958 Dinner on Friday?.eml"));

    // Undo moves it back by mailbox.
    let inbox_mb = find_mailbox(&session, &work_id, "inbox", None).await?;
    mail_call(
        &session,
        "POST",
        "/messages/move",
        Some(&json!({ "ids": [dinner_id], "mailboxId": inbox_mb["id"] })),
    )
    .await?;
    {
        let s = server.state.lock().unwrap();
        let (box_name, _) = s.mail.find_by_text("Dinner on Friday").context("dinner back in inbox")?;
        assert_eq!(box_name, "INBOX");
    }

    // Two-way: changes made in Files go to the server on the next pass.
    let move_file = Reply::from(
        session::client()
            .request(reqwest::Method::from_bytes(b"MOVE").unwrap(), &dinner_file)
            .header("authorization", &dav_auth)
            .header(
                "destination",
                format!("{dav_root}/Projects/Offsite/{}", encode_uri_component("2026-11-22 0958 Dinner on Friday?.eml")),
            )
            .send()
            .await?,
    )
    .await?;
    assert_eq!(move_file.status, 201, "{}", move_file.text());

    let venue_file = format!("{dav_root}/Inbox/{}", encode_uri_component("2026-11-20 1024 Q4 venue shortlist.eml"));
    let del_venue = Reply::from(session::client().delete(&venue_file).header("authorization", &dav_auth).send().await?).await?;
    assert_eq!(del_venue.status, 204);

    let added = build_mail_message(MailMessageSpec {
        subject: "Added from Files",
        from: "Me <anas@example.test>",
        id: Some("from-files@example.test"),
        ..Default::default()
    });
    let put_added = Reply::from(
        session::client()
            .put(format!("{dav_root}/Inbox/added.eml"))
            .header("authorization", &dav_auth)
            .body(added)
            .send()
            .await?,
    )
    .await?;
    assert_eq!(put_added.status, 201);

    let mkcol = Reply::from(
        session::client()
            .request(reqwest::Method::from_bytes(b"MKCOL").unwrap(), format!("{dav_root}/Receipts/"))
            .header("authorization", &dav_auth)
            .send()
            .await?,
    )
    .await?;
    assert_eq!(mkcol.status, 201);

    let receipt_body = build_mail_message(MailMessageSpec {
        subject: "Kept receipt",
        id: Some("receipt@example.test"),
        ..Default::default()
    });
    let put_receipt = Reply::from(
        session::client()
            .put(format!("{dav_root}/Receipts/receipt.eml"))
            .header("authorization", &dav_auth)
            .body(receipt_body)
            .send()
            .await?,
    )
    .await?;
    assert_eq!(put_receipt.status, 201);

    sync_account(&session, &work_id).await?;
    {
        let s = server.state.lock().unwrap();
        assert_eq!(
            s.mail.find_by_text("Dinner on Friday").unwrap().0,
            "Projects/Offsite",
            "moved in Files -> moved on the server"
        );
        assert_eq!(
            s.mail.find_by_text("Message-ID: <venue-1@").unwrap().0,
            "Trash",
            "deleted in Files -> in Trash on the server"
        );
        assert_eq!(
            s.mail.find_by_text("Message-ID: <from-files@").unwrap().0,
            "INBOX",
            "added in Files -> appended"
        );
        assert!(s.created.iter().any(|c| c == "Receipts"), "a folder made in Files -> a new mailbox");
        assert_eq!(s.mail.find_by_text("Message-ID: <receipt@").unwrap().0, "Receipts");
    }
    let mut inbox_after: Vec<String> = list_messages(&session, "inbox", None)
        .await?
        .iter()
        .filter_map(|m| m["subject"].as_str().map(str::to_owned))
        .collect();
    inbox_after.sort();
    assert_eq!(inbox_after, vec!["Added from Files", "Re: Q4 venue shortlist"]);

    // The added file is the message's file (no second copy appears on the next pass).
    sync_account(&session, &work_id).await?;
    assert_eq!(dav_listing("Inbox".into()).await?.iter().filter(|n| n.ends_with(".eml")).count(), 2);
    let receipts_mb = find_mailbox(&session, &work_id, "", Some("Receipts")).await?;
    let receipts_msgs: Vec<String> = list_messages(&session, "mailbox", receipts_mb["id"].as_str())
        .await?
        .iter()
        .filter_map(|m| m["subject"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(receipts_msgs, vec!["Kept receipt"]);

    // Deleting a whole mailbox folder does not delete its messages: it is made again.
    let del_arch = Reply::from(
        session::client()
            .delete(format!("{dav_root}/Archive/"))
            .header("authorization", &dav_auth)
            .send()
            .await?,
    )
    .await?;
    assert_eq!(del_arch.status, 204);
    sync_account(&session, &work_id).await?;
    {
        let s = server.state.lock().unwrap();
        assert_eq!(s.mail.find_by_text("Old receipt").unwrap().0, "Archive");
    }
    assert_eq!(dav_listing("Archive".into()).await?, vec!["2026-11-20 1024 Old receipt.eml"]);

    // Server -> Files: new mail, removed mail and flag changes.
    {
        let mut s = server.state.lock().unwrap();
        s.mail.put(
            "INBOX",
            build_mail_message(MailMessageSpec {
                subject: "Fresh arrival",
                from: "Maya <maya@example.test>",
                date: parse_rfc3339("2026-11-23T08:00:00Z"),
                ..Default::default()
            }),
            &[],
        );
        assert!(s.mail.mutate_by_text("Message-ID: <venue-2@", |m| {
            m.flags.remove("\\Flagged");
        }));
        assert!(s.mail.remove_by_text("Message-ID: <from-files@"));
    }
    sync_account(&session, &work_id).await?;
    inbox = list_messages(&session, "inbox", None).await?;
    assert_eq!(
        inbox.iter().filter_map(|m| m["subject"].as_str()).collect::<Vec<_>>(),
        vec!["Fresh arrival", "Re: Q4 venue shortlist"]
    );
    assert_eq!(inbox[1]["flagged"], json!(false));
    assert!(
        !dav_listing("Inbox".into()).await?.iter().any(|n| n == "added.eml"),
        "removed on the server -> its file moved to deleted files"
    );

    // Sending: SMTP with Bcc only in the envelope, a copy in Sent, the parent answered.
    let sent = mail_call(
        &session,
        "POST",
        "/send",
        Some(&json!({
            "accountId": work_id,
            "to": ["lina@example.test"],
            "cc": [],
            "bcc": ["boss@example.test"],
            "subject": "Re: Q4 venue shortlist",
            "text": "Go ahead.\n.Leading dot",
            "inReplyTo": reply_id,
            "attachments": [{
                "name": "notes.txt",
                "type": "text/plain",
                "data": dar_acceptance::base64(b"plan"),
            }],
        })),
    )
    .await?;
    {
        let s = server.state.lock().unwrap();
        assert_eq!(s.sent.len(), 1);
        assert_eq!(s.sent[0].to, vec!["lina@example.test", "boss@example.test"]);
        assert_eq!(s.sent[0].from, "anas@example.test");
        let data = &s.sent[0].data;
        let has_bcc_hdr = data.lines().any(|l| l.to_ascii_lowercase().starts_with("bcc:"));
        assert!(
            !has_bcc_hdr
                && data.contains("In-Reply-To: <venue-2@example.test>")
                && data.contains("References: <venue-1@example.test> <venue-2@example.test>"),
            "{data}"
        );
        assert!(data.contains("\r\n.Leading dot"), "{data}");
        assert_eq!(s.appended.last().map(|a| a.box_name.as_str()), Some("Sent"));
        let (_, m) = s.mail.find_by_text("Message-ID: <venue-2@").unwrap();
        assert!(m.flags.contains("\\Answered"));
    }
    assert!(sent["id"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(
        list_messages(&session, "sent", None).await?.iter().filter_map(|m| m["subject"].as_str()).collect::<Vec<_>>(),
        vec!["Re: Q4 venue shortlist"]
    );
    assert_eq!(
        mail_call(&session, "GET", &format!("/messages/{reply_id}"), None).await?["thread"].as_array().unwrap().len(),
        3,
        "the reply joins the thread"
    );
    sync_account(&session, &work_id).await?;
    assert_eq!(list_messages(&session, "sent", None).await?.len(), 1, "the Sent copy is not mirrored twice");

    // Drafts: saved on the server; saving again replaces it.
    let draft = mail_call(
        &session,
        "POST",
        "/drafts",
        Some(&json!({ "accountId": work_id, "to": [], "subject": "Unfinished", "text": "first" })),
    )
    .await?;
    let draft2 = mail_call(
        &session,
        "POST",
        "/drafts",
        Some(&json!({
            "accountId": work_id,
            "to": ["x@example.test"],
            "subject": "Unfinished",
            "text": "second",
            "replaces": draft["id"],
        })),
    )
    .await?;
    assert_eq!(overview(&session).await?["counts"]["drafts"], json!(1));
    let drafts_list = list_messages(&session, "drafts", None).await?;
    assert_eq!(drafts_list.len(), 1);
    assert_eq!(drafts_list[0]["id"], draft2["id"]);
    assert_eq!(drafts_list[0]["draft"], json!(true));
    {
        let s = server.state.lock().unwrap();
        let dbox = s.mail.boxes.get("Drafts").unwrap();
        assert_eq!(dbox.messages.len(), 1);
        assert!(dbox.messages[0].flags.contains("\\Draft"));
    }

    // Markdown: sent with an HTML part made by Dar; the text part keeps the source.
    mail_call(
        &session,
        "POST",
        "/send",
        Some(&json!({
            "accountId": work_id,
            "to": ["lina@example.test"],
            "subject": "Plan",
            "text": "Hello **team**\n\n- one\n- [two](https://x.test)\n\n<script>x()</script>",
            "format": "markdown",
        })),
    )
    .await?;
    {
        let s = server.state.lock().unwrap();
        let md = &s.sent.last().unwrap().data;
        assert!(md.contains("multipart/alternative"), "{md}");
        assert!(md.to_ascii_lowercase().contains("x-dar-format: markdown"), "{md}");
        assert!(md.contains("<strong>team</strong>"), "{md}");
        assert!(md.contains(r#"<li><a href="https://x.test">two</a></li>"#), "{md}");
        assert!(md.contains("&lt;script&gt;"), "{md}");
        if let Some(script_pos) = md.find("<script>x()") {
            let html_pos = md.find("text/html").unwrap_or(0);
            assert!(script_pos < html_pos, "raw HTML stays text in the HTML part");
        }
    }
    let md_draft = mail_call(
        &session,
        "POST",
        "/drafts",
        Some(&json!({ "accountId": work_id, "to": [], "subject": "Notes", "text": "# Agenda\n\n*one*", "format": "markdown" })),
    )
    .await?;
    let md_body = mail_call(&session, "GET", &format!("/messages/{}", md_draft["id"].as_str().unwrap()), None).await?;
    assert_eq!(md_body["body"]["format"], json!("markdown"));
    assert_eq!(md_body["body"]["textPart"].as_str().unwrap_or_default().trim(), "# Agenda\n\n*one*");
    assert!(md_body["body"]["html"].as_str().unwrap_or_default().contains("<h1>Agenda</h1>"));

    assert_eq!(
        mail_raw(
            &session,
            "POST",
            "/send",
            Some(&json!({ "accountId": work_id, "to": ["a@example.test"], "subject": "x", "text": "y", "format": "rtf" })),
        )
        .await?
        .status,
        400
    );
    assert_eq!(
        mail_raw(
            &session,
            "POST",
            "/send",
            Some(&json!({ "accountId": work_id, "to": ["not an address"], "subject": "x", "text": "y" })),
        )
        .await?
        .status,
        400
    );

    // Delete from Trash removes for good.
    let trash = find_mailbox(&session, &work_id, "trash", None).await?;
    let trashed = list_messages(&session, "mailbox", trash["id"].as_str()).await?;
    mail_call(&session, "POST", "/messages/move", Some(&json!({ "ids": [trashed[0]["id"]], "role": "trash" }))).await?;
    assert_eq!(server.state.lock().unwrap().mail.boxes.get("Trash").unwrap().messages.len(), 0);

    // Search finds mail by subject and sender.
    let search = session.get_json("/api/v1/search?q=fresh").await?;
    assert!(search.to_string().contains("Fresh arrival"));

    // OAuth: Dar sends the browser to the provider; the code comes back without the session cookie.
    server.state.lock().unwrap().oauth.expires_in = 1;
    let start = mail_call(
        &session,
        "POST",
        "/oauth/start",
        Some(&json!({ "providerId": "test-oauth", "name": "OAuth", "address": "oauth@example.test", "displayName": "O" })),
    )
    .await?;
    let start_url = start["url"].as_str().context("oauth start url")?;
    assert!(start_url.contains("code_challenge_method=S256"), "{start_url}");
    assert!(start_url.contains("access_type=offline"), "{start_url}");

    // The browser's visit to the provider (trusting the fixture's CA).
    let callback = https_get_location(start_url, &server.ca_der).await?;
    assert!(callback.starts_with(&format!("{base}/api/v1/mail/oauth/callback?")), "{callback}");
    let back = Reply::from(session::client().get(&callback).send().await?).await?;
    assert_eq!(back.status, 303);
    let back_loc = back.headers.get("location").and_then(|v| v.to_str().ok()).unwrap_or_default();
    assert!(back_loc.starts_with("/mail?account="), "location={back_loc}");
    let replay = Reply::from(session::client().get(&callback).send().await?).await?;
    assert_eq!(
        replay.headers.get("location").and_then(|v| v.to_str().ok()),
        Some("/mail?oauthError=expired"),
        "a state works once"
    );

    let oauth_account = overview(&session)
        .await?
        .get("accounts")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|x| x["address"] == json!("oauth@example.test")))
        .cloned()
        .context("oauth account")?;
    let oauth_id = oauth_account["id"].as_str().unwrap().to_owned();
    sync_account(&session, &oauth_id).await?;
    let oauth_after = overview(&session)
        .await?
        .get("accounts")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|x| x["id"] == json!(oauth_id)))
        .cloned()
        .context("oauth account after sync")?;
    assert_eq!(oauth_after["status"], json!("ready"));
    {
        let s = server.state.lock().unwrap();
        assert!(s.oauth.token_requests.iter().any(|r| {
            r.get("grant_type").map(String::as_str) == Some("authorization_code")
                && r.get("client_secret").map(String::as_str) == Some(oauth_secret.as_str())
        }));
        assert!(
            s.oauth.token_requests.iter().any(|r| r.get("grant_type").map(String::as_str) == Some("refresh_token")),
            "an expired access token is refreshed"
        );
    }
    mail_call(
        &session,
        "POST",
        "/send",
        Some(&json!({ "accountId": oauth_id, "to": ["lina@example.test"], "subject": "Via OAuth", "text": "hi" })),
    )
    .await?;
    assert_eq!(server.state.lock().unwrap().sent.last().and_then(|m| m.user.as_deref()), Some("oauth@example.test"));

    // JMAP with an API token.
    {
        let mut s = server.state.lock().unwrap();
        s.jmap.add(
            "mi",
            build_mail_message(MailMessageSpec {
                subject: "JMAP hello",
                from: "Fastmail <hi@example.test>",
                ..Default::default()
            }),
            &[],
        );
        s.jmap.add(
            "ma",
            build_mail_message(MailMessageSpec {
                subject: "JMAP archived",
                ..Default::default()
            }),
            &["$seen"],
        );
    }
    let bad_token = mail_raw(
        &session,
        "POST",
        "/accounts",
        Some(&json!({ "providerId": "test-jmap", "name": "Personal", "address": "jmap@example.test", "token": "wrong" })),
    )
    .await?;
    assert_eq!(bad_token.status, 502);
    let jmap_account = mail_call(
        &session,
        "POST",
        "/accounts",
        Some(&json!({ "providerId": "test-jmap", "name": "Personal", "address": "jmap@example.test", "token": "jmap-test-token" })),
    )
    .await?;
    let jmap_id = jmap_account["id"].as_str().unwrap().to_owned();
    sync_account(&session, &jmap_id).await?;
    let jmap_inbox = find_mailbox(&session, &jmap_id, "inbox", None).await?;
    let jmap_messages = list_messages(&session, "mailbox", jmap_inbox["id"].as_str()).await?;
    assert_eq!(
        jmap_messages.iter().filter_map(|m| m["subject"].as_str()).collect::<Vec<_>>(),
        vec!["JMAP hello"]
    );
    let jmap_msg_id = jmap_messages[0]["id"].as_str().unwrap().to_owned();
    mail_call(&session, "POST", "/messages/flags", Some(&json!({ "ids": [jmap_msg_id], "seen": true }))).await?;
    assert!(server.state.lock().unwrap().jmap.find_by_text("JMAP hello").unwrap().keywords.contains("$seen"));
    mail_call(&session, "POST", "/messages/move", Some(&json!({ "ids": [jmap_msg_id], "role": "archive" }))).await?;
    assert!(server.state.lock().unwrap().jmap.find_by_text("JMAP hello").unwrap().mailbox_ids.get("ma").copied() == Some(true));
    mail_call(
        &session,
        "POST",
        "/send",
        Some(&json!({ "accountId": jmap_id, "to": ["lina@example.test"], "subject": "From JMAP", "text": "hello" })),
    )
    .await?;
    {
        let s = server.state.lock().unwrap();
        assert_eq!(s.jmap.submissions.len(), 1);
        assert!(s.jmap.submissions[0].data.contains("Subject: From JMAP"));
        assert_eq!(s.jmap.submissions[0].envelope["rcptTo"], json!([{ "email": "lina@example.test" }]));
        assert!(s.jmap.calls.iter().any(|c| c == "EmailSubmission/set"));
    }
    assert!(!list_messages(&session, "inbox", None).await?.iter().any(|m| m["subject"] == json!("JMAP hello")));

    // Isolation: another account sees none of this and cannot manage providers.
    session
        .post(
            "/api/v1/auth/users",
            &json!({ "username": "other", "displayName": "Other", "password": "another long isolated password" }),
        )
        .await?
        .expect(201)?;
    let other = login_as(&base, "other", "another long isolated password").await?;
    let other_overview = overview(&other).await?;
    assert_eq!(other_overview["accounts"], json!([]));
    assert_eq!(other_overview["administrator"], json!(false));
    assert_eq!(mail_raw(&other, "GET", &format!("/messages/{reply_id}"), None).await?.status, 404);
    assert_eq!(mail_raw(&other, "GET", "/admin/providers", None).await?.status, 403);
    assert_eq!(
        mail_raw(&other, "POST", "/messages/flags", Some(&json!({ "ids": [reply_id], "seen": false }))).await?.status,
        404
    );

    // App passwords cannot use Mail.
    let app_res = Reply::from(
        session::client()
            .get(format!("{base}/api/v1/mail/overview"))
            .header("authorization", &dav_auth)
            .send()
            .await?,
    )
    .await?;
    assert_ne!(app_res.status, 200);

    // A provider removed from the host configuration stops its accounts with a reason.
    let existing: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&providers_file)?)?;
    let reduced: Vec<Value> = existing.into_iter().filter(|p| p["id"] != json!("test-jmap")).collect();
    std::fs::write(&providers_file, serde_json::to_string(&reduced)?)?;
    runtime.restart().await?;
    let again = login(&base).await?;
    let _ = mail_raw(&again, "POST", "/sync", Some(&json!({ "id": jmap_id }))).await?;
    let after_restart = overview(&again).await?;
    assert!(!after_restart["providers"].as_array().unwrap().iter().any(|p| p["id"] == json!("test-jmap")));
    let stopped = after_restart["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == json!(jmap_id))
        .context("stopped jmap account")?;
    assert_eq!(stopped["providerAvailable"], json!(false));
    assert!(stopped["lastError"].as_str().unwrap_or_default().contains("turned off"));
    // Mirrored mail stays readable without the server.
    assert_eq!(mail_raw(&again, "GET", &format!("/messages/{reply_id}"), None).await?.status, 200);

    // Removing an account keeps its files unless asked.
    assert_eq!(mail_raw(&again, "DELETE", &format!("/accounts/{jmap_id}?keepFiles=1"), None).await?.status, 200);
    let keep_check = Reply::from(
        session::client()
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), format!("{base}/dav/files/admin/docs/mail/Personal/"))
            .header("authorization", &dav_auth)
            .header("depth", "0")
            .send()
            .await?,
    )
    .await?;
    assert_eq!(keep_check.status, 207);

    // The administrator's switch turns the API off.
    let apps = again.get_json("/api/v1/appSettings").await?;
    let rev = apps["revision"].as_i64().context("appSettings revision")?;
    let off = Reply::from(
        again
            .request("PUT", "/api/v1/admin/installedApps")
            .header("if-match", format!("\"apps:{rev}\""))
            .json(&json!({ "id": "mail", "enabled": false, "revision": rev }))
            .send()
            .await?,
    )
    .await?;
    ensure!(off.status < 300, "disable mail: {} {}", off.status, off.text());
    assert_eq!(mail_raw(&again, "GET", "/overview", None).await?.status, 404);

    runtime.close().await;
    server.close();
    Ok(())
}
