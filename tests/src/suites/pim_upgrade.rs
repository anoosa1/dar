//! Port of `test/pim-upgrade.integration.test.ts`: auth/Files module (`bf782c0`) upgrades in
//! place to the current module; old v1 backup imports migrate blobs to content-addressed digests
//! and unknown backup formats fail closed.

use super::pim_workspace_support::{run_git, run_spacetime};
use chrono::Utc;
use dar_acceptance::prelude::*;
use dar_acceptance::stack::stack;
use std::os::unix::fs::symlink;

pub const SUITE: Suite = Suite {
    name: "pim-upgrade",
    cases: &[case!(auth_files_upgrade_and_v1_backup_restore, 120)],
};

pub async fn auth_files_upgrade_and_v1_backup_restore(t: &Ctx) -> Result<()> {
    let s = stack();
    let root = t.temp("dar-module-upgrade")?;
    let old = root.join("old");
    std::fs::create_dir_all(old.join("src"))?;
    let issuer = "http://127.0.0.1:49999/auth/issuer";

    // This immutable commit is the independently verified auth/Files baseline.
    let source = run_git(&["show", "bf782c0:next/module/src/index.ts"])?;
    std::fs::write(old.join("src/index.ts"), source)?;
    std::fs::write(
        old.join("package.json"),
        json!({"type": "module", "dependencies": {"spacetimedb": "2.14.1"}}).to_string(),
    )?;
    symlink(
        s.artifacts.repo.join("node_modules"),
        old.join("node_modules"),
    )?;
    let tsconfig = run_git(&["show", "bf782c0:next/module/tsconfig.json"])?;
    std::fs::write(old.join("tsconfig.json"), tsconfig)?;

    let old_str = old.to_string_lossy().into_owned();
    let bundle_str = old.join("dist/bundle.js").to_string_lossy().into_owned();
    let module_str = s.artifacts.module.to_string_lossy().into_owned();
    let db_url = s.database.url.clone();
    let name = format!("dar-upgrade-{}", &uuid()[..8]);

    let (old_s, bundle_s, db_u, name_s) = (
        old_str.clone(),
        bundle_str.clone(),
        db_url.clone(),
        name.clone(),
    );
    tokio::task::spawn_blocking(move || -> Result<()> {
        run_spacetime(&["build", "--module-path", &old_s], issuer)?;
        run_spacetime(
            &[
                "publish",
                &name_s,
                "--js-path",
                &bundle_s,
                "--server",
                &db_u,
                "--yes",
                "--no-config",
            ],
            issuer,
        )?;
        Ok(())
    })
    .await
    .map_err(|e| Error::new(e.to_string()))??;

    let db = Database::new(&db_url, &name, &s.database.token_file)?;
    let user_id = uuid();
    let session_id = uuid();
    let verifier = support::hash_password("old module proof password")?;
    db.call(
        "service_create_account",
        json!([user_id, "upgrade-user", "Upgrade user", verifier, true]),
    )
    .await?;
    let expires_at = Utc::now().timestamp_millis() + 60_000;
    db.call(
        "service_create_session",
        json!([
            session_id,
            user_id,
            "upgrade-test-digest",
            "",
            expires_at,
            verifier
        ]),
    )
    .await?;

    let upload_id = uuid();
    let file_id = uuid();
    let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    db.call(
        "service_begin_upload",
        json!([
            session_id,
            upload_id,
            file_id,
            "",
            "Existing.txt",
            0,
            "text/plain",
            0
        ]),
    )
    .await?;
    db.call(
        "service_finish_upload",
        json!([session_id, upload_id, empty]),
    )
    .await?;

    let pin = uuid();
    let snapshot = db.call("service_backup", json!([pin])).await?;
    assert_eq!(snapshot["format"], "dar-backup-v1");
    db.call("service_release_backup", json!([pin])).await?;

    let (name_s, mod_s, db_u) = (name.clone(), module_str.clone(), db_url.clone());
    tokio::task::spawn_blocking(move || -> Result<()> {
        run_spacetime(
            &[
                "publish",
                &name_s,
                "--bin-path",
                &mod_s,
                "--server",
                &db_u,
                "--yes",
                "--no-config",
            ],
            issuer,
        )?;
        Ok(())
    })
    .await
    .map_err(|e| Error::new(e.to_string()))??;

    let existing = db
        .call("service_download", json!([session_id, file_id, ""]))
        .await?;
    assert_eq!(existing["id"], file_id);

    let pim_payload = json!({
        "id": uuid(),
        "kind": "contacts",
        "name": "After upgrade",
        "color": "#2563eb"
    })
    .to_string();
    let created = db
        .call(
            "service_pim",
            json!([session_id, "createCollection", pim_payload]),
        )
        .await?;
    assert!(
        !created["value"]["id"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );

    let restored_name = format!("dar-old-restore-{}", &uuid()[..8]);
    let (rname_s, mod_s, db_u) = (restored_name.clone(), module_str, db_url.clone());
    tokio::task::spawn_blocking(move || -> Result<()> {
        run_spacetime(
            &[
                "publish",
                &rname_s,
                "--bin-path",
                &mod_s,
                "--server",
                &db_u,
                "--yes",
                "--no-config",
            ],
            issuer,
        )?;
        Ok(())
    })
    .await
    .map_err(|e| Error::new(e.to_string()))??;

    let restored = Database::new(&db_url, &restored_name, &s.database.token_file)?;
    let mut bad_snapshot = snapshot.clone();
    bad_snapshot["format"] = json!("dar-backup-v999");
    assert!(
        restored
            .call("service_restore", json!([bad_snapshot.to_string()]))
            .await
            .is_err()
    );

    restored
        .call("service_restore", json!([snapshot.to_string()]))
        .await?;
    let account = restored
        .call("service_account", json!(["upgrade-user"]))
        .await?;
    assert_eq!(account["id"], user_id);
    assert_eq!(account["verifier"], verifier);

    // A v1 backup names content by upload ID; the blob migration switches it to the digest its upload recorded.
    let plan = restored
        .call("service_blob_migration_plan", json!([]))
        .await?;
    assert_eq!(
        plan,
        json!({"done": false, "blobs": [{"id": upload_id, "size": 0, "digest": empty, "required": true}]})
    );
    restored
        .call("service_migrate_blobs", json!(["{}"]))
        .await?;
    assert_eq!(
        restored
            .call("service_blob_migration_plan", json!([]))
            .await?,
        json!({"done": true, "blobs": []})
    );

    let recovered = restored.call("service_backup", json!([uuid()])).await?;
    assert_eq!(recovered["format"], "dar-backup-v12");
    let recovered_file = recovered["files"]
        .as_array()
        .context("recovered files")?
        .iter()
        .find(|row| row["id"] == file_id)
        .context("recovered file missing")?;
    assert_eq!(recovered_file["blobId"], empty);
    let blobs: Vec<(&str, &str)> = recovered["blobs"]
        .as_array()
        .context("recovered blobs")?
        .iter()
        .map(|b| {
            (
                b["id"].as_str().unwrap_or_default(),
                b["digest"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(blobs, vec![(empty, empty)]);
    assert_eq!(
        recovered["pim"]["collections"].as_array().map(Vec::len),
        Some(0)
    );
    Ok(())
}
