//! The operator's commands that need the database (once `scripts/operator.ts` and
//! `runtime/backup.ts`): backups, restore, a person's file export, account switches and
//! password recovery, plus on-demand maintenance. They read the same environment as the
//! runtime (`DAR_ROOT`, `DAR_DATABASE_URL`, `DAR_DATABASE_NAME`, `DAR_DATABASE_TOKEN_FILE`).
//!
//! Backups are directories: a consistent database snapshot (`snapshot.json`), the content it
//! names under `blobs/` (each copy checked against its size and SHA-256), the instance keys,
//! and `complete.json` with the snapshot's digest, written last. The format is unchanged, so
//! backups made by the TypeScript stack restore here and the other way round.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::db::Database;
use crate::root::{RootLock, create_durable_file, libc_nofollow, open_root, sync_directory};

type Outcome = Result<String, String>;

const SNAPSHOT_FORMATS: [&str; 12] = [
    "dar-backup-v1",
    "dar-backup-v2",
    "dar-backup-v3",
    "dar-backup-v4",
    "dar-backup-v5",
    "dar-backup-v6",
    "dar-backup-v7",
    "dar-backup-v8",
    "dar-backup-v9",
    "dar-backup-v10",
    "dar-backup-v11",
    "dar-backup-v12",
];

fn required(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{name} is required."))
}

fn database() -> Result<Database, String> {
    let token = fs::read_to_string(required("DAR_DATABASE_TOKEN_FILE")?)
        .map_err(|e| format!("The database token could not be read: {e}"))?;
    let token = token.trim();
    if token.is_empty() {
        return Err("A database service credential is required.".into());
    }
    Ok(Database::new(
        &required("DAR_DATABASE_URL")?,
        &required("DAR_DATABASE_NAME")?,
        token,
    ))
}

async fn call(db: &Database, function: &str, args: Value) -> Result<Value, String> {
    db.call(function, args).await.map_err(|e| e.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn private_directory(path: &Path) -> Result<(), String> {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Copies a regular file to a new private file, checking its size and digest when given.
fn copy_verified(
    source: &Path,
    destination: &Path,
    expected: Option<(u64, &str)>,
) -> Result<(), String> {
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc_nofollow())
        .open(source)
        .map_err(|e| format!("{}: {e}", source.display()))?;
    if !input.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Backup content is not a regular file.".into());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc_nofollow())
        .open(destination)
        .map_err(|e| format!("{}: {e}", destination.display()))?;
    let (mut hash, mut size, mut buffer) = (Sha256::new(), 0u64, vec![0u8; 128 * 1024]);
    loop {
        let read = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        size += read as u64;
        output
            .write_all(&buffer[..read])
            .map_err(|e| e.to_string())?;
    }
    if let Some((want_size, want_digest)) = expected
        && (want_size != size || want_digest != hex(&hash.finalize()))
    {
        return Err("Backup content failed its integrity check.".into());
    }
    output.sync_all().map_err(|e| e.to_string())
}

fn blob_path(base: &Path, id: &str) -> Result<PathBuf, String> {
    // Content is named by its digest; older backups name it by upload ID (both 36–64 safe characters).
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err("Unsafe content name in the backup.".into());
    }
    Ok(base.join("blobs").join(id))
}

fn blob_list(snapshot: &Value) -> Result<Vec<(String, u64, String)>, String> {
    snapshot["blobs"]
        .as_array()
        .ok_or("The snapshot lists no content.")?
        .iter()
        .map(|b| {
            Ok((
                b["id"].as_str().ok_or("Bad snapshot.")?.to_owned(),
                b["size"].as_u64().ok_or("Bad snapshot.")?,
                b["digest"].as_str().ok_or("Bad snapshot.")?.to_owned(),
            ))
        })
        .collect()
}

fn has_oidc_keyring(format: &str) -> bool {
    SNAPSHOT_FORMATS[2..].contains(&format)
}

/// A consistent database snapshot pins its content until independent durable copies exist.
async fn backup(db: &Database, root: &Path, destination: &Path) -> Outcome {
    let id = crate::root::uuid();
    private_directory(destination)?;
    let snapshot = call(db, "service_backup", json!([id])).await?;
    create_durable_file(
        &destination.join("pending.json"),
        json!({ "format": "dar-backup-v1", "id": id })
            .to_string()
            .as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    private_directory(&destination.join("blobs"))?;
    let blobs = blob_list(&snapshot)?;
    for (blob, size, digest) in &blobs {
        copy_verified(
            &blob_path(root, blob)?,
            &blob_path(destination, blob)?,
            Some((*size, digest)),
        )?;
    }
    let secrets = root.join("secrets");
    copy_verified(
        &secrets.join("signing-key.json"),
        &destination.join("signing-key.json"),
        None,
    )?;
    copy_verified(
        &secrets.join("identity-keyring.key"),
        &destination.join("identity-keyring.key"),
        None,
    )?;
    if has_oidc_keyring(snapshot["format"].as_str().unwrap_or_default()) {
        copy_verified(
            &secrets.join("oidc-keyring.json"),
            &destination.join("oidc-keyring.json"),
            None,
        )?;
    }
    let text = snapshot.to_string();
    create_durable_file(&destination.join("snapshot.json"), text.as_bytes())
        .map_err(|e| e.to_string())?;
    sync_directory(&destination.join("blobs")).map_err(|e| e.to_string())?;
    let complete = json!({ "format": "dar-backup-v1", "snapshotDigest": hex(&Sha256::digest(text.as_bytes())) });
    create_durable_file(
        &destination.join("complete.json"),
        complete.to_string().as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    sync_directory(destination).map_err(|e| e.to_string())?;
    sync_directory(destination.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    // A completed backup owns its copies; a failed one keeps its pins until released.
    call(db, "service_release_backup", json!([id])).await?;
    Ok(json!({ "id": id, "blobs": blobs.len() }).to_string())
}

/// Restores only into empty content and identity storage; browser sessions are never restored.
async fn restore(db: &Database, source: &Path, root: &Path) -> Outcome {
    for name in ["snapshot.json", "complete.json"] {
        if !fs::symlink_metadata(source.join(name)).is_ok_and(|m| m.is_file()) {
            return Err("Unsafe backup manifest.".into());
        }
    }
    let text = fs::read_to_string(source.join("snapshot.json")).map_err(|e| e.to_string())?;
    let complete: Value = serde_json::from_str(
        &fs::read_to_string(source.join("complete.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|_| "Incomplete or corrupted backup.")?;
    if complete["format"] != "dar-backup-v1"
        || complete["snapshotDigest"].as_str()
            != Some(hex(&Sha256::digest(text.as_bytes())).as_str())
    {
        return Err("Incomplete or corrupted backup.".into());
    }
    let snapshot: Value =
        serde_json::from_str(&text).map_err(|_| "Incomplete or corrupted backup.")?;
    let format = snapshot["format"].as_str().unwrap_or_default();
    if !SNAPSHOT_FORMATS.contains(&format) {
        return Err("Unsupported backup format.".into());
    }
    let root = open_root(root).map_err(|e| e.to_string())?;
    let _lock = RootLock::acquire(&root).map_err(|e| e.to_string())?;
    let empty = |dir: &str, allowed: &[&str]| {
        fs::read_dir(root.join(dir))
            .map(|entries| {
                entries
                    .flatten()
                    .all(|e| allowed.contains(&e.file_name().to_string_lossy().as_ref()))
            })
            .unwrap_or(true)
    };
    if !empty("blobs", &[])
        || !empty("agent-workspaces", &[])
        || !empty("secrets", &["bootstrap-token", "database-token"])
    {
        return Err("Restore requires an empty content and identity root.".into());
    }
    let blobs = blob_list(&snapshot)?;
    for (blob, size, digest) in &blobs {
        copy_verified(
            &blob_path(source, blob)?,
            &blob_path(&root, blob)?,
            Some((*size, digest)),
        )?;
    }
    let secrets = root.join("secrets");
    copy_verified(
        &source.join("signing-key.json"),
        &secrets.join("signing-key.json"),
        None,
    )?;
    copy_verified(
        &source.join("identity-keyring.key"),
        &secrets.join("identity-keyring.key"),
        None,
    )?;
    if has_oidc_keyring(format) {
        copy_verified(
            &source.join("oidc-keyring.json"),
            &secrets.join("oidc-keyring.json"),
            None,
        )?;
    }
    sync_directory(&root.join("blobs")).map_err(|e| e.to_string())?;
    sync_directory(&secrets).map_err(|e| e.to_string())?;
    call(db, "service_restore", json!([text])).await?;
    Ok(json!({ "blobs": blobs.len() }).to_string())
}

/// Portable content plus a readable hierarchy manifest, without account secrets.
async fn export_files(db: &Database, root: &Path, username: &str, destination: &Path) -> Outcome {
    let id = crate::root::uuid();
    let value = call(db, "service_export", json!([id, username])).await?;
    let result = (|| {
        private_directory(destination)?;
        private_directory(&destination.join("files"))?;
        let blobs = blob_list(&value)?;
        let mut manifest = Vec::new();
        for entry in value["files"].as_array().ok_or("Unsafe export metadata.")? {
            let file_id = entry["id"].as_str().unwrap_or_default();
            let name = entry["name"].as_str().unwrap_or_default();
            if file_id.len() != 36
                || !file_id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
                || name.contains(['\0', '/', '\\'])
                || name == "."
                || name == ".."
            {
                return Err("Unsafe export metadata.".to_owned());
            }
            let mut metadata = entry.clone();
            let blob_id = metadata
                .as_object_mut()
                .and_then(|m| m.remove("blobId"))
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            if entry["kind"] == "file" {
                let mut short = String::new();
                for c in name.chars() {
                    if short.len() + c.len_utf8() > 190 {
                        break;
                    }
                    short.push(c);
                }
                let extension = if short != name {
                    name.rfind('.')
                        .map(|i| &name[i..])
                        .filter(|e| {
                            (2..=13).contains(&e.len())
                                && e[1..].chars().all(|c| c.is_ascii_alphanumeric())
                        })
                        .unwrap_or("")
                } else {
                    ""
                };
                let content = format!("files/{file_id}-{short}{extension}");
                let (_, size, digest) = blobs
                    .iter()
                    .find(|(b, _, _)| *b == blob_id)
                    .ok_or("Unsafe export metadata.")?;
                copy_verified(
                    &blob_path(root, &blob_id)?,
                    &destination.join(&content),
                    Some((*size, digest)),
                )?;
                metadata["contentFile"] = json!(content);
            }
            manifest.push(metadata);
        }
        let document = json!({ "format": "dar-files-export-v1", "exportId": id, "username": value["username"], "exportedAt": chrono_now(), "files": manifest });
        create_durable_file(
            &destination.join("manifest.json"),
            serde_json::to_string_pretty(&document)
                .unwrap_or_default()
                .as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        sync_directory(&destination.join("files")).map_err(|e| e.to_string())?;
        sync_directory(destination).map_err(|e| e.to_string())?;
        Ok(json!({ "files": manifest.len() }).to_string())
    })();
    let _ = call(db, "service_release_backup", json!([id])).await;
    result
}

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

async fn account(db: &Database, username: &str) -> Result<Value, String> {
    let user = call(db, "service_account", json!([username.to_lowercase()])).await?;
    if user.is_null() {
        Err("Account not found.".into())
    } else {
        Ok(user)
    }
}

const USAGE: &str = "Usage: dar-runtime backup|restore <directory>, export-files <username> <directory>, release-backup <id>, enable-user|disable-user <username>, recover-password <username> <protected-password-file>, configure-oidc <file> [optional], deliver-reminders, reconcile-storage or sweep-orphans";

/// Runs an operator command; `None` when `args` names none (the runtime then starts).
pub async fn run(args: &[String]) -> Option<i32> {
    let name = args.first()?.as_str();
    let known = [
        "backup",
        "restore",
        "export-files",
        "release-backup",
        "enable-user",
        "disable-user",
        "recover-password",
        "deliver-reminders",
        "reconcile-storage",
        "sweep-orphans",
        "configure-oidc",
    ];
    if !known.contains(&name) {
        return None;
    }
    let argument = args.get(1).map(String::as_str);
    let outcome = async {
        let db = database()?;
        // Only the commands that touch stored content need the root.
        let root = || required("DAR_ROOT").map(PathBuf::from);
        match (name, argument, args.get(2).map(String::as_str)) {
            ("backup", Some(dir), _) => backup(&db, &root()?, Path::new(dir)).await,
            ("restore", Some(dir), _) => restore(&db, Path::new(dir), &root()?).await,
            ("export-files", Some(user), Some(dir)) => export_files(&db, &root()?, user, Path::new(dir)).await,
            ("release-backup", Some(id), _) => call(&db, "service_release_backup", json!([id])).await.map(|_| "Backup pins released.".into()),
            ("enable-user" | "disable-user", Some(user), _) => {
                let user = account(&db, user).await?;
                call(&db, "service_set_enabled", json!([user["id"], name == "enable-user"])).await?;
                Ok("Account status updated.".into())
            }
            ("recover-password", Some(user), Some(file)) => {
                let user = account(&db, user).await?;
                let password = fs::read_to_string(file).map_err(|e| e.to_string())?;
                let password = password.strip_suffix('\n').map(|p| p.strip_suffix('\r').unwrap_or(p)).unwrap_or(&password);
                crate::auth::validate_new_password(password).map_err(|e| e.message)?;
                call(&db, "service_change_password", json!([user["id"], crate::auth::hash_password(password), user["verifier"]])).await?;
                Ok("Password changed; all browser sessions revoked. Securely remove the password file.".into())
            }
            // Applies an OIDC client file (DAR_OIDC_FILE) to the database, as the runtime does at startup.
            ("configure-oidc", Some(file), optional) => {
                let origin = required("DAR_ORIGIN")?;
                let development = std::env::var("DAR_DEVELOPMENT").as_deref() == Ok("1");
                let oidc = crate::oidc::Oidc::open(&origin, development, &root()?.join("secrets"), db).map_err(|e| e.to_string())?;
                oidc.configure(Path::new(file), optional == Some("optional")).await.map(|_| "OIDC clients configured.".into())
            }
            ("deliver-reminders", _, _) => crate::maintenance::deliver_reminders(&db).await.map(|_| "Due reminders delivered.".into()).map_err(|e| e.to_string()),
            ("reconcile-storage" | "sweep-orphans", _, _) => {
                let blobs = crate::blobs::Blobs::open(&root()?).map_err(|e| e.to_string())?;
                let count = if name == "reconcile-storage" { crate::maintenance::reconcile_storage(&db, &blobs).await } else { crate::maintenance::sweep_orphans(&db, &blobs).await };
                count.map(|n| json!({ "count": n }).to_string()).map_err(|e| e.to_string())
            }
            _ => Err(USAGE.into()),
        }
    }
    .await;
    Some(match outcome {
        Ok(message) => {
            println!("{message}");
            0
        }
        Err(message) => {
            eprintln!("{message}");
            1
        }
    })
}
