//! Folder uploads: a folder picked with the directory picker (`webkitRelativePath`) or dropped
//! (the entries API), flattened into the folders to create, parents first, and each file with
//! the folder it goes in.

use js_sys::{Function, Promise, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Folders (relative paths, parents before children) and files with their folder's path.
pub struct Tree {
    pub folders: Vec<String>,
    pub files: Vec<(web_sys::File, String)>,
}

/// Every folder a set of folder paths needs, parents first.
pub fn folders_for<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    for path in paths {
        let mut at = String::new();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            at = if at.is_empty() { part.to_owned() } else { format!("{at}/{part}") };
            if !all.contains(&at) {
                all.push(at.clone());
            }
        }
    }
    all.sort_by_key(|p| p.matches('/').count());
    all
}

fn folder_of(path: &str) -> String {
    path.rfind('/').map_or(String::new(), |i| path[..i].to_owned())
}

/// Files picked with `webkitdirectory`: their relative paths name the folders.
pub fn from_picker(files: Vec<web_sys::File>) -> Tree {
    let files: Vec<(web_sys::File, String)> = files
        .into_iter()
        .map(|f| {
            let relative = Reflect::get(&f, &"webkitRelativePath".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
            let dir = folder_of(&relative);
            (f, dir)
        })
        .collect();
    let folders = folders_for(files.iter().map(|(_, d)| d.as_str()));
    Tree { folders, files }
}

/// The dropped items' entries, read while the drop event runs (they are gone afterwards).
/// Empty when nothing dropped is a folder, so plain file drops keep their usual path.
pub fn dropped_entries(data: &web_sys::DataTransfer) -> Vec<JsValue> {
    let Ok(items) = Reflect::get(data, &"items".into()) else { return Vec::new() };
    let length = Reflect::get(&items, &"length".into()).ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as u32;
    let mut entries = Vec::new();
    for i in 0..length {
        let Ok(item) = Reflect::get(&items, &i.into()) else { continue };
        let Some(get) = Reflect::get(&item, &"webkitGetAsEntry".into()).ok().and_then(|f| f.dyn_into::<Function>().ok()) else { continue };
        if let Ok(entry) = get.call0(&item)
            && !entry.is_null()
        {
            entries.push(entry);
        }
    }
    let any_folder = entries.iter().any(|e| Reflect::get(e, &"isDirectory".into()).ok().and_then(|v| v.as_bool()).unwrap_or(false));
    if any_folder { entries } else { Vec::new() }
}

/// Calls `target.method(onSuccess, onError)` and waits for the callback.
async fn callback(target: &JsValue, method: &str) -> Result<JsValue, JsValue> {
    let function: Function = Reflect::get(target, &method.into())?.dyn_into()?;
    let target = target.clone();
    let promise = Promise::new(&mut |resolve, reject| {
        if let Err(e) = function.call2(&target, &resolve, &reject) {
            let _ = reject.call1(&JsValue::NULL, &e);
        }
    });
    JsFuture::from(promise).await
}

/// Walks dropped entries (folders recursively) into a tree; empty folders are kept.
pub async fn walk(entries: Vec<JsValue>) -> Result<Tree, String> {
    let failed = |_| "A dropped folder could not be read.".to_owned();
    let mut files = Vec::new();
    let mut folders = Vec::new();
    let mut queue: Vec<(JsValue, String)> = entries.into_iter().map(|e| (e, String::new())).collect();
    while let Some((entry, parent)) = queue.pop() {
        let name = Reflect::get(&entry, &"name".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
        if Reflect::get(&entry, &"isDirectory".into()).ok().and_then(|v| v.as_bool()).unwrap_or(false) {
            let path = if parent.is_empty() { name } else { format!("{parent}/{name}") };
            folders.push(path.clone());
            let reader = Reflect::get(&entry, &"createReader".into()).and_then(|f| f.dyn_into::<Function>().map_err(JsValue::from)).and_then(|f| f.call0(&entry)).map_err(failed)?;
            // readEntries answers in batches until it returns none.
            loop {
                let batch: js_sys::Array = callback(&reader, "readEntries").await.map_err(failed)?.dyn_into().map_err(failed)?;
                if batch.length() == 0 {
                    break;
                }
                queue.extend(batch.iter().map(|e| (e, path.clone())));
            }
        } else {
            let file: web_sys::File = callback(&entry, "file").await.map_err(failed)?.dyn_into().map_err(failed)?;
            files.push((file, parent));
        }
    }
    let folders = folders_for(folders.iter().map(String::as_str));
    Ok(Tree { folders, files })
}

/// Path shown in the Uploads panel for a file in `dir`, substituting the created top-level folder
/// name when a collision renamed it (e.g. `Trip` -> `Trip 2`).
pub fn display_path(dir: &str, file_name: &str, top_names: &std::collections::HashMap<String, String>) -> String {
    if dir.is_empty() {
        return file_name.to_owned();
    }
    let (top, rest) = dir.split_once('/').map_or((dir, ""), |(t, r)| (t, r));
    let top = top_names.get(top).map(String::as_str).unwrap_or(top);
    if rest.is_empty() { format!("{top}/{file_name}") } else { format!("{top}/{rest}/{file_name}") }
}

#[cfg(test)]
mod tests {
    use super::{display_path, folders_for};
    use std::collections::HashMap;

    #[test]
    fn parents_come_first() {
        assert_eq!(folders_for(["Trip/Photos/Day 1", "Trip", "Trip/Notes", ""]), ["Trip", "Trip/Photos", "Trip/Notes", "Trip/Photos/Day 1"]);
    }

    #[test]
    fn display_path_uses_created_top_folder() {
        let renamed = HashMap::from([("Trip".to_owned(), "Trip 2".to_owned())]);
        assert_eq!(display_path("", "loose.txt", &renamed), "loose.txt");
        assert_eq!(display_path("Trip", "b.txt", &renamed), "Trip 2/b.txt");
        assert_eq!(display_path("Trip/Photos", "a.txt", &renamed), "Trip 2/Photos/a.txt");
        assert_eq!(display_path("Other/Photos", "a.txt", &renamed), "Other/Photos/a.txt");
    }
}
