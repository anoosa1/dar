//! Content addressing (`blob-migration.integration.test.ts`): a backup from before content was
//! named by its digest restores, and the next start names the content by digest.

use dar_acceptance::prelude::*;
use std::collections::HashMap;
use std::path::Path;

pub const SUITE: Suite = Suite { name: "blob-migration", cases: &[case!(legacy_backup_migrates_to_digests, 60)] };

/// Rewrites a v11 backup as Dar wrote backups before content addressing: content named by
/// upload ID. Returns each digest's new name.
fn as_legacy_backup(directory: &Path) -> Result<HashMap<String, String>> {
    let mut snapshot: Value = serde_json::from_str(&dar_acceptance::read_to_string(directory.join("snapshot.json"))?)?;
    let mut by_digest: HashMap<String, String> = HashMap::new();
    let (mut uploads, mut blobs) = (Vec::new(), Vec::new());
    let owners: HashMap<String, String> = snapshot["files"].as_array().into_iter().flatten()
        .map(|file| (file["id"].as_str().unwrap_or_default().to_owned(), file["ownerId"].as_str().unwrap_or_default().to_owned())).collect();
    // Every uploaded version had its own upload ID, even for identical content.
    for version in snapshot["versions"].as_array_mut().context("versions")? {
        let (id, digest) = (uuid(), version["blobId"].as_str().unwrap_or_default().to_owned());
        by_digest.entry(digest.clone()).or_insert_with(|| id.clone());
        std::fs::copy(directory.join("blobs").join(&digest), directory.join("blobs").join(&id))?;
        uploads.push(json!({"id": id, "sessionId": "", "ownerId": owners.get(version["fileId"].as_str().unwrap_or_default()).cloned().unwrap_or_default(),
            "parentId": "", "fileId": version["fileId"], "name": "", "size": version["size"], "mediaType": version["mediaType"], "expectedRevision": 0,
            "expiresAt": 0, "state": "complete", "digest": digest, "expiresKey": "0000000000000000"}));
        blobs.push(json!({"id": id, "size": version["size"], "digest": digest}));
        version["blobId"] = json!(id);
    }
    for digest in by_digest.keys() {
        std::fs::remove_file(directory.join("blobs").join(digest))?;
    }
    fn rename(value: &mut Value, by_digest: &HashMap<String, String>) {
        match value {
            Value::Array(items) => items.iter_mut().for_each(|item| rename(item, by_digest)),
            Value::Object(map) => for (key, item) in map.iter_mut() {
                match item.as_str().and_then(|s| by_digest.get(s)) {
                    Some(new) if key == "blobId" => *item = json!(new),
                    _ => rename(item, by_digest),
                }
            },
            _ => {}
        }
    }
    let versions = snapshot.as_object_mut().unwrap().remove("versions").unwrap_or_default();
    rename(&mut snapshot, &by_digest);
    let legacy = snapshot.as_object_mut().unwrap();
    legacy.insert("format".into(), json!("dar-backup-v10"));
    legacy.insert("versions".into(), versions);
    legacy.insert("blobs".into(), json!(blobs));
    legacy.insert("uploads".into(), json!(uploads));
    let text = serde_json::to_string(&snapshot)?;
    std::fs::write(directory.join("snapshot.json"), &text)?;
    std::fs::write(directory.join("complete.json"), json!({"format": "dar-backup-v1", "snapshotDigest": dar_acceptance::sha256_hex(&text)}).to_string())?;
    Ok(by_digest)
}

pub async fn legacy_backup_migrates_to_digests(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let session = login(&instance.base).await?;
    let (shared, single) = (format!("Uploaded twice {}", uuid()), format!("Uploaded once {}", uuid()));
    let first = upload(&session, &format!("migrate-{}.txt", uuid()), &shared).await?;
    let second = upload(&session, &format!("migrate-{}.txt", uuid()), &shared).await?;
    let third = upload(&session, &format!("migrate-{}.txt", uuid()), &single).await?;
    let source = instance.database()?;
    let temporary = t.temp("blob-migration")?;
    let directory = temporary.join("backup");
    support::backup(&source, instance.root(), &directory)?;
    let by_digest = as_legacy_backup(&directory)?;
    let sha = dar_acceptance::sha256_hex;
    assert!(by_digest.contains_key(&sha(&shared)) && by_digest.contains_key(&sha(&single)));
    let port = dar_acceptance::stack::free_port()?;
    let origin = format!("http://127.0.0.1:{port}");
    let name = format!("dar-migrate-{}", &uuid()[..8]);
    instance::publish_module(&name, &origin)?;
    let target = Database::new(&source.url, &name, &source.token_file)?;
    let root = temporary.join("restored");
    support::restore(&target, &directory, &root)?;
    let names = |root: &Path| -> Result<Vec<String>> { Ok(std::fs::read_dir(root.join("blobs"))?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()) };
    let uuid_name = regex::Regex::new("^[a-f0-9-]{36}$")?;
    assert!(names(&root)?.iter().all(|file| uuid_name.is_match(file)));
    assert_eq!(target.call("service_blob_migration_plan", json!([])).await?["done"], json!(false));
    let runtime = Instance::start(InstanceConfig { port, origin: Some(origin.clone()), root: root.clone(), database_name: name.clone(), development: true, ..InstanceConfig::default() }).await?;
    let found = names(&root)?;
    let digest_name = regex::Regex::new("^[a-f0-9]{64}$")?;
    assert!(!found.is_empty() && found.iter().all(|file| digest_name.is_match(file)), "{}", found.join(" "));
    assert_eq!(found.iter().filter(|file| **file == sha(&shared)).count(), 1);
    let recovered = login(&origin).await?;
    for (file, text) in [(&first, &shared), (&second, &shared), (&third, &single)] {
        assert_eq!(&recovered.get(&format!("/api/v1/files/{}/content", file.file_id)).await?.text(), text);
    }
    let after = target.call("service_backup", json!([uuid()])).await?;
    assert_eq!(after["format"], "dar-backup-v12");
    let blob_of = |id: &str| after["files"].as_array().and_then(|files| files.iter().find(|row| row["id"] == id)).map(|row| row["blobId"].clone());
    assert_eq!([blob_of(&first.file_id), blob_of(&second.file_id), blob_of(&third.file_id)], [Some(json!(sha(&shared))), Some(json!(sha(&shared))), Some(json!(sha(&single)))]);
    assert!(after["blobs"].as_array().context("blobs")?.iter().all(|blob| blob["id"] == blob["digest"]));
    runtime.close().await;
    Ok(())
}
