//! Books (`books.integration.test.ts`): library indexing, metadata, covers, EPUB/comic/PDF
//! content, reading state revisions & conflicts, bookmarks, account isolation, preferences,
//! admin app toggle, and backup persistence.

use super::photos_books_apps_support::{cbz, create_user, epub, pdf, png, run_cmd};
use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::time::Duration;

pub const SUITE: Suite = Suite { name: "books", cases: &[case!(books_library_and_access, 120)] };

async fn books_get(session: &Session, route: &str) -> Result<Reply> {
    session.get(&format!("/api/v1/books{route}")).await
}

async fn books_json(session: &Session, route: &str) -> Result<Value> {
    let reply = books_get(session, route).await?;
    ensure!(reply.status == 200, "{route}: {} {}", reply.status, reply.text());
    reply.json()
}

async fn wait_library(session: &Session) -> Result<Value> {
    for _ in 0..40 {
        let value = books_json(session, "/library").await?;
        if value["indexing"] != json!(true) && value["indexing"].as_u64().unwrap_or(0) == 0 {
            return Ok(value);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bail!("Books never finished reading the library.")
}

fn encrypted_pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
        "<< /Filter /Standard /V 1 /R 2 /O <000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F> /U <1111111111111111111111111111111111111111111111111111111111111111> /P -4 >>",
    ];
    let mut body = String::from("%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(body.len());
        body.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let xref = body.len();
    body.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
    for offset in offsets {
        body.push_str(&format!("{offset:010} 00000 n \n"));
    }
    body.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R /Encrypt 4 0 R /ID [<0123456789ABCDEF0123456789ABCDEF> <0123456789ABCDEF0123456789ABCDEF>] >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    body.into_bytes()
}

pub async fn books_library_and_access(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let base = instance.base.clone();
    let session = login(&base).await?;
    let db = session.connect().await?;

    let first = books_json(&session, "/library").await?;
    assert_eq!(first["path"], "docs/books", "the default folder is docs/books");
    let root_id = first["rootId"].as_str().context("rootId")?.to_owned();

    let comics_id = uuid();
    reducer(|done| db.reducers.create_folder_then(comics_id.clone(), root_id.clone(), "Comics".into(), done)).await?;

    fn up_in(parent: &str) -> Upload<'_> {
        Upload { parent_id: parent, ..Upload::default() }
    }
    let voyage = upload_with(
        &session,
        "the-test-voyage.epub",
        epub("The Test Voyage", "Ishmael Fixture", Some(("Fixture Series", 2)), false),
        up_in(&root_id),
    )
    .await?;
    let locked = upload_with(
        &session,
        "locked.epub",
        epub("Locked Away", "Store", None, true),
        up_in(&root_id),
    )
    .await?;
    let comic = upload_with(&session, "fixture-comic.cbz", cbz(), up_in(&comics_id)).await?;
    let paper = upload_with(&session, "paper.pdf", pdf(), up_in(&root_id)).await?;
    upload_with(&session, "old-format.mobi", "not a book we open", up_in(&root_id)).await?;

    // A tar comic goes through bsdtar, like CBR and CB7.
    let work = t.temp("dar-books")?;
    let pages_dir = work.join("pages");
    std::fs::create_dir_all(&pages_dir)?;
    for (n, color) in [(1, [10, 10, 10]), (2, [90, 90, 90])] {
        std::fs::write(pages_dir.join(format!("{n}.png")), png(30, 45, color))?;
    }
    let cbt_path = work.join("comic.cbt");
    run_cmd("bsdtar", &["-cf", &cbt_path.to_string_lossy(), "-C", &work.to_string_lossy(), "pages"]).await?;
    upload_with(&session, "tar-comic.cbt", std::fs::read(&cbt_path)?, up_in(&comics_id)).await?;

    let value = wait_library(&session).await?;
    let books = value["books"].as_array().context("books array")?;
    let find = |name: &str| books.iter().find(|b| b["name"] == name).unwrap_or_else(|| panic!("missing book {name}"));

    let mut names: Vec<&str> = books.iter().filter_map(|b| b["name"].as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["fixture-comic.cbz", "locked.epub", "paper.pdf", "tar-comic.cbt", "the-test-voyage.epub"],
        "only book formats are listed"
    );
    let libraries: Vec<(&str, u64)> = value["libraries"]
        .as_array()
        .context("libraries")?
        .iter()
        .map(|l| (l["name"].as_str().unwrap_or_default(), l["count"].as_u64().unwrap_or_default()))
        .collect();
    assert_eq!(libraries, vec![("Comics", 2)], "top folders are libraries");

    let book = find("the-test-voyage.epub");
    assert_eq!(book["title"], "The Test Voyage");
    assert_eq!(book["authors"], json!(["Ishmael Fixture"]));
    assert_eq!(book["series"], "Fixture Series");
    assert_eq!(book["seriesIndex"], 2);
    assert_eq!(book["language"], "en");
    assert_eq!(book["publisher"], "Fixture Press");
    assert_eq!(book["description"], "A test voyage.");
    assert_eq!(book["subjects"], json!(["Sea stories"]));
    assert_eq!(book["cover"], json!(true));
    assert_eq!(book["error"], "");
    assert_eq!(find("locked.epub")["error"], "drm", "an encrypted EPUB is reported as DRM");

    let comic_book = find("fixture-comic.cbz");
    assert_eq!(comic_book["pages"], 3);
    assert_eq!(comic_book["rightToLeft"], json!(true));
    assert_eq!(comic_book["authors"], json!(["Ada Panel"]));
    assert_eq!(comic_book["series"], "Fixture Comics");
    assert_eq!(comic_book["library"], comics_id.as_str());
    assert_eq!(find("tar-comic.cbt")["pages"], 2, "bsdtar unpacks tar comics");

    let paper_book = find("paper.pdf");
    assert_eq!(paper_book["pages"], 2);
    assert_eq!(paper_book["title"], "A Fixture Paper");
    assert_eq!(paper_book["authors"], json!(["Grace Columns"]));
    assert_eq!(paper_book["cover"], json!(true));
    assert_eq!(paper_book["error"], "");

    for id in [&voyage.file_id, &comic.file_id, &paper.file_id] {
        let cover = books_get(&session, &format!("/{id}/cover")).await?;
        assert_eq!(cover.status, 200, "cover for {id}");
        assert_eq!(cover.header("content-type"), Some("image/webp"));
        assert!(cover.body.len() > 20);
        assert_eq!(&cover.body[..4], b"RIFF", "webp RIFF header for {id}");
        assert_eq!(&cover.body[8..12], b"WEBP", "webp WEBP signature for {id}");
    }

    // EPUB: manifest, chapters with a strict policy, no way out of the archive.
    let detail = books_json(&session, &format!("/{}", voyage.file_id)).await?;
    assert_eq!(detail["manifest"]["kind"], "epub");
    let spine: Vec<&str> = detail["manifest"]["spine"]
        .as_array()
        .context("spine")?
        .iter()
        .filter_map(|s| s["href"].as_str())
        .collect();
    assert_eq!(spine, vec!["OEBPS/chapter1.xhtml", "OEBPS/chapter2.xhtml"]);
    let toc: Vec<(&str, u64, u64)> = detail["manifest"]["toc"]
        .as_array()
        .context("toc")?
        .iter()
        .map(|entry| {
            (
                entry["label"].as_str().unwrap_or_default(),
                entry["spine"].as_u64().unwrap_or_default(),
                entry["depth"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(toc, vec![("One: Departure", 0, 0), ("Two: The Storm", 1, 0), ("The mast breaks", 1, 1)]);
    assert_eq!(detail["manifest"]["toc"][2]["href"], "OEBPS/chapter2.xhtml#p3");

    let chapter = books_get(&session, &format!("/{}/epub/r1/OEBPS/chapter1.xhtml", voyage.file_id)).await?;
    assert_eq!(chapter.status, 200);
    assert_eq!(chapter.header("content-type"), Some("application/xhtml+xml"));
    let policy = chapter.header("content-security-policy").unwrap_or_default();
    assert!(policy.contains("script-src 'none'"), "CSP: {policy}");
    assert!(policy.contains("form-action 'none'"), "CSP: {policy}");
    assert!(policy.contains("frame-ancestors 'self'"), "CSP: {policy}");
    assert!(chapter.text().contains("Paragraph 1 of Departure"));
    let as_html = books_get(&session, &format!("/{}/epub/r1/OEBPS/chapter1.xhtml?as=html", voyage.file_id)).await?;
    assert_eq!(as_html.header("content-type"), Some("text/html; charset=utf-8"));
    for bad in [
        "OEBPS/../../../etc/passwd",
        "OEBPS%2F..%2F..%2Fsecret",
        "..%2FMETA-INF%2Fcontainer.xml",
        "OEBPS/missing.xhtml",
    ] {
        assert_eq!(books_get(&session, &format!("/{}/epub/r1/{bad}", voyage.file_id)).await?.status, 404, "{bad}");
    }
    assert_eq!(
        books_get(&session, &format!("/{}/epub/r1/OEBPS/chapter1.xhtml", locked.file_id)).await?.status,
        200,
        "DRM is reported, not enforced by refusing bytes the owner already has"
    );

    // Comics and PDF.
    let page = books_get(&session, &format!("/{}/pages/0", comic.file_id)).await?;
    assert_eq!(page.status, 200);
    assert_eq!(page.header("content-type"), Some("image/png"));
    assert_eq!(&page.body[..4], &[137, 80, 78, 71]);
    assert_eq!(books_get(&session, &format!("/{}/pages/3", comic.file_id)).await?.status, 404);
    assert_eq!(books_get(&session, &format!("/{}/pages/0", paper.file_id)).await?.status, 404);
    assert_eq!(books_json(&session, &format!("/{}", comic.file_id)).await?["manifest"]["kind"], "comic");
    assert_eq!(books_json(&session, &format!("/{}", paper.file_id)).await?["manifest"]["kind"], "pdf");

    // Damaged and password-protected PDFs are isolated in the child process and reported cleanly.
    let damaged_pdf = upload_with(&session, "broken_file.pdf", "not a pdf", up_in(&root_id)).await?;
    let protected_pdf = upload_with(&session, "locked_paper.pdf", encrypted_pdf(), up_in(&root_id)).await?;
    let after_bad = wait_library(&session).await?;
    let after_books = after_bad["books"].as_array().context("books array")?;
    let damaged_entry = after_books.iter().find(|b| b["id"] == damaged_pdf.file_id.as_str()).context("broken_file.pdf")?;
    assert_eq!(damaged_entry["title"], "broken file");
    assert_eq!(damaged_entry["error"], "damaged");
    assert_eq!(damaged_entry["cover"], json!(false));
    assert_eq!(books_get(&session, &format!("/{}/cover", damaged_pdf.file_id)).await?.status, 404);

    let protected_entry = after_books.iter().find(|b| b["id"] == protected_pdf.file_id.as_str()).context("locked_paper.pdf")?;
    assert_eq!(protected_entry["title"], "locked paper");
    assert_eq!(protected_entry["error"], "drm");
    assert_eq!(protected_entry["cover"], json!(false));
    assert_eq!(books_get(&session, &format!("/{}/cover", protected_pdf.file_id)).await?.status, 404);

    // Reading state: revisions, conflicts carry the newer state, lists.
    let put_state = |id: &str, body: Value| {
        let session = session.clone();
        let path = format!("/api/v1/books/{id}/state");
        async move { session.put(&path, &body).await }
    };
    let saved = put_state(
        &voyage.file_id,
        json!({"revision": 0, "status": "reading", "position": {"spine": 1, "fraction": 0.5}, "progress": 0.6, "device": "Test browser"}),
    )
    .await?
    .expect(200)?
    .json()?;
    assert_eq!(saved["revision"], 1);
    assert_eq!(saved["position"], json!({"spine": 1, "fraction": 0.5}));

    let conflict = put_state(&voyage.file_id, json!({"revision": 0, "position": {"spine": 0, "fraction": 0.1}, "progress": 0.05})).await?;
    assert_eq!(conflict.status, 409, "a stale place never overwrites a newer one");
    assert_eq!(conflict.json()?["state"]["progress"], 0.6, "the conflict returns the newer place");

    for bad in [
        json!({"revision": 1, "status": "maybe"}),
        json!({"revision": 1, "position": {"spine": 1, "page": 2, "fraction": 0}}),
        json!({"revision": 1, "progress": 2}),
        json!({"revision": 1, "extra": true}),
    ] {
        assert_eq!(put_state(&voyage.file_id, bad.clone()).await?.status, 400, "{bad}");
    }
    assert_eq!(put_state(&paper.file_id, json!({"revision": 0, "status": "want"})).await?.status, 200);
    assert_eq!(put_state(&uuid(), json!({"revision": 0, "status": "want"})).await?.status, 404);

    let listed = wait_library(&session).await?;
    let listed_books = listed["books"].as_array().context("listed books")?;
    assert_eq!(listed_books.iter().find(|b| b["id"] == voyage.file_id.as_str()).unwrap()["state"]["status"], "reading");
    assert_eq!(listed_books.iter().find(|b| b["id"] == paper.file_id.as_str()).unwrap()["state"]["status"], "want");

    // Bookmarks.
    let mark_id = uuid();
    let mark = session
        .post(
            &format!("/api/v1/books/{}/marks", voyage.file_id),
            &json!({"id": mark_id, "position": {"spine": 1, "fraction": 0.25}, "label": "The storm"}),
        )
        .await?
        .expect(201)?
        .json()?;
    assert_eq!(mark["label"], "The storm");
    assert_eq!(books_json(&session, &format!("/{}", voyage.file_id)).await?["marks"].as_array().map(Vec::len), Some(1));
    assert_eq!(session.delete(&format!("/api/v1/books/{}/marks/{mark_id}", voyage.file_id)).await?.status, 200);
    assert_eq!(books_json(&session, &format!("/{}", voyage.file_id)).await?["marks"].as_array().map(Vec::len), Some(0));

    // Another account sees none of it; the owner's state and books stay private.
    let username = format!("reader-{}", &uuid()[..6]);
    create_user(&session, &username, "Other reader", PASSWORD).await?;
    let other = login_as(&base, &username, PASSWORD).await?;
    for route in [
        format!("/{}", voyage.file_id),
        format!("/{}/cover", voyage.file_id),
        format!("/{}/epub/r1/OEBPS/chapter1.xhtml", voyage.file_id),
        format!("/{}/pages/0", comic.file_id),
    ] {
        let status = books_get(&other, &route).await?.status;
        assert!([403, 404].contains(&status), "{route}: {status}");
    }
    let other_put = other.put(&format!("/api/v1/books/{}/state", voyage.file_id), &json!({"revision": 0, "status": "want"})).await?;
    assert!([403, 404].contains(&other_put.status));
    assert_eq!(books_json(&other, "/library").await?["books"].as_array().map(Vec::len), Some(0));

    // A changed Books folder is followed; preferences validate it.
    let current = session.get_json("/api/v1/pim/preferences").await?;
    assert_eq!(current["booksFolder"].as_str().unwrap_or("docs/books"), "docs/books");
    let put_prefs = |value: Value, revision: u64| {
        let session = session.clone();
        async move {
            let mut body = value;
            body["revision"] = json!(revision);
            Reply::from(
                session
                    .request("PUT", "/api/v1/pim/preferences")
                    .header("if-match", format!("\"preferences:{revision}\""))
                    .json(&body)
                    .send()
                    .await?,
            )
            .await
        }
    };
    let rev = current["revision"].as_u64().context("preferences revision")?;
    assert_eq!(put_prefs(json!({"booksFolder": "../escape"}), rev).await?.status, 400);
    assert_eq!(put_prefs(json!({"booksTheme": "neon"}), rev).await?.status, 400);
    assert_eq!(put_prefs(json!({"booksFolder": "Reading/Shelf", "booksTheme": "sepia", "booksFontSize": 20}), rev).await?.status, 200);
    assert_eq!(books_json(&session, "/library").await?["path"], "Reading/Shelf");
    let rev = session.get_json("/api/v1/pim/preferences").await?["revision"].as_u64().context("preferences revision")?;
    assert_eq!(put_prefs(json!({"booksFolder": "docs/books"}), rev).await?.status, 200);

    // Administrators can turn Books off; the data stays.
    let state = session.get_json("/api/v1/admin/installedApps").await?;
    let books_app = state["items"]
        .as_array()
        .and_then(|items| items.iter().find(|i| i["id"] == "books"))
        .context("books app entry")?;
    assert_eq!(books_app["enabled"], json!(true));
    assert_eq!(books_app["canToggle"], json!(true));
    let toggle = |enabled: bool, revision: u64| {
        let session = session.clone();
        async move {
            Reply::from(
                session
                    .request("PUT", "/api/v1/admin/installedApps")
                    .header("if-match", format!("\"apps:{revision}\""))
                    .json(&json!({"id": "books", "enabled": enabled, "revision": revision}))
                    .send()
                    .await?,
            )
            .await
        }
    };
    assert_eq!(toggle(false, state["revision"].as_u64().context("apps revision")?).await?.status, 200);
    assert_eq!(books_get(&session, "/library").await?.status, 404);
    assert_eq!(books_get(&session, &format!("/{}", voyage.file_id)).await?.status, 404);
    let state = session.get_json("/api/v1/admin/installedApps").await?;
    assert_eq!(toggle(true, state["revision"].as_u64().context("apps revision")?).await?.status, 200);
    let restored = books_json(&session, "/library").await?;
    assert_eq!(
        restored["books"]
            .as_array()
            .and_then(|arr| arr.iter().find(|b| b["id"] == voyage.file_id.as_str()))
            .and_then(|b| b["state"]["progress"].as_f64()),
        Some(0.6)
    );

    // Backups carry reading state.
    let service = instance.database()?;
    let pin = uuid();
    let snapshot = service.call("service_backup", json!([pin])).await;
    let _ = service.call("service_release_backup", json!([pin])).await;
    let snapshot = snapshot?;
    assert_eq!(snapshot["format"], "dar-backup-v12");
    assert!(
        snapshot["books"]["states"]
            .as_array()
            .is_some_and(|states| states.iter().any(|s| s["fileId"] == voyage.file_id.as_str()))
    );

    db.disconnect();
    Ok(())
}
