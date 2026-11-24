//! Files (`files.tsx`): the Focused explorer with sections, breadcrumbs, list and grid
//! views, selection actions, the details panel, uploads and the editing panes.

pub mod editor;
mod folder_upload;
pub mod locked;
pub mod picker;
pub mod preview;
pub mod types;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use spacetimedb_sdk::{DbContext, SubscriptionHandle as _, Table};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::api::{self, BrowserSession, Req, format_bytes};
use crate::dates::{local_date_time, relative, short_date};
use crate::icons::Icon;
use crate::live::{Connection, Live, LiveStatus, answered, procedure, reducer, replied};
use crate::module_bindings::*;
use crate::settings::{AppSettingsButton, FilesSettings};
use crate::shared::{context, save_preferences};
use crate::shell::WorkspaceHeader;
use crate::ui::{
    ActionMenu, ConnectionState, ConnectionStatus, Dialog, EditorPane, MenuAction, Select,
    document, listen, window,
};
use editor::{TextEditor, TextFile, editable_text, write_text_file};
use locked::{LockedFolder, UnlockDialog, UnlockForm};
use picker::{FolderPicker, Selection};
use preview::FilePreview;
use types::{FileTile, file_type, file_type_label};

/// A Files entry as the module's views and procedures describe it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub id: String,
    pub owner_id: String,
    pub parent_id: String,
    pub name: String,
    pub kind: String,
    pub blob_id: String,
    pub size: f64,
    pub media_type: String,
    pub revision: u32,
    pub modified_at: f64,
    pub trashed: bool,
    pub favorite: bool,
    pub list_key: String,
}

impl From<crate::module_bindings::FileEntry> for FileEntry {
    fn from(e: crate::module_bindings::FileEntry) -> Self {
        Self {
            id: e.id,
            owner_id: e.owner_id,
            parent_id: e.parent_id,
            name: e.name,
            kind: e.kind,
            blob_id: e.blob_id,
            size: e.size,
            media_type: e.media_type,
            revision: e.revision,
            modified_at: e.modified_at,
            trashed: e.trashed,
            favorite: e.favorite,
            list_key: e.list_key,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Crumb {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub id: String,
    pub created_at: f64,
    pub size: f64,
    pub blob_id: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Grant {
    pub id: String,
    pub username: String,
    pub role: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicShare {
    pub id: String,
    pub expires_at: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Details {
    pub entry: FileEntry,
    pub role: String,
    pub ancestors: Vec<Crumb>,
    pub versions: Vec<Version>,
    pub grants: Vec<Grant>,
    pub public_shares: Vec<PublicShare>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RootFolder {
    id: String,
    name: String,
    item_count: usize,
    git_tracked: bool,
    favorite: bool,
    shared: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OverviewCounts {
    favorites: usize,
    shared_with_you: usize,
    shared_by_you: usize,
    public_links: usize,
    trash: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StorageBreakdown {
    active_bytes: f64,
    trash_bytes: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesOverview {
    root_folders: Vec<RootFolder>,
    folder_counts: HashMap<String, usize>,
    tracked_folder_ids: Vec<String>,
    shared_ids: Vec<String>,
    trash_parents: HashMap<String, String>,
    counts: OverviewCounts,
    storage: StorageBreakdown,
}

fn disconnected() -> String {
    "Files are disconnected.".into()
}

/// `fileDetails` on the page's connection.
pub async fn file_details(id: &str) -> Result<Details, String> {
    let c = crate::live::connection().ok_or_else(disconnected)?;
    let id = id.to_owned();
    procedure(|tx| c.procedures.file_details_then(id, answered(tx))).await
}

async fn fetch_overview() -> Result<FilesOverview, String> {
    let c = crate::live::connection().ok_or_else(disconnected)?;
    procedure(|tx| c.procedures.files_overview_then(answered(tx))).await
}

fn matches_type_filter(entry: &FileEntry, filter: &str) -> bool {
    if filter == "all" {
        return true;
    }
    if entry.kind == "folder" {
        return false;
    }
    let ty = file_type(&entry.kind, &entry.name, &entry.media_type);
    match filter {
        "doc" => matches!(ty, "doc" | "pdf" | "sheet"),
        "image" => ty == "image",
        "code" => ty == "code",
        "archive" => ty == "archive",
        "media" => matches!(ty, "audio" | "video"),
        _ => true,
    }
}

fn sort_entries(list: &mut [FileEntry], field: &str, asc: bool, folders_first: bool) {
    list.sort_by(|a, b| {
        if folders_first {
            let af = a.kind != "folder";
            let bf = b.kind != "folder";
            if af != bf {
                return af.cmp(&bf);
            }
        }
        let ord = match field {
            "modified" => a.modified_at.total_cmp(&b.modified_at),
            "size" => a.size.total_cmp(&b.size),
            "type" => file_type_label(&a.kind, &a.name, &a.media_type)
                .cmp(file_type_label(&b.kind, &b.name, &b.media_type)),
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        let ord = if asc { ord } else { ord.reverse() };
        ord.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Rows in the server's list order: list keys sort by code point, as the index does.
fn listed(rows: impl Iterator<Item = crate::module_bindings::FileEntry>) -> Vec<FileEntry> {
    let mut rows: Vec<FileEntry> = rows.map(Into::into).collect();
    rows.sort_by(|a, b| a.list_key.chars().cmp(b.list_key.chars()));
    rows
}

const SECTIONS: [(&str, &str, &str); 6] = [
    ("recent", "Recent", "clock"),
    ("files", "All files", "folder"),
    ("favorites", "Favorites", "star"),
    ("shared", "Shared", "share"),
    ("locked", "Locked folder", "lock"),
    ("trash", "Deleted files", "trash"),
];

fn is_uuid_like(id: &str) -> bool {
    id.len() == 36
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase() || b == b'-')
}

fn read_location() -> (&'static str, String) {
    let search = window().location().search().unwrap_or_default();
    let params = web_sys::UrlSearchParams::new_with_str(&search).expect("params");
    let section = params.get("section").unwrap_or_default();
    let id = params.get("folder").unwrap_or_default();
    (
        SECTIONS
            .iter()
            .find(|s| s.0 == section)
            .map(|s| s.0)
            .unwrap_or("files"),
        if is_uuid_like(&id) { id } else { String::new() },
    )
}

fn modifier() -> &'static str {
    let mac = window()
        .navigator()
        .platform()
        .map(|p| p.contains("Mac") || p.contains("iPhone") || p.contains("iPad"))
        .unwrap_or(false);
    if mac { "⌘" } else { "Ctrl+" }
}

fn failure(text: &str) -> String {
    let lower = text.to_lowercase();
    let has = |s: &str| lower.contains(&s.to_lowercase());
    if has("lockedFolder") {
        "Unlock the Locked folder with your password to continue.".into()
    } else if has("conflict") {
        "This item changed or that name is already in use. Refresh the details and try again."
            .into()
    } else if has("forbidden") || has("unauthenticated") {
        "Your access changed. Refresh the page or sign in again.".into()
    } else if has("invalidName") {
        "Choose a name without slashes or control characters.".into()
    } else if has("quotaExceeded") {
        "There isn’t enough storage available for this upload.".into()
    } else if has("notFound") {
        "This item is no longer available, or you no longer have access.".into()
    } else {
        text.to_owned()
    }
}

fn thumbnail_ok(entry: &FileEntry) -> bool {
    matches!(
        entry.media_type.as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) && entry.size <= 8.0 * 1024.0 * 1024.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Clipboard {
    user_id: String,
    id: String,
    name: String,
    revision: u32,
    mode: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModalKind {
    Folder,
    Rename,
    Move,
    Copy,
    Delete,
}

#[derive(Clone, Debug, PartialEq)]
struct Modal {
    kind: ModalKind,
    entry: Option<FileEntry>,
    entries: Vec<FileEntry>,
}

#[derive(Clone, Debug, PartialEq)]
struct Transfer {
    id: String,
    name: String,
    progress: f64,
    status: &'static str,
    message: String,
}

thread_local! {
    /// Requests of uploads in flight, so they can be cancelled.
    static UPLOADS: RefCell<HashMap<String, web_sys::XmlHttpRequest>> = RefCell::new(HashMap::new());
    static RESERVATIONS: RefCell<HashMap<String, web_sys::AbortController>> = RefCell::new(HashMap::new());
}

fn cancel_upload(id: &str) {
    if let Some(c) = RESERVATIONS.with(|r| r.borrow_mut().remove(id)) {
        c.abort();
    }
    if let Some(x) = UPLOADS.with(|u| u.borrow_mut().remove(id)) {
        let _ = x.abort();
    }
}

/// Sends `file` to an upload reservation with progress (`XMLHttpRequest`, as the React client).
async fn put_upload(
    transfer: &str,
    url: &str,
    csrf: &str,
    file: &web_sys::File,
    progress: impl Fn(f64) + 'static,
) -> Result<(), String> {
    let xhr = web_sys::XmlHttpRequest::new()
        .map_err(|_| "The upload could not be completed.".to_owned())?;
    let (sender, receiver) = futures::channel::oneshot::channel::<Result<(), String>>();
    let sender = std::rc::Rc::new(RefCell::new(Some(sender)));
    let finish = {
        let sender = sender.clone();
        move |result: Result<(), String>| {
            if let Some(s) = sender.borrow_mut().take() {
                let _ = s.send(result);
            }
        }
    };
    let _ = xhr.open("PUT", url);
    let _ = xhr.set_request_header("x-csrf-token", csrf);
    let on_progress =
        Closure::<dyn FnMut(web_sys::ProgressEvent)>::new(move |e: web_sys::ProgressEvent| {
            progress(if e.length_computable() {
                e.loaded() / e.total()
            } else {
                0.0
            })
        });
    if let Ok(upload) = xhr.upload() {
        upload.set_onprogress(Some(on_progress.as_ref().unchecked_ref()));
    }
    let on_load = {
        let (xhr, finish) = (xhr.clone(), finish.clone());
        Closure::<dyn FnMut()>::new(move || {
            let status = xhr.status().unwrap_or(0);
            if (200..300).contains(&status) {
                finish(Ok(()))
            } else {
                let text = xhr.response_text().ok().flatten().unwrap_or_default();
                let message = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|v| v["error"]["message"].as_str().map(str::to_owned));
                finish(Err(message.unwrap_or_else(|| {
                    "The upload could not be completed.".into()
                })))
            }
        })
    };
    let on_error = {
        let finish = finish.clone();
        Closure::<dyn FnMut()>::new(move || {
            finish(Err(
                "The connection was interrupted. Try the upload again.".into()
            ))
        })
    };
    let on_abort = {
        let finish = finish.clone();
        Closure::<dyn FnMut()>::new(move || finish(Err("Upload cancelled.".into())))
    };
    xhr.set_onload(Some(on_load.as_ref().unchecked_ref()));
    xhr.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    xhr.set_onabort(Some(on_abort.as_ref().unchecked_ref()));
    UPLOADS.with(|u| u.borrow_mut().insert(transfer.to_owned(), xhr.clone()));
    let _ = xhr.send_with_opt_blob(Some(file));
    let result = receiver
        .await
        .unwrap_or_else(|_| Err("Upload cancelled.".into()));
    drop((on_progress, on_load, on_error, on_abort));
    result
}

#[component]
pub fn Files(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let user_id = session.user.id.clone();
    let shared = context();
    let live = expect_context::<Live>();
    let locked = LockedFolder::new(session.clone());
    let session = StoredValue::new(session);
    let csrf = move || session.with_value(|s| s.csrf_token.clone());
    let me = StoredValue::new(user_id.clone());

    let text_editor: RwSignal<Option<FileEntry>> = RwSignal::new(None);
    let new_file: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let unlock_action: RwSignal<Option<(String, Callback<String>)>> = RwSignal::new(None);
    let settings = RwSignal::new(false);
    let roles: RwSignal<HashMap<String, String>> = RwSignal::new(HashMap::new());
    let clipboard_key = format!("dar.file-clipboard.{user_id}");
    let clipboard: RwSignal<Option<Clipboard>> = RwSignal::new(
        window()
            .session_storage()
            .ok()
            .flatten()
            .and_then(|s| s.get_item(&clipboard_key).ok().flatten())
            .and_then(|v| serde_json::from_str::<Clipboard>(&v).ok())
            .filter(|c| c.user_id == user_id && (c.mode == "cut" || c.mode == "copy")),
    );
    let clipboard_key = StoredValue::new(clipboard_key);
    let connection: RwSignal<Option<Connection>> = RwSignal::new(None);
    let epoch = RwSignal::new(0u32);
    let rows: RwSignal<Vec<FileEntry>> = RwSignal::new(Vec::new());
    let usage: RwSignal<Option<Usage>> = RwSignal::new(None);
    let state = RwSignal::new(ConnectionState::Connecting);
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let (initial_section, initial_parent) = read_location();
    let section = RwSignal::new(initial_section);
    let parent_id = RwSignal::new(initial_parent);
    let loading = RwSignal::new(true);
    let path: RwSignal<Vec<Crumb>> = RwSignal::new(Vec::new());
    let query = RwSignal::new(String::new());
    let cursor = RwSignal::new(String::new());
    let previous: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let selected: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let details: RwSignal<Option<Details>> = RwSignal::new(None);
    // Folder history's result notice outlives the details sheet re-rendering after a restore.
    let history_notice = RwSignal::new(String::new());
    let details_id = Memo::new(move |_| details.with(|d| d.as_ref().map(|d| d.entry.id.clone())));
    Effect::new(move |_| {
        details_id.track();
        history_notice.set(String::new());
    });
    let tab = RwSignal::new("details".to_owned());
    let grid = RwSignal::new(false);
    let sidebar = RwSignal::new(false);
    let modal: RwSignal<Option<Modal>> = RwSignal::new(None);
    let busy = RwSignal::new(false);
    let target_path: RwSignal<Vec<Crumb>> = RwSignal::new(Vec::new());
    let target_folder: RwSignal<Option<Selection>> = RwSignal::new(None);
    let transfers: RwSignal<Vec<Transfer>> = RwSignal::new(Vec::new());
    let public_link = RwSignal::new(String::new());
    let public_expiry_days = RwSignal::new("7".to_owned());
    let preview_entry: RwSignal<Option<FileEntry>> = RwSignal::new(None);
    let trash_confirmation: RwSignal<Option<Vec<FileEntry>>> = RwSignal::new(None);
    let empty_trash_confirm = RwSignal::new(false);
    let undo_trash: RwSignal<Option<Vec<FileEntry>>> = RwSignal::new(None);
    let shared_sub = RwSignal::new("with_you");
    let sort_field = RwSignal::new(if initial_section == "recent" {
        "modified"
    } else {
        "name"
    });
    let sort_asc = RwSignal::new(initial_section != "recent");
    let type_filter = RwSignal::new("all");
    let overview: RwSignal<FilesOverview> = RwSignal::new(FilesOverview::default());
    let overview_seq = RwSignal::new(0u32);
    let tree_open = RwSignal::new(true);
    let dragging_id: RwSignal<Option<String>> = RwSignal::new(None);
    let drop_target: RwSignal<Option<String>> = RwSignal::new(None);
    let upload_input = NodeRef::<html::Input>::new();
    let folder_input = NodeRef::<html::Input>::new();
    // The directory picker (`webkitdirectory`, which `view!` does not know).
    Effect::new(move |_| {
        if let Some(input) = folder_input.get() {
            let _ = input.set_attribute("webkitdirectory", "");
        }
    });
    let replacement_input = NodeRef::<html::Input>::new();
    let detail_sequence = RwSignal::new(0u32);
    let navigation_ready = Arc::new(AtomicBool::new(false));
    let initial_file = RwSignal::new(
        web_sys::UrlSearchParams::new_with_str(&window().location().search().unwrap_or_default())
            .ok()
            .and_then(|p| p.get("file")),
    );

    let preferences = Memo::new(move |_| shared.preferences.value.get().unwrap_or_default());
    let visible =
        Memo::new(move |_| rows.with(|r| r.iter().take(200).cloned().collect::<Vec<_>>()));
    let has_more = Memo::new(move |_| rows.with(|r| r.len() > 200));
    let in_locked = Memo::new(move |_| section.get() == "locked");
    let locked_open = Memo::new(move |_| {
        in_locked.get()
            && locked.unlocked()
            && locked
                .state
                .with(|s| s.as_ref().is_some_and(|s| !s.folder_id.is_empty()))
    });
    let folder = Memo::new(move |_| {
        if in_locked.get() {
            let p = parent_id.get();
            if p.is_empty() {
                locked
                    .state
                    .with(|s| s.as_ref().map(|s| s.folder_id.clone()).unwrap_or_default())
            } else {
                p
            }
        } else {
            parent_id.get()
        }
    });
    let writable_here = Memo::new(move |_| section.get() == "files" || locked_open.get());
    let connected = Memo::new(move |_| {
        connection.with(Option::is_some) && state.get() == ConnectionState::Ready
    });

    Effect::new(move |_| grid.set(preferences.with(|p| p.files_view.as_deref() == Some("grid"))));
    Effect::new(move |_| {
        let e = shared.preferences.error.get();
        if !e.is_empty() {
            error.set(e);
        }
    });
    Effect::new(move |_| {
        if notice.with(String::is_empty) {
            undo_trash.set(None);
        }
    });

    let remember = move |value: Option<Clipboard>| {
        clipboard.set(value.clone());
        let stored = window()
            .session_storage()
            .ok()
            .flatten()
            .map(|s| match &value {
                Some(v) => s.set_item(
                    &clipboard_key.get_value(),
                    &serde_json::to_string(v).unwrap_or_default(),
                ),
                None => s.remove_item(&clipboard_key.get_value()),
            });
        if !matches!(stored, Some(Ok(()))) {
            notice.set("The clipboard is available until you leave Files.".into());
        }
    };
    let collect = move |entry: &FileEntry, mode: &str| {
        remember(Some(Clipboard {
            user_id: me.get_value(),
            id: entry.id.clone(),
            name: entry.name.clone(),
            revision: entry.revision,
            mode: mode.into(),
        }));
        notice.set(format!(
            "{} is ready to {mode}. Choose a folder and paste.",
            entry.name
        ));
    };

    // Runs `work`; its error becomes the page's error; returns whether it succeeded.
    let perform =
        move |work: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>,
              message: String| async move {
            error.set(String::new());
            busy.set(true);
            let result = work.await;
            busy.set(false);
            match result {
                Ok(()) => {
                    if !message.is_empty() {
                        notice.set(message);
                    }
                    true
                }
                Err(e) => {
                    error.set(failure(&e));
                    false
                }
            }
        };
    let conn = move || {
        connection
            .get_untracked()
            .ok_or_else(|| "Wait for the connection to return.".to_owned())
    };

    let show_details = move |entry_id: String| {
        let sequence = detail_sequence.get_untracked() + 1;
        detail_sequence.set(sequence);
        spawn_local(async move {
            let result = file_details(&entry_id).await;
            if detail_sequence.try_get_untracked() != Some(sequence) {
                return;
            }
            match result {
                Ok(value) => details.set(Some(value)),
                Err(e) => {
                    details.set(None);
                    error.set(failure(&e));
                }
            }
        });
    };
    let select = move |entry: &FileEntry| {
        tab.set(
            preferences
                .with_untracked(|p| p.files_details_tab.clone())
                .unwrap_or_else(|| "details".into()),
        );
        selected.set(HashSet::from([entry.id.clone()]));
        public_link.set(String::new());
        show_details(entry.id.clone());
    };
    let toggle = move |id: String| {
        selected.update(|s| {
            if !s.remove(&id) {
                s.insert(id);
            }
        })
    };
    let set_preview = move |entry: Option<FileEntry>| {
        if entry.is_some() {
            text_editor.set(None);
        }
        preview_entry.set(entry);
    };
    let navigate = move |next: &'static str, id: String, crumbs: Vec<Crumb>| {
        let mut params = Vec::new();
        if next != "files" {
            params.push(format!("section={next}"));
        }
        if !id.is_empty() {
            params.push(format!("folder={id}"));
        }
        let pathname = window().location().pathname().unwrap_or_default();
        let url = if params.is_empty() {
            pathname
        } else {
            format!("{pathname}?{}", params.join("&"))
        };
        let _ = window().history().expect("history").push_state_with_url(
            &wasm_bindgen::JsValue::NULL,
            "",
            Some(&url),
        );
        loading.set(true);
        section.set(next);
        parent_id.set(id);
        path.set(crumbs);
        selected.set(HashSet::new());
        details.set(None);
        detail_sequence.update(|s| *s += 1);
        query.set(String::new());
        cursor.set(String::new());
        previous.set(Vec::new());
        type_filter.set("all");
        if next == "recent" {
            sort_field.set("modified");
            sort_asc.set(false);
        } else if sort_field.get_untracked() == "modified" && !sort_asc.get_untracked() {
            sort_field.set("name");
            sort_asc.set(true);
        }
        sidebar.set(false);
        error.set(String::new());
    };
    let open_entry = move |entry: FileEntry| {
        if entry.trashed {
            select(&entry);
            return;
        }
        if entry.kind != "folder" {
            if editable_text(&entry) {
                preview_entry.set(None);
                text_editor.set(Some(entry));
            } else {
                set_preview(Some(entry));
            }
            return;
        }
        spawn_local(async move {
            match file_details(&entry.id).await {
                Ok(d) => {
                    let mut crumbs = d.ancestors;
                    crumbs.push(Crumb {
                        id: entry.id.clone(),
                        name: entry.name.clone(),
                    });
                    navigate(
                        if in_locked.get_untracked() {
                            "locked"
                        } else {
                            "files"
                        },
                        entry.id.clone(),
                        crumbs,
                    );
                }
                Err(e) => error.set(failure(&e)),
            }
        });
    };
    let open_modal = move |value: Modal| {
        sidebar.set(false);
        error.set(String::new());
        modal.set(Some(value));
        target_path.set(Vec::new());
        target_folder.set(None);
    };
    let trash_entries = move |targets: Vec<FileEntry>| async move {
        let count = targets.len();
        let list = targets.clone();
        let ok = perform(
            Box::pin(async move {
                let c = conn()?;
                for entry in list {
                    reducer(|tx| {
                        c.reducers
                            .trash_entry_then(entry.id.clone(), entry.revision, replied(tx))
                    })
                    .await?;
                }
                selected.set(HashSet::new());
                details.set(None);
                Ok(())
            }),
            format!(
                "{count} item{} moved to deleted files.",
                if count == 1 { "" } else { "s" }
            ),
        )
        .await;
        if ok {
            undo_trash.set(Some(targets));
        }
        ok
    };
    let request_trash = move |entries: Vec<FileEntry>| {
        if preferences.with_untracked(|p| p.files_confirm_trash.unwrap_or(false)) {
            trash_confirmation.set(Some(entries));
        } else {
            spawn_local(async move {
                trash_entries(entries).await;
            });
        }
    };
    let undo_last_trash = move || {
        let Some(targets) = undo_trash.get_untracked() else {
            return;
        };
        undo_trash.set(None);
        let label = if targets.len() == 1 {
            targets[0].name.clone()
        } else {
            format!("{} items", targets.len())
        };
        spawn_local(async move {
            perform(
                Box::pin(async move {
                    let c = conn()?;
                    for entry in targets {
                        let current = file_details(&entry.id).await?.entry;
                        if current.trashed {
                            reducer(|tx| {
                                c.reducers.restore_entry_then(
                                    entry.id.clone(),
                                    current.revision,
                                    replied(tx),
                                )
                            })
                            .await?;
                        }
                    }
                    Ok(())
                }),
                format!("{label} restored."),
            )
            .await;
        });
    };
    let move_to_locked = move |entries: Vec<FileEntry>, folder_id: Option<String>| {
        let folder_id = folder_id.or_else(|| {
            locked
                .unlocked()
                .then(|| {
                    locked
                        .state
                        .with_untracked(|s| s.as_ref().map(|s| s.folder_id.clone()))
                })
                .flatten()
        });
        let Some(folder_id) = folder_id else {
            let entries = StoredValue::new(entries);
            unlock_action.set(Some((
                "move items into it".into(),
                Callback::new(move |id: String| {
                    let entries = entries.get_value();
                    spawn_local(async move {
                        let label = if entries.len() == 1 {
                            entries[0].name.clone()
                        } else {
                            format!("{} items", entries.len())
                        };
                        let c = conn();
                        perform(
                            Box::pin(async move {
                                let c = c?;
                                for e in entries {
                                    reducer(|tx| {
                                        c.reducers.move_entry_then(
                                            e.id.clone(),
                                            id.clone(),
                                            e.revision,
                                            None,
                                            replied(tx),
                                        )
                                    })
                                    .await?;
                                }
                                selected.set(HashSet::new());
                                details.set(None);
                                Ok(())
                            }),
                            format!("{label} moved to the Locked folder."),
                        )
                        .await;
                    });
                }),
            )));
            return;
        };
        let label = if entries.len() == 1 {
            entries[0].name.clone()
        } else {
            format!("{} items", entries.len())
        };
        spawn_local(async move {
            perform(
                Box::pin(async move {
                    let c = conn()?;
                    for e in entries {
                        reducer(|tx| {
                            c.reducers.move_entry_then(
                                e.id.clone(),
                                folder_id.clone(),
                                e.revision,
                                None,
                                replied(tx),
                            )
                        })
                        .await?;
                    }
                    selected.set(HashSet::new());
                    details.set(None);
                    Ok(())
                }),
                format!("{label} moved to the Locked folder."),
            )
            .await;
        });
    };
    let paste = move || {
        let Some(captured) = clipboard.get_untracked() else {
            return;
        };
        spawn_local(async move {
            perform(
                Box::pin(async move {
                    let c = conn()?;
                    let source = file_details(&captured.id).await?;
                    let here = folder.try_get_untracked().ok_or_else(disconnected)?;
                    if source.entry.trashed {
                        return Err("Restore the source item before pasting.".into());
                    }
                    if source.entry.revision != captured.revision {
                        return Err(
                            "The source changed. Copy or cut it again before pasting.".into()
                        );
                    }
                    if captured.mode == "cut" && source.entry.parent_id == here {
                        return Err("Choose a different destination for this item.".into());
                    }
                    if captured.mode == "copy"
                        && rows
                            .try_with_untracked(|r| {
                                r.iter().any(|row| row.name == source.entry.name)
                            })
                            .unwrap_or(false)
                    {
                        open_modal(Modal {
                            kind: ModalKind::Copy,
                            entry: Some(source.entry),
                            entries: Vec::new(),
                        });
                        target_path.set(path.try_get_untracked().unwrap_or_default());
                        return Ok(());
                    }
                    let id = crate::random_uuid();
                    if captured.mode == "cut" {
                        reducer(|tx| {
                            c.reducers.move_entry_then(
                                captured.id.clone(),
                                here.clone(),
                                captured.revision,
                                None,
                                replied(tx),
                            )
                        })
                        .await?;
                        remember(None);
                    } else {
                        reducer(|tx| {
                            c.reducers.copy_entry_then(
                                captured.id.clone(),
                                id.clone(),
                                here.clone(),
                                source.entry.name.clone(),
                                captured.revision,
                                replied(tx),
                            )
                        })
                        .await?;
                    }
                    selected.set(HashSet::from([if captured.mode == "cut" {
                        captured.id.clone()
                    } else {
                        id
                    }]));
                    notice.set(if captured.mode == "cut" {
                        "Item moved.".into()
                    } else {
                        "Item copied.".into()
                    });
                    Ok(())
                }),
                String::new(),
            )
            .await;
        });
    };
    let change_view = move |value: bool| {
        grid.set(value);
        let revision = preferences.with_untracked(|p| p.revision);
        spawn_local(async move {
            if let Err(e) = save_preferences(serde_json::json!({ "revision": revision, "filesView": if value { "grid" } else { "list" } }), revision, &session.get_value()).await {
                error.set(e.message);
            }
        });
    };
    let open_menu = move |entry: FileEntry| {
        selected.set(HashSet::from([entry.id.clone()]));
        spawn_local(async move {
            match file_details(&entry.id).await {
                Ok(d) => roles.update(|r| {
                    r.insert(entry.id.clone(), d.role);
                }),
                Err(e) => error.set(failure(&e)),
            }
        });
    };
    let run =
        move |work: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>,
              message: &str| {
            let message = message.to_owned();
            spawn_local(async move {
                perform(work, message).await;
            });
        };
    let download_zip = move |ids: Vec<String>| {
        if ids.is_empty() {
            return;
        }
        let csrf_token = csrf();
        run(
            Box::pin(async move {
                let response = api::fetch(
                    "/api/v1/files/archive",
                    Req {
                        method: "POST",
                        body: Some(serde_json::json!({ "ids": ids }).to_string()),
                        csrf: Some(&csrf_token),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| e.message)?;
                if !response.ok() {
                    return Err("Could not build the ZIP archive.".into());
                }
                let blob_promise = response
                    .blob()
                    .map_err(|_| "Could not read the ZIP archive.".to_owned())?;
                let blob_js = wasm_bindgen_futures::JsFuture::from(blob_promise)
                    .await
                    .map_err(|_| "Could not read the ZIP archive.".to_owned())?;
                let blob: web_sys::Blob = blob_js.unchecked_into();
                let url = web_sys::Url::create_object_url_with_blob(&blob)
                    .map_err(|_| "Could not prepare download.".to_owned())?;
                if let Ok(link) = document().create_element("a") {
                    let link: web_sys::HtmlAnchorElement = link.unchecked_into();
                    link.set_href(&url);
                    link.set_download("dar-files.zip");
                    if let Some(body) = document().body() {
                        let _ = body.append_child(&link);
                    }
                    link.click();
                    link.remove();
                }
                let revoke_url = url.clone();
                set_timeout(
                    move || {
                        let _ = web_sys::Url::revoke_object_url(&revoke_url);
                    },
                    std::time::Duration::from_secs(15),
                );
                Ok(())
            }),
            "",
        );
    };
    let move_dragged_into = move |dest_id: String| {
        drop_target.set(None);
        let Some(drag_id) = dragging_id.get_untracked() else {
            return;
        };
        dragging_id.set(None);
        if drag_id == dest_id {
            return;
        }
        let Some(entry) = rows.with_untracked(|r| r.iter().find(|e| e.id == drag_id).cloned())
        else {
            return;
        };
        if entry.owner_id != me.get_value() || entry.parent_id == dest_id || entry.trashed {
            return;
        }
        let name = entry.name.clone();
        run(
            Box::pin(async move {
                let c = conn()?;
                reducer(|tx| {
                    c.reducers.move_entry_then(
                        entry.id.clone(),
                        dest_id.clone(),
                        entry.revision,
                        None,
                        replied(tx),
                    )
                })
                .await
            }),
            &format!("Moved {name}."),
        );
    };
    let menu_items = move |entry: FileEntry| -> Vec<MenuAction> {
        let owner = entry.owner_id == me.get_value();
        let write =
            owner || roles.with_untracked(|r| r.get(&entry.id).is_some_and(|r| r == "write"));
        let busy_now = busy.get_untracked();
        let e = StoredValue::new(entry.clone());
        if entry.trashed {
            return vec![
                MenuAction::new("Details", "info").run(move || select(&e.get_value())),
                MenuAction::new("Restore", "history")
                    .disabled(!owner || busy_now)
                    .run(move || {
                        let entry = e.get_value();
                        run(
                            Box::pin(async move {
                                let c = conn()?;
                                reducer(|tx| {
                                    c.reducers.restore_entry_then(
                                        entry.id.clone(),
                                        entry.revision,
                                        replied(tx),
                                    )
                                })
                                .await
                            }),
                            "Item restored.",
                        );
                    }),
                MenuAction::new("Delete permanently", "trash")
                    .danger()
                    .separator()
                    .disabled(!owner || busy_now)
                    .run(move || {
                        open_modal(Modal {
                            kind: ModalKind::Delete,
                            entry: Some(e.get_value()),
                            entries: Vec::new(),
                        })
                    }),
            ];
        }
        let mut list = if entry.kind == "file" {
            vec![
                MenuAction::new("Preview", "eye").run(move || set_preview(Some(e.get_value()))),
                MenuAction::new("Download", "download")
                    .href(format!("/api/v1/files/{}/content", entry.id)),
            ]
        } else {
            vec![
                MenuAction::new("Open folder", "folder").run(move || open_entry(e.get_value())),
                MenuAction::new("Download as ZIP", "download")
                    .run(move || download_zip(vec![e.get_value().id])),
            ]
        };
        list.push(MenuAction::new("Details", "info").run(move || select(&e.get_value())));
        list.push(
            MenuAction::new(
                if entry.favorite {
                    "Remove favorite"
                } else {
                    "Add to favorites"
                },
                "star",
            )
            .separator()
            .disabled(!owner || busy_now)
            .run(move || {
                let entry = e.get_value();
                run(
                    Box::pin(async move {
                        let c = conn()?;
                        reducer(|tx| {
                            c.reducers.favorite_entry_then(
                                entry.id.clone(),
                                !entry.favorite,
                                entry.revision,
                                replied(tx),
                            )
                        })
                        .await
                    }),
                    "",
                );
            }),
        );
        list.push(
            MenuAction::new("Rename", "pencil")
                .shortcut("F2")
                .disabled(!write || busy_now)
                .run(move || {
                    open_modal(Modal {
                        kind: ModalKind::Rename,
                        entry: Some(e.get_value()),
                        entries: Vec::new(),
                    })
                }),
        );
        list.push(
            MenuAction::new("Move to…", "move")
                .disabled(!owner || busy_now)
                .run(move || {
                    open_modal(Modal {
                        kind: ModalKind::Move,
                        entry: Some(e.get_value()),
                        entries: Vec::new(),
                    })
                }),
        );
        list.push(MenuAction::new("Copy to…", "copy").run(move || {
            open_modal(Modal {
                kind: ModalKind::Copy,
                entry: Some(e.get_value()),
                entries: Vec::new(),
            })
        }));
        list.push(
            MenuAction::new("Copy", "copy")
                .separator()
                .shortcut(format!("{}C", modifier()))
                .run(move || collect(&e.get_value(), "copy")),
        );
        list.push(
            MenuAction::new("Cut", "scissors")
                .shortcut(format!("{}X", modifier()))
                .disabled(!owner || busy_now)
                .run(move || collect(&e.get_value(), "cut")),
        );
        if in_locked.get_untracked() {
            list.push(
                MenuAction::new("Delete permanently", "trash")
                    .danger()
                    .separator()
                    .disabled(!owner || busy_now)
                    .run(move || {
                        open_modal(Modal {
                            kind: ModalKind::Delete,
                            entry: Some(e.get_value()),
                            entries: Vec::new(),
                        })
                    }),
            );
        } else {
            list.push(
                MenuAction::new("Move to Locked folder", "lock")
                    .separator()
                    .disabled(!owner || busy_now)
                    .run(move || move_to_locked(vec![e.get_value()], None)),
            );
            list.push(
                MenuAction::new("Move to deleted files", "trash")
                    .danger()
                    .disabled(!owner || busy_now)
                    .run(move || request_trash(vec![e.get_value()])),
            );
        }
        list
    };

    // Keyboard shortcuts: F2 renames; Ctrl/Cmd C, X and V copy, cut and paste.
    listen(&document(), "keydown", false, move |event| {
        let e: web_sys::KeyboardEvent = event.unchecked_into();
        let in_field = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|t| {
                t.closest("input,textarea,select,[contenteditable=true],[role=combobox]")
                    .ok()
                    .flatten()
            })
            .is_some();
        let has_selection = window()
            .get_selection()
            .ok()
            .flatten()
            .and_then(|s| s.to_string().as_string())
            .is_some_and(|s| !s.is_empty());
        let dialog_open = document()
            .query_selector("dialog[open]")
            .ok()
            .flatten()
            .is_some();
        if e.default_prevented()
            || modal.with_untracked(Option::is_some)
            || settings.get_untracked()
            || dialog_open
            || in_field
            || has_selection
        {
            return;
        }
        let chosen = selected.get_untracked();
        let entry = rows.with_untracked(|r| r.iter().find(|row| chosen.contains(&row.id)).cloned());
        if e.key() == "F2" && chosen.len() == 1 {
            if let Some(entry) = entry.filter(|e| !e.trashed) {
                e.prevent_default();
                open_modal(Modal {
                    kind: ModalKind::Rename,
                    entry: Some(entry),
                    entries: Vec::new(),
                });
            }
        } else if e.ctrl_key() || e.meta_key() {
            let key = e.key().to_lowercase();
            if key == "v"
                && clipboard.with_untracked(Option::is_some)
                && writable_here.get_untracked()
            {
                e.prevent_default();
                paste();
            } else if (key == "c" || key == "x")
                && chosen.len() == 1
                && let Some(entry) =
                    entry.filter(|en| !en.trashed && (key == "c" || en.owner_id == me.get_value()))
            {
                e.prevent_default();
                collect(&entry, if key == "x" { "cut" } else { "copy" });
            }
        }
    });

    // The page's shared live connection; Files adds its own subscription to it.
    Effect::new(move |_| match live.status.get() {
        LiveStatus::Reconnecting => state.set(ConnectionState::Reconnecting),
        LiveStatus::Error => state.set(ConnectionState::Error),
        _ => {}
    });
    {
        let navigation_ready = navigation_ready.clone();
        Effect::new(move |_| {
            let Some(c) = live.connection.get() else {
                return;
            };
            let disposed = Arc::new(AtomicBool::new(false));
            let sync_files = {
                let (disposed, navigation_ready) = (disposed.clone(), navigation_ready.clone());
                move |ctx: &EventContext| {
                    if disposed.load(Ordering::Relaxed) || !navigation_ready.load(Ordering::Relaxed)
                    {
                        return;
                    }
                    let next = listed(ctx.db.my_files().iter());
                    if rows.try_with_untracked(|cur| *cur != next).unwrap_or(false) {
                        rows.try_set(next);
                    }
                }
            };
            let sync_usage = {
                let disposed = disposed.clone();
                move |ctx: &EventContext| {
                    if disposed.load(Ordering::Relaxed) {
                        return;
                    }
                    let next = ctx.db.my_usage().iter().next();
                    if usage
                        .try_with_untracked(|cur| *cur != next)
                        .unwrap_or(false)
                    {
                        usage.try_set(next);
                    }
                }
            };
            let ids = (
                c.db.my_files().on_insert({
                    let s = sync_files.clone();
                    move |ctx, _| s(ctx)
                }),
                c.db.my_files().on_delete({
                    let s = sync_files.clone();
                    move |ctx, _| s(ctx)
                }),
                c.db.my_usage().on_insert({
                    let s = sync_usage.clone();
                    move |ctx, _| s(ctx)
                }),
                c.db.my_usage().on_delete({
                    let s = sync_usage.clone();
                    move |ctx, _| s(ctx)
                }),
                c.db.me().on_delete({
                    let disposed = disposed.clone();
                    // Another app's unsubscribing also removes the row from the cache; only a real deletion signs out.
                    move |ctx: &EventContext, _| {
                        if !disposed.load(Ordering::Relaxed)
                            && !matches!(ctx.event, spacetimedb_sdk::Event::UnsubscribeApplied)
                        {
                            rows.try_set(Vec::new());
                            details.try_set(None);
                            {
                                on_signed_out.try_run(());
                            };
                        }
                    }
                }),
            );
            let handle = c
                .subscription_builder()
                .on_applied({
                    let (disposed, navigation_ready, c) =
                        (disposed.clone(), navigation_ready.clone(), c.clone());
                    move |ctx| {
                        if disposed.load(Ordering::Relaxed) {
                            return;
                        }
                        if navigation_ready.load(Ordering::Relaxed) {
                            rows.try_set(listed(ctx.db.my_files().iter()));
                        }
                        usage.try_set(ctx.db.my_usage().iter().next());
                        state.try_set(ConnectionState::Ready);
                        connection.try_set(Some(c.clone()));
                        epoch.try_update(|e| *e += 1);
                    }
                })
                .on_error({
                    let disposed = disposed.clone();
                    move |_, _| {
                        if !disposed.load(Ordering::Relaxed) {
                            state.try_set(ConnectionState::Error);
                        }
                    }
                })
                .subscribe([
                    "SELECT * FROM me",
                    "SELECT * FROM my_files",
                    "SELECT * FROM my_usage",
                ]);
            let kept = send_wrapper::SendWrapper::new((c.clone(), handle, ids));
            on_cleanup(move || {
                disposed.store(true, Ordering::Relaxed);
                let (c, handle, (a, b, d, e, g)) = kept.take();
                let _ = handle.unsubscribe();
                c.db.my_files().remove_on_insert(a);
                c.db.my_files().remove_on_delete(b);
                c.db.my_usage().remove_on_insert(d);
                c.db.my_usage().remove_on_delete(e);
                c.db.me().remove_on_delete(g);
            });
        });
    }

    // Browsing: the module fills `my_files` with the folder, section or search page.
    let browse_sequence = RwSignal::new(0u32);
    {
        let navigation_ready = navigation_ready.clone();
        Effect::new(move |_| {
            epoch.track();
            let Some(c) = connection.get() else { return };
            let (here, current_section, sub, q, at, open) = (
                folder.get(),
                section.get(),
                shared_sub.get(),
                query.get(),
                cursor.get(),
                locked_open.get(),
            );
            let id = browse_sequence.get_untracked() + 1;
            browse_sequence.set(id);
            navigation_ready.store(false, Ordering::Relaxed);
            loading.set(true);
            selected.set(HashSet::new());
            details.set(None);
            detail_sequence.update(|s| *s += 1);
            // A locked Locked folder shows the password form; nothing is browsed.
            if current_section == "locked" && !open {
                rows.set(Vec::new());
                loading.set(false);
                return;
            }
            let navigation_ready = navigation_ready.clone();
            spawn_local(async move {
                if !q.is_empty() {
                    crate::sleep(180).await;
                }
                if browse_sequence.try_get_untracked() != Some(id) {
                    return;
                }
                let browsed_section = if current_section == "locked" {
                    "files".to_owned()
                } else if current_section == "shared" {
                    match sub {
                        "by_you" => "shared_by_me".to_owned(),
                        "links" => "public_links".to_owned(),
                        _ => "shared".to_owned(),
                    }
                } else {
                    current_section.to_owned()
                };
                let result = reducer(|tx| {
                    c.reducers
                        .browse_then(here, browsed_section, q, at, replied(tx))
                })
                .await;
                if browse_sequence.try_get_untracked() != Some(id) {
                    return;
                }
                match result {
                    Ok(()) => {
                        navigation_ready.store(true, Ordering::Relaxed);
                        rows.set(listed(c.db.my_files().iter()));
                        loading.set(false);
                    }
                    Err(cause) => {
                        error.set(failure(&cause));
                        rows.set(Vec::new());
                        loading.set(false);
                        if cause.contains("lockedFolder") {
                            locked.refresh();
                        }
                    }
                }
            });
        });
    }
    listen(&window(), "popstate", false, move |_| {
        let (s, p) = read_location();
        section.set(s);
        parent_id.set(p);
        path.set(Vec::new());
        query.set(String::new());
        cursor.set(String::new());
        previous.set(Vec::new());
        selected.set(HashSet::new());
        details.set(None);
        detail_sequence.update(|s| *s += 1);
    });
    // The breadcrumb path of a folder opened by address.
    Effect::new(move |_| {
        let (Some(_), p) = (connection.get(), parent_id.get()) else {
            return;
        };
        if p.is_empty() || path.with_untracked(|path| path.last().is_some_and(|c| c.id == p)) {
            return;
        }
        spawn_local(async move {
            match file_details(&p).await {
                Ok(d) => {
                    if parent_id.try_get_untracked().as_ref() == Some(&p) {
                        let mut crumbs = d.ancestors;
                        crumbs.push(Crumb {
                            id: d.entry.id,
                            name: d.entry.name,
                        });
                        path.set(crumbs);
                    }
                }
                Err(e) => error.set(failure(&e)),
            }
        });
    });
    // `?file=` opens that file's details once connected.
    Effect::new(move |_| {
        if connection.with(Option::is_none) || state.get() != ConnectionState::Ready {
            return;
        }
        let Some(id) = initial_file.get_untracked() else {
            return;
        };
        initial_file.set(None);
        spawn_local(async move {
            match file_details(&id).await {
                Ok(d) => {
                    selected.set(HashSet::from([d.entry.id.clone()]));
                    tab.set("details".into());
                    details.set(Some(d));
                }
                Err(e) => error.set(failure(&e)),
            }
        });
    });
    // Refresh sidebar folder tree, counts, Git folder badges, and trash metadata.
    Effect::new(move |_| {
        if connection.with(Option::is_none) || state.get() != ConnectionState::Ready {
            return;
        }
        rows.track();
        usage.track();
        let seq = overview_seq.get_untracked() + 1;
        overview_seq.set(seq);
        spawn_local(async move {
            if let Ok(next) = fetch_overview().await
                && overview_seq.try_get_untracked() == Some(seq)
            {
                overview.try_set(next);
            }
        });
    });
    // Open details follow the row's revision.
    Effect::new(move |_| {
        let rows = rows.get();
        let Some(current) = details.get_untracked() else {
            return;
        };
        match rows.iter().find(|r| r.id == current.entry.id) {
            Some(row) if row.revision != current.entry.revision => show_details(row.id.clone()),
            None => show_details(current.entry.id.clone()),
            _ => {}
        }
    });
    on_cleanup(|| {
        let ids: Vec<String> = UPLOADS.with(|u| u.borrow().keys().cloned().collect());
        let reserved: Vec<String> = RESERVATIONS.with(|r| r.borrow().keys().cloned().collect());
        for id in ids.into_iter().chain(reserved) {
            cancel_upload(&id);
        }
    });

    // Each file goes to its own folder (a folder upload), else to the folder on screen.
    // The optional label is shown in the Uploads panel (for folder uploads, the path inside the uploaded folder).
    let upload_into = move |files: Vec<(web_sys::File, Option<String>, Option<String>)>,
                            replace: Option<FileEntry>| {
        let files = send_wrapper::SendWrapper::new(files);
        let csrf_token = csrf();
        spawn_local(async move {
            for (file, target, label) in files.take() {
                // Files closed: the remaining uploads do not start.
                let Some(current) = folder.try_get_untracked() else {
                    break;
                };
                let here = target.unwrap_or(current);
                let transfer = crate::random_uuid();
                let controller = web_sys::AbortController::new().expect("abort controller");
                RESERVATIONS.with(|r| r.borrow_mut().insert(transfer.clone(), controller.clone()));
                let display_name = label.unwrap_or_else(|| file.name());
                transfers.update(|t| {
                    t.push(Transfer {
                        id: transfer.clone(),
                        name: display_name,
                        progress: 0.0,
                        status: "uploading",
                        message: String::new(),
                    })
                });
                let update = move |id: &str, f: &dyn Fn(&mut Transfer)| {
                    transfers.try_update(|t| {
                        if let Some(t) = t.iter_mut().find(|t| t.id == id) {
                            f(t)
                        }
                    });
                };
                let mut body = serde_json::json!({ "parentId": replace.as_ref().map(|r| r.parent_id.clone()).unwrap_or(here), "name": file.name(), "size": file.size(), "mediaType": if file.type_().is_empty() { "application/octet-stream".to_owned() } else { file.type_() } });
                if let Some(r) = &replace {
                    body["fileId"] = r.id.clone().into();
                    body["expectedRevision"] = r.revision.into();
                }
                let reservation = api::request::<serde_json::Value>(
                    "/api/v1/files/uploads",
                    Req {
                        method: "POST",
                        body: Some(body.to_string()),
                        csrf: Some(&csrf_token),
                        signal: Some(controller.signal()),
                        ..Default::default()
                    },
                )
                .await;
                let mut upload_id = None;
                let result = match reservation {
                    Err(e) => Err(e.message),
                    Ok(_) if controller.signal().aborted() => Err("Upload cancelled.".to_owned()),
                    Ok(value) => {
                        let id = value["id"].as_str().unwrap_or_default().to_owned();
                        upload_id = Some(id.clone());
                        let progress_id = transfer.clone();
                        put_upload(
                            &transfer,
                            &format!("/api/v1/files/uploads/{id}"),
                            &csrf_token,
                            &file,
                            move |p| update(&progress_id, &|t| t.progress = p),
                        )
                        .await
                    }
                };
                match result {
                    Ok(()) => update(&transfer, &|t| {
                        t.progress = 1.0;
                        t.status = "complete";
                    }),
                    Err(cause) => {
                        let message = if controller.signal().aborted() {
                            "Upload cancelled.".to_owned()
                        } else {
                            failure(&cause)
                        };
                        let cancelled = message.contains("cancelled");
                        update(&transfer, &|t| {
                            t.status = if cancelled { "cancelled" } else { "error" };
                            t.message = message.clone();
                        });
                        if let Some(id) = upload_id {
                            let _ = api::request::<serde_json::Value>(
                                &format!("/api/v1/files/uploads/{id}"),
                                Req {
                                    method: "DELETE",
                                    csrf: Some(&csrf_token),
                                    ..Default::default()
                                },
                            )
                            .await;
                        }
                    }
                }
                UPLOADS.with(|u| u.borrow_mut().remove(&transfer));
                RESERVATIONS.with(|r| r.borrow_mut().remove(&transfer));
            }
        });
    };
    let upload = move |files: Vec<web_sys::File>, replace: Option<FileEntry>| {
        upload_into(
            files.into_iter().map(|f| (f, None, None)).collect(),
            replace,
        )
    };
    // A folder upload recreates the folder and its subfolders here, then uploads each file into
    // its folder. A top-level name already used here gets a number, as Files names are unique.
    let upload_folder = move |tree: folder_upload::Tree| {
        if tree.folders.is_empty() && tree.files.is_empty() {
            return;
        }
        let tree = send_wrapper::SendWrapper::new(tree);
        spawn_local(async move {
            let tree = tree.take();
            let Some(here) = folder.try_get_untracked() else {
                return;
            };
            let mut taken: std::collections::HashSet<String> =
                visible.with_untracked(|v| v.iter().map(|e| e.name.to_lowercase()).collect());
            let mut ids: HashMap<String, String> = HashMap::from([(String::new(), here.clone())]);
            let mut top_names: HashMap<String, String> = HashMap::new();
            for path in &tree.folders {
                let (parent, raw_name) = path
                    .rsplit_once('/')
                    .map_or(("", path.as_str()), |(p, n)| (p, n));
                let name = if parent.is_empty() {
                    let mut unique = raw_name.to_owned();
                    let mut n = 1;
                    while taken.contains(&unique.to_lowercase()) {
                        n += 1;
                        unique = format!("{raw_name} {n}");
                    }
                    taken.insert(unique.to_lowercase());
                    top_names.insert(raw_name.to_owned(), unique.clone());
                    unique
                } else {
                    raw_name.to_owned()
                };
                let Some(parent_id) = ids.get(parent).cloned() else {
                    continue;
                };
                let id = crate::random_uuid();
                let created = match connection.try_get_untracked().flatten() {
                    Some(c) => {
                        reducer(|tx| {
                            c.reducers.create_folder_then(
                                id.clone(),
                                parent_id,
                                name.clone(),
                                replied(tx),
                            )
                        })
                        .await
                    }
                    None => Err("Wait for the connection to return.".to_owned()),
                };
                if let Err(cause) = created {
                    error.try_set(format!(
                        "\u{201c}{name}\u{201d} could not be created: {}",
                        failure(&cause)
                    ));
                    return;
                }
                ids.insert(path.clone(), id);
            }
            let files = tree
                .files
                .into_iter()
                .map(|(file, dir)| {
                    let target = ids.get(&dir).cloned();
                    let label = folder_upload::display_path(&dir, &file.name(), &top_names);
                    (file, target, Some(label))
                })
                .collect();
            upload_into(files, None);
        });
    };
    let files_of = |input: &web_sys::HtmlInputElement| -> Vec<web_sys::File> {
        input
            .files()
            .map(|list| (0..list.length()).filter_map(|i| list.item(i)).collect())
            .unwrap_or_default()
    };

    let submit_modal = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let Some(m) = modal.get_untracked() else {
            return;
        };
        if matches!(m.kind, ModalKind::Move | ModalKind::Copy)
            && target_folder.with_untracked(Option::is_none)
        {
            return;
        }
        let name = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok())
            .and_then(|f| web_sys::FormData::new_with_form(&f).ok())
            .and_then(|d| d.get("name").as_string())
            .unwrap_or_default();
        let target = target_folder
            .get_untracked()
            .map(|t| t.id)
            .unwrap_or_default();
        let many = m.entries.clone();
        let message = if m.entry.is_none()
            && !many.is_empty()
            && matches!(m.kind, ModalKind::Move | ModalKind::Copy)
        {
            format!(
                "{} {} {}.",
                many.len(),
                if many.len() == 1 { "item" } else { "items" },
                if m.kind == ModalKind::Move {
                    "moved"
                } else {
                    "copied"
                }
            )
        } else {
            "Changes saved.".into()
        };
        let here = folder.get_untracked();
        let chosen = visible.with_untracked(|v| {
            v.iter()
                .filter(|e| selected.with_untracked(|s| s.contains(&e.id)))
                .cloned()
                .collect::<Vec<_>>()
        });
        spawn_local(async move {
            let ok = perform(
                Box::pin(async move {
                    let c = connection
                        .get_untracked()
                        .ok_or_else(|| "Wait for the connection to return.".to_owned())?;
                    match (m.kind, &m.entry) {
                        (ModalKind::Folder, _) => {
                            reducer(|tx| {
                                c.reducers.create_folder_then(
                                    crate::random_uuid(),
                                    here.clone(),
                                    name.clone(),
                                    replied(tx),
                                )
                            })
                            .await?
                        }
                        (ModalKind::Rename, Some(e)) => {
                            reducer(|tx| {
                                c.reducers.rename_entry_then(
                                    e.id.clone(),
                                    name.clone(),
                                    e.revision,
                                    replied(tx),
                                )
                            })
                            .await?
                        }
                        (ModalKind::Move, Some(e)) => {
                            reducer(|tx| {
                                c.reducers.move_entry_then(
                                    e.id.clone(),
                                    target.clone(),
                                    e.revision,
                                    None,
                                    replied(tx),
                                )
                            })
                            .await?
                        }
                        (ModalKind::Copy, Some(e)) => {
                            reducer(|tx| {
                                c.reducers.copy_entry_then(
                                    e.id.clone(),
                                    crate::random_uuid(),
                                    target.clone(),
                                    name.clone(),
                                    e.revision,
                                    replied(tx),
                                )
                            })
                            .await?
                        }
                        // Several selected items keep their names; copies into the same folder are marked as copies.
                        (ModalKind::Move, None) => {
                            for item in &many {
                                reducer(|tx| {
                                    c.reducers.move_entry_then(
                                        item.id.clone(),
                                        target.clone(),
                                        item.revision,
                                        None,
                                        replied(tx),
                                    )
                                })
                                .await?;
                            }
                            selected.set(HashSet::new());
                            details.set(None);
                        }
                        (ModalKind::Copy, None) => {
                            for item in &many {
                                let name = if item.parent_id == target {
                                    format!("{} copy", item.name)
                                } else {
                                    item.name.clone()
                                };
                                reducer(|tx| {
                                    c.reducers.copy_entry_then(
                                        item.id.clone(),
                                        crate::random_uuid(),
                                        target.clone(),
                                        name,
                                        item.revision,
                                        replied(tx),
                                    )
                                })
                                .await?;
                            }
                            selected.set(HashSet::new());
                        }
                        (ModalKind::Delete, entry) => {
                            let targets = match entry {
                                Some(e) => vec![e.clone()],
                                None if !many.is_empty() => many.clone(),
                                None => chosen,
                            };
                            for item in targets {
                                reducer(|tx| {
                                    c.reducers.delete_entry_then(
                                        item.id.clone(),
                                        item.revision,
                                        replied(tx),
                                    )
                                })
                                .await?;
                            }
                            selected.set(HashSet::new());
                            details.set(None);
                        }
                        _ => {}
                    }
                    Ok(())
                }),
                message,
            )
            .await;
            if ok {
                modal.set(None);
            }
        });
    };
    let share = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let Some(d) = details.get_untracked() else {
            return;
        };
        let data = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok())
            .and_then(|f| web_sys::FormData::new_with_form(&f).ok());
        let field = |name: &str| {
            data.as_ref()
                .and_then(|d| d.get(name).as_string())
                .unwrap_or_default()
        };
        let (username, role) = (field("username"), field("role"));
        let entry = d.entry;
        spawn_local(async move {
            let id = entry.id.clone();
            let ok = perform(
                Box::pin(async move {
                    let c = conn()?;
                    reducer(|tx| {
                        c.reducers.grant_access_then(
                            crate::random_uuid(),
                            entry.id.clone(),
                            username,
                            role,
                            entry.revision,
                            replied(tx),
                        )
                    })
                    .await
                }),
                "Sharing updated.".into(),
            )
            .await;
            if ok {
                show_details(id);
            }
        });
    };
    let make_public_link = move || {
        let Some(d) = details.get_untracked() else {
            return;
        };
        let days = public_expiry_days
            .get_untracked()
            .parse::<f64>()
            .ok()
            .filter(|v| *v >= 1.0 && *v <= 30.0)
            .unwrap_or(7.0);
        spawn_local(async move {
            let entry = d.entry;
            let id = entry.id.clone();
            perform(
                Box::pin(async move {
                    let c = conn()?;
                    let (token, digest) = crate::secret_and_digest().await?;
                    let expires = js_sys::Date::now() + days * 86_400_000.0;
                    reducer(|tx| {
                        c.reducers.create_public_share_then(
                            crate::random_uuid(),
                            entry.id.clone(),
                            digest,
                            expires,
                            entry.revision,
                            replied(tx),
                        )
                    })
                    .await?;
                    public_link.set(format!(
                        "{}/public/files/{token}",
                        window().location().origin().unwrap_or_default()
                    ));
                    Ok(())
                }),
                String::new(),
            )
            .await;
            show_details(id);
        });
    };

    let title = move || {
        if !parent_id.with(String::is_empty) {
            path.with(|p| p.last().map(|c| c.name.clone()))
                .unwrap_or_else(|| "Folder".into())
        } else {
            SECTIONS
                .iter()
                .find(|s| s.0 == section.get())
                .map(|s| s.1)
                .unwrap_or("All files")
                .to_owned()
        }
    };
    let chosen = Memo::new(move |_| details.with(|d| d.as_ref().map(|d| d.entry.clone())));
    let owner = Memo::new(move |_| details.with(|d| d.as_ref().is_some_and(|d| d.role == "owner")));
    let can_write = Memo::new(move |_| {
        owner.get() || details.with(|d| d.as_ref().is_some_and(|d| d.role == "write"))
    });
    // Folders show their history where files show versions; a remembered tab maps across.
    let shown_tab = Memo::new(move |_| {
        let t = tab.get();
        let folder = chosen.with(|c| c.as_ref().is_some_and(|c| c.kind == "folder"));
        if folder && t == "versions" {
            "history".to_owned()
        } else if !folder && t == "history" {
            "versions".to_owned()
        } else {
            t
        }
    });

    let selection_actions = move || {
        let chosen_items: Vec<FileEntry> = visible.with(|v| {
            v.iter()
                .filter(|e| selected.with(|s| s.contains(&e.id)))
                .cloned()
                .collect()
        });
        let owned = chosen_items.iter().all(|e| e.owner_id == me.get_value());
        let off = !connected.get() || busy.get() || chosen_items.is_empty();
        let count = format!(
            "{} {}",
            chosen_items.len(),
            if chosen_items.len() == 1 {
                "item"
            } else {
                "items"
            }
        );
        let items = StoredValue::new(chosen_items.clone());
        if section.get() == "trash" {
            let count2 = count.clone();
            return view! {
                <button class="text-button" aria-label="Restore selected items" disabled=off || !owned on:click=move |_| {
                    let list = items.get_value();
                    run(Box::pin(async move { let c = conn()?; for item in list { reducer(|tx| c.reducers.restore_entry_then(item.id.clone(), item.revision, replied(tx))).await?; } selected.set(HashSet::new()); details.set(None); Ok(()) }), &format!("{count2} restored."));
                }><Icon name="history"/><span>"Restore"</span></button>
                <button class="text-button danger" aria-label="Delete selected items permanently" disabled=off || !owned on:click=move |_| open_modal(Modal { kind: ModalKind::Delete, entry: None, entries: items.get_value() })><Icon name="trash"/><span>"Delete permanently"</span></button>
            }.into_any();
        }
        let favorite = !chosen_items.iter().all(|e| e.favorite);
        let single_file = (chosen_items.len() == 1 && chosen_items[0].kind == "file")
            .then(|| chosen_items[0].clone());
        let locked_here = in_locked.get();
        view! {
            {if let Some(single) = single_file {
                let id = single.id.clone();
                let name = single.name.clone();
                view! { <a class="text-button" href=format!("/api/v1/files/{id}/content") download=name aria-label="Download selected file" title="Download"><Icon name="download"/><span>"Download"</span></a> }.into_any()
            } else {
                view! { <button class="text-button" aria-label="Download selected items as ZIP" title="Download ZIP" disabled=off on:click=move |_| download_zip(items.get_value().into_iter().map(|i| i.id).collect())><Icon name="download"/><span>"Download ZIP"</span></button> }.into_any()
            }}
            <button class="text-button" aria-label="Move selected items" title="Move to…" disabled=off || !owned on:click=move |_| open_modal(Modal { kind: ModalKind::Move, entry: None, entries: items.get_value() })><Icon name="move"/><span>"Move to…"</span></button>
            <button class="text-button" aria-label="Copy selected items" title="Copy to…" disabled=off on:click=move |_| open_modal(Modal { kind: ModalKind::Copy, entry: None, entries: items.get_value() })><Icon name="copy"/><span>"Copy to…"</span></button>
            {if locked_here {
                view! { <button class="text-button danger" aria-label="Delete selected items permanently" disabled=off || !owned on:click=move |_| open_modal(Modal { kind: ModalKind::Delete, entry: None, entries: items.get_value() })><Icon name="trash"/><span>"Delete permanently"</span></button> }.into_any()
            } else {
                view! { <button class="text-button" aria-label="Move selected items to deleted files" title="Move to deleted files" disabled=off || !owned on:click=move |_| request_trash(items.get_value())><Icon name="trash"/><span>"Move to deleted files"</span></button> }.into_any()
            }}
            <ActionMenu label="More actions for selected items" class="file-selection-more" actions=move |_| {
                let count = count.clone();
                let mut list = vec![MenuAction::new(if favorite { "Add to favorites" } else { "Remove from favorites" }, "star").disabled(off || !owned).run(move || {
                    let (list, count) = (items.get_value(), count.clone());
                    run(Box::pin(async move { let c = conn()?; for item in list { if item.favorite != favorite { reducer(|tx| c.reducers.favorite_entry_then(item.id.clone(), favorite, item.revision, replied(tx))).await?; } } Ok(()) }),
                        &if favorite { format!("{count} added to favorites.") } else { format!("{count} removed from favorites.") });
                })];
                if !locked_here { list.push(MenuAction::new("Move to Locked folder", "lock").disabled(off || !owned).run(move || move_to_locked(items.get_value(), None))); }
                list
            }><Icon name="more"/></ActionMenu>
        }.into_any()
    };

    let filtered_rows = Memo::new(move |_| {
        let mut list: Vec<FileEntry> = visible.with(|v| {
            let f = type_filter.get();
            v.iter()
                .filter(|e| matches_type_filter(e, f))
                .cloned()
                .collect()
        });
        let folders_first = section.get() != "recent";
        sort_entries(&mut list, sort_field.get(), sort_asc.get(), folders_first);
        list
    });
    let separate_folders = Memo::new(move |_| {
        matches!(section.get(), "files" | "locked" | "favorites")
            && type_filter.get() == "all"
            && filtered_rows.with(|r| r.iter().any(|e| e.kind == "folder"))
    });
    let shelf_folders = Memo::new(move |_| {
        if separate_folders.get() {
            filtered_rows.with(|r| {
                r.iter()
                    .filter(|e| e.kind == "folder")
                    .cloned()
                    .collect::<Vec<_>>()
            })
        } else {
            Vec::new()
        }
    });
    let file_rows = Memo::new(move |_| {
        if separate_folders.get() {
            filtered_rows.with(|r| {
                r.iter()
                    .filter(|e| e.kind != "folder")
                    .cloned()
                    .collect::<Vec<_>>()
            })
        } else {
            filtered_rows.get()
        }
    });
    let filter_counts = Memo::new(move |_| {
        visible.with(|v| {
            let (mut all, mut doc, mut image, mut code, mut archive, mut media) =
                (0usize, 0, 0, 0, 0, 0);
            for e in v {
                if e.kind == "folder" {
                    continue;
                }
                all += 1;
                match file_type(&e.kind, &e.name, &e.media_type) {
                    "doc" | "pdf" | "sheet" => doc += 1,
                    "image" => image += 1,
                    "code" => code += 1,
                    "archive" => archive += 1,
                    "audio" | "video" => media += 1,
                    _ => {}
                }
            }
            (all, doc, image, code, archive, media)
        })
    });
    let toggle_sort = move |field: &'static str| {
        if sort_field.get_untracked() == field {
            sort_asc.update(|a| *a = !*a);
        } else {
            sort_field.set(field);
            sort_asc.set(matches!(field, "name" | "type"));
        }
    };
    let toggle_favorite = move |entry: FileEntry| {
        if entry.owner_id != me.get_value() || busy.get_untracked() {
            return;
        }
        run(
            Box::pin(async move {
                let c = conn()?;
                reducer(|tx| {
                    c.reducers.favorite_entry_then(
                        entry.id.clone(),
                        !entry.favorite,
                        entry.revision,
                        replied(tx),
                    )
                })
                .await
            }),
            "",
        );
    };
    let open_share = move |entry: FileEntry| {
        tab.set("sharing".into());
        selected.set(HashSet::from([entry.id.clone()]));
        public_link.set(String::new());
        show_details(entry.id);
    };

    let empty_state = move || {
        let s = section.get();
        let q = !query.with(String::is_empty);
        let filtered = type_filter.get() != "all";
        let locked_here = in_locked.get();
        let icon = if s == "trash" {
            "trash"
        } else if s == "shared" {
            "share"
        } else if s == "recent" {
            "clock"
        } else if q || filtered {
            "search"
        } else if locked_here {
            "lock"
        } else {
            "folder"
        };
        let heading = if filtered {
            "No matching files"
        } else if q {
            "No matching files"
        } else if locked_here {
            "Nothing locked yet"
        } else if s == "trash" {
            "Nothing in deleted files"
        } else if s == "recent" {
            "No recent files"
        } else if s == "shared" {
            match shared_sub.get() {
                "by_you" => "Nothing shared by you",
                "links" => "No active public links",
                _ => "No shared files",
            }
        } else if s == "favorites" {
            "No favorites"
        } else {
            "No files"
        };
        let text = if filtered {
            "No files in this view match the selected type filter."
        } else if q {
            "Search matches the beginning of a filename in this folder."
        } else if locked_here {
            "Choose Move to Locked folder on a file or folder, or use New to upload here. Deleting here is permanent."
        } else if s == "files" {
            "Drop files or folders here, or use New to upload or create something."
        } else if s == "recent" {
            "Files you upload or edit will appear here, newest first."
        } else if s == "favorites" {
            "Star any file or folder to keep it one click away."
        } else if s == "shared" {
            match shared_sub.get() {
                "by_you" => "Files and folders you share with other people will appear here.",
                "links" => "Public download links you create will appear here so you can manage or revoke them.",
                _ => "When someone shares with you, it will appear here.",
            }
        } else {
            "Items you delete will appear here for recovery."
        };
        view! {
            <div class="empty-state"><span class="empty-icon"><Icon name=icon/></span><h2>{heading}</h2><p>{text}</p>
                {filtered.then(|| view! { <button class="secondary-button" on:click=move |_| type_filter.set("all")>"Show all types"</button> })}
                {(writable_here.get() && !q && !filtered).then(|| view! {
                    <div class="empty-state-actions">
                        <button class="primary-button" disabled=move || !connected.get() on:click=move |_| if let Some(i) = upload_input.get_untracked() { i.click() }><Icon name="upload"/>"Upload your first file"</button>
                        <button class="secondary-button" disabled=move || !connected.get() on:click=move |_| open_modal(Modal { kind: ModalKind::Folder, entry: None, entries: Vec::new() })><Icon name="folder"/>"New folder"</button>
                    </div>
                })}
            </div>
        }
    };
    let folder_tile_view = move |entry: FileEntry| {
        let id = entry.id.clone();
        let (sel_id, check_id, toggle_id, drag_id, drop_id, drop_id2, drop_id3, count_id, git_id) = (
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
        );
        let shared_with_me = entry.owner_id != me.get_value();
        let owned = !shared_with_me;
        let can_drag = writable_here.get_untracked() && owned && !entry.trashed;
        let e = StoredValue::new(entry.clone());
        let item_count = move || overview.with(|o| o.folder_counts.get(&count_id).copied().unwrap_or(0));
        let git_tracked = move || overview.with(|o| o.tracked_folder_ids.iter().any(|x| x == &git_id));
        view! {
            <div class=move || {
                let is_sel = selected.with(|s| s.contains(&sel_id))
                    || chosen.with(|c| c.as_ref().is_some_and(|c| c.id == sel_id));
                let is_drop = drop_target.with(|d| d.as_deref() == Some(drop_id.as_str()));
                match (is_sel, is_drop) {
                    (true, true) => "folder-tile is-selected is-drop-target",
                    (true, false) => "folder-tile is-selected",
                    (false, true) => "folder-tile is-drop-target",
                    (false, false) => "folder-tile",
                }
            }
                draggable=if can_drag { "true" } else { "false" }
                on:dragstart=move |ev: web_sys::DragEvent| {
                    if can_drag {
                        dragging_id.set(Some(drag_id.clone()));
                        if let Some(dt) = ev.data_transfer() {
                            let _ = dt.set_data("application/x-dar-file-id", &drag_id);
                        }
                    }
                }
                on:dragend=move |_| { dragging_id.set(None); drop_target.set(None); }
                on:dragover=move |ev: web_sys::DragEvent| {
                    if dragging_id.with_untracked(|d| d.as_deref().is_some_and(|src| src != drop_id2.as_str())) {
                        ev.prevent_default();
                        ev.stop_propagation();
                        drop_target.set(Some(drop_id2.clone()));
                    }
                }
                on:dragleave=move |_| {
                    if drop_target.with_untracked(|d| d.as_deref() == Some(drop_id3.as_str())) {
                        drop_target.set(None);
                    }
                }
                on:drop=move |ev: web_sys::DragEvent| {
                    if dragging_id.with_untracked(Option::is_some) {
                        ev.prevent_default();
                        ev.stop_propagation();
                        move_dragged_into(e.get_value().id);
                    }
                }>
                <input type="checkbox" class="folder-tile-check" prop:checked=move || selected.with(|s| s.contains(&check_id)) aria-label=format!("Select {}", entry.name) on:change=move |_| toggle(toggle_id.clone())/>
                <FileTile kind=entry.kind.clone() name=entry.name.clone() media_type=entry.media_type.clone()/>
                <div class="folder-tile-body">
                    <button class="folder-tile-open" on:click=move |_| open_entry(e.get_value())>{entry.name.clone()}</button>
                    <div class="folder-tile-meta">
                        <span>{move || { let n = item_count(); format!("{n} {}", if n == 1 { "item" } else { "items" }) }}</span>
                        {move || git_tracked().then(|| view! { <span class="folder-git-badge" title="Git history tracked"><Icon name="branch"/>"Git"</span> })}
                    </div>
                </div>
                {entry.favorite.then(|| view! { <Icon name="star" class="favorite-icon"/> })}
                {shared_with_me.then(|| view! { <span class="file-shared" title="Shared with you"><Icon name="share"/><span class="sr-only">"Shared with you"</span></span> })}
                <ActionMenu label=format!("Actions for {}", entry.name) class="ui-file-actions" on_open=Callback::new(move |_| open_menu(e.get_value())) actions=move |_| menu_items(e.get_value())><Icon name="more"/></ActionMenu>
            </div>
        }
    };
    let row_view = move |entry: FileEntry| {
        let id = entry.id.clone();
        let (toggle_id, highlight_id, check_id, drag_id, origin_id, shared_id) = (
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
        );
        let shared_with_me = entry.owner_id != me.get_value();
        let owned = !shared_with_me;
        let is_file = entry.kind == "file";
        let is_trashed = entry.trashed;
        let can_drag = writable_here.get_untracked() && owned && !is_trashed;
        let e = StoredValue::new(entry.clone());
        let size_text = if is_file {
            format_bytes(entry.size)
        } else {
            "—".to_owned()
        };
        let modified_text = relative(entry.modified_at);
        let phone_sub = if is_file {
            format!("{size_text} · {modified_text}")
        } else {
            format!("Folder · {modified_text}")
        };
        let owned_shared = move || !shared_with_me && overview.with(|o| o.shared_ids.iter().any(|x| x == &shared_id));
        let trash_origin = move || {
            is_trashed.then(|| {
                overview.with(|o| {
                    o.trash_parents
                        .get(&origin_id)
                        .cloned()
                        .unwrap_or_else(|| "All files".to_owned())
                })
            })
        };
        view! {
            <tr class=move || if selected.with(|s| s.contains(&highlight_id)) || chosen.with(|c| c.as_ref().is_some_and(|c| c.id == highlight_id)) { "is-selected" } else { "" }
                draggable=if can_drag { "true" } else { "false" }
                on:dragstart=move |ev: web_sys::DragEvent| {
                    if can_drag {
                        dragging_id.set(Some(drag_id.clone()));
                        if let Some(dt) = ev.data_transfer() {
                            let _ = dt.set_data("application/x-dar-file-id", &drag_id);
                        }
                    }
                }
                on:dragend=move |_| { dragging_id.set(None); drop_target.set(None); }>
                <td class="checkbox-cell"><input type="checkbox" prop:checked=move || selected.with(|s| s.contains(&check_id)) aria-label=format!("Select {}", entry.name) on:change=move |_| toggle(toggle_id.clone())/></td>
                <td><div class="file-name">
                    <FileTile kind=entry.kind.clone() name=entry.name.clone() media_type=entry.media_type.clone()/>
                    <div class="file-name-wrap">
                        <div class="file-name-line">
                            <button class="name-button" on:click=move |_| { let en = e.get_value(); if !en.trashed { open_entry(en) } else { select(&en) } } on:dblclick=move |_| { let en = e.get_value(); if en.kind == "file" && !en.trashed { set_preview(Some(en)) } }>{entry.name.clone()}</button>
                            {entry.favorite.then(|| view! { <Icon name="star" class="favorite-icon"/> })}
                            {shared_with_me.then(|| view! { <span class="file-shared" title="Shared with you"><Icon name="share"/><span class="sr-only">"Shared with you"</span></span> })}
                            {move || owned_shared().then(|| view! { <span class="file-shared" title="Shared"><Icon name="share"/></span> })}
                        </div>
                        {move || trash_origin().map(|orig| view! { <span class="file-name-origin">{format!("From {orig}")}</span> })}
                        <span class="file-name-sub">{phone_sub.clone()}</span>
                    </div>
                </div></td>
                <td class="date-cell muted">{modified_text}</td>
                <td class="size-cell muted">{size_text}</td>
                <td>
                    <div class="file-row-actions">
                        {if is_trashed {
                            owned.then(|| view! {
                                <button class="file-restore-pill" aria-label=format!("Restore {}", entry.name) disabled=move || busy.get() on:click=move |_| {
                                    let en = e.get_value();
                                    run(Box::pin(async move { let c = conn()?; reducer(|tx| c.reducers.restore_entry_then(en.id.clone(), en.revision, replied(tx))).await }), "Item restored.");
                                }>"Restore"</button>
                            }).into_any()
                        } else {
                            view! {
                                <div class="file-row-quick">
                                    {owned.then(|| view! {
                                        <button class=if entry.favorite { "icon-button is-starred" } else { "icon-button" }
                                            aria-label=format!("{} {}", if entry.favorite { "Unfavorite" } else { "Favorite" }, entry.name)
                                            title=if entry.favorite { "Remove favorite" } else { "Add to favorites" }
                                            on:click=move |_| toggle_favorite(e.get_value())><Icon name="star"/></button>
                                    })}
                                    {(owned && !in_locked.get_untracked()).then(|| view! {
                                        <button class="icon-button" aria-label=format!("Share {}", entry.name) title="Share" on:click=move |_| open_share(e.get_value())><Icon name="share"/></button>
                                    })}
                                    {is_file.then(|| view! {
                                        <a class="icon-button" href=format!("/api/v1/files/{}/content", entry.id) download=entry.name.clone() aria-label=format!("Download {}", entry.name) title="Download"><Icon name="download"/></a>
                                    })}
                                </div>
                            }.into_any()
                        }}
                        <ActionMenu label=format!("Actions for {}", entry.name) class="ui-file-actions" on_open=Callback::new(move |_| open_menu(e.get_value())) actions=move |_| menu_items(e.get_value())><Icon name="more"/></ActionMenu>
                    </div>
                </td>
            </tr>
        }
    };
    let card_view = move |entry: FileEntry| {
        let id = entry.id.clone();
        let (class_id, check_id, toggle_id) = (id.clone(), id.clone(), id.clone());
        let owned = entry.owner_id == me.get_value();
        let e = StoredValue::new(entry.clone());
        let art = if thumbnail_ok(&entry) {
            view! { <img src=format!("/api/v1/files/{}/thumbnail?v={}", entry.id, entry.revision) alt="" loading="lazy" decoding="async"/> }.into_any()
        } else {
            view! { <FileTile kind=entry.kind.clone() name=entry.name.clone() media_type=entry.media_type.clone() large=true/> }.into_any()
        };
        view! {
            <article class=move || if selected.with(|s| s.contains(&class_id)) { "file-card is-selected" } else { "file-card" }>
                <div class="card-media-wrap">
                    <button class=format!("card-open card-preview-{}", entry.kind) on:click=move |_| open_entry(e.get_value()) aria-label=format!("Open {}", entry.name)>{art}</button>
                    <div class="card-overlay">
                        <label class="card-ov-chip"><input type="checkbox" prop:checked=move || selected.with(|s| s.contains(&check_id)) aria-label=format!("Select {}", entry.name) on:change=move |_| toggle(toggle_id.clone())/></label>
                        {(owned && !entry.trashed).then(|| view! {
                            <button class=if entry.favorite { "card-ov-chip is-starred" } else { "card-ov-chip" }
                                aria-label=format!("{} {}", if entry.favorite { "Unfavorite" } else { "Favorite" }, entry.name)
                                title=if entry.favorite { "Remove favorite" } else { "Add to favorites" }
                                on:click=move |_| toggle_favorite(e.get_value())><Icon name="star"/></button>
                        })}
                    </div>
                </div>
                <div class="card-caption">
                    <button class="name-button" on:click=move |_| open_entry(e.get_value())>{entry.name.clone()}</button>
                    <ActionMenu label=format!("Actions for {}", entry.name) class="ui-file-actions" on_open=Callback::new(move |_| open_menu(e.get_value())) actions=move |_| menu_items(e.get_value())><Icon name="more"/></ActionMenu>
                </div>
                <p>{if entry.kind == "folder" { "Folder".to_owned() } else { format_bytes(entry.size) }}" · "{short_date(entry.modified_at)}{entry.favorite.then(|| view! { <Icon name="star" class="favorite-icon"/> })}</p>
            </article>
        }
    };
    let row_key = |e: &FileEntry| {
        format!(
            "{}:{}:{}:{}:{}:{}",
            e.id, e.revision, e.favorite, e.trashed, e.name, e.modified_at
        )
    };

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ListingMode {
        LockedForm,
        LockedChecking,
        Disconnected,
        Loading,
        Empty,
        Grid,
        Table,
    }
    let listing_mode = Memo::new(move |_| {
        if in_locked.get() && !locked_open.get() {
            if locked.state.with(Option::is_some) {
                ListingMode::LockedForm
            } else {
                ListingMode::LockedChecking
            }
        } else if state.get() != ConnectionState::Ready && connection.with(Option::is_none) {
            ListingMode::Disconnected
        } else if loading.get() {
            ListingMode::Loading
        } else if filtered_rows.with(Vec::is_empty) {
            ListingMode::Empty
        } else if grid.get() {
            ListingMode::Grid
        } else {
            ListingMode::Table
        }
    });

    let folder_shelf_section = move || {
        let folders = shelf_folders.get();
        (!folders.is_empty()).then(|| {
            let count = folders.len();
            view! {
                <div class="files-section-heading">"Folders "<span>{format!("({count})")}</span></div>
                <div class="folder-shelf"><For each=move || shelf_folders.get() key=row_key children=folder_tile_view/></div>
            }
        })
    };

    let listing = move || {
        match listing_mode.get() {
        ListingMode::LockedForm => view! { <UnlockForm folder=locked description="Enter your password to open it. Files here are hidden from All files, search, sharing, WebDAV and other apps, including Photos’ library." on_unlocked=Callback::new(move |_| error.set(String::new()))/> }.into_any(),
        ListingMode::LockedChecking => view! { <p class="muted" role="status">{move || { let e = locked.error.get(); if e.is_empty() { "Checking the Locked folder…".to_owned() } else { e } }}</p> }.into_any(),
        ListingMode::Disconnected => ().into_any(),
        ListingMode::Loading => view! { <div class="file-skeleton" aria-busy="true" aria-label="Loading files">{(0..4).map(|_| view! { <div><span></span><span></span></div> }).collect_view()}</div> }.into_any(),
        ListingMode::Empty => empty_state().into_any(),
        ListingMode::Grid => view! {
            {folder_shelf_section}
            {move || (!file_rows.with(Vec::is_empty)).then(|| view! {
                {move || separate_folders.get().then(|| view! { <div class="files-section-heading">"Files "<span>{format!("({})", file_rows.with(Vec::len))}</span></div> })}
                <div class="file-grid"><For each=move || file_rows.get() key=row_key children=card_view/></div>
            })}
        }.into_any(),
        ListingMode::Table => view! {
            {folder_shelf_section}
            {move || (!file_rows.with(Vec::is_empty)).then(|| {
                let arrow = move |f: &'static str| {
                    if sort_field.get() == f {
                        if sort_asc.get() { " ↑" } else { " ↓" }
                    } else {
                        ""
                    }
                };
                view! {
                    {move || separate_folders.get().then(|| view! { <div class="files-section-heading">"Files "<span>{format!("({})", file_rows.with(Vec::len))}</span></div> })}
                    <div class="table-scroll"><table class=move || if selected.with(HashSet::is_empty) { "file-table" } else { "file-table is-selecting-mobile" }>
                        <thead><tr>
                            <th class="checkbox-cell"><input type="checkbox" aria-label="Select all visible files" prop:checked=move || filtered_rows.with(|v| !v.is_empty() && v.iter().all(|e| selected.with(|s| s.contains(&e.id))))
                                on:change=move |e| selected.set(if event_target_checked(&e) { filtered_rows.with_untracked(|v| v.iter().map(|e| e.id.clone()).collect()) } else { HashSet::new() })/></th>
                            <th><button type="button" class=move || if sort_field.get() == "name" { "th-sort is-sorted" } else { "th-sort" } on:click=move |_| toggle_sort("name")>"Name"{move || arrow("name")}</button></th>
                            <th class="date-cell"><button type="button" class=move || if sort_field.get() == "modified" { "th-sort is-sorted" } else { "th-sort" } on:click=move |_| toggle_sort("modified")>{move || if section.get() == "trash" { "Deleted" } else { "Modified" }}{move || arrow("modified")}</button></th>
                            <th class="size-cell"><button type="button" class=move || if sort_field.get() == "size" { "th-sort is-sorted" } else { "th-sort" } on:click=move |_| toggle_sort("size")>"Size"{move || arrow("size")}</button></th>
                            <th><span class="sr-only">"Actions"</span></th>
                        </tr></thead>
                        <tbody><For each=move || file_rows.get() key=row_key children=row_view/></tbody>
                    </table></div>
                }
            })}
        }.into_any(),
    }
    };

    // The panel is rebuilt for another item only. A newer revision of the open one (a change in
    // the folder, a restore) refreshes its parts in place, so the History tab keeps its panes,
    // notices and the replies still in flight.
    let details_key = Memo::new(move |_| {
        details.with(|d| {
            d.as_ref().map(|d| {
                (
                    d.entry.id.clone(),
                    d.entry.kind == "folder",
                    d.entry.trashed,
                )
            })
        })
    });
    let details_panel = move || {
        let (entry_id, is_folder, trashed) = details_key.get()?;
        let overview = move || {
            let entry = details.get()?.entry;
            let e = StoredValue::new(entry.clone());
            let preview_art = if thumbnail_ok(&entry) {
                view! { <img src=format!("/api/v1/files/{}/thumbnail?size=m&v={}", entry.id, entry.revision) alt="" decoding="async"/> }.into_any()
            } else {
                view! { <FileTile kind=entry.kind.clone() name=entry.name.clone() media_type=entry.media_type.clone() large=true/> }.into_any()
            };
            let summary = view! { {file_type_label(&entry.kind, &entry.name, &entry.media_type)}{if is_folder { String::new() } else { format!(" · {}", format_bytes(entry.size)) }}" · "{relative(entry.modified_at)} };
            let quick = if entry.trashed {
                owner.get().then(|| view! { <button class="quick-action" disabled=move || busy.get() on:click=move |_| { let en = e.get_value(); run(Box::pin(async move { let c = conn()?; reducer(|tx| c.reducers.restore_entry_then(en.id.clone(), en.revision, replied(tx))).await }), "Item restored."); }><span><Icon name="history"/></span>"Restore"</button> }).into_any()
            } else {
                view! {
                    {if is_folder {
                        view! { <button class="quick-action" on:click=move |_| open_entry(e.get_value())><span><Icon name="folder"/></span>"Open"</button> }.into_any()
                    } else {
                        view! { <button class="quick-action" on:click=move |_| set_preview(Some(e.get_value()))><span><Icon name="eye"/></span>"Preview"</button><a class="quick-action" href=format!("/api/v1/files/{}/content", entry.id)><span><Icon name="download"/></span>"Download"</a> }.into_any()
                    }}
                    {owner.get().then(|| view! { <button class="quick-action" aria-pressed=entry.favorite.to_string() aria-label=if entry.favorite { "Remove favorite" } else { "Add to favorites" } disabled=move || busy.get()
                        on:click=move |_| { let en = e.get_value(); run(Box::pin(async move { let c = conn()?; reducer(|tx| c.reducers.favorite_entry_then(en.id.clone(), !en.favorite, en.revision, replied(tx))).await }), ""); }><span><Icon name="star"/></span>"Favorite"</button> })}
                    {(owner.get() && !in_locked.get()).then(|| view! { <button class="quick-action" on:click=move |_| tab.set("sharing".into())><span><Icon name="share"/></span>"Share"</button> })}
                }.into_any()
            };
            Some(view! {
                <div class="details-preview">{preview_art}</div>
                <h2>{entry.name.clone()}</h2>
                <p class="muted details-summary">{summary}</p>
                <div class="quick-actions">{quick}</div>
            })
        };
        let tabs = [
            "details",
            if is_folder { "history" } else { "versions" },
            "sharing",
        ];
        let content = move || {
            // History follows the folder's revision itself; the other tabs show the row.
            if shown_tab.get() == "history" {
                return if trashed {
                    view! { <p class="muted">"Restore this folder to see its history."</p> }
                        .into_any()
                } else {
                    let revision = Signal::derive(move || {
                        details.with(|d| d.as_ref().map_or(0, |d| d.entry.revision as u64))
                    });
                    view! { <crate::history::FolderHistory folder_id=Signal::stored(entry_id.clone()) revision=revision session=session.get_value() notice=history_notice/> }.into_any()
                };
            }
            let Some(d) = details.get() else {
                return ().into_any();
            };
            let entry = d.entry.clone();
            let e = StoredValue::new(entry.clone());
            let location = std::iter::once("All files".to_owned())
                .chain(d.ancestors.iter().map(|a| a.name.clone()))
                .collect::<Vec<_>>()
                .join(" › ");
            let access = if owner.get() {
                "You own this item"
            } else if can_write.get() {
                "You can edit"
            } else {
                "You can view"
            };
            match shown_tab.get().as_str() {
                "details" => view! {
                    <div class="property-card">
                        <div class="property"><Icon name="folder"/><div><span>{location}</span><small>"Location"</small></div></div>
                        <div class="property"><Icon name="clock"/><div><span>{local_date_time(entry.modified_at, &[("dateStyle", "full"), ("timeStyle", "short")])}</span><small>"Modified"</small></div></div>
                        <div class="property"><Icon name="lock"/><div><span>{access}</span><small>{format!("Access · revision {}", entry.revision)}</small></div></div>
                    </div>
                    {(!entry.trashed && can_write.get() && entry.kind == "file").then(|| view! {
                        <div class="property-card"><button class="property property-action" on:click=move |_| if let Some(i) = replacement_input.get_untracked() { i.click() }><Icon name="history"/><div><span>"Upload new version"</span><small>"Keeps the current content in Versions"</small></div></button></div>
                    })}
                }.into_any(),
                "versions" => {
                    if d.versions.is_empty() {
                        view! { <p class="muted">"Folders don’t have content versions."</p> }.into_any()
                    } else {
                        let total = d.versions.len();
                        let file_id = entry.id.clone();
                        d.versions.iter().enumerate().map(|(idx, v)| {
                            let current = v.blob_id == entry.blob_id;
                            let restorable = !entry.trashed && can_write.get() && !current;
                            let version_id = v.id.clone();
                            let dl_url = format!("/api/v1/files/{file_id}/content?version={version_id}");
                            let ver_num = total.saturating_sub(idx);
                            view! {
                                <div class="version">
                                    <span class="version-badge">{format!("v{ver_num}")}</span>
                                    <div><strong>{local_date_time(v.created_at, &[])}</strong><span>{format!("{}{}", format_bytes(v.size), if current { " · Current" } else { "" })}</span></div>
                                    <div class="version-actions">
                                        {(!entry.trashed).then(|| view! {
                                            <a class="icon-button" href=dl_url download="" aria-label=format!("Download version {ver_num}") title="Download version"><Icon name="download"/></a>
                                        })}
                                        {restorable.then(|| view! { <button class="text-button" disabled=move || busy.get() on:click=move |_| { let (en, vid) = (e.get_value(), version_id.clone()); run(Box::pin(async move { let c = conn()?; reducer(|tx| c.reducers.restore_version_then(en.id.clone(), vid, en.revision, replied(tx))).await }), "Version restored."); }>"Restore"</button> })}
                                    </div>
                                </div>
                            }
                        }).collect_view().into_any()
                    }
                }
                _ => {
                    if in_locked.get() {
                        view! { <p class="muted">"Items in the Locked folder can’t be shared. Sharing set up before an item was locked pauses until you move it out."</p> }.into_any()
                    } else if owner.get() && !entry.trashed {
                        let internal_url = format!(
                            "{}/?file={}",
                            window().location().origin().unwrap_or_default(),
                            entry.id
                        );
                        view! {
                            <h3>"Share with a person"</h3>
                            <form class="share-form" on:submit=share>
                                <label>"Username"<input name="username" required maxlength="64" placeholder="Enter a username"/></label>
                                <label>"Permission"<Select name="role" options=vec![("read".to_owned(), "Can view".to_owned()), ("write".to_owned(), "Can edit".to_owned())]/></label>
                                <button class="primary-button" disabled=move || busy.get()>"Share"</button>
                            </form>
                            {d.grants.iter().map(|g| {
                                let grant_id = g.id.clone();
                                view! {
                                    <div class="grant"><span><strong>"@"{g.username.clone()}</strong><small>{if g.role == "write" { "Can edit" } else { "Can view" }}</small></span>
                                        <button class="icon-button" aria-label=format!("Revoke access for {}", g.username) on:click=move |_| { let (gid, en) = (grant_id.clone(), e.get_value()); run(Box::pin(async move { let c = conn()?; reducer(|tx| c.reducers.revoke_grant_then(gid, replied(tx))).await?; show_details(en.id); Ok(()) }), ""); }><Icon name="close"/></button></div>
                                }
                            }).collect_view()}
                            <div class="grant">
                                <span><strong>"Internal link"</strong><small>"For people with access"</small></span>
                                <button type="button" class="text-button" on:click=move |_| {
                                    let _ = window().navigator().clipboard().write_text(&internal_url);
                                    notice.set("Internal link copied.".into());
                                }><Icon name="copy"/>"Copy"</button>
                            </div>
                            {(entry.kind == "file").then(|| view! {
                                <h3>"Public download link"</h3>
                                <p class="muted small">"Anyone with the link can download this file."</p>
                                <div class="share-expiry-row">
                                    <label>"Expires in"<Select value=public_expiry_days aria_label="Link expiration" options=vec![("1".to_owned(), "1 day".to_owned()), ("7".to_owned(), "7 days".to_owned()), ("30".to_owned(), "30 days".to_owned())]/></label>
                                    <button class="secondary-button" disabled=move || busy.get() on:click=move |_| make_public_link()><Icon name="share"/>"Create link"</button>
                                </div>
                                {move || (!public_link.get().is_empty()).then(|| {
                                    let link_val = public_link.get();
                                    let copy_val = link_val.clone();
                                    view! {
                                        <label>"Copy this link"
                                            <div class="public-link-copy">
                                                <input readonly prop:value=link_val on:focus=move |ev| { if let Some(i) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) { i.select() } }/>
                                                <button type="button" class="secondary-button" on:click=move |_| {
                                                    let _ = window().navigator().clipboard().write_text(&copy_val);
                                                    notice.set("Public link copied.".into());
                                                }><Icon name="copy"/>"Copy"</button>
                                            </div>
                                            <small class="field-hint">"The secret link is shown only here. Save it before closing."</small>
                                        </label>
                                    }
                                })}
                                {d.public_shares.iter().map(|link| {
                                    let link_id = link.id.clone();
                                    view! { <div class="grant"><span>{format!("Expires {}", short_date(link.expires_at))}</span><button class="text-button danger" on:click=move |_| { let (lid, en) = (link_id.clone(), e.get_value()); run(Box::pin(async move { let c = conn()?; reducer(|tx| c.reducers.revoke_public_share_then(lid, replied(tx))).await?; public_link.set(String::new()); show_details(en.id); Ok(()) }), ""); }>"Revoke"</button></div> }
                                }).collect_view()}
                            })}
                        }.into_any()
                    } else {
                        view! { <p class="muted">{if entry.trashed { "Restore this item before sharing it." } else { "Only the owner can manage sharing." }}</p> }.into_any()
                    }
                }
            }
        };
        Some(view! {
            <aside class="details-panel" aria-label="File details">
                <div class="details-heading"><span>{if is_folder { "Folder details" } else { "File details" }}</span><button class="icon-button" aria-label="Close details" on:click=move |_| { details.set(None); detail_sequence.update(|s| *s += 1); }><Icon name="close"/></button></div>
                {overview}
                <div class="details-tabs" role="tablist" aria-label="File information">
                    {tabs.into_iter().map(|id| view! { <button role="tab" aria-selected=move || (shown_tab.get() == id).to_string() on:click=move |_| tab.set(id.into())>{format!("{}{}", id[..1].to_uppercase(), &id[1..])}</button> }).collect_view()}
                </div>
                <div class="details-content" role="tabpanel" aria-label=move || shown_tab.get()>{content}</div>
            </aside>
        })
    };

    let breadcrumb = move || {
        let (s, p, crumbs) = (section.get(), parent_id.get(), path.get());
        let in_files = s == "files" && !p.is_empty();
        let in_lock = s == "locked" && !p.is_empty();
        let show_folder_menu = (s == "files" || locked_open.get()) && connected.get();
        let current_folder_id = p.clone();
        view! {
            {in_files.then(|| {
                let parts = crumbs[..crumbs.len().saturating_sub(1)].to_vec();
                view! {
                    <button
                        class=move || if drop_target.with(|d| d.as_deref() == Some("")) { "is-drop-target" } else { "" }
                        on:click=move |_| navigate("files", String::new(), Vec::new())
                        on:dragover=move |ev: web_sys::DragEvent| { if dragging_id.with_untracked(Option::is_some) { ev.prevent_default(); drop_target.set(Some(String::new())); } }
                        on:dragleave=move |_| { if drop_target.with_untracked(|d| d.as_deref() == Some("")) { drop_target.set(None); } }
                        on:drop=move |ev: web_sys::DragEvent| { if dragging_id.with_untracked(Option::is_some) { ev.prevent_default(); ev.stop_propagation(); move_dragged_into(String::new()); } }>"All files"</button>
                    {parts.iter().enumerate().map(|(index, part)| {
                        let (id, id_over, id_leave, id_drop, id_cls, upto) = (part.id.clone(), part.id.clone(), part.id.clone(), part.id.clone(), part.id.clone(), crumbs[..=index].to_vec());
                        view! {
                            <span><Icon name="chevron"/><button
                                class=move || if drop_target.with(|d| d.as_deref() == Some(id_cls.as_str())) { "is-drop-target" } else { "" }
                                on:click=move |_| navigate("files", id.clone(), upto.clone())
                                on:dragover=move |ev: web_sys::DragEvent| { if dragging_id.with_untracked(Option::is_some) { ev.prevent_default(); drop_target.set(Some(id_over.clone())); } }
                                on:dragleave=move |_| { if drop_target.with_untracked(|d| d.as_deref() == Some(id_leave.as_str())) { drop_target.set(None); } }
                                on:drop=move |ev: web_sys::DragEvent| { if dragging_id.with_untracked(Option::is_some) { ev.prevent_default(); ev.stop_propagation(); move_dragged_into(id_drop.clone()); } }>{part.name.clone()}</button></span>
                        }
                    }).collect_view()}
                    <Icon name="chevron"/>
                }
            })}
            {in_lock.then(|| {
                let parts: Vec<Crumb> = crumbs.iter().skip(1).take(crumbs.len().saturating_sub(2)).cloned().collect();
                view! {
                    <button on:click=move |_| navigate("locked", String::new(), Vec::new())>"Locked folder"</button>
                    {parts.iter().enumerate().map(|(index, part)| { let (id, upto) = (part.id.clone(), crumbs[..(index + 2).min(crumbs.len())].to_vec()); view! { <span><Icon name="chevron"/><button on:click=move |_| navigate("locked", id.clone(), upto.clone())>{part.name.clone()}</button></span> } }).collect_view()}
                    <Icon name="chevron"/>
                }
            })}
            <h1>{title()}</h1>
            {show_folder_menu.then(|| {
                let fid = current_folder_id.clone();
                view! {
                    <ActionMenu label="Current folder options" class="breadcrumb-menu" align_start=true actions=move |_| {
                        let mut list = vec![
                            MenuAction::new("New folder", "folder").run(move || open_modal(Modal { kind: ModalKind::Folder, entry: None, entries: Vec::new() })),
                            MenuAction::new("Upload files", "upload").run(move || if let Some(i) = upload_input.get_untracked() { i.click() }),
                            MenuAction::new("Upload folder", "upload").run(move || if let Some(i) = folder_input.get_untracked() { i.click() }),
                        ];
                        if !fid.is_empty() {
                            let (f1, f2, f3, f4) = (fid.clone(), fid.clone(), fid.clone(), fid.clone());
                            list.push(MenuAction::new("Folder details", "info").separator().run(move || { tab.set("details".into()); show_details(f1.clone()); }));
                            list.push(MenuAction::new("Git folder history", "history").run(move || { tab.set("history".into()); show_details(f2.clone()); }));
                            if !in_locked.get_untracked() {
                                list.push(MenuAction::new("Share folder", "share").run(move || { tab.set("sharing".into()); show_details(f3.clone()); }));
                            }
                            list.push(MenuAction::new("Download as ZIP", "download").separator().run(move || download_zip(vec![f4.clone()])));
                        }
                        list
                    }><Icon name="chevron"/></ActionMenu>
                }
            })}
        }
    };

    let modal_title = move |m: &Modal| -> String {
        let n = m.entries.len();
        let items = |verb: &str, one: &str| {
            if n > 0 {
                format!("{verb} {n} {}", if n == 1 { "item" } else { "items" })
            } else {
                one.to_owned()
            }
        };
        match m.kind {
            ModalKind::Folder => "New folder".into(),
            ModalKind::Rename => "Rename item".into(),
            ModalKind::Move => items("Move", "Move item"),
            ModalKind::Copy => items("Copy", "Copy item"),
            ModalKind::Delete => "Delete permanently?".into(),
        }
    };
    let modal_form = move |m: Modal| {
        let has_name = matches!(m.kind, ModalKind::Folder | ModalKind::Rename)
            || (m.kind == ModalKind::Copy && m.entry.is_some());
        let default_name = match (&m.kind, &m.entry) {
            (ModalKind::Copy, Some(e)) => format!("{} copy", e.name),
            (_, Some(e)) => e.name.clone(),
            _ => String::new(),
        };
        let picking = matches!(m.kind, ModalKind::Move | ModalKind::Copy);
        let exclude: Vec<String> = match &m.entry {
            Some(e) => vec![e.id.clone()],
            None => m.entries.iter().map(|e| e.id.clone()).collect(),
        };
        let delete_text = match &m.entry {
            Some(e) => format!(
                "This permanently removes “{}” and their versions. This cannot be undone.",
                e.name
            ),
            None => format!(
                "This permanently removes {} selected items and their versions. This cannot be undone.",
                if m.entries.is_empty() {
                    selected.with_untracked(HashSet::len)
                } else {
                    m.entries.len()
                }
            ),
        };
        let kind = m.kind;
        let initial = target_path
            .with_untracked(|p| p.last().map(|c| c.id.clone()))
            .unwrap_or_default();
        view! {
            <form on:submit=submit_modal>
                {has_name.then(|| view! { <label>"Name"<input name="name" required maxlength="255" value=default_name.clone() autofocus/></label> })}
                {picking.then(|| view! { <FolderPicker initial_folder_id=initial.clone() exclude_ids=exclude.clone() disabled=Signal::derive(move || busy.get()) on_change=Callback::new(move |s| target_folder.set(s))/> })}
                {(kind == ModalKind::Delete).then(|| view! { <p>{delete_text.clone()}</p> })}
                {move || (!error.get().is_empty()).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                <div class="dialog-actions">
                    <button type="button" class="text-button" on:click=move |_| if kind == ModalKind::Delete { modal.set(None) }>"Cancel"</button>
                    <button class=if kind == ModalKind::Delete { "danger-button" } else { "primary-button" } disabled=move || busy.get() || (picking && target_folder.with(Option::is_none))>
                        {move || if busy.get() { "Saving…" } else { match kind { ModalKind::Folder => "Create folder", ModalKind::Delete => "Delete permanently", ModalKind::Move => "Move here", ModalKind::Copy => "Copy here", ModalKind::Rename => "Save" } }}
                    </button>
                </div>
            </form>
        }
    };
    let close_modal = Callback::new(move |_| modal.set(None));
    let header_session = session.get_value();

    view! {
        <div class="app-shell has-files-main"
            on:dragover=move |e: web_sys::DragEvent| { if e.data_transfer().is_some_and(|d| { let types = d.types(); types.iter().any(|t| t.as_string().as_deref() == Some("Files")) }) { e.prevent_default() } }
            on:drop=move |e: web_sys::DragEvent| {
                // Dropped folders are read now: their entries are gone once the event ends.
                let entries = e.data_transfer().map(|d| folder_upload::dropped_entries(&d)).unwrap_or_default();
                if !entries.is_empty() {
                    e.prevent_default();
                    if writable_here.get_untracked() {
                        let entries = send_wrapper::SendWrapper::new(entries);
                        spawn_local(async move {
                            match folder_upload::walk(entries.take()).await {
                                Ok(tree) => upload_folder(tree),
                                Err(cause) => { error.try_set(cause); }
                            }
                        });
                    }
                    return;
                }
                let files: Vec<web_sys::File> = e.data_transfer().and_then(|d| d.files()).map(|l| (0..l.length()).filter_map(|i| l.item(i)).collect()).unwrap_or_default();
                if !files.is_empty() { e.prevent_default(); if writable_here.get_untracked() { upload(files, None) } }
            }>
            <WorkspaceHeader app="files" session=header_session on_signed_out=on_signed_out on_settings=Callback::new(move |_| settings.set(true))/>
            <div class="workspace">
                {move || sidebar.get().then(|| view! { <button class="nav-scrim" aria-label="Close navigation" on:click=move |_| sidebar.set(false)></button> })}
                <aside id="files-navigation" class=move || if sidebar.get() { "sidebar is-open" } else { "sidebar " } aria-label="Files navigation">
                    <ActionMenu label="New" class="files-new-menu" align_start=true actions=move |_| {
                        // Tracked: a menu opened while the connection is still starting enables its items
                        // as soon as it is ready.
                        let off = !connected.get() || !writable_here.get();
                        vec![
                            MenuAction::new("Folder", "folder").disabled(off).run(move || open_modal(Modal { kind: ModalKind::Folder, entry: None, entries: Vec::new() })),
                            MenuAction::new("Upload files", "upload").separator().disabled(off).run(move || { sidebar.set(false); if let Some(i) = upload_input.get_untracked() { i.click() } }),
                            MenuAction::new("Upload folder", "upload").disabled(off).run(move || { sidebar.set(false); if let Some(i) = folder_input.get_untracked() { i.click() } }),
                            MenuAction::new("File", "file").separator().disabled(off).items(vec![
                                MenuAction::plain("Text file").run(move || { sidebar.set(false); new_file.set(Some("text")); }),
                                MenuAction::plain("Markdown file").run(move || { sidebar.set(false); new_file.set(Some("markdown")); }),
                            ]),
                        ]
                    }><Icon name="plus"/><span>"New"</span><Icon name="chevron"/></ActionMenu>
                    <nav>{SECTIONS.iter().map(|&(id, label, icon)| {
                        let show_divider = id == "locked";
                        let is_all_files = id == "files";
                        let badge = move || overview.with(|o| match id {
                            "favorites" => (o.counts.favorites > 0).then_some(o.counts.favorites),
                            "shared" => {
                                let total = o.counts.shared_with_you + o.counts.shared_by_you + o.counts.public_links;
                                (total > 0).then_some(total)
                            }
                            "trash" => (o.counts.trash > 0).then_some(o.counts.trash),
                            _ => None,
                        });
                        view! {
                            {show_divider.then(|| view! { <div class="nav-divider" aria-hidden="true"></div> })}
                            <button class=move || if section.get() == id { "nav-item active" } else { "nav-item" }
                                aria-current=move || (section.get() == id).then_some("page")
                                on:click=move |_| {
                                    if is_all_files && section.get_untracked() == "files" && parent_id.with_untracked(String::is_empty) {
                                        tree_open.update(|o| *o = !*o);
                                    } else {
                                        navigate(id, String::new(), Vec::new());
                                    }
                                }><Icon name=icon/>{label}{move || badge().map(|n| view! { <span class="nav-count" aria-hidden="true">{n}</span> })}</button>
                            {is_all_files.then(|| view! {
                                {move || {
                                    let folders = overview.with(|o| o.root_folders.clone());
                                    (tree_open.get() && !folders.is_empty()).then(|| view! {
                                        <div class="folder-tree" role="group" aria-label="Folders">
                                            {folders.into_iter().take(12).map(|rf| {
                                                let (fid, fid_act, fname, fname_lbl) = (rf.id.clone(), rf.id.clone(), rf.name.clone(), rf.name.clone());
                                                let tracked = rf.git_tracked;
                                                view! {
                                                    <button class=move || if section.get() == "files" && parent_id.get() == fid_act { "folder-tree-item active" } else { "folder-tree-item" }
                                                        aria-label=format!("Go to folder {fname_lbl}")
                                                        on:click=move |_| navigate("files", fid.clone(), vec![Crumb { id: fid.clone(), name: fname.clone() }])>
                                                        <Icon name="folder"/><span>{rf.name.clone()}</span>
                                                        {tracked.then(|| view! { <span class="folder-tree-git" title="Git history tracked" aria-hidden="true"></span> })}
                                                    </button>
                                                }
                                            }).collect_view()}
                                        </div>
                                    })
                                }}
                            })}
                        }
                    }).collect_view()}</nav>
                    <div class="storage">
                        <div class="storage-top">
                            <strong><Icon name="folder"/>"Storage"</strong>
                            {move || usage.get().filter(|u| u.quota > 0.0).map(|u| view! {
                                <span class="storage-pct">{format!("{}%", ((u.used / u.quota) * 100.0).round().clamp(0.0, 100.0) as u32)}</span>
                            })}
                        </div>
                        {move || match usage.get().filter(|u| u.quota > 0.0) {
                            Some(u) => view! {
                                <progress max=u.quota value=u.used aria-label="Storage used"></progress>
                                <div class="storage-meta"><span>{format_bytes(u.used)}" used"</span><span>{format_bytes(u.quota)}</span></div>
                            }.into_any(),
                            None => view! {
                                <div class="storage-meta"><span>{format_bytes(usage.with(|u| u.as_ref().map(|u| u.used).unwrap_or(0.0)))}" used"</span></div>
                            }.into_any(),
                        }}
                        {move || {
                            let tb = overview.with(|o| o.storage.trash_bytes);
                            (tb > 0.0).then(|| view! {
                                <button type="button" class="storage-trash-link" on:click=move |_| navigate("trash", String::new(), Vec::new())>{format!("{} in deleted files", format_bytes(tb))}</button>
                            })
                        }}
                    </div>
                    <AppSettingsButton app="files" on_click=Callback::new(move |_| { sidebar.set(false); settings.set(true); })/>
                </aside>
                <main class="files-main" tabindex="-1">
                    <ConnectionStatus state=state/>
                    <div class="page-heading files-heading">
                        <nav class="breadcrumb" aria-label="Breadcrumb">{breadcrumb}</nav>
                        <div class="heading-actions">
                            {move || locked_open.get().then(|| view! { <button class="secondary-button" title="Lock now" on:click=move |_| spawn_local(async move { match locked.lock().await { Ok(_) => navigate("locked", String::new(), Vec::new()), Err(e) => error.set(failure(&e.message)) } })><Icon name="lock"/><span>"Lock"</span></button> })}
                            {move || (section.get() == "trash" && !visible.with(Vec::is_empty)).then(|| view! {
                                <button class="files-empty-trash" disabled=move || !connected.get() || busy.get() on:click=move |_| empty_trash_confirm.set(true)><Icon name="trash"/><span>"Empty deleted files"</span></button>
                            })}
                            {move || (!(in_locked.get() && !locked_open.get())).then(|| view! {
                                <div class="view-switch" aria-label="File display">
                                    <button class=move || if !grid.get() { "icon-button selected" } else { "icon-button" } aria-label="List view" title="List view" aria-pressed=move || (!grid.get()).to_string() on:click=move |_| change_view(false)><Icon name="list"/></button>
                                    <button class=move || if grid.get() { "icon-button selected" } else { "icon-button" } aria-label="Grid view" title="Grid view" aria-pressed=move || grid.get().to_string() on:click=move |_| change_view(true)><Icon name="grid"/></button>
                                </div>
                                <button class=move || if details.with(Option::is_some) { "icon-button files-inspector-toggle is-active" } else { "icon-button files-inspector-toggle" }
                                    aria-label="Toggle details" title="Toggle details"
                                    aria-pressed=move || details.with(Option::is_some).to_string()
                                    on:click=move |_| {
                                        if details.with_untracked(Option::is_some) {
                                            details.set(None);
                                            detail_sequence.update(|s| *s += 1);
                                        } else if let Some(first_sel) = selected.with_untracked(|s| s.iter().next().cloned()) {
                                            show_details(first_sel);
                                        } else if !parent_id.with_untracked(String::is_empty) {
                                            show_details(parent_id.get_untracked());
                                        } else if let Some(first) = filtered_rows.with_untracked(|r| r.first().cloned()) {
                                            select(&first);
                                        }
                                    }><Icon name="info"/></button>
                            })}
                        </div>
                    </div>
                    {move || (!error.get().is_empty()).then(|| view! { <div class="error-banner" role="alert"><span>{error.get()}</span><button class="icon-button" aria-label="Dismiss error" on:click=move |_| error.set(String::new())><Icon name="close"/></button></div> })}
                    {move || clipboard.get().map(|c| view! {
                        <div class="ui-clipboard" role="status"><span>{if c.mode == "cut" { "Cut" } else { "Copied" }}": "{c.name.clone()}</span>
                            <button class="secondary-button" disabled=move || !connected.get() || busy.get() || !writable_here.get() on:click=move |_| paste()>"Paste here"</button>
                            <button class="text-button" on:click=move |_| remember(None)>"Clear clipboard"</button></div>
                    })}
                    {move || (!notice.get().is_empty()).then(|| view! {
                        <div class="notice files-toast" role="status"><Icon name="check"/><span>{notice.get()}</span>
                            {(undo_trash.with(Option::is_some) && notice.get().ends_with("moved to deleted files.")).then(|| view! { <button class="text-button" on:click=move |_| undo_last_trash()>"Undo"</button> })}
                            <button class="icon-button" aria-label="Dismiss notification" on:click=move |_| notice.set(String::new())><Icon name="close"/></button></div>
                    })}
                    {move || (!(in_locked.get() && !locked_open.get()) && !(section.get() == "trash" && visible.with(Vec::is_empty))).then(|| {
                        let sort_label = move || {
                            let dir = if sort_asc.get() { "↑" } else { "↓" };
                            let name = match sort_field.get() {
                                "modified" => if section.get() == "trash" { "Deleted" } else { "Modified" },
                                "size" => "Size",
                                "type" => "Type",
                                _ => "Name",
                            };
                            format!("{name} {dir}")
                        };
                        let sort_menu = move || view! {
                            <ActionMenu label="Sort files" class="file-sort-menu" actions=move |_| {
                                let (cur, asc) = (sort_field.get(), sort_asc.get());
                                vec![
                                    MenuAction::plain("Name (A–Z)").current(cur == "name" && asc).run(move || { sort_field.set("name"); sort_asc.set(true); }),
                                    MenuAction::plain("Name (Z–A)").current(cur == "name" && !asc).run(move || { sort_field.set("name"); sort_asc.set(false); }),
                                    MenuAction::plain("Modified (newest)").separator().current(cur == "modified" && !asc).run(move || { sort_field.set("modified"); sort_asc.set(false); }),
                                    MenuAction::plain("Modified (oldest)").current(cur == "modified" && asc).run(move || { sort_field.set("modified"); sort_asc.set(true); }),
                                    MenuAction::plain("Size (largest)").separator().current(cur == "size" && !asc).run(move || { sort_field.set("size"); sort_asc.set(false); }),
                                    MenuAction::plain("Size (smallest)").current(cur == "size" && asc).run(move || { sort_field.set("size"); sort_asc.set(true); }),
                                    MenuAction::plain("Type").separator().current(cur == "type").run(move || { sort_field.set("type"); sort_asc.set(true); }),
                                ]
                            }><Icon name="sort"/><span>{sort_label}</span></ActionMenu>
                        };
                        view! {
                            <div class=move || if selected.with(HashSet::is_empty) { "file-toolbar" } else { "file-toolbar is-selecting" }>
                                {move || if !selected.with(HashSet::is_empty) {
                                    view! { <button class="icon-button" aria-label="Clear selection" title="Clear selection" on:click=move |_| selected.set(HashSet::new())><Icon name="close"/></button><strong>{selected.with(HashSet::len)}" selected"</strong><span class="file-toolbar-spacer"></span>{selection_actions()} }.into_any()
                                } else if section.get() == "shared" {
                                    let counts = overview.with(|o| (o.counts.shared_with_you, o.counts.shared_by_you, o.counts.public_links));
                                    view! {
                                        <div class="segmented" role="tablist" aria-label="Shared views">
                                            <button type="button" role="tab" aria-pressed=move || (shared_sub.get() == "with_you").to_string() on:click=move |_| shared_sub.set("with_you")>"Shared with you"{(counts.0 > 0).then(|| format!(" ({})", counts.0))}</button>
                                            <button type="button" role="tab" aria-pressed=move || (shared_sub.get() == "by_you").to_string() on:click=move |_| shared_sub.set("by_you")>"Shared by you"{(counts.1 > 0).then(|| format!(" ({})", counts.1))}</button>
                                            <button type="button" role="tab" aria-pressed=move || (shared_sub.get() == "links").to_string() on:click=move |_| shared_sub.set("links")>"Public links"{(counts.2 > 0).then(|| format!(" ({})", counts.2))}</button>
                                        </div>
                                        {sort_menu()}
                                    }.into_any()
                                } else if section.get() == "trash" {
                                    let tb = overview.with(|o| o.storage.trash_bytes);
                                    view! {
                                        <div class="file-trash-strip">
                                            <span>"Items keep their original folder until permanently deleted."</span>
                                            {(tb > 0.0).then(|| view! { <strong>{format!("{} reclaimable", format_bytes(tb))}</strong> })}
                                        </div>
                                        {sort_menu()}
                                    }.into_any()
                                } else {
                                    let (all, doc, image, code, archive, media) = filter_counts.get();
                                    let chips: [(&'static str, &'static str, usize); 6] = [
                                        ("all", "All", visible.with(Vec::len)),
                                        ("doc", "Docs", doc),
                                        ("image", "Images", image),
                                        ("code", "Code", code),
                                        ("archive", "Archives", archive),
                                        ("media", "Media", media),
                                    ];
                                    view! {
                                        <div class="file-filter-chips" role="group" aria-label="Filter by type">
                                            {chips.into_iter().filter(|(k, _, c)| *k == "all" || *c > 0 || type_filter.get() == *k).map(|(k, lbl, c)| {
                                                let count_str = if k == "all" && all == 0 && c == 0 { String::new() } else { c.to_string() };
                                                view! {
                                                    <button type="button" class=move || if type_filter.get() == k { "file-filter-chip is-active" } else { "file-filter-chip" }
                                                        aria-pressed=move || (type_filter.get() == k).to_string()
                                                        on:click=move |_| type_filter.set(k)>{lbl}{(!count_str.is_empty()).then(|| view! { <span>{count_str}</span> })}</button>
                                                }
                                            }).collect_view()}
                                        </div>
                                        {sort_menu()}
                                    }.into_any()
                                }}
                            </div>
                        }
                    })}
                    {listing}
                    {move || (!previous.with(Vec::is_empty) || has_more.get()).then(|| view! {
                        <div class="pagination">
                            <button class="secondary-button" disabled=move || previous.with(Vec::is_empty) on:click=move |_| { cursor.set(previous.with_untracked(|p| p.last().cloned()).unwrap_or_default()); previous.update(|p| { p.pop(); }); }>"Previous"</button>
                            <span>"Page "{previous.with(Vec::len) + 1}</span>
                            <button class="secondary-button" disabled=move || !has_more.get() on:click=move |_| { previous.update(|p| p.push(cursor.get_untracked())); cursor.set(visible.with_untracked(|v| v.last().map(|e| e.list_key.clone())).unwrap_or_default()); }>"Next"</button>
                        </div>
                    })}
                    {move || (writable_here.get() && !visible.with(Vec::is_empty)).then(|| view! { <div class="drop-hint"><Icon name="upload"/>" Drag files or folders here to upload"</div> })}
                </main>
                {details_panel}
            </div>
            <input node_ref=upload_input class="sr-only" type="file" multiple tabindex="-1" aria-label="Choose files to upload"
                on:change=move |e| { if let Some(i) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) { upload(files_of(&i), None); i.set_value(""); } }/>
            <input node_ref=folder_input class="sr-only" type="file" multiple tabindex="-1" aria-label="Choose a folder to upload"
                on:change=move |e| { if let Some(i) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) { upload_folder(folder_upload::from_picker(files_of(&i))); i.set_value(""); } }/>
            <input node_ref=replacement_input class="sr-only" type="file" tabindex="-1" aria-label="Choose a new file version"
                on:change=move |e| { if let Some(i) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) { upload(files_of(&i), chosen.get_untracked()); i.set_value(""); } }/>
            {move || (!transfers.with(Vec::is_empty)).then(|| view! {
                <section class="transfer-panel" aria-label="Uploads">
                    <div class="transfer-heading"><strong>"Uploads"</strong><button class="icon-button" aria-label="Dismiss finished uploads" on:click=move |_| transfers.update(|t| t.retain(|i| i.status == "uploading"))><Icon name="close"/></button></div>
                    {transfers.get().into_iter().map(|t| {
                        let id = t.id.clone();
                        let uploading = t.status == "uploading";
                        view! {
                            <div class="transfer"><Icon name=if t.status == "complete" { "check" } else { "upload" }/>
                                <div><strong>{t.name.clone()}</strong><span role="status">{if uploading { format!("Uploading · {}%", (t.progress * 100.0).round()) } else if t.status == "complete" { "Uploaded".into() } else { t.message.clone() }}</span>
                                    {uploading.then(|| view! { <progress prop:value=t.progress max="1" aria-label=format!("Upload progress for {}", t.name)></progress> })}</div>
                                {uploading.then(|| view! { <button class="icon-button" aria-label=format!("Cancel upload {}", t.name) on:click=move |_| cancel_upload(&id)><Icon name="close"/></button> })}
                            </div>
                        }
                    }).collect_view()}
                </section>
            })}
            {move || text_editor.get().map(|entry| view! { <TextEditor entry=entry session=session.get_value() on_close=Callback::new(move |_| text_editor.set(None))/> })}
            {move || new_file.get().map(|kind| {
                let markdown = kind == "markdown";
                view! {
                    <EditorPane title=if markdown { "New Markdown file" } else { "New text file" } on_close=Callback::new(move |_| new_file.set(None))>
                        <form on:submit=move |event: web_sys::SubmitEvent| {
                            event.prevent_default();
                            let name = event.target().and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok()).and_then(|f| web_sys::FormData::new_with_form(&f).ok()).and_then(|d| d.get("name").as_string()).unwrap_or_default();
                            let here = folder.get_untracked();
                            spawn_local(async move {
                                perform(Box::pin(async move {
                                    connection.get_untracked().ok_or_else(disconnected)?;
                                    let id = write_text_file(&session.get_value(), TextFile { name: &name, parent_id: &here, content: "", media_type: if markdown { "text/markdown" } else { "text/plain" }, file_id: None, expected_revision: None }).await.map_err(|e| e.message)?;
                                    let detail = file_details(&id).await?;
                                    new_file.set(None);
                                    text_editor.set(Some(detail.entry));
                                    Ok(())
                                }), String::new()).await;
                            });
                        }>
                            <label>"Filename"<input name="name" required maxlength="255" value=if markdown { "Untitled.md" } else { "Untitled.txt" }/></label>
                            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                            <div class="dialog-actions"><button type="button" class="text-button">"Cancel"</button><button class="primary-button" disabled=move || busy.get()>"Create file"</button></div>
                        </form>
                    </EditorPane>
                }
            })}
            {move || unlock_action.get().map(|(action, then)| view! {
                <UnlockDialog folder=locked action=action on_close=Callback::new(move |_| unlock_action.set(None)) on_unlocked=Callback::new(move |s: locked::LockedState| { unlock_action.set(None); if !s.folder_id.is_empty() { then.run(s.folder_id) } })/>
            })}
            {move || settings.get().then(|| view! { <FilesSettings session=session.get_value() on_close=Callback::new(move |_| settings.set(false))/> })}
            {move || trash_confirmation.get().map(|items| {
                let text = if items.len() == 1 { items[0].name.clone() } else { format!("{} selected items", items.len()) };
                let items = StoredValue::new(items);
                view! {
                    <Dialog title="Move to deleted files?" on_close=Callback::new(move |_| trash_confirmation.set(None))>
                        <p>{format!("{text} can be restored from Deleted files.")}</p>
                        <div class="dialog-actions"><button class="text-button" on:click=move |_| trash_confirmation.set(None)>"Cancel"</button>
                            <button class="primary-button" disabled=move || busy.get() on:click=move |_| spawn_local(async move { if trash_entries(items.get_value()).await { trash_confirmation.set(None) } })>"Move to deleted files"</button></div>
                    </Dialog>
                }
            })}
            {move || modal.get().map(|m| {
                let title = modal_title(&m);
                if m.kind == ModalKind::Delete {
                    let m = StoredValue::new(m);
                    view! { <Dialog title=title on_close=close_modal>{move || modal_form(m.get_value())}</Dialog> }.into_any()
                } else {
                    let m = StoredValue::new(m);
                    view! { <EditorPane title=title on_close=close_modal>{move || modal_form(m.get_value())}</EditorPane> }.into_any()
                }
            })}
            {move || empty_trash_confirm.get().then(|| view! {
                <Dialog title="Empty deleted files?" on_close=Callback::new(move |_| empty_trash_confirm.set(false))>
                    <p>"This permanently removes every item in Deleted files and their versions. This cannot be undone."</p>
                    <div class="dialog-actions">
                        <button class="text-button" on:click=move |_| empty_trash_confirm.set(false)>"Cancel"</button>
                        <button class="danger-button" disabled=move || busy.get() on:click=move |_| spawn_local(async move {
                            let ok = perform(Box::pin(async move {
                                let c = conn()?;
                                reducer(|tx| c.reducers.empty_trash_then(replied(tx))).await?;
                                selected.set(HashSet::new());
                                details.set(None);
                                Ok(())
                            }), "Deleted files emptied.".into()).await;
                            if ok { empty_trash_confirm.set(false); }
                        })>"Empty deleted files"</button>
                    </div>
                </Dialog>
            })}
            {move || preview_entry.get().map(|entry| {
                let siblings: Vec<FileEntry> = filtered_rows.get_untracked().into_iter().filter(|i| i.kind == "file" && !editable_text(i)).collect();
                let e = StoredValue::new(entry.clone());
                let details_open = Signal::derive(move || details.with(Option::is_some));
                view! {
                    <FilePreview
                        entry=entry
                        siblings=siblings
                        on_open=Callback::new(move |en| set_preview(Some(en)))
                        on_close=Callback::new(move |_| set_preview(None))
                        actions=vec![MenuAction::new("Details", "info").run(move || select(&e.get_value()))]
                        on_share=Callback::new(move |en| open_share(en))
                        on_toggle_details=Callback::new(move |en| {
                            if details.with_untracked(Option::is_some) {
                                details.set(None);
                                detail_sequence.update(|s| *s += 1);
                            } else {
                                select(&en);
                            }
                        })
                        details_open=details_open
                    />
                }
            })}
        </div>
    }
}
