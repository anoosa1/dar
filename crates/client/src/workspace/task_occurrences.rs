//! Repeating-task occurrence picker and due-reminder inbox (`pim/task-occurrences.tsx`).

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ReactiveSelect, browser_timezone, search_params};
use crate::api::{self, BrowserSession, Req};
use crate::date_popover::add_days;
use crate::dates::local_date_time;
use crate::ui::Dialog;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskData {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub due: String,
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub progress: i32,
    #[serde(default)]
    pub completed_at: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub parent_uid: String,
    #[serde(default)]
    pub recurrence: String,
    #[serde(default)]
    pub recurring: bool,
    #[serde(default)]
    pub reminder_minutes: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOccurrence {
    pub recurrence_id: String,
    #[serde(flatten)]
    pub data: TaskData,
}

#[derive(Deserialize)]
struct MasterWrap {
    data: TaskData,
}

#[derive(Deserialize)]
struct OccurrencesWrap {
    items: Vec<TaskOccurrence>,
}

/// Occurrence date range and scope selector for repeating tasks (`TaskOccurrences`).
#[component]
pub fn TaskOccurrences(
    id: String,
    start: String,
    scope: RwSignal<String>,
    on_pick: Callback<(TaskData, Option<TaskData>, bool)>,
) -> impl IntoView {
    let today_iso = js_sys::Date::new_0()
        .to_iso_string()
        .as_string()
        .unwrap_or_else(|| "2026-01-01".into());
    let base_date: String = if start.len() >= 10 {
        start.chars().take(10).collect()
    } else {
        today_iso.chars().take(10).collect()
    };
    let from = RwSignal::new(base_date.clone());
    let to = RwSignal::new(add_days(&base_date, 90));
    let items: RwSignal<Vec<TaskOccurrence>> = RwSignal::new(Vec::new());
    let master: RwSignal<Option<TaskData>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let sequence = RwSignal::new(0u32);
    let id = StoredValue::new(id);

    Effect::new(move |_| {
        let (f, t) = (from.get(), to.get());
        if f.is_empty() || t.is_empty() {
            return;
        }
        let seq = sequence.get_untracked() + 1;
        sequence.set(seq);
        let task_id = id.get_value();
        let params = search_params(&[("from", f), ("to", t), ("timezone", browser_timezone())]);
        spawn_local(async move {
            let (record, occs) = futures::future::join(
                api::get::<MasterWrap>(&format!("/api/v1/tasks/items/{task_id}")),
                api::get::<OccurrencesWrap>(&format!(
                    "/api/v1/tasks/items/{task_id}/occurrences?{params}"
                )),
            )
            .await;
            if sequence.try_get_untracked() != Some(seq) {
                return;
            }
            match (record, occs) {
                (Ok(rec), Ok(list)) => {
                    master.set(Some(rec.data));
                    items.set(list.items);
                    error.set(String::new());
                }
                (Err(e), _) | (_, Err(e)) => error.set(e.message),
            }
        });
    });

    let options = Signal::derive(move || {
        let cur = scope.get();
        let list = items.get();
        let mut opts = vec![(String::new(), "Whole series".to_owned())];
        if !cur.is_empty() && !list.iter().any(|i| i.recurrence_id == cur) {
            opts.push((cur.clone(), format!("{cur} \u{00b7} Selected occurrence")));
        }
        for item in list {
            let label = if item.data.status == "COMPLETED" {
                "Completed"
            } else {
                "Occurrence"
            };
            opts.push((
                item.recurrence_id.clone(),
                format!("{} \u{00b7} {label}", item.recurrence_id),
            ));
        }
        opts
    });

    let on_select = Callback::new(move |next: String| {
        scope.set(next.clone());
        let master_data = master.get_untracked();
        let chosen = if !next.is_empty() {
            items.with_untracked(|list| {
                list.iter()
                    .find(|i| i.recurrence_id == next)
                    .map(|i| i.data.clone())
            })
        } else {
            master_data.clone()
        };
        if let Some(item) = chosen {
            {
                on_pick.try_run((item, master_data, !next.is_empty()));
            };
        }
    });

    view! {
        <fieldset class="task-occurrence-picker">
            <legend>"Repeating task"</legend>
            <div class="workspace-form-grid">
                <label>"Occurrences from"<input type="date" prop:value=move || from.get() on:change=move |e| from.set(event_target_value(&e))/></label>
                <label>"Occurrences until"<input type="date" prop:value=move || to.get() on:change=move |e| to.set(event_target_value(&e))/></label>
            </div>
            <label>"Edit scope"
                <ReactiveSelect aria_label="Edit scope" name="occurrenceId" options=options value=scope on_change=on_select/>
            </label>
            <p class="muted small">"Completing one occurrence leaves future occurrences active. Completing the whole series stops future reminders."</p>
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
        </fieldset>
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reminder {
    id: String,
    title: String,
    due_at: Value,
}

#[derive(Deserialize)]
struct ReminderList {
    items: Vec<Reminder>,
}

fn format_due_at(value: &Value) -> String {
    let ms = match value {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => js_sys::Date::parse(s),
        _ => f64::NAN,
    };
    if ms.is_nan() {
        value.as_str().unwrap_or_default().to_owned()
    } else {
        local_date_time(ms, &[])
    }
}

/// Due reminders modal dialog (`ReminderInbox`).
#[component]
pub fn ReminderInbox(session: BrowserSession, on_close: Callback<()>) -> impl IntoView {
    let items: RwSignal<Vec<Reminder>> = RwSignal::new(Vec::new());
    let error = RwSignal::new(String::new());
    let csrf = StoredValue::new(session.csrf_token.clone());

    let refresh = move || {
        spawn_local(async move {
            match api::get::<ReminderList>("/api/v1/pim/reminders").await {
                Ok(list) => {
                    items.set(list.items);
                    error.set(String::new());
                }
                Err(e) => error.set(e.message),
            }
        });
    };
    refresh();

    view! {
        <Dialog title="Reminders" on_close=on_close>
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            {move || items.with(Vec::is_empty).then(|| view! { <p>"No due reminders."</p> })}
            {move || items.get().into_iter().map(|item| {
                let id = item.id.clone();
                let due = format_due_at(&item.due_at);
                view! {
                    <div class="workspace-row">
                        <div>
                            <strong>{item.title}</strong>
                            <small>{due}</small>
                        </div>
                        <button class="text-button" on:click=move |_| {
                            let encoded = js_sys::encode_uri_component(&id).as_string().unwrap_or_default();
                            let token = csrf.get_value();
                            spawn_local(async move {
                                match api::request::<Value>(&format!("/api/v1/pim/reminders/{encoded}"), Req { method: "DELETE", csrf: Some(&token), ..Default::default() }).await {
                                    Ok(_) => refresh(),
                                    Err(e) => error.set(e.message),
                                }
                            });
                        }>"Dismiss"</button>
                    </div>
                }
            }).collect_view()}
        </Dialog>
    }
}
