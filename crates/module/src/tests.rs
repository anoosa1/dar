//! Module tests on SpacetimeDB's in-process test datastore (no server, no live data).

use spacetimedb::test_utils::{TestAuth, TestContext};
use spacetimedb::{ReducerContext, Table};

use crate::auth::*;
use crate::files::*;
use crate::global_search::service_global_search;
use crate::pim::service_pim;
use crate::tables::*;

const ACCOUNT: &str = "11111111-1111-4111-8111-111111111111";
const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BLOB: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

fn setup() -> TestContext {
    let test = TestContext::new().unwrap();
    // November 18, 2026: file times from 2023 are in the past.
    test.clock
        .set(spacetimedb::Timestamp::from_micros_since_unix_epoch(
            1_795_000_000_000_000,
        ));
    test.set_env("DAR_ISSUER", "https://dar.test/auth/issuer")
        .unwrap();
    test.with_reducer_tx(TestAuth::internal(), |ctx| {
        crate::init(ctx);
        Ok::<_, String>(())
    })
    .unwrap();
    test
}

fn tx<T>(
    test: &TestContext,
    body: impl FnOnce(&ReducerContext) -> Result<T, String>,
) -> Result<T, String> {
    test.with_reducer_tx(TestAuth::internal(), body)
}

/// A procedure's JSON, or the code it aborted with (`lib.rs` `abort`: a panic named after it).
fn call(
    test: &TestContext,
    body: impl FnOnce(&mut spacetimedb::ProcedureContext) -> String,
) -> Result<serde_json::Value, String> {
    let mut ctx = test.procedure_context(TestAuth::internal());
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&mut ctx)));
    std::panic::set_hook(hook);
    match result {
        Ok(text) => Ok(serde_json::from_str(&text).unwrap()),
        Err(payload) => Err(payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default()),
    }
}

/// An account with a v2 Files read/write app password; returns its protocol session.
fn account_with_password(test: &TestContext, scope: &str) -> String {
    tx(test, |ctx| {
        service_create_account(
            ctx,
            ACCOUNT.into(),
            "amina".into(),
            "Amina".into(),
            "verifier".into(),
            true,
        )
    })
    .unwrap();
    let session_id = "22222222-2222-4222-8222-222222222222";
    tx(test, |ctx| {
        let expires = crate::now(ctx) + 3_600_000.0;
        service_create_session(
            ctx,
            session_id.into(),
            ACCOUNT.into(),
            "browser-digest".into(),
            "csrf".into(),
            expires,
            "verifier".into(),
        )
    })
    .unwrap();
    tx(test, |ctx| {
        service_create_app_password(
            ctx,
            session_id.into(),
            "pw-1".into(),
            DIGEST.into(),
            "Laptop".into(),
            scope.into(),
        )
    })
    .unwrap();
    let write = scope.contains("files:write");
    let auth = call(test, |ctx| {
        service_protocol_authenticate(ctx, "Amina".into(), DIGEST.into(), write)
    })
    .unwrap();
    assert_eq!(auth["username"], "amina");
    auth["sessionId"].as_str().unwrap().to_owned()
}

fn dav_call(
    test: &TestContext,
    session: &str,
    method: &str,
    path: &str,
    destination: &str,
    revision: u32,
    depth: u32,
) -> Result<serde_json::Value, String> {
    let (s, m, p, d) = (
        session.to_owned(),
        method.to_owned(),
        path.to_owned(),
        destination.to_owned(),
    );
    call(test, move |ctx| {
        service_dav(ctx, s, m, p, d, revision, true, depth, 0.0)
    })
}

#[test]
fn only_the_service_identity_may_call_service_operations() {
    let test = setup();
    assert_eq!(call(&test, service_status).unwrap()["initialized"], false);
    let other = TestAuth::from_jwt_payload(
        r#"{"iss":"https://elsewhere.test","sub":"x","aud":["x"],"iat":0,"exp":9999999999}"#,
        spacetimedb::ConnectionId::ZERO,
    );
    let auth = other.expect("a well-formed client token");
    let result = test.with_reducer_tx(auth, |ctx| {
        service_create_account(
            ctx,
            ACCOUNT.into(),
            "amina".into(),
            "Amina".into(),
            "v".into(),
            true,
        )
    });
    assert_eq!(result, Err("forbidden".into()));
}

#[test]
fn accounts_sessions_and_app_passwords() {
    let test = setup();
    assert_eq!(
        tx(&test, |ctx| service_create_account(
            ctx,
            ACCOUNT.into(),
            "Bad Name".into(),
            "A".into(),
            "v".into(),
            true
        )),
        Err("invalidInput".into())
    );
    let session = account_with_password(&test, "v2 files:read files:write");
    assert_eq!(call(&test, service_status).unwrap()["initialized"], true);
    // A second "first" account is refused.
    assert_eq!(
        tx(&test, |ctx| service_create_account(
            ctx,
            "33333333-3333-4333-8333-333333333333".into(),
            "other".into(),
            "O".into(),
            "v".into(),
            true
        )),
        Err("conflict".into())
    );
    // Wrong digest, wrong user, read-only scope.
    assert!(
        call(&test, |ctx| service_protocol_authenticate(
            ctx,
            "amina".into(),
            "b".repeat(64),
            false
        ))
        .unwrap_err()
        .contains("unauthenticated")
    );
    assert!(
        call(&test, |ctx| service_pim_authenticate(
            ctx,
            "amina".into(),
            DIGEST.into(),
            "contacts".into(),
            false
        ))
        .unwrap_err()
        .contains("forbidden")
    );
    let listed = call(&test, |ctx| {
        service_app_passwords(ctx, "22222222-2222-4222-8222-222222222222".into())
    })
    .unwrap();
    assert_eq!(listed[0]["label"], "Laptop");

    // Changing the password revokes every session and app password.
    tx(&test, |ctx| {
        service_change_password(ctx, ACCOUNT.into(), "new".into(), "verifier".into())
    })
    .unwrap();
    assert!(
        call(&test, |ctx| service_protocol_authenticate(
            ctx,
            "amina".into(),
            DIGEST.into(),
            false
        ))
        .unwrap_err()
        .contains("unauthenticated")
    );
    assert!(
        dav_call(&test, &session, "STAT", "", "", 0, 0)
            .unwrap_err()
            .contains("unauthenticated")
    );
}

#[test]
fn webdav_operations() {
    let test = setup();
    let session = account_with_password(&test, "v2 files:read files:write");
    // MKCOL, then a missing parent conflicts.
    assert_eq!(
        dav_call(&test, &session, "MKCOL", "Docs", "", 0, 0).unwrap()["created"],
        true
    );
    assert_eq!(
        dav_call(&test, &session, "MKCOL", "Docs", "", 0, 0).unwrap()["created"],
        false
    );
    assert!(
        dav_call(&test, &session, "MKCOL", "Missing/Child", "", 0, 0)
            .unwrap_err()
            .contains("conflict")
    );

    // PUT: prepare, reserve, finish with the content's digest.
    let target = dav_call(&test, &session, "PREPARE", "Docs/Note.md", "", 0, 0).unwrap();
    assert!(target["entry"].is_null());
    let parent_id = target["parentId"].as_str().unwrap().to_owned();
    let (s, p) = (session.clone(), parent_id.clone());
    let upload = call(&test, move |ctx| {
        service_begin_upload(
            ctx,
            s,
            "44444444-4444-4444-8444-444444444444".into(),
            "55555555-5555-4555-8555-555555555555".into(),
            p,
            "Note.md".into(),
            5.0,
            "text/markdown".into(),
            0,
        )
    })
    .unwrap();
    assert_eq!(upload["state"], "uploading");
    tx(&test, |ctx| {
        service_finish_upload(
            ctx,
            session.clone(),
            "44444444-4444-4444-8444-444444444444".into(),
            BLOB.into(),
            1_700_000_000_000.0,
        )
    })
    .unwrap();
    // Finishing again with the same digest is idempotent.
    tx(&test, |ctx| {
        service_finish_upload(
            ctx,
            session.clone(),
            "44444444-4444-4444-8444-444444444444".into(),
            BLOB.into(),
            0.0,
        )
    })
    .unwrap();

    let listing = dav_call(&test, &session, "PROPFIND", "", "", 0, 2).unwrap();
    let paths: Vec<&str> = listing["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["Docs", "Docs/Note.md"]);
    let shallow = dav_call(&test, &session, "PROPFIND", "", "", 0, 1).unwrap();
    assert_eq!(shallow["children"].as_array().unwrap().len(), 1);
    let note = dav_call(&test, &session, "STAT", "docs/note.md", "", 0, 0).unwrap();
    assert_eq!(
        (
            note["blobId"].as_str(),
            note["revision"].as_u64(),
            note["modifiedAt"].as_f64()
        ),
        (Some(BLOB), Some(1), Some(1_700_000_000_000.0)),
        "names match case-insensitively"
    );

    // Downloads read the stored blob; usage counts it.
    let file_id = note["id"].as_str().unwrap().to_owned();
    let download = call(&test, |ctx| {
        service_download(ctx, session.clone(), file_id.clone(), String::new())
    })
    .unwrap();
    assert_eq!(download["blobId"], BLOB);
    tx(&test, |ctx| {
        assert_eq!(
            ctx.db
                .usage()
                .user_id()
                .find(ACCOUNT.to_owned())
                .unwrap()
                .used,
            5.0
        );
        assert!(ctx.db.file_blobs().id().find(BLOB.to_owned()).is_some());
        Ok(())
    })
    .unwrap();

    // Stale revisions conflict; COPY then MOVE onto it replaces (the old one goes to the trash).
    assert!(
        dav_call(&test, &session, "MOVE", "Docs/Note.md", "Moved.md", 7, 0)
            .unwrap_err()
            .contains("conflict")
    );
    assert_eq!(
        dav_call(&test, &session, "COPY", "Docs/Note.md", "Copy.md", 1, 0).unwrap()["replaced"],
        false
    );
    assert_eq!(
        dav_call(&test, &session, "MOVE", "Docs/Note.md", "Copy.md", 1, 0).unwrap()["replaced"],
        true
    );
    let root = dav_call(&test, &session, "PROPFIND", "", "", 0, 2).unwrap();
    let paths: Vec<&str> = root["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["Copy.md", "Docs"]);
    tx(&test, |ctx| {
        assert_eq!(
            ctx.db
                .usage()
                .user_id()
                .find(ACCOUNT.to_owned())
                .unwrap()
                .used,
            10.0,
            "copies count toward the quota"
        );
        assert_eq!(
            ctx.db.file_entries().iter().filter(|e| e.trashed).count(),
            1
        );
        Ok(())
    })
    .unwrap();

    // TOUCH sets the client's modified time; DELETE trashes.
    let copy = dav_call(&test, &session, "STAT", "Copy.md", "", 0, 0).unwrap();
    let (s, revision) = (session.clone(), copy["revision"].as_u64().unwrap() as u32);
    let touched = call(&test, move |ctx| {
        service_dav(
            ctx,
            s,
            "TOUCH".into(),
            "Copy.md".into(),
            String::new(),
            0,
            true,
            0,
            1_600_000_000_000.0,
        )
    })
    .unwrap();
    assert_eq!(touched["revision"].as_u64().unwrap() as u32, revision + 1);
    assert_eq!(
        dav_call(&test, &session, "DELETE", "Copy.md", "", revision + 1, 0).unwrap()["ok"],
        true
    );
    assert!(
        dav_call(&test, &session, "STAT", "Copy.md", "", 0, 0)
            .unwrap_err()
            .contains("notFound")
    );
    assert!(
        dav_call(&test, &session, "STAT", "../x", "", 0, 0)
            .unwrap_err()
            .contains("invalidName")
    );
}

#[test]
fn read_only_passwords_cannot_write() {
    let test = setup();
    let session = account_with_password(&test, "v2 files:read");
    assert!(dav_call(&test, &session, "PROPFIND", "", "", 0, 1).is_ok());
    assert!(
        dav_call(&test, &session, "MKCOL", "Docs", "", 0, 0)
            .unwrap_err()
            .contains("forbidden")
    );
}

const BROWSER: &str = "22222222-2222-4222-8222-222222222222";

/// A browser tab acting for the account's browser session (what `connected` records).
fn tab(test: &TestContext, byte: u8, session_id: &str) -> spacetimedb::Identity {
    let identity = spacetimedb::Identity::from_byte_array([byte; 32]);
    tx(test, |ctx| {
        ctx.db.principals().insert(PrincipalRow {
            identity,
            session_id: session_id.into(),
        });
        Ok(())
    })
    .unwrap();
    identity
}

fn as_tab(identity: spacetimedb::Identity) -> TestAuth {
    TestAuth::Authenticated {
        jwt_payload: "{}".into(),
        connection_id: spacetimedb::ConnectionId::ZERO,
        sender: identity,
    }
}

fn browser_account(test: &TestContext, id: &str, username: &str, session: &str, first: bool) {
    tx(test, |ctx| {
        service_create_account(
            ctx,
            id.into(),
            username.into(),
            username.into(),
            "verifier".into(),
            first,
        )
    })
    .unwrap();
    tx(test, |ctx| {
        let expires = crate::now(ctx) + 3_600_000.0;
        service_create_session(
            ctx,
            session.into(),
            id.into(),
            format!("digest-{session}"),
            "csrf".into(),
            expires,
            "verifier".into(),
        )
    })
    .unwrap();
}

fn revision_of(test: &TestContext, id: &str) -> u32 {
    tx(test, |ctx| {
        Ok(ctx
            .db
            .file_entries()
            .id()
            .find(id.to_owned())
            .unwrap()
            .revision)
    })
    .unwrap()
}

fn names(rows: &[FileEntry]) -> Vec<&str> {
    rows.iter().map(|r| r.name.as_str()).collect()
}

#[test]
fn browsing_favorites_sharing_and_details() {
    use crate::browse::*;
    let test = setup();
    browser_account(&test, ACCOUNT, "amina", BROWSER, true);
    let me = tab(&test, 7, BROWSER);
    let folder = "66666666-6666-4666-8666-666666666666";
    let mine = |test: &TestContext| my_files(&test.view_context(as_tab(me)));
    test.with_reducer_tx(as_tab(me), |ctx| {
        create_folder(ctx, folder.into(), String::new(), "Docs".into())
    })
    .unwrap();
    test.with_reducer_tx(as_tab(me), |ctx| {
        create_folder(
            ctx,
            "77777777-7777-4777-8777-777777777777".into(),
            folder.into(),
            "Inner".into(),
        )
    })
    .unwrap();
    assert_eq!(
        names(&mine(&test)),
        ["Docs"],
        "a tab that never browsed sees All files"
    );
    test.with_reducer_tx(as_tab(me), |ctx| {
        browse(
            ctx,
            folder.into(),
            "files".into(),
            String::new(),
            String::new(),
        )
    })
    .unwrap();
    assert_eq!(names(&mine(&test)), ["Inner"]);
    assert_eq!(
        test.with_reducer_tx(as_tab(me), |ctx| browse(
            ctx,
            String::new(),
            "pictures".into(),
            String::new(),
            String::new()
        )),
        Err("invalidInput".into())
    );

    // Favorites list the entry by name; the view follows the tab's section.
    let before = revision_of(&test, folder);
    test.with_reducer_tx(as_tab(me), |ctx| {
        favorite_entry(ctx, folder.into(), true, before)
    })
    .unwrap();
    assert_eq!(
        test.with_reducer_tx(as_tab(me), |ctx| favorite_entry(
            ctx,
            folder.into(),
            false,
            before
        )),
        Err("conflict".into()),
        "stale revisions conflict"
    );
    test.with_reducer_tx(as_tab(me), |ctx| {
        browse(
            ctx,
            String::new(),
            "favorites".into(),
            String::new(),
            String::new(),
        )
    })
    .unwrap();
    let favorites = mine(&test);
    assert_eq!(
        (names(&favorites), favorites[0].favorite),
        (vec!["Docs"], true)
    );

    // Details: role, ancestors, versions and grants, for the owner only.
    let mut ctx = test.procedure_context(as_tab(me));
    let details: serde_json::Value = serde_json::from_str(&file_details(
        &mut ctx,
        "77777777-7777-4777-8777-777777777777".into(),
    ))
    .unwrap();
    assert_eq!(
        (
            details["role"].as_str(),
            details["ancestors"][0]["name"].as_str(),
            details["entry"]["listKey"].is_string()
        ),
        (Some("owner"), Some("Docs"), true)
    );

    // Sharing: the recipient's Shared section lists it; revoking removes it.
    let other = "33333333-3333-4333-8333-333333333333";
    browser_account(
        &test,
        other,
        "bilal",
        "44444444-4444-4444-8444-444444444444",
        false,
    );
    let them = tab(&test, 9, "44444444-4444-4444-8444-444444444444");
    let grant = "88888888-8888-4888-8888-888888888888";
    let revision = revision_of(&test, folder);
    test.with_reducer_tx(as_tab(me), |ctx| {
        grant_access(
            ctx,
            grant.into(),
            folder.into(),
            "Bilal".into(),
            "read".into(),
            revision,
        )
    })
    .unwrap();
    test.with_reducer_tx(as_tab(them), |ctx| {
        browse(
            ctx,
            String::new(),
            "shared".into(),
            String::new(),
            String::new(),
        )
    })
    .unwrap();
    assert_eq!(names(&my_files(&test.view_context(as_tab(them)))), ["Docs"]);
    let mut ctx = test.procedure_context(as_tab(them));
    let shared: serde_json::Value =
        serde_json::from_str(&file_details(&mut ctx, folder.into())).unwrap();
    assert_eq!(
        (
            shared["role"].as_str(),
            shared["grants"].as_array().map(Vec::len)
        ),
        (Some("read"), Some(0)),
        "only owners see grants"
    );
    assert_eq!(
        test.with_reducer_tx(as_tab(them), |ctx| revoke_grant(ctx, grant.into())),
        Err("forbidden".into())
    );
    test.with_reducer_tx(as_tab(me), |ctx| revoke_grant(ctx, grant.into()))
        .unwrap();
    assert!(my_files(&test.view_context(as_tab(them))).is_empty());

    // Search finds names the caller can read, plus matching app names.
    let found = call(&test, |ctx| {
        service_global_search(ctx, BROWSER.into(), "doc".into())
    })
    .unwrap();
    assert_eq!(found["items"][0]["title"], "Docs");
    let other_search = call(&test, |ctx| {
        service_global_search(
            ctx,
            "44444444-4444-4444-8444-444444444444".into(),
            "doc".into(),
        )
    })
    .unwrap();
    assert!(
        other_search["items"].as_array().unwrap().is_empty(),
        "revoked shares are not searchable"
    );
}

#[test]
fn locked_folder_preferences_and_public_links() {
    use crate::browse::*;
    let test = setup();
    browser_account(&test, ACCOUNT, "amina", BROWSER, true);
    let me = tab(&test, 7, BROWSER);
    // The Locked folder opens only with the account's current verifier, for this session.
    let state = call(&test, |ctx| {
        service_locked_folder(ctx, BROWSER.into(), "state".into(), String::new())
    })
    .unwrap();
    assert_eq!(
        (state["folderId"].as_str(), state["unlockedUntil"].as_f64()),
        (Some(""), Some(0.0))
    );
    assert_eq!(
        call(&test, |ctx| service_locked_folder(
            ctx,
            BROWSER.into(),
            "unlock".into(),
            "old".into()
        )),
        Err("conflict".into())
    );
    let open = call(&test, |ctx| {
        service_locked_folder(ctx, BROWSER.into(), "unlock".into(), "verifier".into())
    })
    .unwrap();
    let locked = open["folderId"].as_str().unwrap().to_owned();
    assert!(open["unlockedUntil"].as_f64().unwrap() > 0.0);
    test.with_reducer_tx(as_tab(me), |ctx| {
        browse(
            ctx,
            locked.clone(),
            "files".into(),
            String::new(),
            String::new(),
        )
    })
    .unwrap();
    test.with_reducer_tx(as_tab(me), |ctx| {
        create_folder(
            ctx,
            "99999999-9999-4999-8999-999999999999".into(),
            locked.clone(),
            "Private".into(),
        )
    })
    .unwrap();
    assert_eq!(
        names(&my_files(&test.view_context(as_tab(me)))),
        ["Private"]
    );
    call(&test, |ctx| {
        service_locked_folder(ctx, BROWSER.into(), "lock".into(), String::new())
    })
    .unwrap();
    assert!(
        my_files(&test.view_context(as_tab(me))).is_empty(),
        "a locked folder shows nothing"
    );
    test.with_reducer_tx(as_tab(me), |ctx| {
        browse(
            ctx,
            String::new(),
            "files".into(),
            String::new(),
            String::new(),
        )
    })
    .unwrap();
    assert!(
        my_files(&test.view_context(as_tab(me))).is_empty(),
        "the Locked folder is outside All files"
    );

    // Preferences save at the current revision only (service_pim, as in TypeScript).
    let save = |revision: u32, value: &str| {
        serde_json::json!({ "revision": revision, "value": value }).to_string()
    };
    call(&test, |ctx| {
        service_pim(
            ctx,
            BROWSER.into(),
            "savePreferences".into(),
            save(0, r#"{"filesView":"grid"}"#),
        )
    })
    .unwrap();
    assert_eq!(
        call(&test, |ctx| service_pim(
            ctx,
            BROWSER.into(),
            "savePreferences".into(),
            save(0, "{}")
        )),
        Err("conflict".into())
    );
    let saved = call(&test, |ctx| {
        service_pim(ctx, BROWSER.into(), "preferences".into(), "{}".into())
    })
    .unwrap();
    assert_eq!(
        (
            saved["value"]["revision"].as_u64(),
            saved["value"]["value"].as_str()
        ),
        (Some(1), Some(r#"{"filesView":"grid"}"#))
    );

    // A public link downloads its file until revoked.
    let file = "55555555-5555-4555-8555-555555555555";
    let begin = call(&test, |ctx| {
        service_begin_upload(
            ctx,
            BROWSER.into(),
            "44444444-4444-4444-8444-444444444444".into(),
            file.into(),
            String::new(),
            "a.txt".into(),
            5.0,
            "text/plain".into(),
            0,
        )
    })
    .unwrap();
    assert_eq!(begin["state"], "uploading");
    tx(&test, |ctx| {
        service_finish_upload(
            ctx,
            BROWSER.into(),
            "44444444-4444-4444-8444-444444444444".into(),
            BLOB.into(),
            0.0,
        )
    })
    .unwrap();
    let expires = 1_795_000_000_000.0 + 86_400_000.0;
    test.with_reducer_tx(as_tab(me), |ctx| {
        create_public_share(
            ctx,
            "12121212-1212-4212-8212-121212121212".into(),
            file.into(),
            DIGEST.into(),
            expires,
            1,
        )
    })
    .unwrap();
    assert_eq!(
        call(&test, |ctx| service_public_download(ctx, DIGEST.into())).unwrap()["blobId"],
        BLOB
    );
    test.with_reducer_tx(as_tab(me), |ctx| {
        revoke_public_share(ctx, "12121212-1212-4212-8212-121212121212".into())
    })
    .unwrap();
    assert_eq!(
        call(&test, |ctx| service_public_download(ctx, DIGEST.into())),
        Err("notFound".into())
    );
}
