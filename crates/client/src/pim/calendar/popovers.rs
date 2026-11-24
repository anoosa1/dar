//! Floating popovers and dialogs:
//! - `EventPeekPopover`: Zero-layout-shift event inspection card with clickable location links,
//!   duration/timezone cues, Duplicate, Edit, Delete, and ICS Export.
//! - `QuickCreatePopover`: Instant click-on-grid creation with duration chips, calendar picker,
//!   and seamless handoff to "More options" (`EventEditor`).
//! - `ShortcutsDialog`: Keyboard shortcuts cheat sheet (`?`).

use leptos::prelude::*;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use super::model::{
    BASE, QuickCreate, at, day, duration_short, event_when, format_hhmm, local, reminder_text, s,
    split_location_url,
};
use crate::date_popover::add_days;
use crate::icons::Icon;
use crate::pim::presentation::recurrence_label;
use crate::pim::{Collection, CollectionDot, Done, Item};
use crate::ui::{Dialog, Select};

#[component]
pub fn EventPeekPopover(
    event: Value,
    detail: Option<Item>,
    calendar: Option<Collection>,
    zone: String,
    time_format: String,
    can_edit: bool,
    on_close: Callback<()>,
    on_edit: Callback<()>,
    on_duplicate: Callback<()>,
    on_delete: Callback<()>,
) -> impl IntoView {
    let title = s(&event, "title");
    let color = calendar
        .as_ref()
        .map(|c| c.color.clone())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "#73396f".into());
    let cal_name = calendar
        .as_ref()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let all_day = event["allDay"] == json!(true);
    let when_date = event_when(&event, &zone);
    let start_ms = at(&event, "startAt");
    let end_ms = at(&event, "endAt");
    let time_line = if all_day {
        "All day".to_owned()
    } else {
        format!(
            "{} – {} ({})",
            format_hhmm(&local(start_ms, &zone)[11..16], &time_format),
            format_hhmm(&local(end_ms, &zone)[11..16], &time_format),
            duration_short(start_ms, end_ms)
        )
    };
    let ev_tz = s(&event, "timezone");
    let tz_note = (!all_day && !ev_tz.is_empty() && ev_tz != zone && ev_tz != "floating")
        .then(|| format!("Originally scheduled in {ev_tz}"));

    let status = s(&event, "status");
    let location = s(&event, "location");
    let description = s(&event, "description");
    let recurrence = detail
        .as_ref()
        .map(|d| s(&d.data, "recurrence"))
        .unwrap_or_else(|| s(&event, "recurrence"));
    let recurring = event["recurring"] == json!(true) || !recurrence.is_empty();
    let repeat_label = recurring.then(|| recurrence_label(&recurrence, recurring));
    let reminder_mins = detail
        .as_ref()
        .and_then(|d| d.data["reminderMinutes"].as_i64())
        .or_else(|| event["reminderMinutes"].as_i64());
    let attendees: Vec<String> = detail
        .as_ref()
        .and_then(|d| d.data["attendees"].as_array())
        .or_else(|| event["attendees"].as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let export_href = format!(
        "{BASE}/{}/items/{}/export",
        s(&event, "collectionId"),
        s(&event, "href")
    );

    view! {
        <div
            class="calendar-peek-backdrop"
            on:click=move |e| {
                let outside = e
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .is_some_and(|el| el.class_list().contains("calendar-peek-backdrop"));
                if outside {
                    on_close.run(());
                }
            }
        >
            <aside class="calendar-peek-card" aria-label="Event details">
                <div class="calendar-peek-bar">
                    <span class="calendar-peek-cal">
                        <CollectionDot color=color.clone()/>
                        <span>{cal_name}</span>
                    </span>
                    <div class="calendar-peek-actions">
                        {can_edit.then(|| view! {
                            <button type="button" class="icon-button" aria-label="Edit event" title="Edit event" on:click=move |_| on_edit.run(())>
                                <Icon name="pencil"/>
                            </button>
                            <button type="button" class="icon-button" aria-label="Duplicate event" title="Duplicate event" on:click=move |_| on_duplicate.run(())>
                                <Icon name="copy"/>
                            </button>
                            <button type="button" class="icon-button danger" aria-label="Delete event" title="Delete event" on:click=move |_| on_delete.run(())>
                                <Icon name="trash"/>
                            </button>
                        })}
                        <button type="button" class="icon-button" aria-label="Close event details" title="Close" on:click=move |_| on_close.run(())>
                            <Icon name="close"/>
                        </button>
                    </div>
                </div>

                <div class="calendar-peek-header">
                    <svg class="calendar-peek-swatch" viewBox="0 0 18 18" aria-hidden="true">
                        <rect x="2" y="2" width="14" height="14" rx="5" fill=color></rect>
                    </svg>
                    <div class="calendar-peek-title">
                        <h2>{title}</h2>
                        <p class="calendar-peek-when">{when_date}</p>
                        <p class="calendar-peek-time">{time_line}</p>
                        {tz_note.map(|n| view! { <p class="calendar-peek-tz">{n}</p> })}
                        {(status == "TENTATIVE").then(|| view! { <span class="status-chip warning">"Tentative"</span> })}
                        {(status == "CANCELLED").then(|| view! { <span class="status-chip danger">"Cancelled"</span> })}
                    </div>
                </div>

                <div class="calendar-peek-rows">
                    {(!location.is_empty()).then(|| {
                        let parsed = split_location_url(&location);
                        view! {
                            <div class="calendar-peek-row">
                                <Icon name="location"/>
                                <div class="calendar-peek-val">
                                    {match parsed {
                                        Some((prefix, url, suffix)) => {
                                            let href_url = url.clone();
                                            view! {
                                                <span>
                                                    {(!prefix.is_empty()).then(|| view! { <span>{format!("{prefix} · ")}</span> })}
                                                    <a class="calendar-peek-link" href=href_url target="_blank" rel="noopener noreferrer">
                                                        {url}
                                                    </a>
                                                    {(!suffix.is_empty()).then(|| view! { <span>{format!(" {suffix}")}</span> })}
                                                </span>
                                            }.into_any()
                                        }
                                        None => view! { <span>{location}</span> }.into_any(),
                                    }}
                                </div>
                            </div>
                        }
                    })}

                    {repeat_label.map(|label| view! {
                        <div class="calendar-peek-row">
                            <Icon name="repeat"/>
                            <div class="calendar-peek-val">
                                <span>{label}</span>
                                {(!recurrence.is_empty()).then(|| view! {
                                    <details class="calendar-peek-rrule">
                                        <summary>"Rule"</summary>
                                        <code>{recurrence}</code>
                                    </details>
                                })}
                            </div>
                        </div>
                    })}

                    {reminder_mins.map(|mins| view! {
                        <div class="calendar-peek-row">
                            <Icon name="bell"/>
                            <div class="calendar-peek-val">
                                <span>{if mins == 0 { "At start".to_owned() } else { format!("{} before", reminder_text(mins)) }}</span>
                            </div>
                        </div>
                    })}

                    {(!attendees.is_empty()).then(|| view! {
                        <div class="calendar-peek-row">
                            <Icon name="contacts"/>
                            <div class="calendar-peek-val">
                                <div class="calendar-peek-attendees">
                                    {attendees.into_iter().map(|a| view! { <span class="calendar-attendee-chip">{a}</span> }).collect_view()}
                                </div>
                                <small class="muted">"Saved with event · invitations are not emailed"</small>
                            </div>
                        </div>
                    })}

                    {(!description.is_empty()).then(|| view! {
                        <div class="calendar-peek-row">
                            <Icon name="note"/>
                            <div class="calendar-peek-val calendar-peek-notes">{description}</div>
                        </div>
                    })}
                </div>

                <div class="calendar-peek-footer">
                    <a class="text-button" href=export_href>
                        <Icon name="download"/>
                        <span>"Export event"</span>
                    </a>
                    {can_edit.then(|| view! {
                        <button type="button" class="secondary-button" on:click=move |_| on_edit.run(())>
                            "Edit details"
                        </button>
                    })}
                </div>
            </aside>
        </div>
    }
}

#[component]
pub fn QuickCreatePopover(
    draft: QuickCreate,
    calendars: Vec<Collection>,
    default_calendar_id: String,
    zone: String,
    time_format: String,
    on_close: Callback<()>,
    on_save: Callback<(Value, String, Done)>,
    on_more_options: Callback<(Value, String)>,
) -> impl IntoView {
    let title = RwSignal::new(String::new());
    let location = RwSignal::new(String::new());
    let duration_mins = RwSignal::new(draft.duration_mins);
    let all_day = RwSignal::new(draft.all_day);
    let calendar_id = RwSignal::new(default_calendar_id);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);

    let date = StoredValue::new(draft.date.clone());
    let hour = draft.hour;
    let zone_sv = StoredValue::new(zone);
    let fmt_sv = StoredValue::new(time_format);

    let build_payload = move || -> Value {
        let d = date.get_value();
        let z = zone_sv.get_value();
        let is_all = all_day.get_untracked();
        let dur = duration_mins.get_untracked();
        let (start, end) = if is_all {
            (d.clone(), add_days(&d, 1))
        } else {
            let start_total = hour * 60;
            let end_total = start_total + dur;
            let end_days = (end_total / 1440) as i64;
            let end_rem = end_total % 1440;
            let end_date = if end_days > 0 {
                add_days(&d, end_days)
            } else {
                d.clone()
            };
            (
                format!("{d}T{hour:02}:00"),
                format!("{end_date}T{:02}:{:02}", end_rem / 60, end_rem % 60),
            )
        };
        json!({
            "title": title.get_untracked().trim(),
            "start": start,
            "end": end,
            "timezone": z,
            "allDay": is_all,
            "location": location.get_untracked().trim(),
            "description": "",
            "recurrence": "",
            "attendees": [],
            "reminderMinutes": 10,
            "status": "CONFIRMED",
        })
    };

    let summary = move || {
        let d = date.get_value();
        let pretty_day = day(
            &d,
            &[("weekday", "short"), ("month", "short"), ("day", "numeric")],
        );
        if all_day.get() {
            format!("{pretty_day} · All day")
        } else {
            let fmt = fmt_sv.get_value();
            let dur = duration_mins.get();
            let end_total = (hour * 60 + dur) % 1440;
            let s_str = format_hhmm(&format!("{hour:02}:00"), &fmt);
            let e_str = format_hhmm(
                &format!("{:02}:{:02}", end_total / 60, end_total % 60),
                &fmt,
            );
            format!("{pretty_day} · {s_str} – {e_str}")
        }
    };

    let cal_options: Vec<(String, String)> =
        calendars.into_iter().map(|c| (c.id, c.name)).collect();

    view! {
        <div
            class="calendar-peek-backdrop"
            on:click=move |e| {
                let outside = e
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .is_some_and(|el| el.class_list().contains("calendar-peek-backdrop"));
                if outside {
                    on_close.run(());
                }
            }
        >
            <div class="calendar-quick-card" role="dialog" aria-label="Quick create event">
                <form on:submit=move |e: web_sys::SubmitEvent| {
                    e.prevent_default();
                    if title.with_untracked(|t| t.trim().is_empty()) {
                        error.set("Add an event title.".into());
                        return;
                    }
                    busy.set(true);
                    error.set(String::new());
                    let payload = build_payload();
                    let cid = calendar_id.get_untracked();
                    on_save.run((payload, cid, Callback::new(move |res: Result<(), String>| {
                        busy.set(false);
                        match res {
                            Ok(()) => on_close.run(()),
                            Err(msg) => error.set(msg),
                        }
                    })));
                }>
                    <div class="calendar-quick-head">
                        <input
                            class="calendar-quick-title"
                            aria-label="Event title"
                            placeholder="Add title…"
                            autofocus
                            required
                            maxlength="1024"
                            prop:value=move || title.get()
                            on:input=move |e| title.set(event_target_value(&e))
                        />
                        <button type="button" class="icon-button" aria-label="Close quick create" on:click=move |_| on_close.run(())>
                            <Icon name="close"/>
                        </button>
                    </div>

                    <p class="calendar-quick-when">
                        <Icon name="clock"/>
                        <span>{summary}</span>
                    </p>

                    <div class="calendar-quick-chips" role="group" aria-label="Duration">
                        {[(30u32, "30m"), (60, "1h"), (90, "1h 30m"), (120, "2h")].into_iter().map(|(mins, lbl)| {
                            let active = move || !all_day.get() && duration_mins.get() == mins;
                            view! {
                                <button
                                    type="button"
                                    class=move || if active() { "calendar-chip on" } else { "calendar-chip" }
                                    aria-pressed=move || active().to_string()
                                    on:click=move |_| {
                                        all_day.set(false);
                                        duration_mins.set(mins);
                                    }
                                >{lbl}</button>
                            }
                        }).collect_view()}
                        <button
                            type="button"
                            class=move || if all_day.get() { "calendar-chip on" } else { "calendar-chip" }
                            aria-pressed=move || all_day.get().to_string()
                            on:click=move |_| all_day.update(|v| *v = !*v)
                        >"All day"</button>
                    </div>

                    <div class="calendar-quick-fields">
                        <label>
                            <span class="sr-only">"Calendar"</span>
                            <Select aria_label="Calendar" options=cal_options value=calendar_id/>
                        </label>
                        <label>
                            <span class="sr-only">"Location"</span>
                            <input
                                placeholder="Add location or meeting link"
                                maxlength="2048"
                                prop:value=move || location.get()
                                on:input=move |e| location.set(event_target_value(&e))
                            />
                        </label>
                    </div>

                    {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}

                    <div class="calendar-quick-actions">
                        <button
                            type="button"
                            class="text-button"
                            on:click=move |_| on_more_options.run((build_payload(), calendar_id.get_untracked()))
                        >
                            "More options"
                        </button>
                        <button type="submit" class="primary-button" disabled=move || busy.get()>
                            {move || if busy.get() { "Saving…" } else { "Save" }}
                        </button>
                    </div>
                </form>
            </div>
        </div>
    }
}

#[component]
pub fn ShortcutsDialog(on_close: Callback<()>) -> impl IntoView {
    let rows = [
        ("C", "Create a new event"),
        ("T", "Jump to today"),
        (
            "D / W / M / Y / A",
            "Switch to Day, Week, Month, Year, or Agenda",
        ),
        ("J / K", "Next or previous period"),
        ("?", "Toggle this keyboard shortcuts reference"),
        ("Escape", "Close open popover or dialog"),
    ];
    view! {
        <Dialog title="Keyboard shortcuts" on_close=on_close>
            <div class="calendar-shortcuts-list">
                {rows.into_iter().map(|(keys, desc)| view! {
                    <div class="calendar-shortcut-row">
                        <kbd>{keys}</kbd>
                        <span>{desc}</span>
                    </div>
                }).collect_view()}
            </div>
        </Dialog>
    }
}
