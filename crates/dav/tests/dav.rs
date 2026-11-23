//! WebDAV, CalDAV and CardDAV over HTTP against isolated in-memory backends.

use std::sync::Arc;

use base64::Engine as _;
use dar_dav::memory::{MemoryAccounts, MemoryFiles, MemoryPim};
use dar_dav::pim::PimKind;
use dar_dav::{App, Dav};
use reqwest::{Method, StatusCode};

const SECRET: &str = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG";
const READ_ONLY: &str = "zyxwvutsrqponmlkjihgfedcba9876543210ZYXWVUT";

struct Harness {
    base: String,
    http: reqwest::Client,
}

async fn start() -> Harness {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let accounts = Arc::new(MemoryAccounts::default());
    accounts.add("amina", SECRET, &[App::Files, App::Contacts, App::Calendar], true);
    accounts.add("amina", READ_ONLY, &[App::Files], false);
    let pim = MemoryPim::new(accounts.clone());
    pim.add_collection("amina", "cal-1", PimKind::Calendar, "Personal");
    pim.add_collection("amina", "book-1", PimKind::Contacts, "Contacts");
    let dav = Dav { origin: base.clone(), accounts: accounts.clone(), files: Arc::new(MemoryFiles::new(accounts.clone())), pim: Arc::new(pim) };
    tokio::spawn(async move { axum::serve(listener, dar_dav::router(dav)).await.unwrap() });
    Harness { base, http: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap() }
}

impl Harness {
    fn request(&self, method: &str, path: &str, secret: &str) -> reqwest::RequestBuilder {
        let auth = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("amina:{secret}")));
        self.http.request(Method::from_bytes(method.as_bytes()).unwrap(), format!("{}{path}", self.base)).header("authorization", auth)
    }
    async fn send(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (StatusCode, reqwest::header::HeaderMap, String) {
        let mut r = self.request(method, path, SECRET);
        for (k, v) in headers {
            r = r.header(*k, *v);
        }
        let response = r.body(body.to_owned()).send().await.unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        (status, headers, response.text().await.unwrap())
    }
}

#[tokio::test]
async fn files_round_trip() {
    let h = start().await;
    let root = "/dav/files/amina";
    // Unauthenticated requests are challenged for the Files realm.
    let response = h.http.request(Method::from_bytes(b"PROPFIND").unwrap(), format!("{}{root}/", h.base)).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers()["www-authenticate"].to_str().unwrap().contains("Dar Files"));
    // Another user's tree is off limits.
    assert_eq!(h.send("PROPFIND", "/dav/files/other/", &[("depth", "0")], "").await.0, StatusCode::FORBIDDEN);

    assert_eq!(h.send("MKCOL", &format!("{root}/Docs"), &[], "").await.0, StatusCode::CREATED);
    assert_eq!(h.send("MKCOL", &format!("{root}/Docs"), &[], "").await.0, StatusCode::METHOD_NOT_ALLOWED, "already there");
    assert_eq!(h.send("MKCOL", &format!("{root}/Missing/Child"), &[], "").await.0, StatusCode::CONFLICT);

    let (status, headers, _) = h.send("PUT", &format!("{root}/Docs/n%C3%B6te.md"), &[("x-oc-mtime", "1700000000")], "hello").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(headers["x-oc-mtime"], "accepted");
    let etag = headers["etag"].to_str().unwrap().to_owned();
    assert!(etag.ends_with("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824\""), "content-addressed ETag: {etag}");

    let (status, headers, body) = h.send("GET", &format!("{root}/Docs/n%C3%B6te.md"), &[], "").await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "hello"));
    assert_eq!(headers["content-type"], "text/markdown");
    let (status, _, body) = h.send("GET", &format!("{root}/Docs/n%C3%B6te.md"), &[("range", "bytes=1-3")], "").await;
    assert_eq!((status, body.as_str()), (StatusCode::PARTIAL_CONTENT, "ell"));

    // Conditional writes.
    assert_eq!(h.send("PUT", &format!("{root}/Docs/n%C3%B6te.md"), &[("if-none-match", "*")], "x").await.0, StatusCode::PRECONDITION_FAILED);
    assert_eq!(h.send("PUT", &format!("{root}/Docs/n%C3%B6te.md"), &[("if-match", "\"stale\"")], "x").await.0, StatusCode::PRECONDITION_FAILED);
    assert_eq!(h.send("PUT", &format!("{root}/Docs/n%C3%B6te.md"), &[("if-match", &etag), ("x-oc-mtime", "1700000000")], "hello again").await.0, StatusCode::NO_CONTENT);

    let (status, _, xml) = h.send("PROPFIND", &format!("{root}/"), &[("depth", "infinity")], "").await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(xml.contains("<d:href>/dav/files/amina/Docs/n%C3%B6te.md</d:href>"), "{xml}");
    assert!(xml.contains("<d:getcontentlength>11</d:getcontentlength>"));
    assert!(xml.contains("<d:getlastmodified>Tue, 14 Nov 2023 22:13:20 GMT</d:getlastmodified>"));
    assert!(xml.contains("<oc:checksum>SHA256:"));
    let (_, _, shallow) = h.send("PROPFIND", &format!("{root}/"), &[("depth", "1")], "").await;
    assert!(shallow.contains("/Docs/<") && !shallow.contains("te.md"));

    // PROPPATCH sets the modified time; unknown properties fail together with 424.
    let patch = r#"<d:propertyupdate xmlns:d="DAV:"><d:set><d:prop><d:lastmodified>1600000000</d:lastmodified></d:prop></d:set></d:propertyupdate>"#;
    let (status, _, xml) = h.send("PROPPATCH", &format!("{root}/Docs/n%C3%B6te.md"), &[], patch).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(xml.contains("HTTP/1.1 200 OK"));
    let mixed = r#"<d:propertyupdate xmlns:d="DAV:" xmlns:z="urn:z"><d:set><d:prop><d:lastmodified>1600000001</d:lastmodified><z:color>red</z:color></d:prop></d:set></d:propertyupdate>"#;
    let (_, _, xml) = h.send("PROPPATCH", &format!("{root}/Docs/n%C3%B6te.md"), &[], mixed).await;
    assert!(xml.contains("424 Failed Dependency") && xml.contains("403 Forbidden"), "{xml}");

    // LOCK on an unmapped URL creates an empty file.
    let lock = r#"<d:lockinfo xmlns:d="DAV:"><d:lockscope><d:exclusive/></d:lockscope><d:locktype><d:write/></d:locktype></d:lockinfo>"#;
    let (status, headers, _) = h.send("LOCK", &format!("{root}/Docs/new.txt"), &[("timeout", "Second-60")], lock).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(headers["lock-token"].to_str().unwrap().starts_with("<opaquelocktoken:"));
    assert_eq!(h.send("UNLOCK", &format!("{root}/Docs/new.txt"), &[("lock-token", "<opaquelocktoken:x>")], "").await.0, StatusCode::NO_CONTENT);

    // COPY, MOVE (with and without overwrite), DELETE.
    let destination = format!("{}{root}/Copy.md", h.base);
    assert_eq!(h.send("COPY", &format!("{root}/Docs/n%C3%B6te.md"), &[("destination", &destination)], "").await.0, StatusCode::CREATED);
    assert_eq!(h.send("COPY", &format!("{root}/Docs/n%C3%B6te.md"), &[("destination", &destination), ("overwrite", "F")], "").await.0, StatusCode::PRECONDITION_FAILED);
    assert_eq!(h.send("MOVE", &format!("{root}/Docs"), &[("destination", &format!("{root}/Archive"))], "").await.0, StatusCode::CREATED);
    assert_eq!(h.send("GET", &format!("{root}/Archive/n%C3%B6te.md"), &[], "").await.2, "hello again");
    assert_eq!(h.send("MOVE", &format!("{root}/Archive"), &[("destination", "https://elsewhere.example/dav/files/amina/X")], "").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(h.send("DELETE", &format!("{root}/Archive"), &[], "").await.0, StatusCode::NO_CONTENT);
    assert_eq!(h.send("GET", &format!("{root}/Archive/n%C3%B6te.md"), &[], "").await.0, StatusCode::NOT_FOUND);
    assert_eq!(h.send("GET", &format!("{root}/..%2Fx"), &[], "").await.0, StatusCode::BAD_REQUEST);

    // A read-only app password reads but does not write.
    assert_eq!(h.request("PROPFIND", &format!("{root}/"), READ_ONLY).header("depth", "0").send().await.unwrap().status(), StatusCode::MULTI_STATUS);
    assert_eq!(h.request("PUT", &format!("{root}/ro.txt"), READ_ONLY).body("x").send().await.unwrap().status(), StatusCode::FORBIDDEN);
    // Browser pages on other origins are refused.
    assert_eq!(h.send("PROPFIND", &format!("{root}/"), &[("origin", "https://evil.example")], "").await.0, StatusCode::FORBIDDEN);
}

const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//test//EN\r\nBEGIN:VEVENT\r\nUID:weekly-1\r\nDTSTAMP:20260101T000000Z\r\nDTSTART;TZID=Europe/Berlin:20260105T090000\r\nDTEND;TZID=Europe/Berlin:20260105T100000\r\nRRULE:FREQ=WEEKLY;COUNT=4\r\nSUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
const TASK: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//test//EN\r\nBEGIN:VTODO\r\nUID:task-1\r\nDTSTAMP:20260101T000000Z\r\nDUE:20260301T120000Z\r\nSUMMARY:File taxes\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
const CARD: &str = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:card-1\r\nFN:Jane Doe\r\nN:Doe;Jane;;;\r\nEMAIL:jane@example.test\r\nEND:VCARD\r\n";

#[tokio::test]
async fn caldav_and_carddav() {
    let h = start().await;
    // Discovery, as DAVx5 and Apple clients do it.
    let (status, headers, _) = h.send("GET", "/.well-known/caldav", &[], "").await;
    assert_eq!((status, headers["location"].to_str().unwrap()), (StatusCode::PERMANENT_REDIRECT, "/dav/"));
    let ask = r#"<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:current-user-principal/><c:calendar-home-set/><d:owner/></d:prop></d:propfind>"#;
    let (status, _, xml) = h.send("PROPFIND", "/dav/", &[("depth", "0")], ask).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(xml.contains("<d:href>/dav/principals/amina/</d:href>"), "{xml}");
    assert!(xml.contains("<c:calendar-home-set><d:href>/dav/calendars/amina/</d:href>"));
    assert!(xml.contains("404 Not Found") && xml.contains("owner"));
    let (_, _, xml) = h.send("PROPFIND", "/dav/", &[("depth", "1")], "").await;
    assert!(xml.contains("/dav/files/amina/"), "the DAV root lists the Files home");

    let (_, _, xml) = h.send("PROPFIND", "/dav/calendars/amina/", &[("depth", "1")], "").await;
    assert!(xml.contains("<d:href>/dav/calendars/amina/cal-1/</d:href>") && xml.contains("<c:calendar/>") && xml.contains("<cs:getctag>"), "{xml}");
    assert_eq!(h.send("PROPFIND", "/dav/calendars/amina/", &[("depth", "infinity")], "").await.0, StatusCode::FORBIDDEN);

    // Writes need preconditions.
    let cal = "/dav/calendars/amina/cal-1";
    assert_eq!(h.send("PUT", &format!("{cal}/standup.ics"), &[], EVENT).await.0, StatusCode::PRECONDITION_REQUIRED);
    let (status, headers, _) = h.send("PUT", &format!("{cal}/standup.ics"), &[("if-none-match", "*")], EVENT).await;
    assert_eq!(status, StatusCode::CREATED);
    let etag = headers["etag"].to_str().unwrap().to_owned();
    assert_eq!(h.send("PUT", &format!("{cal}/task.ics"), &[("if-none-match", "*")], TASK).await.0, StatusCode::CREATED);
    assert_eq!(h.send("PUT", &format!("{cal}/bad.ics"), &[("if-none-match", "*")], "BEGIN:VCARD\r\nEND:VCARD\r\n").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(h.send("PUT", &format!("{cal}/dupe.ics"), &[("if-none-match", "*")], EVENT).await.0, StatusCode::CONFLICT, "one resource per UID");
    let changed_uid = EVENT.replace("UID:weekly-1", "UID:other");
    assert_eq!(h.send("PUT", &format!("{cal}/standup.ics"), &[("if-match", &etag)], &changed_uid).await.0, StatusCode::BAD_REQUEST);

    let query = |start: &str, end: &str, comp: &str| {
        format!(r#"<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="{comp}"><c:time-range start="{start}" end="{end}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#)
    };
    // Third occurrence (Jan 19, 08:00 UTC in winter) is found; after COUNT nothing is.
    let (status, _, xml) = h.send("REPORT", &format!("{cal}/"), &[("depth", "1")], &query("20260119T080000Z", "20260119T083000Z", "VEVENT")).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(xml.contains("standup.ics") && !xml.contains("task.ics"), "{xml}");
    let (_, _, xml) = h.send("REPORT", &format!("{cal}/"), &[], &query("20260202T000000Z", "20260210T000000Z", "VEVENT")).await;
    assert!(!xml.contains("standup.ics"), "{xml}");
    let (_, _, xml) = h.send("REPORT", &format!("{cal}/"), &[], &query("20260301T000000Z", "20260302T000000Z", "VTODO")).await;
    assert!(xml.contains("task.ics") && !xml.contains("standup.ics"));
    assert_eq!(h.send("REPORT", &format!("{cal}/"), &[], &query("bad", "20260302T000000Z", "VEVENT")).await.0, StatusCode::BAD_REQUEST);

    // calendar-multiget returns data and 404s for unknown members.
    let multiget = format!(r#"<c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop><d:href>{cal}/standup.ics</d:href><d:href>{cal}/gone.ics</d:href></c:calendar-multiget>"#);
    let (_, _, xml) = h.send("REPORT", &format!("{cal}/"), &[], &multiget).await;
    assert!(xml.contains("SUMMARY:Standup") && xml.contains("gone.ics</d:href><d:status>HTTP/1.1 404"), "{xml}");

    assert_eq!(h.send("DELETE", &format!("{cal}/standup.ics"), &[("if-match", "\"nope\"")], "").await.0, StatusCode::PRECONDITION_FAILED);
    assert_eq!(h.send("DELETE", &format!("{cal}/standup.ics"), &[("if-match", &etag)], "").await.0, StatusCode::NO_CONTENT);

    // CardDAV: text-match filters.
    let book = "/dav/addressbooks/amina/book-1";
    assert_eq!(h.send("PUT", &format!("{book}/jane.vcf"), &[("if-none-match", "*")], CARD).await.0, StatusCode::CREATED);
    let (status, headers, body) = h.send("GET", &format!("{book}/jane.vcf"), &[], "").await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, CARD));
    assert!(headers["content-type"].to_str().unwrap().starts_with("text/vcard"));
    let find = |name: &str, text: &str, kind: &str| {
        format!(r#"<a:addressbook-query xmlns:d="DAV:" xmlns:a="urn:ietf:params:xml:ns:carddav"><d:prop><d:getetag/></d:prop><a:filter><a:prop-filter name="{name}"><a:text-match match-type="{kind}">{text}</a:text-match></a:prop-filter></a:filter></a:addressbook-query>"#)
    };
    let (_, _, xml) = h.send("REPORT", &format!("{book}/"), &[], &find("N", "jane", "equals")).await;
    assert!(xml.contains("jane.vcf"), "{xml}");
    let (_, _, xml) = h.send("REPORT", &format!("{book}/"), &[], &find("EMAIL", "@nowhere", "ends-with")).await;
    assert!(!xml.contains("jane.vcf"));
    assert_eq!(h.send("REPORT", &format!("{book}/"), &[], &find("EMAIL", "x", "regex")).await.0, StatusCode::FORBIDDEN);
}
