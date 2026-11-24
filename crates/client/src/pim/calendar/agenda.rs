//! Chronologically sorted Agenda / Schedule view with sticky day columns, duration tags,
//! status cues, repeat marks, and clickable task checkboxes.

use leptos::prelude::*;
use serde_json::{Value, json};

use super::model::{
    at, day, duration_short, format_hhmm, local, occurrence_key, on_day, s, sort_events,
};
use crate::icons::Icon;
use crate::pim::Collection;

#[component]
pub fn AgendaView(
    days: Memo<Vec<String>>,
    today: Memo<String>,
    events: RwSignal<Vec<Value>>,
    calendars: RwSignal<Vec<Collection>>,
    zone: Memo<String>,
    time_format: Signal<String>,
    selected: RwSignal<Option<Value>>,
    on_select: Callback<Value>,
    on_toggle_task: Callback<Value>,
) -> impl IntoView {
    view! {
        <div class="calendar-agenda" aria-label="Calendar agenda">
            {move || {
                let z = zone.get();
                let fmt = time_format.get();
                let today_now = today.get();
                let all_days = days.get();
                let cals = calendars.get();
                let list = events.get();
                let any = list.iter().any(|e| all_days.iter().any(|d| on_day(e, d, &z)));
                if !any {
                    return view! {
                        <div class="pim-empty">
                            <h2>"Clear schedule"</h2>
                            <p>"No events in this 30-day window."</p>
                        </div>
                    }
                    .into_any();
                }
                all_days.into_iter().filter_map(|date| {
                    let mut rows: Vec<Value> = list.iter().filter(|e| on_day(e, &date, &z)).cloned().collect();
                    if rows.is_empty() {
                        return None;
                    }
                    sort_events(&mut rows);
                    let is_today = date == today_now;
                    let wd = day(&date, &[("weekday", "short")]);
                    let num = date[8..].trim_start_matches('0').to_owned();
                    let full = day(&date, &[("weekday", "long"), ("month", "short"), ("day", "numeric")]);
                    let full_aria = full.clone();
                    Some(view! {
                        <section class="calendar-agenda-day" aria-label=full_aria>
                            <div class=if is_today { "calendar-agenda-date is-today" } else { "calendar-agenda-date" }>
                                <span class="calendar-agenda-wd">{wd}</span>
                                <b class="calendar-agenda-num">{num}</b>
                            </div>
                            <div class="calendar-agenda-list">
                                <h2 class="sr-only">{full}</h2>
                                {rows.into_iter().map(|ev| {
                                    let cal = cals.iter().find(|c| c.id == s(&ev, "collectionId")).cloned();
                                    let color = cal.as_ref().map(|c| c.color.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| "#73396f".into());
                                    let cal_name = cal.as_ref().map(|c| c.name.clone()).unwrap_or_default();
                                    let all_day = ev["allDay"] == json!(true);
                                    let is_task = ev["task"] == json!(true);
                                    let title = s(&ev, "title");
                                    let location = s(&ev, "location");
                                    let status = s(&ev, "status");
                                    let cancelled = status == "CANCELLED";
                                    let tentative = status == "TENTATIVE";
                                    let repeats = ev["recurring"] == json!(true) || !s(&ev, "recurrence").is_empty();
                                    let start_ms = at(&ev, "startAt");
                                    let end_ms = at(&ev, "endAt");
                                    let time_text = if all_day {
                                        "All day".to_owned()
                                    } else if is_task {
                                        format_hhmm(&local(start_ms, &z)[11..16], &fmt)
                                    } else {
                                        format!(
                                            "{} – {}",
                                            format_hhmm(&local(start_ms, &z)[11..16], &fmt),
                                            format_hhmm(&local(end_ms, &z)[11..16], &fmt)
                                        )
                                    };
                                    let dur = (!all_day && !is_task && end_ms > start_ms).then(|| duration_short(start_ms, end_ms));

                                    if is_task {
                                        let done = status == "COMPLETED";
                                        let ev_toggle = ev.clone();
                                        return view! {
                                            <div class=format!("calendar-event-wrap is-task calendar-agenda-row{}", if done { " is-done" } else { "" })>
                                                <button
                                                    type="button"
                                                    class=if done { "calendar-task-toggle is-done" } else { "calendar-task-toggle" }
                                                    aria-label=if done { format!("Reopen task {title}") } else { format!("Complete task {title}") }
                                                    on:click=move |_| on_toggle_task.run(ev_toggle.clone())
                                                >
                                                    <Icon name="check"/>
                                                </button>
                                                <a
                                                    class=format!("calendar-event calendar-task{}{}", if all_day { " all-day" } else { "" }, if done { " is-done" } else { "" })
                                                    href=format!("/tasks?item={}", js_sys::encode_uri_component(&s(&ev, "resourceId")))
                                                >
                                                    <svg class="calendar-event-edge" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                    <span class="calendar-event-time">{if all_day { "Due".to_owned() } else { time_text }}</span>
                                                    <div class="calendar-agenda-body">
                                                        <strong>{title}</strong>
                                                        <span class="calendar-agenda-meta">{"Task · "}{cal_name}</span>
                                                    </div>
                                                </a>
                                            </div>
                                        }.into_any();
                                    }

                                    let key = occurrence_key(&ev);
                                    let is_sel = Signal::derive(move || selected.with(|sel| sel.as_ref().is_some_and(|x| occurrence_key(x) == key)));
                                    let ev_click = ev.clone();
                                    let cls = format!(
                                        "calendar-event{}{}{}",
                                        if all_day { " all-day" } else { "" },
                                        if cancelled { " cancelled" } else { "" },
                                        if tentative { " tentative" } else { "" }
                                    );
                                    view! {
                                        <div class=move || format!("calendar-event-wrap calendar-agenda-row{}", if is_sel.get() { " is-selected" } else { "" })>
                                            <button
                                                type="button"
                                                class=cls
                                                aria-pressed=move || is_sel.get().to_string()
                                                on:click=move |_| on_select.run(ev_click.clone())
                                            >
                                                <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                <svg class="calendar-event-edge" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                <span class="calendar-event-time">{time_text}</span>
                                                <div class="calendar-agenda-body">
                                                    <span class="calendar-agenda-title-line">
                                                        <strong>{title}</strong>
                                                        {tentative.then(|| view! { <span class="status-chip warning">"Tentative"</span> })}
                                                        {cancelled.then(|| view! { <span class="status-chip danger">"Cancelled"</span> })}
                                                        {repeats.then(|| view! { <span class="calendar-event-repeat" aria-label="Repeats"><Icon name="repeat"/></span> })}
                                                    </span>
                                                    <span class="calendar-agenda-meta">
                                                        {(!location.is_empty()).then(|| view! { <span>{location}</span> })}
                                                        <span>{cal_name}</span>
                                                    </span>
                                                </div>
                                                {dur.map(|d| view! { <span class="calendar-agenda-dur">{d}</span> })}
                                            </button>
                                        </div>
                                    }.into_any()
                                }).collect_view()}
                            </div>
                        </section>
                    })
                }).collect_view().into_any()
            }}
        </div>
    }
}
