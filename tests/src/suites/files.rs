//! Files on real storage (`files.integration.test.ts`): immutable uploads, ranges, revisions,
//! versions, copies, cycles, trash and restore; isolation between people, grants, live
//! revocation and public links; storage limits.

use dar_acceptance::prelude::*;
use dar_acceptance::session::Upload;
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "files",
    cases: &[case!(immutable_upload_and_trash, 40), case!(isolation_grants_and_public_links, 40), case!(storage_limits, 40)],
};

async fn status(session: &Session, url: &str) -> Result<u16> {
    Ok(session.get(url).await?.status)
}

async fn text(session: &Session, url: &str) -> Result<String> {
    Ok(session.get(url).await?.text())
}

pub async fn immutable_upload_and_trash(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let session = login(&base).await?;
    let db = session.connect().await?;
    let name = format!("proof-{}.txt", uuid());
    let first = upload(&session, &name, "hello 🌍 from immutable storage").await?;
    let content_url = format!("/api/v1/files/{}/content", first.file_id);
    let content = session.get(&content_url).await?;
    assert_eq!(content.text(), "hello 🌍 from immutable storage");
    assert!(content.header("content-disposition").unwrap_or_default().contains("attachment"));
    let partial = Reply::from(session.request("GET", &content_url).header("range", "bytes=0-4").send().await?).await?;
    assert_eq!(partial.status, 206);
    assert_eq!(partial.text(), "hello");
    let head = Reply::from(session.request("HEAD", &content_url).send().await?).await?;
    assert_eq!(head.header("content-length"), Some("hello 🌍 from immutable storage".len().to_string().as_str()));
    let revision = || db.db.my_files().iter().find(|row| row.id == first.file_id).map(|row| row.revision);
    assert_eq!(revision(), Some(1));
    reducer(|done| db.reducers.rename_entry_then(first.file_id.clone(), format!("renamed-{name}"), 1, done)).await?;
    assert!(reducer(|done| db.reducers.rename_entry_then(first.file_id.clone(), "stale".into(), 1, done)).await.is_err());
    let second = upload_with(&session, &name, "replacement", Upload { file_id: Some(&first.file_id), expected_revision: Some(2), ..Upload::default() }).await?;
    assert_eq!(text(&session, &content_url).await?, "replacement");
    reducer(|done| db.reducers.restore_version_then(first.file_id.clone(), first.id.clone(), 3, done)).await?;
    assert_eq!(text(&session, &content_url).await?, "hello 🌍 from immutable storage");
    assert_eq!(text(&session, &format!("{content_url}?version={}", second.id)).await?, "replacement");
    let folder_id = uuid();
    reducer(|done| db.reducers.create_folder_then(folder_id.clone(), String::new(), format!("folder-{}", uuid()), done)).await?;
    assert!(reducer(|done| db.reducers.move_entry_then(folder_id.clone(), folder_id.clone(), 1, None, done)).await.is_err());
    reducer(|done| db.reducers.copy_entry_then(first.file_id.clone(), uuid(), folder_id.clone(), format!("copy-{name}"), 4, done)).await?;
    reducer(|done| db.reducers.trash_entry_then(first.file_id.clone(), 4, done)).await?;
    assert_eq!(status(&session, &content_url).await?, 404);
    reducer(|done| db.reducers.browse_then(String::new(), "trash".into(), String::new(), String::new(), done)).await?;
    assert_eq!(db.db.my_files().iter().find(|row| row.id == first.file_id).map(|row| row.trashed), Some(true));
    reducer(|done| db.reducers.restore_entry_then(first.file_id.clone(), 5, done)).await?;
    assert_eq!(text(&session, &content_url).await?, "hello 🌍 from immutable storage");
    reducer(|done| db.reducers.trash_entry_then(first.file_id.clone(), 6, done)).await?;
    reducer(|done| db.reducers.delete_entry_then(first.file_id.clone(), 7, done)).await?;
    assert_eq!(status(&session, &content_url).await?, 404);
    let empty = upload(&session, &format!("empty-{}.txt", uuid()), "").await?;
    assert_eq!(session.get(&format!("/api/v1/files/{}/content", empty.file_id)).await?.header("content-length"), Some("0"));
    let unicode = upload(&session, &format!("🌿 café-{}.txt", uuid()), "Unicode filenames stay visible.").await?;
    reducer(|done| db.reducers.browse_then(String::new(), "files".into(), "🌿".into(), String::new(), done)).await?;
    assert!(db.db.my_files().iter().any(|row| row.id == unicode.file_id));
    Ok(())
}

pub async fn isolation_grants_and_public_links(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let owner = login(&base).await?;
    let db = owner.connect().await?;
    let username = format!("user-{}", &uuid()[..8]);
    owner.post("/api/v1/auth/users", &json!({"username": username, "displayName": "Sharing recipient", "password": "a separate recipient password"})).await?.expect(201)?;
    let session = login_as(&base, &username, "a separate recipient password").await?;
    let recipient = session.connect().await?;
    let file = upload(&owner, &format!("sharing-{}.txt", uuid()), "private content").await?;
    let url = format!("/api/v1/files/{}/content", file.file_id);
    assert_eq!(status(&session, &url).await?, 404);
    let grant_id = uuid();
    reducer(|done| db.reducers.grant_access_then(grant_id.clone(), file.file_id.clone(), username.clone(), "read".into(), 1, done)).await?;
    reducer(|done| recipient.reducers.browse_then(String::new(), "shared".into(), String::new(), String::new(), done)).await?;
    assert!(recipient.db.my_files().iter().any(|row| row.id == file.file_id));
    assert_eq!(text(&session, &url).await?, "private content");
    assert!(reducer(|done| recipient.reducers.rename_entry_then(file.file_id.clone(), "not permitted".into(), 2, done)).await.is_err());
    let (removed, gone) = tokio::sync::oneshot::channel();
    let removed = std::sync::Mutex::new(Some(removed));
    let watched = file.file_id.clone();
    recipient.db.my_files().on_delete(move |_, row| if row.id == watched && let Some(sender) = removed.lock().unwrap().take() { let _ = sender.send(()); });
    reducer(|done| db.reducers.revoke_grant_then(grant_id.clone(), done)).await?;
    tokio::time::timeout(Duration::from_secs(10), gone).await.context("the revoked file was not removed live")?.ok();
    let denied = procedure(|done| recipient.procedures.file_details_then(file.file_id.clone(), done)).await;
    assert!(denied.err().is_some_and(|e| e.to_string().ends_with(": notFound")));
    assert_eq!(status(&session, &url).await?, 404);
    let (secret, digest) = support::create_opaque_token();
    let share_id = uuid();
    let expires = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() + 60_000) as f64;
    reducer(|done| db.reducers.create_public_share_then(share_id.clone(), file.file_id.clone(), digest.clone(), expires, 2, done)).await?;
    let public_url = format!("{base}/public/files/{secret}");
    assert_eq!(session::client().get(&public_url).send().await?.text().await?, "private content");
    reducer(|done| db.reducers.revoke_public_share_then(share_id.clone(), done)).await?;
    assert_eq!(session::client().get(&public_url).send().await?.status().as_u16(), 404);
    for parent_id in [String::new(), uuid()] {
        if !parent_id.is_empty() {
            reducer(|done| db.reducers.create_folder_then(parent_id.clone(), String::new(), format!("private-parent-{}", uuid()), done)).await?;
        }
        let direct = upload_with(&owner, &format!("write-{}.txt", uuid()), "original", Upload { parent_id: &parent_id, ..Upload::default() }).await?;
        let grant = uuid();
        reducer(|done| db.reducers.grant_access_then(grant.clone(), direct.file_id.clone(), username.clone(), "write".into(), 1, done)).await?;
        upload_with(&session, "ignored.txt", "edited", Upload { parent_id: &parent_id, file_id: Some(&direct.file_id), expected_revision: Some(2), ..Upload::default() }).await?;
        assert_eq!(text(&owner, &format!("/api/v1/files/{}/content", direct.file_id)).await?, "edited");
        let reservation = session.post("/api/v1/files/uploads", &json!({"fileId": direct.file_id, "expectedRevision": 3, "parentId": parent_id, "name": "ignored.txt", "size": 6, "mediaType": "text/plain"})).await?.expect(201)?;
        let pending = reservation.json()?["id"].as_str().unwrap_or_default().to_owned();
        reducer(|done| db.reducers.revoke_grant_then(grant.clone(), done)).await?;
        let put = Reply::from(session.request("PUT", &format!("/api/v1/files/uploads/{pending}")).body("denied").send().await?).await?;
        assert_eq!(put.status, 403);
        assert!(upload_with(&session, "ignored.txt", "denied", Upload { parent_id: &parent_id, file_id: Some(&direct.file_id), expected_revision: Some(3), ..Upload::default() }).await.is_err());
    }
    let forged = session::client().post(format!("{base}/api/v1/files/uploads")).header("cookie", &owner.cookie).header("origin", "https://attacker.example")
        .header("content-type", "application/json").body("{}").send().await?;
    assert_eq!(forged.status().as_u16(), 403);
    Ok(())
}

pub async fn storage_limits(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let admin = IdentityAdmin::new(&base).await?;
    let users = admin.get("/users").await?;
    let id = users["items"].as_array().and_then(|items| items.iter().find(|u| u["userName"] == "admin")).and_then(|u| u["id"].as_str()).context("admin user")?.to_owned();
    let path = format!("/users/{id}/storage");
    let meter = || async {
        let storage = admin.get("/storage").await?;
        storage["items"].as_array().and_then(|items| items.iter().find(|row| row["userId"] == id.as_str()).cloned()).context("storage row")
    };
    let reserve = |size: u64| { let body = json!({"name": format!("quota-{}.txt", uuid()), "size": size, "mediaType": "text/plain", "parentId": ""}); let session = admin.session.clone(); async move { session.post("/api/v1/files/uploads", &body).await } };
    assert_eq!(meter().await?["quota"], 0);
    assert_eq!(admin.api(&path, "PUT", Some(&json!({"quota": 1024}))).await?.status, 403);
    assert_eq!(admin.write(&path, "PUT", Some(&json!({"quota": -1})), None).await?.status, 400);
    let used = meter().await?["used"].as_u64().unwrap_or_default();
    admin.write(&path, "PUT", Some(&json!({"quota": used + 10})), None).await?.expect(200)?;
    assert_eq!(meter().await?["quota"], used + 10);
    assert_eq!(reserve(100).await?.status, 507);
    assert_eq!(admin.write(&path, "PUT", Some(&json!({"quota": null})), None).await?.status, 200);
    assert_eq!(meter().await?["quota"], 0);
    upload(&admin.session, &format!("unlimited-{}.txt", uuid()), "x".repeat(100)).await?;
    Ok(())
}
