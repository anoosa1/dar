//! In-memory backends for tests and the demo. Dar's real ones are SpacetimeDB reducers plus
//! the content-addressed blob store.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::files::{Entry, FilesBackend, Kind, Listed, Target, Transfer, uuid};
use crate::ical;
use crate::pim::{Collection, PimBackend, PimKind, Resource};
use crate::{Accounts, App, DavError, DavResult, Principal, digest_token, now};

struct AppPassword {
    username: String,
    digest: String,
    apps: Vec<App>,
    write: bool,
    session_id: String,
}

#[derive(Default)]
pub struct MemoryAccounts {
    passwords: Mutex<Vec<AppPassword>>,
}

impl MemoryAccounts {
    /// Issue an app password; returns its session ID.
    pub fn add(&self, username: &str, secret: &str, apps: &[App], write: bool) -> String {
        let session_id = format!("session-{}", uuid());
        self.passwords.lock().unwrap().push(AppPassword {
            username: username.into(),
            digest: digest_token(secret),
            apps: apps.to_vec(),
            write,
            session_id: session_id.clone(),
        });
        session_id
    }

    pub fn user_of(&self, session: &str) -> Option<String> {
        self.passwords
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.session_id == session)
            .map(|p| p.username.clone())
    }
}

impl Accounts for MemoryAccounts {
    fn authenticate(
        &self,
        username: &str,
        secret_digest: &str,
        app: App,
        write: bool,
    ) -> DavResult<Principal> {
        let passwords = self.passwords.lock().unwrap();
        let found = passwords
            .iter()
            .find(|p| p.username == username && p.digest == secret_digest)
            .ok_or(DavError::unauthenticated())?;
        if !found.apps.contains(&app) || (write && !found.write) {
            return Err(DavError::new(403, "forbidden"));
        }
        Ok(Principal {
            session_id: found.session_id.clone(),
            username: found.username.clone(),
        })
    }
}

#[derive(Clone)]
struct Node {
    entry: Entry,
    content: Vec<u8>,
}

/// One Files tree per account, keyed by logical path.
pub struct MemoryFiles {
    accounts: Arc<MemoryAccounts>,
    trees: Mutex<HashMap<String, BTreeMap<String, Node>>>,
    /// Depth-infinity listings past this many entries are refused.
    pub listing_limit: usize,
}

impl MemoryFiles {
    pub fn new(accounts: Arc<MemoryAccounts>) -> Self {
        MemoryFiles {
            accounts,
            trees: Mutex::new(HashMap::new()),
            listing_limit: 10_000,
        }
    }

    fn with<T>(
        &self,
        session: &str,
        f: impl FnOnce(&mut BTreeMap<String, Node>, &str) -> DavResult<T>,
    ) -> DavResult<T> {
        let user = self
            .accounts
            .user_of(session)
            .ok_or(DavError::unauthenticated())?;
        let mut trees = self.trees.lock().unwrap();
        f(trees.entry(user.clone()).or_default(), &user)
    }
}

fn root(user: &str) -> Entry {
    Entry {
        id: String::new(),
        kind: Kind::Folder,
        name: user.into(),
        size: 0,
        media_type: "inode/directory".into(),
        revision: 0,
        blob_id: String::new(),
        modified_at: 0,
        sha1: None,
        md5: None,
    }
}

fn parent_of(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn folder_id(tree: &BTreeMap<String, Node>, path: &str) -> DavResult<String> {
    if path.is_empty() {
        return Ok(String::new());
    }
    match tree.get(path) {
        Some(n) if n.entry.kind == Kind::Folder => Ok(n.entry.id.clone()),
        _ => Err(DavError::new(409, "parentNotFound")),
    }
}

fn under<'a>(
    tree: &'a BTreeMap<String, Node>,
    path: &'a str,
) -> impl Iterator<Item = (&'a String, &'a Node)> + 'a {
    let prefix = if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    };
    tree.iter()
        .filter(move |(p, _)| p.starts_with(&prefix) && p.as_str() != path)
}

impl FilesBackend for MemoryFiles {
    fn stat(&self, session: &str, path: &str) -> DavResult<Entry> {
        self.with(session, |tree, user| {
            if path.is_empty() {
                Ok(root(user))
            } else {
                tree.get(path)
                    .map(|n| n.entry.clone())
                    .ok_or(DavError::not_found())
            }
        })
    }

    fn list(&self, session: &str, path: &str, depth: u8) -> DavResult<(Entry, Vec<Listed>)> {
        let limit = self.listing_limit;
        self.with(session, |tree, user| {
            let entry = if path.is_empty() {
                root(user)
            } else {
                tree.get(path)
                    .map(|n| n.entry.clone())
                    .ok_or(DavError::not_found())?
            };
            if entry.kind == Kind::File || depth == 0 {
                return Ok((entry, Vec::new()));
            }
            let skip = if path.is_empty() { 0 } else { path.len() + 1 };
            let children: Vec<Listed> = under(tree, path)
                .filter(|(p, _)| depth == 2 || !p[skip..].contains('/'))
                .map(|(p, n)| Listed {
                    entry: n.entry.clone(),
                    path: p[skip..].to_owned(),
                })
                .collect();
            if children.len() > limit {
                return Err(DavError::new(413, "operationTooLarge"));
            }
            Ok((entry, children))
        })
    }

    fn prepare(&self, session: &str, path: &str) -> DavResult<Target> {
        self.with(session, |tree, _| {
            if path.is_empty() {
                return Err(DavError::new(405, "isDirectory"));
            }
            let (parent, name) = parent_of(path);
            Ok(Target {
                entry: tree.get(path).map(|n| n.entry.clone()),
                parent_id: folder_id(tree, parent)?,
                name: name.into(),
            })
        })
    }

    fn mkcol(&self, session: &str, path: &str) -> DavResult<bool> {
        self.with(session, |tree, _| {
            let (parent, name) = parent_of(path);
            folder_id(tree, parent)?;
            if tree.contains_key(path) || path.is_empty() {
                return Ok(false);
            }
            let entry = Entry {
                id: uuid(),
                kind: Kind::Folder,
                name: name.into(),
                size: 0,
                media_type: "inode/directory".into(),
                revision: 1,
                blob_id: String::new(),
                modified_at: now(),
                sha1: None,
                md5: None,
            };
            tree.insert(
                path.into(),
                Node {
                    entry,
                    content: Vec::new(),
                },
            );
            Ok(true)
        })
    }

    fn touch(&self, session: &str, path: &str, modified_at: i64) -> DavResult<()> {
        self.with(session, |tree, _| {
            let node = tree.get_mut(path).ok_or(DavError::not_found())?;
            node.entry.modified_at = modified_at;
            node.entry.revision += 1;
            Ok(())
        })
    }

    fn delete(&self, session: &str, path: &str, expected_revision: u64) -> DavResult<()> {
        self.with(session, |tree, _| {
            if path.is_empty() {
                return Err(DavError::new(403, "forbidden"));
            }
            if tree.get(path).ok_or(DavError::not_found())?.entry.revision != expected_revision {
                return Err(DavError::new(412, "revisionConflict"));
            }
            let doomed: Vec<String> = under(tree, path)
                .map(|(p, _)| p.clone())
                .chain([path.to_owned()])
                .collect();
            for p in doomed {
                tree.remove(&p);
            }
            Ok(())
        })
    }

    fn transfer(
        &self,
        session: &str,
        how: Transfer,
        path: &str,
        destination: &str,
        expected_revision: u64,
        overwrite: bool,
    ) -> DavResult<bool> {
        self.with(session, |tree, _| {
            let source = tree.get(path).ok_or(DavError::not_found())?.clone();
            if source.entry.revision != expected_revision {
                return Err(DavError::new(412, "revisionConflict"));
            }
            if destination.is_empty()
                || destination == path
                || destination.starts_with(&format!("{path}/"))
            {
                return Err(DavError::new(403, "invalidDestination"));
            }
            let (parent, name) = parent_of(destination);
            folder_id(tree, parent)?;
            let replaced = tree.contains_key(destination);
            if replaced && !overwrite {
                return Err(DavError::new(412, "alreadyExists"));
            }
            let doomed: Vec<String> = under(tree, destination)
                .map(|(p, _)| p.clone())
                .chain([destination.to_owned()])
                .collect();
            for p in doomed {
                tree.remove(&p);
            }
            let moving: Vec<(String, Node)> = under(tree, path)
                .map(|(p, n)| (p.clone(), n.clone()))
                .chain([(path.to_owned(), source)])
                .collect();
            for (old, mut node) in moving {
                let new = format!("{destination}{}", &old[path.len()..]);
                if old == path {
                    node.entry.name = name.into();
                }
                if how == Transfer::Move {
                    tree.remove(&old);
                    node.entry.revision += 1;
                } else {
                    node.entry.id = uuid();
                    node.entry.revision = 1;
                }
                tree.insert(new, node);
            }
            Ok(replaced)
        })
    }

    fn store(
        &self,
        session: &str,
        target: &Target,
        content: &[u8],
        media_type: &str,
        file_time: i64,
    ) -> DavResult<Entry> {
        self.with(session, |tree, _| {
            let path = tree
                .iter()
                .find(|(_, n)| target.entry.as_ref().is_some_and(|e| e.id == n.entry.id))
                .map(|(p, _)| p.clone())
                .or_else(|| {
                    if target.parent_id.is_empty() {
                        Some(target.name.clone())
                    } else {
                        tree.iter()
                            .find(|(_, n)| n.entry.id == target.parent_id)
                            .map(|(p, _)| format!("{p}/{}", target.name))
                    }
                })
                .ok_or(DavError::new(409, "parentNotFound"))?;
            let current = tree.get(&path).map(|n| n.entry.revision).unwrap_or(0);
            if current != target.entry.as_ref().map_or(0, |e| e.revision) {
                return Err(DavError::new(412, "revisionConflict"));
            }
            let entry = Entry {
                id: target.entry.as_ref().map_or_else(uuid, |e| e.id.clone()),
                kind: Kind::File,
                name: target.name.clone(),
                size: content.len() as u64,
                media_type: media_type.into(),
                revision: current + 1,
                blob_id: Sha256::digest(content)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
                modified_at: if file_time > 0 { file_time } else { now() },
                sha1: None,
                md5: None,
            };
            tree.insert(
                path,
                Node {
                    entry: entry.clone(),
                    content: content.to_vec(),
                },
            );
            Ok(entry)
        })
    }

    fn read(&self, session: &str, entry: &Entry) -> DavResult<Vec<u8>> {
        self.with(session, |tree, _| {
            tree.values()
                .find(|n| n.entry.id == entry.id)
                .map(|n| n.content.clone())
                .ok_or(DavError::not_found())
        })
    }
}

/// A collection and its resources by href.
type Book = (Collection, BTreeMap<String, Resource>);

/// Contacts and calendar collections per account.
pub struct MemoryPim {
    accounts: Arc<MemoryAccounts>,
    collections: Mutex<HashMap<String, Vec<Book>>>,
}

impl MemoryPim {
    pub fn new(accounts: Arc<MemoryAccounts>) -> Self {
        MemoryPim {
            accounts,
            collections: Mutex::new(HashMap::new()),
        }
    }

    pub fn add_collection(&self, username: &str, id: &str, kind: PimKind, name: &str) {
        let collection = Collection {
            id: id.into(),
            kind,
            name: name.into(),
            revision: 1,
            modified_at: now(),
        };
        self.collections
            .lock()
            .unwrap()
            .entry(username.into())
            .or_default()
            .push((collection, BTreeMap::new()));
    }

    fn user(&self, session: &str) -> DavResult<String> {
        self.accounts
            .user_of(session)
            .ok_or(DavError::unauthenticated())
    }
}

fn invalid(_: impl std::fmt::Display) -> DavError {
    DavError::bad("invalidInput")
}

/// The UID and component kind of a resource, validated like `readEvent`/`readTask`/`readContact`.
fn identity(kind: PimKind, content: &str) -> DavResult<(String, bool)> {
    if kind == PimKind::Contacts {
        let card = ical::parse(content, "VCARD").map_err(invalid)?;
        return Ok((
            card.value("UID")
                .filter(|u| !u.is_empty())
                .ok_or(DavError::bad("invalidInput"))?
                .to_owned(),
            false,
        ));
    }
    let calendar = ical::parse(content, "VCALENDAR").map_err(invalid)?;
    let task = ical::is_task(content);
    let name = if task { "VTODO" } else { "VEVENT" };
    let items: Vec<_> = calendar.components(name).collect();
    let uid = items
        .first()
        .and_then(|c| c.value("UID"))
        .filter(|u| !u.is_empty())
        .ok_or(DavError::bad("invalidInput"))?;
    if items.iter().any(|c| c.value("UID") != Some(uid))
        || items.iter().any(|c| c.prop("DTSTART").is_none() && !task)
    {
        return Err(DavError::bad("invalidInput"));
    }
    Ok((uid.to_owned(), task))
}

impl PimBackend for MemoryPim {
    fn collections(&self, session: &str) -> DavResult<Vec<Collection>> {
        let user = self.user(session)?;
        Ok(self
            .collections
            .lock()
            .unwrap()
            .get(&user)
            .map(|c| c.iter().map(|(c, _)| c.clone()).collect())
            .unwrap_or_default())
    }

    fn list(
        &self,
        session: &str,
        collection_id: &str,
        _metadata_only: bool,
    ) -> DavResult<Vec<Resource>> {
        let user = self.user(session)?;
        let all = self.collections.lock().unwrap();
        let (_, items) = all
            .get(&user)
            .and_then(|c| c.iter().find(|(c, _)| c.id == collection_id))
            .ok_or(DavError::not_found())?;
        Ok(items.values().cloned().collect())
    }

    fn put(
        &self,
        session: &str,
        collection: &Collection,
        href: &str,
        content: &str,
        revision: u64,
    ) -> DavResult<Resource> {
        let user = self.user(session)?;
        if href.is_empty() || href.len() > 255 || href.contains(['/', '\\']) {
            return Err(DavError::bad("invalidHref"));
        }
        let (uid, task) = identity(collection.kind, content)?;
        let mut all = self.collections.lock().unwrap();
        let (meta, items) = all
            .get_mut(&user)
            .and_then(|c| c.iter_mut().find(|(c, _)| c.id == collection.id))
            .ok_or(DavError::not_found())?;
        let previous = items.get(href);
        if previous.map_or(0, |p| p.revision) != revision {
            return Err(DavError::new(412, "revisionConflict"));
        }
        if let Some(previous) = previous {
            let (old_uid, old_task) = identity(collection.kind, &previous.content)?;
            if old_uid != uid {
                return Err(DavError::bad("uidChanged"));
            }
            if old_task != task {
                return Err(DavError::bad("componentChanged"));
            }
        } else if items
            .values()
            .any(|r| identity(collection.kind, &r.content).is_ok_and(|(u, _)| u == uid))
        {
            // RFC 4791 §5.3.2.1 / RFC 6352 §6.3.2.1: one resource per UID in a collection.
            return Err(DavError::new(409, "uidConflict"));
        }
        let row = Resource {
            id: previous.map_or_else(uuid, |p| p.id.clone()),
            href: href.into(),
            revision: revision + 1,
            modified_at: now(),
            content: content.into(),
        };
        items.insert(href.into(), row.clone());
        meta.revision += 1;
        meta.modified_at = now();
        Ok(row)
    }

    fn delete(
        &self,
        session: &str,
        collection_id: &str,
        href: &str,
        revision: u64,
    ) -> DavResult<()> {
        let user = self.user(session)?;
        let mut all = self.collections.lock().unwrap();
        let (meta, items) = all
            .get_mut(&user)
            .and_then(|c| c.iter_mut().find(|(c, _)| c.id == collection_id))
            .ok_or(DavError::not_found())?;
        match items.get(href) {
            None => Err(DavError::not_found()),
            Some(r) if r.revision != revision => Err(DavError::new(412, "revisionConflict")),
            Some(_) => {
                items.remove(href);
                meta.revision += 1;
                Ok(())
            }
        }
    }
}
