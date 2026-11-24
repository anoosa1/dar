//! Tasks (`pim/workspace.tsx` when `kind === "tasks"`): task lists, smart views
//! (All/Inbox/Today/Upcoming/Important/Completed), quick add, due-date sections,
//! task details with subtasks and repeating occurrences, task editor, sharing,
//! reminders inbox, and live PIM synchronization.

use std::sync::Arc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};
use wasm_bindgen::JsValue;

use super::task_occurrences::{ReminderInbox, TaskData, TaskOccurrence, TaskOccurrences};
use super::{ReactiveSelect, WorkspaceShell, browser_timezone, capitalized, search_params};
use crate::api::{self, BrowserSession};
use crate::date_popover::add_days;
use crate::icons::Icon;
use crate::pim::presentation::{recurrence_label, task_date};
use crate::pim::schedule::{ReminderField, RepeatField};
use crate::pim::{
    Collection, CollectionDot, CollectionEditor, CollectionNavigation, Done, Sharing, etag, send,
    use_pim,
};
use crate::shared::{PREFERENCES, Preferences, context, save_preferences};
use crate::ui::{ActionMenu, Dialog, EditorPane, MenuAction, Select, window};

const LISTS_BASE: &str = "/api/v1/tasks/lists";
const ITEMS_BASE: &str = "/api/v1/tasks/items";

const VIEWS: [(&str, &str); 6] = [
    ("all", "All tasks"),
    ("inbox", "Inbox"),
    ("today", "Today"),
    ("upcoming", "Upcoming"),
    ("important", "Important"),
    ("completed", "Completed"),
];

const SECTION_ORDER: [&str; 6] = [
    "Overdue",
    "Today",
    "Tomorrow",
    "Upcoming",
    "No date",
    "Completed",
];

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskChild {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    status: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskItem {
    id: String,
    collection_id: String,
    #[serde(default)]
    uid: String,
    revision: u64,
    data: TaskData,
    #[serde(default)]
    role: String,
    #[serde(default)]
    collection_name: Option<String>,
    #[serde(default)]
    overdue: Option<bool>,
    #[serde(default)]
    next_occurrence: Option<TaskOccurrence>,
    #[serde(skip)]
    editing_occurrence_id: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    children: Option<Vec<TaskChild>>,
}

impl TaskItem {
    fn is_overdue(&self) -> bool {
        self.overdue.unwrap_or(false)
    }

    fn children_slice(&self) -> &[TaskChild] {
        self.children.as_deref().unwrap_or(&[])
    }
}

#[derive(Deserialize)]
struct CollectionList {
    items: Vec<Collection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskList {
    items: Vec<TaskItem>,
    #[serde(default)]
    next_cursor: String,
}

#[derive(Clone, Debug, PartialEq)]
enum TaskEditorMode {
    New {
        collection_id: Option<String>,
        data: Option<TaskData>,
    },
    Edit(TaskItem),
}

#[derive(Clone)]
enum ConfirmAction {
    DeleteTask {
        id: String,
        revision: u64,
        children: Option<&'static str>,
    },
    CompleteWithSubtasks {
        id: String,
        revision: u64,
    },
    DeleteList {
        id: String,
        revision: u64,
    },
}

#[derive(Clone)]
struct ConfirmState {
    title: String,
    text: String,
    destructive: bool,
    action: ConfirmAction,
    alternative: Option<(String, ConfirmAction)>,
}

#[derive(Clone)]
struct MoveState {
    row: TaskItem,
    copy: bool,
}

fn view_icon(view: &str) -> &'static str {
    match view {
        "inbox" => "folder",
        "today" => "calendar",
        "upcoming" => "clock",
        "important" => "flag",
        "completed" => "check",
        _ => "list",
    }
}

fn task_status_label(status: &str) -> &'static str {
    match status {
        "NEEDS-ACTION" => "To do",
        "IN-PROCESS" => "In progress",
        "COMPLETED" => "Completed",
        "CANCELLED" => "Cancelled",
        _ => "",
    }
}

fn task_priority_label(priority: i32) -> &'static str {
    if priority == 0 {
        "None"
    } else if priority < 5 {
        "High"
    } else if priority == 5 {
        "Normal"
    } else {
        "Low"
    }
}

fn priority_class(priority: i32) -> &'static str {
    if (1..=4).contains(&priority) {
        "high"
    } else if priority == 5 {
        "medium"
    } else if priority >= 6 {
        "low"
    } else {
        "none"
    }
}

fn date_slice(value: &str, all_day: bool) -> String {
    value.chars().take(if all_day { 10 } else { 16 }).collect()
}

fn encode_uri(value: &str) -> String {
    js_sys::encode_uri_component(value)
        .as_string()
        .unwrap_or_else(|| value.to_owned())
}

fn today_in_zone(zone: &str) -> String {
    let locales = js_sys::Array::of1(&JsValue::from_str("en-CA"));
    let opts = js_sys::Object::new();
    for (k, v) in [
        ("timeZone", zone),
        ("year", "numeric"),
        ("month", "2-digit"),
        ("day", "2-digit"),
    ] {
        let _ = js_sys::Reflect::set(&opts, &JsValue::from_str(k), &JsValue::from_str(v));
    }
    let fmt = js_sys::Intl::DateTimeFormat::new(&locales, &opts);
    let now = js_sys::Date::new_0();
    fmt.format()
        .call1(&JsValue::NULL, &now)
        .ok()
        .and_then(|v| v.as_string())
        .filter(|s| s.len() >= 10)
        .unwrap_or_else(|| {
            now.to_iso_string()
                .as_string()
                .unwrap_or_else(|| "2026-01-01".into())
                .chars()
                .take(10)
                .collect()
        })
}

fn section_rank(section: &str) -> usize {
    SECTION_ORDER
        .iter()
        .position(|s| *s == section)
        .unwrap_or(SECTION_ORDER.len())
}

/// Due-date sections for task lists sorted by due date (`taskSection`).
fn task_section(row: &TaskItem, today: &str) -> &'static str {
    let data = row
        .next_occurrence
        .as_ref()
        .map(|o| &o.data)
        .unwrap_or(&row.data);
    if matches!(row.data.status.as_str(), "COMPLETED" | "CANCELLED") {
        return "Completed";
    }
    if row.is_overdue() {
        return "Overdue";
    }
    let due: String = data.due.chars().take(10).collect();
    if due.is_empty() {
        return "No date";
    }
    if due.as_str() < today {
        return "Overdue";
    }
    if due == today {
        return "Today";
    }
    let tomorrow = add_days(today, 1);
    if due == tomorrow {
        "Tomorrow"
    } else {
        "Upcoming"
    }
}

/// Compact due date label for list rows (`dueLabel`).
fn due_label(value: &str, all_day: bool, today: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let date: String = value.chars().take(10).collect();
    let tomorrow = add_days(today, 1);
    let yesterday = add_days(today, -1);
    let time: String = if !all_day && value.chars().count() >= 16 {
        value.chars().skip(11).take(5).collect()
    } else {
        String::new()
    };
    let day = if date == today {
        "Today".to_owned()
    } else if date == tomorrow {
        "Tomorrow".to_owned()
    } else if date == yesterday {
        "Yesterday".to_owned()
    } else {
        let date_ms = js_sys::Date::parse(&date);
        let today_ms = js_sys::Date::parse(today);
        let near = !date_ms.is_nan()
            && !today_ms.is_nan()
            && (date_ms - today_ms).abs() < 6.0 * 86_400_000.0;
        let noon_ms = js_sys::Date::parse(&format!("{date}T12:00:00Z"));
        if noon_ms.is_nan() {
            date
        } else if near {
            crate::dates::local_date(
                noon_ms,
                &[
                    ("weekday", "short"),
                    ("month", "short"),
                    ("day", "numeric"),
                    ("timeZone", "UTC"),
                ],
            )
        } else {
            crate::dates::local_date(
                noon_ms,
                &[("month", "short"), ("day", "numeric"), ("timeZone", "UTC")],
            )
        }
    };
    if time.is_empty() {
        day
    } else {
        format!("{day}, {time}")
    }
}

fn set_url_item(id: Option<&str>) {
    let Ok(url) = web_sys::Url::new(&window().location().href().unwrap_or_default()) else {
        return;
    };
    let sp = url.search_params();
    match id {
        Some(id) => sp.set("item", id),
        None => sp.delete("item"),
    }
    let search = sp.to_string().as_string().unwrap_or_default();
    let next = if search.is_empty() {
        url.pathname()
    } else {
        format!("{}?{search}", url.pathname())
    };
    if let Ok(history) = window().history() {
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&next));
    }
}

/// Priority group selector with exact 0–9 override (`PriorityField` in `schedule-controls.tsx`).
#[component]
fn PriorityField(value: RwSignal<String>) -> impl IntoView {
    let group = Memo::new(move |_| {
        let p = value.get().trim().parse::<i32>().unwrap_or(0);
        if p == 0 {
            "0".to_owned()
        } else if p < 5 {
            "1".to_owned()
        } else if p == 5 {
            "5".to_owned()
        } else {
            "9".to_owned()
        }
    });
    let selected = RwSignal::new(group.get_untracked());
    Effect::new(move |_| selected.set(group.get()));
    let options: Vec<(String, String)> =
        [("0", "None"), ("1", "High"), ("5", "Normal"), ("9", "Low")]
            .into_iter()
            .map(|(v, l)| (v.to_owned(), l.to_owned()))
            .collect();
    view! {
        <div>
            <input type="hidden" name="priority" prop:value=move || value.get()/>
            <label>"Priority"
                <Select aria_label="Priority" options=options value=selected on_change=Callback::new(move |next: String| value.set(next))/>
            </label>
            <details>
                <summary>"Exact priority"</summary>
                <label>"Priority number"
                    <input type="number" min="0" max="9" prop:value=move || value.get() on:input=move |e| value.set(event_target_value(&e)) on:change=move |e| value.set(event_target_value(&e))/>
                    <small>"0 means none; 1\u{2013}4 high, 5 normal, 6\u{2013}9 low. Imported priorities are preserved."</small>
                </label>
            </details>
        </div>
    }
}

/// Task creation and editing pane (`TaskEditor`).
#[component]
fn TaskEditor(
    zone: String,
    mode: TaskEditorMode,
    collections: Signal<Vec<Collection>>,
    default_collection: String,
    session: BrowserSession,
    on_close: Callback<()>,
    on_save: Callback<TaskItem>,
) -> impl IntoView {
    let (is_new, edit_id, edit_revision, initial_collection, initial_data, initial_scope) =
        match mode {
            TaskEditorMode::New {
                collection_id,
                data,
            } => (
                true,
                None,
                0u64,
                collection_id,
                data.unwrap_or_default(),
                String::new(),
            ),
            TaskEditorMode::Edit(item) => (
                false,
                Some(item.id),
                item.revision,
                Some(item.collection_id),
                item.data,
                item.editing_occurrence_id.unwrap_or_default(),
            ),
        };

    let initial_list_id = initial_collection.unwrap_or_else(|| {
        collections.with_untracked(|cols| {
            if cols
                .iter()
                .any(|c| c.id == default_collection && c.role != "read")
            {
                default_collection.clone()
            } else {
                cols.iter()
                    .find(|c| c.role != "read")
                    .map(|c| c.id.clone())
                    .unwrap_or_default()
            }
        })
    });

    let title = RwSignal::new(initial_data.title.clone());
    let collection_id = RwSignal::new(initial_list_id);
    let description = RwSignal::new(initial_data.description.clone());
    let all_day = RwSignal::new(initial_data.all_day);
    let start = RwSignal::new(date_slice(&initial_data.start, initial_data.all_day));
    let due = RwSignal::new(date_slice(&initial_data.due, initial_data.all_day));
    let default_zone = StoredValue::new(zone.clone());
    let timezone = RwSignal::new(
        if !initial_data.timezone.is_empty() && initial_data.timezone != "floating" {
            initial_data.timezone.clone()
        } else {
            zone
        },
    );
    let recurrence = RwSignal::new(initial_data.recurrence.clone());
    let reminder_minutes = RwSignal::new(
        initial_data
            .reminder_minutes
            .map(|m| m.to_string())
            .unwrap_or_default(),
    );
    let status = RwSignal::new(if initial_data.status.is_empty() {
        "NEEDS-ACTION".to_owned()
    } else {
        initial_data.status.clone()
    });
    let progress = RwSignal::new(initial_data.progress.to_string());
    let priority = RwSignal::new(initial_data.priority.to_string());
    let categories = RwSignal::new(initial_data.categories.join(", "));
    let parent_uid = RwSignal::new(initial_data.parent_uid.clone());
    let occurrence_scope = RwSignal::new(initial_scope.clone());

    let show_occurrences = !is_new
        && (!initial_data.recurrence.is_empty()
            || initial_data.recurring
            || !initial_scope.is_empty());
    let occ_start = initial_data.start.clone();

    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let csrf = StoredValue::new(session.csrf_token);
    let edit_id_sv = StoredValue::new(edit_id.clone());

    let writable_options = Signal::derive(move || {
        collections
            .get()
            .into_iter()
            .filter(|c| c.role != "read")
            .map(|c| (c.id, c.name))
            .collect::<Vec<_>>()
    });
    let has_writable =
        Signal::derive(move || collections.with(|cols| cols.iter().any(|c| c.role != "read")));

    let status_options: Vec<(String, String)> = [
        ("NEEDS-ACTION", "To do"),
        ("IN-PROCESS", "In progress"),
        ("COMPLETED", "Completed"),
        ("CANCELLED", "Cancelled"),
    ]
    .into_iter()
    .map(|(v, l)| (v.to_owned(), l.to_owned()))
    .collect();

    let on_pick_occurrence = Callback::new(
        move |(item, master, is_occurrence): (TaskData, Option<TaskData>, bool)| {
            all_day.set(item.all_day);
            title.set(item.title);
            description.set(item.description);
            start.set(date_slice(&item.start, item.all_day));
            due.set(date_slice(&item.due, item.all_day));
            status.set(if item.status.is_empty() {
                "NEEDS-ACTION".into()
            } else {
                item.status
            });
            progress.set(item.progress.to_string());
            priority.set(item.priority.to_string());
            categories.set(item.categories.join(", "));
            parent_uid.set(item.parent_uid);
            timezone.set(
                if !item.timezone.is_empty() && item.timezone != "floating" {
                    item.timezone
                } else {
                    default_zone.get_value()
                },
            );
            recurrence.set(if is_occurrence {
                item.recurrence
            } else {
                master.map(|m| m.recurrence).unwrap_or_default()
            });
        },
    );

    let submit = move |e: web_sys::SubmitEvent| {
        e.prevent_default();
        let is_all_day = all_day.get_untracked();
        let cats: Vec<String> = categories
            .get_untracked()
            .split(',')
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect();
        let rem_raw = reminder_minutes.get_untracked();
        let rem_val = if rem_raw.trim().is_empty() {
            Value::Null
        } else {
            rem_raw
                .trim()
                .parse::<i64>()
                .map(Value::from)
                .unwrap_or(Value::Null)
        };
        let data = json!({
            "title": title.get_untracked(),
            "description": description.get_untracked(),
            "start": start.get_untracked(),
            "due": due.get_untracked(),
            "timezone": if is_all_day { "floating".to_owned() } else { timezone.get_untracked() },
            "allDay": is_all_day,
            "priority": priority.get_untracked().trim().parse::<i32>().unwrap_or(0),
            "progress": progress.get_untracked().trim().parse::<i32>().unwrap_or(0),
            "status": status.get_untracked(),
            "parentUid": parent_uid.get_untracked(),
            "categories": cats,
            "recurrence": recurrence.get_untracked(),
            "reminderMinutes": rem_val,
        });
        let occ = occurrence_scope.get_untracked();
        let mut body = json!({ "data": data });
        if let Some(obj) = body.as_object_mut() {
            if is_new {
                obj.insert(
                    "collectionId".into(),
                    Value::String(collection_id.get_untracked()),
                );
            }
            if !occ.is_empty() {
                obj.insert("occurrenceId".into(), Value::String(occ));
            }
        }
        busy.set(true);
        error.set(String::new());
        let token = csrf.get_value();
        let id = edit_id_sv.get_value();
        spawn_local(async move {
            let (url, method, if_match) = match id {
                Some(ref id) if !is_new => (
                    format!("{ITEMS_BASE}/{id}"),
                    "PUT",
                    Some(etag(id, edit_revision)),
                ),
                _ => (ITEMS_BASE.to_owned(), "POST", None),
            };
            match send::<TaskItem>(&url, method, Some(body), &token, if_match).await {
                Ok(row) => {
                    busy.set(false);
                    {
                        on_save.try_run(row);
                    };
                }
                Err(cause) => {
                    error.set(cause.message);
                    busy.set(false);
                }
            }
        });
    };

    view! {
        <EditorPane title=if is_new { "New task" } else { "Edit task" } on_close=on_close>
            <form class="task-editor" on:submit=submit>
                <label class="event-title-field">
                    <span class="sr-only">"Title"</span>
                    <input name="title" aria-label="Title" required autofocus maxlength="1000" placeholder="Task title"
                        prop:value=move || title.get()
                        on:input=move |e| title.set(event_target_value(&e))/>
                </label>
                {is_new.then(|| view! {
                    <label>"List"
                        <ReactiveSelect aria_label="List" name="collectionId" required=true options=writable_options value=collection_id/>
                    </label>
                })}
                <label>"Description"
                    <textarea name="description" rows="3" placeholder="Add details"
                        prop:value=move || description.get()
                        on:input=move |e| description.set(event_target_value(&e))></textarea>
                </label>
                <section class="form-section">
                    <h3><Icon name="calendar"/>"Schedule"</h3>
                    <label class="workspace-check-label">
                        <input type="checkbox"
                            prop:checked=move || all_day.get()
                            on:change=move |e| {
                                let next = event_target_checked(&e);
                                all_day.set(next);
                                if next {
                                    start.update(|s| *s = date_slice(s, true));
                                    due.update(|d| *d = date_slice(d, true));
                                }
                            }/>
                        "Date only"
                    </label>
                    {move || if all_day.get() {
                        view! {
                            <div class="workspace-form-grid">
                                <label>"Starts"
                                    <input name="start" type="date"
                                        prop:value=move || start.get()
                                        on:input=move |e| start.set(event_target_value(&e))
                                        on:change=move |e| start.set(event_target_value(&e))/>
                                </label>
                                <label>"Due"
                                    <input name="due" type="date"
                                        prop:value=move || due.get()
                                        on:input=move |e| due.set(event_target_value(&e))
                                        on:change=move |e| due.set(event_target_value(&e))/>
                                </label>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="workspace-form-grid">
                                <label>"Starts"
                                    <input name="start" type="datetime-local"
                                        prop:value=move || start.get()
                                        on:input=move |e| start.set(event_target_value(&e))
                                        on:change=move |e| start.set(event_target_value(&e))/>
                                </label>
                                <label>"Due"
                                    <input name="due" type="datetime-local"
                                        prop:value=move || due.get()
                                        on:input=move |e| due.set(event_target_value(&e))
                                        on:change=move |e| due.set(event_target_value(&e))/>
                                </label>
                            </div>
                        }.into_any()
                    }}
                    {move || (!all_day.get()).then(|| view! {
                        <label>"Timezone"
                            <input name="timezone" prop:value=move || timezone.get() on:input=move |e| timezone.set(event_target_value(&e))/>
                        </label>
                    })}
                    <RepeatField value=recurrence/>
                    {(show_occurrences).then(|| edit_id.clone().map(|id| view! {
                        <TaskOccurrences id=id start=occ_start.clone() scope=occurrence_scope on_pick=on_pick_occurrence/>
                    })).flatten()}
                    <ReminderField value=reminder_minutes/>
                    <p class="field-hint">"Reminders use the start time, or the due time when no start is set."</p>
                </section>
                <section class="form-section">
                    <h3><Icon name="progress"/>"Progress"</h3>
                    <div class="workspace-form-grid">
                        <label>"Status"
                            <Select aria_label="Status" name="status" options=status_options.clone() value=status/>
                        </label>
                        <label>"Progress (%)"
                            <input name="progress" type="number" min="0" max="100"
                                prop:value=move || progress.get()
                                on:input=move |e| progress.set(event_target_value(&e))
                                on:change=move |e| progress.set(event_target_value(&e))/>
                        </label>
                    </div>
                    <PriorityField value=priority/>
                </section>
                <label>"Categories"
                    <input name="categories" placeholder="Comma-separated"
                        prop:value=move || categories.get()
                        on:input=move |e| categories.set(event_target_value(&e))/>
                </label>
                <details class="form-more">
                    <summary>"Advanced task fields"</summary>
                    <label>"Parent task identifier"
                        <input name="parentUid" prop:value=move || parent_uid.get() on:input=move |e| parent_uid.set(event_target_value(&e))/>
                        <small>"Use Add subtask in a task\u{2019}s details for an ordinary parent relationship."</small>
                    </label>
                </details>
                {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions">
                    <button class="text-button" type="button" disabled=move || busy.get()>"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get() || (is_new && !has_writable.get())>"Save task"</button>
                </div>
                {move || (is_new && collections.with(Vec::is_empty)).then(|| view! { <p>"Create a task list before adding tasks."</p> })}
            </form>
        </EditorPane>
    }
}

/// Task details panel (`TaskCard`).
#[component]
fn TaskCard(
    row: TaskItem,
    collection: Option<Collection>,
    busy: Signal<bool>,
    on_close: Callback<()>,
    on_edit: Callback<()>,
    on_complete: Callback<()>,
    on_subtask: Callback<()>,
    on_delete: Callback<()>,
    on_move: Callback<bool>,
    on_open: Callback<String>,
) -> impl IntoView {
    let task = row.data.clone();
    let done = task.status == "COMPLETED";
    let writable = row.role != "read";
    let overdue = row.is_overdue();
    let priority_text = task_priority_label(task.priority);
    let high = (1..=4).contains(&task.priority);
    let medium = task.priority == 5;
    let children: Vec<TaskChild> = row.children_slice().to_vec();
    let finished = children.iter().filter(|c| c.status == "COMPLETED").count();
    let row_id = row.id.clone();
    let collection_color = collection
        .as_ref()
        .map(|c| c.color.clone())
        .unwrap_or_default();
    let collection_name = collection
        .as_ref()
        .map(|c| c.name.clone())
        .or_else(|| row.collection_name.clone())
        .unwrap_or_default();

    view! {
        <section class="workspace-task-details">
            <div class="pim-detail-top">
                <span>"Task"</span>
                {writable.then(|| view! {
                    <button class="icon-button" aria-label="Edit task" title="Edit task" on:click=move |_| { on_edit.try_run(()); }>
                        <Icon name="pencil"/>
                    </button>
                })}
                <ActionMenu label="Task options" actions=move |_| {
                    let is_busy = busy.get_untracked();
                    let mut list = Vec::new();
                    if writable {
                        list.push(MenuAction::new("Move to\u{2026}", "move").disabled(is_busy).run(move || { on_move.try_run(false); }));
                    }
                    list.push(MenuAction::new("Copy to\u{2026}", "copy").disabled(is_busy).run(move || { on_move.try_run(true); }));
                    list.push(MenuAction::new("Export task", "download").href(format!("{ITEMS_BASE}/{row_id}/export")));
                    if writable {
                        list.push(MenuAction::new("Delete task", "trash").danger().separator().disabled(is_busy).run(move || { on_delete.try_run(()); }));
                    }
                    list
                }>
                    <Icon name="more"/>
                </ActionMenu>
                <button class="icon-button" aria-label="Close task details" title="Close" on:click=move |_| { on_close.try_run(()); }>
                    <Icon name="close"/>
                </button>
            </div>
            <div class="task-card-title">
                <span class=format!("task-check is-large priority-{}", priority_class(task.priority)) aria-hidden="true" data-done=done.then_some("true")>
                    {done.then(|| view! { <Icon name="check"/> })}
                </span>
                <div>
                    <h2>{task.title.clone()}</h2>
                    {if !task.description.is_empty() {
                        view! { <p class="pim-notes">{task.description.clone()}</p> }.into_any()
                    } else {
                        view! { <p class="muted">"No description"</p> }.into_any()
                    }}
                </div>
            </div>
            {row.next_occurrence.as_ref().map(|next| {
                let dt = if !next.data.due.is_empty() { &next.data.due } else { &next.data.start };
                view! { <p class="task-next">{format!("Next pending occurrence: {}", task_date(dt, next.data.all_day))}</p> }
            })}
            {writable.then(|| view! {
                <div class="workspace-actions">
                    <button class=if done { "secondary-button" } else { "primary-button" } disabled=move || busy.get() on:click=move |_| { on_complete.try_run(()); }>
                        <Icon name=if done { "undo" } else { "check" }/>
                        {if done { "Reopen task" } else { "Complete task" }}
                    </button>
                    <button class="secondary-button" on:click=move |_| { on_subtask.try_run(()); }>
                        <Icon name="subtask"/>"Add subtask"
                    </button>
                </div>
            })}
            <div class="property-card">
                <div class="property">
                    <Icon name="calendar"/>
                    <span class="property-key">"Due"</span>
                    <span class=format!("property-value{}", if overdue { " is-overdue" } else { "" })>
                        {if !task.due.is_empty() { task_date(&task.due, task.all_day) } else { "No due date".to_owned() }}
                        {overdue.then(|| view! { <span class="status-chip danger">"Overdue"</span> })}
                    </span>
                </div>
                {(!task.start.is_empty()).then(|| view! {
                    <div class="property">
                        <Icon name="clock"/>
                        <span class="property-key">"Starts"</span>
                        <span class="property-value">{task_date(&task.start, task.all_day)}</span>
                    </div>
                })}
                <div class="property">
                    <Icon name="flag"/>
                    <span class="property-key">"Priority"</span>
                    <span class="property-value">
                        {if task.priority != 0 {
                            let cls = if high { "status-chip danger" } else if medium { "status-chip warning" } else { "status-chip neutral" };
                            view! { <span class=cls>{priority_text}</span> }.into_any()
                        } else {
                            view! { {priority_text} }.into_any()
                        }}
                    </span>
                </div>
                <div class="property">
                    <Icon name="progress"/>
                    <span class="property-key">"Status"</span>
                    <span class="property-value">
                        {task_status_label(&task.status)}
                        <progress max="100" value=task.progress.to_string() aria-label="Progress"></progress>
                        <span>{format!("{}%", task.progress)}</span>
                    </span>
                </div>
                <div class="property">
                    <Icon name="repeat"/>
                    <span class="property-key">"Repeats"</span>
                    <span class="property-value">
                        <span>{recurrence_label(&task.recurrence, task.recurring)}</span>
                        {(!task.recurrence.is_empty()).then(|| view! {
                            <details><summary>"Schedule details"</summary><code>{task.recurrence.clone()}</code></details>
                        })}
                    </span>
                </div>
                <div class="property">
                    <Icon name="list"/>
                    <span class="property-key">"List"</span>
                    <span class="property-value">
                        <CollectionDot color=collection_color/>
                        {collection_name}
                    </span>
                </div>
                {(!task.categories.is_empty()).then(|| view! {
                    <div class="property">
                        <Icon name="tag"/>
                        <span class="property-key">"Tags"</span>
                        <span class="property-value">
                            {task.categories.iter().cloned().map(|tag| view! { <span class="row-tag">{format!("#{tag}")}</span> }).collect_view()}
                        </span>
                    </div>
                })}
            </div>
            {row.parent_id.clone().map(|pid| view! {
                <div class="property-card">
                    <button class="property property-action" on:click=move |_| { on_open.try_run(pid.clone()); }>
                        <Icon name="back"/>
                        <div><span>"Open parent task"</span></div>
                    </button>
                </div>
            })}
            {(!children.is_empty()).then(|| {
                let total = children.len();
                view! {
                    <div class="property-card">
                        <h3>{format!("Subtasks \u{00b7} {finished} of {total}")}</h3>
                        {children.into_iter().map(|child| {
                            let cid = child.id.clone();
                            let cdone = child.status == "COMPLETED";
                            view! {
                                <button class="property property-action task-subtask" on:click=move |_| { on_open.try_run(cid.clone()); }>
                                    <span class="task-check" data-done=cdone.then_some("true") aria-hidden="true">
                                        {cdone.then(|| view! { <Icon name="check"/> })}
                                    </span>
                                    <div>
                                        <span>{child.title}</span>
                                        <span class="sr-only">{if cdone { "Completed" } else { "Open" }}</span>
                                    </div>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                }
            })}
        </section>
    }
}

/// Tasks application (`Workspace` with `kind="tasks"`).
#[component]
pub fn TasksWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let pim = use_pim(on_signed_out);
    let shared_prefs = context().preferences;
    let session_sv = StoredValue::new(session.clone());

    let collections: RwSignal<Vec<Collection>> = RwSignal::new(Vec::new());
    let items: RwSignal<Vec<TaskItem>> = RwSignal::new(Vec::new());
    let selected: RwSignal<Option<TaskItem>> = RwSignal::new(None);
    let query = RwSignal::new(String::new());
    let collection = RwSignal::new(String::new());
    let view = RwSignal::new("today".to_owned());
    let sort = RwSignal::new("due".to_owned());
    let zone = RwSignal::new(browser_timezone());
    let cursor = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let selection_error = RwSignal::new(String::new());
    let loading = RwSignal::new(true);
    let busy = RwSignal::new(false);
    let notice = RwSignal::new(String::new());
    let quick = RwSignal::new(String::new());

    let collection_editor: RwSignal<Option<Option<Collection>>> = RwSignal::new(None);
    let sharing: RwSignal<Option<Collection>> = RwSignal::new(None);
    let task_editor: RwSignal<Option<TaskEditorMode>> = RwSignal::new(None);
    let confirm: RwSignal<Option<ConfirmState>> = RwSignal::new(None);
    let move_dialog: RwSignal<Option<MoveState>> = RwSignal::new(None);
    let inbox = RwSignal::new(false);

    let latest_load = RwSignal::new(0u32);
    let sel_seq = RwSignal::new(0u32);
    let reload = RwSignal::new(0u32);

    // Synchronize sort, timezone and default view from account preferences.
    let pref_sort = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("tasksSort").and_then(Value::as_str))
                .map(str::to_owned)
        })
    });
    let pref_zone = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("timezone").and_then(Value::as_str))
                .map(str::to_owned)
        })
    });
    let pref_view = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("tasksView").and_then(Value::as_str))
                .map(str::to_owned)
        })
    });

    Effect::new(move |_| {
        if let Some(s) = pref_sort.get() {
            sort.set(s);
        }
    });
    Effect::new(move |_| {
        if let Some(z) = pref_zone.get() {
            zone.set(z);
        }
    });
    Effect::new(move |_| {
        if let Some(v) = pref_view.get()
            && !v.is_empty()
        {
            view.set(v);
        }
    });

    let today = Memo::new(move |_| today_in_zone(&zone.get()));
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
        collections.with(|cols| cols.iter().find(|c| c.role != "read").cloned())
    });

    let close_task = move || {
        selected.set(None);
        set_url_item(None);
    };

    let open_task_async = move |id: String| async move {
        match api::get::<TaskItem>(&format!("{ITEMS_BASE}/{}", encode_uri(&id))).await {
            Ok(value) => {
                set_url_item(Some(&value.id));
                selected.set(Some(value));
            }
            Err(cause) => error.set(cause.message),
        }
    };

    let open_task = move |id: String| {
        spawn_local(open_task_async(id));
    };

    let refresh_list = move |append: bool| async move {
        let generation = latest_load.get_untracked() + 1;
        latest_load.set(generation);
        let books = api::get::<CollectionList>(LISTS_BASE)
            .await
            .map_err(|e| e.message)?;
        if latest_load.try_get_untracked() != Some(generation) {
            return Ok(());
        }
        collections.set(books.items);
        let mut pairs = vec![
            ("q", query.get_untracked()),
            ("view", view.get_untracked()),
            ("sort", sort.get_untracked()),
            ("collection", collection.get_untracked()),
            ("timezone", zone.get_untracked()),
        ];
        if append {
            let cur = cursor.get_untracked();
            if !cur.is_empty() {
                pairs.push(("cursor", cur));
            }
        }
        let qs = search_params(&pairs);
        let result = api::get::<TaskList>(&format!("{ITEMS_BASE}?{qs}"))
            .await
            .map_err(|e| e.message)?;
        if latest_load.try_get_untracked() == Some(generation) {
            if append {
                items.update(|old| old.extend(result.items));
            } else {
                items.set(result.items);
            }
            cursor.set(result.next_cursor);
        }
        Ok::<(), String>(())
    };

    // Load lists and tasks whenever filters, timezone, live epoch or manual reload change.
    Effect::new(move |_| {
        let _q = query.get();
        let _v = view.get();
        let _s = sort.get();
        let _c = collection.get();
        let _z = zone.get();
        let _e = pim.epoch.get();
        let _r = reload.get();
        loading.set(true);
        spawn_local(async move {
            if let Err(msg) = refresh_list(false).await {
                error.set(msg);
            }
            loading.set(false);
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
                    Ok(value) => selected.set(Some(value)),
                    Err(cause) => error.set(cause.message),
                }
            });
        }
    }

    // Refresh the selected task whenever live PIM data changes or a new task is selected.
    let selected_id = Memo::new(move |_| selected.with(|s| s.as_ref().map(|i| i.id.clone())));
    Effect::new(move |_| {
        let _epoch = pim.epoch.get();
        selection_error.set(String::new());
        // A new selection, or closing, makes any refresh still in flight stale.
        let seq = sel_seq.get_untracked() + 1;
        sel_seq.set(seq);
        let Some(id) = selected_id.get() else { return };
        spawn_local(async move {
            match api::get::<TaskItem>(&format!("{ITEMS_BASE}/{}", encode_uri(&id))).await {
                Ok(value) if sel_seq.try_get_untracked() == Some(seq) => selected.set(Some(value)),
                Err(cause) if sel_seq.try_get_untracked() == Some(seq) => {
                    if matches!(cause.status, 401 | 403 | 404 | 410) {
                        close_task();
                        error.set("The selected item is no longer accessible.".into());
                    } else {
                        selection_error.set(
                            "This task could not be refreshed. Your editor is still open.".into(),
                        );
                    }
                }
                _ => {}
            }
        });
    });

    let change_sort = move |value: String| {
        sort.set(value.clone());
        let sess = session_sv.get_value();
        spawn_local(async move {
            match api::get::<Preferences>(PREFERENCES).await {
                Ok(old) => {
                    let rev = old.revision;
                    let mut body = serde_json::to_value(&old).unwrap_or_else(|_| json!({}));
                    if let Some(obj) = body.as_object_mut() {
                        obj.insert("tasksSort".into(), Value::String(value));
                    }
                    if let Err(cause) = save_preferences(body, rev, &sess).await {
                        error.set(cause.message);
                    }
                }
                Err(cause) => error.set(cause.message),
            }
        });
    };

    let quick_add = move || {
        let title = quick.get_untracked().trim().to_owned();
        let Some(target) = quick_target.get_untracked() else {
            return;
        };
        if title.is_empty() {
            return;
        }
        let current_view = view.get_untracked();
        let current_today = today.get_untracked();
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            busy.set(true);
            error.set(String::new());
            notice.set(String::new());
            let mut data = json!({ "title": title });
            if let Some(obj) = data.as_object_mut() {
                if current_view == "today" {
                    obj.insert("due".into(), Value::String(current_today));
                    obj.insert("start".into(), Value::String(String::new()));
                    obj.insert("allDay".into(), Value::Bool(true));
                    obj.insert("timezone".into(), Value::String("floating".into()));
                }
                if current_view == "important" {
                    obj.insert("priority".into(), Value::from(1));
                }
            }
            let body = json!({ "collectionId": target.id, "data": data });
            match send::<TaskItem>(ITEMS_BASE, "POST", Some(body), &csrf, None).await {
                Ok(row) => {
                    quick.set(String::new());
                    notice.set(format!(
                        "Added \u{201c}{}\u{201d} to {}.",
                        row.data.title, target.name
                    ));
                    if let Err(msg) = refresh_list(false).await {
                        error.set(msg);
                    }
                    pim.refresh();
                }
                Err(cause) => error.set(cause.message),
            }
            busy.set(false);
        });
    };

    let remove_task = move |row: TaskItem| {
        spawn_local(async move {
            match api::get::<TaskItem>(&format!("{ITEMS_BASE}/{}", encode_uri(&row.id))).await {
                Ok(full) => {
                    let has_children = !full.children_slice().is_empty();
                    confirm.set(Some(ConfirmState {
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
                Err(cause) => error.set(cause.message),
            }
        });
    };

    let complete_task = move |row: TaskItem| {
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            busy.set(true);
            error.set(String::new());
            notice.set(String::new());
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
                    confirm.set(Some(ConfirmState {
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
                    edited.editing_occurrence_id = next_occ.as_ref().map(|o| o.recurrence_id.clone());
                    let mut next_data = next_occ.map(|o| o.data).unwrap_or_else(|| full.data.clone());
                    let reopen = data.status == "COMPLETED";
                    next_data.status = if reopen { "NEEDS-ACTION".into() } else { "COMPLETED".into() };
                    next_data.progress = if reopen { 0 } else { 100 };
                    edited.data = next_data;
                    task_editor.set(Some(TaskEditorMode::Edit(edited)));
                    notice.set(
                        "Choose an occurrence in the task editor to complete that occurrence. Completing the series stops future reminders."
                            .into(),
                    );
                    return Ok(());
                }
                let reopen = data.status == "COMPLETED";
                let mut updated = data;
                updated.status = if reopen { "NEEDS-ACTION".into() } else { "COMPLETED".into() };
                updated.progress = if reopen { 0 } else { 100 };
                send::<Value>(
                    &format!("{ITEMS_BASE}/{}", encode_uri(&row.id)),
                    "PUT",
                    Some(json!({ "data": updated })),
                    &csrf,
                    Some(etag(&row.id, row.revision)),
                )
                .await
                .map_err(|e| e.message)?;
                if selected.with_untracked(|s| s.as_ref().map(|x| x.id.as_str()) == Some(row.id.as_str())) {
                    open_task_async(row.id.clone()).await;
                }
                Ok(())
            }
            .await;
            match outcome {
                Ok(()) => {
                    if let Err(msg) = refresh_list(false).await {
                        error.set(msg);
                    }
                    pim.refresh();
                }
                Err(msg) => error.set(msg),
            }
            busy.set(false);
        });
    };

    let run_confirm = move |action: ConfirmAction| {
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            busy.set(true);
            error.set(String::new());
            notice.set(String::new());
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
                    .map(|_| ())
                    .map_err(|e| e.message)
                    .map(|()| close_task())
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
                        collection.set(String::new());
                    }
                    if selected
                        .with_untracked(|s| s.as_ref().is_some_and(|x| x.collection_id == id))
                    {
                        close_task();
                    }
                })
                .map_err(|e| e.message),
            };
            match result {
                Ok(()) => {
                    confirm.set(None);
                    if let Err(msg) = refresh_list(false).await {
                        error.set(msg);
                    }
                    pim.refresh();
                }
                Err(msg) => error.set(msg),
            }
            busy.set(false);
        });
    };

    // Computed display rows without subscribing to `selected`, so opening/closing
    // task details preserves `.workspace-row-content` DOM identity for focus restoration.
    let displayed_rows = Memo::new(move |_| {
        let mut list = items.get();
        let s = sort.get();
        let t = today.get();
        if s == "due" {
            list.sort_by_key(|row| section_rank(task_section(row, &t)));
        }
        let mut prev_section: Option<&'static str> = None;
        list.into_iter()
            .map(|row| {
                let section = if s == "due" {
                    let sec = task_section(&row, &t);
                    let show = prev_section != Some(sec);
                    prev_section = Some(sec);
                    show.then_some(sec)
                } else {
                    None
                };
                (row, section)
            })
            .collect::<Vec<_>>()
    });

    let sidebar: ChildrenFn = Arc::new(move || {
        view! {
            <button type="button" class="new-button ui-new" disabled=move || busy.get() on:click=move |_| {
                if !collections.with_untracked(|cols| cols.iter().any(|c| c.role != "read")) {
                    collection_editor.set(Some(None));
                } else {
                    task_editor.set(Some(TaskEditorMode::New { collection_id: None, data: None }));
                }
            }>
                <Icon name="plus"/>"New task"
            </button>
            <nav aria-label="tasks views">
                {VIEWS.into_iter().map(|(val, label)| {
                    view! {
                        <button
                            class=move || if view.get() == val && collection.get().is_empty() { "nav-item active" } else { "nav-item " }
                            aria-current=move || (view.get() == val && collection.get().is_empty()).then_some("page")
                            on:click=move |_| {
                                view.set(val.to_owned());
                                collection.set(String::new());
                                close_task();
                            }>
                            <Icon name=view_icon(val)/>{label}
                        </button>
                    }
                }).collect_view()}
            </nav>
            <div class="sidebar-heading">
                "Task lists"
                <button class="text-button" aria-label="New task list" on:click=move |_| collection_editor.set(Some(None))>"\u{ff0b}"</button>
            </div>
            <CollectionNavigation
                rows=Signal::derive(move || collections.get())
                selected=Signal::derive(move || collection.get())
                noun="task list"
                on_select=Callback::new(move |c: Collection| {
                    collection.set(c.id);
                    view.set("all".into());
                    close_task();
                })
                on_edit=Callback::new(move |row: Collection| collection_editor.set(Some(Some(row))))
                on_share=Callback::new(move |row: Collection| sharing.set(Some(row)))
                on_delete=Callback::new(move |target: Collection| {
                    confirm.set(Some(ConfirmState {
                        title: "Delete task list?".into(),
                        destructive: true,
                        text: format!("Delete \u{201c}{}\u{201d}? All events and tasks in this calendar will be permanently removed.", target.name),
                        action: ConfirmAction::DeleteList { id: target.id, revision: target.revision },
                        alternative: None,
                    }));
                })
            />
            <div class="sidebar-footer-row">
                <button class="nav-item" on:click=move |_| inbox.set(true)>
                    <Icon name="bell"/>
                    <span>"Reminders"</span>
                    <small>{move || zone.get()}</small>
                </button>
            </div>
        }
        .into_any()
    });

    let details: ChildrenFn = Arc::new(move || {
        view! {
            {move || (!selection_error.get().is_empty()).then(|| view! {
                <div role="alert" class="error">
                    <span>{selection_error.get()}</span>
                    <button class="text-button" on:click=move |_| pim.refresh()>"Retry refresh"</button>
                </div>
            })}
            {move || selected.get().map(|row| {
                let col = collections.with(|cols| cols.iter().find(|c| c.id == row.collection_id).cloned());
                let row_edit = row.clone();
                let row_comp = row.clone();
                let row_sub = row.clone();
                let row_del = row.clone();
                let row_move = row.clone();
                view! {
                    <TaskCard
                        row=row
                        collection=col
                        busy=busy.into()
                        on_close=Callback::new(move |_| close_task())
                        on_edit=Callback::new(move |_| task_editor.set(Some(TaskEditorMode::Edit(row_edit.clone()))))
                        on_complete=Callback::new(move |_| complete_task(row_comp.clone()))
                        on_subtask=Callback::new(move |_| {
                            let mut data = row_sub.data.clone();
                            data.title = String::new();
                            data.parent_uid = row_sub.uid.clone();
                            data.status = "NEEDS-ACTION".into();
                            data.progress = 0;
                            data.completed_at = String::new();
                            data.recurrence = String::new();
                            data.recurring = false;
                            data.reminder_minutes = None;
                            task_editor.set(Some(TaskEditorMode::New {
                                collection_id: Some(row_sub.collection_id.clone()),
                                data: Some(data),
                            }));
                        })
                        on_delete=Callback::new(move |_| remove_task(row_del.clone()))
                        on_move=Callback::new(move |copy: bool| move_dialog.set(Some(MoveState { row: row_move.clone(), copy })))
                        on_open=Callback::new(move |id: String| open_task(id))
                    />
                }
            })}
        }
        .into_any()
    });

    let has_details = Signal::from(Memo::new(move |_| selected.with(Option::is_some)));

    view! {
        <WorkspaceShell kind="tasks" session=session on_signed_out=on_signed_out sidebar=sidebar has_details=has_details details=details pim=pim>
            <div class="workspace-list tasks-list">
                <div class="workspace-heading">
                    <h1>{move || {
                        if let Some(c) = current_collection.get() {
                            c.name
                        } else {
                            let v = view.get();
                            if v == "all" { "All tasks".to_owned() } else { capitalized(&v) }
                        }
                    }}</h1>
                    <ActionMenu label="Sort tasks" class="workspace-sort" actions=move |_| {
                        let cur = sort.get_untracked();
                        [("modified", "Recently edited"), ("title", "Title"), ("due", "Due date")]
                            .into_iter()
                            .map(|(val, label)| {
                                let v = val.to_owned();
                                MenuAction::plain(label).current(cur == val).run(move || change_sort(v.clone()))
                            })
                            .collect()
                    }>
                        <Icon name="sort"/>
                        <span>{move || match sort.get().as_str() {
                            "modified" => "Edited",
                            "title" => "Title",
                            "due" => "Due date",
                            _ => "Sort",
                        }}</span>
                    </ActionMenu>
                </div>
                {move || (view.get() != "completed").then(|| view! {
                    <form class="task-quick-add" on:submit=move |e: web_sys::SubmitEvent| {
                        e.prevent_default();
                        quick_add();
                    }>
                        <Icon name="plus"/>
                        <label class="sr-only" for="task-quick-add">"Add a task"</label>
                        <input id="task-quick-add"
                            prop:value=move || quick.get()
                            maxlength="1000"
                            disabled=move || busy.get() || quick_target.with(Option::is_none)
                            placeholder=move || match quick_target.get() {
                                Some(t) => format!("Add a task to {}{}", t.name, if view.get() == "today" { " for today" } else { "" }),
                                None => "Create a task list to add tasks".to_owned(),
                            }
                            on:input=move |e| quick.set(event_target_value(&e))/>
                        {move || (!quick.get().trim().is_empty()).then(|| view! { <kbd aria-hidden="true">"Enter"</kbd> })}
                    </form>
                })}
                {move || (!error.get().is_empty()).then(|| view! {
                    <div role="alert" class="error">
                        {error.get()}
                        <button class="text-button" on:click=move |_| {
                            error.set(String::new());
                            reload.update(|n| *n += 1);
                            pim.refresh();
                        }>"Reload"</button>
                    </div>
                })}
                {move || (!notice.get().is_empty()).then(|| view! { <p role="status">{notice.get()}</p> })}
                {move || loading.get().then(|| view! { <p role="status">"Loading tasks\u{2026}"</p> })}
                <For
                    each=move || displayed_rows.get()
                    key=|(row, sec)| (
                        row.id.clone(),
                        row.revision,
                        row.data.status.clone(),
                        row.data.title.clone(),
                        row.is_overdue(),
                        row.next_occurrence.as_ref().map(|o| o.recurrence_id.clone()),
                        *sec,
                    )
                    children=move |(row, section)| {
                        let task = row.data.clone();
                        let next_due = row.next_occurrence.as_ref().map(|o| o.data.due.clone()).unwrap_or_else(|| task.due.clone());
                        let is_done = task.status == "COMPLETED";
                        let read_only = row.role == "read";
                        let overdue = row.is_overdue();
                        let row_id_sel = row.id.clone();
                        let row_id_open = row.id.clone();
                        let row_check = row.clone();
                        let row_actions = row.clone();
                        let row_col_id = row.collection_id.clone();
                        let title_display = if task.title.is_empty() { "Untitled task".to_owned() } else { task.title.clone() };
                        let check_label = format!("{} {}", if is_done { "Reopen" } else { "Complete" }, task.title);
                        let action_label = format!("Actions for {}", task.title);
                        let all_day = task.all_day;
                        view! {
                            <div class="workspace-entry">
                                {section.map(|sec| view! {
                                    <h2 class=format!("task-section task-section-{}", sec.to_lowercase().replace(' ', "-"))>{sec}</h2>
                                })}
                                <article class=move || {
                                    let is_sel = selected.with(|s| s.as_ref().map(|x| x.id.as_str()) == Some(row_id_sel.as_str()));
                                    format!("workspace-row {}{}", if is_sel { "selected" } else { "" }, if is_done { " is-done" } else { "" })
                                }>
                                    <button
                                        class=format!("task-check priority-{}", priority_class(task.priority))
                                        aria-pressed=is_done.to_string()
                                        aria-label=check_label
                                        disabled=move || busy.get() || read_only
                                        on:click=move |_| complete_task(row_check.clone())>
                                        {is_done.then(|| view! { <Icon name="check"/> })}
                                    </button>
                                    <button class="workspace-row-content" on:click=move |_| {
                                        if selected.with_untracked(|s| s.as_ref().map(|x| x.id.as_str()) != Some(row_id_open.as_str())) {
                                            open_task(row_id_open.clone());
                                        }
                                    }>
                                        <strong>{title_display}</strong>
                                        <span class="row-meta">
                                            {(!next_due.is_empty()).then(|| {
                                                let due_val = next_due.clone();
                                                view! {
                                                    <span class=overdue.then_some("is-overdue")>
                                                        <Icon name="calendar"/>
                                                        {move || due_label(&due_val, all_day, &today.get())}
                                                        {overdue.then(|| view! { <span class="sr-only">" (overdue)"</span> })}
                                                    </span>
                                                }
                                            })}
                                            {(!task.recurrence.is_empty() || task.recurring).then(|| view! {
                                                <span><Icon name="repeat"/>{recurrence_label(&task.recurrence, task.recurring)}</span>
                                            })}
                                            {(task.status == "IN-PROCESS").then(|| view! {
                                                <span><Icon name="progress"/>{format!("{}%", task.progress)}</span>
                                            })}
                                            {move || {
                                                let cid = row_col_id.clone();
                                                collection.get().is_empty().then(|| {
                                                    collections.with(|cols| cols.iter().find(|c| c.id == cid).cloned()).map(|book| view! {
                                                        <span><CollectionDot color=book.color/>{book.name}</span>
                                                    })
                                                }).flatten()
                                            }}
                                            {(!task.parent_uid.is_empty()).then(|| view! {
                                                <span><Icon name="subtask"/>"Subtask"</span>
                                            })}
                                            {task.categories.iter().cloned().map(|tag| view! {
                                                <span class="row-tag">{format!("#{tag}")}</span>
                                            }).collect_view()}
                                        </span>
                                    </button>
                                    <div class="workspace-row-tools">
                                        {(!read_only).then(|| view! {
                                            <ActionMenu label=action_label class="ui-file-actions" actions=move |_| {
                                                let is_busy = busy.get_untracked();
                                                let cols = collections.get_untracked();
                                                let source_owner = cols.iter().find(|c| c.id == row_actions.collection_id).map(|c| c.owner_id.as_str());
                                                let can_move = !is_busy && cols.iter().any(|c| c.id != row_actions.collection_id && c.role != "read" && Some(c.owner_id.as_str()) == source_owner);
                                                let r_move = row_actions.clone();
                                                let r_copy = row_actions.clone();
                                                let r_del = row_actions.clone();
                                                vec![
                                                    MenuAction::new("Move to\u{2026}", "move").disabled(!can_move).run(move || move_dialog.set(Some(MoveState { row: r_move.clone(), copy: false }))),
                                                    MenuAction::new("Copy to\u{2026}", "copy").disabled(is_busy).run(move || move_dialog.set(Some(MoveState { row: r_copy.clone(), copy: true }))),
                                                    MenuAction::new("Export task", "download").href(format!("{ITEMS_BASE}/{}/export", row_actions.id)),
                                                    MenuAction::new("Delete task", "trash").danger().separator().disabled(is_busy).run(move || remove_task(r_del.clone())),
                                                ]
                                            }>
                                                <Icon name="more"/>
                                            </ActionMenu>
                                        })}
                                    </div>
                                </article>
                            </div>
                        }
                    }
                />
                {move || (!loading.get() && items.with(Vec::is_empty)).then(|| {
                    let has_query = !query.get().is_empty();
                    view! {
                        <div class="workspace-empty">
                            <h2>{if has_query { "No matches" } else { "No tasks in this view" }}</h2>
                            <p>{if has_query { "Try another search." } else { "Add a task, or choose another view." }}</p>
                        </div>
                    }
                })}
                {move || (!cursor.get().is_empty()).then(|| view! {
                    <button class="secondary-button" disabled=move || busy.get() on:click=move |_| {
                        spawn_local(async move {
                            busy.set(true);
                            if let Err(msg) = refresh_list(true).await {
                                error.set(msg);
                            }
                            busy.set(false);
                        });
                    }>"Load more"</button>
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
                                        collection.set(saved.id);
                                        view.set("all".into());
                                    }
                                    let _ = refresh_list(false).await;
                                    pim.refresh();
                                    { done.try_run(Ok(())); };
                                }
                                Err(e) => { done.try_run(Err(e.message)); },
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
                        <button class="text-button" on:click=move |_| confirm.set(None)>"Cancel"</button>
                        {alt.clone().map(|(alt_label, alt_action)| view! {
                            <button class="danger-button" disabled=move || busy.get() on:click=move |_| run_confirm(alt_action.clone())>{alt_label}</button>
                        })}
                        <button class=if destructive { "danger-button" } else { "primary-button" } disabled=move || busy.get() on:click=move |_| run_confirm(primary_action.get_value())>{label.clone()}</button>
                    </div>
                </Dialog>
            }
        })}

        {move || move_dialog.get().map(|state| {
            let copy = state.copy;
            let source_col = state.row.collection_id.clone();
            let initial_dest = collections
                .get_untracked()
                .into_iter()
                .find(|c| c.role != "read" && (copy || c.id != source_col))
                .map(|c| c.id)
                .unwrap_or_default();
            let dest = RwSignal::new(initial_dest);
            let source_col_opts = source_col.clone();
            let dest_options = Signal::derive(move || {
                collections
                    .get()
                    .into_iter()
                    .filter(|c| c.role != "read" && (copy || c.id != source_col_opts))
                    .map(|c| (c.id, c.name))
                    .collect::<Vec<_>>()
            });
            let row_submit = StoredValue::new(state.row);
            view! {
                <Dialog title=if copy { "Copy item" } else { "Move item" } on_close=Callback::new(move |_| move_dialog.set(None))>
                    <form on:submit=move |e: web_sys::SubmitEvent| {
                        e.prevent_default();
                        let collection_id = dest.get_untracked();
                        if collection_id.is_empty() {
                            return;
                        }
                        let r = row_submit.get_value();
                        let op = if copy { "copy" } else { "move" };
                        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
                        spawn_local(async move {
                            busy.set(true);
                            error.set(String::new());
                            notice.set(String::new());
                            match send::<Value>(
                                &format!("{ITEMS_BASE}/{}/{op}", encode_uri(&r.id)),
                                "POST",
                                Some(json!({ "collectionId": collection_id })),
                                &csrf,
                                Some(etag(&r.id, r.revision)),
                            )
                            .await
                            {
                                Ok(_) => {
                                    move_dialog.set(None);
                                    close_task();
                                    if let Err(msg) = refresh_list(false).await {
                                        error.set(msg);
                                    }
                                    pim.refresh();
                                }
                                Err(cause) => error.set(cause.message),
                            }
                            busy.set(false);
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
            <ReminderInbox session=session_sv.get_value() on_close=Callback::new(move |_| inbox.set(false))/>
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
                        view.set("all".into());
                        let id = row.id;
                        spawn_local(async move {
                            if let Err(msg) = refresh_list(false).await {
                                error.set(msg);
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
