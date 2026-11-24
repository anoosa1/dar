//! Port of `test/workspace-upgrade.integration.test.ts`: Identity v3 module (`85e9dbd`) upgrades
//! in place and v3 backups restore with Notes/Tasks schema additions.

use super::pim_workspace_support::{run_git, run_spacetime};
use chrono::Utc;
use dar_acceptance::prelude::*;
use dar_acceptance::stack::stack;
use std::os::unix::fs::symlink;
use std::path::Path;

pub const SUITE: Suite = Suite {
    name: "workspace-upgrade",
    cases: &[case!(identity_v3_upgrade_and_restore, 120)],
};

pub async fn identity_v3_upgrade_and_restore(t: &Ctx) -> Result<()> {
    let s = stack();
    let root = t.temp("dar-workspace-upgrade")?;
    let old = root.join("old");
    std::fs::create_dir_all(old.join("src"))?;
    let issuer = "http://127.0.0.1:49999/auth/issuer";
    let baseline = "85e9dbd";

    let tree_out = String::from_utf8(run_git(&[
        "ls-tree",
        "-r",
        "--name-only",
        baseline,
        "next/module/src",
    ])?)
    .map_err(|e| Error::new(e.to_string()))?;
    let files: Vec<&str> = tree_out.trim().lines().filter(|l| !l.is_empty()).collect();
    assert!(
        files.len() > 1,
        "expected multiple files in {baseline}:next/module/src"
    );
    for file in files {
        let content = run_git(&["show", &format!("{baseline}:{file}")])?;
        let base_name = Path::new(file).file_name().context("file name")?;
        std::fs::write(old.join("src").join(base_name), content)?;
    }
    std::fs::write(
        old.join("package.json"),
        json!({"type": "module", "dependencies": {"spacetimedb": "2.14.1"}}).to_string(),
    )?;
    symlink(
        s.artifacts.repo.join("node_modules"),
        old.join("node_modules"),
    )?;
    let tsconfig = run_git(&["show", &format!("{baseline}:next/module/tsconfig.json")])?;
    std::fs::write(old.join("tsconfig.json"), tsconfig)?;

    let old_str = old.to_string_lossy().into_owned();
    let bundle_str = old.join("dist/bundle.js").to_string_lossy().into_owned();
    let module_str = s.artifacts.module.to_string_lossy().into_owned();
    let db_url = s.database.url.clone();
    let name = format!("dar-workspace-upgrade-{}", &uuid()[..8]);

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
    let id = uuid();
    let sid = uuid();
    let verifier = support::hash_password("old directory account password")?;
    let collection_id = uuid();
    db.call(
        "service_create_account",
        json!([id, "old-directory", "Old directory", verifier, true]),
    )
    .await?;
    let expires_at = Utc::now().timestamp_millis() + 60_000;
    db.call(
        "service_create_session",
        json!([sid, id, "upgrade-digest", "", expires_at, verifier]),
    )
    .await?;
    let pim_payload = json!({
        "id": collection_id,
        "kind": "contacts",
        "name": "Existing contacts",
        "color": "#2563eb"
    })
    .to_string();
    db.call("service_pim", json!([sid, "createCollection", pim_payload]))
        .await?;

    let pin = uuid();
    let snapshot = db.call("service_backup", json!([pin])).await?;
    db.call("service_release_backup", json!([pin])).await?;
    assert_eq!(snapshot["format"], "dar-backup-v3");

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

    db.call("service_startup", json!([issuer])).await?;
    let users = db
        .call("service_identity", json!([sid, "", "", "users", "{}"]))
        .await?;
    assert_eq!(users[0]["id"], id);
    assert_eq!(users[0]["userName"], "old-directory");
    assert_eq!(users[0]["revision"], 1);

    let upgraded = db.call("service_backup", json!([uuid()])).await?;
    assert_eq!(upgraded["format"], "dar-backup-v12");
    assert_eq!(upgraded["pim"]["collections"][0]["id"], collection_id);

    let restored_name = format!("dar-v3-restore-{}", &uuid()[..8]);
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
    restored
        .call("service_restore", json!([snapshot.to_string()]))
        .await?;
    let recovered = restored.call("service_backup", json!([uuid()])).await?;
    assert_eq!(recovered["accounts"][0]["id"], id);
    assert_eq!(recovered["pim"]["collections"][0]["id"], collection_id);
    assert_eq!(
        recovered["identity"]["clients"].as_array().map(Vec::len),
        Some(0)
    );
    Ok(())
}
