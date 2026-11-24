//! Direct-manipulation TaskInspector (right sheet inside `aside[aria-label="Task details"]`)
//! and full RFC 5545 VTODO TaskEditor (`EditorPane`).

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use super::model::{
    ITEMS_BASE, TaskData, TaskEditorMode, TaskItem, date_slice, priority_class, task_status_label,
    visible_url,
};
use crate::api::BrowserSession;
use crate::date_popover::add_days;
use crate::icons::Icon;
use crate::pim::presentation::{recurrence_label, task_date};
use crate::pim::schedule::{ReminderField, RepeatField};
use crate::pim::{Collection, CollectionDot, etag, send};
use crate::ui::{ActionMenu, EditorPane, MenuAction, Select};
use crate::workspace::ReactiveSelect;
use crate::workspace::task_occurrences::TaskOccurrences;

/// Priority group selector with exact 0–9 override (`PriorityField`).
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
                    <input
                        type="number"
                        min="0"
                        max="9"
                        prop:value=move || value.get()
                        on:input=move |e| value.set(event_target_value(&e))
                        on:change=move |e| value.set(event_target_value(&e))
                    />
                    <small>"0 means none; 1\u{2013}4 high, 5 normal, 6\u{2013}9 low. Imported priorities are preserved."</small>
                </label>
            </details>
        </div>
    }
}

/// Full task creation and editing pane (`EditorPane`), preserving every RFC 5545 VTODO field.
#[component]
pub fn TaskEditor(
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
    let location = RwSignal::new(initial_data.location.clone());
    let url = RwSignal::new(initial_data.url.clone());
    let starred = RwSignal::new(initial_data.starred);
    let sort_order = RwSignal::new(initial_data.sort_order);
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
            location.set(item.location);
            url.set(item.url);
            starred.set(item.starred);
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
            "location": location.get_untracked(),
            "url": url.get_untracked(),
            "starred": starred.get_untracked(),
            "sortOrder": sort_order.get_untracked(),
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
            let (url_req, method, if_match) = match id {
                Some(ref id) if !is_new => (
                    format!("{ITEMS_BASE}/{id}"),
                    "PUT",
                    Some(etag(id, edit_revision)),
                ),
                _ => (ITEMS_BASE.to_owned(), "POST", None),
            };
            match send::<TaskItem>(&url_req, method, Some(body), &token, if_match).await {
                Ok(row) => {
                    busy.try_set(false);
                    on_save.try_run(row);
                }
                Err(cause) => {
                    error.try_set(cause.message);
                    busy.try_set(false);
                }
            }
        });
    };

    view! {
        <EditorPane title=if is_new { "New task" } else { "Edit task" } on_close=on_close>
            <form class="task-editor" on:submit=submit>
                <label class="event-title-field">
                    <span class="sr-only">"Title"</span>
                    <input
                        name="title"
                        aria-label="Title"
                        required
                        autofocus
                        maxlength="1000"
                        placeholder="Task title"
                        prop:value=move || title.get()
                        on:input=move |e| title.set(event_target_value(&e))
                    />
                </label>
                {is_new.then(|| view! {
                    <label>"List"
                        <ReactiveSelect aria_label="List" name="collectionId" required=true options=writable_options value=collection_id/>
                    </label>
                })}
                <label>"Description"
                    <textarea
                        name="description"
                        rows="3"
                        placeholder="Add details"
                        prop:value=move || description.get()
                        on:input=move |e| description.set(event_target_value(&e))
                    ></textarea>
                </label>
                <section class="form-section">
                    <h3><Icon name="calendar"/>"Schedule"</h3>
                    <label class="workspace-check-label">
                        <input
                            type="checkbox"
                            prop:checked=move || all_day.get()
                            on:change=move |e| {
                                let next = event_target_checked(&e);
                                all_day.set(next);
                                if next {
                                    start.update(|s| *s = date_slice(s, true));
                                    due.update(|d| *d = date_slice(d, true));
                                }
                            }
                        />
                        "Date only"
                    </label>
                    {move || if all_day.get() {
                        view! {
                            <div class="workspace-form-grid">
                                <label>"Starts"
                                    <input
                                        name="start"
                                        type="date"
                                        prop:value=move || start.get()
                                        on:input=move |e| start.set(event_target_value(&e))
                                        on:change=move |e| start.set(event_target_value(&e))
                                    />
                                </label>
                                <label>"Due"
                                    <input
                                        name="due"
                                        type="date"
                                        prop:value=move || due.get()
                                        on:input=move |e| due.set(event_target_value(&e))
                                        on:change=move |e| due.set(event_target_value(&e))
                                    />
                                </label>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="workspace-form-grid">
                                <label>"Starts"
                                    <input
                                        name="start"
                                        type="datetime-local"
                                        prop:value=move || start.get()
                                        on:input=move |e| start.set(event_target_value(&e))
                                        on:change=move |e| start.set(event_target_value(&e))
                                    />
                                </label>
                                <label>"Due"
                                    <input
                                        name="due"
                                        type="datetime-local"
                                        prop:value=move || due.get()
                                        on:input=move |e| due.set(event_target_value(&e))
                                        on:change=move |e| due.set(event_target_value(&e))
                                    />
                                </label>
                            </div>
                        }.into_any()
                    }}
                    {move || (!all_day.get()).then(|| view! {
                        <label>"Timezone"
                            <input
                                name="timezone"
                                prop:value=move || timezone.get()
                                on:input=move |e| timezone.set(event_target_value(&e))
                            />
                        </label>
                    })}
                    <RepeatField value=recurrence/>
                    {show_occurrences.then(|| edit_id.clone().map(|id| view! {
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
                            <input
                                name="progress"
                                type="number"
                                min="0"
                                max="100"
                                prop:value=move || progress.get()
                                on:input=move |e| progress.set(event_target_value(&e))
                                on:change=move |e| progress.set(event_target_value(&e))
                            />
                        </label>
                    </div>
                    <PriorityField value=priority/>
                </section>
                <label>"Categories"
                    <input
                        name="categories"
                        placeholder="Comma-separated"
                        prop:value=move || categories.get()
                        on:input=move |e| categories.set(event_target_value(&e))
                    />
                </label>
                <details class="form-more">
                    <summary>"Advanced task fields"</summary>
                    <label>"Location"
                        <input
                            name="location"
                            placeholder="Room, address, or link"
                            prop:value=move || location.get()
                            on:input=move |e| location.set(event_target_value(&e))
                        />
                    </label>
                    <label>"URL"
                        <input
                            name="url"
                            type="url"
                            placeholder="https://example.com"
                            prop:value=move || url.get()
                            on:input=move |e| url.set(event_target_value(&e))
                        />
                    </label>
                    <label>"Parent task identifier"
                        <input
                            name="parentUid"
                            prop:value=move || parent_uid.get()
                            on:input=move |e| parent_uid.set(event_target_value(&e))
                        />
                        <small>"Use Add subtask in a task\u{2019}s details for an ordinary parent relationship."</small>
                    </label>
                </details>
                {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                {move || (is_new && collections.with(Vec::is_empty)).then(|| view! { <p>"Create a task list before adding tasks."</p> })}
                <div class="dialog-actions">
                    <button
                        class="text-button"
                        type="button"
                        disabled=move || busy.get()
                        on:click=move |_| { on_close.try_run(()); }
                    >"Cancel"</button>
                    <button
                        class="primary-button"
                        disabled=move || busy.get() || (is_new && !has_writable.get())
                    >"Save task"</button>
                </div>
            </form>
        </EditorPane>
    }
}

/// Direct-manipulation TaskInspector inside `<aside aria-label="Task details">`.
#[component]
pub fn TaskInspector(
    row: TaskItem,
    collection: Option<Collection>,
    all_items: Signal<Vec<TaskItem>>,
    today: Memo<String>,
    busy: Signal<bool>,
    on_close: Callback<()>,
    on_edit: Callback<()>,
    on_complete: Callback<()>,
    on_toggle_star: Callback<()>,
    on_patch: Callback<(TaskItem, TaskData)>,
    on_subtask_modal: Callback<()>,
    on_quick_subtask: Callback<(TaskItem, String)>,
    on_toggle_child: Callback<TaskItem>,
    on_delete: Callback<()>,
    on_move: Callback<bool>,
    on_open: Callback<String>,
) -> impl IntoView {
    let task = row.data.clone();
    let done = row.is_done();
    let writable = row.role != "read";
    let is_starred = row.is_starred();
    let today_now = today.get_untracked();
    let overdue = row.is_overdue(&today_now);

    let row_sv = StoredValue::new(row.clone());
    let row_id = row.id.clone();

    let editing_title = RwSignal::new(false);
    let title_draft = RwSignal::new(task.title.clone());
    let desc_draft = RwSignal::new(task.description.clone());
    let progress_draft = RwSignal::new(task.progress);
    let loc_draft = RwSignal::new(task.location.clone());
    let url_draft = RwSignal::new(task.url.clone());
    let new_sub_title = RwSignal::new(String::new());
    let adding_tag = RwSignal::new(false);
    let new_tag = RwSignal::new(String::new());

    let collection_color = collection
        .as_ref()
        .map(|c| c.color.clone())
        .unwrap_or_default();
    let collection_name = collection
        .as_ref()
        .map(|c| c.name.clone())
        .or_else(|| row.collection_name.clone())
        .unwrap_or_default();

    let commit_title = move || {
        editing_title.set(false);
        let next_title = title_draft.get_untracked().trim().to_owned();
        let cur = row_sv.get_value();
        if !next_title.is_empty() && next_title != cur.data.title {
            let mut next = cur.data.clone();
            next.title = next_title;
            on_patch.try_run((cur, next));
        } else {
            title_draft.set(cur.data.title);
        }
    };

    let commit_desc = move || {
        let next_desc = desc_draft.get_untracked();
        let cur = row_sv.get_value();
        if next_desc != cur.data.description {
            let mut next = cur.data.clone();
            next.description = next_desc;
            on_patch.try_run((cur, next));
        }
    };

    let commit_loc = move || {
        let next_loc = loc_draft.get_untracked().trim().to_owned();
        let cur = row_sv.get_value();
        if next_loc != cur.data.location {
            let mut next = cur.data.clone();
            next.location = next_loc;
            on_patch.try_run((cur, next));
        }
    };

    let commit_url = move || {
        let next_url = url_draft.get_untracked().trim().to_owned();
        let cur = row_sv.get_value();
        if next_url != cur.data.url {
            let mut next = cur.data.clone();
            next.url = next_url;
            on_patch.try_run((cur, next));
        }
    };

    let set_due_quick = move |new_due: String| {
        let cur = row_sv.get_value();
        let mut next = cur.data.clone();
        next.due = new_due.clone();
        if !new_due.is_empty() {
            next.all_day = true;
            next.timezone = "floating".into();
        }
        on_patch.try_run((cur, next));
    };

    let set_priority_quick = move |pri: i32| {
        let cur = row_sv.get_value();
        if cur.data.priority == pri {
            return;
        }
        let mut next = cur.data.clone();
        next.priority = pri;
        on_patch.try_run((cur, next));
    };

    let set_progress_quick = move |pct: i32| {
        let cur = row_sv.get_value();
        let clamped = pct.clamp(0, 100);
        if cur.data.progress == clamped {
            return;
        }
        let mut next = cur.data.clone();
        next.progress = clamped;
        if clamped == 100 {
            next.status = "COMPLETED".into();
        } else if clamped > 0 && next.status != "IN-PROCESS" {
            next.status = "IN-PROCESS".into();
        } else if clamped == 0 && next.status == "COMPLETED" {
            next.status = "NEEDS-ACTION".into();
        }
        on_patch.try_run((cur, next));
    };

    let remove_tag = move |tg: String| {
        let cur = row_sv.get_value();
        let mut next = cur.data.clone();
        next.categories.retain(|c| c != &tg);
        on_patch.try_run((cur, next));
    };

    let add_tag = move || {
        let val = new_tag
            .get_untracked()
            .trim()
            .trim_start_matches('#')
            .to_owned();
        adding_tag.set(false);
        new_tag.set(String::new());
        if val.is_empty() {
            return;
        }
        let cur = row_sv.get_value();
        if cur
            .data
            .categories
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&val))
        {
            return;
        }
        let mut next = cur.data.clone();
        next.categories.push(val);
        on_patch.try_run((cur, next));
    };

    let reminder_text = task.reminder_minutes.map(|mins| {
        if mins == 0 {
            "At due/start time".to_owned()
        } else if mins < 60 {
            format!("{mins} minutes before")
        } else if mins % 1440 == 0 {
            let days = mins / 1440;
            if days == 1 {
                "1 day before".to_owned()
            } else {
                format!("{days} days before")
            }
        } else if mins % 60 == 0 {
            let hrs = mins / 60;
            if hrs == 1 {
                "1 hour before".to_owned()
            } else {
                format!("{hrs} hours before")
            }
        } else {
            format!("{mins} minutes before")
        }
    });

    let children_full = Memo::new(move |_| {
        let cur = row_sv.get_value();
        let all = all_items.get();
        let mut list: Vec<TaskItem> = all
            .into_iter()
            .filter(|i| {
                !cur.uid.is_empty()
                    && i.collection_id == cur.collection_id
                    && i.data.parent_uid == cur.uid
            })
            .collect();
        if list.is_empty() && !cur.children_slice().is_empty() {
            // Fallback to server-returned children summaries if not yet in `all_items`.
            return cur
                .children_slice()
                .iter()
                .map(|c| {
                    let mut d = TaskData::default();
                    d.title = c.title.clone();
                    d.status = c.status.clone();
                    TaskItem {
                        id: c.id.clone(),
                        collection_id: cur.collection_id.clone(),
                        uid: c.uid.clone(),
                        revision: 0,
                        modified_at: String::new(),
                        data: d,
                        role: cur.role.clone(),
                        collection_name: cur.collection_name.clone(),
                        overdue: None,
                        next_occurrence: None,
                        editing_occurrence_id: None,
                        parent_id: Some(cur.id.clone()),
                        children: None,
                    }
                })
                .collect();
        }
        list.sort_by(|a, b| {
            a.is_done()
                .cmp(&b.is_done())
                .then_with(|| a.data.sort_order.cmp(&b.data.sort_order))
                .then_with(|| a.data.title.cmp(&b.data.title))
        });
        list
    });

    let due_day: String = task.due.chars().take(10).collect();
    let t_today = today.get_untracked();
    let t_tomorrow = add_days(&t_today, 1);
    let t_next_week = add_days(&t_today, 7);

    let pri_group = if task.priority == 0 {
        0
    } else if task.priority < 5 {
        1
    } else if task.priority == 5 {
        5
    } else {
        9
    };

    view! {
        <section class="workspace-task-details tasks-inspector">
            <div class="pim-detail-top">
                <span class="tasks-inspector-state">
                    {move || if busy.get() { "Saving\u{2026}" } else { "Task" }}
                </span>
                {writable.then(|| view! {
                    <button
                        type="button"
                        class=if is_starred { "icon-button tasks-star-btn is-starred" } else { "icon-button tasks-star-btn" }
                        aria-pressed=is_starred.to_string()
                        aria-label=if is_starred { "Unstar task" } else { "Star task" }
                        title=if is_starred { "Unstar (S)" } else { "Star (S)" }
                        on:click=move |_| { on_toggle_star.try_run(()); }
                    >
                        <Icon name="star"/>
                    </button>
                    <button
                        type="button"
                        class="icon-button"
                        aria-label="Edit task"
                        title="Edit all fields (E)"
                        on:click=move |_| { on_edit.try_run(()); }
                    >
                        <Icon name="pencil"/>
                    </button>
                })}
                <ActionMenu
                    label="Task options"
                    actions=move |_| {
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
                    }
                >
                    <Icon name="more"/>
                </ActionMenu>
                <button
                    type="button"
                    class="icon-button"
                    aria-label="Close task details"
                    title="Close (Esc)"
                    on:click=move |_| { on_close.try_run(()); }
                >
                    <Icon name="close"/>
                </button>
            </div>

            <div class="task-card-title tasks-inspector-hero">
                <button
                    type="button"
                    class=format!("task-check is-large priority-{}", priority_class(task.priority))
                    aria-pressed=done.to_string()
                    aria-label=format!("{} {}", if done { "Reopen" } else { "Complete" }, if task.title.is_empty() { "task" } else { &task.title })
                    disabled=move || busy.get() || !writable
                    on:click=move |_| { on_complete.try_run(()); }
                >
                    {done.then(|| view! { <Icon name="check"/> })}
                </button>
                <div class="tasks-inspector-title-wrap">
                    {move || if editing_title.get() && writable {
                        view! {
                            <input
                                type="text"
                                class="tasks-inline-title-input"
                                aria-label="Edit task title"
                                autofocus
                                maxlength="1000"
                                prop:value=move || title_draft.get()
                                on:input=move |e| title_draft.set(event_target_value(&e))
                                on:blur=move |_| commit_title()
                                on:keydown=move |e: web_sys::KeyboardEvent| {
                                    if e.key() == "Enter" {
                                        e.prevent_default();
                                        commit_title();
                                    } else if e.key() == "Escape" {
                                        e.stop_propagation();
                                        title_draft.set(row_sv.with_value(|r| r.data.title.clone()));
                                        editing_title.set(false);
                                    }
                                }
                            />
                        }.into_any()
                    } else {
                        let title_text = if task.title.is_empty() { "Untitled task".to_owned() } else { task.title.clone() };
                        view! {
                            <h2
                                class=if done { "is-done" } else { "" }
                                title=if writable { "Click to rename" } else { "" }
                                on:click=move |_| {
                                    if writable {
                                        editing_title.set(true);
                                    }
                                }
                            >
                                {title_text}
                            </h2>
                        }.into_any()
                    }}
                    {if writable {
                        view! {
                            <textarea
                                class="tasks-inline-desc"
                                rows="2"
                                aria-label="Task description"
                                placeholder="Add description\u{2026}"
                                prop:value=move || desc_draft.get()
                                on:input=move |e| desc_draft.set(event_target_value(&e))
                                on:blur=move |_| commit_desc()
                            ></textarea>
                        }.into_any()
                    } else if !task.description.is_empty() {
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
                    <button
                        type="button"
                        class=if done { "secondary-button" } else { "primary-button" }
                        disabled=move || busy.get()
                        on:click=move |_| { on_complete.try_run(()); }
                    >
                        <Icon name=if done { "undo" } else { "check" }/>
                        {if done { "Reopen task" } else { "Complete task" }}
                    </button>
                    <button
                        type="button"
                        class="secondary-button"
                        disabled=move || busy.get()
                        on:click=move |_| { on_subtask_modal.try_run(()); }
                    >
                        <Icon name="subtask"/>"Add subtask"
                    </button>
                </div>
            })}

            {writable.then(|| {
                let d_today = t_today.clone();
                let d_tom = t_tomorrow.clone();
                let d_week = t_next_week.clone();
                view! {
                    <div class="tasks-quick-schedule" role="group" aria-label="Quick schedule">
                        <button
                            type="button"
                            class=if due_day == t_today { "tasks-sched-pill is-active" } else { "tasks-sched-pill" }
                            aria-label="Set due date to Today"
                            disabled=move || busy.get()
                            on:click=move |_| set_due_quick(d_today.clone())
                        >"Today"</button>
                        <button
                            type="button"
                            class=if due_day == t_tomorrow { "tasks-sched-pill is-active" } else { "tasks-sched-pill" }
                            aria-label="Set due date to Tomorrow"
                            disabled=move || busy.get()
                            on:click=move |_| set_due_quick(d_tom.clone())
                        >"Tomorrow"</button>
                        <button
                            type="button"
                            class=if due_day == t_next_week { "tasks-sched-pill is-active" } else { "tasks-sched-pill" }
                            aria-label="Set due date to Next week"
                            disabled=move || busy.get()
                            on:click=move |_| set_due_quick(d_week.clone())
                        >"Next week"</button>
                        {(!due_day.is_empty()).then(|| view! {
                            <button
                                type="button"
                                class="tasks-sched-pill"
                                aria-label="Clear due date"
                                disabled=move || busy.get()
                                on:click=move |_| set_due_quick(String::new())
                            >"Clear"</button>
                        })}
                    </div>
                }
            })}

            {row.parent_id.clone().map(|pid| view! {
                <div class="property-card">
                    <button type="button" class="property property-action" on:click=move |_| { on_open.try_run(pid.clone()); }>
                        <Icon name="back"/>
                        <div><span>"Open parent task"</span></div>
                    </button>
                </div>
            })}

            <div class="property-card tasks-subtasks-card">
                <h3>{move || {
                    let kids = children_full.get();
                    if kids.is_empty() {
                        "Subtasks".to_owned()
                    } else {
                        let fin = kids.iter().filter(|c| c.is_done()).count();
                        format!("Subtasks \u{00b7} {fin} of {}", kids.len())
                    }
                }}</h3>
                {move || children_full.get().into_iter().map(|child| {
                    let cid = child.id.clone();
                    let cdone = child.is_done();
                    let c_title = if child.data.title.is_empty() { "Untitled subtask".to_owned() } else { child.data.title.clone() };
                    let c_ro = child.role == "read" || child.revision == 0;
                    let c_toggle = child.clone();
                    view! {
                        <div class=if cdone { "property tasks-inspector-sub is-done" } else { "property tasks-inspector-sub" }>
                            <button
                                type="button"
                                class="task-check is-sub"
                                aria-pressed=cdone.to_string()
                                aria-label=format!("{} {c_title}", if cdone { "Reopen" } else { "Complete" })
                                disabled=move || busy.get() || c_ro
                                on:click=move |_| { on_toggle_child.try_run(c_toggle.clone()); }
                            >
                                {cdone.then(|| view! { <Icon name="check"/> })}
                            </button>
                            <button type="button" class="tasks-inspector-sub-open" on:click=move |_| { on_open.try_run(cid.clone()); }>
                                <span>{c_title}</span>
                                <span class="sr-only">{if cdone { "Completed" } else { "Open" }}</span>
                            </button>
                        </div>
                    }
                }).collect_view()}
                {writable.then(|| view! {
                    <form class="tasks-inspector-sub-add" on:submit=move |e: web_sys::SubmitEvent| {
                        e.prevent_default();
                        let val = new_sub_title.get_untracked().trim().to_owned();
                        if !val.is_empty() {
                            new_sub_title.set(String::new());
                            on_quick_subtask.try_run((row_sv.get_value(), val));
                        }
                    }>
                        <Icon name="plus"/>
                        <input
                            type="text"
                            maxlength="500"
                            placeholder="Add subtask\u{2026}"
                            aria-label="Quick add subtask"
                            disabled=move || busy.get()
                            prop:value=move || new_sub_title.get()
                            on:input=move |e| new_sub_title.set(event_target_value(&e))
                        />
                    </form>
                })}
            </div>

            <div class="property-card tasks-props-card">
                <div class="property">
                    <Icon name="calendar"/>
                    <span class="property-key">"Due"</span>
                    <span class=format!("property-value{}", if overdue { " is-overdue" } else { "" })>
                        <span>{if !task.due.is_empty() { task_date(&task.due, task.all_day) } else { "No due date".to_owned() }}</span>
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
                        {if writable {
                            view! {
                                <div class="tasks-pri-pills" role="group" aria-label="Priority">
                                    {[(0, "None"), (9, "Low"), (5, "Normal"), (1, "High")].into_iter().map(|(val, lbl)| {
                                        let active = pri_group == val;
                                        let cls = match (active, val) {
                                            (true, 1) => "tasks-pri-pill is-active is-high",
                                            (true, 5) => "tasks-pri-pill is-active is-medium",
                                            (true, _) => "tasks-pri-pill is-active",
                                            _ => "tasks-pri-pill",
                                        };
                                        view! {
                                            <button
                                                type="button"
                                                class=cls
                                                aria-pressed=active.to_string()
                                                disabled=move || busy.get()
                                                on:click=move |_| set_priority_quick(val)
                                            >{lbl}</button>
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_any()
                        } else {
                            view! { <span>{super::model::task_priority_label(task.priority)}</span> }.into_any()
                        }}
                    </span>
                </div>
                <div class="property">
                    <Icon name="progress"/>
                    <span class="property-key">"Status"</span>
                    <div class="property-value tasks-status-progress">
                        <div class="tasks-status-line">
                            <span>{task_status_label(&task.status)}</span>
                            <span class="tasks-progress-pct">{move || format!("{}%", progress_draft.get())}</span>
                        </div>
                        {if writable {
                            view! {
                                <input
                                    type="range"
                                    class="tasks-progress-slider"
                                    min="0"
                                    max="100"
                                    step="10"
                                    aria-label="Progress"
                                    disabled=move || busy.get()
                                    prop:value=move || progress_draft.get().to_string()
                                    on:input=move |e| {
                                        if let Ok(v) = event_target_value(&e).parse::<i32>() {
                                            progress_draft.set(v);
                                        }
                                    }
                                    on:change=move |e| {
                                        if let Ok(v) = event_target_value(&e).parse::<i32>() {
                                            progress_draft.set(v);
                                            set_progress_quick(v);
                                        }
                                    }
                                />
                            }.into_any()
                        } else {
                            view! { <progress max="100" value=task.progress.to_string() aria-label="Progress"></progress> }.into_any()
                        }}
                    </div>
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
                {reminder_text.map(|r| view! {
                    <div class="property">
                        <Icon name="bell"/>
                        <span class="property-key">"Reminder"</span>
                        <span class="property-value">{r}</span>
                    </div>
                })}
                <div class="property">
                    <Icon name="list"/>
                    <span class="property-key">"List"</span>
                    <span class="property-value tasks-list-val">
                        <CollectionDot color=collection_color/>
                        <span>{collection_name}</span>
                    </span>
                </div>
                {(writable || !task.location.is_empty()).then(|| view! {
                    <div class="property">
                        <Icon name="location"/>
                        <span class="property-key">"Location"</span>
                        <span class="property-value">
                            {if writable {
                                view! {
                                    <input
                                        type="text"
                                        class="tasks-inline-field"
                                        aria-label="Location"
                                        placeholder="Add location"
                                        prop:value=move || loc_draft.get()
                                        on:input=move |e| loc_draft.set(event_target_value(&e))
                                        on:blur=move |_| commit_loc()
                                        on:keydown=move |e: web_sys::KeyboardEvent| {
                                            if e.key() == "Enter" {
                                                e.prevent_default();
                                                commit_loc();
                                            }
                                        }
                                    />
                                }.into_any()
                            } else {
                                view! { <span>{task.location.clone()}</span> }.into_any()
                            }}
                        </span>
                    </div>
                })}
                {(writable || !task.url.is_empty()).then(|| {
                    let safe_href = visible_url(&task.url);
                    view! {
                        <div class="property">
                            <Icon name="link"/>
                            <span class="property-key">"URL"</span>
                            <span class="property-value tasks-url-val">
                                {if writable {
                                    view! {
                                        <input
                                            type="url"
                                            class="tasks-inline-field"
                                            aria-label="URL"
                                            placeholder="https://\u{2026}"
                                            prop:value=move || url_draft.get()
                                            on:input=move |e| url_draft.set(event_target_value(&e))
                                            on:blur=move |_| commit_url()
                                            on:keydown=move |e: web_sys::KeyboardEvent| {
                                                if e.key() == "Enter" {
                                                    e.prevent_default();
                                                    commit_url();
                                                }
                                            }
                                        />
                                    }.into_any()
                                } else {
                                    view! { <span>{task.url.clone()}</span> }.into_any()
                                }}
                                {safe_href.map(|href| view! {
                                    <a class="icon-button tasks-url-open" href=href target="_blank" rel="noopener noreferrer" aria-label="Open link" title="Open link">
                                        <Icon name="external"/>
                                    </a>
                                })}
                            </span>
                        </div>
                    }
                })}
                <div class="property">
                    <Icon name="tag"/>
                    <span class="property-key">"Tags"</span>
                    <span class="property-value tasks-tags-val">
                        {task.categories.iter().cloned().map(|tag| {
                            let t_rm = tag.clone();
                            view! {
                                <span class="tasks-inspector-tag">
                                    <span>{format!("#{tag}")}</span>
                                    {writable.then(|| view! {
                                        <button
                                            type="button"
                                            class="tasks-tag-rm"
                                            aria-label=format!("Remove tag {tag}")
                                            disabled=move || busy.get()
                                            on:click=move |_| remove_tag(t_rm.clone())
                                        >
                                            <Icon name="close"/>
                                        </button>
                                    })}
                                </span>
                            }
                        }).collect_view()}
                        {writable.then(|| view! {
                            {move || if adding_tag.get() {
                                view! {
                                    <input
                                        type="text"
                                        class="tasks-tag-input"
                                        aria-label="New tag name"
                                        placeholder="tag"
                                        autofocus
                                        maxlength="60"
                                        prop:value=move || new_tag.get()
                                        on:input=move |e| new_tag.set(event_target_value(&e))
                                        on:blur=move |_| add_tag()
                                        on:keydown=move |e: web_sys::KeyboardEvent| {
                                            if e.key() == "Enter" {
                                                e.prevent_default();
                                                add_tag();
                                            } else if e.key() == "Escape" {
                                                e.stop_propagation();
                                                adding_tag.set(false);
                                                new_tag.set(String::new());
                                            }
                                        }
                                    />
                                }.into_any()
                            } else {
                                view! {
                                    <button
                                        type="button"
                                        class="tasks-tag-add"
                                        disabled=move || busy.get()
                                        on:click=move |_| adding_tag.set(true)
                                    >"+ Tag"</button>
                                }.into_any()
                            }}
                        })}
                    </span>
                </div>
            </div>
        </section>
    }
}
