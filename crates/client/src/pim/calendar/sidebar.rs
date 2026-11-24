//! Calendar sidebar: + New event, dot-annotated MiniMonth, scrollable My calendars &
//! Subscribed sections (no artificial 1/2 pagination), first-class Tasks toggle, quick
//! solo/color calendar actions, and a compact "Up next" / Reminders footer card.

use std::collections::HashMap;

use leptos::prelude::*;
use serde_json::{Value, json};

use super::model::{QUICK_COLORS, at, format_hhmm, local, relative_countdown, s};
use crate::date_popover::MiniMonth;
use crate::icons::Icon;
use crate::pim::Collection;
use crate::ui::{ActionMenu, MenuAction};

#[component]
pub fn CalendarSidebar(
    ready_to_create: Memo<bool>,
    anchor: RwSignal<String>,
    today: Memo<String>,
    week_start: Signal<u32>,
    view_mode: RwSignal<String>,
    days: Memo<Vec<String>>,
    calendars: RwSignal<Vec<Collection>>,
    hidden: Memo<Vec<String>>,
    managed: RwSignal<String>,
    show_tasks: Signal<bool>,
    time_format: Signal<String>,
    zone: Memo<String>,
    events: RwSignal<Vec<Value>>,
    reminders: RwSignal<Vec<Value>>,
    now_tick: RwSignal<f64>,
    dots: Memo<HashMap<String, Vec<String>>>,
    on_new_event: Callback<()>,
    on_open_modal: Callback<&'static str>,
    on_toggle_calendar: Callback<(String, bool)>,
    on_solo_calendar: Callback<String>,
    on_show_all_calendars: Callback<()>,
    on_quick_color: Callback<(Collection, String)>,
    on_toggle_tasks: Callback<bool>,
    on_calendar_action: Callback<(Collection, &'static str)>,
    on_refresh_subscription: Callback<Collection>,
    on_edit_subscription: Callback<Collection>,
    on_select_event: Callback<Value>,
) -> impl IntoView {
    let range_days = Signal::derive(move || match view_mode.get().as_str() {
        "week" => Some(days.get()),
        "day" => Some(vec![anchor.get()]),
        _ => None,
    });

    let my_calendars = Memo::new(move |_| {
        calendars
            .get()
            .into_iter()
            .filter(|c| c.subscription.is_none())
            .collect::<Vec<_>>()
    });
    let subscribed_calendars = Memo::new(move |_| {
        calendars
            .get()
            .into_iter()
            .filter(|c| c.subscription.is_some())
            .collect::<Vec<_>>()
    });

    // Next upcoming non-cancelled timed event today (or next 24h) for the Up Next footer card.
    let up_next = Memo::new(move |_| {
        let now = now_tick.get();
        let list = events.get();
        list.into_iter()
            .filter(|e| {
                e["allDay"] != json!(true)
                    && e["task"] != json!(true)
                    && s(e, "status") != "CANCELLED"
                    && at(e, "endAt") > now
                    && at(e, "startAt") <= now + 24.0 * 3_600_000.0
            })
            .min_by(|a, b| at(a, "startAt").total_cmp(&at(b, "startAt")))
    });

    let render_row = move |c: Collection| {
        let hide = hidden.get();
        let shown = !hide.contains(&c.id);
        let any_hidden = !hide.is_empty();
        let id = c.id.clone();
        let (toggle_id, pick, solo_id) = (c.id.clone(), c.id.clone(), c.id.clone());
        let fill = if c.color.is_empty() {
            "currentColor".to_owned()
        } else {
            c.color.clone()
        };
        let subscription = c.subscription.clone();
        let failed = subscription
            .as_ref()
            .map(|sub| sub["lastError"].as_str().unwrap_or_default().to_owned())
            .filter(|e| !e.is_empty());
        let subscribed = subscription.is_some();
        let (c1, c2, c3, c4, c5, c6) = (
            c.clone(),
            c.clone(),
            c.clone(),
            c.clone(),
            c.clone(),
            c.clone(),
        );
        view! {
            <div class="calendar-nav-row">
                <label class="calendar-visibility">
                    <input
                        aria-label=format!("Show {}", c.name)
                        type="checkbox"
                        prop:checked=shown
                        on:change=move |e| on_toggle_calendar.run((toggle_id.clone(), event_target_checked(&e)))
                    />
                    <svg viewBox="0 0 16 16" aria-hidden="true">
                        <rect x="1" y="1" width="14" height="14" rx="4" fill=fill></rect>
                        <path d="m4.5 8.2 2.3 2.3 4.7-5" fill="none"></path>
                    </svg>
                </label>
                <button
                    type="button"
                    class=move || if managed.get() == id { "nav-item active" } else { "nav-item" }
                    aria-pressed=move || (managed.get() == pick).to_string()
                    on:click=move |_| managed.set(c1.id.clone())
                >
                    <span>{c.name.clone()}</span>
                    {subscription.map(|_| view! {
                        <span
                            class=if failed.is_some() { "calendar-subscribed has-error" } else { "calendar-subscribed" }
                            title=failed.clone().unwrap_or_else(|| "Subscribed by link".into())
                        >
                            <Icon name=if failed.is_some() { "alert" } else { "link" }/>
                            <span class="sr-only">
                                {failed.clone().map(|e| format!("Refresh failed: {e}")).unwrap_or_else(|| "Subscribed by link".into())}
                            </span>
                        </span>
                    })}
                </button>
                <ActionMenu
                    label=format!("Actions for calendar {}", c.name)
                    class="calendar-actions"
                    actions=move |_| {
                        let mut list = vec![
                            MenuAction::new("Show only this", "eye").run({
                                let solo_id = solo_id.clone();
                                move || on_solo_calendar.run(solo_id.clone())
                            }),
                        ];
                        if any_hidden {
                            list.push(MenuAction::new("Show all calendars", "checks").run(move || on_show_all_calendars.run(())));
                        }
                        if c.role == "owner" {
                            if subscribed {
                                list.push(MenuAction::new("Refresh now", "refresh").run({
                                    let c = c2.clone();
                                    move || on_refresh_subscription.run(c.clone())
                                }));
                                list.push(MenuAction::new("Edit subscription", "pencil").run({
                                    let c = c2.clone();
                                    move || on_edit_subscription.run(c.clone())
                                }));
                            } else {
                                list.push(MenuAction::new("Edit calendar", "pencil").run({
                                    let c = c3.clone();
                                    move || on_calendar_action.run((c.clone(), "renameCalendar"))
                                }));
                                let color_actions: Vec<MenuAction> = QUICK_COLORS
                                    .iter()
                                    .map(|(hex, label)| {
                                        let cal = c6.clone();
                                        let hex = (*hex).to_owned();
                                        let is_cur = cal.color.eq_ignore_ascii_case(&hex);
                                        MenuAction::new(*label, "palette")
                                            .current(is_cur)
                                            .run(move || on_quick_color.run((cal.clone(), hex.clone())))
                                    })
                                    .collect();
                                list.push(MenuAction::new("Color", "palette").items(color_actions));
                            }
                            list.push(MenuAction::new("Share calendar", "share").run({
                                let c = c4.clone();
                                move || on_calendar_action.run((c.clone(), "share"))
                            }));
                            list.push(
                                MenuAction::new(
                                    if subscribed { "Unsubscribe" } else { "Delete calendar" },
                                    "trash",
                                )
                                .danger()
                                .run({
                                    let c = c5.clone();
                                    move || on_calendar_action.run((c.clone(), "deleteCalendar"))
                                }),
                            );
                        }
                        list
                    }
                >
                    <Icon name="more"/>
                </ActionMenu>
            </div>
        }
    };

    view! {
        <button
            type="button"
            class="new-button ui-new"
            disabled=move || !ready_to_create.get()
            on:click=move |_| on_new_event.run(())
        >
            <Icon name="plus"/>"New event"
        </button>

        <MiniMonth
            value=anchor
            today=today
            week_start=week_start
            range=range_days
            dots=dots
            on_change=Callback::new(move |d| anchor.set(d))
        />

        <div class="calendar-sidebar-scroll">
            <div class="pim-sidebar-heading">
                <strong>"Calendars"</strong>
                <ActionMenu
                    label="Add calendar"
                    class="calendar-add-menu"
                    actions=move |_| vec![
                        MenuAction::new("New calendar", "calendar").run(move || on_open_modal.run("newCalendar")),
                        MenuAction::new("Subscribe by link", "link").run(move || on_open_modal.run("subscribe")),
                    ]
                >
                    <Icon name="plus"/>
                </ActionMenu>
            </div>
            <div class="calendar-list">
                {move || my_calendars.get().into_iter().map(render_row).collect_view()}
                <div class="calendar-nav-row calendar-tasks-toggle-row">
                    <label class="calendar-visibility">
                        <input
                            aria-label="Show dated tasks"
                            type="checkbox"
                            prop:checked=move || show_tasks.get()
                            on:change=move |e| on_toggle_tasks.run(event_target_checked(&e))
                        />
                        <svg viewBox="0 0 16 16" aria-hidden="true">
                            <rect x="1" y="1" width="14" height="14" rx="4" fill="#5b4b8a"></rect>
                            <path d="m4.5 8.2 2.3 2.3 4.7-5" fill="none"></path>
                        </svg>
                    </label>
                    <button
                        type="button"
                        class="nav-item"
                        on:click=move |_| on_toggle_tasks.run(!show_tasks.get_untracked())
                    >
                        <span>"Tasks"</span>
                    </button>
                </div>
            </div>

            {move || {
                let subs = subscribed_calendars.get();
                (!subs.is_empty()).then(|| view! {
                    <div class="pim-sidebar-heading">
                        <strong>"Subscribed"</strong>
                    </div>
                    <div class="calendar-list">
                        {subs.into_iter().map(render_row).collect_view()}
                    </div>
                })
            }}
        </div>

        <div class="calendar-sidebar-footer">
            {move || match up_next.get() {
                Some(ev) => {
                    let z = zone.get();
                    let fmt = time_format.get();
                    let start_ms = at(&ev, "startAt");
                    let end_ms = at(&ev, "endAt");
                    let start_hm = format_hhmm(&local(start_ms, &z)[11..16], &fmt);
                    let end_hm = format_hhmm(&local(end_ms, &z)[11..16], &fmt);
                    let countdown = relative_countdown(now_tick.get(), start_ms);
                    let cal_color = calendars.with(|list| {
                        list.iter()
                            .find(|c| c.id == s(&ev, "collectionId"))
                            .map(|c| c.color.clone())
                            .filter(|c| !c.is_empty())
                            .unwrap_or_else(|| "#73396f".into())
                    });
                    let due_count = reminders.with(Vec::len);
                    let ev_click = ev.clone();
                    view! {
                        <div class="calendar-up-next">
                            <button
                                type="button"
                                class="calendar-up-next-main"
                                on:click=move |_| on_select_event.run(ev_click.clone())
                            >
                                <span class="calendar-up-next-top">
                                    <span>"Up next"</span>
                                    <span class="calendar-up-next-pill">{countdown}</span>
                                </span>
                                <strong class="calendar-up-next-title">{s(&ev, "title")}</strong>
                                <span class="calendar-up-next-meta">
                                    <svg class="collection-dot" viewBox="0 0 16 16" aria-hidden="true">
                                        <circle cx="8" cy="8" r="5" fill=cal_color></circle>
                                    </svg>
                                    <span>{format!("{start_hm} – {end_hm}")}</span>
                                </span>
                            </button>
                            <button
                                type="button"
                                class="nav-item calendar-reminders-inline"
                                on:click=move |_| on_open_modal.run("reminders")
                            >
                                <Icon name="bell"/>
                                <span>"Reminders"</span>
                                {(due_count > 0).then(|| view! { <span class="reminder-count">{due_count}</span> })}
                            </button>
                        </div>
                    }
                    .into_any()
                }
                None => view! {
                    <button
                        type="button"
                        class="nav-item calendar-reminders"
                        on:click=move |_| on_open_modal.run("reminders")
                    >
                        <Icon name="bell"/>
                        <span>"Reminders"</span>
                        {move || (!reminders.with(Vec::is_empty)).then(|| view! {
                            <span class="reminder-count">{reminders.with(Vec::len)}</span>
                        })}
                    </button>
                }
                .into_any(),
            }}
        </div>
    }
}
