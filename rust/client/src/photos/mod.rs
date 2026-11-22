//! Photos (`photos/workspace.tsx`, `sharing.tsx`, `public.tsx`, `immich-settings.tsx`):
//! month-grouped timeline, albums, sharing, viewer with details, duplicates, Locked folder,
//! Immich settings and the public shared-link page.

pub mod immich_settings;
pub mod public;
pub mod sharing;
pub mod workspace;

pub use public::PublicPhotos;
pub use workspace::PhotosWorkspace;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::ui::window;

pub(crate) const BASE: &str = "/api/v1/photos";

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoExif {
    #[serde(default)]
    pub make: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub lens: Option<String>,
    #[serde(default)]
    pub f_number: Option<f64>,
    #[serde(default)]
    pub exposure: Option<f64>,
    #[serde(default)]
    pub iso: Option<f64>,
    #[serde(default)]
    pub focal_length: Option<f64>,
    #[serde(default)]
    pub orientation: Option<i64>,
    #[serde(default)]
    pub video_codec: Option<String>,
    #[serde(default)]
    pub audio_codec: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoStackRef {
    pub id: String,
    #[serde(default)]
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoAsset {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub media_type: String,
    #[serde(default)]
    pub size: f64,
    #[serde(default)]
    pub taken_at: f64,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub thumbnail: String,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub playback: Option<String>,
    #[serde(default)]
    pub video_codec: Option<String>,
    #[serde(default)]
    pub live: Option<String>,
    #[serde(default)]
    pub stack: Option<PhotoStackRef>,
    #[serde(default)]
    pub trashed_at: Option<f64>,
    #[serde(default)]
    pub owner: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumRef {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoDetail {
    #[serde(flatten)]
    pub asset: PhotoAsset,
    #[serde(default)]
    pub checksum: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub exif: PhotoExif,
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub longitude: Option<f64>,
    #[serde(default)]
    pub albums: Vec<AlbumRef>,
    #[serde(default)]
    pub parent_id: String,
    #[serde(default)]
    pub modified_at: f64,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub stack_id: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumOwner {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoAlbum {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub cover_id: String,
    #[serde(default)]
    pub updated_at: f64,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub owner: Option<AlbumOwner>,
    #[serde(default)]
    pub shared: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoPage {
    #[serde(default)]
    pub items: Vec<PhotoAsset>,
    #[serde(default)]
    pub cursor: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoStack {
    pub id: String,
    pub primary_id: String,
    #[serde(default)]
    pub items: Vec<PhotoAsset>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoDuplicateItem {
    #[serde(flatten)]
    pub asset: PhotoAsset,
    #[serde(default)]
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoDuplicateGroup {
    pub key: String,
    #[serde(default)]
    pub identical: bool,
    #[serde(default)]
    pub items: Vec<PhotoDuplicateItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoDuplicatePage {
    #[serde(default)]
    pub groups: Vec<PhotoDuplicateGroup>,
    #[serde(default)]
    pub cursor: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoCounts {
    #[serde(default)]
    pub library: u64,
    #[serde(default)]
    pub folder: u64,
    #[serde(default)]
    pub favorites: u64,
    #[serde(default)]
    pub videos: u64,
    #[serde(default)]
    pub archive: u64,
    #[serde(default)]
    pub trash: u64,
    #[serde(default)]
    pub duplicates: u64,
    #[serde(default)]
    pub shared: u64,
}

impl PhotoCounts {
    pub fn get(&self, view: &str) -> Option<u64> {
        match view {
            "library" => Some(self.library),
            "folder" => Some(self.folder),
            "favorites" => Some(self.favorites),
            "videos" => Some(self.videos),
            "archive" => Some(self.archive),
            "trash" => Some(self.trash),
            "duplicates" => Some(self.duplicates),
            "shared" => Some(self.shared),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoImmichProgress {
    pub phase: String,
    #[serde(default)]
    pub done: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub imported: u64,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub started_at: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoOverview {
    #[serde(default)]
    pub albums: Vec<PhotoAlbum>,
    #[serde(default)]
    pub shared_albums: Vec<PhotoAlbum>,
    #[serde(default)]
    pub counts: PhotoCounts,
    #[serde(default)]
    pub indexing: bool,
    #[serde(default)]
    pub upload_folder: String,
    #[serde(default)]
    pub immich: Option<PhotoImmichProgress>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoMoved {
    pub id: String,
    pub parent_id: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoAlbumMember {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    pub role: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoLink {
    pub id: String,
    #[serde(default)]
    pub album_id: String,
    #[serde(default)]
    pub album_name: String,
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub expires_at: f64,
    #[serde(default)]
    pub has_password: bool,
    #[serde(default)]
    pub allow_download: bool,
    #[serde(default)]
    pub allow_upload: bool,
    #[serde(default)]
    pub show_metadata: bool,
    #[serde(default)]
    pub created_at: f64,
    #[serde(default)]
    pub revision: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoAlbumSharing {
    #[serde(default)]
    pub members: Vec<PhotoAlbumMember>,
    #[serde(default)]
    pub links: Vec<PhotoLink>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoPublicShare {
    pub needs_password: bool,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub album: bool,
    #[serde(default)]
    pub expires_at: f64,
    #[serde(default)]
    pub allow_download: bool,
    #[serde(default)]
    pub allow_upload: bool,
    #[serde(default)]
    pub show_metadata: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoPublicDetail {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub taken_at: Option<f64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub exif: Option<PhotoExif>,
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub longitude: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playback {
    pub state: String,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Transfer {
    pub id: String,
    pub name: String,
    pub progress: f64,
    pub status: &'static str,
    pub message: Option<String>,
}

#[derive(Clone)]
pub(crate) struct PhotosNotice {
    pub message: String,
    pub undo: Option<Arc<dyn Fn() + Send + Sync>>,
}

pub(crate) fn thumb_url(id: &str, size: &str) -> String {
    if size == "m" {
        format!("{BASE}/assets/{id}/thumbnail?size=m")
    } else {
        format!("{BASE}/assets/{id}/thumbnail")
    }
}

pub(crate) fn content_url(id: &str) -> String {
    format!("/api/v1/files/{id}/content?preview=1")
}

pub(crate) fn format_duration(seconds: f64) -> String {
    let s = seconds.round().max(0.0) as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let r = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{r:02}")
    } else {
        format!("{m}:{r:02}")
    }
}

pub(crate) fn format_duration_short(seconds: f64) -> String {
    let s = seconds.round().max(0.0) as u64;
    let m = s / 60;
    let r = s % 60;
    format!("{m}:{r:02}")
}

pub(crate) fn format_number(n: u64) -> String {
    let num = JsValue::from_f64(n as f64);
    if let Ok(func) = js_sys::Reflect::get(&num, &JsValue::from_str("toLocaleString"))
        && let Some(func) = func.dyn_ref::<js_sys::Function>()
        && let Ok(val) = func.call0(&num)
        && let Some(s) = val.as_string()
    {
        return s;
    }
    n.to_string()
}

pub(crate) fn noun(n: usize) -> String {
    format!("{n} item{}", if n == 1 { "" } else { "s" })
}

pub(crate) fn error_message(msg: &str) -> String {
    if msg.is_empty() {
        "The request could not be completed.".to_owned()
    } else {
        msg.to_owned()
    }
}

/// A Files folder path like "pics" or "Media/Camera": 1–8 names, no dots-only or control characters.
pub(crate) fn photo_folder_parts(value: &str) -> Option<Vec<String>> {
    if value.encode_utf16().count() > 500 {
        return None;
    }
    let parts: Vec<String> = value
        .split('/')
        .map(|part| {
            let nfc: String = js_sys::JsString::from(part).normalize("NFC").into();
            nfc.trim().to_owned()
        })
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty()
        || parts.len() > 8
        || parts.iter().any(|part| {
            part.encode_utf16().count() > 255
                || part == "."
                || part == ".."
                || part.chars().any(|c| (c as u32) <= 0x1f || c == '\\')
        })
    {
        return None;
    }
    Some(parts)
}

/// Shared-link passwords: 4–200 characters.
pub(crate) fn valid_link_password(value: &str) -> bool {
    value.chars().count() >= 4 && value.encode_utf16().count() <= 200
}

pub(crate) fn encode_uri_component(value: &str) -> String {
    js_sys::encode_uri_component(value).as_string().unwrap_or_else(|| value.to_owned())
}

pub(crate) fn css_escape(value: &str) -> String {
    if let Ok(css) = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("CSS"))
        && let Ok(escape) = js_sys::Reflect::get(&css, &JsValue::from_str("escape"))
        && let Some(func) = escape.dyn_ref::<js_sys::Function>()
        && let Ok(result) = func.call1(&css, &JsValue::from_str(value))
        && let Some(s) = result.as_string()
    {
        return s;
    }
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn file_last_modified(file: &web_sys::File) -> f64 {
    let lm = js_sys::Reflect::get(file, &JsValue::from_str("lastModified"))
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    if lm > 0.0 { lm } else { js_sys::Date::now() }
}

pub(crate) async fn copy_to_clipboard(text: &str) -> Result<(), ()> {
    let nav = window().navigator();
    let clip = js_sys::Reflect::get(&nav, &JsValue::from_str("clipboard")).map_err(|_| ())?;
    if clip.is_undefined() || clip.is_null() {
        return Err(());
    }
    let write = js_sys::Reflect::get(&clip, &JsValue::from_str("writeText"))
        .map_err(|_| ())?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| ())?;
    let promise: js_sys::Promise = write
        .call1(&clip, &JsValue::from_str(text))
        .map_err(|_| ())?
        .dyn_into()
        .map_err(|_| ())?;
    wasm_bindgen_futures::JsFuture::from(promise).await.map_err(|_| ())?;
    Ok(())
}

pub(crate) fn request_animation_frame(f: impl FnOnce() + 'static) {
    let cb = Closure::once_into_js(f);
    let _ = window().request_animation_frame(cb.unchecked_ref());
}

/// Sends `file` via `XMLHttpRequest` PUT with upload progress, returning the response text on 2xx.
pub(crate) async fn put_upload_file(
    url: &str,
    csrf: Option<&str>,
    file: &web_sys::File,
    progress: impl Fn(f64) + 'static,
) -> Result<String, String> {
    let xhr = web_sys::XmlHttpRequest::new().map_err(|_| "The upload could not be completed.".to_owned())?;
    let (sender, receiver) = futures::channel::oneshot::channel::<Result<String, String>>();
    let sender = Rc::new(RefCell::new(Some(sender)));
    let finish = {
        let sender = sender.clone();
        move |result: Result<String, String>| {
            if let Some(s) = sender.borrow_mut().take() {
                let _ = s.send(result);
            }
        }
    };
    let _ = xhr.open("PUT", url);
    if let Some(token) = csrf {
        let _ = xhr.set_request_header("x-csrf-token", token);
    }
    let on_progress = Closure::<dyn FnMut(web_sys::ProgressEvent)>::new(move |e: web_sys::ProgressEvent| {
        progress(if e.length_computable() && e.total() > 0.0 {
            e.loaded() / e.total()
        } else {
            0.0
        });
    });
    if let Ok(upload) = xhr.upload() {
        upload.set_onprogress(Some(on_progress.as_ref().unchecked_ref()));
    }
    let on_load = {
        let (xhr, finish) = (xhr.clone(), finish.clone());
        Closure::<dyn FnMut()>::new(move || {
            let status = xhr.status().unwrap_or(0);
            let text = xhr.response_text().ok().flatten().unwrap_or_default();
            if (200..300).contains(&status) {
                finish(Ok(text));
            } else {
                let message = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "The upload could not be completed.".to_owned());
                finish(Err(message));
            }
        })
    };
    let on_error = {
        let finish = finish.clone();
        Closure::<dyn FnMut()>::new(move || {
            finish(Err("The connection was interrupted. Try the upload again.".to_owned()));
        })
    };
    xhr.set_onload(Some(on_load.as_ref().unchecked_ref()));
    xhr.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    let _ = xhr.send_with_opt_blob(Some(file));
    let result = receiver.await.unwrap_or_else(|_| Err("The upload could not be completed.".to_owned()));
    drop((on_progress, on_load, on_error));
    result
}
