//! Tasks workspace: modular layout with live sidebar counts, List / Board / Status Kanban
//! layouts, Smart Quick-Add, expandable subtask trees, direct-manipulation inspector,
//! Undo toast, bulk actions, keyboard shortcuts, and live PIM synchronization.

pub mod board;
pub mod inspector;
pub mod list;
pub mod model;
pub mod settings;
pub mod sidebar;

pub use settings::TasksSettings;

use std::collections::HashSet;
use std::sync::Arc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use board::{TasksBoard, TasksKanban};
use inspector::{TaskEditor, TaskInspector};
use list::{TasksHeader, TasksList};
use model::{
    CollectionList, ConfirmAction, ConfirmState, ITEMS_BASE, LISTS_BASE, MoveState, SmartViewVis,
    TaskData, TaskEditorMode, TaskFilterChip, TaskItem, TaskLayout, TaskListResponse, TasksPrefs,
    UndoSnapshot, UndoToast, build_display_rows, compute_counts, encode_uri, filter_tasks,
    normalize_view, sort_tasks, today_in_zone, view_label,
};
use settings::ShortcutsDialog;
use sidebar::TasksSidebar;

use crate::api::{self, BrowserSession};
use crate::icons::Icon;
use crate::pim::{Collection, CollectionEditor, Done, Sharing, etag, send, use_pim};
use crate::shared::{PREFERENCES, Preferences, context, save_preferences};
use crate::ui::{Dialog, document, listen, window};
use crate::workspace::task_occurrences::ReminderInbox;
use crate::workspace::{ReactiveSelect, WorkspaceShell, browser_timezone, search_params};

#[derive(serde::Deserialize)]
struct ReminderCountResp {
    items: Vec<Value>,
}

fn is_typing_target(target: Option<web_sys::EventTarget>) -> bool {
    let Some(el) = target.and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else {
        return false;
    };
    let tag = el.tag_name().to_uppercase();
    if matches!(tag.as_str(), "INPUT" | "TEXTAREA" | "SELECT") {
        return true;
    }
    if el.get_attribute("contenteditable").is_some() {
        return true;
    }
    el.closest("[role=\"dialog\"],[data-editor-pane],[data-settings-sheet],[role=\"menu\"],[role=\"listbox\"]")
        .ok()
        .flatten()
        .is_some()
}

/// Tasks application (`TasksWorkspace`).
#[component]
pub fn TasksWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let pim = use_pim(on_signed_out);
    let shared_prefs = context().preferences;
    let session_sv = StoredValue::new(session.clone());

    let collections: RwSignal<Vec<Collection>> = RwSignal::new(Vec::new());
    let all_items: RwSignal<Vec<TaskItem>> = RwSignal::new(Vec::new());
    let selected: RwSignal<Option<TaskItem>> = RwSignal::new(None);

    let collection = RwSignal::new(String::new());
    let view = RwSignal::new("today".to_owned());
    let sort = RwSignal::new("due".to_owned());
    let layout = RwSignal::new(TaskLayout::List);
    let active_tag = RwSignal::new(Option::<String>::None);
    let filter_chip = RwSignal::new(TaskFilterChip::All);
    let zone = RwSignal::new(browser_timezone());

    let selected_ids: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let expanded_subtasks: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let hidden_board_lists: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());

    let error = RwSignal::new(String::new());
    let selection_error = RwSignal::new(String::new());
    let loading = RwSignal::new(true);
    let busy = RwSignal::new(false);
    let toast: RwSignal<Option<UndoToast>> = RwSignal::new(None);
    let toast_seq = RwSignal::new(0u32);
    let reminder_count = RwSignal::new(0usize);

    let collection_editor: RwSignal<Option<Option<Collection>>> = RwSignal::new(None);
    let sharing: RwSignal<Option<Collection>> = RwSignal::new(None);
    let task_editor: RwSignal<Option<TaskEditorMode>> = RwSignal::new(None);
    let confirm: RwSignal<Option<ConfirmState>> = RwSignal::new(None);
    let move_dialog: RwSignal<Option<MoveState>> = RwSignal::new(None);
    let inbox = RwSignal::new(false);
    let shortcuts_open = RwSignal::new(false);

    let latest_load = RwSignal::new(0u32);
    let sel_seq = RwSignal::new(0u32);
    let reload = RwSignal::new(0u32);
    let prefs_applied = RwSignal::new(false);

    let prefs = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            let mut out = TasksPrefs::default();
            if let Some(p) = p {
                if let Some(v) = p.rest.get("tasksView").and_then(Value::as_str) {
                    out.default_view = normalize_view(v);
                }
                if let Some(s) = p.rest.get("tasksSort").and_then(Value::as_str) {
                    out.default_sort = s.to_owned();
                }
                if let Some(l) = p.rest.get("tasksLayout").and_then(Value::as_str) {
                    out.default_layout = TaskLayout::from_str(l);
                }
                if let Some(dl) = p.rest.get("tasksDefaultList").and_then(Value::as_str) {
                    out.default_list = dl.to_owned();
                }
                if let Some(b) = p.rest.get("tasksShowCompleted").and_then(Value::as_bool) {
                    out.show_completed_inline = b;
                }
                if let Some(b) = p.rest.get("tasksExpandSubtasks").and_then(Value::as_bool) {
                    out.expand_subtasks = b;
                }
                if let Some(b) = p.rest.get("tasksShowPace").and_then(Value::as_bool) {
                    out.show_pace = b;
                }
                if let Some(b) = p.rest.get("tasksConfirmDelete").and_then(Value::as_bool) {
                    out.confirm_delete = b;
                }
                out.smart_views = SmartViewVis::from_value(p.rest.get("tasksSmartViews"));
            }
            out
        })
    });

    let pref_zone = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("timezone").and_then(Value::as_str))
                .map(str::to_owned)
        })
    });

    Effect::new(move |_| {
        if let Some(z) = pref_zone.get() {
            zone.set(z);
        }
    });

    // Apply default view, sort, and layout once preferences load, and keep sort/layout reactive when changed in Settings.
    Effect::new(move |_| {
        let has_prefs = shared_prefs.value.with(Option::is_some);
        if !has_prefs {
            return;
        }
        let p = prefs.get();
        if !prefs_applied.get_untracked() {
            prefs_applied.set(true);
            view.set(p.default_view.clone());
            sort.set(p.default_sort.clone());
            layout.set(p.default_layout);
        } else {
            // Keep default sort / layout / view synced if changed while settings sheet is open.
            sort.set(p.default_sort.clone());
        }
    });

    let show_toast = move |message: String, snapshot: Option<UndoSnapshot>| {
        let seq = toast_seq.get_untracked() + 1;
        toast_seq.set(seq);
        toast.set(Some(UndoToast { message, snapshot }));
        spawn_local(async move {
            crate::sleep(6000).await;
            if toast_seq.try_get_untracked() == Some(seq) {
                toast.try_set(None);
            }
        });
    };

    let today = Memo::new(move |_| today_in_zone(&zone.get()));
    let counts = Memo::new(move |_| {
        let items = all_items.get();
        let t = today.get();
        compute_counts(&items, &t)
    });

    let current_collection = Memo::new(move |_| {
        let cid = collection.get();
        collections.with(|cols| cols.iter().find(|c| c.id == cid).cloned())
    });

    let quick_target = Memo::new(move |_| {
        if let Some(c) = current_collection.get()
            && c.role != "read"
        {
            return Some(c);
        }
        let pref_list = prefs.with(|p| p.default_list.clone());
        if !pref_list.is_empty()
            && let Some(c) = collections.with(|cols| {
                cols.iter()
                    .find(|c| c.id == pref_list && c.role != "read")
                    .cloned()
            })
        {
            return Some(c);
        }
        collections.with(|cols| cols.iter().find(|c| c.role != "read").cloned())
    });

    let close_task = move || {
        selected.set(None);
        model::set_url_item(None);
    };

    let open_task_async = move |id: String| async move {
        match api::get::<TaskItem>(&format!("{ITEMS_BASE}/{}", encode_uri(&id))).await {
            Ok(value) => {
                model::set_url_item(Some(&value.id));
                selected.try_set(Some(value));
            }
            Err(cause) => {
                error.try_set(cause.message);
            }
        }
    };

    let open_task = move |id: String| {
        spawn_local(open_task_async(id));
    };

    // Fetch all task lists and tasks across all lists (paginated up to 1000) so smart views,
    // counts, tags, Today's pace, subtask trees, List Board and Status Kanban stay instant.
    let refresh_all = move || async move {
        let generation = latest_load.get_untracked() + 1;
        latest_load.set(generation);
        let books = api::get::<CollectionList>(LISTS_BASE)
            .await
            .map_err(|e| e.message)?;
        if latest_load.try_get_untracked() != Some(generation) {
            return Ok(());
        }
        collections.try_set(books.items);

        let tz = zone.get_untracked();
        let mut combined: Vec<TaskItem> = Vec::new();
        let mut cur = String::new();
        for _ in 0..5 {
            let mut pairs = vec![
                ("view", "all".to_owned()),
                ("sort", "due".to_owned()),
                ("timezone", tz.clone()),
                ("limit", "200".to_owned()),
            ];
            if !cur.is_empty() {
                pairs.push(("cursor", cur.clone()));
            }
            let qs = search_params(&pairs);
            let page = api::get::<TaskListResponse>(&format!("{ITEMS_BASE}?{qs}"))
                .await
                .map_err(|e| e.message)?;
            combined.extend(page.items);
            if page.next_cursor.is_empty() {
                break;
            }
            cur = page.next_cursor;
        }
        if latest_load.try_get_untracked() == Some(generation) {
            // If expand_subtasks preference is enabled, auto-expand parents on first load.
            if prefs.with_untracked(|p| p.expand_subtasks)
                && expanded_subtasks.with_untracked(HashSet::is_empty)
            {
                let parents: HashSet<String> = combined
                    .iter()
                    .filter(|t| !t.children_slice().is_empty())
                    .map(|t| t.id.clone())
                    .collect();
                if !parents.is_empty() {
                    expanded_subtasks.try_set(parents);
                }
            }
            all_items.try_set(combined);
        }
        // Best-effort refresh of due reminder badge count.
        if let Ok(rems) = api::get::<ReminderCountResp>("/api/v1/pim/reminders").await {
            reminder_count.try_set(rems.items.len());
        }
        Ok::<(), String>(())
    };

    Effect::new(move |_| {
        let _z = zone.get();
        let _e = pim.epoch.get();
        let _r = reload.get();
        loading.set(true);
        spawn_local(async move {
            if let Err(msg) = refresh_all().await {
                error.try_set(msg);
            }
            loading.try_set(false);
        });
    });

    // Deep-link ?item=<id> on initial mount.
    {
        let initial_item = web_sys::UrlSearchParams::new_with_str(
            &window().location().search().unwrap_or_default(),
        )
        .ok()
        .and_then(|p| p.get("item"));
        if let Some(id) = initial_item
            && !id.is_empty()
        {
            spawn_local(async move {
                match api::get::<TaskItem>(&format!("{ITEMS_BASE}/{}", encode_uri(&id))).await {
                    Ok(value) => {
                        selected.try_set(Some(value));
                    }
                    Err(cause) => {
                        error.try_set(cause.message);
                    }
                }
            });
        }
    }

    // Refresh the selected task whenever live PIM data changes or a new task is selected.
    let selected_id = Memo::new(move |_| selected.with(|s| s.as_ref().map(|i| i.id.clone())));
    Effect::new(move |_| {
        let _epoch = pim.epoch.get();
        selection_error.set(String::new());
        let seq = sel_seq.get_untracked() + 1;
        sel_seq.set(seq);
        let Some(id) = selected_id.get() else { return };
        spawn_local(async move {
            match api::get::<TaskItem>(&format!("{ITEMS_BASE}/{}", encode_uri(&id))).await {
                Ok(value) if sel_seq.try_get_untracked() == Some(seq) => {
                    selected.try_set(Some(value));
                }
                Err(cause) if sel_seq.try_get_untracked() == Some(seq) => {
                    if matches!(cause.status, 401 | 403 | 404 | 410) {
                        close_task();
                        error.try_set("The selected item is no longer accessible.".into());
                    } else {
                        selection_error.try_set(
                            "This task could not be refreshed. Your editor is still open.".into(),
                        );
                    }
                }
                _ => {}
            }
        });
    });

    // Filtered items for List & Kanban.
    let filtered_items = Memo::new(move |_| {
        let all = all_items.get();
        let v = view.get();
        let col = collection.get();
        let tg = active_tag.get();
        let chip = filter_chip.get();
        let t = today.get();
        let mut out = filter_tasks(&all, &v, &col, tg.as_deref(), chip, &t);
        sort_tasks(&mut out, &sort.get(), &t);
        out
    });

    // Display rows (top-level + nested subtasks) and completed drawer rows for List view.
    // Deliberately does NOT subscribe to `selected` so `.workspace-row-content` DOM nodes keep
    // identity for focus restoration on Escape/close.
    let list_split = Memo::new(move |_| {
        let filtered = filtered_items.get();
        let all = all_items.get();
        let s = sort.get();
        let t = today.get();
        let separate_completed = view.get() != "completed";
        build_display_rows(filtered, &all, &s, &t, separate_completed)
    });
    let display_rows = Memo::new(move |_| list_split.with(|(d, _)| d.clone()));
    let completed_rows = Memo::new(move |_| list_split.with(|(_, c)| c.clone()));

    let save_pref_field = move |key: &'static str, value: Value| {
        let sess = session_sv.get_value();
        spawn_local(async move {
            if let Ok(old) = api::get::<Preferences>(PREFERENCES).await {
                let rev = old.revision;
                let mut body = serde_json::to_value(&old).unwrap_or_else(|_| json!({}));
                if let Some(obj) = body.as_object_mut() {
                    obj.insert(key.into(), value);
                }
                if let Ok(next) = save_preferences(body, rev, &sess).await {
                    shared_prefs.value.try_set(Some(next));
                }
            }
        });
    };

    let patch_task_data = move |row: TaskItem, next_data: TaskData, toast_msg: Option<String>| {
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        let prev_data = row.data.clone();
        spawn_local(async move {
            busy.try_set(true);
            error.try_set(String::new());
            match send::<TaskItem>(
                &format!("{ITEMS_BASE}/{}", encode_uri(&row.id)),
                "PUT",
                Some(json!({ "data": next_data })),
                &csrf,
                Some(etag(&row.id, row.revision)),
            )
            .await
            {
                Ok(updated) => {
                    if selected.with_untracked(|s| {
                        s.as_ref().map(|x| x.id.as_str()) == Some(row.id.as_str())
                    }) {
                        selected.try_set(Some(updated.clone()));
                    }
                    if let Some(msg) = toast_msg {
                        show_toast(
                            msg,
                            Some(UndoSnapshot::RestoreTask {
                                id: row.id.clone(),
                                collection_id: row.collection_id.clone(),
                                data: prev_data,
                            }),
                        );
                    }
                    if let Err(msg) = refresh_all().await {
                        error.try_set(msg);
                    }
                    pim.refresh();
                }
                Err(cause) => {
                    error.try_set(cause.message);
                }
            }
            busy.try_set(false);
        });
    };

    let toggle_star = move |row: TaskItem| {
        let mut next = row.data.clone();
        if row.is_starred() {
            next.starred = false;
            if (1..=4).contains(&next.priority) {
                next.priority = 0;
            }
        } else {
            next.starred = true;
        }
        let msg = if next.starred {
            format!("Starred \u{201c}{}\u{201d}", row.data.title)
        } else {
            format!("Removed star from \u{201c}{}\u{201d}", row.data.title)
        };
        patch_task_data(row, next, Some(msg));
    };

    let complete_task = move |row: TaskItem| {
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            busy.try_set(true);
            error.try_set(String::new());
            let outcome: Result<(), String> = async {
                let data = row.data.clone();
                let full = api::get::<TaskItem>(&format!("{ITEMS_BASE}/{}", encode_uri(&row.id)))
                    .await
                    .map_err(|e| e.message)?;
                if data.status != "COMPLETED"
                    && full
                        .children_slice()
                        .iter()
                        .any(|c| !matches!(c.status.as_str(), "COMPLETED" | "CANCELLED"))
                {
                    confirm.try_set(Some(ConfirmState {
                        title: "Complete task and subtasks?".into(),
                        text: "All unfinished subtasks will be marked completed. Reopening the parent later will keep its children completed.".into(),
                        destructive: false,
                        action: ConfirmAction::CompleteWithSubtasks {
                            id: full.id,
                            revision: full.revision,
                        },
                        alternative: None,
                    }));
                    return Ok(());
                }
                if !data.recurrence.is_empty() || data.recurring {
                    let mut edited = full.clone();
                    let next_occ = full.next_occurrence.clone();
                    edited.editing_occurrence_id =
                        next_occ.as_ref().map(|o| o.recurrence_id.clone());
                    let mut next_data = next_occ
                        .map(|o| o.data)
                        .unwrap_or_else(|| full.data.clone());
                    let reopen = data.status == "COMPLETED";
                    next_data.status = if reopen {
                        "NEEDS-ACTION".into()
                    } else {
                        "COMPLETED".into()
                    };
                    next_data.progress = if reopen { 0 } else { 100 };
                    edited.data = next_data;
                    task_editor.try_set(Some(TaskEditorMode::Edit(edited)));
                    show_toast(
                        "Choose an occurrence in the task editor to complete it.".into(),
                        None,
                    );
                    return Ok(());
                }
                let reopen = data.status == "COMPLETED";
                let prev_data = data.clone();
                let mut updated = data;
                updated.status = if reopen {
                    "NEEDS-ACTION".into()
                } else {
                    "COMPLETED".into()
                };
                updated.progress = if reopen { 0 } else { 100 };
                send::<Value>(
                    &format!("{ITEMS_BASE}/{}", encode_uri(&row.id)),
                    "PUT",
                    Some(json!({ "data": updated })),
                    &csrf,
                    Some(etag(&row.id, full.revision)),
                )
                .await
                .map_err(|e| e.message)?;
                if selected.with_untracked(|s| s.as_ref().map(|x| x.id.as_str()) == Some(row.id.as_str())) {
                    open_task_async(row.id.clone()).await;
                }
                let msg = if reopen {
                    format!("Reopened \u{201c}{}\u{201d}", row.data.title)
                } else {
                    format!("Completed \u{201c}{}\u{201d}", row.data.title)
                };
                show_toast(
                    msg,
                    Some(UndoSnapshot::RestoreTask {
                        id: row.id.clone(),
                        collection_id: row.collection_id.clone(),
                        data: prev_data,
                    }),
                );
                Ok(())
            }
            .await;
            match outcome {
                Ok(()) => {
                    if let Err(msg) = refresh_all().await {
                        error.try_set(msg);
                    }
                    pim.refresh();
                }
                Err(msg) => {
                    error.try_set(msg);
                }
            }
            busy.try_set(false);
        });
    };

    let delete_task_direct = move |row: TaskItem| {
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            busy.try_set(true);
            error.try_set(String::new());
            let prev_col = row.collection_id.clone();
            let prev_data = row.data.clone();
            match send::<Value>(
                &format!("{ITEMS_BASE}/{}", encode_uri(&row.id)),
                "DELETE",
                None,
                &csrf,
                Some(etag(&row.id, row.revision)),
            )
            .await
            {
                Ok(_) => {
                    if selected.with_untracked(|s| {
                        s.as_ref().map(|x| x.id.as_str()) == Some(row.id.as_str())
                    }) {
                        close_task();
                    }
                    show_toast(
                        format!("Deleted \u{201c}{}\u{201d}", prev_data.title),
                        Some(UndoSnapshot::RecreateTask {
                            collection_id: prev_col,
                            data: prev_data,
                        }),
                    );
                    if let Err(msg) = refresh_all().await {
                        error.try_set(msg);
                    }
                    pim.refresh();
                }
                Err(cause) => {
                    error.try_set(cause.message);
                }
            }
            busy.try_set(false);
        });
    };

    let remove_task = move |row: TaskItem| {
        let need_confirm = prefs.with_untracked(|p| p.confirm_delete);
        spawn_local(async move {
            match api::get::<TaskItem>(&format!("{ITEMS_BASE}/{}", encode_uri(&row.id))).await {
                Ok(full) => {
                    let has_children = !full.children_slice().is_empty();
                    if !need_confirm && !has_children {
                        delete_task_direct(full);
                        return;
                    }
                    confirm.try_set(Some(ConfirmState {
                        title: "Delete task?".into(),
                        destructive: true,
                        text: if has_children {
                            "Deleting this task keeps its subtasks as top-level tasks. You can instead delete the entire subtree.".into()
                        } else {
                            "This permanently removes the task.".into()
                        },
                        action: ConfirmAction::DeleteTask {
                            id: full.id.clone(),
                            revision: full.revision,
                            children: has_children.then_some("detach"),
                        },
                        alternative: has_children.then(|| {
                            (
                                "Delete task and all subtasks".into(),
                                ConfirmAction::DeleteTask {
                                    id: full.id,
                                    revision: full.revision,
                                    children: Some("delete"),
                                },
                            )
                        }),
                    }));
                }
                Err(cause) => {
                    error.try_set(cause.message);
                }
            }
        });
    };

    let open_subtask_editor = move |parent: TaskItem| {
        let mut data = parent.data.clone();
        data.title = String::new();
        data.parent_uid = parent.uid.clone();
        data.status = "NEEDS-ACTION".into();
        data.progress = 0;
        data.completed_at = String::new();
        data.recurrence = String::new();
        data.recurring = false;
        data.reminder_minutes = None;
        task_editor.set(Some(TaskEditorMode::New {
            collection_id: Some(parent.collection_id),
            data: Some(data),
        }));
    };

    let quick_add = move |title: String,
                          due_override: Option<String>,
                          pri_override: Option<i32>,
                          list_override: Option<String>| {
        let target_id = list_override
            .or_else(|| quick_target.get_untracked().map(|c| c.id))
            .unwrap_or_default();
        if target_id.is_empty() || title.trim().is_empty() {
            return;
        }
        let target_name = collections
            .with_untracked(|cols| {
                cols.iter()
                    .find(|c| c.id == target_id)
                    .map(|c| c.name.clone())
            })
            .unwrap_or_else(|| "Tasks".into());
        let cur_view = view.get_untracked();
        let in_list = !collection.with_untracked(String::is_empty);
        let cur_today = today.get_untracked();
        let cur_tag = active_tag.get_untracked();
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());

        spawn_local(async move {
            busy.try_set(true);
            error.try_set(String::new());
            let mut data = json!({ "title": title.trim() });
            if let Some(obj) = data.as_object_mut() {
                let effective_due = due_override.or_else(|| {
                    if !in_list && cur_view == "today" {
                        Some(cur_today)
                    } else {
                        None
                    }
                });
                if let Some(d) = effective_due {
                    obj.insert("due".into(), Value::String(d));
                    obj.insert("start".into(), Value::String(String::new()));
                    obj.insert("allDay".into(), Value::Bool(true));
                    obj.insert("timezone".into(), Value::String("floating".into()));
                }
                if let Some(p) = pri_override {
                    obj.insert("priority".into(), Value::from(p));
                } else if !in_list && (cur_view == "starred" || cur_view == "important") {
                    obj.insert("starred".into(), Value::Bool(true));
                }
                if let Some(tg) = cur_tag {
                    obj.insert("categories".into(), json!([tg]));
                }
            }
            let body = json!({ "collectionId": target_id, "data": data });
            match send::<TaskItem>(ITEMS_BASE, "POST", Some(body), &csrf, None).await {
                Ok(row) => {
                    show_toast(
                        format!("Added \u{201c}{}\u{201d} to {target_name}", row.data.title),
                        None,
                    );
                    if let Err(msg) = refresh_all().await {
                        error.try_set(msg);
                    }
                    pim.refresh();
                }
                Err(cause) => {
                    error.try_set(cause.message);
                }
            }
            busy.try_set(false);
        });
    };

    let quick_subtask = move |parent: TaskItem, title: String| {
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        let parent_id = parent.id.clone();
        spawn_local(async move {
            busy.try_set(true);
            error.try_set(String::new());
            let data = json!({
                "title": title,
                "parentUid": parent.uid,
                "status": "NEEDS-ACTION",
                "progress": 0,
            });
            let body = json!({ "collectionId": parent.collection_id, "data": data });
            match send::<TaskItem>(ITEMS_BASE, "POST", Some(body), &csrf, None).await {
                Ok(_) => {
                    expanded_subtasks.try_update(|e| {
                        e.insert(parent_id.clone());
                    });
                    if let Err(msg) = refresh_all().await {
                        error.try_set(msg);
                    }
                    open_task_async(parent_id).await;
                    pim.refresh();
                }
                Err(cause) => {
                    error.try_set(cause.message);
                }
            }
            busy.try_set(false);
        });
    };

    let duplicate_task = move |row: TaskItem| {
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            busy.try_set(true);
            error.try_set(String::new());
            match send::<Value>(
                &format!("{ITEMS_BASE}/{}/copy", encode_uri(&row.id)),
                "POST",
                Some(json!({ "collectionId": row.collection_id })),
                &csrf,
                Some(etag(&row.id, row.revision)),
            )
            .await
            {
                Ok(_) => {
                    show_toast(
                        format!("Duplicated \u{201c}{}\u{201d}", row.data.title),
                        None,
                    );
                    if let Err(msg) = refresh_all().await {
                        error.try_set(msg);
                    }
                    pim.refresh();
                }
                Err(cause) => {
                    error.try_set(cause.message);
                }
            }
            busy.try_set(false);
        });
    };

    let run_undo = move || {
        let Some(t) = toast.get_untracked() else {
            return;
        };
        toast.set(None);
        let Some(snap) = t.snapshot else { return };
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            busy.try_set(true);
            error.try_set(String::new());
            match snap {
                UndoSnapshot::RestoreTask { id, data, .. } => {
                    if let Ok(cur) =
                        api::get::<TaskItem>(&format!("{ITEMS_BASE}/{}", encode_uri(&id))).await
                    {
                        let _ = send::<TaskItem>(
                            &format!("{ITEMS_BASE}/{}", encode_uri(&id)),
                            "PUT",
                            Some(json!({ "data": data })),
                            &csrf,
                            Some(etag(&id, cur.revision)),
                        )
                        .await;
                        if selected.with_untracked(|s| {
                            s.as_ref().map(|x| x.id.as_str()) == Some(id.as_str())
                        }) {
                            open_task_async(id).await;
                        }
                    }
                }
                UndoSnapshot::RestoreMany { items } => {
                    for (id, data) in items {
                        if let Ok(cur) =
                            api::get::<TaskItem>(&format!("{ITEMS_BASE}/{}", encode_uri(&id))).await
                        {
                            let _ = send::<TaskItem>(
                                &format!("{ITEMS_BASE}/{}", encode_uri(&id)),
                                "PUT",
                                Some(json!({ "data": data })),
                                &csrf,
                                Some(etag(&id, cur.revision)),
                            )
                            .await;
                        }
                    }
                }
                UndoSnapshot::RecreateTask {
                    collection_id,
                    data,
                } => {
                    let _ = send::<TaskItem>(
                        ITEMS_BASE,
                        "POST",
                        Some(json!({ "collectionId": collection_id, "data": data })),
                        &csrf,
                        None,
                    )
                    .await;
                }
            }
            let _ = refresh_all().await;
            pim.refresh();
            busy.try_set(false);
        });
    };

    let move_overdue_to_today = move || {
        let t = today.get_untracked();
        let overdue_rows: Vec<TaskItem> = display_rows
            .get_untracked()
            .into_iter()
            .map(|d| d.item)
            .filter(|r| r.role != "read" && r.is_overdue(&t))
            .collect();
        if overdue_rows.is_empty() {
            return;
        }
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            busy.try_set(true);
            error.try_set(String::new());
            let mut prev = Vec::new();
            let count = overdue_rows.len();
            for row in overdue_rows {
                prev.push((row.id.clone(), row.data.clone()));
                let mut next = row.data.clone();
                next.due = t.clone();
                next.all_day = true;
                next.timezone = "floating".into();
                let _ = send::<TaskItem>(
                    &format!("{ITEMS_BASE}/{}", encode_uri(&row.id)),
                    "PUT",
                    Some(json!({ "data": next })),
                    &csrf,
                    Some(etag(&row.id, row.revision)),
                )
                .await;
            }
            show_toast(
                format!(
                    "Moved {count} overdue task{} to Today",
                    if count == 1 { "" } else { "s" }
                ),
                Some(UndoSnapshot::RestoreMany { items: prev }),
            );
            let _ = refresh_all().await;
            pim.refresh();
            busy.try_set(false);
        });
    };

    let selected_rows_vec = move || -> Vec<TaskItem> {
        let ids = selected_ids.get_untracked();
        all_items
            .get_untracked()
            .into_iter()
            .filter(|t| ids.contains(&t.id) && t.role != "read")
            .collect()
    };

    let bulk_mutate = move |label: &'static str, mutate: fn(&mut TaskData, &str)| {
        let rows = selected_rows_vec();
        if rows.is_empty() {
            return;
        }
        let t = today.get_untracked();
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            busy.try_set(true);
            error.try_set(String::new());
            let mut prev = Vec::new();
            let count = rows.len();
            for row in rows {
                prev.push((row.id.clone(), row.data.clone()));
                let mut next = row.data.clone();
                mutate(&mut next, &t);
                let _ = send::<TaskItem>(
                    &format!("{ITEMS_BASE}/{}", encode_uri(&row.id)),
                    "PUT",
                    Some(json!({ "data": next })),
                    &csrf,
                    Some(etag(&row.id, row.revision)),
                )
                .await;
            }
            selected_ids.try_set(HashSet::new());
            show_toast(
                format!("{label} {count} task{}", if count == 1 { "" } else { "s" }),
                Some(UndoSnapshot::RestoreMany { items: prev }),
            );
            let _ = refresh_all().await;
            pim.refresh();
            busy.try_set(false);
        });
    };

    let run_confirm = move |action: ConfirmAction| {
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            busy.try_set(true);
            error.try_set(String::new());
            let result: Result<(), String> = match action {
                ConfirmAction::DeleteTask {
                    id,
                    revision,
                    children,
                } => {
                    let body = children.map(|c| json!({ "children": c }));
                    send::<Value>(
                        &format!("{ITEMS_BASE}/{}", encode_uri(&id)),
                        "DELETE",
                        body,
                        &csrf,
                        Some(etag(&id, revision)),
                    )
                    .await
                    .map(|_| close_task())
                    .map_err(|e| e.message)
                }
                ConfirmAction::CompleteWithSubtasks { id, revision } => {
                    match send::<Value>(
                        &format!("{ITEMS_BASE}/{}/complete", encode_uri(&id)),
                        "POST",
                        Some(json!({ "children": "complete" })),
                        &csrf,
                        Some(etag(&id, revision)),
                    )
                    .await
                    {
                        Ok(_) => {
                            open_task_async(id).await;
                            Ok(())
                        }
                        Err(e) => Err(e.message),
                    }
                }
                ConfirmAction::DeleteList { id, revision } => send::<Value>(
                    &format!("{LISTS_BASE}/{}", encode_uri(&id)),
                    "DELETE",
                    None,
                    &csrf,
                    Some(etag(&id, revision)),
                )
                .await
                .map(|_| {
                    if collection.get_untracked() == id {
                        collection.try_set(String::new());
                    }
                    if selected
                        .with_untracked(|s| s.as_ref().is_some_and(|x| x.collection_id == id))
                    {
                        close_task();
                    }
                })
                .map_err(|e| e.message),
                ConfirmAction::BulkDelete { rows } => {
                    for (id, rev) in rows {
                        let _ = send::<Value>(
                            &format!("{ITEMS_BASE}/{}", encode_uri(&id)),
                            "DELETE",
                            None,
                            &csrf,
                            Some(etag(&id, rev)),
                        )
                        .await;
                    }
                    selected_ids.try_set(HashSet::new());
                    close_task();
                    Ok(())
                }
            };
            match result {
                Ok(()) => {
                    confirm.try_set(None);
                    if let Err(msg) = refresh_all().await {
                        error.try_set(msg);
                    }
                    pim.refresh();
                }
                Err(msg) => {
                    error.try_set(msg);
                }
            }
            busy.try_set(false);
        });
    };

    // Single-key keyboard shortcuts (outside inputs/dialogs/editors).
    Effect::new(move |_| {
        listen(&document(), "keydown", false, move |event| {
            let Ok(ke) = event.dyn_into::<web_sys::KeyboardEvent>() else {
                return;
            };
            if ke.ctrl_key() || ke.meta_key() || ke.alt_key() || ke.default_prevented() {
                return;
            }
            let key = ke.key();
            if key == "Escape" {
                if !selected_ids.with_untracked(HashSet::is_empty) {
                    ke.prevent_default();
                    selected_ids.set(HashSet::new());
                    return;
                }
                if selected.with_untracked(Option::is_some) && !is_typing_target(ke.target()) {
                    ke.prevent_default();
                    close_task();
                    return;
                }
                return;
            }
            if is_typing_target(ke.target()) {
                return;
            }
            match key.as_str() {
                "c" | "C" => {
                    ke.prevent_default();
                    if !collections.with_untracked(|cols| cols.iter().any(|c| c.role != "read")) {
                        collection_editor.set(Some(None));
                    } else {
                        task_editor.set(Some(TaskEditorMode::New {
                            collection_id: None,
                            data: None,
                        }));
                    }
                }
                "?" => {
                    ke.prevent_default();
                    shortcuts_open.set(true);
                }
                "1" => {
                    ke.prevent_default();
                    layout.set(TaskLayout::List);
                }
                "2" => {
                    ke.prevent_default();
                    layout.set(TaskLayout::Board);
                }
                "3" => {
                    ke.prevent_default();
                    layout.set(TaskLayout::Kanban);
                }
                "j" | "J" | "k" | "K" => {
                    let rows = display_rows.get_untracked();
                    if rows.is_empty() {
                        return;
                    }
                    ke.prevent_default();
                    let cur_id = selected.with_untracked(|s| s.as_ref().map(|x| x.id.clone()));
                    let idx = cur_id
                        .as_deref()
                        .and_then(|id| rows.iter().position(|r| r.item.id == id));
                    let next_idx = match (idx, key.as_str()) {
                        (Some(i), "j" | "J") => (i + 1).min(rows.len() - 1),
                        (Some(i), _) => i.saturating_sub(1),
                        (None, _) => 0,
                    };
                    open_task(rows[next_idx].item.id.clone());
                }
                " " => {
                    if let Some(row) = selected.get_untracked()
                        && row.role != "read"
                    {
                        ke.prevent_default();
                        complete_task(row);
                    }
                }
                "s" | "S" => {
                    if let Some(row) = selected.get_untracked()
                        && row.role != "read"
                    {
                        ke.prevent_default();
                        toggle_star(row);
                    }
                }
                "e" | "E" => {
                    if let Some(row) = selected.get_untracked()
                        && row.role != "read"
                    {
                        ke.prevent_default();
                        task_editor.set(Some(TaskEditorMode::Edit(row)));
                    }
                }
                "x" | "X" => {
                    if let Some(row) = selected.get_untracked() {
                        ke.prevent_default();
                        selected_ids.update(|set| {
                            if !set.remove(&row.id) {
                                set.insert(row.id);
                            }
                        });
                    }
                }
                _ => {}
            }
        });
    });

    let heading_title = Signal::derive(move || {
        if layout.get() == TaskLayout::Board {
            return "Board".to_owned();
        }
        if let Some(c) = current_collection.get() {
            c.name
        } else {
            view_label(&view.get()).to_owned()
        }
    });

    let total_count = Signal::derive(move || {
        if layout.get() == TaskLayout::Board {
            counts.with(|c| c.all)
        } else {
            display_rows.with(Vec::len)
        }
    });

    let overdue_count = Signal::derive(move || counts.with(|c| c.overdue));
    let show_completed_inline = Signal::derive(move || prefs.with(|p| p.show_completed_inline));

    let sidebar: ChildrenFn = Arc::new(move || {
        view! {
            <TasksSidebar
                collections=Signal::derive(move || collections.get())
                counts=counts
                prefs=Signal::from(prefs)
                layout=layout
                view=view
                collection=collection
                active_tag=active_tag
                hidden_board_lists=hidden_board_lists
                zone=zone
                reminder_count=reminder_count
                busy=busy
                on_new_task=Callback::new(move |_| {
                    if !collections.with_untracked(|cols| cols.iter().any(|c| c.role != "read")) {
                        collection_editor.set(Some(None));
                    } else {
                        task_editor.set(Some(TaskEditorMode::New { collection_id: None, data: None }));
                    }
                })
                on_new_list=Callback::new(move |_| collection_editor.set(Some(None)))
                on_select_view=Callback::new(move |val: String| {
                    if layout.get_untracked() == TaskLayout::Board {
                        layout.set(TaskLayout::List);
                    }
                    view.set(val);
                    collection.set(String::new());
                    active_tag.set(None);
                    selected_ids.set(HashSet::new());
                    close_task();
                })
                on_select_list=Callback::new(move |c: Collection| {
                    collection.set(c.id);
                    view.set("all".into());
                    active_tag.set(None);
                    selected_ids.set(HashSet::new());
                    close_task();
                })
                on_toggle_board_list=Callback::new(move |id: String| {
                    hidden_board_lists.update(|set| {
                        if !set.remove(&id) {
                            set.insert(id);
                        }
                    });
                })
                on_toggle_tag=Callback::new(move |tg: String| {
                    active_tag.update(|cur| {
                        if cur.as_deref() == Some(tg.as_str()) {
                            *cur = None;
                        } else {
                            *cur = Some(tg);
                        }
                    });
                })
                on_edit_list=Callback::new(move |row: Collection| collection_editor.set(Some(Some(row))))
                on_share_list=Callback::new(move |row: Collection| sharing.set(Some(row)))
                on_delete_list=Callback::new(move |target: Collection| {
                    confirm.set(Some(ConfirmState {
                        title: "Delete task list?".into(),
                        destructive: true,
                        text: format!("Delete \u{201c}{}\u{201d}? All events and tasks in this calendar will be permanently removed.", target.name),
                        action: ConfirmAction::DeleteList { id: target.id, revision: target.revision },
                        alternative: None,
                    }));
                })
                on_open_reminders=Callback::new(move |_| inbox.set(true))
            />
        }
        .into_any()
    });

    let details: ChildrenFn = Arc::new(move || {
        view! {
            {move || (!selection_error.get().is_empty()).then(|| view! {
                <div role="alert" class="error">
                    <span>{selection_error.get()}</span>
                    <button type="button" class="text-button" on:click=move |_| pim.refresh()>"Retry refresh"</button>
                </div>
            })}
            {move || selected.get().map(|row| {
                let col = collections.with(|cols| cols.iter().find(|c| c.id == row.collection_id).cloned());
                let row_edit = row.clone();
                let row_comp = row.clone();
                let row_star = row.clone();
                let row_sub = row.clone();
                let row_del = row.clone();
                let row_move = row.clone();
                view! {
                    <TaskInspector
                        row=row
                        collection=col
                        all_items=Signal::derive(move || all_items.get())
                        today=today
                        busy=busy.into()
                        on_close=Callback::new(move |_| close_task())
                        on_edit=Callback::new(move |_| task_editor.set(Some(TaskEditorMode::Edit(row_edit.clone()))))
                        on_complete=Callback::new(move |_| complete_task(row_comp.clone()))
                        on_toggle_star=Callback::new(move |_| toggle_star(row_star.clone()))
                        on_patch=Callback::new(move |(r, next): (TaskItem, TaskData)| patch_task_data(r, next, None))
                        on_subtask_modal=Callback::new(move |_| open_subtask_editor(row_sub.clone()))
                        on_quick_subtask=Callback::new(move |(p, title): (TaskItem, String)| quick_subtask(p, title))
                        on_toggle_child=Callback::new(move |child: TaskItem| complete_task(child))
                        on_delete=Callback::new(move |_| remove_task(row_del.clone()))
                        on_move=Callback::new(move |copy: bool| move_dialog.set(Some(MoveState { rows: vec![row_move.clone()], copy })))
                        on_open=Callback::new(move |id: String| open_task(id))
                    />
                }
            })}
        }
        .into_any()
    });

    let has_details = Signal::from(Memo::new(move |_| selected.with(Option::is_some)));

    view! {
        <WorkspaceShell
            kind="tasks"
            session=session
            on_signed_out=on_signed_out
            sidebar=sidebar
            has_details=has_details
            details=details
            pim=pim
        >
            <div class="workspace-list tasks-main">
                <TasksHeader
                    heading_title=heading_title
                    total_count=total_count
                    overdue_count=overdue_count
                    active_tag=active_tag
                    layout=layout
                    sort=sort
                    filter_chip=filter_chip
                    selected_ids=selected_ids
                    busy=busy
                    on_set_layout=Callback::new(move |next: TaskLayout| {
                        layout.set(next);
                        save_pref_field("tasksLayout", Value::String(next.as_str().into()));
                    })
                    on_set_sort=Callback::new(move |next: String| {
                        sort.set(next.clone());
                        save_pref_field("tasksSort", Value::String(next));
                    })
                    on_open_shortcuts=Callback::new(move |_| shortcuts_open.set(true))
                    on_clear_tag=Callback::new(move |_| active_tag.set(None))
                    on_bulk_complete=Callback::new(move |_| {
                        bulk_mutate("Completed", |d, _| {
                            d.status = "COMPLETED".into();
                            d.progress = 100;
                        });
                    })
                    on_bulk_today=Callback::new(move |_| {
                        bulk_mutate("Moved to Today:", |d, t| {
                            d.due = t.to_owned();
                            d.all_day = true;
                            d.timezone = "floating".into();
                        });
                    })
                    on_bulk_star=Callback::new(move |_| {
                        bulk_mutate("Starred", |d, _| {
                            d.starred = true;
                        });
                    })
                    on_bulk_move=Callback::new(move |_| {
                        let rows = selected_rows_vec();
                        if !rows.is_empty() {
                            move_dialog.set(Some(MoveState { rows, copy: false }));
                        }
                    })
                    on_bulk_delete=Callback::new(move |_| {
                        let rows: Vec<(String, u64)> = selected_rows_vec()
                            .into_iter()
                            .map(|r| (r.id, r.revision))
                            .collect();
                        if !rows.is_empty() {
                            let n = rows.len();
                            confirm.set(Some(ConfirmState {
                                title: format!("Delete {n} task{}?", if n == 1 { "" } else { "s" }),
                                text: "This permanently removes the selected tasks.".into(),
                                destructive: true,
                                action: ConfirmAction::BulkDelete { rows },
                                alternative: None,
                            }));
                        }
                    })
                    on_clear_selection=Callback::new(move |_| selected_ids.set(HashSet::new()))
                />

                {move || (!error.get().is_empty()).then(|| view! {
                    <div role="alert" class="error tasks-error-banner">
                        <span>{error.get()}</span>
                        <button type="button" class="text-button" on:click=move |_| {
                            error.set(String::new());
                            reload.update(|n| *n += 1);
                            pim.refresh();
                        }>"Reload"</button>
                    </div>
                })}

                {move || match layout.get() {
                    TaskLayout::List => view! {
                        <TasksList
                            collections=Signal::derive(move || collections.get())
                            collection=collection
                            view=view
                            today=today
                            display_rows=display_rows
                            completed_rows=completed_rows
                            selected_id=selected_id
                            selected_ids=selected_ids
                            expanded_subtasks=expanded_subtasks
                            show_completed_inline=show_completed_inline
                            quick_target=quick_target
                            busy=busy
                            loading=loading
                            on_quick_add=Callback::new(move |(t, d, p, l)| quick_add(t, d, p, l))
                            on_open_task=Callback::new(move |id: String| open_task(id))
                            on_complete_task=Callback::new(move |r: TaskItem| complete_task(r))
                            on_toggle_star=Callback::new(move |r: TaskItem| toggle_star(r))
                            on_add_subtask=Callback::new(move |r: TaskItem| open_subtask_editor(r))
                            on_duplicate_task=Callback::new(move |r: TaskItem| duplicate_task(r))
                            on_move_task=Callback::new(move |(r, copy): (TaskItem, bool)| move_dialog.set(Some(MoveState { rows: vec![r], copy })))
                            on_delete_task=Callback::new(move |r: TaskItem| remove_task(r))
                            on_toggle_select=Callback::new(move |id: String| {
                                selected_ids.update(|set| {
                                    if !set.remove(&id) {
                                        set.insert(id);
                                    }
                                });
                            })
                            on_toggle_expand=Callback::new(move |id: String| {
                                expanded_subtasks.update(|set| {
                                    if !set.remove(&id) {
                                        set.insert(id);
                                    }
                                });
                            })
                            on_move_overdue_today=Callback::new(move |_| move_overdue_to_today())
                            on_pick_tag=Callback::new(move |tg: String| active_tag.set(Some(tg)))
                        />
                    }.into_any(),
                    TaskLayout::Board => view! {
                        <TasksBoard
                            collections=Signal::derive(move || collections.get())
                            all_items=Signal::derive(move || all_items.get())
                            hidden_board_lists=hidden_board_lists
                            active_tag=active_tag
                            filter_chip=filter_chip
                            sort=sort
                            today=today
                            selected_id=selected_id
                            show_completed_inline=show_completed_inline
                            busy=busy
                            on_open_task=Callback::new(move |id: String| open_task(id))
                            on_complete_task=Callback::new(move |r: TaskItem| complete_task(r))
                            on_toggle_star=Callback::new(move |r: TaskItem| toggle_star(r))
                            on_quick_add_to_list=Callback::new(move |(col_id, title): (String, String)| {
                                quick_add(title, None, None, Some(col_id));
                            })
                            on_drag_move_to_list=Callback::new(move |(task_id, dest_col): (String, String)| {
                                let Some(row) = all_items.with_untracked(|items| items.iter().find(|i| i.id == task_id).cloned()) else {
                                    return;
                                };
                                if row.collection_id == dest_col || row.role == "read" {
                                    return;
                                }
                                let csrf = session_sv.with_value(|s| s.csrf_token.clone());
                                spawn_local(async move {
                                    busy.try_set(true);
                                    match send::<Value>(
                                        &format!("{ITEMS_BASE}/{}/move", encode_uri(&row.id)),
                                        "POST",
                                        Some(json!({ "collectionId": dest_col })),
                                        &csrf,
                                        Some(etag(&row.id, row.revision)),
                                    )
                                    .await
                                    {
                                        Ok(_) => {
                                            let _ = refresh_all().await;
                                            pim.refresh();
                                        }
                                        Err(e) => {
                                            error.try_set(e.message);
                                        }
                                    }
                                    busy.try_set(false);
                                });
                            })
                            on_new_list=Callback::new(move |_| collection_editor.set(Some(None)))
                        />
                    }.into_any(),
                    TaskLayout::Kanban => view! {
                        <TasksKanban
                            collections=Signal::derive(move || collections.get())
                            items=filtered_items
                            today=today
                            selected_id=selected_id
                            busy=busy
                            on_open_task=Callback::new(move |id: String| open_task(id))
                            on_complete_task=Callback::new(move |r: TaskItem| complete_task(r))
                            on_toggle_star=Callback::new(move |r: TaskItem| toggle_star(r))
                            on_drag_set_status=Callback::new(move |(task_id, new_status): (String, &'static str)| {
                                let Some(row) = all_items.with_untracked(|items| items.iter().find(|i| i.id == task_id).cloned()) else {
                                    return;
                                };
                                if row.data.status == new_status || row.role == "read" {
                                    return;
                                }
                                let mut next = row.data.clone();
                                next.status = new_status.to_owned();
                                next.progress = match new_status {
                                    "COMPLETED" => 100,
                                    "IN-PROCESS" => if next.progress == 0 || next.progress == 100 { 50 } else { next.progress },
                                    _ => 0,
                                };
                                let msg = format!(
                                    "Moved \u{201c}{}\u{201d} to {}",
                                    row.data.title,
                                    model::task_status_label(new_status)
                                );
                                patch_task_data(row, next, Some(msg));
                            })
                        />
                    }.into_any(),
                }}

                {move || toast.get().map(|t| view! {
                    <div class="tasks-toast" role="status" aria-live="polite">
                        <span>{t.message}</span>
                        {t.snapshot.is_some().then(|| view! {
                            <button type="button" class="text-button" on:click=move |_| run_undo()>"Undo"</button>
                        })}
                        <button type="button" class="icon-button" aria-label="Dismiss notification" on:click=move |_| toast.set(None)>
                            <Icon name="close"/>
                        </button>
                    </div>
                })}
            </div>
        </WorkspaceShell>

        {move || collection_editor.get().map(|old| {
            let title = if old.is_some() { "Edit list" } else { "New task list" };
            let old_for_save = old.clone();
            let initial = old.unwrap_or_else(|| Collection {
                id: String::new(),
                owner_id: String::new(),
                kind: String::new(),
                name: String::new(),
                color: "#73396f".into(),
                revision: 0,
                role: String::new(),
                subscription: None,
            });
            view! {
                <CollectionEditor
                    title=title
                    collection=initial
                    on_close=Callback::new(move |_| collection_editor.set(None))
                    on_save=Callback::new(move |(name, color, done): (String, String, Done)| {
                        let target = old_for_save.clone();
                        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
                        spawn_local(async move {
                            let (url, method, if_match) = match &target {
                                Some(t) => (format!("{LISTS_BASE}/{}", encode_uri(&t.id)), "PUT", Some(etag(&t.id, t.revision))),
                                None => (LISTS_BASE.to_owned(), "POST", None),
                            };
                            match send::<Collection>(&url, method, Some(json!({ "name": name, "color": color })), &csrf, if_match).await {
                                Ok(saved) => {
                                    if target.is_none() {
                                        collection.try_set(saved.id);
                                        view.try_set("all".into());
                                        if layout.try_get_untracked() == Some(TaskLayout::Board) {
                                            layout.try_set(TaskLayout::List);
                                        }
                                    }
                                    let _ = refresh_all().await;
                                    pim.refresh();
                                    done.try_run(Ok(()));
                                }
                                Err(e) => {
                                    done.try_run(Err(e.message));
                                }
                            }
                        });
                    })
                />
            }
        })}

        {move || sharing.get().map(|target| {
            let base = format!("{LISTS_BASE}/{}", target.id);
            view! {
                <Sharing
                    collection=target
                    base=base
                    session=session_sv.get_value()
                    on_close=Callback::new(move |_| sharing.set(None))
                    on_changed=Callback::new(move |_| {
                        reload.update(|n| *n += 1);
                        pim.refresh();
                    })
                />
            }
        })}

        {move || confirm.get().map(|state| {
            let label = state.title.trim_end_matches('?').to_owned();
            let primary_action = StoredValue::new(state.action.clone());
            let alt = state.alternative.clone();
            let destructive = state.destructive;
            view! {
                <Dialog title=state.title.clone() on_close=Callback::new(move |_| confirm.set(None))>
                    <p>{state.text.clone()}</p>
                    {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                    <div class="dialog-actions">
                        <button type="button" class="text-button" on:click=move |_| confirm.set(None)>"Cancel"</button>
                        {alt.clone().map(|(alt_label, alt_action)| view! {
                            <button type="button" class="danger-button" disabled=move || busy.get() on:click=move |_| run_confirm(alt_action.clone())>{alt_label}</button>
                        })}
                        <button type="button" class=if destructive { "danger-button" } else { "primary-button" } disabled=move || busy.get() on:click=move |_| run_confirm(primary_action.get_value())>{label.clone()}</button>
                    </div>
                </Dialog>
            }
        })}

        {move || move_dialog.get().map(|state| {
            let copy = state.copy;
            let single_source_col = if state.rows.len() == 1 {
                Some(state.rows[0].collection_id.clone())
            } else {
                None
            };
            let initial_dest = collections
                .get_untracked()
                .into_iter()
                .find(|c| c.role != "read" && (copy || single_source_col.as_deref() != Some(c.id.as_str())))
                .map(|c| c.id)
                .unwrap_or_default();
            let dest = RwSignal::new(initial_dest);
            let source_col_opts = single_source_col.clone();
            let dest_options = Signal::derive(move || {
                collections
                    .get()
                    .into_iter()
                    .filter(|c| c.role != "read" && (copy || source_col_opts.as_deref() != Some(c.id.as_str())))
                    .map(|c| (c.id, c.name))
                    .collect::<Vec<_>>()
            });
            let rows_submit = StoredValue::new(state.rows);
            let dlg_title = if copy { "Copy item" } else { "Move item" };
            view! {
                <Dialog title=dlg_title on_close=Callback::new(move |_| move_dialog.set(None))>
                    <form on:submit=move |e: web_sys::SubmitEvent| {
                        e.prevent_default();
                        let collection_id = dest.get_untracked();
                        if collection_id.is_empty() {
                            return;
                        }
                        let rows = rows_submit.get_value();
                        let op = if copy { "copy" } else { "move" };
                        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
                        spawn_local(async move {
                            busy.try_set(true);
                            error.try_set(String::new());
                            let mut failed = None;
                            for r in rows {
                                if !copy && r.collection_id == collection_id {
                                    continue;
                                }
                                if let Err(cause) = send::<Value>(
                                    &format!("{ITEMS_BASE}/{}/{op}", encode_uri(&r.id)),
                                    "POST",
                                    Some(json!({ "collectionId": collection_id })),
                                    &csrf,
                                    Some(etag(&r.id, r.revision)),
                                )
                                .await
                                {
                                    failed = Some(cause.message);
                                    break;
                                }
                            }
                            match failed {
                                None => {
                                    move_dialog.try_set(None);
                                    selected_ids.try_set(HashSet::new());
                                    close_task();
                                    if let Err(msg) = refresh_all().await {
                                        error.try_set(msg);
                                    }
                                    pim.refresh();
                                }
                                Some(msg) => {
                                    error.try_set(msg);
                                }
                            }
                            busy.try_set(false);
                        });
                    }>
                        <label>"Destination"
                            <ReactiveSelect aria_label="Destination" name="collectionId" required=true options=dest_options value=dest/>
                        </label>
                        {move || (!error.get().is_empty()).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                        <button class="primary-button" disabled=move || busy.get() || dest.get().is_empty()>"Save"</button>
                    </form>
                </Dialog>
            }
        })}

        {move || inbox.get().then(|| view! {
            <ReminderInbox session=session_sv.get_value() on_close=Callback::new(move |_| {
                inbox.set(false);
                reload.update(|n| *n += 1);
            })/>
        })}

        {move || shortcuts_open.get().then(|| view! {
            <ShortcutsDialog on_close=Callback::new(move |_| shortcuts_open.set(false))/>
        })}

        {move || task_editor.get().map(|mode| {
            view! {
                <TaskEditor
                    zone=zone.get_untracked()
                    mode=mode
                    collections=Signal::derive(move || collections.get())
                    default_collection=collection.get_untracked()
                    session=session_sv.get_value()
                    on_close=Callback::new(move |_| task_editor.set(None))
                    on_save=Callback::new(move |row: TaskItem| {
                        task_editor.set(None);
                        let id = row.id;
                        spawn_local(async move {
                            if let Err(msg) = refresh_all().await {
                                error.try_set(msg);
                            }
                            open_task_async(id).await;
                            pim.refresh();
                        });
                    })
                />
            }
        })}
    }
}
