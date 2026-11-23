//! The `dar-dav` handlers on Dar's module and blob store: app passwords through
//! `service_protocol_authenticate`, Files through `service_dav` and the upload procedures.
//! CalDAV and CardDAV through `service_pim`, with the same checks as the apps.

use std::sync::Arc;

use dar_dav::files::{Entry, FilesBackend, Kind, Listed, Target, Transfer};
use dar_dav::pim::{Collection, PimBackend, PimKind, Resource};
use dar_dav::{Accounts, App, DavError, DavResult, Principal};
use serde_json::{Value, json};

use crate::blobs::Blobs;
use crate::db::Database;
use crate::error::AppError;

/// The DAV traits are synchronous; they run on a runtime worker that may block.
fn block<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(work))
}

fn dav_error(error: AppError) -> DavError {
    let code: &'static str = match error.code.as_str() {
        "conflict" => "conflict",
        "notFound" => "notFound",
        "forbidden" => "forbidden",
        "unauthenticated" => "unauthenticated",
        "invalidInput" => "invalidInput",
        "invalidName" => "invalidName",
        "quotaExceeded" => "quotaExceeded",
        "operationTooLarge" => "operationTooLarge",
        "lockedFolder" => "lockedFolder",
        "contentUnavailable" => "contentUnavailable",
        "uploadTooLarge" => "uploadTooLarge",
        "incompleteUpload" => "incompleteUpload",
        _ => "databaseUnavailable",
    };
    DavError::new(error.status, code)
}

fn entry(value: &Value) -> Entry {
    let text = |name: &str| value[name].as_str().unwrap_or_default().to_owned();
    let optional = |name: &str| Some(text(name)).filter(|s| !s.is_empty());
    Entry {
        id: text("id"),
        kind: if value["kind"] == "folder" { Kind::Folder } else { Kind::File },
        name: text("name"),
        size: value["size"].as_f64().unwrap_or(0.0) as u64,
        media_type: text("mediaType"),
        revision: value["revision"].as_u64().unwrap_or(0),
        blob_id: text("blobId"),
        modified_at: value["modifiedAt"].as_f64().unwrap_or(0.0) as i64,
        sha1: optional("sha1"),
        md5: optional("md5"),
    }
}

pub struct DarAccounts(pub Database);

impl Accounts for DarAccounts {
    fn authenticate(&self, username: &str, secret_digest: &str, app: App, write: bool) -> DavResult<Principal> {
        let value = block(async {
            match app {
                App::Files => self.0.call("service_protocol_authenticate", json!([username, secret_digest, write])).await,
                App::Contacts | App::Calendar => {
                    let name = if matches!(app, App::Contacts) { "contacts" } else { "calendar" };
                    self.0.call("service_pim_authenticate", json!([username, secret_digest, name, write])).await
                }
            }
        })
        .map_err(dav_error)?;
        Ok(Principal { session_id: value["sessionId"].as_str().unwrap_or_default().into(), username: value["username"].as_str().unwrap_or_default().into() })
    }
}

pub struct DarFiles {
    pub db: Database,
    pub blobs: Blobs,
    pub checksums: crate::maintenance::Worker,
    pub texts: crate::maintenance::Worker,
}

impl DarFiles {
    #[allow(clippy::too_many_arguments)]
    fn dav(&self, session: &str, method: &str, path: &str, destination: &str, revision: u64, overwrite: bool, depth: u8, modified_at: i64) -> DavResult<Value> {
        block(self.db.call("service_dav", json!([session, method, path, destination, revision, overwrite, depth, modified_at]))).map_err(dav_error)
    }
}

impl FilesBackend for DarFiles {
    fn stat(&self, session: &str, path: &str) -> DavResult<Entry> {
        Ok(entry(&self.dav(session, "STAT", path, "", 0, true, 0, 0)?))
    }

    fn list(&self, session: &str, path: &str, depth: u8) -> DavResult<(Entry, Vec<Listed>)> {
        let value = self.dav(session, "PROPFIND", path, "", 0, true, depth, 0)?;
        let children = value["children"].as_array().map(|c| c.iter().map(|child| Listed { entry: entry(child), path: child["path"].as_str().unwrap_or_default().into() }).collect()).unwrap_or_default();
        Ok((entry(&value["root"]), children))
    }

    fn prepare(&self, session: &str, path: &str) -> DavResult<Target> {
        let value = self.dav(session, "PREPARE", path, "", 0, true, 0, 0)?;
        Ok(Target { entry: (!value["entry"].is_null()).then(|| entry(&value["entry"])), parent_id: value["parentId"].as_str().unwrap_or_default().into(), name: value["name"].as_str().unwrap_or_default().into() })
    }

    fn mkcol(&self, session: &str, path: &str) -> DavResult<bool> {
        Ok(self.dav(session, "MKCOL", path, "", 0, true, 0, 0)?["created"] == json!(true))
    }

    fn touch(&self, session: &str, path: &str, modified_at: i64) -> DavResult<()> {
        self.dav(session, "TOUCH", path, "", 0, true, 0, modified_at).map(|_| ())
    }

    fn delete(&self, session: &str, path: &str, expected_revision: u64) -> DavResult<()> {
        self.dav(session, "DELETE", path, "", expected_revision, true, 0, 0).map(|_| ())
    }

    fn transfer(&self, session: &str, how: Transfer, path: &str, destination: &str, expected_revision: u64, overwrite: bool) -> DavResult<bool> {
        let method = if how == Transfer::Move { "MOVE" } else { "COPY" };
        Ok(self.dav(session, method, path, destination, expected_revision, overwrite, 0, 0)?["replaced"] == json!(true))
    }

    fn store(&self, session: &str, target: &Target, content: &[u8], media_type: &str, file_time: i64) -> DavResult<Entry> {
        let upload_id = crate::auth::uuid();
        let file_id = target.entry.as_ref().map(|e| e.id.clone()).unwrap_or_else(crate::auth::uuid);
        let expected = target.entry.as_ref().map_or(0, |e| e.revision);
        let result = block(async {
            self.db.call("service_begin_upload", json!([session, upload_id, file_id, target.parent_id, target.name, content.len(), media_type, expected])).await?;
            let chunks = futures_util::stream::iter([Ok::<_, std::io::Error>(axum::body::Bytes::copy_from_slice(content))]);
            let digest = match self.blobs.receive(chunks, content.len() as u64).await {
                Ok(d) => d,
                Err(e) => {
                    let _ = self.db.call("service_cancel_upload", json!([session, upload_id])).await;
                    return Err(e);
                }
            };
            self.db.call("service_finish_upload", json!([session, upload_id, digest, file_time])).await?;
            let stored = self.db.read("service_download", json!([session, file_id, ""])).await;
            Ok(match stored {
                Ok(value) => entry(&value),
                // Write-only passwords cannot read back: describe what was stored.
                Err(_) => Entry { id: file_id.clone(), kind: Kind::File, name: target.name.clone(), size: content.len() as u64, media_type: media_type.into(), revision: expected + 1, blob_id: digest, modified_at: file_time, sha1: None, md5: None },
            })
        })
        .map_err(dav_error)?;
        self.checksums.nudge();
        self.texts.nudge();
        Ok(result)
    }

    fn read(&self, _session: &str, entry: &Entry) -> DavResult<Vec<u8>> {
        let path = self.blobs.path(&entry.blob_id).map_err(dav_error)?;
        std::fs::read(path).map_err(|_| DavError::new(503, "contentUnavailable"))
    }
}


/// CalDAV and CardDAV on `service_pim` for an app password's protocol session.
pub struct DarPim(pub Database);

fn pim_kind(value: &Value) -> PimKind {
    if value == "contacts" { PimKind::Contacts } else { PimKind::Calendar }
}

fn resource(value: &Value) -> Resource {
    Resource {
        id: value["id"].as_str().unwrap_or_default().into(),
        href: value["href"].as_str().unwrap_or_default().into(),
        revision: value["revision"].as_u64().unwrap_or(0),
        modified_at: value["modifiedAt"].as_f64().unwrap_or(0.0) as i64,
        content: value["content"].as_str().unwrap_or_default().into(),
    }
}

impl DarPim {
    fn call(&self, session: &str, operation: &str, payload: Value) -> DavResult<Value> {
        let value = block(self.0.call("service_pim", json!([session, operation, payload.to_string()]))).map_err(dav_error)?;
        Ok(value["value"].clone())
    }
}

impl PimBackend for DarPim {
    fn collections(&self, session: &str) -> DavResult<Vec<Collection>> {
        let rows = self.call(session, "collections", json!({}))?;
        Ok(rows
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["kind"] == "contacts" || c["kind"] == "calendar")
            .map(|c| Collection { id: c["id"].as_str().unwrap_or_default().into(), kind: pim_kind(&c["kind"]), name: c["name"].as_str().unwrap_or_default().into(), revision: c["revision"].as_u64().unwrap_or(0), modified_at: c["modifiedAt"].as_f64().unwrap_or(0.0) as i64 })
            .collect())
    }

    fn list(&self, session: &str, collection_id: &str, metadata_only: bool) -> DavResult<Vec<Resource>> {
        let value = self.call(session, "list", json!({ "collectionId": collection_id, "metadataOnly": metadata_only }))?;
        Ok(value["items"].as_array().into_iter().flatten().map(resource).collect())
    }

    fn put(&self, session: &str, collection: &Collection, href: &str, content: &str, revision: u64) -> DavResult<Resource> {
        let kind = if collection.kind == PimKind::Contacts { "contacts" } else { "calendar" };
        let parsed = crate::pim::read(kind, content).map_err(|_| DavError::new(403, "invalidInput"))?;
        if revision > 0 {
            let old = self.call(session, "read", json!({ "collectionId": collection.id, "href": href }))?;
            let previous = crate::pim::read(kind, old["content"].as_str().unwrap_or_default()).map_err(dav_error)?;
            if previous.uid != parsed.uid {
                return Err(DavError::new(403, "invalidInput"));
            }
        }
        let payload = json!({ "collectionId": collection.id, "href": href, "revision": revision, "id": crate::auth::uuid(), "uid": parsed.uid, "content": parsed.content, "projection": parsed.data.to_string() });
        Ok(resource(&self.call(session, "put", payload)?))
    }

    fn delete(&self, session: &str, collection_id: &str, href: &str, revision: u64) -> DavResult<()> {
        self.call(session, "delete", json!({ "collectionId": collection_id, "href": href, "revision": revision })).map(|_| ())
    }
}

pub fn router(origin: &str, db: Database, blobs: Blobs, checksums: crate::maintenance::Worker, texts: crate::maintenance::Worker) -> axum::Router {
    let db_pim = db.clone();
    dar_dav::router(dar_dav::Dav { origin: origin.into(), accounts: Arc::new(DarAccounts(db.clone())), files: Arc::new(DarFiles { db, blobs, checksums, texts }), pim: Arc::new(DarPim(db_pim)) })
}
