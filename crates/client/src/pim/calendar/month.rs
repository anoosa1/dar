//! Month view: 6-week grid with overlaid continuous multi-day/all-day horizontal ribbons,
//! chronologically sorted per-day timed events, drag-and-drop rescheduling, and an in-place
//! Day Overflow Popover when "+N more" is clicked.

use leptos::html;
use leptos::prelude::*;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use super::model::{
    SpanPlacement, at, day, format_hhmm, is_spanning, is_weekend, local, occurrence_key, on_day, s,
    sort_events, week_spans,
};
use crate::icons::Icon;
use crate::pim::Collection;

const MAX_MONTH_SPAN_ROWS: usize = 2;

#[component]
pub fn MonthView(
    days: Memo<Vec<String>>,
    anchor: RwSignal<String>,
    today: Memo<String>,
    events: RwSignal<Vec<Value>>,
    calendars: RwSignal<Vec<Collection>>,
    zone: Memo<String>,
    time_format: Signal<String>,
    selected: RwSignal<Option<Value>>,
    month_limit: RwSignal<usize>,
    month_grid: NodeRef<html::Div>,
    can_create: Memo<bool>,
    on_select: Callback<Value>,
    on_pick_day: Callback<String>,
    on_quick_create: Callback<(String, u32)>,
    on_drag_start: Callback<(Value, bool)>,
    on_drop: Callback<String>,
    on_toggle_task: Callback<Value>,
) -> impl IntoView {
    let overflow_day: RwSignal<Option<String>> = RwSignal::new(None);
    let drag_over_day: RwSignal<Option<String>> = RwSignal::new(None);

    let color_of = move |cals: &[Collection], cid: &str| -> String {
        cals.iter()
            .find(|c| c.id == cid)
            .map(|c| c.color.clone())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "#73396f".into())
    };

    let editable_of = move |cals: &[Collection], ev: &Value| -> bool {
        ev["task"] != json!(true)
            && cals
                .iter()
                .find(|c| c.id == s(ev, "collectionId"))
                .is_some_and(|c| c.role != "read" && c.subscription.is_none())
    };

    view! {
        <div class="calendar-month-shell">
            <div node_ref=month_grid class="calendar-month" aria-label="Month calendar">
                <div class="calendar-month-weekdays">
                    {move || {
                        days.get().into_iter().take(7).map(|d| {
                            let wknd = is_weekend(&d);
                            view! {
                                <div class=if wknd { "calendar-weekday is-weekend" } else { "calendar-weekday" }>
                                    {day(&d, &[("weekday", "short")])}
                                </div>
                            }
                        }).collect_view()
                    }}
                </div>
                <div class="calendar-month-weeks">
                    {move || {
                        let all_days = days.get();
                        let list = events.get();
                        let cals = calendars.get();
                        let z = zone.get();
                        let fmt = time_format.get();
                        let today_now = today.get();
                        let anchor_month = anchor.get()[..7].to_owned();
                        let limit = month_limit.get().max(1);

                        all_days.chunks(7).enumerate().map(|(_w_idx, week_slice)| {
                            let week_days: Vec<String> = week_slice.to_vec();
                            let spans = week_spans(&week_days, &list, &z);
                            let visible_spans: Vec<SpanPlacement> = spans
                                .iter()
                                .filter(|sp| sp.row <= MAX_MONTH_SPAN_ROWS)
                                .cloned()
                                .collect();

                            // How many span rows occupy each column (0..7) so timed events sit below.
                            let mut col_span_rows = [0usize; 7];
                            let mut col_hidden_spans = [0usize; 7];
                            for sp in &spans {
                                let start_c = sp.col_start.saturating_sub(1);
                                let end_c = (start_c + sp.col_span).min(7);
                                if sp.row <= MAX_MONTH_SPAN_ROWS {
                                    for c in start_c..end_c {
                                        col_span_rows[c] = col_span_rows[c].max(sp.row);
                                    }
                                } else {
                                    for c in start_c..end_c {
                                        col_hidden_spans[c] += 1;
                                    }
                                }
                            }

                            view! {
                                <div class="calendar-month-week">
                                    <div class="calendar-month-week-days">
                                        {week_days.iter().enumerate().map(|(col_idx, date)| {
                                            let date = date.clone();
                                            let outside = date[..7] != anchor_month;
                                            let is_today = date == today_now;
                                            let wknd = is_weekend(&date);
                                            let span_rows_here = col_span_rows[col_idx];
                                            let hidden_spans_here = col_hidden_spans[col_idx];

                                            // Single-day timed events & tasks on this day, chronologically sorted.
                                            let mut single_events: Vec<Value> = list
                                                .iter()
                                                .filter(|e| !is_spanning(e, &z) && on_day(e, &date, &z))
                                                .cloned()
                                                .collect();
                                            sort_events(&mut single_events);

                                            let remaining_slots = limit.saturating_sub(span_rows_here);
                                            let shown_single = single_events.len().min(remaining_slots);
                                            let hidden_count = (single_events.len() - shown_single) + hidden_spans_here;

                                            let d_drop = date.clone();
                                            let d_over = date.clone();
                                            let d_pick = date.clone();
                                            let d_quick = date.clone();
                                            let d_more = date.clone();
                                            let d_over_check = date.clone();
                                            let label_num = date[8..].trim_start_matches('0').to_owned();
                                            let full_label = day(&date, &[("weekday", "long"), ("month", "long"), ("day", "numeric")]);

                                            view! {
                                                <div
                                                    class=move || {
                                                        let mut cls = String::from("calendar-month-day");
                                                        if outside { cls.push_str(" outside-month"); }
                                                        if is_today { cls.push_str(" is-today"); }
                                                        if wknd { cls.push_str(" is-weekend"); }
                                                        if drag_over_day.with(|cur| cur.as_deref() == Some(d_over_check.as_str())) {
                                                            cls.push_str(" is-drag-over");
                                                        }
                                                        cls
                                                    }
                                                    data-date=date.clone()
                                                    on:dragover=move |e| {
                                                        e.prevent_default();
                                                        if drag_over_day.with_untracked(|cur| cur.as_deref() != Some(d_over.as_str())) {
                                                            drag_over_day.set(Some(d_over.clone()));
                                                        }
                                                    }
                                                    on:dragleave=move |_| {
                                                        drag_over_day.set(None);
                                                    }
                                                    on:drop=move |e| {
                                                        e.prevent_default();
                                                        drag_over_day.set(None);
                                                        on_drop.run(d_drop.clone());
                                                    }
                                                    on:click=move |e| {
                                                        if !can_create.get_untracked() {
                                                            return;
                                                        }
                                                        // Only trigger quick-create when clicking empty cell background.
                                                        let hit_control = e
                                                            .target()
                                                            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                                                            .and_then(|t| t.closest("button,a").ok().flatten())
                                                            .is_some();
                                                        if !hit_control {
                                                            on_quick_create.run((d_quick.clone(), 9));
                                                        }
                                                    }
                                                >
                                                    <div class="calendar-month-day-head">
                                                        <button
                                                            type="button"
                                                            class=if is_today { "calendar-date today" } else { "calendar-date" }
                                                            aria-label=full_label
                                                            on:click=move |_| on_pick_day.run(d_pick.clone())
                                                        >
                                                            {label_num}
                                                        </button>
                                                    </div>
                                                    {(span_rows_here > 0).then(|| view! {
                                                        <div class=format!("calendar-span-spacer span-spacer-{span_rows_here}") aria-hidden="true"></div>
                                                    })}
                                                    <div class="calendar-month-day-events">
                                                        {single_events.into_iter().take(shown_single).map(|ev| {
                                                            let color = color_of(&cals, &s(&ev, "collectionId"));
                                                            let is_task = ev["task"] == json!(true);
                                                            let title = s(&ev, "title");
                                                            let status = s(&ev, "status");
                                                            let cancelled = status == "CANCELLED";
                                                            let tentative = status == "TENTATIVE";
                                                            let repeats = ev["recurring"] == json!(true) || !s(&ev, "recurrence").is_empty();
                                                            let start_hm = format_hhmm(&local(at(&ev, "startAt"), &z)[11..16], &fmt);

                                                            if is_task {
                                                                let done = status == "COMPLETED";
                                                                let ev_t = ev.clone();
                                                                return view! {
                                                                    <div class=format!("calendar-event-wrap is-task{}", if done { " is-done" } else { "" })>
                                                                        <button
                                                                            type="button"
                                                                            class=if done { "calendar-task-toggle mini is-done" } else { "calendar-task-toggle mini" }
                                                                            aria-label=if done { format!("Reopen task {title}") } else { format!("Complete task {title}") }
                                                                            on:click=move |_| on_toggle_task.run(ev_t.clone())
                                                                        >
                                                                            <Icon name="check"/>
                                                                        </button>
                                                                        <a
                                                                            class=format!("calendar-event calendar-task{}", if done { " is-done" } else { "" })
                                                                            href=format!("/tasks?item={}", js_sys::encode_uri_component(&s(&ev, "resourceId")))
                                                                        >
                                                                            <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                                            <svg class="calendar-event-dot" viewBox="0 0 10 10" aria-hidden="true"><circle cx="5" cy="5" r="4" fill=color></circle></svg>
                                                                            <strong>{title}</strong>
                                                                            <span class="calendar-event-time">{start_hm}</span>
                                                                        </a>
                                                                    </div>
                                                                }.into_any();
                                                            }

                                                            let key = occurrence_key(&ev);
                                                            let is_sel = Signal::derive(move || selected.with(|sel| sel.as_ref().is_some_and(|x| occurrence_key(x) == key)));
                                                            let can_drag = editable_of(&cals, &ev);
                                                            let ev_click = ev.clone();
                                                            let ev_drag = ev.clone();
                                                            let cls = format!(
                                                                "calendar-event{}{}",
                                                                if cancelled { " cancelled" } else { "" },
                                                                if tentative { " tentative" } else { "" }
                                                            );
                                                            view! {
                                                                <div class=move || format!("calendar-event-wrap{}", if is_sel.get() { " is-selected" } else { "" })>
                                                                    <button
                                                                        type="button"
                                                                        class=cls
                                                                        draggable=can_drag.then_some("true")
                                                                        aria-pressed=move || is_sel.get().to_string()
                                                                        on:dragstart=move |e: web_sys::DragEvent| if can_drag {
                                                                            if let Some(dt) = e.data_transfer() { let _ = dt.set_data("text/plain", &s(&ev_drag, "resourceId")); }
                                                                            on_drag_start.run((ev_drag.clone(), false));
                                                                        }
                                                                        on:click=move |_| on_select.run(ev_click.clone())
                                                                    >
                                                                        <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                                        <svg class="calendar-event-edge" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                                        <svg class="calendar-event-dot" viewBox="0 0 10 10" aria-hidden="true"><circle cx="5" cy="5" r="4" fill=color></circle></svg>
                                                                        <strong>{title}</strong>
                                                                        <span class="calendar-event-time">{start_hm}</span>
                                                                        {repeats.then(|| view! { <span class="calendar-event-repeat" aria-label="Repeats"><Icon name="repeat"/></span> })}
                                                                    </button>
                                                                </div>
                                                            }.into_any()
                                                        }).collect_view()}
                                                        {(hidden_count > 0).then(|| {
                                                            let more_label = format!("Show all events for {d_more}");
                                                            let d_open = d_more.clone();
                                                            view! {
                                                                <button
                                                                    type="button"
                                                                    class="calendar-more"
                                                                    aria-label=more_label
                                                                    on:click=move |_| overflow_day.set(Some(d_open.clone()))
                                                                >
                                                                    {format!("+{hidden_count} more")}
                                                                </button>
                                                            }
                                                        })}
                                                    </div>
                                                </div>
                                            }
                                        }).collect_view()}
                                    </div>

                                    {(!visible_spans.is_empty()).then(|| view! {
                                        <div class="calendar-month-spans">
                                            {visible_spans.into_iter().map(|sp| {
                                                let ev = sp.event;
                                                let color = color_of(&cals, &s(&ev, "collectionId"));
                                                let is_task = ev["task"] == json!(true);
                                                let title = s(&ev, "title");
                                                let status = s(&ev, "status");
                                                let cancelled = status == "CANCELLED";
                                                let tentative = status == "TENTATIVE";
                                                let repeats = ev["recurring"] == json!(true) || !s(&ev, "recurrence").is_empty();
                                                let span_cls = format!(
                                                    "calendar-span-slot span-col-{} span-len-{} span-row-{}{}{}",
                                                    sp.col_start,
                                                    sp.col_span,
                                                    sp.row,
                                                    if sp.continues_left { " cont-left" } else { "" },
                                                    if sp.continues_right { " cont-right" } else { "" }
                                                );

                                                if is_task {
                                                    let done = status == "COMPLETED";
                                                    return view! {
                                                        <div class=span_cls>
                                                            <a
                                                                class=format!("calendar-event all-day calendar-task{}", if done { " is-done" } else { "" })
                                                                href=format!("/tasks?item={}", js_sys::encode_uri_component(&s(&ev, "resourceId")))
                                                            >
                                                                <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                                <svg class="calendar-event-edge" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color></rect></svg>
                                                                <strong>{title}</strong>
                                                            </a>
                                                        </div>
                                                    }.into_any();
                                                }

                                                let key = occurrence_key(&ev);
                                                let is_sel = Signal::derive(move || selected.with(|sel| sel.as_ref().is_some_and(|x| occurrence_key(x) == key)));
                                                let can_drag = editable_of(&cals, &ev);
                                                let ev_click = ev.clone();
                                                let ev_drag = ev.clone();
                                                let cls = format!(
                                                    "calendar-event all-day{}{}",
                                                    if cancelled { " cancelled" } else { "" },
                                                    if tentative { " tentative" } else { "" }
                                                );
                                                view! {
                                                    <div class=move || format!("{span_cls}{}", if is_sel.get() { " is-selected" } else { "" })>
                                                        <button
                                                            type="button"
                                                            class=cls
                                                            draggable=can_drag.then_some("true")
                                                            aria-pressed=move || is_sel.get().to_string()
                                                            on:dragstart=move |e: web_sys::DragEvent| if can_drag {
                                                                if let Some(dt) = e.data_transfer() { let _ = dt.set_data("text/plain", &s(&ev_drag, "resourceId")); }
                                                                on_drag_start.run((ev_drag.clone(), false));
                                                            }
                                                            on:click=move |_| on_select.run(ev_click.clone())
                                                        >
                                                            <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                            <svg class="calendar-event-edge" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color></rect></svg>
                                                            <strong>{title}</strong>
                                                            {repeats.then(|| view! { <span class="calendar-event-repeat" aria-label="Repeats"><Icon name="repeat"/></span> })}
                                                        </button>
                                                    </div>
                                                }.into_any()
                                            }).collect_view()}
                                        </div>
                                    })}
                                </div>
                            }
                        }).collect_view()
                    }}
                </div>
            </div>

            {move || overflow_day.get().map(|date| {
                let z = zone.get();
                let fmt = time_format.get();
                let cals = calendars.get();
                let mut rows: Vec<Value> = events.get().into_iter().filter(|e| on_day(e, &date, &z)).collect();
                sort_events(&mut rows);
                let title = day(&date, &[("weekday", "long"), ("month", "long"), ("day", "numeric")]);
                let title_aria = format!("Events on {title}");
                let d_day_view = date.clone();
                view! {
                    <div class="calendar-overflow-backdrop" on:click=move |e| {
                        let outside = e
                            .target()
                            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                            .is_some_and(|el| el.class_list().contains("calendar-overflow-backdrop"));
                        if outside { overflow_day.set(None); }
                    }>
                        <div class="calendar-overflow-card" role="dialog" aria-label=title_aria>
                            <div class="calendar-overflow-head">
                                <h3>{title}</h3>
                                <button type="button" class="text-button" on:click=move |_| {
                                    overflow_day.set(None);
                                    on_pick_day.run(d_day_view.clone());
                                }>"Day view"</button>
                                <button type="button" class="icon-button" aria-label="Close day events" on:click=move |_| overflow_day.set(None)>
                                    <Icon name="close"/>
                                </button>
                            </div>
                            <div class="calendar-overflow-list">
                                {rows.into_iter().map(|ev| {
                                    let color = color_of(&cals, &s(&ev, "collectionId"));
                                    let is_task = ev["task"] == json!(true);
                                    let all_day = ev["allDay"] == json!(true);
                                    let ev_title = s(&ev, "title");
                                    let time_label = if all_day {
                                        "All day".to_owned()
                                    } else {
                                        format_hhmm(&local(at(&ev, "startAt"), &z)[11..16], &fmt)
                                    };
                                    if is_task {
                                        return view! {
                                            <a class="calendar-event calendar-task" href=format!("/tasks?item={}", js_sys::encode_uri_component(&s(&ev, "resourceId")))>
                                                <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                <svg class="calendar-event-dot" viewBox="0 0 10 10" aria-hidden="true"><circle cx="5" cy="5" r="4" fill=color></circle></svg>
                                                <strong>{ev_title}</strong>
                                                <span class="calendar-event-time">{time_label}</span>
                                            </a>
                                        }.into_any();
                                    }
                                    let ev_pick = ev.clone();
                                    view! {
                                        <button
                                            type="button"
                                            class=if all_day { "calendar-event all-day" } else { "calendar-event" }
                                            on:click=move |_| {
                                                overflow_day.set(None);
                                                on_select.run(ev_pick.clone());
                                            }
                                        >
                                            <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                            <svg class="calendar-event-edge" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                            <svg class="calendar-event-dot" viewBox="0 0 10 10" aria-hidden="true"><circle cx="5" cy="5" r="4" fill=color></circle></svg>
                                            <strong>{ev_title}</strong>
                                            <span class="calendar-event-time">{time_label}</span>
                                        </button>
                                    }.into_any()
                                }).collect_view()}
                            </div>
                        </div>
                    </div>
                }
            })}
        </div>
    }
}
