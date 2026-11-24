//! Content storage (`gc.integration.test.ts`): identical files share one blob, kept until no
//! version or backup pin uses it; content no upload recorded is swept.

use dar_acceptance::prelude::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const SUITE: Suite = Suite {
    name: "gc",
    cases: &[case!(shared_blobs_pins_and_orphans, 30)],
};

/// Content lives under `<root>/blobs/<id>`.
fn blob(root: &Path, id: &str) -> PathBuf {
    root.join("blobs").join(id)
}

fn age(path: &Path) -> Result<()> {
    std::fs::File::options()
        .write(true)
        .open(path)?
        .set_modified(SystemTime::now() - Duration::from_secs(3 * 3600))?;
    Ok(())
}

pub async fn shared_blobs_pins_and_orphans(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let session = login(&instance.base).await?;
    let contents = format!("Shared by two files {}", uuid());
    let digest = dar_acceptance::sha256_hex(&contents);
    let original = upload(&session, &format!("GC-{}.txt", uuid()), &contents).await?;
    let twin = upload(&session, &format!("GC-twin-{}.txt", uuid()), &contents).await?;
    let source = instance.database()?;
    let source_pin = uuid();
    let snapshot = source.call("service_backup", json!([source_pin])).await?;
    let blob_of = |id: &str| {
        snapshot["files"]
            .as_array()
            .and_then(|files| files.iter().find(|row| row["id"] == id))
            .map(|row| row["blobId"].clone())
    };
    assert_eq!(
        [blob_of(&original.file_id), blob_of(&twin.file_id)],
        [Some(json!(digest)), Some(json!(digest))]
    );
    let blobs = snapshot["blobs"].as_array().context("blobs")?;
    assert!(blobs.iter().all(|blob| blob["id"] == blob["digest"]));
    let root = t.temp("gc-proof")?;
    std::fs::write(
        root.join("instance.json"),
        json!({"format": "dar-spacetime-v1"}).to_string(),
    )?;
    std::fs::create_dir(root.join("blobs"))?;
    let outcome = async {
        for row in blobs {
            let id = row["id"].as_str().unwrap_or_default();
            std::fs::copy(blob(instance.root(), id), blob(&root, id))?;
        }
        let name = format!("dar-gc-{}", &uuid()[..8]);
        instance::publish_module(&name, &instance.base)?;
        let target = Database::new(&source.url, &name, &source.token_file)?;
        target
            .call("service_restore", json!([snapshot.to_string()]))
            .await?;
        // Content no upload recorded (a failed upload or a crash) goes; recorded content stays.
        let orphan_bytes = format!("orphan {}", uuid());
        let orphan = dar_acceptance::sha256_hex(&orphan_bytes);
        std::fs::write(blob(&root, &orphan), &orphan_bytes)?;
        // Past the grace period that protects content still being written.
        age(&blob(&root, &orphan))?;
        assert_eq!(support::sweep_orphans(&target, &root)?, 1);
        assert!(!blob(&root, &orphan).exists());
        assert_eq!(support::reconcile_storage(&target, &root)?, 0);
        assert!(blob(&root, &digest).exists());
        let pin = uuid();
        target.call("service_backup", json!([pin])).await?;
        let user = snapshot["accounts"]
            .as_array()
            .and_then(|a| a.iter().find(|u| u["username"] == "admin"))
            .context("admin account")?;
        let id = uuid();
        let expires_at = chrono::Utc::now().timestamp_millis() as u64 + 60_000;
        target
            .call(
                "service_create_session",
                json!([
                    id,
                    user["id"],
                    format!("test:{id}"),
                    "",
                    expires_at,
                    user["verifier"]
                ]),
            )
            .await?;
        let token = support::browser_token(
            &instance.root().join("secrets"),
            &instance.url("/auth/issuer"),
            &id,
            expires_at,
        )?;
        let db = Session {
            token,
            database_name: name.clone(),
            ..session.clone()
        }
        .connect()
        .await?;
        reducer(|done| {
            db.reducers
                .trash_entry_then(original.file_id.clone(), 1, done)
        })
        .await?;
        reducer(|done| {
            db.reducers
                .delete_entry_then(original.file_id.clone(), 2, done)
        })
        .await?;
        assert_eq!(support::reconcile_storage(&target, &root)?, 0);
        target.call("service_release_backup", json!([pin])).await?;
        assert_eq!(support::reconcile_storage(&target, &root)?, 0);
        assert!(blob(&root, &digest).exists());
        reducer(|done| db.reducers.trash_entry_then(twin.file_id.clone(), 1, done)).await?;
        reducer(|done| db.reducers.delete_entry_then(twin.file_id.clone(), 2, done)).await?;
        // Content linked or used again within the grace period stays claimed until it settles.
        assert_eq!(support::reconcile_storage(&target, &root)?, 0);
        assert!(blob(&root, &digest).exists());
        age(&blob(&root, &digest))?;
        assert_eq!(support::reconcile_storage(&target, &root)?, 1);
        assert!(!blob(&root, &digest).exists());
        assert_eq!(support::reconcile_storage(&target, &root)?, 0);
        Ok::<_, Error>(())
    }
    .await;
    source
        .call("service_release_backup", json!([source_pin]))
        .await?;
    outcome
}
