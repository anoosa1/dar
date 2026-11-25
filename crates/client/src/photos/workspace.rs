//! Photos (`photos/workspace.tsx`): views and albums in the sidebar, a month-grouped timeline grid,
//! viewer with details panel, Live Photos, video transcoding, stacks, duplicates, trash, Locked folder,
//! album sharing, uploads and Photos settings.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use super::explore::{AlbumPickerDialog, AlbumsHub, ExploreView, FoldersView, album_date_span};
use super::immich_settings::{ImmichProgressBar, ImmichSettings};
use super::sharing::{LinkDialog, LinksList, SharePane};
use super::{
    BASE, PhotoAlbum, PhotoAsset, PhotoDetail, PhotoDuplicateGroup, PhotoDuplicatePage,
    PhotoImmichProgress, PhotoMoved, PhotoOverview, PhotoPage, PhotoStack, PhotosNotice, Playback,
    Transfer, content_url, css_escape, download_zip_archive, encode_uri_component, error_message,
    file_last_modified, format_duration, format_number, noun, photo_folder_parts, put_upload_file,
    request_animation_frame, save_edited_copy, thumb_url,
};
use crate::api::{self, BrowserSession, Req, RequestError, format_bytes};
use crate::dates::{local_date, local_date_time, local_time};
use crate::files::locked::{LockedFolder, UnlockDialog, UnlockForm};
use crate::icons::Icon;
use crate::settings::{SaveProblems, Saver, SettingRow, SettingsSheet};
use crate::shell::WorkspaceHeader;
use crate::ui::{ActionMenu, Dialog, MenuAction, document, listen, window};

#[derive(Clone, Debug, PartialEq, Eq)]
enum View {
    Folder,
    Library,
    Explore,
    Favorites,
    Albums,
    Shared,
    Videos,
    Folders,
    Duplicates,
    Archive,
    Locked,
    Trash,
    Recent,
    Large,
    Unassigned,
    FolderId(String, String),
    Camera(String),
    Place(String, String),
    Memory(i64, u32, u32),
    Album(String),
}

impl View {
    fn from_type_str(s: &str) -> Option<Self> {
        match s {
            "folder" => Some(Self::Folder),
            "library" => Some(Self::Library),
            "explore" => Some(Self::Explore),
            "favorites" => Some(Self::Favorites),
            "albums" => Some(Self::Albums),
            "shared" => Some(Self::Shared),
            "videos" => Some(Self::Videos),
            "folders" => Some(Self::Folders),
            "duplicates" => Some(Self::Duplicates),
            "archive" => Some(Self::Archive),
            "locked" => Some(Self::Locked),
            "trash" => Some(Self::Trash),
            "recent" => Some(Self::Recent),
            "large" => Some(Self::Large),
            "unassigned" => Some(Self::Unassigned),
            _ => None,
        }
    }

    fn as_type_str(&self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Library => "library",
            Self::Explore => "explore",
            Self::Favorites => "favorites",
            Self::Albums => "albums",
            Self::Shared => "shared",
            Self::Videos => "videos",
            Self::Folders => "folders",
            Self::Duplicates => "duplicates",
            Self::Archive => "archive",
            Self::Locked => "locked",
            Self::Trash => "trash",
            Self::Recent => "recent",
            Self::Large => "large",
            Self::Unassigned => "unassigned",
            Self::FolderId(_, _) => "folderId",
            Self::Camera(_) => "camera",
            Self::Place(_, _) => "place",
            Self::Memory(_, _, _) => "memory",
            Self::Album(_) => "album",
        }
    }

    fn key(&self) -> String {
        match self {
            Self::Album(id) => format!("album:{id}"),
            Self::FolderId(id, _) => format!("folderId:{id}"),
            Self::Camera(name) => format!("camera:{name}"),
            Self::Place(key, _) => format!("place:{key}"),
            Self::Memory(y, m, d) => format!("memory:{y}-{m}-{d}"),
            _ => self.as_type_str().to_owned(),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Folder => "Photos".to_owned(),
            Self::Library => "All photos".to_owned(),
            Self::Explore => "Explore".to_owned(),
            Self::Favorites => "Favorites".to_owned(),
            Self::Albums => "Albums".to_owned(),
            Self::Shared => "Shared with me".to_owned(),
            Self::Videos => "Videos".to_owned(),
            Self::Folders => "Folders".to_owned(),
            Self::Duplicates => "Duplicates".to_owned(),
            Self::Archive => "Archive".to_owned(),
            Self::Locked => "Locked folder".to_owned(),
            Self::Trash => "Trash".to_owned(),
            Self::Recent => "Recently added".to_owned(),
            Self::Large => "Large files".to_owned(),
            Self::Unassigned => "Not in any album".to_owned(),
            Self::FolderId(_, path) => path.clone(),
            Self::Camera(name) => name.clone(),
            Self::Place(_, label) => format!("Place · {label}"),
            Self::Memory(y, _, _) => format!("On this day in {y}"),
            Self::Album(_) => "Album".to_owned(),
        }
    }

    fn is_sub_collection(&self) -> bool {
        matches!(
            self,
            Self::Recent
                | Self::Large
                | Self::Unassigned
                | Self::FolderId(_, _)
                | Self::Camera(_)
                | Self::Place(_, _)
                | Self::Memory(_, _, _)
        )
    }

    fn supports_filters(&self) -> bool {
        matches!(
            self,
            Self::Folder
                | Self::Library
                | Self::Favorites
                | Self::Videos
                | Self::Album(_)
                | Self::Recent
                | Self::Large
                | Self::Unassigned
                | Self::FolderId(_, _)
                | Self::Camera(_)
                | Self::Place(_, _)
                | Self::Memory(_, _, _)
        )
    }
}

const MAIN_VIEWS: [(&str, &str, &str); 8] = [
    ("folder", "Photos", "image"),
    ("library", "All photos", "grid"),
    ("explore", "Explore", "compass"),
    ("favorites", "Favorites", "star"),
    ("albums", "Albums", "albums"),
    ("shared", "Shared with me", "share"),
    ("videos", "Videos", "video"),
    ("folders", "Folders", "folder"),
];

const MORE_VIEWS: [(&str, &str, &str); 4] = [
    ("duplicates", "Duplicates", "copy"),
    ("archive", "Archive", "archive"),
    ("locked", "Locked folder", "lock"),
    ("trash", "Trash", "trash"),
];

const ZOOM_STEPS: [u32; 7] = [3, 4, 5, 6, 8, 10, 12];

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListState {
    Loading,
    Ready,
    Error,
}

#[derive(Clone)]
struct AlbumDialogState {
    album: Option<PhotoAlbum>,
    ids: Option<Vec<String>>,
}

#[derive(Clone)]
struct DeletingState {
    ids: Vec<String>,
    empty: bool,
}

#[derive(Clone)]
struct PendingUnlock {
    action: String,
    run: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Clone, Default)]
struct AssetPatch {
    favorite: Option<bool>,
    archived: Option<bool>,
    revision: Option<u64>,
}

#[derive(Deserialize)]
struct TrashedResponse {
    #[serde(default)]
    trashed: Vec<String>,
}

#[derive(Deserialize)]
struct RestoredResponse {
    #[serde(default)]
    restored: Vec<String>,
}

#[derive(Deserialize)]
struct LockedMovedResponse {
    #[serde(default)]
    moved: Vec<PhotoMoved>,
}

#[derive(Deserialize)]
struct UnlockedMovedResponse {
    #[serde(default)]
    moved: Vec<String>,
}

#[derive(Deserialize)]
struct IdResponse {
    id: String,
}

#[derive(Deserialize)]
struct AddToAlbumResponse {
    #[serde(default)]
    added: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadTargetResponse {
    parent_id: String,
}

#[derive(Deserialize)]
struct EmptyTrashResponse {
    #[serde(default)]
    deleted: usize,
    #[serde(default)]
    remaining: bool,
}

#[derive(Deserialize)]
struct DeleteAssetsResponse {
    #[serde(default)]
    deleted: Vec<String>,
}

fn read_view() -> (View, Option<String>) {
    let search = window().location().search().unwrap_or_default();
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok();
    let get = |k: &str| {
        params
            .as_ref()
            .and_then(|p| p.get(k))
            .filter(|s| !s.is_empty())
    };
    let album = get("album");
    let view_param = get("view");
    let asset = get("asset");
    let view = if let Some(id) = album {
        View::Album(id)
    } else if view_param.as_deref() == Some("camera")
        && let Some(cam) = get("camera")
    {
        View::Camera(cam)
    } else if view_param.as_deref() == Some("place")
        && let Some(place) = get("place")
    {
        let label = get("label").unwrap_or_else(|| place.clone());
        View::Place(place, label)
    } else if view_param.as_deref() == Some("folderId") {
        let folder = get("folder").unwrap_or_default();
        let label = get("label").unwrap_or_else(|| {
            if folder.is_empty() {
                "Files".to_owned()
            } else {
                "Folder".to_owned()
            }
        });
        View::FolderId(folder, label)
    } else if view_param.as_deref() == Some("memory")
        && let (Some(y), Some(m), Some(d)) = (
            get("year").and_then(|s| s.parse::<i64>().ok()),
            get("month").and_then(|s| s.parse::<u32>().ok()),
            get("day").and_then(|s| s.parse::<u32>().ok()),
        )
    {
        View::Memory(y, m, d)
    } else if let Some(v) = view_param.as_deref().and_then(View::from_type_str) {
        v
    } else {
        View::Folder
    };
    (view, asset)
}

fn write_view(view: &View, asset: Option<&str>) {
    let Ok(params) = web_sys::UrlSearchParams::new() else {
        return;
    };
    match view {
        View::Album(id) => params.set("album", id),
        View::Folder => {}
        View::Camera(name) => {
            params.set("view", "camera");
            params.set("camera", name);
        }
        View::Place(key, label) => {
            params.set("view", "place");
            params.set("place", key);
            params.set("label", label);
        }
        View::FolderId(id, label) => {
            params.set("view", "folderId");
            params.set("folder", id);
            params.set("label", label);
        }
        View::Memory(y, m, d) => {
            params.set("view", "memory");
            params.set("year", &y.to_string());
            params.set("month", &m.to_string());
            params.set("day", &d.to_string());
        }
        _ => params.set("view", view.as_type_str()),
    }
    if let Some(id) = asset
        && !id.is_empty()
    {
        params.set("asset", id);
    }
    let qs = params.to_string().as_string().unwrap_or_default();
    let next = if qs.is_empty() {
        "/photos".to_owned()
    } else {
        format!("/photos?{qs}")
    };
    let loc = window().location();
    let current = format!(
        "{}{}",
        loc.pathname().unwrap_or_default(),
        loc.search().unwrap_or_default()
    );
    if next != current
        && let Ok(history) = window().history()
    {
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&next));
    }
}

fn month_label(time: f64) -> String {
    local_date(time, &[("month", "long"), ("year", "numeric")])
}

fn day_label(time: f64) -> String {
    local_date(
        time,
        &[
            ("weekday", "short"),
            ("month", "short"),
            ("day", "numeric"),
            ("year", "numeric"),
        ],
    )
}

fn scrubber_month_label(time: f64) -> String {
    local_date(time, &[("month", "short"), ("year", "numeric")])
}

fn read_layout() -> &'static str {
    match window()
        .local_storage()
        .ok()
        .flatten()
        .and_then(|s| s.get_item("dar:photos:layout").ok().flatten())
        .as_deref()
    {
        Some("square") => "square",
        _ => "justified",
    }
}

fn read_memories_hidden() -> bool {
    window()
        .local_storage()
        .ok()
        .flatten()
        .and_then(|s| s.get_item("dar:photos:hideMemories").ok().flatten())
        .as_deref()
        == Some("1")
}

fn has_owner(asset: &PhotoAsset) -> bool {
    asset.owner.as_deref().is_some_and(|s| !s.is_empty())
}

fn has_live(asset: &PhotoAsset) -> bool {
    asset.live.as_deref().is_some_and(|s| !s.is_empty())
}

fn is_browser_decodable_image(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp" | "image/avif"
    )
}

fn is_supported_upload_file(file: &web_sys::File) -> bool {
    let mime = file.type_();
    if mime.starts_with("image/") || mime.starts_with("video/") {
        return true;
    }
    let name = file.name().to_lowercase();
    [
        ".heic", ".heif", ".dng", ".cr2", ".cr3", ".nef", ".arw", ".orf", ".rw2", ".raf", ".mov",
        ".mkv",
    ]
    .iter()
    .any(|ext| name.ends_with(ext))
}

fn hevc_playable() -> bool {
    document()
        .create_element("video")
        .ok()
        .and_then(|el| el.dyn_into::<web_sys::HtmlVideoElement>().ok())
        .is_some_and(|v| {
            !v.can_play_type("video/mp4; codecs=\"hvc1.1.6.L93.B0\"")
                .is_empty()
        })
}

fn read_zoom() -> u32 {
    window()
        .local_storage()
        .ok()
        .flatten()
        .and_then(|s| s.get_item("dar:photos:zoom").ok().flatten())
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(5)
}

fn query_assets_url(view: &View, sort: &str, filter: &str, after: &str) -> String {
    let params = web_sys::UrlSearchParams::new().expect("UrlSearchParams");
    params.set("view", view.as_type_str());
    params.set("limit", "120");
    match view {
        View::Album(id) => params.set("album", id),
        View::FolderId(id, _) => params.set("folder", id),
        View::Camera(name) => params.set("camera", name),
        View::Place(key, _) => params.set("place", key),
        View::Memory(y, m, d) => {
            params.set("year", &y.to_string());
            params.set("month", &m.to_string());
            params.set("day", &d.to_string());
        }
        _ => {}
    }
    if view.supports_filters() {
        if !sort.is_empty() && sort != "newest" {
            params.set("sort", sort);
        }
        if !filter.is_empty() && filter != "all" {
            params.set("filter", filter);
        }
    }
    if !after.is_empty() {
        params.set("cursor", after);
    }
    format!(
        "{BASE}/assets?{}",
        params.to_string().as_string().unwrap_or_default()
    )
}

fn to_datetime_local_value(ms: f64) -> String {
    let d = js_sys::Date::new(&JsValue::from_f64(ms));
    let y = d.get_full_year();
    let m = d.get_month() + 1;
    let day = d.get_date();
    let h = d.get_hours();
    let min = d.get_minutes();
    format!("{y:04}-{m:02}-{day:02}T{h:02}:{min:02}")
}

fn parse_datetime_local_value(val: &str) -> Option<f64> {
    let ms = js_sys::Date::parse(val);
    if ms.is_finite() && ms >= 0.0 {
        Some(ms)
    } else {
        None
    }
}

/// Runs work that may need the Locked folder; a 423 asks for the password, then retries.
fn run_guarded<F, Fut>(
    folder: LockedFolder,
    unlock_action: RwSignal<Option<PendingUnlock>>,
    notice: RwSignal<Option<PhotosNotice>>,
    action: &'static str,
    make_work: F,
) where
    F: Fn() -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), RequestError>> + 'static,
{
    let make_retry = make_work.clone();
    spawn_local(async move {
        if let Err(cause) = make_work().await {
            if cause.status == 423 {
                folder.refresh();
                unlock_action.try_set(Some(PendingUnlock {
                    action: action.to_owned(),
                    run: Arc::new(move || {
                        run_guarded(folder, unlock_action, notice, action, make_retry.clone());
                    }),
                }));
            } else {
                notice.try_set(Some(PhotosNotice {
                    message: error_message(&cause.message),
                    undo: None,
                }));
            }
        }
    });
}

/// Photos: views and albums in the sidebar, a month-grouped grid, the photo in the middle sheet
/// and details on the right.
#[component]
pub fn PhotosWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let (start_view, start_asset) = read_view();
    let overview: RwSignal<Option<PhotoOverview>> = RwSignal::new(None);
    let overview_seq = RwSignal::new(0u32);
    let overview_error = RwSignal::new(String::new());
    let view = RwSignal::new(start_view);
    let items: RwSignal<Vec<PhotoAsset>> = RwSignal::new(Vec::new());
    let cursor = RwSignal::new(String::new());
    let state = RwSignal::new(ListState::Loading);
    let list_error = RwSignal::new(String::new());
    let open: RwSignal<Option<String>> = RwSignal::new(start_asset);
    let selected: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let zoom = RwSignal::new(read_zoom());
    let layout = RwSignal::new(read_layout());
    let sort = RwSignal::new("newest");
    let filter = RwSignal::new("all");
    let hide_memories = RwSignal::new(read_memories_hidden());
    let desktop_info = RwSignal::new(true);
    let mobile_select = RwSignal::new(false);
    let slideshow = RwSignal::new(false);
    let slideshow_timer = RwSignal::new(0u32);
    let picker_album: RwSignal<Option<PhotoAlbum>> = RwSignal::new(None);
    let notice: RwSignal<Option<PhotosNotice>> = RwSignal::new(None);
    let notice_seq = RwSignal::new(0u32);
    let album_dialog: RwSignal<Option<AlbumDialogState>> = RwSignal::new(None);
    let confirm: RwSignal<Option<PhotoAlbum>> = RwSignal::new(None);
    let settings = RwSignal::new(false);
    let transfers: RwSignal<Vec<Transfer>> = RwSignal::new(Vec::new());
    let dragging = RwSignal::new(false);
    let info = RwSignal::new(false);
    let member: RwSignal<Option<PhotoAsset>> = RwSignal::new(None);
    let unlock_action: RwSignal<Option<PendingUnlock>> = RwSignal::new(None);
    let deleting: RwSignal<Option<DeletingState>> = RwSignal::new(None);
    let sharing: RwSignal<Option<PhotoAlbum>> = RwSignal::new(None);
    let link_for: RwSignal<Option<Vec<String>>> = RwSignal::new(None);

    let folder = LockedFolder::new(session.clone());
    let session = StoredValue::new(session);
    let csrf = move || session.with_value(|s| s.csrf_token.clone());

    let generation = StoredValue::new(0u32);
    let shown_key = StoredValue::new(String::new());
    let return_focus: StoredValue<Option<String>> = StoredValue::new(None);
    let last_selected: StoredValue<Option<String>> = StoredValue::new(None);
    let imported_seen = StoredValue::new(0u64);
    let import_running = StoredValue::new(false);
    let indexing_timer = RwSignal::new(0u32);
    let immich_timer = RwSignal::new(0u32);
    let notice_timer = RwSignal::new(0u32);

    let shell_ref = NodeRef::<html::Div>::new();
    let file_input = NodeRef::<html::Input>::new();
    let sentinel = NodeRef::<html::Div>::new();
    let grid = NodeRef::<html::Div>::new();

    let show_notice = move |n: PhotosNotice| {
        notice.try_set(Some(n));
        notice_seq.try_update(|s| *s = s.wrapping_add(1));
    };
    let fail = move |msg: String| {
        show_notice(PhotosNotice {
            message: error_message(&msg),
            undo: None,
        });
    };

    let locked_closed = Memo::new(move |_| view.get() == View::Locked && !folder.unlocked());

    let current_album = Memo::new(move |_| match view.get() {
        View::Album(id) => overview.with(|o| {
            let o = o.as_ref()?;
            o.albums
                .iter()
                .chain(o.shared_albums.iter())
                .find(|a| a.id == id)
                .cloned()
        }),
        _ => None,
    });
    let album_owner =
        Memo::new(move |_| current_album.with(|a| a.as_ref().is_some_and(|a| a.role == "owner")));
    let title = Memo::new(move |_| match view.get() {
        View::Album(_) => current_album
            .with(|a| a.as_ref().map(|a| a.name.clone()))
            .unwrap_or_else(|| "Album".to_owned()),
        v => v.label(),
    });

    let load_overview = move || {
        spawn_local(async move {
            match api::get::<PhotoOverview>(&format!("{BASE}/overview")).await {
                Ok(data) => {
                    overview.try_set(Some(data));
                    overview_error.try_set(String::new());
                    overview_seq.try_update(|s| *s = s.wrapping_add(1));
                }
                Err(cause) => {
                    overview_error.try_set(error_message(&cause.message));
                }
            }
        });
    };

    let load_items = move || {
        let current_view = view.get_untracked();
        let current_sort = sort.get_untracked();
        let current_filter = filter.get_untracked();
        let is_locked_closed = locked_closed.get_untracked();
        let vk = format!("{}:{current_sort}:{current_filter}", current_view.key());
        let ticket = generation.get_value().wrapping_add(1);
        generation.set_value(ticket);
        state.try_set(ListState::Loading);
        selected.try_set(HashSet::new());
        last_selected.set_value(None);
        // A different view never shows the previous view's photos; refreshes of the same view keep them while loading.
        if shown_key.get_value() != vk {
            shown_key.set_value(vk);
            items.try_set(Vec::new());
            cursor.try_set(String::new());
        }
        if matches!(
            current_view,
            View::Duplicates | View::Shared | View::Explore | View::Albums | View::Folders
        ) || is_locked_closed
        {
            items.try_set(Vec::new());
            cursor.try_set(String::new());
            state.try_set(ListState::Ready);
            return;
        }
        let url = query_assets_url(&current_view, current_sort, current_filter, "");
        spawn_local(async move {
            match api::get::<PhotoPage>(&url).await {
                Ok(page) => {
                    if generation.try_get_value() != Some(ticket) {
                        return;
                    }
                    items.try_set(page.items);
                    cursor.try_set(page.cursor);
                    state.try_set(ListState::Ready);
                    if let Some(el) = grid.get_untracked() {
                        el.set_scroll_top(0);
                    }
                }
                Err(cause) => {
                    if generation.try_get_value() != Some(ticket) {
                        return;
                    }
                    if cause.status == 423 {
                        folder.refresh();
                        items.try_set(Vec::new());
                        state.try_set(ListState::Ready);
                        return;
                    }
                    state.try_set(ListState::Error);
                    list_error.try_set(error_message(&cause.message));
                }
            }
        });
    };

    let load_more = move || {
        let after = cursor.get_untracked();
        if after.is_empty() || state.get_untracked() != ListState::Ready {
            return;
        }
        let ticket = generation.get_value();
        let url = query_assets_url(
            &view.get_untracked(),
            sort.get_untracked(),
            filter.get_untracked(),
            &after,
        );
        spawn_local(async move {
            let Ok(page) = api::get::<PhotoPage>(&url).await else {
                return;
            };
            if generation.try_get_value() != Some(ticket) {
                return;
            }
            items.try_update(|current| {
                for item in page.items {
                    if !current.iter().any(|c| c.id == item.id) {
                        current.push(item);
                    }
                }
            });
            cursor.try_set(page.cursor);
        });
    };

    let refresh = move || {
        load_items();
        load_overview();
    };

    // Initial load.
    load_overview();

    // Reload items whenever view, locked state, sort or filter changes.
    Effect::new(move |_| {
        let _ = view.get();
        let _ = locked_closed.get();
        let _ = sort.get();
        let _ = filter.get();
        load_items();
    });

    // Keep URL in sync and reset viewer sub-states when open asset changes.
    Effect::new(move |_| {
        let v = view.get();
        let o = open.get();
        write_view(&v, o.as_deref());
        if o.is_none() {
            info.set(false);
            slideshow.set(false);
        }
        member.set(None);
    });

    // Persist zoom, layout and memories ribbon preferences.
    Effect::new(move |_| {
        let z = zoom.get();
        if let Some(storage) = window().local_storage().ok().flatten() {
            let _ = storage.set_item("dar:photos:zoom", &z.to_string());
        }
    });
    Effect::new(move |_| {
        let l = layout.get();
        if let Some(storage) = window().local_storage().ok().flatten() {
            let _ = storage.set_item("dar:photos:layout", l);
        }
    });
    Effect::new(move |_| {
        let h = hide_memories.get();
        if let Some(storage) = window().local_storage().ok().flatten() {
            let _ = storage.set_item("dar:photos:hideMemories", if h { "1" } else { "0" });
        }
    });

    // When the Locked folder relocks, its photos leave the screen.
    Effect::new(move |_| {
        if locked_closed.get() && open.with_untracked(Option::is_some) {
            open.set(None);
        }
    });

    // While the background scan is running, refresh so newly indexed media appears.
    Effect::new(move |_| {
        let _ = overview_seq.get();
        let is_indexing = overview.with_untracked(|o| o.as_ref().is_some_and(|o| o.indexing));
        if !is_indexing {
            return;
        }
        let id = indexing_timer.get_untracked().wrapping_add(1);
        indexing_timer.set(id);
        spawn_local(async move {
            crate::sleep(8000).await;
            if indexing_timer.try_get_untracked() != Some(id) {
                return;
            }
            load_overview();
            let busy = selected
                .try_with_untracked(|s| !s.is_empty())
                .unwrap_or(false)
                || open.try_with_untracked(Option::is_some).unwrap_or(false);
            if !busy {
                load_items();
            }
        });
    });

    // During an Immich import the sidebar bar updates every few seconds; new photos appear as they arrive.
    Effect::new(move |_| {
        let _ = overview_seq.get();
        let (has_immich, imported) =
            overview.with_untracked(|o| match o.as_ref().and_then(|o| o.immich.as_ref()) {
                Some(i) => (true, i.imported),
                None => (false, 0),
            });
        let ended = import_running.get_value() && !has_immich;
        import_running.set_value(has_immich);
        if imported != imported_seen.get_value() || ended {
            let changed = imported > imported_seen.get_value() || ended;
            imported_seen.set_value(imported);
            let busy =
                selected.with_untracked(|s| !s.is_empty()) || open.with_untracked(Option::is_some);
            if changed && !busy {
                load_items();
            }
        }
        if !has_immich {
            return;
        }
        let id = immich_timer.get_untracked().wrapping_add(1);
        immich_timer.set(id);
        spawn_local(async move {
            crate::sleep(3000).await;
            if immich_timer.try_get_untracked() != Some(id) {
                return;
            }
            load_overview();
        });
    });

    // Infinite scroll sentinel observer.
    Effect::new(move |_| {
        let cur = cursor.get();
        let Some(node) = sentinel.get() else { return };
        if cur.is_empty() {
            return;
        }
        let cb = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
            let hit = entries.iter().any(|entry| {
                entry
                    .dyn_into::<web_sys::IntersectionObserverEntry>()
                    .is_ok_and(|e| e.is_intersecting())
            });
            if hit {
                load_more();
            }
        });
        let opts = web_sys::IntersectionObserverInit::new();
        if let Some(root) = grid.get_untracked() {
            opts.set_root(Some(root.unchecked_ref()));
        }
        opts.set_root_margin("600px");
        if let Ok(observer) =
            web_sys::IntersectionObserver::new_with_options(cb.as_ref().unchecked_ref(), &opts)
        {
            observer.observe(node.unchecked_ref());
            let kept = send_wrapper::SendWrapper::new((observer, cb));
            on_cleanup(move || {
                let (observer, _cb) = kept.take();
                observer.disconnect();
            });
        }
    });

    // Auto-dismiss toast notices.
    Effect::new(move |_| {
        let _ = notice_seq.get();
        let Some(has_undo) = notice.with(|n| n.as_ref().map(|n| n.undo.is_some())) else {
            return;
        };
        let id = notice_timer.get_untracked().wrapping_add(1);
        notice_timer.set(id);
        spawn_local(async move {
            crate::sleep(if has_undo { 8000 } else { 4000 }).await;
            if notice_timer.try_get_untracked() == Some(id) {
                notice.try_set(None);
            }
        });
    });

    // Restore focus to the photo tile after closing the viewer.
    Effect::new(move |_| {
        if open.get().is_none()
            && let Some(id) = return_focus.get_value()
        {
            return_focus.set_value(None);
            request_animation_frame(move || {
                let selector = format!("[data-asset=\"{}\"] .photos-open", css_escape(&id));
                let target = grid
                    .get_untracked()
                    .and_then(|g| g.query_selector(&selector).ok().flatten())
                    .or_else(|| document().query_selector(&selector).ok().flatten());
                if let Some(el) = target.and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
                    let _ = el.focus();
                }
            });
        }
    });

    let choose = move |next: View| {
        if next.key() != view.get_untracked().key() {
            items.set(Vec::new());
            cursor.set(String::new());
            state.set(ListState::Loading);
            sort.set("newest");
            filter.set("all");
            mobile_select.set(false);
            last_selected.set_value(None);
        }
        view.set(next);
        open.set(None);
    };

    let toggle_selected = move |id: String, shift: bool| {
        if shift
            && let Some(anchor) = last_selected.get_value()
        {
            let range_ids: Vec<String> = items.with_untracked(|list| {
                let a = list.iter().position(|i| i.id == anchor);
                let b = list.iter().position(|i| i.id == id);
                match (a, b) {
                    (Some(x), Some(y)) => {
                        let (lo, hi) = if x <= y { (x, y) } else { (y, x) };
                        list[lo..=hi].iter().map(|i| i.id.clone()).collect()
                    }
                    _ => Vec::new(),
                }
            });
            if !range_ids.is_empty() {
                selected.update(|set| {
                    for rid in range_ids {
                        set.insert(rid);
                    }
                });
                last_selected.set_value(Some(id));
                return;
            }
        }
        selected.update(|set| {
            if !set.remove(&id) {
                set.insert(id.clone());
            }
        });
        last_selected.set_value(Some(id));
    };

    let toggle_day_selected = move |day_ids: Vec<String>| {
        selected.update(|set| {
            let all_in = day_ids.iter().all(|id| set.contains(id));
            if all_in {
                for id in &day_ids {
                    set.remove(id);
                }
            } else {
                for id in day_ids {
                    set.insert(id);
                }
            }
        });
    };

    let download_zip = move |ids: Vec<String>, filename: String| {
        if ids.is_empty() {
            return;
        }
        let csrf_tok = csrf();
        show_notice(PhotosNotice {
            message: format!("Preparing ZIP for {}\u{2026}", noun(ids.len())),
            undo: None,
        });
        spawn_local(async move {
            if let Err(msg) = download_zip_archive(ids, &filename, &csrf_tok).await {
                fail(msg);
            }
        });
    };

    let set_album_cover = move |album: PhotoAlbum, cover_id: String| {
        let csrf_tok = csrf();
        spawn_local(async move {
            match api::send::<PhotoAlbum>(
                &format!("{BASE}/albums/{}", album.id),
                "PATCH",
                Some(json!({ "coverId": cover_id, "revision": album.revision })),
                &csrf_tok,
            )
            .await
            {
                Ok(_) => {
                    load_overview();
                    show_notice(PhotosNotice {
                        message: "Album cover updated".to_owned(),
                        undo: None,
                    });
                }
                Err(cause) => fail(cause.message),
            }
        });
    };

    let save_album_description = move |album: PhotoAlbum, next_desc: String| {
        let trimmed = next_desc.trim().to_owned();
        if trimmed == album.description {
            return;
        }
        let csrf_tok = csrf();
        spawn_local(async move {
            match api::send::<PhotoAlbum>(
                &format!("{BASE}/albums/{}", album.id),
                "PATCH",
                Some(json!({ "description": trimmed, "revision": album.revision })),
                &csrf_tok,
            )
            .await
            {
                Ok(_) => {
                    load_overview();
                    show_notice(PhotosNotice {
                        message: "Album description saved".to_owned(),
                        undo: None,
                    });
                }
                Err(cause) => fail(cause.message),
            }
        });
    };

    let patch_local = move |ids: Vec<String>, patch: AssetPatch| {
        items.try_update(|list| {
            for item in list.iter_mut() {
                if ids.contains(&item.id) {
                    if let Some(fav) = patch.favorite {
                        item.favorite = fav;
                    }
                    if let Some(arch) = patch.archived {
                        item.archived = arch;
                    }
                    if let Some(rev) = patch.revision {
                        item.revision = rev;
                    }
                }
            }
        });
    };

    let remove_local = move |ids: Vec<String>| {
        items.try_update(|list| list.retain(|i| !ids.contains(&i.id)));
        selected.try_set(HashSet::new());
        if open.with_untracked(|o| o.as_ref().is_some_and(|id| ids.contains(id))) {
            open.try_set(None);
        }
    };

    let update_assets = move |ids: Vec<String>,
                              favorite: Option<bool>,
                              archived: Option<bool>,
                              undo: Option<(Option<bool>, Option<bool>)>,
                              reload_after: bool| {
        #[allow(clippy::too_many_arguments)]
        fn dispatch(
            folder: LockedFolder,
            unlock_action: RwSignal<Option<PendingUnlock>>,
            notice: RwSignal<Option<PhotosNotice>>,
            notice_seq: RwSignal<u32>,
            view: RwSignal<View>,
            items: RwSignal<Vec<PhotoAsset>>,
            selected: RwSignal<HashSet<String>>,
            open: RwSignal<Option<String>>,
            session: StoredValue<BrowserSession>,
            load_overview_cb: Callback<()>,
            load_items_cb: Callback<()>,
            ids: Vec<String>,
            favorite: Option<bool>,
            archived: Option<bool>,
            undo: Option<(Option<bool>, Option<bool>)>,
            reload_after: bool,
        ) {
            run_guarded(
                folder,
                unlock_action,
                notice,
                "change these photos",
                move || {
                    let ids = ids.clone();
                    let csrf_tok = session.with_value(|s| s.csrf_token.clone());
                    async move {
                        let mut body = serde_json::Map::new();
                        body.insert("ids".to_owned(), json!(ids));
                        if let Some(f) = favorite {
                            body.insert("favorite".to_owned(), json!(f));
                        }
                        if let Some(a) = archived {
                            body.insert("archived".to_owned(), json!(a));
                        }
                        api::send::<Value>(
                            &format!("{BASE}/assets"),
                            "PATCH",
                            Some(Value::Object(body)),
                            &csrf_tok,
                        )
                        .await?;
                        let cur_view = view.get_untracked();
                        let leaves = archived.is_some_and(|a| {
                            if cur_view == View::Archive {
                                !a
                            } else {
                                !matches!(cur_view, View::Album(_) | View::Favorites | View::Locked)
                                    && a
                            }
                        }) || (favorite == Some(false) && cur_view == View::Favorites);
                        if leaves {
                            items.try_update(|list| list.retain(|i| !ids.contains(&i.id)));
                            selected.try_set(HashSet::new());
                            if open
                                .with_untracked(|o| o.as_ref().is_some_and(|id| ids.contains(id)))
                            {
                                open.try_set(None);
                            }
                        } else {
                            items.try_update(|list| {
                                for item in list.iter_mut() {
                                    if ids.contains(&item.id) {
                                        if let Some(f) = favorite {
                                            item.favorite = f;
                                        }
                                        if let Some(a) = archived {
                                            item.archived = a;
                                        }
                                    }
                                }
                            });
                            selected.try_set(HashSet::new());
                        }
                        load_overview_cb.run(());
                        if reload_after {
                            load_items_cb.run(());
                        }
                        if let Some((undo_fav, undo_arch)) = undo {
                            let msg = if let Some(a) = archived {
                                if a {
                                    format!("{} archived", noun(ids.len()))
                                } else {
                                    format!("{} restored to the library", noun(ids.len()))
                                }
                            } else if favorite == Some(true) {
                                format!("{} added to Favorites", noun(ids.len()))
                            } else {
                                format!("{} removed from Favorites", noun(ids.len()))
                            };
                            let ids_undo = ids.clone();
                            notice.try_set(Some(PhotosNotice {
                                message: msg,
                                undo: Some(Arc::new(move || {
                                    dispatch(
                                        folder,
                                        unlock_action,
                                        notice,
                                        notice_seq,
                                        view,
                                        items,
                                        selected,
                                        open,
                                        session,
                                        load_overview_cb,
                                        load_items_cb,
                                        ids_undo.clone(),
                                        undo_fav,
                                        undo_arch,
                                        None,
                                        true,
                                    );
                                })),
                            }));
                            notice_seq.try_update(|s| *s = s.wrapping_add(1));
                        }
                        Ok(())
                    }
                },
            );
        }
        dispatch(
            folder,
            unlock_action,
            notice,
            notice_seq,
            view,
            items,
            selected,
            open,
            session,
            Callback::new(move |_| load_overview()),
            Callback::new(move |_| load_items()),
            ids,
            favorite,
            archived,
            undo,
            reload_after,
        );
    };

    let trash_assets = move |ids: Vec<String>| {
        run_guarded(
            folder,
            unlock_action,
            notice,
            "move these photos",
            move || {
                let ids = ids.clone();
                let csrf_tok = csrf();
                async move {
                    let res: TrashedResponse = api::send(
                        &format!("{BASE}/assets/trash"),
                        "POST",
                        Some(json!({ "ids": ids })),
                        &csrf_tok,
                    )
                    .await?;
                    let trashed = res.trashed;
                    remove_local(trashed.clone());
                    load_overview();
                    show_notice(PhotosNotice {
                        message: format!("{} moved to the trash", noun(ids.len())),
                        undo: Some(Arc::new(move || {
                            let trashed = trashed.clone();
                            let csrf_tok = csrf();
                            spawn_local(async move {
                                match api::send::<Value>(
                                    &format!("{BASE}/assets/restore"),
                                    "POST",
                                    Some(json!({ "ids": trashed })),
                                    &csrf_tok,
                                )
                                .await
                                {
                                    Ok(_) => refresh(),
                                    Err(cause) => fail(cause.message),
                                }
                            });
                        })),
                    });
                    Ok(())
                }
            },
        );
    };

    let restore_assets = move |ids: Vec<String>| {
        run_guarded(
            folder,
            unlock_action,
            notice,
            "restore these photos",
            move || {
                let ids = ids.clone();
                let csrf_tok = csrf();
                async move {
                    let res: RestoredResponse = api::send(
                        &format!("{BASE}/assets/restore"),
                        "POST",
                        Some(json!({ "ids": ids })),
                        &csrf_tok,
                    )
                    .await?;
                    let restored = res.restored;
                    remove_local(restored.clone());
                    load_overview();
                    show_notice(PhotosNotice {
                        message: format!("{} restored", noun(ids.len())),
                        undo: Some(Arc::new(move || {
                            let restored = restored.clone();
                            let csrf_tok = csrf();
                            spawn_local(async move {
                                match api::send::<Value>(
                                    &format!("{BASE}/assets/trash"),
                                    "POST",
                                    Some(json!({ "ids": restored })),
                                    &csrf_tok,
                                )
                                .await
                                {
                                    Ok(_) => refresh(),
                                    Err(cause) => fail(cause.message),
                                }
                            });
                        })),
                    });
                    Ok(())
                }
            },
        );
    };

    let lock_photos = move |ids: Vec<String>| {
        run_guarded(
            folder,
            unlock_action,
            notice,
            "move photos into it",
            move || {
                let ids = ids.clone();
                let csrf_tok = csrf();
                async move {
                    let res: LockedMovedResponse = api::send(
                        &format!("{BASE}/assets/lock"),
                        "POST",
                        Some(json!({ "ids": ids })),
                        &csrf_tok,
                    )
                    .await?;
                    let moved = res.moved;
                    let moved_ids: Vec<String> = moved.iter().map(|m| m.id.clone()).collect();
                    remove_local(moved_ids.clone());
                    load_overview();
                    show_notice(PhotosNotice {
                        message: format!("{} moved to the Locked folder", noun(ids.len())),
                        undo: Some(Arc::new(move || {
                            let moved = moved.clone();
                            let moved_ids = moved_ids.clone();
                            run_guarded(
                                folder,
                                unlock_action,
                                notice,
                                "move them back",
                                move || {
                                    let moved = moved.clone();
                                    let moved_ids = moved_ids.clone();
                                    let csrf_tok = csrf();
                                    async move {
                                        api::send::<Value>(
                                            &format!("{BASE}/assets/unlock"),
                                            "POST",
                                            Some(json!({ "ids": moved_ids, "to": moved })),
                                            &csrf_tok,
                                        )
                                        .await?;
                                        refresh();
                                        Ok(())
                                    }
                                },
                            );
                        })),
                    });
                    Ok(())
                }
            },
        );
    };

    let unlock_photos = move |ids: Vec<String>| {
        run_guarded(
            folder,
            unlock_action,
            notice,
            "move photos out of it",
            move || {
                let ids = ids.clone();
                let csrf_tok = csrf();
                async move {
                    let res: UnlockedMovedResponse = api::send(
                        &format!("{BASE}/assets/unlock"),
                        "POST",
                        Some(json!({ "ids": ids })),
                        &csrf_tok,
                    )
                    .await?;
                    let moved = res.moved;
                    remove_local(moved.clone());
                    load_overview();
                    let dest = overview
                        .with_untracked(|o| {
                            o.as_ref()
                                .map(|o| o.upload_folder.clone())
                                .filter(|s| !s.is_empty())
                        })
                        .unwrap_or_else(|| "your Photos folder".to_owned());
                    show_notice(PhotosNotice {
                        message: format!("{} moved to {dest}", noun(ids.len())),
                        undo: Some(Arc::new(move || {
                            let moved = moved.clone();
                            run_guarded(
                                folder,
                                unlock_action,
                                notice,
                                "move them back",
                                move || {
                                    let moved = moved.clone();
                                    let csrf_tok = csrf();
                                    async move {
                                        api::send::<Value>(
                                            &format!("{BASE}/assets/lock"),
                                            "POST",
                                            Some(json!({ "ids": moved })),
                                            &csrf_tok,
                                        )
                                        .await?;
                                        refresh();
                                        Ok(())
                                    }
                                },
                            );
                        })),
                    });
                    Ok(())
                }
            },
        );
    };

    let stack_photos = move |ids: Vec<String>| {
        run_guarded(
            folder,
            unlock_action,
            notice,
            "stack these photos",
            move || {
                let ids = ids.clone();
                let csrf_tok = csrf();
                async move {
                    let created: IdResponse = api::send(
                        &format!("{BASE}/stacks"),
                        "POST",
                        Some(json!({ "ids": ids })),
                        &csrf_tok,
                    )
                    .await?;
                    refresh();
                    let stack_id = created.id;
                    show_notice(PhotosNotice {
                        message: format!("Stacked {} photos", ids.len()),
                        undo: Some(Arc::new(move || {
                            let stack_id = stack_id.clone();
                            let csrf_tok = csrf();
                            spawn_local(async move {
                                match api::request::<Value>(
                                    &format!("{BASE}/stacks/{stack_id}"),
                                    Req {
                                        method: "DELETE",
                                        csrf: Some(&csrf_tok),
                                        ..Default::default()
                                    },
                                )
                                .await
                                {
                                    Ok(_) => refresh(),
                                    Err(cause) => fail(cause.message),
                                }
                            });
                        })),
                    });
                    Ok(())
                }
            },
        );
    };

    let unstack_photos = move |value: PhotoStack| {
        run_guarded(
            folder,
            unlock_action,
            notice,
            "unstack these photos",
            move || {
                let value = value.clone();
                let csrf_tok = csrf();
                async move {
                    api::request::<Value>(
                        &format!("{BASE}/stacks/{}", value.id),
                        Req {
                            method: "DELETE",
                            csrf: Some(&csrf_tok),
                            ..Default::default()
                        },
                    )
                    .await?;
                    member.try_set(None);
                    refresh();
                    let count = value.items.len();
                    let mut restack_ids = vec![value.primary_id.clone()];
                    for item in &value.items {
                        if item.id != value.primary_id {
                            restack_ids.push(item.id.clone());
                        }
                    }
                    show_notice(PhotosNotice {
                        message: format!("Unstacked {count} photos"),
                        undo: Some(Arc::new(move || {
                            let restack_ids = restack_ids.clone();
                            let csrf_tok = csrf();
                            spawn_local(async move {
                                match api::send::<Value>(
                                    &format!("{BASE}/stacks"),
                                    "POST",
                                    Some(json!({ "ids": restack_ids })),
                                    &csrf_tok,
                                )
                                .await
                                {
                                    Ok(_) => refresh(),
                                    Err(cause) => fail(cause.message),
                                }
                            });
                        })),
                    });
                    Ok(())
                }
            },
        );
    };

    let leave_album = move |target: PhotoAlbum| {
        let user_id = session.with_value(|s| s.user.id.clone());
        let csrf_tok = csrf();
        spawn_local(async move {
            match api::request::<Value>(
                &format!("{BASE}/albums/{}/members/{user_id}", target.id),
                Req {
                    method: "DELETE",
                    csrf: Some(&csrf_tok),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) => {
                    if view.get_untracked() == View::Album(target.id.clone()) {
                        choose(View::Shared);
                    }
                    load_overview();
                    show_notice(PhotosNotice {
                        message: format!("You left {}", target.name),
                        undo: None,
                    });
                }
                Err(cause) => fail(cause.message),
            }
        });
    };

    let add_to_album = move |target: PhotoAlbum, ids: Vec<String>| {
        let csrf_tok = csrf();
        spawn_local(async move {
            match api::send::<AddToAlbumResponse>(
                &format!("{BASE}/albums/{}/add", target.id),
                "POST",
                Some(json!({ "ids": ids })),
                &csrf_tok,
            )
            .await
            {
                Ok(result) => {
                    selected.try_set(HashSet::new());
                    load_overview();
                    let msg = if result.added > 0 {
                        format!("Added {} to {}", result.added, target.name)
                    } else {
                        format!("Already in {}", target.name)
                    };
                    show_notice(PhotosNotice {
                        message: msg,
                        undo: None,
                    });
                }
                Err(cause) => fail(cause.message),
            }
        });
    };

    let remove_from_album = move |ids: Vec<String>| {
        let View::Album(album_id) = view.get_untracked() else {
            return;
        };
        let album_name = current_album
            .with_untracked(|a| a.as_ref().map(|a| a.name.clone()))
            .unwrap_or_else(|| "the album".to_owned());
        let csrf_tok = csrf();
        spawn_local(async move {
            match api::send::<Value>(
                &format!("{BASE}/albums/{album_id}/remove"),
                "POST",
                Some(json!({ "ids": ids })),
                &csrf_tok,
            )
            .await
            {
                Ok(_) => {
                    remove_local(ids.clone());
                    load_overview();
                    let count = ids.len();
                    show_notice(PhotosNotice {
                        message: format!("Removed {count} from {album_name}"),
                        undo: Some(Arc::new(move || {
                            let ids = ids.clone();
                            let album_id = album_id.clone();
                            let csrf_tok = csrf();
                            spawn_local(async move {
                                match api::send::<Value>(
                                    &format!("{BASE}/albums/{album_id}/add"),
                                    "POST",
                                    Some(json!({ "ids": ids })),
                                    &csrf_tok,
                                )
                                .await
                                {
                                    Ok(_) => refresh(),
                                    Err(cause) => fail(cause.message),
                                }
                            });
                        })),
                    });
                }
                Err(cause) => fail(cause.message),
            }
        });
    };

    let upload_files = move |raw_files: Vec<web_sys::File>| {
        let had_files = !raw_files.is_empty();
        let list: Vec<web_sys::File> = raw_files
            .into_iter()
            .filter(is_supported_upload_file)
            .collect();
        if list.is_empty() {
            if had_files {
                show_notice(PhotosNotice {
                    message: "Only photos and videos can be uploaded here. Use Files for other documents.".to_owned(),
                    undo: None,
                });
            }
            return;
        }
        let list = send_wrapper::SendWrapper::new(list);
        let csrf_tok = csrf();
        spawn_local(async move {
            for file in list.take() {
                let transfer_id = crate::random_uuid();
                let name = file.name();
                transfers.try_update(|t| {
                    t.push(Transfer {
                        id: transfer_id.clone(),
                        name: name.clone(),
                        progress: 0.0,
                        status: "uploading",
                        message: None,
                    });
                });
                let change_transfer = move |tid: &str, f: &dyn Fn(&mut Transfer)| {
                    transfers.try_update(|list| {
                        if let Some(t) = list.iter_mut().find(|t| t.id == tid) {
                            f(t);
                        }
                    });
                };
                let mut upload_id = String::new();
                let res: Result<(), String> = async {
                    let when = js_sys::Date::new(&JsValue::from_f64(file_last_modified(&file)));
                    let target: UploadTargetResponse = api::send(
                        &format!("{BASE}/upload-target"),
                        "POST",
                        Some(json!({
                            "year": when.get_full_year(),
                            "month": when.get_month() + 1,
                        })),
                        &csrf_tok,
                    )
                    .await
                    .map_err(|e| error_message(&e.message))?;
                    let (stem, ext) = match name.rfind('.') {
                        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
                        _ => (name.as_str(), ""),
                    };
                    let mime = file.type_();
                    let media_type = if mime.is_empty() {
                        "application/octet-stream".to_owned()
                    } else {
                        mime
                    };
                    let mut attempt = 0u32;
                    while upload_id.is_empty() {
                        let candidate = if attempt > 0 {
                            format!("{stem} ({}){ext}", attempt + 1)
                        } else {
                            name.clone()
                        };
                        match api::send::<IdResponse>(
                            "/api/v1/files/uploads",
                            "POST",
                            Some(json!({
                                "parentId": target.parent_id,
                                "name": candidate,
                                "size": file.size(),
                                "mediaType": media_type,
                            })),
                            &csrf_tok,
                        )
                        .await
                        {
                            Ok(reserved) => upload_id = reserved.id,
                            Err(cause) if cause.status == 409 && attempt < 20 => {
                                attempt += 1;
                            }
                            Err(cause) => return Err(error_message(&cause.message)),
                        }
                    }
                    let progress_id = transfer_id.clone();
                    let text = put_upload_file(
                        &format!("/api/v1/files/uploads/{upload_id}"),
                        Some(&csrf_tok),
                        &file,
                        move |p| {
                            change_transfer(&progress_id, &|t| t.progress = p);
                        },
                    )
                    .await?;
                    let uploaded: IdResponse = serde_json::from_str(&text)
                        .map_err(|_| "The upload could not be completed.".to_owned())?;
                    let _ = api::send::<Value>(
                        &format!("{BASE}/assets/{}/index", uploaded.id),
                        "POST",
                        None,
                        &csrf_tok,
                    )
                    .await;
                    Ok(())
                }
                .await;

                match res {
                    Ok(()) => change_transfer(&transfer_id, &|t| {
                        t.progress = 1.0;
                        t.status = "complete";
                    }),
                    Err(msg) => {
                        change_transfer(&transfer_id, &|t| {
                            t.status = "error";
                            t.message = Some(msg.clone());
                        });
                        if !upload_id.is_empty() {
                            let _ = api::request::<Value>(
                                &format!("/api/v1/files/uploads/{upload_id}"),
                                Req {
                                    method: "DELETE",
                                    csrf: Some(&csrf_tok),
                                    ..Default::default()
                                },
                            )
                            .await;
                        }
                    }
                }
            }
            load_overview();
            load_items();
        });
    };

    let current_index = Memo::new(move |_| {
        let id = open.get()?;
        items.with(|list| list.iter().position(|i| i.id == id))
    });
    let current_asset = Memo::new(move |_| {
        let idx = current_index.get()?;
        items.with(|list| list.get(idx).cloned())
    });
    let shown_asset = Memo::new(move |_| member.get().or_else(|| current_asset.get()));
    let has_viewer = Memo::new(move |_| current_asset.with(Option::is_some));

    let step = move |delta: isize| {
        let Some(idx) = current_index.get_untracked() else {
            return;
        };
        let next_idx = idx as isize + delta;
        let list = items.get_untracked();
        if next_idx >= 0
            && let Some(next) = list.get(next_idx as usize)
        {
            open.set(Some(next.id.clone()));
        }
        if next_idx >= list.len() as isize - 8 {
            load_more();
        }
    };

    let close_viewer = move || {
        slideshow.set(false);
        return_focus.set_value(open.get_untracked());
        open.set(None);
    };

    // Slideshow auto-advance (4 seconds per photo while playing).
    Effect::new(move |_| {
        let playing = slideshow.get() && has_viewer.get();
        let _ = open.get();
        if !playing {
            return;
        }
        let ticket = slideshow_timer.get_untracked().wrapping_add(1);
        slideshow_timer.set(ticket);
        spawn_local(async move {
            crate::sleep(4000).await;
            if slideshow_timer.try_get_untracked() != Some(ticket)
                || slideshow.try_get_untracked() != Some(true)
            {
                return;
            }
            let Some(idx) = current_index.try_get_untracked().flatten() else {
                slideshow.try_set(false);
                return;
            };
            let len = items.try_with_untracked(Vec::len).unwrap_or(0);
            if idx + 1 < len {
                step(1);
            } else {
                slideshow.try_set(false);
            }
        });
    });

    // Keyboard navigation while the viewer is open.
    listen(&document(), "keydown", false, move |event| {
        if open.with_untracked(Option::is_none) {
            return;
        }
        let event: web_sys::KeyboardEvent = event.unchecked_into();
        if event.default_prevented() || event.ctrl_key() || event.meta_key() || event.alt_key() {
            return;
        }
        let in_control = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|t| {
                t.closest("input,textarea,select,[contenteditable=true],dialog,.ui-menu")
                    .ok()
                    .flatten()
            })
            .is_some();
        let has_dialog = document()
            .query_selector("dialog[open]")
            .ok()
            .flatten()
            .is_some();
        if in_control || has_dialog {
            return;
        }
        let cur_view = view.get_untracked();
        let in_trash = cur_view == View::Trash;
        let in_locked = cur_view == View::Locked;
        match event.key().as_str() {
            "ArrowRight" => {
                event.prevent_default();
                step(1);
            }
            "ArrowLeft" => {
                event.prevent_default();
                step(-1);
            }
            "Escape" => {
                event.prevent_default();
                close_viewer();
            }
            "i" | "I" => {
                event.prevent_default();
                desktop_info.update(|v| *v = !*v);
            }
            "s" | "S" => {
                event.prevent_default();
                slideshow.update(|v| *v = !*v);
            }
            "f" => {
                if !in_trash && let Some(cur) = current_asset.get_untracked() {
                    event.prevent_default();
                    update_assets(vec![cur.id], Some(!cur.favorite), None, None, false);
                }
            }
            "Delete" => {
                if !in_trash
                    && !in_locked
                    && let Some(cur) = current_asset.get_untracked()
                {
                    event.prevent_default();
                    trash_assets(vec![cur.id]);
                }
            }
            _ => {}
        }
    });

    let album_menu_actions = move |target: Vec<String>| -> Vec<MenuAction> {
        let mut actions = Vec::new();
        let target_new = target.clone();
        actions.push(MenuAction::new("New album\u{2026}", "plus").run(move || {
            album_dialog.set(Some(AlbumDialogState {
                album: None,
                ids: Some(target_new.clone()),
            }));
        }));
        let cur_view = view.get_untracked();
        let list: Vec<PhotoAlbum> = overview.with_untracked(|o| {
            let Some(o) = o else { return Vec::new() };
            o.albums
                .iter()
                .chain(o.shared_albums.iter().filter(|a| a.role == "editor"))
                .filter(|a| cur_view != View::Album(a.id.clone()))
                .cloned()
                .collect()
        });
        for (idx, a) in list.into_iter().enumerate() {
            let label = match &a.owner {
                Some(o) => format!("{} ({})", a.name, o.name),
                None => a.name.clone(),
            };
            let target_add = target.clone();
            let mut action = MenuAction::new(label, "image").run(move || {
                add_to_album(a.clone(), target_add.clone());
            });
            if idx == 0 {
                action = action.separator();
            }
            actions.push(action);
        }
        actions
    };

    let album_actions = move |a: PhotoAlbum| -> Vec<MenuAction> {
        if a.role == "owner" {
            let a_share = a.clone();
            let a_rename = a.clone();
            let a_del = a;
            vec![
                MenuAction::new("Share\u{2026}", "share")
                    .run(move || sharing.set(Some(a_share.clone()))),
                MenuAction::new("Rename album", "pencil").run(move || {
                    album_dialog.set(Some(AlbumDialogState {
                        album: Some(a_rename.clone()),
                        ids: None,
                    }));
                }),
                MenuAction::new("Delete album\u{2026}", "trash")
                    .danger()
                    .separator()
                    .run(move || confirm.set(Some(a_del.clone()))),
            ]
        } else {
            vec![
                MenuAction::new("Leave album", "logout")
                    .danger()
                    .run(move || leave_album(a.clone())),
            ]
        }
    };

    let viewer_more = move |asset: PhotoAsset| -> Vec<MenuAction> {
        let cur_view = view.get_untracked();
        let in_trash = cur_view == View::Trash;
        let in_locked = cur_view == View::Locked;
        let is_album = matches!(cur_view, View::Album(_));
        let is_album_owner = album_owner.get_untracked();
        let id = asset.id.clone();

        if has_owner(&asset) {
            let mut list = vec![
                MenuAction::new("Download original", "download")
                    .href(format!("{BASE}/assets/{id}/original"))
                    .download(),
            ];
            if is_album && is_album_owner {
                let id_rem = id.clone();
                list.push(
                    MenuAction::new("Remove from album", "close")
                        .separator()
                        .run(move || remove_from_album(vec![id_rem.clone()])),
                );
            }
            return list;
        }
        if in_trash {
            let id_res = id.clone();
            let id_del = id;
            return vec![
                MenuAction::new("Restore", "undo")
                    .run(move || restore_assets(vec![id_res.clone()])),
                MenuAction::new("Delete permanently\u{2026}", "trash")
                    .danger()
                    .separator()
                    .run(move || {
                        deleting.set(Some(DeletingState {
                            ids: vec![id_del.clone()],
                            empty: false,
                        }));
                    }),
            ];
        }
        if in_locked {
            let id_unlock = id.clone();
            let id_del = id.clone();
            return vec![
                MenuAction::new("Move out of Locked folder", "lock")
                    .run(move || unlock_photos(vec![id_unlock.clone()])),
                MenuAction::new("Download original", "download")
                    .href(format!("/api/v1/files/{id}/content"))
                    .download()
                    .separator(),
                MenuAction::new("Delete permanently\u{2026}", "trash")
                    .danger()
                    .separator()
                    .run(move || {
                        deleting.set(Some(DeletingState {
                            ids: vec![id_del.clone()],
                            empty: false,
                        }));
                    }),
            ];
        }
        let mut list = vec![
            MenuAction::new("Add to album", "plus").items(album_menu_actions(vec![id.clone()])),
            MenuAction::new(
                if asset.archived {
                    "Restore to library"
                } else {
                    "Archive"
                },
                if asset.archived { "undo" } else { "archive" },
            )
            .run({
                let id = id.clone();
                let archived = asset.archived;
                move || {
                    update_assets(
                        vec![id.clone()],
                        None,
                        Some(!archived),
                        Some((None, Some(archived))),
                        false,
                    )
                }
            }),
        ];
        if is_album {
            if is_album_owner && let Some(album_now) = current_album.get_untracked() {
                let id_cov = id.clone();
                list.push(MenuAction::new("Set as album cover", "image").run(move || {
                    set_album_cover(album_now.clone(), id_cov.clone());
                }));
            }
            let id_rem = id.clone();
            list.push(
                MenuAction::new("Remove from album", "close")
                    .run(move || remove_from_album(vec![id_rem.clone()])),
            );
        }
        let id_link = id.clone();
        let id_lock = id.clone();
        let id_trash = id.clone();
        list.push(
            MenuAction::new("Create shared link", "link")
                .run(move || link_for.set(Some(vec![id_link.clone()]))),
        );
        list.push(
            MenuAction::new("Move to Locked folder", "lock")
                .run(move || lock_photos(vec![id_lock.clone()])),
        );
        list.push(
            MenuAction::new("Download original", "download")
                .href(format!("/api/v1/files/{id}/content"))
                .download()
                .separator(),
        );
        list.push(
            MenuAction::new("Move to trash", "trash")
                .danger()
                .separator()
                .shortcut("Delete")
                .run(move || trash_assets(vec![id_trash.clone()])),
        );
        list
    };

    let nav_button = move |(id, label, icon): (&'static str, &'static str, &'static str)| {
        let target_view = View::from_type_str(id).expect("valid list view");
        let target_active = target_view.clone();
        let target_current = target_view.clone();
        let target_click = target_view;
        view! {
            <button
                type="button"
                class=move || if view.get() == target_active { "nav-item active" } else { "nav-item" }
                aria-current=move || (view.get() == target_current).then_some("page")
                on:click=move |_| choose(target_click.clone())
            >
                <Icon name=icon/>
                <span class="photos-row-title">{label}</span>
                {move || {
                    if id == "locked" {
                        return None;
                    }
                    let count = overview.with(|o| o.as_ref().and_then(|o| o.counts.get(id))).unwrap_or(0);
                    (count > 0).then(|| view! { <span class="photos-count">{format_number(count)}</span> })
                }}
            </button>
        }
    };

    let columns = Memo::new(move |_| {
        let z = zoom.get();
        if ZOOM_STEPS.contains(&z) { z } else { 5 }
    });

    #[derive(Clone, PartialEq)]
    struct DayHeading {
        key: String,
        label: String,
        ids: Vec<String>,
    }

    let grid_rows = Memo::new(move |_| {
        let in_trash = view.get() == View::Trash;
        let is_largest = sort.get() == "largest" || view.get() == View::Large;
        items.with(|list| {
            // Collect group IDs per heading first so each day header knows all its tile IDs.
            let group_label_for = |item: &PhotoAsset| -> String {
                if is_largest {
                    return "Largest files".to_owned();
                }
                let ts = if in_trash {
                    item.trashed_at.unwrap_or(item.taken_at)
                } else {
                    item.taken_at
                };
                if in_trash {
                    format!("Deleted {}", month_label(ts))
                } else {
                    day_label(ts)
                }
            };
            let mut group_ids: Vec<(String, Vec<String>)> = Vec::new();
            for item in list {
                let lbl = group_label_for(item);
                if let Some(last) = group_ids.last_mut()
                    && last.0 == lbl
                {
                    last.1.push(item.id.clone());
                } else {
                    group_ids.push((lbl, vec![item.id.clone()]));
                }
            }
            let mut group_idx = 0usize;
            let mut seen_in_group = 0usize;
            list.iter()
                .map(|item| {
                    let heading = if seen_in_group == 0 {
                        let (lbl, ids) = &group_ids[group_idx];
                        Some(DayHeading {
                            key: format!("g-{group_idx}"),
                            label: lbl.clone(),
                            ids: ids.clone(),
                        })
                    } else {
                        None
                    };
                    seen_in_group += 1;
                    if seen_in_group >= group_ids[group_idx].1.len() {
                        group_idx += 1;
                        seen_in_group = 0;
                    }
                    (heading, item.clone())
                })
                .collect::<Vec<_>>()
        })
    });

    let scrubber_ticks = Memo::new(move |_| {
        let rows = grid_rows.get();
        let mut ticks: Vec<(String, String)> = Vec::new();
        for (heading, item) in rows {
            if let Some(h) = heading {
                let m = scrubber_month_label(item.taken_at);
                if ticks.last().is_none_or(|(prev, _)| prev != &m) {
                    ticks.push((m, h.key));
                }
            }
        }
        ticks
    });

    let has_immich =
        Memo::new(move |_| overview.with(|o| o.as_ref().and_then(|o| o.immich.as_ref()).is_some()));
    let immich_progress = Signal::derive(move || {
        overview.with(|o| {
            o.as_ref()
                .and_then(|o| o.immich.clone())
                .unwrap_or(PhotoImmichProgress {
                    phase: String::new(),
                    done: 0,
                    total: 0,
                    imported: 0,
                    file: String::new(),
                    started_at: 0.0,
                })
        })
    });

    let open_info = move || {
        info.set(true);
        if let Some(el) = shell_ref.get_untracked() {
            let _ = el.class_list().add_1("is-info");
        }
        if let Some(el) = document()
            .query_selector(".photos-detail-back")
            .ok()
            .flatten()
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = el.focus();
        }
    };

    let close_info = move || {
        info.set(false);
        if let Some(el) = shell_ref.get_untracked() {
            let _ = el.class_list().remove_1("is-info");
        }
        if let Some(el) = document()
            .query_selector(".photos-info-toggle")
            .ok()
            .flatten()
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = el.focus();
        }
    };

    let header_session = session.get_value();
    view! {
        <div
            node_ref=shell_ref
            class="app-shell pim-shell photos-shell"
            class:has-pim-details=move || has_viewer.get()
            class:is-viewing=move || has_viewer.get()
            class:is-inspector-hidden=move || has_viewer.get() && !desktop_info.get()
            class:is-info=move || has_viewer.get() && info.get()
        >
            <WorkspaceHeader
                app="photos"
                session=header_session
                on_signed_out=on_signed_out
                on_settings=Callback::new(move |_| settings.set(true))
            />
            <div class="pim-workspace photos-workspace">
                <aside class="sidebar photos-sidebar" aria-label="Photos navigation">
                    <button
                        type="button"
                        class="new-button ui-new"
                        on:click=move |_| {
                            if let Some(input) = file_input.get_untracked() {
                                input.click();
                            }
                        }
                    >
                        <Icon name="plus"/>"Upload"
                    </button>
                    <input
                        node_ref=file_input
                        type="file"
                        multiple
                        hidden
                        accept="image/*,video/*,.heic,.heif,.dng,.cr2,.cr3,.nef,.arw,.orf,.rw2,.raf"
                        aria-label="Upload photos and videos"
                        on:change=move |e| {
                            if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                let files: Vec<web_sys::File> = input
                                    .files()
                                    .map(|list| (0..list.length()).filter_map(|i| list.item(i)).collect())
                                    .unwrap_or_default();
                                input.set_value("");
                                upload_files(files);
                            }
                        }
                    />
                    <nav aria-label="Photos views">
                        {MAIN_VIEWS.into_iter().map(nav_button).collect_view()}
                    </nav>
                    <div class="sidebar-heading photos-heading">
                        <h2>"Albums"</h2>
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="New album"
                            title="New album"
                            on:click=move |_| album_dialog.set(Some(AlbumDialogState { album: None, ids: None }))
                        >
                            <Icon name="plus"/>
                        </button>
                    </div>
                    <nav aria-label="Albums" class="photos-albums">
                        <For
                            each=move || overview.with(|o| o.as_ref().map(|o| o.albums.clone()).unwrap_or_default())
                            key=|a| (a.id.clone(), a.name.clone(), a.count, a.cover_id.clone(), a.shared, a.revision)
                            children=move |a| {
                                let id_active = a.id.clone();
                                let id_current = a.id.clone();
                                let id_click = a.id.clone();
                                let menu_label = format!("Actions for {}", a.name);
                                let a_menu = a.clone();
                                let cover_id = a.cover_id.clone();
                                let name = a.name.clone();
                                let is_shared = a.shared;
                                let count = a.count;
                                view! {
                                    <div class="photos-row">
                                        <button
                                            type="button"
                                            class=move || if view.get() == View::Album(id_active.clone()) { "nav-item active" } else { "nav-item" }
                                            aria-current=move || (view.get() == View::Album(id_current.clone())).then_some("page")
                                            on:click=move |_| choose(View::Album(id_click.clone()))
                                        >
                                            {if !cover_id.is_empty() {
                                                view! { <img class="photos-album-cover" src=thumb_url(&cover_id, "s") alt="" loading="lazy"/> }.into_any()
                                            } else {
                                                view! { <span class="photos-album-cover is-empty" aria-hidden="true"><Icon name="image"/></span> }.into_any()
                                            }}
                                            <span class="photos-row-title">{name}</span>
                                            {is_shared.then(|| view! {
                                                <span class="photos-shared-mark" title="Shared"><Icon name="share"/><span class="sr-only">", shared"</span></span>
                                            })}
                                            <span class="photos-count">{format_number(count)}</span>
                                        </button>
                                        <ActionMenu
                                            label=menu_label
                                            class="photos-row-menu"
                                            actions=move |_| album_actions(a_menu.clone())
                                        >
                                            <Icon name="more"/>
                                        </ActionMenu>
                                    </div>
                                }
                            }
                        />
                        {move || overview.with(|o| o.as_ref().is_some_and(|o| o.albums.is_empty())).then(|| view! {
                            <p class="photos-hint">"Albums you create appear here."</p>
                        })}
                    </nav>
                    <div class="sidebar-heading photos-heading"><h2>"More"</h2></div>
                    <nav aria-label="More photos views">
                        {MORE_VIEWS.into_iter().map(nav_button).collect_view()}
                    </nav>
                    {move || overview.with(|o| o.as_ref().is_some_and(|o| o.indexing)).then(|| view! {
                        <p class="photos-indexing" role="status">
                            <span class="photos-spinner" aria-hidden="true"></span>"Finding photos in Files\u{2026}"
                        </p>
                    })}
                    {move || overview.with(|o| {
                        let o = o.as_ref()?;
                        (o.total_bytes > 0.0).then(|| (o.total_bytes, o.counts.library))
                    }).map(|(bytes, count)| view! {
                        <div class="photos-storage-card" aria-label="Library storage">
                            <span class="photos-storage-meta">
                                <strong>{format_bytes(bytes)}</strong>
                                <small>{format_number(count)}" items"</small>
                            </span>
                        </div>
                    })}
                    <Show when=move || has_immich.get()>
                        <button
                            type="button"
                            class="photos-sync-entry"
                            on:click=move |_| settings.set(true)
                            aria-label="Immich import in progress, open Photos settings"
                        >
                            <ImmichProgressBar compact=true progress=immich_progress/>
                        </button>
                    </Show>
                    <button
                        type="button"
                        class="nav-item ui-app-settings-entry"
                        on:click=move |_| settings.set(true)
                    >
                        <Icon name="settings"/>"Photos settings"
                    </button>
                </aside>

                <main
                    class="pim-main photos-main"
                    tabindex="-1"
                    aria-label=move || current_asset.with(|c| c.as_ref().map(|a| a.name.clone())).unwrap_or_else(|| title.get())
                >
                    {
                        #[derive(Clone, Copy, PartialEq, Eq)]
                        enum MainPane {
                            Duplicates,
                            Shared,
                            Albums,
                            Explore,
                            Folders,
                            Viewer,
                            Missing,
                            Grid,
                        }
                        let main_pane = Memo::new(move |_| {
                            let cur_view = view.get();
                            if cur_view == View::Duplicates {
                                MainPane::Duplicates
                            } else if cur_view == View::Shared {
                                MainPane::Shared
                            } else if cur_view == View::Albums {
                                MainPane::Albums
                            } else if cur_view == View::Explore {
                                MainPane::Explore
                            } else if cur_view == View::Folders {
                                MainPane::Folders
                            } else if has_viewer.get() {
                                MainPane::Viewer
                            } else if open.with(Option::is_some) && state.get() == ListState::Ready {
                                MainPane::Missing
                            } else {
                                MainPane::Grid
                            }
                        });
                        move || {
                            match main_pane.get() {
                                MainPane::Albums => {
                                    return view! {
                                        <AlbumsHub
                                            overview=overview.into()
                                            on_open_album=Callback::new(move |id| choose(View::Album(id)))
                                            on_new_album=Callback::new(move |_| {
                                                album_dialog.set(Some(AlbumDialogState { album: None, ids: None }));
                                            })
                                            album_actions=Callback::new(move |a: PhotoAlbum| album_actions(a))
                                        />
                                    }.into_any();
                                }
                                MainPane::Explore => {
                                    return view! {
                                        <ExploreView
                                            on_open_recent=Callback::new(move |_| choose(View::Recent))
                                            on_open_large=Callback::new(move |_| choose(View::Large))
                                            on_open_unassigned=Callback::new(move |_| choose(View::Unassigned))
                                            on_open_folders=Callback::new(move |_| choose(View::Folders))
                                            on_open_camera=Callback::new(move |cam| choose(View::Camera(cam)))
                                            on_open_place=Callback::new(move |(key, lbl)| choose(View::Place(key, lbl)))
                                            on_open_folder=Callback::new(move |(id, path)| choose(View::FolderId(id, path)))
                                        />
                                    }.into_any();
                                }
                                MainPane::Folders => {
                                    return view! {
                                        <FoldersView
                                            on_open_folder=Callback::new(move |(id, path)| choose(View::FolderId(id, path)))
                                        />
                                    }.into_any();
                                }
                                MainPane::Duplicates => {
                                    let sess = session.get_value();
                                    return view! {
                                        <DuplicatesView
                                            session=sess
                                            on_notice=Callback::new(move |n| show_notice(n))
                                            on_changed=Callback::new(move |_| load_overview())
                                        />
                                    }
                                    .into_any();
                                }
                                MainPane::Shared => {
                            return view! {
                                <div class="photos-list-heading">
                                    <h1>"Shared with me"</h1>
                                    {move || overview.with(|o| o.as_ref().map(|o| o.shared_albums.len())).map(|n| view! {
                                        <span class="photos-heading-count">{n}</span>
                                    })}
                                </div>
                                <div class="photos-grid-scroll">
                                    {move || {
                                        let shared = overview.with(|o| o.as_ref().map(|o| o.shared_albums.clone()));
                                        match shared {
                                            Some(list) if list.is_empty() => view! {
                                                <div class="photos-empty">
                                                    <Icon name="share"/>
                                                    <h2>"Nothing shared with you"</h2>
                                                    <p>"Albums other people on this Dar share with you appear here."</p>
                                                </div>
                                            }.into_any(),
                                            Some(list) => view! {
                                                <ul class="photos-album-cards" aria-label="Albums shared with you">
                                                    {list.into_iter().map(|a| {
                                                        let id = a.id.clone();
                                                        let owner = a.owner.as_ref().map(|o| o.name.clone()).unwrap_or_default();
                                                        let role_label = if a.role == "editor" { "Can add" } else { "Can view" };
                                                        let meta = format!(
                                                            "{owner} \u{00b7} {} item{} \u{00b7} {role_label}",
                                                            format_number(a.count),
                                                            if a.count == 1 { "" } else { "s" }
                                                        );
                                                        view! {
                                                            <li>
                                                                <button
                                                                    type="button"
                                                                    class="photos-album-card"
                                                                    on:click=move |_| choose(View::Album(id.clone()))
                                                                >
                                                                    <span class="photos-album-card-cover">
                                                                        {if !a.cover_id.is_empty() {
                                                                            view! { <img src=thumb_url(&a.cover_id, "s") alt="" loading="lazy"/> }.into_any()
                                                                        } else {
                                                                            view! { <Icon name="image"/> }.into_any()
                                                                        }}
                                                                    </span>
                                                                    <strong>{a.name}</strong>
                                                                    <small>{meta}</small>
                                                                </button>
                                                            </li>
                                                        }
                                                    }).collect_view()}
                                                </ul>
                                            }.into_any(),
                                            None => view! { <span></span> }.into_any(),
                                        }
                                    }}
                                </div>
                            }
                                    .into_any()
                                }
                                MainPane::Viewer => {
                                    // Closing the viewer clears the asset before this view unmounts; keep showing the last one.
                                    let asset_sig = Signal::from(Memo::new(move |last: Option<&PhotoAsset>| current_asset.get().or_else(|| last.cloned()).expect("viewer asset")));
                                    let shown_sig = Signal::from(Memo::new(move |last: Option<&PhotoAsset>| shown_asset.get().or_else(|| last.cloned()).expect("shown asset")));
                                    let index_sig = Signal::derive(move || current_index.get().unwrap_or(0));
                                    let trash_sig = Signal::derive(move || {
                                        view.get() == View::Trash || shown_asset.with(|s| s.as_ref().is_some_and(has_owner))
                                    });
                                    let sess = session.get_value();
                                    view! {
                                        <Viewer
                                            asset=asset_sig
                                            shown=shown_sig
                                            items=items.into()
                                            index=index_sig
                                            session=sess
                                            trash=trash_sig
                                            more=Callback::new(move |a: PhotoAsset| viewer_more(a))
                                            on_member=Callback::new(move |m: Option<PhotoAsset>| member.set(m))
                                            on_unstack=Callback::new(move |s: PhotoStack| unstack_photos(s))
                                            on_cover=Callback::new(move |s: PhotoStack| {
                                                let Some(shown_now) = shown_asset.get_untracked() else { return };
                                                let shown_id = shown_now.id;
                                                let stack_id = s.id;
                                                run_guarded(folder, unlock_action, notice, "change the cover", move || {
                                                    let stack_id = stack_id.clone();
                                                    let shown_id = shown_id.clone();
                                                    let csrf_tok = csrf();
                                                    async move {
                                                        api::send::<Value>(
                                                            &format!("{BASE}/stacks/{stack_id}"),
                                                            "PATCH",
                                                            Some(json!({ "primaryId": shown_id })),
                                                            &csrf_tok,
                                                        )
                                                        .await?;
                                                        member.try_set(None);
                                                        refresh();
                                                        open.try_set(Some(shown_id));
                                                        show_notice(PhotosNotice {
                                                            message: "Stack cover changed".to_owned(),
                                                            undo: None,
                                                        });
                                                        Ok(())
                                                    }
                                                });
                                            })
                                            on_info=Callback::new(move |_| open_info())
                                            on_toggle_inspector=Callback::new(move |_| desktop_info.update(|v| *v = !*v))
                                            inspector_open=desktop_info.into()
                                            slideshow=slideshow
                                            on_share_asset=Callback::new(move |id: String| link_for.set(Some(vec![id])))
                                            album_menu=Callback::new(move |id: String| album_menu_actions(vec![id]))
                                            on_edited=Callback::new(move |new_id: String| {
                                                refresh();
                                                open.set(Some(new_id));
                                                show_notice(PhotosNotice {
                                                    message: "Saved edited copy".to_owned(),
                                                    undo: None,
                                                });
                                            })
                                            on_error=Callback::new(move |msg: String| fail(msg))
                                            on_close=Callback::new(move |_| close_viewer())
                                            on_step=Callback::new(move |d| step(d))
                                            on_open=Callback::new(move |id| open.set(Some(id)))
                                            on_favorite=Callback::new(move |_| {
                                                let Some(shown_now) = shown_asset.get_untracked() else { return };
                                                let next_fav = !shown_now.favorite;
                                                update_assets(vec![shown_now.id], Some(next_fav), None, None, false);
                                                member.update(|m| {
                                                    if let Some(m) = m {
                                                        m.favorite = next_fav;
                                                    }
                                                });
                                            })
                                        />
                                    }
                                    .into_any()
                                }
                                MainPane::Missing => {
                                    view! {
                                        <div class="photos-empty" role="alert">
                                            <h2>"This photo is not available"</h2>
                                            <p>"It may have been moved to the trash, archived, locked or removed from this view."</p>
                                            <button class="secondary-button" on:click=move |_| open.set(None)>
                                                "Back to "{move || title.get()}
                                            </button>
                                        </div>
                                    }
                                    .into_any()
                                }
                                MainPane::Grid => {
                        // Grid header + filter bar + album hero + memories + day-grouped grid + scrubber.
                        view! {
                            {move || {
                                let ids: Vec<String> = selected.with(|s| s.iter().cloned().collect());
                                let cur_view = view.get();
                                let in_trash = cur_view == View::Trash;
                                let in_locked = cur_view == View::Locked;
                                let is_album = matches!(cur_view, View::Album(_));
                                let is_album_owner = album_owner.get();
                                if !ids.is_empty() {
                                    let count = ids.len();
                                    let foreign = items.with(|list| {
                                        ids.iter().any(|id| list.iter().find(|i| &i.id == id).is_some_and(has_owner))
                                    });
                                    let all_fav = items.with(|list| {
                                        ids.iter().all(|id| list.iter().find(|i| &i.id == id).is_some_and(|i| i.favorite))
                                    });
                                    let is_archive = cur_view == View::Archive;
                                    let tools = if in_trash {
                                        let ids_res = ids.clone();
                                        let ids_del = ids;
                                        view! {
                                            <button type="button" class="text-button" on:click=move |_| restore_assets(ids_res.clone())>
                                                <Icon name="undo"/>"Restore"
                                            </button>
                                            <button type="button" class="text-button danger" on:click=move |_| deleting.set(Some(DeletingState { ids: ids_del.clone(), empty: false }))>
                                                <Icon name="trash"/>"Delete permanently"
                                            </button>
                                        }.into_any()
                                    } else if in_locked {
                                        let ids_unlock = ids.clone();
                                        let ids_del = ids;
                                        view! {
                                            <button type="button" class="text-button" on:click=move |_| unlock_photos(ids_unlock.clone())>
                                                <Icon name="lock"/>"Move out"
                                            </button>
                                            <button type="button" class="text-button danger" on:click=move |_| deleting.set(Some(DeletingState { ids: ids_del.clone(), empty: false }))>
                                                <Icon name="trash"/>"Delete permanently"
                                            </button>
                                        }.into_any()
                                    } else if foreign {
                                        let ids_rem = ids;
                                        view! {
                                            {(is_album && is_album_owner).then(|| view! {
                                                <button type="button" class="text-button" on:click=move |_| remove_from_album(ids_rem.clone())>
                                                    <Icon name="close"/>"Remove from album"
                                                </button>
                                            })}
                                            {(!is_album_owner).then(|| view! {
                                                <span class="photos-hint">"Only the owner of these photos can change them."</span>
                                            })}
                                        }.into_any()
                                    } else {
                                        let ids_fav = ids.clone();
                                        let ids_menu = ids.clone();
                                        let ids_rem = ids.clone();
                                        let ids_zip = ids.clone();
                                        let ids_link = ids.clone();
                                        let ids_stack = ids.clone();
                                        let ids_arch = ids.clone();
                                        let ids_lock = ids.clone();
                                        let ids_trash = ids;
                                        let fav_label = if all_fav { "Remove from Favorites" } else { "Add to Favorites" };
                                        let arch_label = if is_archive { "Restore to library" } else { "Archive" };
                                        let arch_title = if is_archive {
                                            "Restore to library"
                                        } else {
                                            "Archive (hidden from the library, kept in albums and Files)"
                                        };
                                        let arch_icon = if is_archive { "undo" } else { "archive" };
                                        view! {
                                            <button
                                                type="button"
                                                class="icon-button"
                                                aria-label=fav_label
                                                title=fav_label
                                                aria-pressed=all_fav.to_string()
                                                on:click=move |_| update_assets(ids_fav.clone(), Some(!all_fav), None, Some((Some(all_fav), None)), false)
                                            >
                                                <Icon name="star"/>
                                            </button>
                                            <ActionMenu label="Add to album" actions=move |_| album_menu_actions(ids_menu.clone())>
                                                <Icon name="plus"/>
                                            </ActionMenu>
                                            {is_album.then(|| view! {
                                                <button
                                                    type="button"
                                                    class="icon-button"
                                                    aria-label="Remove from album"
                                                    title="Remove from album"
                                                    on:click=move |_| remove_from_album(ids_rem.clone())
                                                >
                                                    <Icon name="close"/>
                                                </button>
                                            })}
                                            <button
                                                type="button"
                                                class="icon-button"
                                                aria-label="Download ZIP"
                                                title="Download selected as ZIP"
                                                on:click=move |_| download_zip(ids_zip.clone(), "dar-photos.zip".to_owned())
                                            >
                                                <Icon name="download"/>
                                            </button>
                                            <button
                                                type="button"
                                                class="icon-button"
                                                aria-label="Create shared link"
                                                title="Create shared link"
                                                on:click=move |_| link_for.set(Some(ids_link.clone()))
                                            >
                                                <Icon name="link"/>
                                            </button>
                                            {(count > 1).then(|| view! {
                                                <button
                                                    type="button"
                                                    class="icon-button"
                                                    aria-label="Stack"
                                                    title="Stack (show one cover for these photos)"
                                                    on:click=move |_| stack_photos(ids_stack.clone())
                                                >
                                                    <Icon name="stack"/>
                                                </button>
                                            })}
                                            <button
                                                type="button"
                                                class="icon-button"
                                                aria-label=arch_label
                                                title=arch_title
                                                on:click=move |_| update_assets(ids_arch.clone(), None, Some(!is_archive), Some((None, Some(is_archive))), false)
                                            >
                                                <Icon name=arch_icon/>
                                            </button>
                                            <button
                                                type="button"
                                                class="icon-button"
                                                aria-label="Move to Locked folder"
                                                title="Move to Locked folder"
                                                on:click=move |_| lock_photos(ids_lock.clone())
                                            >
                                                <Icon name="lock"/>
                                            </button>
                                            <button
                                                type="button"
                                                class="icon-button"
                                                aria-label="Move to trash"
                                                title="Move to trash"
                                                on:click=move |_| trash_assets(ids_trash.clone())
                                            >
                                                <Icon name="trash"/>
                                            </button>
                                        }.into_any()
                                    };
                                    view! {
                                        <div class="photos-selection" role="toolbar" aria-label="Selection">
                                            <button
                                                type="button"
                                                class="icon-button"
                                                aria-label="Clear selection"
                                                on:click=move |_| {
                                                    selected.set(HashSet::new());
                                                    mobile_select.set(false);
                                                }
                                            >
                                                <Icon name="close"/>
                                            </button>
                                            <strong>{count}" selected"</strong>
                                            <span class="photos-spacer"></span>
                                            {tools}
                                        </div>
                                    }.into_any()
                                } else {
                                    let count_val = if filter.get() != "all" {
                                        None
                                    } else {
                                        match &cur_view {
                                            View::Album(_) => current_album.with(|a| a.as_ref().map(|a| a.count)),
                                            View::Locked | View::Duplicates => None,
                                            v => overview.with(|o| o.as_ref().and_then(|o| o.counts.get(v.as_type_str()))),
                                        }
                                    };
                                    let shown_count = count_val.unwrap_or_else(|| items.with(Vec::len) as u64);
                                    let album_now = current_album.get();
                                    let show_back = cur_view.is_sub_collection();
                                    let back_target = match &cur_view {
                                        View::FolderId(_, _) => View::Folders,
                                        View::Memory(_, _, _) => View::Folder,
                                        _ => View::Explore,
                                    };
                                    let back_label = match &cur_view {
                                        View::FolderId(_, _) => "Back to Folders",
                                        View::Memory(_, _, _) => "Back to Photos",
                                        _ => "Back to Explore",
                                    };
                                    view! {
                                        <div class="photos-list-heading">
                                            {show_back.then(|| view! {
                                                <button
                                                    type="button"
                                                    class="icon-button"
                                                    aria-label=back_label
                                                    on:click=move |_| choose(back_target.clone())
                                                >
                                                    <Icon name="back"/>
                                                </button>
                                            })}
                                            <h1>{move || title.get()}</h1>
                                            {(state.get() == ListState::Ready && !locked_closed.get()).then(|| view! {
                                                <span class="photos-heading-count">{format_number(shown_count)}</span>
                                            })}
                                            {album_now.as_ref().and_then(|a| a.owner.as_ref().map(|o| (o.name.clone(), a.role.clone()))).map(|(owner_name, role)| view! {
                                                <span class="photos-shared-by">
                                                    "Shared by "{owner_name}" \u{00b7} "{if role == "editor" { "you can add photos" } else { "view only" }}
                                                </span>
                                            })}
                                            <div class="photos-tools">
                                                {(in_trash && !items.with(Vec::is_empty)).then(|| view! {
                                                    <button
                                                        type="button"
                                                        class="text-button danger"
                                                        on:click=move |_| deleting.set(Some(DeletingState { ids: Vec::new(), empty: true }))
                                                    >
                                                        "Empty trash"
                                                    </button>
                                                })}
                                                {(in_locked && folder.unlocked()).then(|| view! {
                                                    <button
                                                        type="button"
                                                        class="text-button"
                                                        on:click=move |_| {
                                                            spawn_local(async move {
                                                                if let Err(cause) = folder.lock().await {
                                                                    fail(cause.message);
                                                                }
                                                            });
                                                        }
                                                    >
                                                        <Icon name="lock"/>"Lock"
                                                    </button>
                                                })}
                                                {(!locked_closed.get() && !items.with(Vec::is_empty)).then(|| view! {
                                                    <button
                                                        type="button"
                                                        class="text-button photos-mobile-select-btn"
                                                        aria-pressed=move || mobile_select.get().to_string()
                                                        on:click=move |_| mobile_select.update(|v| *v = !*v)
                                                    >
                                                        {move || if mobile_select.get() { "Done" } else { "Select" }}
                                                    </button>
                                                })}
                                                <div class="photos-layout-toggle" role="group" aria-label="Grid layout">
                                                    <button
                                                        type="button"
                                                        class="icon-button"
                                                        class:is-active=move || layout.get() == "justified"
                                                        aria-label="Justified layout"
                                                        aria-pressed=move || (layout.get() == "justified").to_string()
                                                        title="Justified aspect-ratio rows"
                                                        on:click=move |_| layout.set("justified")
                                                    >
                                                        <Icon name="justified"/>
                                                    </button>
                                                    <button
                                                        type="button"
                                                        class="icon-button"
                                                        class:is-active=move || layout.get() == "square"
                                                        aria-label="Square grid"
                                                        aria-pressed=move || (layout.get() == "square").to_string()
                                                        title="Square grid"
                                                        on:click=move |_| layout.set("square")
                                                    >
                                                        <Icon name="grid"/>
                                                    </button>
                                                </div>
                                                <label class="photos-zoom">
                                                    <Icon name="grid"/>
                                                    <span class="sr-only">"Columns"</span>
                                                    <input
                                                        type="range"
                                                        min="0"
                                                        max=(ZOOM_STEPS.len() - 1).to_string()
                                                        step="1"
                                                        prop:value=move || {
                                                            let c = columns.get();
                                                            ZOOM_STEPS.iter().position(|&x| x == c).unwrap_or(2).to_string()
                                                        }
                                                        aria-valuetext=move || format!("{} columns", columns.get())
                                                        on:input=move |e| {
                                                            if let Ok(idx) = event_target_value(&e).parse::<usize>()
                                                                && let Some(&step_val) = ZOOM_STEPS.get(idx)
                                                            {
                                                                zoom.set(step_val);
                                                            }
                                                        }
                                                    />
                                                </label>
                                                {album_now.map(|a| {
                                                    let label = format!("Actions for {}", a.name);
                                                    view! {
                                                        <ActionMenu label=label actions=move |_| album_actions(a.clone())>
                                                            <Icon name="more"/>
                                                        </ActionMenu>
                                                    }
                                                })}
                                            </div>
                                        </div>
                                    }.into_any()
                                }
                            }}

                            {move || {
                                let cur_view = view.get();
                                if !cur_view.supports_filters() || locked_closed.get() || !selected.with(HashSet::is_empty) {
                                    return None;
                                }
                                let is_album = matches!(cur_view, View::Album(_));
                                let chips: Vec<(&'static str, &'static str)> = if cur_view == View::Videos {
                                    vec![("all", "All"), ("favorites", "Starred")]
                                } else if is_album {
                                    vec![("all", "All"), ("images", "Stills"), ("videos", "Clips"), ("favorites", "Starred")]
                                } else {
                                    vec![
                                        ("all", "All"),
                                        ("images", "Stills"),
                                        ("videos", "Clips"),
                                        ("live", "Live"),
                                        ("raw", "RAW"),
                                        ("favorites", "Starred"),
                                        ("unassigned", "Unfiled"),
                                    ]
                                };
                                Some(view! {
                                    <div class="photos-subbar" role="region" aria-label="Filter and sort">
                                        <div class="photos-filter-chips" role="group" aria-label="Filter items">
                                            {chips.into_iter().map(|(val, lbl)| {
                                                view! {
                                                    <button
                                                        type="button"
                                                        class="photos-filter-chip"
                                                        class:is-active=move || filter.get() == val
                                                        aria-pressed=move || (filter.get() == val).to_string()
                                                        on:click=move |_| filter.set(val)
                                                    >
                                                        {lbl}
                                                    </button>
                                                }
                                            }).collect_view()}
                                        </div>
                                        <div class="photos-sort-wrap">
                                            <Icon name="sort"/>
                                            <label class="sr-only" for="photos-grid-sort">"Sort photos"</label>
                                            <select
                                                id="photos-grid-sort"
                                                class="photos-sort-select"
                                                prop:value=move || sort.get()
                                                on:change=move |e| {
                                                    let v = event_target_value(&e);
                                                    sort.set(match v.as_str() {
                                                        "oldest" => "oldest",
                                                        "largest" => "largest",
                                                        _ => "newest",
                                                    });
                                                }
                                            >
                                                <option value="newest">"Newest first"</option>
                                                <option value="oldest">"Oldest first"</option>
                                                <option value="largest">"Largest first"</option>
                                            </select>
                                        </div>
                                    </div>
                                })
                            }}

                            <div class="photos-grid-frame">
                            <div
                                node_ref=grid
                                class="photos-grid-scroll"
                                class:is-dropping=move || dragging.get()
                                on:dragover=move |e: web_sys::DragEvent| {
                                    let has_files = e
                                        .data_transfer()
                                        .is_some_and(|dt| dt.types().iter().any(|v| v.as_string().as_deref() == Some("Files")));
                                    if has_files {
                                        e.prevent_default();
                                        dragging.set(true);
                                    }
                                }
                                on:dragleave=move |e: web_sys::DragEvent| {
                                    if e.current_target() == e.target() {
                                        dragging.set(false);
                                    }
                                }
                                on:drop=move |e: web_sys::DragEvent| {
                                    e.prevent_default();
                                    dragging.set(false);
                                    let files: Vec<web_sys::File> = e
                                        .data_transfer()
                                        .and_then(|dt| dt.files())
                                        .map(|list| (0..list.length()).filter_map(|i| list.item(i)).collect())
                                        .unwrap_or_default();
                                    upload_files(files);
                                }
                            >
                                {move || {
                                    let a = current_album.get()?;
                                    let can_add = a.role == "owner" || a.role == "editor";
                                    let is_owner = a.role == "owner";
                                    let span = album_date_span(&a);
                                    let desc_draft = RwSignal::new(a.description.clone());
                                    let a_desc = a.clone();
                                    let a_add = a.clone();
                                    let a_share = a.clone();
                                    let a_zip_name = format!("{}.zip", a.name.replace('/', "-"));
                                    let has_items = items.with(|l| !l.is_empty());
                                    Some(view! {
                                        <section class="photos-album-hero" aria-label="Album overview">
                                            {(!a.cover_id.is_empty()).then(|| view! {
                                                <img class="photos-album-hero-bg" src=thumb_url(&a.cover_id, "m") alt="" aria-hidden="true"/>
                                            })}
                                            <div class="photos-album-hero-scrim">
                                                <div class="photos-album-hero-copy">
                                                    <div class="photos-album-hero-meta">
                                                        {(!span.is_empty()).then(|| view! { <span>{span}</span> })}
                                                        <span>{format_number(a.count)}" item"{if a.count == 1 { "" } else { "s" }}</span>
                                                        {(a.shared || a.owner.is_some()).then(|| view! {
                                                            <span class="photos-album-hero-pill">
                                                                <Icon name="share"/>
                                                                {if let Some(o) = &a.owner {
                                                                    format!("Shared by {}", o.name)
                                                                } else if a.member_count > 0 {
                                                                    format!("Shared with {}", a.member_count)
                                                                } else {
                                                                    "Shared".to_owned()
                                                                }}
                                                            </span>
                                                        })}
                                                    </div>
                                                    {if is_owner {
                                                        view! {
                                                            <input
                                                                type="text"
                                                                class="photos-album-hero-desc-input"
                                                                placeholder="Add a description for this album\u{2026}"
                                                                aria-label="Album description"
                                                                maxlength="2000"
                                                                prop:value=move || desc_draft.get()
                                                                on:input=move |e| desc_draft.set(event_target_value(&e))
                                                                on:blur=move |_| save_album_description(a_desc.clone(), desc_draft.get_untracked())
                                                            />
                                                        }.into_any()
                                                    } else if !a.description.is_empty() {
                                                        view! { <p class="photos-album-hero-desc">{a.description.clone()}</p> }.into_any()
                                                    } else {
                                                        view! { <span></span> }.into_any()
                                                    }}
                                                </div>
                                                <div class="photos-album-hero-actions">
                                                    {can_add.then(|| view! {
                                                        <button
                                                            type="button"
                                                            class="primary-button"
                                                            on:click=move |_| picker_album.set(Some(a_add.clone()))
                                                        >
                                                            <Icon name="plus"/>"Add photos"
                                                        </button>
                                                    })}
                                                    {has_items.then(|| view! {
                                                        <button
                                                            type="button"
                                                            class="secondary-button"
                                                            on:click=move |_| {
                                                                if let Some(first) = items.with_untracked(|l| l.first().map(|x| x.id.clone())) {
                                                                    open.set(Some(first));
                                                                    slideshow.set(true);
                                                                }
                                                            }
                                                        >
                                                            <Icon name="play"/>"Slideshow"
                                                        </button>
                                                    })}
                                                    {(has_items && is_owner).then(|| {
                                                        let zip_name = a_zip_name.clone();
                                                        view! {
                                                            <button
                                                                type="button"
                                                                class="secondary-button"
                                                                on:click=move |_| {
                                                                    let ids: Vec<String> = items.with_untracked(|l| l.iter().filter(|x| !has_owner(x)).map(|x| x.id.clone()).collect());
                                                                    download_zip(ids, zip_name.clone());
                                                                }
                                                            >
                                                                <Icon name="download"/>"Download ZIP"
                                                            </button>
                                                        }
                                                    })}
                                                    {is_owner.then(|| view! {
                                                        <button
                                                            type="button"
                                                            class="secondary-button"
                                                            on:click=move |_| sharing.set(Some(a_share.clone()))
                                                        >
                                                            <Icon name="share"/>"Share"
                                                        </button>
                                                    })}
                                                </div>
                                            </div>
                                        </section>
                                    })
                                }}

                                {move || {
                                    let cur_view = view.get();
                                    if !matches!(cur_view, View::Folder | View::Library) || filter.get() != "all" {
                                        return None;
                                    }
                                    let mems = overview.with(|o| o.as_ref().map(|o| o.memories.clone()).unwrap_or_default());
                                    if mems.is_empty() {
                                        return None;
                                    }
                                    let hidden = hide_memories.get();
                                    Some(view! {
                                        <section class="photos-memories" aria-label="On this day">
                                            <div class="photos-memories-head">
                                                <strong>"On this day"</strong>
                                                <button
                                                    type="button"
                                                    class="text-button"
                                                    on:click=move |_| hide_memories.update(|v| *v = !*v)
                                                >
                                                    {if hidden { "Show" } else { "Hide" }}
                                                </button>
                                            </div>
                                            {(!hidden).then(|| view! {
                                                <div class="photos-memories-strip">
                                                    {mems.into_iter().map(|m| {
                                                        let label = if m.years_ago == 1 {
                                                            "1 year ago".to_owned()
                                                        } else {
                                                            format!("{} years ago", m.years_ago)
                                                        };
                                                        let sub = format!("{} \u{00b7} {} photo{}", m.year, format_number(m.count), if m.count == 1 { "" } else { "s" });
                                                        view! {
                                                            <button
                                                                type="button"
                                                                class="photos-memory-card"
                                                                on:click=move |_| choose(View::Memory(m.year, m.month, m.day))
                                                            >
                                                                <img src=thumb_url(&m.cover_id, "s") alt="" loading="lazy"/>
                                                                <span class="photos-memory-scrim">
                                                                    <strong>{label}</strong>
                                                                    <small>{sub}</small>
                                                                </span>
                                                            </button>
                                                        }
                                                    }).collect_view()}
                                                </div>
                                            })}
                                        </section>
                                    })
                                }}

                                {
                                    let has_grid_error = Memo::new(move |_| !overview_error.with(String::is_empty) || state.get() == ListState::Error);
                                    move || has_grid_error.get().then(|| view! {
                                        <div class="photos-empty" role="alert">
                                            <h2>"Photos could not be loaded"</h2>
                                            <p>{move || {
                                                let le = list_error.get();
                                                if !le.is_empty() { le } else { overview_error.get() }
                                            }}</p>
                                            <button class="secondary-button" on:click=move |_| refresh()>"Retry"</button>
                                        </div>
                                    })
                                }
                                {move || if locked_closed.get() {
                                    if folder.state.with(Option::is_some) {
                                        view! {
                                            <UnlockForm
                                                folder=folder
                                                description="Enter your password to see your locked photos. They are also in Files \u{203a} Locked folder, which opens with the same password."
                                                on_unlocked=Callback::new(move |_| {})
                                            />
                                        }.into_any()
                                    } else {
                                        view! {
                                            <p class="photos-empty" role="status">
                                                {move || {
                                                    let err = folder.error.get();
                                                    if !err.is_empty() { err } else { "Checking the Locked folder\u{2026}".to_owned() }
                                                }}
                                            </p>
                                        }.into_any()
                                    }
                                } else {
                                    let grid_loading_empty = Memo::new(move |_| state.get() == ListState::Loading && items.with(Vec::is_empty));
                                    let grid_ready_empty = Memo::new(move |_| state.get() == ListState::Ready && items.with(Vec::is_empty));
                                    let grid_has_more = Memo::new(move |_| !cursor.with(String::is_empty) && state.get() == ListState::Ready);
                                    view! {
                                        {move || grid_loading_empty.get().then(|| view! {
                                            <p class="photos-empty" role="status">"Loading photos\u{2026}"</p>
                                        })}
                                        {move || grid_ready_empty.get().then(|| {
                                            if filter.get() != "all" {
                                                return view! {
                                                    <div class="photos-empty">
                                                        <Icon name="image"/>
                                                        <h2>"No matching photos"</h2>
                                                        <p>"Try clearing the active filter to see everything in this view."</p>
                                                        <button type="button" class="secondary-button" on:click=move |_| filter.set("all")>"Show all"</button>
                                                    </div>
                                                };
                                            }
                                            let is_indexing = overview.with(|o| o.as_ref().is_some_and(|o| o.indexing));
                                            let upload_folder = overview
                                                .with(|o| o.as_ref().map(|o| o.upload_folder.clone()).filter(|s| !s.is_empty()))
                                                .unwrap_or_else(|| "pics".to_owned());
                                            let trigger_upload = move |_| {
                                                if let Some(input) = file_input.get_untracked() {
                                                    input.click();
                                                }
                                            };
                                            let body = match view.get() {
                                                View::Folder => view! {
                                                    <Icon name="image"/>
                                                    <h2>{if is_indexing { "Looking for photos\u{2026}" } else { "No photos yet" }}</h2>
                                                    <p>"Upload photos and videos, or drop them here. They go to Files \u{203a} "{upload_folder}". All photos also shows the pictures and videos elsewhere in your Files."</p>
                                                    <button class="primary-button" on:click=trigger_upload><Icon name="upload"/>"Upload"</button>
                                                }.into_any(),
                                                View::Library => view! {
                                                    <Icon name="image"/>
                                                    <h2>{if is_indexing { "Looking for photos\u{2026}" } else { "No photos yet" }}</h2>
                                                    <p>"Upload photos and videos, or drop them here. Photos also shows the pictures and videos already in your Files."</p>
                                                    <button class="primary-button" on:click=trigger_upload><Icon name="upload"/>"Upload"</button>
                                                }.into_any(),
                                                View::Favorites => view! {
                                                    <h2>"No favorites"</h2>
                                                    <p>"Star a photo to keep it here."</p>
                                                }.into_any(),
                                                View::Archive => view! {
                                                    <h2>"Archive is empty"</h2>
                                                    <p>"Archived photos are hidden from the library but stay in albums and Files."</p>
                                                }.into_any(),
                                                View::Videos => view! {
                                                    <h2>"No videos"</h2>
                                                    <p>"Videos you upload or keep in Files appear here."</p>
                                                }.into_any(),
                                                View::Trash => view! {
                                                    <Icon name="trash"/>
                                                    <h2>"Trash is empty"</h2>
                                                    <p>"Photos you delete stay here, and in Files \u{203a} Deleted files, until you delete them permanently."</p>
                                                }.into_any(),
                                                View::Locked => view! {
                                                    <Icon name="lock"/>
                                                    <h2>"Nothing locked yet"</h2>
                                                    <p>"Select photos and choose Move to Locked folder. They move to Files \u{203a} Locked folder and disappear from the library, albums and search."</p>
                                                }.into_any(),
                                                View::Large => view! {
                                                    <h2>"No large files"</h2>
                                                    <p>"Photos and videos over 5 MB appear here."</p>
                                                }.into_any(),
                                                View::Unassigned => view! {
                                                    <h2>"Everything is in an album"</h2>
                                                    <p>"Photos that are not in any album appear here."</p>
                                                }.into_any(),
                                                View::Album(_) => view! {
                                                    <h2>"This album is empty"</h2>
                                                    <p>"Use Add photos above, or select photos in your library and choose Add to album."</p>
                                                }.into_any(),
                                                _ => view! {
                                                    <h2>"No photos in this collection"</h2>
                                                    <p>"Items matching this view appear here."</p>
                                                }.into_any(),
                                            };
                                            view! { <div class="photos-empty">{body}</div> }
                                        })}
                                        <div
                                            class="photos-grid"
                                            class:is-justified=move || layout.get() == "justified"
                                            class:is-selecting=move || mobile_select.get() || selected.with(|s| !s.is_empty() && items.with(|list| list.iter().any(|i| s.contains(&i.id))))
                                            role="list"
                                            aria-label=move || {
                                                if view.get() == View::Folder {
                                                    "Photos".to_owned()
                                                } else {
                                                    format!("{} photos", title.get())
                                                }
                                            }
                                            data-columns=move || columns.get().to_string()
                                        >
                                            <For
                                                each=move || grid_rows.get()
                                                key=|(heading, item)| (heading.as_ref().map(|h| h.key.clone()), item.id.clone(), item.revision, item.favorite, item.stack.clone())
                                                children=move |(heading, item)| {
                                                    let can_fav_tile = view.get_untracked() != View::Trash && !has_owner(&item);
                                                    view! {
                                                        {heading.map(|h| {
                                                            let ids_check = h.ids.clone();
                                                            let ids_click = h.ids.clone();
                                                            let day_count = h.ids.len();
                                                            let btn_label = format!("Select all photos from {}", h.label);
                                                            view! {
                                                                <div class="photos-day-head" data-day=h.key>
                                                                    <button
                                                                        type="button"
                                                                        class="photos-day-check"
                                                                        class:is-all=move || selected.with(|s| !ids_check.is_empty() && ids_check.iter().all(|id| s.contains(id)))
                                                                        aria-label=btn_label
                                                                        on:click=move |_| toggle_day_selected(ids_click.clone())
                                                                    >
                                                                        <Icon name="check"/>
                                                                    </button>
                                                                    <h2 class="photos-month">{h.label}</h2>
                                                                    <span class="photos-day-count">{day_count}</span>
                                                                </div>
                                                            }
                                                        })}
                                                        <PhotoTile
                                                            item=item
                                                            selected=selected
                                                            can_fav=can_fav_tile
                                                            on_open=Callback::new(move |id| open.set(Some(id)))
                                                            on_toggle=Callback::new(move |(id, shift)| toggle_selected(id, shift))
                                                            on_favorite=Callback::new(move |(id, next_fav)| {
                                                                update_assets(vec![id], Some(next_fav), None, None, false);
                                                            })
                                                        />
                                                    }
                                                }
                                            />
                                        </div>
                                        {move || grid_has_more.get().then(|| view! {
                                            <div class="photos-more" node_ref=sentinel>
                                                <button class="text-button" on:click=move |_| load_more()>"Load more"</button>
                                            </div>
                                        })}
                                    }.into_any()
                                }}
                            </div>
                            {move || {
                                let ticks = scrubber_ticks.get();
                                if ticks.len() < 2 || locked_closed.get() {
                                    return None;
                                }
                                Some(view! {
                                    <nav class="photos-scrubber" aria-label="Timeline jump">
                                        {ticks.into_iter().map(|(label, day_key)| {
                                            let tip = format!("Jump to {label}");
                                            let title_str = label.clone();
                                            view! {
                                                <button
                                                    type="button"
                                                    class="photos-scrubber-tick"
                                                    aria-label=tip
                                                    title=title_str
                                                    on:click=move |_| {
                                                        let sel = format!("[data-day=\"{}\"]", css_escape(&day_key));
                                                        if let Some(el) = grid.get_untracked().and_then(|g| g.query_selector(&sel).ok().flatten()) {
                                                            let opts = web_sys::ScrollIntoViewOptions::new();
                                                            opts.set_behavior(web_sys::ScrollBehavior::Smooth);
                                                            opts.set_block(web_sys::ScrollLogicalPosition::Start);
                                                            el.scroll_into_view_with_scroll_into_view_options(&opts);
                                                        }
                                                    }
                                                >
                                                    <span>{label}</span>
                                                </button>
                                            }
                                        }).collect_view()}
                                    </nav>
                                })
                            }}
                            </div>
                        }.into_any()
                                }
                            }
                        }
                    }
                </main>

                <Show when=move || has_viewer.get() && view.get() != View::Duplicates>
                    <aside class="pim-details photos-details" tabindex="-1" aria-label="Photo details">
                        {
                            let detail_id = Memo::new(move |_| {
                                shown_asset.with(|s| s.as_ref().map(|a| a.id.clone()).unwrap_or_default())
                            });
                            let read_only = Signal::derive(move || {
                                view.get() == View::Trash || shown_asset.with(|s| s.as_ref().is_some_and(has_owner))
                            });
                            let sess = session.get_value();
                            view! {
                                <Details
                                    id=Signal::from(detail_id)
                                    session=sess
                                    read_only=read_only
                                    album_menu=Callback::new(move |id: String| album_menu_actions(vec![id]))
                                    on_share=Callback::new(move |id: String| link_for.set(Some(vec![id])))
                                    on_back=Callback::new(move |_| close_info())
                                    on_changed=Callback::new(move |rev: u64| {
                                        if let Some(id) = shown_asset.with_untracked(|s| s.as_ref().map(|a| a.id.clone())) {
                                            patch_local(vec![id], AssetPatch { revision: Some(rev), ..Default::default() });
                                        }
                                    })
                                    on_meta_changed=Callback::new(move |_| refresh())
                                    on_album=Callback::new(move |id: String| choose(View::Album(id)))
                                    on_error=Callback::new(move |msg: String| fail(msg))
                                />
                            }
                        }
                    </aside>
                </Show>
            </div>

            {move || picker_album.get().map(|album| {
                let sess = session.get_value();
                let existing: HashSet<String> = items.with_untracked(|l| l.iter().map(|i| i.id.clone()).collect());
                let name = album.name.clone();
                view! {
                    <AlbumPickerDialog
                        album=album
                        existing_ids=existing
                        session=sess
                        on_close=Callback::new(move |_| picker_album.set(None))
                        on_added=Callback::new(move |added: u64| {
                            picker_album.set(None);
                            refresh();
                            let msg = if added > 0 {
                                format!("Added {added} to {name}")
                            } else {
                                format!("Already in {name}")
                            };
                            show_notice(PhotosNotice { message: msg, undo: None });
                        })
                    />
                }
            })}

            {move || (!transfers.with(Vec::is_empty)).then(|| view! {
                <section class="transfer-panel photos-transfers" aria-label="Uploads">
                    <div class="transfer-heading">
                        <strong>
                            {move || transfers.with(|list| {
                                let active = list.iter().filter(|t| t.status == "uploading").count();
                                if active > 0 { format!("Uploading {active}") } else { "Uploads".to_owned() }
                            })}
                        </strong>
                        <button
                            class="icon-button"
                            aria-label="Dismiss finished uploads"
                            disabled=move || transfers.with(|list| list.iter().any(|t| t.status == "uploading"))
                            on:click=move |_| transfers.set(Vec::new())
                        >
                            <Icon name="close"/>
                        </button>
                    </div>
                    <ul>
                        {move || transfers.get().into_iter().map(|t| {
                            let cls = format!("photos-transfer is-{}", t.status);
                            let label = format!("{} upload progress", t.name);
                            let pct = format!("{}%", (t.progress * 100.0).round() as u32);
                            view! {
                                <li class=cls>
                                    <span>{t.name}</span>
                                    <progress max="1" value=t.progress.to_string() aria-label=label/>
                                    {match t.message {
                                        Some(msg) => view! { <small role="alert">{msg}</small> }.into_any(),
                                        None => view! { <small>{if t.status == "complete" { "Done".to_owned() } else { pct }}</small> }.into_any(),
                                    }}
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                </section>
            })}

            {move || album_dialog.get().map(|dlg| {
                let sess = session.get_value();
                let added_count = dlg.ids.as_ref().map(Vec::len).unwrap_or(0);
                view! {
                    <AlbumDialog
                        album=dlg.album
                        ids=dlg.ids
                        session=sess
                        on_close=Callback::new(move |_| album_dialog.set(None))
                        on_saved=Callback::new(move |(saved, created): (PhotoAlbum, bool)| {
                            album_dialog.set(None);
                            selected.set(HashSet::new());
                            load_overview();
                            if created {
                                let msg = if added_count > 0 {
                                    format!("Added {added_count} to {}", saved.name)
                                } else {
                                    format!("Created {}", saved.name)
                                };
                                show_notice(PhotosNotice { message: msg, undo: None });
                            }
                        })
                    />
                }
            })}

            {move || confirm.get().map(|album| {
                let title = format!("Delete {}?", album.name);
                let album_for_delete = album.clone();
                let album_id = StoredValue::new(album.id.clone());
                let album_name = StoredValue::new(album.name.clone());
                let sess = StoredValue::new(session.get_value());
                let album_sv = StoredValue::new(album_for_delete);
                view! {
                    <Dialog
                        title=title
                        on_close=Callback::new(move |_| confirm.set(None))
                    >
                        <ConfirmBody
                            album=album_sv.get_value()
                            session=sess.get_value()
                            on_close=Callback::new(move |_| confirm.set(None))
                            on_deleted=Callback::new(move |_| {
                                if view.get_untracked() == View::Album(album_id.get_value()) {
                                    choose(View::Folder);
                                }
                                confirm.set(None);
                                load_overview();
                                show_notice(PhotosNotice {
                                    message: format!("Deleted {}", album_name.get_value()),
                                    undo: None,
                                });
                            })
                        />
                    </Dialog>
                }
            })}

            {move || deleting.get().map(|del| {
                let sess = session.get_value();
                let in_locked = view.get_untracked() == View::Locked;
                view! {
                    <DeleteDialog
                        session=sess
                        ids=del.ids
                        empty=del.empty
                        locked=in_locked
                        on_close=Callback::new(move |_| deleting.set(None))
                        on_deleted=Callback::new(move |(removed, label): (Option<Vec<String>>, String)| {
                            deleting.set(None);
                            match removed {
                                Some(ids) => remove_local(ids),
                                None => {
                                    items.set(Vec::new());
                                    open.set(None);
                                }
                            }
                            refresh();
                            show_notice(PhotosNotice { message: label, undo: None });
                        })
                        on_locked=Callback::new(move |_| {
                            deleting.set(None);
                            folder.refresh();
                        })
                    />
                }
            })}

            {move || unlock_action.get().map(|pending| {
                let run = pending.run;
                view! {
                    <UnlockDialog
                        folder=folder
                        action=pending.action
                        on_close=Callback::new(move |_| unlock_action.set(None))
                        on_unlocked=Callback::new(move |_| {
                            unlock_action.set(None);
                            run();
                        })
                    />
                }
            })}

            {move || sharing.get().map(|album| {
                let sess = session.get_value();
                view! {
                    <SharePane
                        album=album
                        session=sess
                        on_close=Callback::new(move |_| sharing.set(None))
                        on_changed=Callback::new(move |_| load_overview())
                    />
                }
            })}

            {move || link_for.get().map(|ids| {
                let sess = session.get_value();
                view! {
                    <LinkDialog
                        ids=ids
                        session=sess
                        on_close=Callback::new(move |_| {
                            link_for.set(None);
                            selected.set(HashSet::new());
                        })
                        on_created=Callback::new(move |_| {})
                    />
                }
            })}

            <Show when=move || settings.get()>
                <PhotosSettings
                    session=session.get_value()
                    overview=overview.into()
                    folder=folder
                    on_changed=Callback::new(move |_| load_overview())
                    on_close=Callback::new(move |_| {
                        settings.set(false);
                        load_overview();
                    })
                />
            </Show>

            <div class="photos-toast-region" role="status" aria-live="polite">
                {move || notice.get().map(|n| view! {
                    <div class="settings-toast photos-toast">
                        <span>{n.message}</span>
                        {n.undo.map(|undo| view! {
                            <button
                                type="button"
                                class="text-button"
                                on:click=move |_| {
                                    notice.set(None);
                                    undo();
                                }
                            >
                                "Undo"
                            </button>
                        })}
                    </div>
                })}
            </div>
        </div>
    }
}

/// One grid tile. Sets `--ar` via CSSOM so justified rows preserve natural photo aspect ratios
/// without inline style attributes (CSP `style-src 'self'`).
#[component]
fn PhotoTile(
    item: PhotoAsset,
    selected: RwSignal<HashSet<String>>,
    can_fav: bool,
    on_open: Callback<String>,
    on_toggle: Callback<(String, bool)>,
    on_favorite: Callback<(String, bool)>,
) -> impl IntoView {
    let tile_ref = NodeRef::<html::Div>::new();
    let ar = if item.width > 0.0 && item.height > 0.0 {
        (item.width / item.height).clamp(0.6, 2.35)
    } else {
        1.333
    };
    Effect::new(move |_| {
        if let Some(el) = tile_ref.get() {
            let _ = web_sys::HtmlElement::style(&el).set_property("--ar", &format!("{ar:.4}"));
        }
    });

    let id = item.id.clone();
    let id_sel = item.id.clone();
    let id_check = item.id.clone();
    let id_click = item.id.clone();
    let id_toggle = item.id.clone();
    let id_fav = item.id.clone();

    let kind_label = if item.kind == "video" {
        "Video"
    } else if has_live(&item) {
        "Live Photo"
    } else {
        "Photo"
    };
    let taken = local_date_time(
        item.taken_at,
        &[("dateStyle", "medium"), ("timeStyle", "short")],
    );
    let fav_part = if item.favorite { ", favorite" } else { "" };
    let stack_part = match &item.stack {
        Some(s) => format!(", stack of {}", s.count),
        None => String::new(),
    };
    let open_label = format!("{kind_label} {}, {taken}{fav_part}{stack_part}", item.name);
    let check_label = format!("Select {}", item.name);
    let fav_btn_label = if item.favorite {
        format!("Remove {} from Favorites", item.name)
    } else {
        format!("Add {} to Favorites", item.name)
    };
    let is_video = item.kind == "video";
    let dur = if item.duration > 0.0 {
        format_duration(item.duration)
    } else {
        String::new()
    };
    let is_live = has_live(&item);
    let stack_count = item.stack.as_ref().map(|s| s.count);
    let is_fav = item.favorite;

    view! {
        <div
            node_ref=tile_ref
            role="listitem"
            data-asset=id
            class="photos-tile"
            class:has-stack=stack_count.is_some()
            class:is-selected=move || selected.with(|s| s.contains(&id_sel))
        >
            <button
                type="button"
                class="photos-open"
                aria-label=open_label
                on:click=move |e: web_sys::MouseEvent| {
                    let selecting = selected.with_untracked(|s| !s.is_empty());
                    if selecting || e.shift_key() {
                        { on_toggle.try_run((id_click.clone(), e.shift_key())); };
                    } else {
                        { on_open.try_run(id_click.clone()); };
                    }
                }
            >
                <Thumb asset=item size="s"/>
                {is_video.then(|| view! {
                    <span class="photos-badge"><Icon name="video"/>{dur}</span>
                })}
                {is_live.then(|| view! {
                    <span class="photos-badge"><Icon name="live"/>"Live"</span>
                })}
                {stack_count.map(|c| view! {
                    <span class="photos-badge photos-stack-badge"><Icon name="stack"/>{c}</span>
                })}
                {(!can_fav && is_fav).then(|| view! {
                    <span class="photos-fav"><Icon name="star"/></span>
                })}
            </button>
            {can_fav.then(|| view! {
                <button
                    type="button"
                    class="photos-tile-fav"
                    class:is-on=is_fav
                    aria-label=fav_btn_label
                    aria-pressed=is_fav.to_string()
                    on:click=move |e: web_sys::MouseEvent| {
                        e.stop_propagation();
                        on_favorite.try_run((id_fav.clone(), !is_fav));
                    }
                >
                    <Icon name="star"/>
                </button>
            })}
            <input
                type="checkbox"
                class="photos-check"
                aria-label=check_label
                prop:checked=move || selected.with(|s| s.contains(&id_check))
                on:click=move |e: web_sys::MouseEvent| {
                    e.stop_propagation();
                    on_toggle.try_run((id_toggle.clone(), e.shift_key()));
                }
            />
        </div>
    }
}

#[component]
fn Thumb(asset: PhotoAsset, #[prop(default = "s")] size: &'static str) -> impl IntoView {
    let failed = RwSignal::new(false);
    let initial_failed = asset.thumbnail == "failed" && asset.kind == "video";
    let icon = if asset.kind == "video" {
        "video"
    } else {
        "image"
    };
    let name = asset.name.clone();
    let sep = if size == "m" { "&" } else { "?" };
    let src = format!("{}{sep}v={}", thumb_url(&asset.id, size), asset.revision);

    move || {
        if failed.get() || initial_failed {
            view! {
                <span class="photos-placeholder">
                    <Icon name=icon/>
                    <small>{name.clone()}</small>
                </span>
            }
            .into_any()
        } else {
            view! {
                <img
                    src=src.clone()
                    alt=""
                    loading="lazy"
                    decoding="async"
                    draggable="false"
                    on:error=move |_| failed.set(true)
                />
            }
            .into_any()
        }
    }
}

#[component]
fn Viewer(
    asset: Signal<PhotoAsset>,
    shown: Signal<PhotoAsset>,
    items: Signal<Vec<PhotoAsset>>,
    index: Signal<usize>,
    session: BrowserSession,
    trash: Signal<bool>,
    more: Callback<PhotoAsset, Vec<MenuAction>>,
    album_menu: Callback<String, Vec<MenuAction>>,
    inspector_open: Signal<bool>,
    slideshow: RwSignal<bool>,
    on_toggle_inspector: Callback<()>,
    on_share_asset: Callback<String>,
    on_edited: Callback<String>,
    on_error: Callback<String>,
    on_member: Callback<Option<PhotoAsset>>,
    on_unstack: Callback<PhotoStack>,
    on_cover: Callback<PhotoStack>,
    on_info: Callback<()>,
    on_close: Callback<()>,
    on_step: Callback<isize>,
    on_open: Callback<String>,
    on_favorite: Callback<()>,
) -> impl IntoView {
    let heading = NodeRef::<html::H1>::new();
    let strip = NodeRef::<html::Div>::new();
    let media_wrap = NodeRef::<html::Div>::new();
    let stack: RwSignal<Option<PhotoStack>> = RwSignal::new(None);
    let stack_seq = RwSignal::new(0u32);
    let zoom_scale = RwSignal::new(1.0_f64);
    let editing = RwSignal::new(false);
    let rotation = RwSignal::new(0i32);
    let filter_preset = RwSignal::new("none");
    let saving_edit = RwSignal::new(false);
    let session = StoredValue::new(session);

    let css_filter_for = |preset: &str| -> &'static str {
        match preset {
            "vivid" => "saturate(1.32) contrast(1.08)",
            "warm" => "sepia(0.24) saturate(1.18) brightness(1.03)",
            "mono" => "grayscale(1) contrast(1.12)",
            _ => "none",
        }
    };

    // Focus heading when the viewer mounts.
    Effect::new(move |_| {
        if let Some(el) = heading.get() {
            let _ = el.focus();
        }
    });

    // Reset zoom & quick edit state whenever the displayed photo changes.
    Effect::new(move |_| {
        let _ = shown.with(|s| s.id.clone());
        zoom_scale.set(1.0);
        editing.set(false);
        rotation.set(0);
        filter_preset.set("none");
    });

    // Apply zoom, rotation and quick-edit filter preview via CSSOM custom properties (CSP safe).
    Effect::new(move |_| {
        let z = zoom_scale.get();
        let rot = rotation.get();
        let filt = css_filter_for(filter_preset.get());
        if let Some(el) = media_wrap.get() {
            let st = web_sys::HtmlElement::style(&el);
            let _ = st.set_property("--stage-zoom", &format!("{z:.2}"));
            let _ = st.set_property("--stage-rot", &format!("{rot}deg"));
            let _ = st.set_property("--stage-filter", filt);
        }
    });

    // Zoom keyboard shortcuts (+, -, 0).
    listen(&document(), "keydown", false, move |event| {
        let event: web_sys::KeyboardEvent = event.unchecked_into();
        if event.default_prevented() || event.ctrl_key() || event.meta_key() || event.alt_key() {
            return;
        }
        let in_control = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|t| {
                t.closest("input,textarea,select,[contenteditable=true],dialog,.ui-menu")
                    .ok()
                    .flatten()
            })
            .is_some();
        if in_control {
            return;
        }
        match event.key().as_str() {
            "+" | "=" => {
                event.prevent_default();
                zoom_scale.update(|z| *z = (*z + 0.5).min(4.0));
            }
            "-" | "_" => {
                event.prevent_default();
                zoom_scale.update(|z| *z = (*z - 0.5).max(1.0));
            }
            "0" => {
                event.prevent_default();
                zoom_scale.set(1.0);
            }
            _ => {}
        }
    });

    // Keep the current thumbnail centered in the bottom strip.
    Effect::new(move |_| {
        let _ = asset.with(|a| a.id.clone());
        if let Some(container) = strip.get()
            && let Some(cur) = container.query_selector(".is-current").ok().flatten()
        {
            let opts = web_sys::ScrollIntoViewOptions::new();
            opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
            opts.set_inline(web_sys::ScrollLogicalPosition::Center);
            cur.scroll_into_view_with_scroll_into_view_options(&opts);
        }
    });

    // Fetch stack details when the open asset belongs to a stack.
    let stack_key = Memo::new(move |_| {
        asset.with(|a| {
            (
                a.id.clone(),
                a.stack.as_ref().map(|s| (s.id.clone(), s.count)),
            )
        })
    });
    Effect::new(move |_| {
        let (_, stack_ref) = stack_key.get();
        stack.set(None);
        let Some((stack_id, _)) = stack_ref else {
            return;
        };
        let ticket = stack_seq.get_untracked().wrapping_add(1);
        stack_seq.set(ticket);
        spawn_local(async move {
            if let Ok(value) = api::get::<PhotoStack>(&format!("{BASE}/stacks/{stack_id}")).await
                && stack_seq.try_get_untracked() == Some(ticket)
            {
                stack.try_set(Some(value));
            }
        });
    });

    let stage_key = Memo::new(move |_| {
        shown.with(|s| {
            (
                s.id.clone(),
                s.kind.clone(),
                s.live.clone(),
                s.playback.clone(),
                s.revision,
            )
        })
    });

    let menu_actions = move |_: ()| -> Vec<MenuAction> {
        let shown_now = shown.get_untracked();
        let mut actions = more.run(shown_now.clone());
        if let Some(st) = stack.get_untracked() {
            let mut sub = Vec::new();
            if shown_now.id != st.primary_id {
                let st_cover = st.clone();
                sub.push(MenuAction::new("Make this the cover", "star").run(move || {
                    on_cover.try_run(st_cover.clone());
                }));
            }
            let st_unstack = st;
            sub.push(MenuAction::new("Unstack", "close").run(move || {
                on_unstack.try_run(st_unstack.clone());
            }));
            actions.push(MenuAction::new("Stack", "stack").separator().items(sub));
        }
        actions
    };

    let can_quick_edit = Memo::new(move |_| {
        !trash.get()
            && shown.with(|s| {
                s.kind != "video" && !has_owner(s) && is_browser_decodable_image(&s.media_type)
            })
    });
    let is_still = Memo::new(move |_| shown.with(|s| s.kind != "video"));

    let save_edit = move |_| {
        let current = shown.get_untracked();
        let rot = rotation.get_untracked();
        let filt = css_filter_for(filter_preset.get_untracked());
        if rot == 0 && filt == "none" {
            return;
        }
        saving_edit.set(true);
        let csrf = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            match save_edited_copy(&current, rot, filt, &csrf).await {
                Ok(new_id) => {
                    saving_edit.try_set(false);
                    editing.try_set(false);
                    on_edited.try_run(new_id);
                }
                Err(msg) => {
                    saving_edit.try_set(false);
                    on_error.try_run(msg);
                }
            }
        });
    };

    view! {
        <section
            class="photos-viewer"
            aria-label=move || format!("Viewer for {}", shown.with(|s| s.name.clone()))
        >
            <div class="photos-viewer-bar">
                <button
                    type="button"
                    class="icon-button"
                    aria-label="Back to photos"
                    title="Back (Escape)"
                    on:click=move |_| { on_close.try_run(()); }
                >
                    <Icon name="back"/>
                </button>
                <h1 node_ref=heading tabindex="-1">{move || shown.with(|s| s.name.clone())}</h1>
                <span class="photos-heading-count">
                    {move || format!("{} of {}", index.get() + 1, items.with(Vec::len))}
                </span>
                <span class="photos-spacer"></span>
                <Show when={move || items.with(|l| l.len() > 1)}>
                    <button
                        type="button"
                        class="icon-button"
                        class:is-active=move || slideshow.get()
                        aria-label=move || if slideshow.get() { "Pause slideshow" } else { "Start slideshow" }
                        aria-pressed=move || slideshow.get().to_string()
                        title="Slideshow (s)"
                        on:click=move |_| slideshow.update(|v| *v = !*v)
                    >
                        <Icon name=Signal::derive(move || if slideshow.get() { "pause" } else { "play" })/>
                    </button>
                </Show>
                <Show when=move || can_quick_edit.get()>
                    <button
                        type="button"
                        class="icon-button"
                        class:is-active=move || editing.get()
                        aria-label="Quick edit"
                        aria-pressed=move || editing.get().to_string()
                        title="Rotate and filter"
                        on:click=move |_| editing.update(|v| *v = !*v)
                    >
                        <Icon name="sliders"/>
                    </button>
                </Show>
                <Show when=move || !trash.get()>
                    <button
                        type="button"
                        class="icon-button"
                        class:is-on=move || shown.with(|s| s.favorite)
                        aria-label=move || if shown.with(|s| s.favorite) { "Remove from Favorites" } else { "Add to Favorites" }
                        aria-pressed=move || shown.with(|s| s.favorite).to_string()
                        title="Favorite (f)"
                        on:click=move |_| { on_favorite.try_run(()); }
                    >
                        <Icon name="star"/>
                    </button>
                    <button
                        type="button"
                        class="icon-button photos-viewer-desktop-btn"
                        aria-label="Share link"
                        title="Create shared link"
                        on:click=move |_| {
                            let id = shown.with_untracked(|s| s.id.clone());
                            on_share_asset.try_run(id);
                        }
                    >
                        <Icon name="share"/>
                    </button>
                    <ActionMenu
                        label="Add photo to album"
                        class="photos-viewer-desktop-btn"
                        actions=move |_| album_menu.run(shown.with_untracked(|s| s.id.clone()))
                    >
                        <Icon name="plus"/>
                    </ActionMenu>
                </Show>
                <a
                    class="icon-button photos-viewer-desktop-btn"
                    href=move || format!("{BASE}/assets/{}/original", shown.with(|s| s.id.clone()))
                    download
                    aria-label="Download original"
                    title="Download original"
                >
                    <Icon name="download"/>
                </a>
                <button
                    type="button"
                    class="icon-button photos-inspector-toggle"
                    class:is-active=move || inspector_open.get()
                    aria-label="Toggle details"
                    aria-pressed=move || inspector_open.get().to_string()
                    title="Toggle details (i)"
                    on:click=move |_| { on_toggle_inspector.try_run(()); }
                >
                    <Icon name="panel"/>
                </button>
                <button
                    type="button"
                    class="icon-button photos-info-toggle"
                    aria-label="Photo info"
                    on:click=move |_| { on_info.try_run(()); }
                >
                    <Icon name="info"/>
                </button>
                <ActionMenu label="More photo actions" actions=menu_actions>
                    <Icon name="more"/>
                </ActionMenu>
            </div>

            <div
                class="photos-stage"
                class:is-zoomed={move || zoom_scale.get() > 1.01}
                on:wheel=move |e: web_sys::WheelEvent| {
                    if !is_still.get_untracked() {
                        return;
                    }
                    if e.ctrl_key() || zoom_scale.get_untracked() > 1.01 {
                        e.prevent_default();
                        let delta = if e.delta_y() < 0.0 { 0.25 } else { -0.25 };
                        zoom_scale.update(|z| *z = (*z + delta).clamp(1.0, 4.0));
                    }
                }
            >
                <Show when={move || index.get() > 0}>
                    <button
                        type="button"
                        class="photos-stage-nav is-prev"
                        aria-label="Previous"
                        title="Previous (\u{2190})"
                        on:click=move |_| { on_step.try_run(-1); }
                    >
                        <Icon name="back"/>
                    </button>
                </Show>
                <div
                    node_ref=media_wrap
                    class="photos-stage-media"
                    on:dblclick=move |_| {
                        if is_still.get_untracked() {
                            zoom_scale.update(|z| *z = if *z > 1.05 { 1.0 } else { 2.0 });
                        }
                    }
                >
                    {move || {
                        let _ = stage_key.get();
                        let s = shown.get_untracked();
                        if s.kind == "video" {
                            let poster = thumb_url(&s.id, "m");
                            view! {
                                <VideoStage
                                    asset=s
                                    session=session.get_value()
                                    controls=true
                                    poster=poster
                                />
                            }.into_any()
                        } else if let Some(motion) = s.live.clone().filter(|m| !m.is_empty()) {
                            view! {
                                <LiveStage
                                    asset=s
                                    motion=motion
                                    session=session.get_value()
                                />
                            }.into_any()
                        } else {
                            view! { <StageImage asset=s/> }.into_any()
                        }
                    }}
                </div>
                <Show when={move || index.get() + 1 < items.with(Vec::len)}>
                    <button
                        type="button"
                        class="photos-stage-nav is-next"
                        aria-label="Next"
                        title="Next (\u{2192})"
                        on:click=move |_| { on_step.try_run(1); }
                    >
                        <Icon name="chevron"/>
                    </button>
                </Show>
                <Show when={move || is_still.get() && !editing.get()}>
                    <div class="photos-zoom-pill" role="group" aria-label="Image zoom">
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="Zoom out"
                            disabled={move || zoom_scale.get() <= 1.01}
                            on:click=move |_| zoom_scale.update(|z| *z = (*z - 0.5).max(1.0))
                        >
                            <Icon name="zoomOut"/>
                        </button>
                        <button
                            type="button"
                            class="photos-zoom-readout"
                            aria-label="Reset zoom"
                            on:click=move |_| zoom_scale.set(1.0)
                        >
                            {move || {
                                let z = zoom_scale.get();
                                if (z - 1.0).abs() < 0.05 { "Fit".to_owned() } else { format!("{}%", (z * 100.0).round() as i32) }
                            }}
                        </button>
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="Zoom in"
                            disabled={move || zoom_scale.get() >= 3.99}
                            on:click=move |_| zoom_scale.update(|z| *z = (*z + 0.5).min(4.0))
                        >
                            <Icon name="zoomIn"/>
                        </button>
                    </div>
                </Show>
                <Show when=move || editing.get()>
                    <div class="photos-edit-bar" role="region" aria-label="Quick edit">
                        <button
                            type="button"
                            class="secondary-button"
                            on:click=move |_| rotation.update(|r| *r = (*r + 90) % 360)
                        >
                            <Icon name="rotate"/>"Rotate 90\u{00b0}"
                        </button>
                        <div class="photos-edit-presets" role="group" aria-label="Filter preset">
                            {[("none", "Original"), ("vivid", "Vivid"), ("warm", "Warm"), ("mono", "Mono")].into_iter().map(|(val, lbl)| {
                                view! {
                                    <button
                                        type="button"
                                        class="photos-filter-chip"
                                        class:is-active=move || filter_preset.get() == val
                                        aria-pressed=move || (filter_preset.get() == val).to_string()
                                        on:click=move |_| filter_preset.set(val)
                                    >
                                        {lbl}
                                    </button>
                                }
                            }).collect_view()}
                        </div>
                        <span class="photos-spacer"></span>
                        <button
                            type="button"
                            class="text-button"
                            on:click=move |_| {
                                rotation.set(0);
                                filter_preset.set("none");
                            }
                        >
                            "Reset"
                        </button>
                        <button
                            type="button"
                            class="primary-button"
                            disabled=move || saving_edit.get() || (rotation.get() == 0 && filter_preset.get() == "none")
                            on:click=save_edit
                        >
                            {move || if saving_edit.get() { "Saving\u{2026}" } else { "Save copy" }}
                        </button>
                    </div>
                </Show>
            </div>
            {move || stack.get().filter(|s| s.items.len() > 1).map(|st| {
                let label = format!("Stack of {}", st.items.len());
                let primary_id = st.primary_id.clone();
                view! {
                    <div class="photos-stack-strip" aria-label=label role="group">
                        <Icon name="stack"/>
                        {st.items.into_iter().map(|item| {
                            let id_cur = item.id.clone();
                            let id_pressed = item.id.clone();
                            let btn_label = if item.id == primary_id {
                                format!("{}, cover", item.name)
                            } else {
                                item.name.clone()
                            };
                            let item_click = item.clone();
                            view! {
                                <button
                                    type="button"
                                    class=move || if shown.with(|s| s.id == id_cur) { "is-current" } else { "" }
                                    aria-label=btn_label
                                    aria-pressed=move || shown.with(|s| s.id == id_pressed).to_string()
                                    on:click=move |_| {
                                        if asset.with_untracked(|a| a.id == item_click.id) {
                                            { on_member.try_run(None); };
                                        } else {
                                            { on_member.try_run(Some(item_click.clone())); };
                                        }
                                    }
                                >
                                    <Thumb asset=item size="s"/>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                }
            })}
            <Show when={move || items.with(|l| l.len() > 1)}>
                <div class="photos-strip" node_ref=strip aria-label="Nearby photos">
                    {move || {
                        let idx = index.get();
                        let list = items.get();
                        let start = idx.saturating_sub(12);
                        let end = (idx + 13).min(list.len());
                        let cur_id = asset.with(|a| a.id.clone());
                        list[start..end].iter().cloned().map(|item| {
                            let is_cur = item.id == cur_id;
                            let id = item.id.clone();
                            let label = item.name.clone();
                            view! {
                                <button
                                    type="button"
                                    class=if is_cur { "is-current" } else { "" }
                                    aria-label=label
                                    aria-current=is_cur.then_some("true")
                                    on:click=move |_| { on_open.try_run(id.clone()); }
                                >
                                    <Thumb asset=item size="s"/>
                                </button>
                            }
                        }).collect_view()
                    }}
                </div>
            </Show>
        </section>
    }
}

/// Shows the large preview, falling back to the original for browser-decodable files.
#[component]
fn StageImage(asset: PhotoAsset) -> impl IntoView {
    let source = RwSignal::new("m");
    let asset = StoredValue::new(asset);

    move || {
        let a = asset.get_value();
        match source.get() {
            "none" => view! {
                <div class="photos-empty">
                    <Icon name="image"/>
                    <h2>"No preview available"</h2>
                    <p>"Dar could not render this file. You can still download the original."</p>
                    <a class="secondary-button" href=format!("/api/v1/files/{}/content", a.id) download>
                        <Icon name="download"/>"Download"
                    </a>
                </div>
            }.into_any(),
            cur => {
                let src = if cur == "m" {
                    format!("{}&v={}", thumb_url(&a.id, "m"), a.revision)
                } else {
                    content_url(&a.id)
                };
                let decodable = is_browser_decodable_image(&a.media_type);
                view! {
                    <img
                        src=src
                        alt=a.name
                        on:error=move |_| {
                            if cur == "m" && decodable {
                                source.set("original");
                            } else {
                                source.set("none");
                            }
                        }
                    />
                }.into_any()
            }
        }
    }
}

/// Plays a video the way this browser can: the original, the original HEVC where the browser
/// decodes it, or a copy the server remuxes or transcodes once (with progress).
#[component]
fn VideoStage(
    asset: PhotoAsset,
    session: BrowserSession,
    #[prop(optional)] controls: bool,
    #[prop(optional, into)] poster: Option<String>,
    #[prop(optional)] auto_play: bool,
    #[prop(optional)] on_ended: Option<Callback<()>>,
) -> impl IntoView {
    let playback: RwSignal<Option<Playback>> = RwSignal::new(None);
    let convert = RwSignal::new(false);
    let poll_seq = RwSignal::new(0u32);
    let asset = StoredValue::new(asset);
    let session = StoredValue::new(session);
    let poster = StoredValue::new(poster);

    Effect::new(move |_| {
        let do_convert = convert.get();
        let a = asset.get_value();
        let pb = a.playback.as_deref().unwrap_or("");
        // Not yet indexed for playback: the Files original, as before.
        if pb.is_empty() {
            playback.set(Some(Playback {
                state: "ready".to_owned(),
                progress: 1.0,
                url: content_url(&a.id),
            }));
            return;
        }
        if pb == "original"
            || (!do_convert && a.video_codec.as_deref() == Some("hevc") && hevc_playable())
        {
            let suffix = if pb == "original" {
                ""
            } else {
                "?source=original"
            };
            playback.set(Some(Playback {
                state: "ready".to_owned(),
                progress: 1.0,
                url: format!("{BASE}/assets/{}/video{suffix}", a.id),
            }));
            return;
        }
        let ticket = poll_seq.get_untracked().wrapping_add(1);
        poll_seq.set(ticket);
        playback.set(Some(Playback {
            state: "preparing".to_owned(),
            progress: 0.0,
            url: String::new(),
        }));
        let url = format!("{BASE}/assets/{}/playback", a.id);
        let csrf_tok = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            loop {
                if poll_seq.try_get_untracked() != Some(ticket) {
                    return;
                }
                match api::send::<Playback>(&url, "POST", None, &csrf_tok).await {
                    Ok(value) => {
                        if poll_seq.try_get_untracked() != Some(ticket) {
                            return;
                        }
                        let preparing = value.state == "preparing";
                        playback.try_set(Some(value));
                        if !preparing {
                            return;
                        }
                        crate::sleep(1500).await;
                    }
                    Err(_) => {
                        if poll_seq.try_get_untracked() == Some(ticket) {
                            playback.try_set(Some(Playback {
                                state: "failed".to_owned(),
                                progress: 0.0,
                                url: String::new(),
                            }));
                        }
                        return;
                    }
                }
            }
        });
    });

    move || {
        let a = asset.get_value();
        match playback.get() {
            None => view! {
                <div class="photos-video-wait" role="status">
                    <span class="photos-spinner" aria-hidden="true"></span>
                    <span>"Preparing this video for your browser\u{2026}"</span>
                </div>
            }.into_any(),
            Some(pb) if pb.state == "preparing" => {
                let suffix = if pb.progress > 0.0 {
                    format!(" \u{00b7} {}%", (pb.progress * 100.0).round() as u32)
                } else {
                    "\u{2026}".to_owned()
                };
                view! {
                    <div class="photos-video-wait" role="status">
                        <span class="photos-spinner" aria-hidden="true"></span>
                        <span>"Preparing this video for your browser"{suffix}</span>
                    </div>
                }.into_any()
            }
            Some(pb) if pb.state == "failed" => view! {
                <div class="photos-empty" role="alert">
                    <Icon name="video"/>
                    <h2>"This video can\u{2019}t be played here"</h2>
                    <p>"Dar could not convert it for your browser. You can still download the original."</p>
                    <a class="secondary-button" href=format!("/api/v1/files/{}/content", a.id) download>
                        <Icon name="download"/>"Download"
                    </a>
                </div>
            }.into_any(),
            Some(pb) => {
                let can_fallback = a.playback.as_deref().is_some_and(|p| !p.is_empty() && p != "original");
                view! {
                    <video
                        src=pb.url
                        controls=controls
                        playsinline
                        preload="metadata"
                        poster=poster.get_value()
                        autoplay=auto_play
                        muted=auto_play
                        prop:muted=auto_play
                        aria-label=a.name
                        on:ended=move |_| {
                            if let Some(cb) = on_ended {
                                cb.run(());
                            }
                        }
                        on:error=move |_| {
                            if can_fallback && !convert.get_untracked() {
                                convert.set(true);
                            }
                        }
                    />
                }.into_any()
            }
        }
    }
}

/// A Live Photo: the still, with its motion played over it on demand.
#[component]
fn LiveStage(asset: PhotoAsset, motion: String, session: BrowserSession) -> impl IntoView {
    let playing = RwSignal::new(false);
    let clip: RwSignal<Option<PhotoAsset>> = RwSignal::new(None);
    let error = RwSignal::new(false);
    let seq = RwSignal::new(0u32);
    let motion = StoredValue::new(motion);
    let session = StoredValue::new(session);

    Effect::new(move |_| {
        if !playing.get() || clip.with_untracked(Option::is_some) {
            return;
        }
        let ticket = seq.get_untracked().wrapping_add(1);
        seq.set(ticket);
        let url = format!("{BASE}/assets/{}", motion.get_value());
        spawn_local(async move {
            match api::get::<PhotoDetail>(&url).await {
                Ok(value) if seq.try_get_untracked() == Some(ticket) => {
                    clip.try_set(Some(value.asset));
                }
                Err(_) if seq.try_get_untracked() == Some(ticket) => {
                    error.try_set(true);
                    playing.try_set(false);
                }
                _ => {}
            }
        });
    });

    view! {
        <div class="photos-live">
            <StageImage asset=asset/>
            {move || {
                if !playing.get() {
                    return None;
                }
                let clip_asset = clip.get()?;
                Some(view! {
                    <div class="photos-live-motion">
                        <VideoStage
                            asset=clip_asset
                            session=session.get_value()
                            auto_play=true
                            on_ended=Callback::new(move |_| playing.set(false))
                        />
                    </div>
                })
            }}
            <button
                type="button"
                class="photos-live-toggle"
                aria-pressed=move || playing.get().to_string()
                disabled=move || error.get()
                title=move || if error.get() { "The motion part is not available" } else { "Play the Live Photo" }
                on:click=move |_| playing.update(|p| *p = !*p)
            >
                <Icon name="live"/>"Live"
            </button>
        </div>
    }
}

#[component]
fn Details(
    id: Signal<String>,
    session: BrowserSession,
    read_only: Signal<bool>,
    album_menu: Callback<String, Vec<MenuAction>>,
    on_share: Callback<String>,
    on_back: Callback<()>,
    on_changed: Callback<u64>,
    on_meta_changed: Callback<()>,
    on_album: Callback<String>,
    on_error: Callback<String>,
) -> impl IntoView {
    let detail: RwSignal<Option<PhotoDetail>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let description = RwSignal::new(String::new());
    let saving = RwSignal::new("");
    let conflict = RwSignal::new(false);
    let editing_date = RwSignal::new(false);
    let date_draft = RwSignal::new(String::new());
    let saving_date = RwSignal::new(false);
    let seq = RwSignal::new(0u32);
    let session = StoredValue::new(session);

    let load = move || {
        let asset_id = id.get_untracked();
        if asset_id.is_empty() {
            return;
        }
        let ticket = seq.get_untracked().wrapping_add(1);
        seq.set(ticket);
        error.set(String::new());
        spawn_local(async move {
            match api::get::<PhotoDetail>(&format!("{BASE}/assets/{asset_id}")).await {
                Ok(value) if seq.try_get_untracked() == Some(ticket) => {
                    description.try_set(value.description.clone());
                    detail.try_set(Some(value));
                    conflict.try_set(false);
                    editing_date.try_set(false);
                }
                Err(cause) if seq.try_get_untracked() == Some(ticket) => {
                    error.try_set(error_message(&cause.message));
                }
                _ => {}
            }
        });
    };

    Effect::new(move |_| {
        let _ = id.get();
        detail.set(None);
        saving.set("");
        editing_date.set(false);
        load();
    });

    let save = move || {
        let Some(cur) = detail.get_untracked() else {
            return;
        };
        let desc = description.get_untracked();
        if read_only.get_untracked() || desc.trim() == cur.description {
            return;
        }
        saving.set("saving");
        let asset_id = cur.asset.id.clone();
        let orig_desc = cur.description.clone();
        let orig_rev = cur.asset.revision;
        let csrf_tok = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            let put = |rev: u64| {
                let url = format!("{BASE}/assets/{asset_id}/description");
                let body = json!({ "description": desc, "revision": rev });
                let csrf = csrf_tok.clone();
                async move { api::send::<PhotoAsset>(&url, "PUT", Some(body), &csrf).await }
            };
            let result = match put(orig_rev).await {
                Ok(val) => Ok(val),
                Err(cause) if cause.status == 409 => {
                    match api::get::<PhotoDetail>(&format!("{BASE}/assets/{asset_id}")).await {
                        Ok(fresh) if fresh.description != orig_desc => {
                            detail.try_set(Some(fresh));
                            Err(cause)
                        }
                        Ok(fresh) => put(fresh.asset.revision).await,
                        Err(_) => Err(cause),
                    }
                }
                Err(cause) => Err(cause),
            };
            match result {
                Ok(value) => {
                    let trimmed = desc.trim().to_owned();
                    let rev = value.revision;
                    detail.try_update(|d| {
                        if let Some(d) = d {
                            d.asset = value.clone();
                            d.description = trimmed.clone();
                        }
                    });
                    {
                        on_changed.try_run(rev);
                    };
                    saving.try_set("saved");
                }
                Err(cause) => {
                    saving.try_set("");
                    if cause.status == 409 {
                        conflict.try_set(true);
                    } else {
                        {
                            on_error.try_run(cause.message);
                        };
                    }
                }
            }
        });
    };

    let save_date = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let Some(cur) = detail.get_untracked() else {
            return;
        };
        let Some(ms) = parse_datetime_local_value(&date_draft.get_untracked()) else {
            on_error.try_run("Enter a valid date and time.".to_owned());
            return;
        };
        saving_date.set(true);
        let asset_id = cur.asset.id.clone();
        let rev = cur.asset.revision;
        let csrf_tok = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            match api::send::<PhotoAsset>(
                &format!("{BASE}/assets/{asset_id}/meta"),
                "PUT",
                Some(json!({ "takenAt": ms, "revision": rev })),
                &csrf_tok,
            )
            .await
            {
                Ok(updated) => {
                    let new_rev = updated.revision;
                    detail.try_update(|d| {
                        if let Some(d) = d {
                            d.asset = updated.clone();
                        }
                    });
                    editing_date.try_set(false);
                    saving_date.try_set(false);
                    on_changed.try_run(new_rev);
                    on_meta_changed.try_run(());
                }
                Err(cause) => {
                    saving_date.try_set(false);
                    on_error.try_run(cause.message);
                }
            }
        });
    };

    move || {
        if !error.get().is_empty() {
            return view! {
                <div class="photos-empty" role="alert">
                    <h2>"Details could not be loaded"</h2>
                    <p>{error.get()}</p>
                    <button class="secondary-button" on:click=move |_| load()>"Retry"</button>
                </div>
            }
            .into_any();
        }
        let Some(d) = detail.get() else {
            return view! { <p class="photos-empty" role="status">"Loading details\u{2026}"</p> }
                .into_any();
        };

        let e = &d.exif;
        let mut cam_parts = Vec::new();
        if let (Some(make), Some(model)) = (&e.make, &e.model)
            && !make.is_empty()
            && !model.is_empty()
            && !model.starts_with(make.as_str())
        {
            cam_parts.push(make.clone());
        }
        if let Some(model) = &e.model
            && !model.is_empty()
        {
            cam_parts.push(model.clone());
        }
        let camera = cam_parts.join(" ");

        let megapixels = if d.asset.width > 0.0 && d.asset.height > 0.0 {
            format!("{:.1} MP \u{00b7} ", d.asset.width * d.asset.height / 1e6)
        } else {
            String::new()
        };

        let mut exif_pills = Vec::new();
        if let Some(fl) = e.focal_length
            && fl > 0.0
        {
            exif_pills.push(format!("{} mm", fl.round() as i64));
        }
        if let Some(f) = e.f_number
            && f > 0.0
        {
            exif_pills.push(format!("\u{0192}/{f}"));
        }
        if let Some(exp) = e.exposure
            && exp > 0.0
        {
            if exp < 1.0 {
                exif_pills.push(format!("1/{} s", (1.0 / exp).round() as i64));
            } else {
                exif_pills.push(format!("{exp} s"));
            }
        }
        if let Some(iso) = e.iso
            && iso > 0.0
        {
            exif_pills.push(format!("ISO {iso}"));
        }
        let lens_name = e.lens.clone().unwrap_or_default();
        let show_camera = !camera.is_empty() || !lens_name.is_empty() || !exif_pills.is_empty();

        let video_info = if d.asset.kind == "video" {
            let mut parts = Vec::new();
            if let Some(vc) = &e.video_codec
                && !vc.is_empty()
            {
                parts.push(vc.to_uppercase());
            }
            if let Some(ac) = &e.audio_codec
                && !ac.is_empty()
            {
                parts.push(ac.to_uppercase());
            }
            match d.asset.playback.as_deref() {
                Some("transcode") => parts.push("Converted for browsers".to_owned()),
                Some("remux") => parts.push("Repackaged for browsers".to_owned()),
                _ => {}
            }
            parts.join(" \u{00b7} ")
        } else {
            String::new()
        };

        let owner_opt = d.asset.owner.clone().filter(|s| !s.is_empty());
        let is_locked = d.locked;
        let trashed_at = d.asset.trashed_at.filter(|&t| t > 0.0);
        let taken_ms = d.asset.taken_at;
        let taken_date = local_date(
            taken_ms,
            &[
                ("weekday", "long"),
                ("year", "numeric"),
                ("month", "long"),
                ("day", "numeric"),
            ],
        );
        let taken_time = local_time(
            taken_ms,
            &[
                ("hour", "numeric"),
                ("minute", "2-digit"),
                ("timeZoneName", "short"),
            ],
        );
        let dims = if d.asset.width > 0.0 {
            format!("{} \u{00d7} {} \u{00b7} ", d.asset.width, d.asset.height)
        } else {
            String::new()
        };
        let dur = if d.asset.kind == "video" && d.asset.duration > 0.0 {
            format!("{} \u{00b7} ", format_duration(d.asset.duration))
        } else {
            String::new()
        };
        let size_str = format_bytes(d.asset.size);
        let file_icon = if d.asset.kind == "video" {
            "video"
        } else {
            "image"
        };
        let is_live = has_live(&d.asset);
        let coords = match (d.latitude, d.longitude) {
            (Some(lat), Some(lon)) => Some((lat, lon, format!("{lat:.5}, {lon:.5}"))),
            _ => None,
        };
        let show_files_href = if is_locked {
            if d.parent_id.is_empty() {
                "/?section=locked".to_owned()
            } else {
                format!("/?section=locked&folder={}", d.parent_id)
            }
        } else {
            format!("/?folder={}&file={}", d.parent_id, d.asset.id)
        };
        let show_in_files = !read_only.get() && owner_opt.is_none();
        let can_edit_meta = show_in_files;
        let original_href = format!("{BASE}/assets/{}/original", d.asset.id);
        let dl_href = original_href.clone();
        let asset_id_share = d.asset.id.clone();
        let asset_id_album = d.asset.id.clone();
        let asset_id_album2 = d.asset.id.clone();
        let albums = d.albums.clone();

        view! {
            <div class="photos-detail">
                <div class="photos-detail-head">
                    <button
                        type="button"
                        class="icon-button photos-detail-back"
                        aria-label="Back to photo"
                        on:click=move |_| { on_back.try_run(()); }
                    >
                        <Icon name="back"/>
                    </button>
                    <h2>"Details"</h2>
                </div>
                <div class="photos-detail-body">
                    <div class="photos-detail-actions" role="group" aria-label="Quick actions">
                        <a class="photos-quick-btn" href=dl_href download>
                            <Icon name="download"/><span>"Download"</span>
                        </a>
                        {can_edit_meta.then(|| view! {
                            <button
                                type="button"
                                class="photos-quick-btn"
                                on:click=move |_| { on_share.try_run(asset_id_share.clone()); }
                            >
                                <Icon name="share"/><span>"Share"</span>
                            </button>
                            <ActionMenu
                                label="Add to album"
                                class="photos-quick-menu"
                                actions=move |_| album_menu.run(asset_id_album.clone())
                            >
                                <span class="photos-quick-btn-inner"><Icon name="plus"/><span>"Album"</span></span>
                            </ActionMenu>
                        })}
                        {show_in_files.then(|| view! {
                            <a class="photos-quick-btn" href=show_files_href.clone()>
                                <Icon name="folder"/><span>"Files"</span>
                            </a>
                        })}
                    </div>

                    {owner_opt.map(|owner| view! {
                        <p class="photos-detail-note">
                            <Icon name="share"/>"Shared by "{owner}". "
                            <a href=original_href download>"Download original"</a>
                        </p>
                    })}
                    {is_locked.then(|| view! {
                        <p class="photos-detail-note">
                            <Icon name="lock"/>"In the Locked folder: hidden from the library, albums and search."
                        </p>
                    })}
                    {(read_only.get() && trashed_at.is_some()).then(|| {
                        let date = local_date(trashed_at.unwrap_or(0.0), &[("dateStyle", "medium")]);
                        view! {
                            <p class="photos-detail-note">
                                <Icon name="trash"/>"Moved to the trash "{date}"."
                            </p>
                        }
                    })}
                    <label class="photos-description">
                        <span class="sr-only">"Description"</span>
                        <textarea
                            prop:value=move || description.get()
                            maxlength="2000"
                            rows="2"
                            readonly=move || read_only.get()
                            placeholder=move || if read_only.get() { "No description" } else { "Add a description\u{2026}" }
                            on:input=move |e| {
                                description.set(event_target_value(&e));
                                saving.set("");
                            }
                            on:blur=move |_| save()
                        />
                    </label>
                    {move || {
                        let s = saving.get();
                        (!s.is_empty()).then(|| view! {
                            <p class="photos-saved" role="status">
                                {if s == "saving" { "Saving\u{2026}" } else { "Saved" }}
                            </p>
                        })
                    }}
                    {move || conflict.get().then(|| view! {
                        <div class="error" role="alert">
                            <p>"This description changed elsewhere."</p>
                            <button class="text-button" on:click=move |_| load()>"Reload"</button>
                        </div>
                    })}
                    <ul class="photos-props">
                        <li>
                            <Icon name="calendar"/>
                            <span class="photos-prop-grow">
                                <div class="photos-prop-row">
                                    <strong>{taken_date}</strong>
                                    {can_edit_meta.then(|| view! {
                                        <button
                                            type="button"
                                            class="icon-button photos-prop-edit"
                                            aria-label="Edit date and time"
                                            title="Edit date and time"
                                            on:click=move |_| {
                                                date_draft.set(to_datetime_local_value(taken_ms));
                                                editing_date.update(|v| *v = !*v);
                                            }
                                        >
                                            <Icon name="pencil"/>
                                        </button>
                                    })}
                                </div>
                                <small>{taken_time}</small>
                                {move || editing_date.get().then(|| view! {
                                    <form class="photos-date-form" on:submit=save_date>
                                        <label class="sr-only" for="photos-edit-date">"Capture date and time"</label>
                                        <input
                                            id="photos-edit-date"
                                            type="datetime-local"
                                            prop:value=move || date_draft.get()
                                            on:input=move |e| date_draft.set(event_target_value(&e))
                                        />
                                        <div class="photos-date-actions">
                                            <button type="button" class="text-button" on:click=move |_| editing_date.set(false)>"Cancel"</button>
                                            <button type="submit" class="primary-button" disabled=move || saving_date.get()>
                                                {move || if saving_date.get() { "Saving\u{2026}" } else { "Save" }}
                                            </button>
                                        </div>
                                    </form>
                                })}
                            </span>
                        </li>
                        <li>
                            <Icon name=file_icon/>
                            <span>
                                <strong>{d.asset.name.clone()}</strong>
                                <small>{megapixels}{dims}{dur}{size_str}</small>
                                {(!video_info.is_empty()).then(|| view! { <small>{video_info}</small> })}
                            </span>
                        </li>
                        {is_live.then(|| view! {
                            <li>
                                <Icon name="live"/>
                                <span>
                                    <strong>"Live Photo"</strong>
                                    <small>"The motion part is kept next to the still in Files."</small>
                                </span>
                            </li>
                        })}
                        {show_camera.then(|| view! {
                            <li>
                                <Icon name="camera"/>
                                <span>
                                    <strong>{if camera.is_empty() { "Camera".to_owned() } else { camera }}</strong>
                                    {(!lens_name.is_empty()).then(|| view! { <small>{lens_name}</small> })}
                                    {(!exif_pills.is_empty()).then(|| view! {
                                        <div class="photos-exif-pills">
                                            {exif_pills.into_iter().map(|p| view! {
                                                <span class="photos-exif-pill">{p}</span>
                                            }).collect_view()}
                                        </div>
                                    })}
                                </span>
                            </li>
                        })}
                    </ul>
                    {coords.map(|(lat, lon, loc_str)| {
                        let osm_url = format!("https://www.openstreetmap.org/?mlat={lat:.6}&mlon={lon:.6}#map=14/{lat:.6}/{lon:.6}");
                        view! {
                            <div class="photos-map-card" aria-label="Photo location">
                                <div class="photos-map-canvas" aria-hidden="true">
                                    <span class="photos-map-pin"><Icon name="location"/></span>
                                </div>
                                <div class="photos-map-foot">
                                    <span><strong>"Location"</strong><small>{loc_str}</small></span>
                                    <a
                                        class="text-button"
                                        href=osm_url
                                        target="_blank"
                                        rel="noopener noreferrer"
                                    >
                                        "Map"
                                        <Icon name="external"/>
                                    </a>
                                </div>
                            </div>
                        }
                    })}
                    <div class="photos-detail-section-row">
                        <h3 class="photos-detail-section">"Albums"</h3>
                        {can_edit_meta.then(|| view! {
                            <ActionMenu
                                label="Add to album"
                                actions=move |_| album_menu.run(asset_id_album2.clone())
                            >
                                <Icon name="plus"/>
                            </ActionMenu>
                        })}
                    </div>
                    {if !albums.is_empty() {
                        view! {
                            <div class="photos-chips">
                                {albums.into_iter().map(|a| {
                                    let aid = a.id;
                                    view! {
                                        <button
                                            type="button"
                                            class="photos-chip"
                                            on:click=move |_| { on_album.try_run(aid.clone()); }
                                        >
                                            {a.name}
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    } else {
                        view! { <p class="photos-hint">"Not in any album."</p> }.into_any()
                    }}
                    {show_in_files.then(|| view! {
                        <a class="secondary-button photos-in-files" href=show_files_href>
                            <Icon name="folder"/>"Show in Files"
                        </a>
                    })}
                </div>
            </div>
        }.into_any()
    }
}

/// Duplicates: groups of the same picture; keep some, the rest go to the trash with Undo.
#[component]
fn DuplicatesView(
    session: BrowserSession,
    on_notice: Callback<PhotosNotice>,
    on_changed: Callback<()>,
) -> impl IntoView {
    let groups: RwSignal<Option<Vec<PhotoDuplicateGroup>>> = RwSignal::new(None);
    let cursor = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let keep: RwSignal<HashMap<String, Vec<String>>> = RwSignal::new(HashMap::new());
    let busy = RwSignal::new(String::new());
    let session = StoredValue::new(session);

    let load = move |after: String| {
        error.set(String::new());
        spawn_local(async move {
            let url = if after.is_empty() {
                format!("{BASE}/duplicates")
            } else {
                format!("{BASE}/duplicates?cursor={}", encode_uri_component(&after))
            };
            match api::get::<PhotoDuplicatePage>(&url).await {
                Ok(page) => {
                    let new_groups = page.groups;
                    keep.try_update(|map| {
                        for g in &new_groups {
                            if !map.contains_key(&g.key)
                                && let Some(first) = g.items.first()
                            {
                                map.insert(g.key.clone(), vec![first.asset.id.clone()]);
                            }
                        }
                    });
                    groups.try_update(|cur| {
                        if after.is_empty() {
                            *cur = Some(new_groups);
                        } else if let Some(existing) = cur {
                            existing.extend(new_groups);
                        } else {
                            *cur = Some(new_groups);
                        }
                    });
                    cursor.try_set(page.cursor);
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
        });
    };
    load(String::new());

    let resolve = move |group: PhotoDuplicateGroup| {
        let chosen = keep.with_untracked(|m| m.get(&group.key).cloned().unwrap_or_default());
        if chosen.is_empty() {
            return;
        }
        busy.set(group.key.clone());
        let csrf_tok = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            match api::send::<TrashedResponse>(
                &format!("{BASE}/duplicates/resolve"),
                "POST",
                Some(json!({ "key": group.key, "keep": chosen })),
                &csrf_tok,
            )
            .await
            {
                Ok(res) => {
                    let trashed = res.trashed;
                    groups.try_update(|g| {
                        if let Some(list) = g {
                            list.retain(|x| x.key != group.key);
                        }
                    });
                    {
                        on_changed.try_run(());
                    };
                    let kept_n = chosen.len();
                    let trashed_n = trashed.len();
                    {
                        on_notice.try_run(PhotosNotice {
                            message: format!("Kept {kept_n}, moved {trashed_n} to the trash"),
                            undo: Some(Arc::new(move || {
                                let trashed = trashed.clone();
                                let csrf_tok = session.with_value(|s| s.csrf_token.clone());
                                spawn_local(async move {
                                    match api::send::<Value>(
                                        &format!("{BASE}/assets/restore"),
                                        "POST",
                                        Some(json!({ "ids": trashed })),
                                        &csrf_tok,
                                    )
                                    .await
                                    {
                                        Ok(_) => {
                                            on_changed.run(());
                                            load(String::new());
                                        }
                                        Err(cause) => on_notice.run(PhotosNotice {
                                            message: error_message(&cause.message),
                                            undo: None,
                                        }),
                                    }
                                });
                            })),
                        });
                    };
                }
                Err(cause) => {
                    {
                        on_notice.try_run(PhotosNotice {
                            message: error_message(&cause.message),
                            undo: None,
                        });
                    };
                    if cause.status == 409 {
                        load(String::new());
                    }
                }
            }
            busy.try_set(String::new());
        });
    };

    let resolve_all = move |_| {
        let all = groups.with_untracked(|g| g.clone().unwrap_or_default());
        if all.is_empty() {
            return;
        }
        busy.set("all".to_owned());
        let csrf_tok = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            let mut total_trashed = Vec::new();
            for group in all {
                let Some(first) = group.items.first() else {
                    continue;
                };
                if let Ok(res) = api::send::<TrashedResponse>(
                    &format!("{BASE}/duplicates/resolve"),
                    "POST",
                    Some(json!({ "key": group.key, "keep": [first.asset.id] })),
                    &csrf_tok,
                )
                .await
                {
                    total_trashed.extend(res.trashed);
                    groups.try_update(|g| {
                        if let Some(list) = g {
                            list.retain(|x| x.key != group.key);
                        }
                    });
                }
            }
            busy.try_set(String::new());
            on_changed.try_run(());
            if !total_trashed.is_empty() {
                let n = total_trashed.len();
                on_notice.try_run(PhotosNotice {
                    message: format!("Moved {n} duplicate{} to the trash", if n == 1 { "" } else { "s" }),
                    undo: Some(Arc::new(move || {
                        let trashed = total_trashed.clone();
                        let csrf_tok = session.with_value(|s| s.csrf_token.clone());
                        spawn_local(async move {
                            if api::send::<Value>(
                                &format!("{BASE}/assets/restore"),
                                "POST",
                                Some(json!({ "ids": trashed })),
                                &csrf_tok,
                            )
                            .await
                            .is_ok()
                            {
                                on_changed.run(());
                                load(String::new());
                            }
                        });
                    })),
                });
            }
        });
    };

    let dismiss = move |group: PhotoDuplicateGroup| {
        busy.set(group.key.clone());
        let csrf_tok = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            match api::send::<Value>(
                &format!("{BASE}/duplicates/dismiss"),
                "POST",
                Some(json!({ "key": group.key })),
                &csrf_tok,
            )
            .await
            {
                Ok(_) => {
                    groups.try_update(|g| {
                        if let Some(list) = g {
                            list.retain(|x| x.key != group.key);
                        }
                    });
                    {
                        on_changed.try_run(());
                    };
                    {
                        on_notice.try_run(PhotosNotice {
                            message: "Marked as not duplicates".to_owned(),
                            undo: None,
                        });
                    };
                }
                Err(cause) => {
                    {
                        on_notice.try_run(PhotosNotice {
                            message: error_message(&cause.message),
                            undo: None,
                        });
                    };
                }
            }
            busy.try_set(String::new());
        });
    };

    let toggle = move |group_key: String, item_id: String| {
        keep.update(|map| {
            let entry = map.entry(group_key).or_default();
            if let Some(pos) = entry.iter().position(|x| x == &item_id) {
                entry.remove(pos);
            } else {
                entry.push(item_id);
            }
        });
    };

    let reclaimable_bytes = Memo::new(move |_| {
        groups.with(|g| {
            let Some(list) = g else {
                return 0.0;
            };
            list.iter()
                .map(|grp| grp.items.iter().skip(1).map(|i| i.asset.size).sum::<f64>())
                .sum::<f64>()
        })
    });

    view! {
        <div class="photos-list-heading">
            <h1>"Duplicates"</h1>
            {move || groups.with(|g| g.as_ref().map(Vec::len)).map(|len| {
                let has_cursor = !cursor.get().is_empty();
                let label = if len == 1 && !has_cursor { "group" } else { "groups" };
                let save = reclaimable_bytes.get();
                view! {
                    <span class="photos-heading-count">
                        {len}{if has_cursor { "+" } else { "" }}" "{label}
                        {(save > 0.0).then(|| format!(" \u{00b7} Save {}", format_bytes(save)))}
                    </span>
                }
            })}
            <div class="photos-tools">
                {move || groups.with(|g| g.as_ref().is_some_and(|l| l.len() >= 2)).then(|| view! {
                    <button
                        type="button"
                        class="primary-button photos-head-cta"
                        disabled=move || !busy.get().is_empty()
                        on:click=resolve_all
                    >
                        {move || if busy.get() == "all" { "Cleaning\u{2026}" } else { "Keep best in all groups" }}
                    </button>
                })}
            </div>
        </div>
        <div class="photos-grid-scroll photos-duplicates">
            {move || (!error.get().is_empty()).then(|| view! {
                <div class="photos-empty" role="alert">
                    <h2>"Duplicates could not be loaded"</h2>
                    <p>{error.get()}</p>
                    <button class="secondary-button" on:click=move |_| load(String::new())>"Retry"</button>
                </div>
            })}
            {move || (groups.with(Option::is_none) && error.get().is_empty()).then(|| view! {
                <p class="photos-empty" role="status">"Looking for duplicates\u{2026}"</p>
            })}
            {move || groups.with(|g| g.as_ref().is_some_and(Vec::is_empty)).then(|| view! {
                <div class="photos-empty">
                    <Icon name="copy"/>
                    <h2>"No duplicates"</h2>
                    <p>"Copies of the same picture, even resized or re-saved, show up here so you can keep the best one."</p>
                </div>
            })}
            <For
                each=move || groups.with(|g| g.clone().unwrap_or_default())
                key=|g| g.key.clone()
                children=move |group| {
                    let key_for_chosen = group.key.clone();
                    let key_for_btn_dis = group.key.clone();
                    let key_for_btn_txt = group.key.clone();
                    let total = group.items.len();
                    let first_name = group.items.first().map(|i| i.asset.name.clone()).unwrap_or_default();
                    let region_label = format!("{total} copies of {first_name}");
                    let heading = if group.identical { "Identical files" } else { "Same picture" };
                    let g_dismiss = group.clone();
                    let g_resolve = group.clone();
                    view! {
                        <section class="photos-dup-group" aria-label=region_label>
                            <div class="photos-dup-head">
                                <strong>{heading}</strong>
                                <small>
                                    {total}" copies \u{00b7} keep "
                                    {move || keep.with(|m| m.get(&key_for_chosen).map(Vec::len).unwrap_or(0))}
                                </small>
                                <span class="photos-spacer"></span>
                                <button
                                    type="button"
                                    class="text-button"
                                    disabled=move || !busy.get().is_empty()
                                    on:click=move |_| dismiss(g_dismiss.clone())
                                >
                                    "Not duplicates"
                                </button>
                                <button
                                    type="button"
                                    class="primary-button"
                                    disabled=move || {
                                        let chosen_len = keep.with(|m| m.get(&key_for_btn_dis).map(Vec::len).unwrap_or(0));
                                        !busy.get().is_empty() || chosen_len == 0 || chosen_len == total
                                    }
                                    on:click=move |_| resolve(g_resolve.clone())
                                >
                                    {move || {
                                        if busy.get() == key_for_btn_txt {
                                            "Saving\u{2026}".to_owned()
                                        } else {
                                            let chosen_len = keep.with(|m| m.get(&key_for_btn_txt).map(Vec::len).unwrap_or(0));
                                            format!("Keep {chosen_len}, trash {}", total.saturating_sub(chosen_len))
                                        }
                                    }}
                                </button>
                            </div>
                            <ul class="photos-dup-items">
                                {group.items.into_iter().enumerate().map(|(idx, item)| {
                                    let gkey_cls = group.key.clone();
                                    let gkey_chk = group.key.clone();
                                    let gkey_tog = group.key.clone();
                                    let id_cls = item.asset.id.clone();
                                    let id_chk = item.asset.id.clone();
                                    let id_tog = item.asset.id.clone();
                                    let path_label = if item.path.is_empty() { "All files".to_owned() } else { item.path.clone() };
                                    let check_label = format!("Keep {} in {path_label}", item.asset.name);
                                    let dims = if item.asset.width > 0.0 {
                                        format!("{} \u{00d7} {} \u{00b7} ", item.asset.width, item.asset.height)
                                    } else {
                                        String::new()
                                    };
                                    let size_str = format_bytes(item.asset.size);
                                    let date_str = format!(
                                        "{}{}",
                                        local_date(item.asset.taken_at, &[("dateStyle", "medium")]),
                                        if item.asset.favorite { " \u{00b7} Favorite" } else { "" }
                                    );
                                    let name = item.asset.name.clone();
                                    let is_best = idx == 0;
                                    view! {
                                        <li class=move || if keep.with(|m| m.get(&gkey_cls).is_some_and(|l| l.contains(&id_cls))) { "is-kept" } else { "" }>
                                            <label>
                                                <span class="photos-dup-thumb"><Thumb asset=item.asset size="s"/></span>
                                                <span class="photos-dup-keep">
                                                    <input
                                                        type="checkbox"
                                                        prop:checked=move || keep.with(|m| m.get(&gkey_chk).is_some_and(|l| l.contains(&id_chk)))
                                                        on:change=move |_| toggle(gkey_tog.clone(), id_tog.clone())
                                                        aria-label=check_label
                                                    />
                                                    "Keep"
                                                </span>
                                                {is_best.then(|| view! { <span class="photos-dup-best">"Best copy"</span> })}
                                            </label>
                                            <strong>{name}</strong>
                                            <small>{dims}{size_str}</small>
                                            <small>{date_str}</small>
                                            <small class="photos-dup-path">{path_label}</small>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        </section>
                    }
                }
            />
            {move || (!cursor.get().is_empty()).then(|| view! {
                <div class="photos-more">
                    <button class="text-button" on:click=move |_| load(cursor.get_untracked())>"Load more"</button>
                </div>
            })}
            {move || groups.with(|g| g.as_ref().is_some_and(|l| !l.is_empty())).then(|| view! {
                <p class="photos-hint photos-dup-hint">
                    "Kept photos receive the albums, favorite and description of the copies you trash."
                </p>
            })}
        </div>
    }
}

#[component]
fn DeleteDialog(
    session: BrowserSession,
    ids: Vec<String>,
    empty: bool,
    locked: bool,
    on_close: Callback<()>,
    on_deleted: Callback<(Option<Vec<String>>, String)>,
    on_locked: Callback<()>,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let session = StoredValue::new(session);
    let ids = StoredValue::new(ids);

    let run = move |_| {
        busy.set(true);
        error.set(String::new());
        let csrf_tok = session.with_value(|s| s.csrf_token.clone());
        let ids_vec = ids.get_value();
        spawn_local(async move {
            let result: Result<(Option<Vec<String>>, String), RequestError> = if empty {
                async {
                    let mut total = 0usize;
                    for _ in 0..100 {
                        let r: EmptyTrashResponse =
                            api::send(&format!("{BASE}/trash/empty"), "POST", None, &csrf_tok)
                                .await?;
                        total += r.deleted;
                        if !r.remaining {
                            break;
                        }
                    }
                    Ok((None, format!("Deleted {} permanently", noun(total))))
                }
                .await
            } else {
                async {
                    let r: DeleteAssetsResponse = api::send(
                        &format!("{BASE}/assets/delete"),
                        "POST",
                        Some(json!({ "ids": ids_vec })),
                        &csrf_tok,
                    )
                    .await?;
                    Ok((
                        Some(r.deleted),
                        format!("Deleted {} permanently", noun(ids_vec.len())),
                    ))
                }
                .await
            };
            match result {
                Ok(done) => {
                    on_deleted.try_run(done);
                }
                Err(cause) if cause.status == 423 => {
                    on_locked.try_run(());
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                    busy.try_set(false);
                }
            }
        });
    };

    let title = if empty {
        "Empty the trash?"
    } else {
        "Delete permanently?"
    };
    let body_text = StoredValue::new(if empty {
        "Every photo and video in the trash is deleted from Files with its versions. This cannot be undone.".to_owned()
    } else {
        let len = ids.with_value(Vec::len);
        let items_phrase = if len == 1 {
            "This item is".to_owned()
        } else {
            format!("These {len} items are")
        };
        let poss = if len == 1 { "its" } else { "their" };
        let locked_part = if locked {
            ", without going through the trash"
        } else {
            ""
        };
        format!(
            "{items_phrase} deleted from Files with {poss} versions{locked_part}. This cannot be undone."
        )
    });

    view! {
        <Dialog title=title on_close=on_close>
            <p>{body_text.get_value()}</p>
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            <div class="dialog-actions">
                <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                <button type="button" class="danger-button" disabled=move || busy.get() on:click=run>
                    {move || if busy.get() { "Deleting\u{2026}" } else if empty { "Empty trash" } else { "Delete permanently" }}
                </button>
            </div>
        </Dialog>
    }
}

#[component]
fn AlbumDialog(
    album: Option<PhotoAlbum>,
    ids: Option<Vec<String>>,
    session: BrowserSession,
    on_close: Callback<()>,
    on_saved: Callback<(PhotoAlbum, bool)>,
) -> impl IntoView {
    let is_rename = album.is_some();
    let initial_name = album.as_ref().map(|a| a.name.clone()).unwrap_or_default();
    let name = RwSignal::new(initial_name);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let input = NodeRef::<html::Input>::new();
    let album_sv = StoredValue::new(album);
    let ids_sv = StoredValue::new(ids);
    let session_sv = StoredValue::new(session);

    Effect::new(move |_| {
        if let Some(el) = input.get() {
            let _ = el.focus();
        }
    });

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let trimmed = name.get_untracked().trim().to_owned();
        if trimmed.is_empty() {
            error.set("Enter an album name.".to_owned());
            return;
        }
        busy.set(true);
        error.set(String::new());
        let csrf_tok = session_sv.with_value(|s| s.csrf_token.clone());
        let album_opt = album_sv.get_value();
        let ids_opt = ids_sv.get_value();
        spawn_local(async move {
            let result = match album_opt {
                Some(a) => {
                    api::send::<PhotoAlbum>(
                        &format!("{BASE}/albums/{}", a.id),
                        "PATCH",
                        Some(json!({ "name": trimmed, "revision": a.revision })),
                        &csrf_tok,
                    )
                    .await
                }
                None => {
                    let mut body = serde_json::Map::new();
                    body.insert("name".to_owned(), json!(trimmed));
                    if let Some(list) = ids_opt
                        && !list.is_empty()
                    {
                        body.insert("ids".to_owned(), json!(list));
                    }
                    api::send::<PhotoAlbum>(
                        &format!("{BASE}/albums"),
                        "POST",
                        Some(Value::Object(body)),
                        &csrf_tok,
                    )
                    .await
                }
            };
            match result {
                Ok(saved) => {
                    on_saved.try_run((saved, !is_rename));
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                    busy.try_set(false);
                }
            }
        });
    };

    let title = if is_rename {
        "Rename album"
    } else {
        "New album"
    };
    let ids_count = ids_sv.with_value(|v| v.as_ref().map(Vec::len).unwrap_or(0));

    view! {
        <Dialog title=title on_close=on_close>
            <form on:submit=submit>
                <label class="field">
                    "Name"
                    <input
                        node_ref=input
                        autofocus
                        prop:value=move || name.get()
                        maxlength="200"
                        on:input=move |e| name.set(event_target_value(&e))
                        aria-invalid=move || (!error.get().is_empty()).to_string()
                    />
                </label>
                {(ids_count > 0).then(|| view! {
                    <p>{ids_count}" selected item"{if ids_count == 1 { "" } else { "s" }}" will be added."</p>
                })}
                {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions">
                    <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get()>
                        {move || if busy.get() { "Saving\u{2026}" } else if is_rename { "Rename" } else { "Create album" }}
                    </button>
                </div>
            </form>
        </Dialog>
    }
}

#[component]
fn ConfirmBody(
    album: PhotoAlbum,
    session: BrowserSession,
    on_close: Callback<()>,
    on_deleted: Callback<()>,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let album_id = StoredValue::new(album.id);
    let session = StoredValue::new(session);
    let count = album.count;

    let run = move |_| {
        busy.set(true);
        let id = album_id.get_value();
        let csrf_tok = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            match api::request::<Value>(
                &format!("{BASE}/albums/{id}"),
                Req {
                    method: "DELETE",
                    csrf: Some(&csrf_tok),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) => {
                    on_deleted.try_run(());
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                    busy.try_set(false);
                }
            }
        });
    };

    view! {
        <p>
            "The album is removed. Its "{format_number(count)}" photo"{if count == 1 { "" } else { "s" }}" stay in your library and in Files."
        </p>
        {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
        <div class="dialog-actions">
            <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
            <button type="button" class="danger-button" disabled=move || busy.get() on:click=run>
                {move || if busy.get() { "Deleting\u{2026}" } else { "Delete album" }}
            </button>
        </div>
    }
}

#[component]
fn PhotosSettings(
    session: BrowserSession,
    overview: Signal<Option<PhotoOverview>>,
    folder: LockedFolder,
    on_changed: Callback<()>,
    on_close: Callback<()>,
) -> impl IntoView {
    let section = RwSignal::new("General");
    let saver = Saver::new(session.clone());
    let draft: RwSignal<Option<String>> = RwSignal::new(None);
    let session = StoredValue::new(session);

    let saved = Memo::new(move |_| {
        saver
            .value("photosUploadFolder")
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                overview.with(|o| {
                    o.as_ref()
                        .map(|o| o.upload_folder.clone())
                        .filter(|s| !s.is_empty())
                })
            })
            .unwrap_or_else(|| "pics".to_owned())
    });
    let value = Signal::derive(move || draft.get().unwrap_or_else(|| saved.get()));
    let valid = Memo::new(move |_| photo_folder_parts(&value.get()).is_some());

    let save_folder = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let current = value.get_untracked();
        let Some(parts) = photo_folder_parts(&current) else {
            return;
        };
        let next = parts.join("/");
        draft.set(None);
        if next != saved.get_untracked() {
            saver.change(
                "photosUploadFolder",
                json!(next),
                format!("Uploads go to {next}"),
                true,
            );
            spawn_local(async move {
                crate::sleep(800).await;
                {
                    on_changed.try_run(());
                };
            });
        }
    };

    let none_state = Signal::derive(|| None);
    let folder_state = saver.state("photosUploadFolder");

    view! {
        <SettingsSheet
            title="Photos settings"
            icon="image"
            sections=vec!["General", "Privacy", "Immich", "Keyboard shortcuts"]
            section=section
            on_close=on_close
            notice=saver.notice
        >
            <SaveProblems saver=saver/>
            {move || match section.get() {
                "General" => {
                    let saved_now = saved.get();
                    let upload_hint = format!(
                        "Uploads go to Files \u{203a} {saved_now} \u{203a} year \u{203a} month. Same-named files are kept with a number."
                    );
                    view! {
                        <section class="settings-group" aria-label="Library">
                            <h3>"Library"</h3>
                            <SettingRow
                                label="Where photos come from"
                                hint="Every picture and video you own in Files, including RAW, HEIC and videos. Shared files from others are not included."
                                state=none_state
                                stacked=true
                            >
                                <a class="secondary-button" href="/">"Open Files"</a>
                            </SettingRow>
                            <SettingRow
                                label="Upload folder"
                                hint=upload_hint
                                state=folder_state
                                stacked=true
                            >
                                <form class="photos-folder-form" on:submit=save_folder>
                                    <label>
                                        <span class="sr-only">"Upload folder"</span>
                                        <input
                                            prop:value=move || value.get()
                                            maxlength="500"
                                            on:input=move |e| draft.set(Some(event_target_value(&e)))
                                            aria-invalid=move || (!valid.get()).to_string()
                                            aria-describedby=move || (!valid.get()).then_some("photos-folder-error")
                                        />
                                    </label>
                                    <button
                                        class="secondary-button"
                                        disabled=move || !valid.get() || value.get() == saved.get() || saver.preferences.with(Option::is_none)
                                    >
                                        "Save"
                                    </button>
                                </form>
                                {move || (!valid.get()).then(|| view! {
                                    <p id="photos-folder-error" class="field-error" role="alert">
                                        "Use folder names separated by /, like pics or Media/Camera."
                                    </p>
                                })}
                            </SettingRow>
                            <SettingRow
                                label="Indexing"
                                hint="New and changed files are picked up in the background"
                                state=none_state
                            >
                                {move || if overview.with(|o| o.as_ref().is_some_and(|o| o.indexing)) {
                                    view! {
                                        <span class="photos-indexing">
                                            <span class="photos-spinner" aria-hidden="true"></span>"Working\u{2026}"
                                        </span>
                                    }.into_any()
                                } else {
                                    view! { <span class="photos-hint">"Up to date"</span> }.into_any()
                                }}
                            </SettingRow>
                        </section>
                        <section class="settings-group" aria-label="Media">
                            <h3>"Media"</h3>
                            <SettingRow
                                label="Videos"
                                hint="Videos your browser can\u{2019}t play (HEVC, MKV and others) are converted to H.264 the first time you open them. Converted copies are kept in the cache and can be deleted."
                                state=none_state
                                stacked=true
                            >
                                <span class="photos-hint">"Automatic"</span>
                            </SettingRow>
                            <SettingRow
                                label="Live Photos, stacks and duplicates"
                                hint="A still and its short MOV appear as one Live Photo. RAW+JPEG pairs and bursts stack under one cover. Duplicates finds copies of the same picture."
                                state=none_state
                                stacked=true
                            >
                                <span class="photos-hint">"Automatic"</span>
                            </SettingRow>
                        </section>
                    }.into_any()
                }
                "Privacy" => {
                    let sess = session.get_value();
                    view! {
                        <section class="settings-group" aria-label="Locked folder and trash">
                            <h3>"Locked folder and trash"</h3>
                            <SettingRow
                                label="Locked folder"
                                hint="Locked photos live in Files \u{203a} Locked folder. Your password opens it for 10 minutes on this device. They are hidden from the library, albums, search, sharing and WebDAV."
                                state=none_state
                                stacked=true
                            >
                                {move || if folder.unlocked() {
                                    view! {
                                        <button
                                            type="button"
                                            class="secondary-button"
                                            on:click=move |_| {
                                                spawn_local(async move {
                                                    let _ = folder.lock().await;
                                                });
                                            }
                                        >
                                            <Icon name="lock"/>"Lock now"
                                        </button>
                                    }.into_any()
                                } else {
                                    view! { <span class="photos-hint">"Locked"</span> }.into_any()
                                }}
                            </SettingRow>
                            <SettingRow
                                label="Trash"
                                hint="The trash is Files \u{203a} Deleted files. Nothing is removed until you delete it permanently or empty the trash. Deleting in the Locked folder is permanent."
                                state=none_state
                                stacked=true
                            >
                                <span class="photos-hint">"Kept until deleted"</span>
                            </SettingRow>
                        </section>
                        <section class="settings-group" aria-label="Shared links">
                            <h3>"Shared links"</h3>
                            <LinksList session=sess/>
                        </section>
                    }.into_any()
                }
                "Immich" => {
                    let sess = session.get_value();
                    view! { <ImmichSettings session=sess/> }.into_any()
                }
                "Keyboard shortcuts" => view! {
                    <dl class="ui-shortcuts">
                        <dt>"Shift click"</dt><dd>"Select a range of photos"</dd>
                        <dt>"\u{2190} / \u{2192}"</dt><dd>"Previous or next photo"</dd>
                        <dt>"f"</dt><dd>"Favorite or unfavorite"</dd>
                        <dt>"i"</dt><dd>"Show or hide photo details"</dd>
                        <dt>"s"</dt><dd>"Start or pause slideshow"</dd>
                        <dt>"+ / - / 0"</dt><dd>"Zoom in, zoom out, or reset zoom"</dd>
                        <dt>"Delete"</dt><dd>"Move the open photo to the trash"</dd>
                        <dt>"Escape"</dt><dd>"Close the photo"</dd>
                        <dt>"Ctrl / Cmd K"</dt><dd>"Search all apps"</dd>
                    </dl>
                }.into_any(),
                _ => view! { <span></span> }.into_any(),
            }}
        </SettingsSheet>
    }
}
