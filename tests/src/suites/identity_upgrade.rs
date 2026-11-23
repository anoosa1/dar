//! Identity upgrade (`identity-upgrade.integration.test.ts`): Contacts/Calendar v2 module
//! upgrades in place and v2 backups restore with stable directory identities.

use dar_acceptance::prelude::*;
use dar_acceptance::stack::stack;
use std::process::{Command, Stdio};

pub const SUITE: Suite = Suite {
    name: "identity-upgrade",
    cases: &[case!(v2_upgrade_and_restore, 90)],
};

fn git(repo: &std::path::Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git").current_dir(repo).args(args).stdin(Stdio::null()).output().context("running git")?;
    ensure!(output.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&output.stderr));
    Ok(output.stdout)
}

pub async fn v2_upgrade_and_restore(t: &Ctx) -> Result<()> {
    let stack = stack();
    let project = &stack.artifacts.repo;
    let root = t.temp("dar-identity-upgrade")?;
    let old = root.join("old");
    std::fs::create_dir_all(old.join("src"))?;

    let origin = format!("http://127.0.0.1:{}", stack.database.port);
    let issuer = format!("{origin}/auth/issuer");
    let baseline = "d9758fc";

    let tree = String::from_utf8(git(project, &["ls-tree", "-r", "--name-only", baseline, "next/module/src"])?)?;
    let files: Vec<&str> = tree.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(files.len() > 1);
    for file in files {
        let name = std::path::Path::new(file).file_name().context("file name")?;
        std::fs::write(old.join("src").join(name), git(project, &["show", &format!("{baseline}:{file}")])?)?;
    }
    std::fs::write(old.join("package.json"), json!({ "type": "module", "dependencies": { "spacetimedb": "2.14.1" } }).to_string())?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(project.join("node_modules"), old.join("node_modules"))?;
    std::fs::write(old.join("tsconfig.json"), git(project, &["show", &format!("{baseline}:next/module/tsconfig.json")])?)?;

    let run_spacetime = |args: &[&str]| -> Result<()> {
        let output = Command::new(&stack.artifacts.spacetime)
            .arg("--root-dir")
            .arg(&stack.database.cli_root)
            .args(args)
            .env("DAR_ISSUER", &issuer)
            .stdin(Stdio::null())
            .output()
            .context("running spacetime CLI")?;
        ensure!(
            output.status.success(),
            "spacetime {:?} failed:\n{}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };

    let old_str = old.to_string_lossy().into_owned();
    let bundle_str = old.join("dist/bundle.js").to_string_lossy().into_owned();
    run_spacetime(&["build", "--module-path", &old_str])?;

    let name = format!("dar-identity-upgrade-{}", &uuid()[..8]);
    run_spacetime(&["publish", &name, "--js-path", &bundle_str, "--server", &stack.database.url, "--yes", "--no-config"])?;

    let db = Database::new(&stack.database.url, &name, &stack.database.token_file)?;
    let id = uuid();
    let sid = uuid();
    let verifier = support::hash_password("old directory account password")?;
    let collection_id = uuid();

    db.call("service_create_account", json!([id, "old-directory", "Old directory", verifier, true])).await?;
    let expires_at = chrono::Utc::now().timestamp_millis() + 60_000;
    db.call("service_create_session", json!([sid, id, "upgrade-digest", "", expires_at, verifier])).await?;
    db.call(
        "service_pim",
        json!([sid, "createCollection", json!({ "id": collection_id, "kind": "contacts", "name": "Existing contacts", "color": "#2563eb" }).to_string()]),
    )
    .await?;

    let pin = uuid();
    let snapshot = db.call("service_backup", json!([pin])).await?;
    db.call("service_release_backup", json!([pin])).await?;
    assert_eq!(snapshot["format"], json!("dar-backup-v2"));

    stack.database.publish(&stack.artifacts, &name, &origin)?;
    db.call("service_startup", json!([issuer])).await?;

    let users = db.call("service_identity", json!([sid, "", "", "users", "{}"])).await?;
    assert_eq!(users[0]["id"], json!(id));
    assert_eq!(users[0]["userName"], json!("old-directory"));
    assert_eq!(users[0]["revision"], json!(1));

    let upgraded = db.call("service_backup", json!([uuid()])).await?;
    assert_eq!(upgraded["format"], json!("dar-backup-v12"));
    assert_eq!(upgraded["pim"]["collections"][0]["id"], json!(collection_id));

    let restored_name = format!("dar-v2-restore-{}", &uuid()[..8]);
    stack.database.publish(&stack.artifacts, &restored_name, &origin)?;
    let restored = Database::new(&stack.database.url, &restored_name, &stack.database.token_file)?;
    restored.call("service_restore", json!([snapshot.to_string()])).await?;
    let recovered = restored.call("service_backup", json!([uuid()])).await?;
    assert_eq!(recovered["accounts"][0]["id"], json!(id));
    assert_eq!(recovered["pim"]["collections"][0]["id"], json!(collection_id));
    assert_eq!(recovered["identity"]["clients"].as_array().map(Vec::len), Some(0));
    Ok(())
}
