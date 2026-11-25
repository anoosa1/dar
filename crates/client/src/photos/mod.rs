//! Photos (`photos/workspace.tsx`, `sharing.tsx`, `public.tsx`, `immich-settings.tsx`):
//! month-grouped timeline, albums, sharing, viewer with details, duplicates, Locked folder,
//! Immich settings and the public shared-link page.

pub mod explore;
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
    #[serde(default)]
    pub min_taken_at: f64,
    #[serde(default)]
    pub max_taken_at: f64,
    #[serde(default)]
    pub member_count: u64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoMemory {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    #[serde(default)]
    pub years_ago: i64,
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub cover_id: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoPlace {
    pub key: String,
    #[serde(default)]
    pub latitude: f64,
    #[serde(default)]
    pub longitude: f64,
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub cover_id: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoCamera {
    pub name: String,
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub cover_id: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoFolder {
    pub id: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub cover_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoExplore {
    #[serde(default)]
    pub places: Vec<PhotoPlace>,
    #[serde(default)]
    pub cameras: Vec<PhotoCamera>,
    #[serde(default)]
    pub folders: Vec<PhotoFolder>,
    #[serde(default)]
    pub total_bytes: f64,
    #[serde(default)]
    pub unassigned_count: u64,
    #[serde(default)]
    pub large_count: u64,
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
    #[serde(default)]
    pub total_bytes: f64,
    #[serde(default)]
    pub memories: Vec<PhotoMemory>,
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
    js_sys::encode_uri_component(value)
        .as_string()
        .unwrap_or_else(|| value.to_owned())
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
    wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|_| ())?;
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
    let xhr = web_sys::XmlHttpRequest::new()
        .map_err(|_| "The upload could not be completed.".to_owned())?;
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
    let on_progress =
        Closure::<dyn FnMut(web_sys::ProgressEvent)>::new(move |e: web_sys::ProgressEvent| {
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
            finish(Err(
                "The connection was interrupted. Try the upload again.".to_owned()
            ));
        })
    };
    xhr.set_onload(Some(on_load.as_ref().unchecked_ref()));
    xhr.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    let _ = xhr.send_with_opt_blob(Some(file));
    let result = receiver
        .await
        .unwrap_or_else(|_| Err("The upload could not be completed.".to_owned()));
    drop((on_progress, on_load, on_error));
    result
}

/// Builds a multi-file ZIP via `/api/v1/files/archive` and triggers a browser download.
pub(crate) async fn download_zip_archive(
    ids: Vec<String>,
    filename: &str,
    csrf: &str,
) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let response = crate::api::fetch(
        "/api/v1/files/archive",
        crate::api::Req {
            method: "POST",
            body: Some(serde_json::json!({ "ids": ids }).to_string()),
            csrf: Some(csrf),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| error_message(&e.message))?;
    if !response.ok() {
        return Err("Could not build the ZIP archive.".to_owned());
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
    if let Ok(link) = crate::ui::document().create_element("a") {
        let link: web_sys::HtmlAnchorElement = link.unchecked_into();
        link.set_href(&url);
        link.set_download(filename);
        if let Some(body) = crate::ui::document().body() {
            let _ = body.append_child(&link);
        }
        link.click();
        link.remove();
    }
    let revoke_url = url.clone();
    leptos::prelude::set_timeout(
        move || {
            let _ = web_sys::Url::revoke_object_url(&revoke_url);
        },
        std::time::Duration::from_secs(60),
    );
    Ok(())
}

/// Renders an image with rotation (0/90/180/270) and CSS filter onto an offscreen canvas,
/// uploads the resulting JPEG copy into the same Files folder (or Photos upload target),
/// indexes it, and returns the new asset ID.
pub(crate) async fn save_edited_copy(
    asset: &PhotoAsset,
    rotation_deg: i32,
    css_filter: &str,
    csrf: &str,
) -> Result<String, String> {
    let img_url = format!("{BASE}/assets/{}/original", asset.id);
    let response = crate::api::fetch(
        &img_url,
        crate::api::Req {
            method: "GET",
            ..Default::default()
        },
    )
    .await
    .map_err(|e| error_message(&e.message))?;
    if !response.ok() {
        return Err("Could not load original photo for editing.".to_owned());
    }
    let blob_js = wasm_bindgen_futures::JsFuture::from(
        response
            .blob()
            .map_err(|_| "Could not read original photo.".to_owned())?,
    )
    .await
    .map_err(|_| "Could not read original photo.".to_owned())?;
    let src_blob: web_sys::Blob = blob_js.unchecked_into();
    let obj_url = web_sys::Url::create_object_url_with_blob(&src_blob)
        .map_err(|_| "Could not decode photo.".to_owned())?;

    let img = web_sys::HtmlImageElement::new().map_err(|_| "Could not create image.".to_owned())?;
    let (tx, rx) = futures::channel::oneshot::channel::<Result<(), String>>();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let on_load = {
        let tx = tx.clone();
        Closure::once_into_js(move || {
            if let Some(s) = tx.borrow_mut().take() {
                let _ = s.send(Ok(()));
            }
        })
    };
    let on_err = {
        let tx = tx.clone();
        Closure::once_into_js(move || {
            if let Some(s) = tx.borrow_mut().take() {
                let _ = s.send(Err("Browser could not decode this image format.".to_owned()));
            }
        })
    };
    img.set_onload(Some(on_load.unchecked_ref()));
    img.set_onerror(Some(on_err.unchecked_ref()));
    img.set_src(&obj_url);
    let load_res = rx
        .await
        .unwrap_or_else(|_| Err("Image decode cancelled.".to_owned()));
    let _ = web_sys::Url::revoke_object_url(&obj_url);
    load_res?;

    let nat_w = img.natural_width();
    let nat_h = img.natural_height();
    if nat_w == 0 || nat_h == 0 {
        return Err("Image has zero dimensions.".to_owned());
    }
    let norm_rot = ((rotation_deg % 360) + 360) % 360;
    let swap = norm_rot == 90 || norm_rot == 270;
    let canvas_w = if swap { nat_h } else { nat_w };
    let canvas_h = if swap { nat_w } else { nat_h };

    let canvas: web_sys::HtmlCanvasElement = crate::ui::document()
        .create_element("canvas")
        .map_err(|_| "Could not create canvas.".to_owned())?
        .unchecked_into();
    canvas.set_width(canvas_w);
    canvas.set_height(canvas_h);
    let ctx: web_sys::CanvasRenderingContext2d = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .ok_or_else(|| "Canvas 2D unavailable.".to_owned())?
        .unchecked_into();
    if !css_filter.is_empty() && css_filter != "none" {
        let _ = js_sys::Reflect::set(
            &ctx,
            &JsValue::from_str("filter"),
            &JsValue::from_str(css_filter),
        );
    }
    let _ = ctx.translate(canvas_w as f64 / 2.0, canvas_h as f64 / 2.0);
    let _ = ctx.rotate((norm_rot as f64).to_radians());
    ctx.draw_image_with_html_image_element(&img, -(nat_w as f64) / 2.0, -(nat_h as f64) / 2.0)
        .map_err(|_| "Could not render edited photo.".to_owned())?;

    let (btx, brx) = futures::channel::oneshot::channel::<Option<web_sys::Blob>>();
    let btx = Rc::new(RefCell::new(Some(btx)));
    let blob_cb = Closure::once_into_js(move |b: JsValue| {
        if let Some(s) = btx.borrow_mut().take() {
            let _ = s.send(b.dyn_into::<web_sys::Blob>().ok());
        }
    });
    canvas
        .to_blob_with_type_and_encoder_options(
            blob_cb.unchecked_ref(),
            "image/jpeg",
            &JsValue::from_f64(0.92),
        )
        .map_err(|_| "Could not encode edited photo.".to_owned())?;
    let out_blob = brx
        .await
        .ok()
        .flatten()
        .ok_or_else(|| "Could not encode edited photo.".to_owned())?;

    let when = js_sys::Date::new(&JsValue::from_f64(if asset.taken_at > 0.0 {
        asset.taken_at
    } else {
        js_sys::Date::now()
    }));
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TargetRes {
        parent_id: String,
    }
    #[derive(Deserialize)]
    struct IdRes {
        id: String,
    }
    let parent_id = match crate::api::get::<PhotoDetail>(&format!("{BASE}/assets/{}", asset.id)).await
    {
        Ok(det) if !det.parent_id.is_empty() => det.parent_id,
        _ => {
            let t: TargetRes = crate::api::send(
                &format!("{BASE}/upload-target"),
                "POST",
                Some(serde_json::json!({
                    "year": when.get_full_year(),
                    "month": when.get_month() + 1,
                })),
                csrf,
            )
            .await
            .map_err(|e| error_message(&e.message))?;
            t.parent_id
        }
    };

    let stem = match asset.name.rfind('.') {
        Some(dot) if dot > 0 => &asset.name[..dot],
        _ => asset.name.as_str(),
    };
    let mut upload_id = String::new();
    let mut attempt = 0u32;
    while upload_id.is_empty() {
        let candidate = if attempt == 0 {
            format!("{stem} (edited).jpg")
        } else {
            format!("{stem} (edited {}).jpg", attempt + 1)
        };
        match crate::api::send::<IdRes>(
            "/api/v1/files/uploads",
            "POST",
            Some(serde_json::json!({
                "parentId": parent_id,
                "name": candidate,
                "size": out_blob.size(),
                "mediaType": "image/jpeg",
            })),
            csrf,
        )
        .await
        {
            Ok(res) => upload_id = res.id,
            Err(cause) if cause.status == 409 && attempt < 20 => attempt += 1,
            Err(cause) => return Err(error_message(&cause.message)),
        }
    }

    // Use XHR with Blob so the exact binary JPEG bytes are uploaded cleanly.
    let xhr = web_sys::XmlHttpRequest::new()
        .map_err(|_| "Could not start upload.".to_owned())?;
    let (utx, urx) = futures::channel::oneshot::channel::<Result<String, String>>();
    let utx = Rc::new(RefCell::new(Some(utx)));
    let _ = xhr.open("PUT", &format!("/api/v1/files/uploads/{upload_id}"));
    let _ = xhr.set_request_header("x-csrf-token", csrf);
    let u_load = {
        let xhr = xhr.clone();
        let utx = utx.clone();
        Closure::once_into_js(move || {
            let status = xhr.status().unwrap_or(0);
            let text = xhr.response_text().ok().flatten().unwrap_or_default();
            if let Some(s) = utx.borrow_mut().take() {
                if (200..300).contains(&status) {
                    let _ = s.send(Ok(text));
                } else {
                    let _ = s.send(Err("Could not save edited photo.".to_owned()));
                }
            }
        })
    };
    let u_err = {
        let utx = utx.clone();
        Closure::once_into_js(move || {
            if let Some(s) = utx.borrow_mut().take() {
                let _ = s.send(Err("Upload interrupted.".to_owned()));
            }
        })
    };
    xhr.set_onload(Some(u_load.unchecked_ref()));
    xhr.set_onerror(Some(u_err.unchecked_ref()));
    let _ = xhr.send_with_opt_blob(Some(&out_blob));
    let text = urx
        .await
        .unwrap_or_else(|_| Err("Upload cancelled.".to_owned()))?;
    let uploaded: IdRes =
        serde_json::from_str(&text).map_err(|_| "Could not save edited photo.".to_owned())?;
    let _ = crate::api::send::<serde_json::Value>(
        &format!("{BASE}/assets/{}/index", uploaded.id),
        "POST",
        None,
        csrf,
    )
    .await;
    Ok(uploaded.id)
}
