//! Week & Day time grids: sticky day headers with timezone corner, dedicated sticky All-day
//! band (spanning continuous ribbons in Week view, never colliding with 00:00!), 24-hour
//! scrollable canvas with live now-line pill, rich duration-sized event blocks, and sleek
//! bottom-edge drag-to-resize.

use leptos::html;
use leptos::prelude::*;
use serde_json::{Value, json};

use super::model::{
    at, day, day_lanes, format_hhmm, format_hour_label, is_weekend, local, occurrence_key, on_day,
    s, sort_events, week_spans, zone_short,
};
use crate::icons::Icon;
use crate::pim::Collection;

#[component]
pub fn TimeGridView(
    days: Memo<Vec<String>>,
    single_day: Memo<bool>,
    today: Memo<String>,
    events: RwSignal<Vec<Value>>,
    calendars: RwSignal<Vec<Collection>>,
    zone: Memo<String>,
    time_format: Signal<String>,
    selected: RwSignal<Option<Value>>,
    now_tick: RwSignal<f64>,
    grid_ref: NodeRef<html::Div>,
    can_create: Memo<bool>,
    on_select: Callback<Value>,
    on_pick_day: Callback<String>,
    on_quick_create: Callback<(String, u32)>,
    on_drag_start: Callback<(Value, bool)>,
    on_drop: Callback<(String, Option<u32>)>,
    on_toggle_task: Callback<Value>,
) -> impl IntoView {
    let lanes = Memo::new(move |_| day_lanes(&days.get(), &events.get(), &zone.get()));
    let drag_target: RwSignal<Option<String>> = RwSignal::new(None);

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
        <div class=move || if single_day.get() { "calendar-time-wrap single-day" } else { "calendar-time-wrap" }>
            <div class="calendar-time-top">
                <div class="calendar-time-header">
                    <div class="calendar-tz-corner" title=move || zone.get()>
                        {move || zone_short(&zone.get())}
                    </div>
                    {move || {
                        let today_now = today.get();
                        let is_single = single_day.get();
                        days.get().into_iter().map(|date| {
                            let is_today = date == today_now;
                            let wknd = is_weekend(&date);
                            let wd = day(&date, &[("weekday", "short")]);
                            let num = date[8..].trim_start_matches('0').to_owned();
                            let d_pick = date.clone();
                            let cls = format!(
                                "calendar-day-heading{}{}",
                                if is_today { " is-today" } else { "" },
                                if wknd { " is-weekend" } else { "" }
                            );
                            if is_single {
                                view! {
                                    <div class=cls>
                                        <span>{wd}</span>
                                        <b>{num}</b>
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <button type="button" class=cls on:click=move |_| on_pick_day.run(d_pick.clone())>
                                        <span>{wd}</span>
                                        <b>{num}</b>
                                    </button>
                                }.into_any()
                            }
                        }).collect_view()
                    }}
                </div>

                <div class="calendar-all-day-band">
                    <div class="calendar-all-day-label">"All day"</div>
                    <div class="calendar-all-day-body">
                        {move || {
                            let vis_days = days.get();
                            let is_single = single_day.get();
                            let z = zone.get();
                            let cals = calendars.get();
                            let list = events.get();

                            if is_single {
                                let date = vis_days.first().cloned().unwrap_or_default();
                                let mut all_day_events: Vec<Value> = list
                                    .iter()
                                    .filter(|e| e["allDay"] == json!(true) && on_day(e, &date, &z))
                                    .cloned()
                                    .collect();
                                sort_events(&mut all_day_events);
                                let d_drop = date.clone();
                                view! {
                                    <div
                                        class="calendar-all-day-single all-day-slot"
                                        on:dragover=move |e| e.prevent_default()
                                        on:drop=move |e| {
                                            e.prevent_default();
                                            on_drop.run((d_drop.clone(), None));
                                        }
                                    >
                                        {all_day_events.into_iter().map(|ev| {
                                            let color = color_of(&cals, &s(&ev, "collectionId"));
                                            let title = s(&ev, "title");
                                            let is_task = ev["task"] == json!(true);
                                            if is_task {
                                                let done = s(&ev, "status") == "COMPLETED";
                                                return view! {
                                                    <a class=format!("calendar-event all-day calendar-task{}", if done { " is-done" } else { "" }) href=format!("/tasks?item={}", js_sys::encode_uri_component(&s(&ev, "resourceId")))>
                                                        <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                        <svg class="calendar-event-edge" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color></rect></svg>
                                                        <strong>{title}</strong>
                                                    </a>
                                                }.into_any();
                                            }
                                            let key = occurrence_key(&ev);
                                            let is_sel = Signal::derive(move || selected.with(|sel| sel.as_ref().is_some_and(|x| occurrence_key(x) == key)));
                                            let ev_click = ev.clone();
                                            view! {
                                                <button
                                                    type="button"
                                                    class=move || format!("calendar-event all-day{}", if is_sel.get() { " is-selected" } else { "" })
                                                    aria-pressed=move || is_sel.get().to_string()
                                                    on:click=move |_| on_select.run(ev_click.clone())
                                                >
                                                    <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                    <svg class="calendar-event-edge" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color></rect></svg>
                                                    <strong>{title}</strong>
                                                </button>
                                            }.into_any()
                                        }).collect_view()}
                                    </div>
                                }.into_any()
                            } else {
                                let spans = week_spans(&vis_days, &list, &z);
                                view! {
                                    <div class="calendar-all-day-week">
                                        <div class="calendar-all-day-dropcols">
                                            {vis_days.iter().map(|date| {
                                                let d_drop = date.clone();
                                                let is_today = *date == today.get();
                                                let wknd = is_weekend(date);
                                                view! {
                                                    <div
                                                        class=format!(
                                                            "all-day-slot{}{}",
                                                            if is_today { " is-today" } else { "" },
                                                            if wknd { " is-weekend" } else { "" }
                                                        )
                                                        on:dragover=move |e| e.prevent_default()
                                                        on:drop=move |e| {
                                                            e.prevent_default();
                                                            on_drop.run((d_drop.clone(), None));
                                                        }
                                                    ></div>
                                                }
                                            }).collect_view()}
                                        </div>
                                        {(!spans.is_empty()).then(|| view! {
                                            <div class="calendar-all-day-spans">
                                                {spans.into_iter().take(12).map(|sp| {
                                                    let ev = sp.event;
                                                    let color = color_of(&cals, &s(&ev, "collectionId"));
                                                    let title = s(&ev, "title");
                                                    let is_task = ev["task"] == json!(true);
                                                    let row_clamped = sp.row.min(4);
                                                    let span_cls = format!(
                                                        "calendar-span-slot span-col-{} span-len-{} span-row-{}{}{}",
                                                        sp.col_start,
                                                        sp.col_span,
                                                        row_clamped,
                                                        if sp.continues_left { " cont-left" } else { "" },
                                                        if sp.continues_right { " cont-right" } else { "" }
                                                    );
                                                    if is_task {
                                                        let done = s(&ev, "status") == "COMPLETED";
                                                        return view! {
                                                            <div class=span_cls>
                                                                <a class=format!("calendar-event all-day calendar-task{}", if done { " is-done" } else { "" }) href=format!("/tasks?item={}", js_sys::encode_uri_component(&s(&ev, "resourceId")))>
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
                                                    let status = s(&ev, "status");
                                                    let cls = format!(
                                                        "calendar-event all-day{}{}",
                                                        if status == "CANCELLED" { " cancelled" } else { "" },
                                                        if status == "TENTATIVE" { " tentative" } else { "" }
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
                                                            </button>
                                                        </div>
                                                    }.into_any()
                                                }).collect_view()}
                                            </div>
                                        })}
                                    </div>
                                }.into_any()
                            }
                        }}
                    </div>
                </div>
            </div>

            <div node_ref=grid_ref class="calendar-time-scroll" aria-label="Calendar time grid">
                <div class="calendar-time-grid">
                    {move || {
                        let vis_days = days.get();
                        let list = events.get();
                        let cals = calendars.get();
                        let lane_map = lanes.get();
                        let z = zone.get();
                        let fmt = time_format.get();
                        let now_ms = now_tick.get();
                        let now_local = local(now_ms, &z);
                        let now_day = now_local[..10].to_owned();
                        let now_hour: u32 = now_local[11..13].parse().unwrap_or(0);
                        let now_min: u32 = now_local[14..16].parse().unwrap_or(0);
                        let now_step = (now_min / 5).min(11);
                        let now_pill = format_hhmm(&now_local[11..16], &fmt);

                        (0..24u32).map(|hour| {
                            let hour_text = if hour == 0 {
                                String::new()
                            } else {
                                format_hour_label(hour, &fmt)
                            };
                            view! {
                                <div class="calendar-time-label calendar-hour-label">
                                    <span>{hour_text}</span>
                                </div>
                                {vis_days.iter().map(|date| {
                                    let date = date.clone();
                                    let is_today = date == now_day;
                                    let wknd = is_weekend(&date);
                                    let show_now = is_today && hour == now_hour;
                                    let slot_key = format!("{date}-{hour}");
                                    let sk_over = slot_key.clone();
                                    let sk_check = slot_key.clone();
                                    let d_drop = date.clone();
                                    let d_create = date.clone();

                                    // Timed events starting in this date + hour.
                                    let hour_events: Vec<Value> = list
                                        .iter()
                                        .filter(|e| {
                                            if e["allDay"] == json!(true) {
                                                return false;
                                            }
                                            let st = local(at(e, "startAt"), &z);
                                            st[..10] == date
                                                && st[11..13].parse::<u32>().unwrap_or(99) == hour
                                        })
                                        .cloned()
                                        .collect();

                                    view! {
                                        <div
                                            class=move || {
                                                let mut cls = String::from("calendar-slot");
                                                if is_today { cls.push_str(" is-today"); }
                                                if wknd { cls.push_str(" is-weekend"); }
                                                if drag_target.with(|cur| cur.as_deref() == Some(sk_check.as_str())) {
                                                    cls.push_str(" is-drag-over");
                                                }
                                                cls
                                            }
                                            on:dragover=move |e| {
                                                e.prevent_default();
                                                if drag_target.with_untracked(|cur| cur.as_deref() != Some(sk_over.as_str())) {
                                                    drag_target.set(Some(sk_over.clone()));
                                                }
                                            }
                                            on:dragleave=move |_| {
                                                drag_target.set(None);
                                            }
                                            on:drop=move |e| {
                                                e.prevent_default();
                                                drag_target.set(None);
                                                on_drop.run((d_drop.clone(), Some(hour)));
                                            }
                                        >
                                            <button
                                                type="button"
                                                class="calendar-slot-hit"
                                                aria-label=format!("New event {date} {hour:02}:00")
                                                disabled=move || !can_create.get()
                                                on:click=move |_| on_quick_create.run((d_create.clone(), hour))
                                            ></button>

                                            {show_now.then(|| view! {
                                                <div class=format!("calendar-now ev-at-{now_step}") aria-hidden="true">
                                                    <span class="calendar-now-pill">{now_pill.clone()}</span>
                                                </div>
                                            })}

                                            {hour_events.into_iter().map(|ev| {
                                                let color = color_of(&cals, &s(&ev, "collectionId"));
                                                let is_task = ev["task"] == json!(true);
                                                let title = s(&ev, "title");
                                                let location = s(&ev, "location");
                                                let status = s(&ev, "status");
                                                let cancelled = status == "CANCELLED";
                                                let tentative = status == "TENTATIVE";
                                                let repeats = ev["recurring"] == json!(true) || !s(&ev, "recurrence").is_empty();

                                                let start_ms = at(&ev, "startAt");
                                                let end_ms = at(&ev, "endAt").max(start_ms + 900_000.0);
                                                let st_local = local(start_ms, &z);
                                                let en_local = local(end_ms, &z);
                                                let start_min: u32 = st_local[14..16].parse().unwrap_or(0);
                                                let at_step = (start_min / 5).min(11);
                                                let duration_mins = ((end_ms - start_ms) / 60_000.0).round().max(15.0) as u32;
                                                // Cap visual height to remaining minutes in day so it stays inside the 24h grid.
                                                let max_rem_mins = (24 - hour) * 60 - start_min;
                                                let span_quarters = ((duration_mins.min(max_rem_mins)) as f64 / 15.0).ceil().clamp(1.0, 96.0) as usize;

                                                let lane_key = format!("{date}/{}", occurrence_key(&ev));
                                                let (lane, total_lanes) = lane_map.get(&lane_key).copied().unwrap_or((0, 1));
                                                let lane_clamped = lane.min(3);
                                                let lanes_clamped = total_lanes.clamp(1, 4);

                                                let time_range = format!(
                                                    "{} – {}",
                                                    format_hhmm(&st_local[11..16], &fmt),
                                                    format_hhmm(&en_local[11..16], &fmt)
                                                );

                                                let pos_cls = format!(
                                                    "calendar-event-wrap ev-at-{at_step} ev-span-{span_quarters} ev-lane-{lane_clamped} ev-lanes-{lanes_clamped}"
                                                );

                                                if is_task {
                                                    let done = status == "COMPLETED";
                                                    let ev_t = ev.clone();
                                                    return view! {
                                                        <div class=format!("{pos_cls} is-task{}", if done { " is-done" } else { "" })>
                                                            <a
                                                                class=format!("calendar-event calendar-task{}", if done { " is-done" } else { "" })
                                                                href=format!("/tasks?item={}", js_sys::encode_uri_component(&s(&ev, "resourceId")))
                                                            >
                                                                <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                                <svg class="calendar-event-edge" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color></rect></svg>
                                                                <strong>{title.clone()}</strong>
                                                                <span class="calendar-event-time">{format_hhmm(&st_local[11..16], &fmt)}</span>
                                                            </a>
                                                            <button
                                                                type="button"
                                                                class=if done { "calendar-task-toggle corner is-done" } else { "calendar-task-toggle corner" }
                                                                aria-label=if done { format!("Reopen task {title}") } else { format!("Complete task {title}") }
                                                                on:click=move |_| on_toggle_task.run(ev_t.clone())
                                                            >
                                                                <Icon name="check"/>
                                                            </button>
                                                        </div>
                                                    }.into_any();
                                                }

                                                let key = occurrence_key(&ev);
                                                let is_sel = Signal::derive(move || selected.with(|sel| sel.as_ref().is_some_and(|x| occurrence_key(x) == key)));
                                                let can_drag = editable_of(&cals, &ev);
                                                let ev_click = ev.clone();
                                                let ev_move = ev.clone();
                                                let ev_resize = ev.clone();
                                                let show_location = span_quarters >= 4 && !location.is_empty();
                                                let btn_cls = format!(
                                                    "calendar-event{}{}",
                                                    if cancelled { " cancelled" } else { "" },
                                                    if tentative { " tentative" } else { "" }
                                                );
                                                view! {
                                                    <div class=move || format!("{pos_cls}{}", if is_sel.get() { " is-selected" } else { "" })>
                                                        <button
                                                            type="button"
                                                            class=btn_cls
                                                            draggable=can_drag.then_some("true")
                                                            aria-pressed=move || is_sel.get().to_string()
                                                            on:dragstart=move |e: web_sys::DragEvent| if can_drag {
                                                                if let Some(dt) = e.data_transfer() { let _ = dt.set_data("text/plain", &s(&ev_move, "resourceId")); }
                                                                on_drag_start.run((ev_move.clone(), false));
                                                            }
                                                            on:click=move |_| on_select.run(ev_click.clone())
                                                        >
                                                            <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                                                            <svg class="calendar-event-edge" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color></rect></svg>
                                                            <strong>{title.clone()}</strong>
                                                            <span class="calendar-event-time">{time_range}</span>
                                                            {show_location.then(|| view! {
                                                                <span class="calendar-event-loc">{location}</span>
                                                            })}
                                                            {repeats.then(|| view! { <span class="calendar-event-repeat" aria-label="Repeats"><Icon name="repeat"/></span> })}
                                                        </button>
                                                        {can_drag.then(|| view! {
                                                            <button
                                                                type="button"
                                                                class="calendar-resize"
                                                                draggable="true"
                                                                aria-label=format!("Resize {title}")
                                                                title="Drag to resize"
                                                                on:dragstart=move |e: web_sys::DragEvent| {
                                                                    e.stop_propagation();
                                                                    if let Some(dt) = e.data_transfer() { let _ = dt.set_data("text/plain", &s(&ev_resize, "resourceId")); }
                                                                    on_drag_start.run((ev_resize.clone(), true));
                                                                }
                                                            ></button>
                                                        })}
                                                    </div>
                                                }.into_any()
                                            }).collect_view()}
                                        </div>
                                    }
                                }).collect_view()}
                            }
                        }).collect_view()
                    }}
                </div>
            </div>
        </div>
    }
}
