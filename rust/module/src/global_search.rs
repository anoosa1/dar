//! Search across apps (`global-search.ts`): only account-owned or currently granted scopes,
//! bounded work and response size.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};
use spacetimedb::{ReducerContext, Table};
use unicode_normalization::UnicodeNormalization;

use crate::app_settings::app_enabled;
use crate::files::{Unlock, access};
use crate::tables::*;
use crate::text_index::{excerpt, is_text_file};
use crate::{Res, fail};

#[derive(Clone)]
pub struct Hit {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub href: String,
    pub thumbnail: String,
}

pub struct Search {
    q: String,
    remaining: i64,
    pub truncated: bool,
    order: Vec<&'static str>,
    groups: HashMap<&'static str, Vec<Value>>,
}

impl Search {
    pub fn new(query: &str) -> Self {
        Self { q: query.nfc().collect::<String>().to_lowercase().trim().to_owned(), remaining: 20000, truncated: false, order: Vec::new(), groups: HashMap::new() }
    }
    pub fn query(&self) -> &str {
        &self.q
    }
    /// One more row looked at; false once the budget is spent.
    pub fn visit(&mut self) -> bool {
        let left = self.remaining;
        self.remaining -= 1;
        if left <= 0 {
            self.truncated = true;
            return false;
        }
        true
    }
    pub fn spent(&self) -> bool {
        self.remaining <= 0
    }
    pub fn matches(&self, value: &str) -> bool {
        value.nfc().collect::<String>().to_lowercase().contains(&self.q)
    }
    pub fn add(&mut self, group: &'static str, hit: Hit) {
        let q = self.q.clone();
        if !self.groups.contains_key(group) {
            self.order.push(group);
        }
        let list = self.groups.entry(group).or_default();
        if list.iter().any(|r| r["id"] == json!(hit.id)) {
            return;
        }
        let mut item = json!({ "id": hit.id, "group": group, "title": hit.title, "detail": hit.detail, "href": hit.href });
        if !hit.thumbnail.is_empty() {
            item["thumbnail"] = json!(hit.thumbnail);
        }
        list.push(item);
        let starts = |r: &Value| !r["title"].as_str().unwrap_or_default().to_lowercase().starts_with(&q);
        list.sort_by(|a, b| {
            starts(a)
                .cmp(&starts(b))
                .then_with(|| crate::locale_compare(a["title"].as_str().unwrap_or_default(), b["title"].as_str().unwrap_or_default()))
                .then_with(|| crate::locale_compare(a["id"].as_str().unwrap_or_default(), b["id"].as_str().unwrap_or_default()))
        });
        if list.len() > 8 {
            list.pop();
            self.truncated = true;
        }
    }
    pub fn result(self) -> Value {
        let items: Vec<Value> = self.order.iter().flat_map(|g| self.groups[g].clone()).collect();
        json!({ "items": items, "truncated": self.truncated })
    }
}

fn hit(id: &str, title: &str, detail: &str, href: &str) -> Hit {
    Hit { id: id.into(), title: title.into(), detail: detail.into(), href: href.into(), thumbnail: String::new() }
}

pub fn encode_uri_component(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn strip_note_extension(name: &str) -> String {
    let lower = name.to_lowercase();
    for ext in [".markdown", ".md"] {
        if lower.ends_with(ext) {
            return name[..name.len() - ext.len()].to_owned();
        }
    }
    name.to_owned()
}

pub fn global_search(ctx: &ReducerContext, user_id: &str, query: &str, is_note: &dyn Fn(&FileEntry) -> bool) -> Res<Value> {
    let mut s = Search::new(query);
    let preferences: Value = ctx.db.pim_preferences().user_id().find(user_id.to_owned()).and_then(|p| serde_json::from_str(&p.value).ok()).unwrap_or(json!({}));
    let show_hidden = preferences["filesShowHidden"] == json!(true);
    let mut seen: HashSet<String> = HashSet::new();
    let can_read = |entry: &FileEntry| access(ctx, entry, user_id, Unlock::Caller).is_some();
    let mut file = |s: &mut Search, entry: &FileEntry| {
        if !seen.insert(entry.id.clone()) || entry.trashed || !can_read(entry) || !show_hidden && entry.name.starts_with('.') {
            return;
        }
        let named = s.matches(&entry.name);
        let text = if entry.kind == "file" && !named && is_text_file(&entry.name, &entry.media_type) {
            ctx.db.blob_texts().id().find(entry.blob_id.clone()).map(|t| t.text).unwrap_or_default()
        } else {
            String::new()
        };
        let inside = if text.is_empty() { String::new() } else { excerpt(&text, s.query()) };
        let file_href = if entry.owner_id == user_id { format!("/?folder={}&file={}", entry.parent_id, entry.id) } else { format!("/?section=shared&file={}", entry.id) };
        if (named || !inside.is_empty()) && entry.owner_id == user_id && is_note(entry) {
            let detail = if inside.is_empty() { "Note".to_owned() } else { inside };
            s.add("Notes", hit(&entry.id, &strip_note_extension(&entry.name), &detail, &format!("/notes?note={}", entry.id)));
        } else if !inside.is_empty() {
            s.add("Files", hit(&entry.id, &entry.name, &inside, &file_href));
        } else if named {
            let href = if entry.kind == "folder" { format!("/?folder={}", entry.id) } else { file_href };
            s.add("Files", hit(&entry.id, &entry.name, if entry.kind == "folder" { "Folder" } else { "File" }, &href));
        }
    };
    for entry in ctx.db.file_entries().owner_id().filter(user_id) {
        if !s.visit() {
            break;
        }
        file(&mut s, &entry);
    }
    let mut folders: Vec<String> = Vec::new();
    for grant in ctx.db.grants().user_id().filter(user_id) {
        if !s.visit() {
            break;
        }
        if let Some(entry) = ctx.db.file_entries().id().find(grant.file_id) {
            file(&mut s, &entry);
            if entry.kind == "folder" && can_read(&entry) {
                folders.push(entry.id);
            }
        }
    }
    let mut walked: HashSet<String> = HashSet::new();
    let mut i = 0;
    while i < folders.len() {
        let id = folders[i].clone();
        i += 1;
        if !walked.insert(id.clone()) {
            continue;
        }
        let Some(folder) = ctx.db.file_entries().id().find(id.clone()) else { continue };
        let window = crate::folder_window(&format!("{}/{id}/", folder.owner_id));
        for entry in ctx.db.file_entries().list_key().filter(crate::bounds(&window)) {
            if !s.visit() {
                break;
            }
            file(&mut s, &entry);
            if entry.kind == "folder" && !entry.trashed && can_read(&entry) {
                folders.push(entry.id);
            }
        }
        if s.spent() {
            break;
        }
    }
    let collections = crate::pim::pim_operation(ctx, user_id, "collections", &json!({}), None, false)?;
    for collection in collections.as_array().cloned().unwrap_or_default() {
        let kind = collection["kind"].as_str().unwrap_or_default();
        if kind == "notes" {
            continue;
        }
        let cid = collection["id"].as_str().unwrap_or_default().to_owned();
        let cname = collection["name"].as_str().unwrap_or_default().to_owned();
        for r in ctx.db.pim_resources().collection_id().filter(&cid) {
            if !s.visit() {
                break;
            }
            let data: Value = serde_json::from_str(&r.projection).unwrap_or(json!({}));
            if !s.matches(&r.projection) {
                continue;
            }
            let group = if kind == "contacts" { "Contacts" } else if data["kind"] == "task" { "Tasks" } else { "Calendar" };
            if group == "Tasks" && !app_enabled(ctx, "tasks") {
                continue;
            }
            let title = match (&data["displayName"], &data["title"]) {
                (v, _) if !v.is_null() => crate::text_of(Some(v)),
                (_, v) if !v.is_null() => crate::text_of(Some(v)),
                _ => "Untitled".to_owned(),
            };
            let start = data.get("start").filter(|v| !v.is_null()).map(|v| crate::text_of(Some(v))).unwrap_or_default();
            let date: String = start.chars().take(10).collect();
            let is_date = date.len() == 10 && date.chars().enumerate().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
            let href = match group {
                "Tasks" => format!("/tasks?item={}", r.id),
                "Contacts" => format!("/contacts?book={cid}&contact={}", encode_uri_component(&r.href)),
                _ => format!("/calendar?book={cid}&event={}{}", encode_uri_component(&r.href), if is_date { format!("&date={date}") } else { String::new() }),
            };
            s.add(group, hit(&r.id, &title, &cname, &href));
        }
        if s.spent() {
            break;
        }
    }
    for app in ctx.db.installed_apps().iter() {
        let data: Value = serde_json::from_str(&app.document).unwrap_or(json!({}));
        if !app_enabled(ctx, &app.id) {
            continue;
        }
        let name = data["name"].as_str().unwrap_or_default().to_owned();
        if s.matches(&name) {
            s.add("Applications", hit(&app.id, &name, "Installed app", &format!("/apps/{}", app.id)));
        }
        if data["permissions"].as_array().is_some_and(|p| p.iter().any(|x| x == "ownUserData:read")) {
            let owner_key = serde_json::to_string(&[&app.id, user_id]).expect("key");
            for r in ctx.db.app_data().owner_key().filter(&owner_key) {
                if !s.visit() {
                    break;
                }
                if s.matches(&r.value) {
                    s.add("Applications", hit(&app.id, &name, "Matching saved app data", &format!("/apps/{}", app.id)));
                }
            }
        }
    }
    if app_enabled(ctx, "agent") {
        for r in ctx.db.agent_projects().owner_id().filter(user_id) {
            if !s.visit() {
                break;
            }
            if s.matches(&r.name) {
                s.add("Applications", hit(&r.id, &r.name, "Agent project", &format!("/agent?project={}", r.id)));
            }
        }
        for r in ctx.db.agent_threads().owner_id().filter(user_id) {
            if !s.visit() {
                break;
            }
            if s.matches(&r.title) {
                s.add("Applications", hit(&r.id, &r.title, "Agent thread", &format!("/agent?project={}&thread={}", r.project_id, r.id)));
            }
        }
    }
    if app_enabled(ctx, "news") {
        crate::news::news_search(ctx, user_id, &mut s);
    }
    if app_enabled(ctx, "mail") {
        crate::mail::mail_search(ctx, user_id, &mut s);
    }
    if app_enabled(ctx, "photos") {
        crate::photos::photos_search(ctx, user_id, &mut s);
    }
    if app_enabled(ctx, "chat") && !crate::chat::chat_search(ctx, user_id, &mut s) {
        s.truncated = true;
    }
    for (name, href) in [("Files", "/"), ("Contacts", "/contacts"), ("Calendar", "/calendar"), ("Notes", "/notes"), ("Tasks", "/tasks"), ("Agent", "/agent"), ("News", "/news"), ("Mail", "/mail"), ("Photos", "/photos"), ("Settings", "/settings")] {
        if app_enabled(ctx, &name.to_lowercase()) && s.matches(name) {
            s.add("Applications", hit(href, name, "Open app", href));
        }
    }
    Ok(s.result())
}

impl Search {
    /// For the per-app searches: a hit in `group`.
    pub fn found(&mut self, group: &'static str, id: String, title: String, detail: String, href: String, thumbnail: String) {
        self.add(group, Hit { id, title, detail, href, thumbnail });
    }
}

#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_global_search(ctx: &mut spacetimedb::ProcedureContext, sessionId: String, query: String) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        let user = crate::from_session(tx, &sessionId)?;
        if tx.db.app_passwords().session_id().find(sessionId.clone()).is_some() {
            return fail("forbidden");
        }
        if query.encode_utf16().count() > 200 {
            return fail("invalidInput");
        }
        if query.trim().is_empty() {
            return Ok(json!({ "items": [], "truncated": false }));
        }
        // Markdown files under the Notes folder are Notes results; a search never creates the folder.
        let notes = if app_enabled(tx, "notes") { crate::folders::notes_root(tx, &user.id, false).ok().flatten() } else { None };
        let is_note = |entry: &FileEntry| -> bool {
            let Some(notes) = &notes else { return false };
            if entry.kind != "file" || !crate::folders::is_note_file(&entry.name) {
                return false;
            }
            let mut parent = entry.parent_id.clone();
            for _ in 0..20 {
                if parent.is_empty() {
                    break;
                }
                if &parent == notes {
                    return true;
                }
                parent = tx.db.file_entries().id().find(parent).map(|e| e.parent_id).unwrap_or_default();
            }
            false
        };
        global_search(tx, &user.id, &query, &is_note)
    })
}
