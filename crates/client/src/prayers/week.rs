//! Week timetable and right-hand Day Inspector with 3-icon prayer tracking buttons.

use std::collections::BTreeMap;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::icons::Icon;
use crate::prayers::calc::{
    civil_to_hijri, compute_current_window, compute_day_times, minutes_delta,
    qibla_bearing_and_distance,
};
use crate::prayers::model::{
    ALL_PRAYERS, OBLIGATORY_PRAYERS, PrayerKey, PrayerSettings, PrayerStatus, YearLog,
    method_short_label,
};
use crate::prayers::today::{StatusIcon, StatusLegend};
use crate::prayers::tz::{CivilDate, format_countdown, format_time_in_tz};
use crate::ui::document;

#[component]
pub fn WeekMain(
    settings: Signal<PrayerSettings>,
    previous_settings: RwSignal<Option<PrayerSettings>>,
    logs: RwSignal<BTreeMap<u32, YearLog>>,
    now_ms: Signal<f64>,
    selected_date: RwSignal<CivilDate>,
    week_start: RwSignal<CivilDate>,
    on_select_day: Callback<CivilDate>,
) -> impl IntoView {
    let win_key = Memo::new(move |_| {
        let s = settings.get();
        let w = compute_current_window(now_ms.get(), &s.location, &s.calculation);
        (w.today, w.current_prayer, w.next_prayer, w.next_date, w.next_time_ms)
    });

    view! {
        {move || {
            let s = settings.get();
            let prev_s = previous_settings.get();
            let start = week_start.get();
            let end = start.add_days(6);
            let sel = selected_date.get();
            let (win_today, win_cur, win_next, win_next_date, win_next_ms) = win_key.get();
            let use_24h = s.display.time_format == "24h";
            let tz = &s.location.timezone;
            let next_str = format_time_in_tz(win_next_ms, tz, use_24h);
            let live_cd = Signal::derive(move || {
                let s2 = settings.get();
                let w = compute_current_window(now_ms.get(), &s2.location, &s2.calculation);
                format!("in {}", format_countdown(w.remaining_ms))
            });

            let range_sub = if start.month == end.month {
                format!("{} {} \u{2013} {}, {} \u{00b7} {}", start.short_month_name(), start.day, end.day, start.year, method_short_label(&s.calculation.method))
            } else {
                format!("{} {} \u{2013} {} {}, {} \u{00b7} {}", start.short_month_name(), start.day, end.short_month_name(), end.day, end.year, method_short_label(&s.calculation.method))
            };

            view! {
                <header class="prayers-sheet-head">
                    <div class="prayers-head-title">
                        <h1>{format!("{} at {}", win_next.label(), next_str)}</h1>
                        <div class="prayers-head-sub">{format!("{} \u{00b7} {range_sub}", s.location.label)}</div>
                    </div>
                    <div class="prayers-head-actions">
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="Previous week"
                            on:click=move |_| week_start.update(|d| *d = d.add_days(-7))
                        >
                            <Icon name="back"/>
                        </button>
                        <button
                            type="button"
                            class="prayers-pill-btn"
                            on:click=move |_| {
                                let t = win_today;
                                let offset = t.weekday() as i64;
                                week_start.set(t.add_days(-offset));
                                on_select_day.try_run(t);
                            }
                        >
                            "Today"
                        </button>
                        <button
                            type="button"
                            class="icon-button is-next-arrow"
                            aria-label="Next week"
                            on:click=move |_| week_start.update(|d| *d = d.add_days(7))
                        >
                            <Icon name="chevron"/>
                        </button>
                    </div>
                </header>

                <div class="prayers-sheet-body">
                    <div class="prayers-status-strip">
                        <div class="prayers-status-strip-left">
                            <span class="prayers-phase-pill">
                                <span class="prayers-phase-dot" aria-hidden="true"></span>
                                {format!("Now \u{00b7} {}", win_cur.label())}
                            </span>
                            <strong>{format!("Next: {} at {next_str}", win_next.label())}</strong>
                        </div>
                        {s.display.show_countdown.then(|| view! {
                            <span class="prayers-status-cd prayers-num">{move || live_cd.get()}</span>
                        })}
                    </div>

                    <div class="prayers-table-wrap">
                        <table class="prayer-week" aria-label="Seven-day prayer schedule">
                            <thead>
                                <tr>
                                    <th scope="col">"Day"</th>
                                    {ALL_PRAYERS.into_iter().filter(|p| s.display.show_sunrise || *p != PrayerKey::Sunrise).map(|p| {
                                        view! { <th scope="col">{p.label()}</th> }
                                    }).collect_view()}
                                    {s.display.show_tracking.then(|| view! { <th scope="col">"Log"</th> })}
                                </tr>
                            </thead>
                            <tbody>
                                {(0..7).map(|idx| {
                                    let d = start.add_days(idx);
                                    let times = compute_day_times(d, &s.location, &s.calculation);
                                    let prev_times = prev_s.as_ref().map(|p| compute_day_times(d, &p.location, &p.calculation));
                                    let iso = d.format_iso();
                                    let d_log = logs.with(|m| m.get(&(d.year as u32)).and_then(|yl| yl.days.get(&iso)).copied()).unwrap_or_default();
                                    let is_today = d == win_today;
                                    let is_selected = d == sel;
                                    let is_jumuah = d.weekday() == 5;
                                    let hijri = civil_to_hijri(d, s.display.hijri_offset);
                                    let mut row_cls = String::new();
                                    if is_today {
                                        row_cls.push_str("is-today ");
                                    }
                                    if is_selected {
                                        row_cls.push_str("is-selected ");
                                    }
                                    if is_jumuah {
                                        row_cls.push_str("is-jumuah");
                                    }
                                    let day_title = if is_today {
                                        "Today".to_owned()
                                    } else {
                                        format!("{} {} {}", d.short_weekday_name(), d.short_month_name(), d.day)
                                    };
                                    let day_sub = if is_jumuah {
                                        if s.display.show_hijri {
                                            format!("Jumu\u{2019}ah \u{00b7} {}", hijri.format_short())
                                        } else {
                                            "Jumu\u{2019}ah".to_owned()
                                        }
                                    } else if s.display.show_hijri {
                                        hijri.format_short()
                                    } else {
                                        format!("{} {}", d.short_month_name(), d.day)
                                    };

                                    view! {
                                        <tr
                                            class=row_cls
                                            on:click=move |_| { on_select_day.try_run(d); }
                                        >
                                            <th scope="row">
                                                <button
                                                    type="button"
                                                    class="prayer-day"
                                                    data-date=iso
                                                    aria-pressed=is_selected.to_string()
                                                    on:click=move |ev| {
                                                        ev.stop_propagation();
                                                        on_select_day.try_run(d);
                                                    }
                                                >
                                                    <strong>{day_title}</strong>
                                                    <span class="prayer-day-sub">{day_sub}</span>
                                                </button>
                                            </th>
                                            {ALL_PRAYERS.into_iter().filter(|p| s.display.show_sunrise || *p != PrayerKey::Sunrise).map(|p| {
                                                let ms = times.get(p);
                                                let txt = ms.map(|m| format_time_in_tz(m, tz, use_24h)).unwrap_or_else(|| "\u{2014}".into());
                                                let is_next = d == win_next_date && win_next == p;
                                                let is_changed = prev_times.as_ref().is_some_and(|pt| minutes_delta(ms, pt.get(p)) != 0);
                                                let mut td_cls = String::new();
                                                if !p.is_obligatory() {
                                                    td_cls.push_str("is-solar ");
                                                }
                                                if is_next {
                                                    td_cls.push_str("is-next ");
                                                }
                                                if is_changed {
                                                    td_cls.push_str("is-changed");
                                                }
                                                let st = d_log.get(p);
                                                view! {
                                                    <td class=td_cls data-label=p.label()>
                                                        {(s.display.show_tracking && p.is_obligatory() && st != PrayerStatus::Unprayed).then(|| view! {
                                                            <span class=format!("prayers-cell-dot is-{}", st.as_str()) aria-hidden="true"></span>
                                                        })}
                                                        <time>{txt}</time>
                                                    </td>
                                                }
                                            }).collect_view()}
                                            {s.display.show_tracking.then(|| view! {
                                                <td data-label="Log">
                                                    <span class="prayers-pip-row" aria-label=format!("{}/5 logged", d_log.counts().0 + d_log.counts().1)>
                                                        {OBLIGATORY_PRAYERS.into_iter().map(|p| {
                                                            view! { <span class=format!("prayers-pip-sm is-{}", d_log.get(p).as_str())></span> }
                                                        }).collect_view()}
                                                    </span>
                                                </td>
                                            })}
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                </div>
            }
        }}
    }
}

#[component]
pub fn WeekDayInspector(
    settings: Signal<PrayerSettings>,
    logs: RwSignal<BTreeMap<u32, YearLog>>,
    now_ms: Signal<f64>,
    selected_date: RwSignal<CivilDate>,
    mobile_details_open: RwSignal<bool>,
    on_set_status: Callback<(CivilDate, PrayerKey, PrayerStatus)>,
    on_copy_day: Callback<CivilDate>,
    on_export_day_ics: Callback<CivilDate>,
    on_open_settings: Callback<()>,
    on_close_desktop: Callback<()>,
) -> impl IntoView {
    let back_btn: NodeRef<html::Button> = NodeRef::new();
    let was_open = StoredValue::new(false);

    let win_key = Memo::new(move |_| {
        let s = settings.get();
        let w = compute_current_window(now_ms.get(), &s.location, &s.calculation);
        (w.today, w.next_prayer, w.next_date, w.next_time_ms)
    });

    Effect::new(move |_| {
        let is_open = mobile_details_open.get();
        if is_open && !was_open.get_value() {
            if let Some(btn) = back_btn.get() {
                let _ = btn.focus();
            }
        } else if !is_open && was_open.get_value() {
            let iso = selected_date.get_untracked().format_iso();
            if let Ok(Some(el)) = document().query_selector(&format!(".prayer-day[data-date=\"{iso}\"]"))
                && let Ok(btn) = el.dyn_into::<web_sys::HtmlElement>()
            {
                let _ = btn.focus();
            }
        }
        was_open.set_value(is_open);
    });

    let header_text = Memo::new(move |_| {
        let s = settings.get();
        let d = selected_date.get();
        let (_, win_next, _, win_next_ms) = win_key.get();
        let use_24h = s.display.time_format == "24h";
        let next_str = format_time_in_tz(win_next_ms, &s.location.timezone, use_24h);
        (
            format!("{}, {} {}", d.short_weekday_name(), d.short_month_name(), d.day),
            format!("Next: {} at {next_str}", win_next.label()),
        )
    });

    view! {
        <header class="prayers-sheet-head">
            <button
                node_ref=back_btn
                type="button"
                class="prayers-pill-btn prayers-back-btn"
                on:click=move |_| mobile_details_open.set(false)
            >
                <Icon name="back"/><span>"Back to week"</span>
            </button>
            <div class="prayers-head-title">
                <h2>{move || header_text.get().0}</h2>
                <div class="prayers-head-sub">{move || header_text.get().1}</div>
            </div>
            <button
                type="button"
                class="icon-button prayers-close-desktop"
                aria-label="Close day details"
                on:click=move |_| { on_close_desktop.try_run(()); }
            >
                <Icon name="close"/>
            </button>
        </header>

        {move || {
            let s = settings.get();
            let d = selected_date.get();
            let (win_today, win_next, win_next_date, _) = win_key.get();
            let times = compute_day_times(d, &s.location, &s.calculation);
            let use_24h = s.display.time_format == "24h";
            let tz = &s.location.timezone;
            let iso = d.format_iso();
            let d_log = logs.with(|m| m.get(&(d.year as u32)).and_then(|yl| yl.days.get(&iso)).copied()).unwrap_or_default();
            let hijri = civil_to_hijri(d, s.display.hijri_offset);
            let (bearing, distance_km, wind) = qibla_bearing_and_distance(s.location.latitude, s.location.longitude);

            view! {
                <div class="prayers-right-body">
                    <section class="prayers-ctx-card" aria-label="Prayer timetable">
                        <div class="prayers-ctx-head">
                            <h3>{if d == win_today { "Today\u{2019}s prayers" } else { "Day timetable" }}</h3>
                            {s.display.show_hijri.then(|| view! {
                                <span class="prayers-ctx-stat">{hijri.format_short()}</span>
                            })}
                        </div>
                        <div>
                            {ALL_PRAYERS.into_iter().filter(|p| s.display.show_sunrise || *p != PrayerKey::Sunrise).map(|p| {
                                let formatted = times
                                    .get(p)
                                    .map(|m| format_time_in_tz(m, tz, use_24h))
                                    .unwrap_or_else(|| "\u{2014}".into());
                                let is_next = d == win_next_date && win_next == p;
                                let st = d_log.get(p);
                                let row_cls = if is_next {
                                    "prayers-insp-row prayer-time-row is-next"
                                } else {
                                    "prayers-insp-row prayer-time-row"
                                };
                                view! {
                                    <div class=row_cls>
                                        <div class="prayers-insp-lbl">
                                            <strong>{p.label()}</strong>
                                            <time class="prayers-num">{formatted}</time>
                                        </div>
                                        {(s.display.show_tracking && p.is_obligatory()).then(|| view! {
                                            <div class="prayers-tri-seg" role="group" aria-label=format!("{} status", p.label())>
                                                {[
                                                    (PrayerStatus::Unprayed, "act-unprayed"),
                                                    (PrayerStatus::OnTime, "act-on_time"),
                                                    (PrayerStatus::Late, "act-late"),
                                                ].into_iter().map(|(opt, act_cls)| {
                                                    let active = st == opt;
                                                    view! {
                                                        <button
                                                            type="button"
                                                            class=if active { act_cls } else { "" }
                                                            data-prayer=p.as_str()
                                                            data-status=opt.as_str()
                                                            aria-pressed=active.to_string()
                                                            aria-label=format!("Mark {} {}", p.label(), opt.label().to_lowercase())
                                                            title=opt.label()
                                                            on:click=move |_| { on_set_status.try_run((d, p, opt)); }
                                                        >
                                                            <StatusIcon status=opt/>
                                                        </button>
                                                    }
                                                }).collect_view()}
                                            </div>
                                        })}
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                        {s.display.show_tracking.then(|| view! { <StatusLegend/> })}
                    </section>

                    <div class="prayers-insp-actions">
                        <button
                            type="button"
                            class="prayers-pill-btn"
                            on:click=move |_| { on_copy_day.try_run(d); }
                        >
                            <Icon name="copy"/><span>"Copy times"</span>
                        </button>
                        <button
                            type="button"
                            class="prayers-pill-btn"
                            on:click=move |_| { on_export_day_ics.try_run(d); }
                        >
                            <Icon name="download"/><span>"Export .ics"</span>
                        </button>
                    </div>

                    <button
                        type="button"
                        class="secondary-button full"
                        on:click=move |_| { on_open_settings.try_run(()); }
                    >
                        "Edit prayer settings"
                    </button>

                    {s.display.show_sunna.then(|| {
                        let mid_s = times.midnight_ms.map(|m| format_time_in_tz(m, tz, use_24h)).unwrap_or_else(|| "\u{2014}".into());
                        let last_third_s = times.last_third_ms.map(|m| format_time_in_tz(m, tz, use_24h)).unwrap_or_else(|| "\u{2014}".into());
                        view! {
                            <section class="prayers-ctx-card" aria-label="Night windows">
                                <div class="prayers-ctx-head"><h3>"Night & Qibla"</h3></div>
                                <div class="prayers-kv-list">
                                    <div class="prayers-kv-row"><span class="prayers-kv-k">"Islamic midnight"</span><span class="prayers-kv-v prayers-num">{mid_s}</span></div>
                                    <div class="prayers-kv-row"><span class="prayers-kv-k">"Last third of night"</span><span class="prayers-kv-v prayers-num">{last_third_s}</span></div>
                                    <div class="prayers-kv-row"><span class="prayers-kv-k">"Qibla"</span><span class="prayers-kv-v prayers-num">{format!("{bearing:.0}\u{00b0} {wind} \u{00b7} {distance_km:.0} km")}</span></div>
                                </div>
                            </section>
                        }
                    })}
                </div>
            }
        }}
    }
}
