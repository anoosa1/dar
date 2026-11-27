//! Today view: Solar Horizon Arc + morphing icon-only daily prayer tracker +
//! Daily log & Sun right sheet.

use std::collections::BTreeMap;

use leptos::prelude::*;

use crate::icons::Icon;
use crate::prayers::calc::{
    civil_to_hijri, compute_current_window, compute_day_times, qibla_bearing_and_distance,
    sun_arc_xy,
};
use crate::prayers::model::{
    ALL_PRAYERS, OBLIGATORY_PRAYERS, PrayerKey, PrayerSettings, PrayerStatus, YearLog,
    method_short_label,
};
use crate::prayers::tz::{
    CivilDate, format_countdown, format_duration_minutes, format_time_in_tz, local_minutes_of_day,
};

/// Morphing icon representing Unprayed / Prayed on time / Prayed late (zero visible text).
#[component]
pub fn StatusIcon(status: PrayerStatus) -> impl IntoView {
    match status {
        PrayerStatus::Unprayed => view! {
            <svg class="icon" viewBox="0 0 24 24" aria-hidden="true">
                <circle
                    cx="12"
                    cy="12"
                    r="8.5"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.8"
                    stroke-dasharray="3.2 2.4"
                />
            </svg>
        }
        .into_any(),
        PrayerStatus::OnTime => view! {
            <svg class="icon" viewBox="0 0 24 24" aria-hidden="true">
                <circle
                    cx="12"
                    cy="12"
                    r="9"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.9"
                />
                <path
                    d="M8.2 12.2l2.6 2.6 5.2-5.4"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2.1"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                />
            </svg>
        }
        .into_any(),
        PrayerStatus::Late => view! {
            <svg class="icon" viewBox="0 0 24 24" aria-hidden="true">
                <circle
                    cx="12"
                    cy="12"
                    r="9"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.9"
                />
                <path
                    d="M12 7.5v4.8l3.1 1.8"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                />
            </svg>
        }
        .into_any(),
    }
}

#[component]
pub fn StatusLegend() -> impl IntoView {
    view! {
        <div class="prayers-ico-legend" aria-label="Status icon key">
            <span><i class="is-ok"><StatusIcon status=PrayerStatus::OnTime/></i>"On time"</span>
            <span><i class="is-late"><StatusIcon status=PrayerStatus::Late/></i>"Late"</span>
            <span><i class="is-none"><StatusIcon status=PrayerStatus::Unprayed/></i>"Unprayed"</span>
        </div>
    }
}

#[component]
pub fn TodayMain(
    settings: Signal<PrayerSettings>,
    logs: RwSignal<BTreeMap<u32, YearLog>>,
    now_ms: Signal<f64>,
    on_cycle_prayer: Callback<(CivilDate, PrayerKey)>,
    on_copy_day: Callback<CivilDate>,
    on_open_week: Callback<CivilDate>,
) -> impl IntoView {
    // Coarsened structural snapshot so 1-second timer ticks only update countdown text & SVG sun position,
    // never recreating row DOM buttons under focus/click.
    let model = Memo::new(move |_| {
        let s = settings.get();
        let now = now_ms.get();
        let win = compute_current_window(now, &s.location, &s.calculation);
        let day_times = compute_day_times(win.today, &s.location, &s.calculation);
        let tom_times = compute_day_times(win.today.add_days(1), &s.location, &s.calculation);
        let iso = win.today.format_iso();
        let day_log = logs
            .with(|m| {
                m.get(&(win.today.year as u32))
                    .and_then(|yl| yl.days.get(&iso))
                    .copied()
            })
            .unwrap_or_default();
        (
            s,
            win.today,
            win.current_prayer,
            win.next_prayer,
            win.next_time_ms,
            win.is_tomorrow_fajr,
            day_times,
            tom_times,
            day_log,
        )
    });

    view! {
        {move || {
            let (s, today, current_prayer, next_prayer, next_time_ms, is_tomorrow_fajr, day_times, tom_times, day_log) = model.get();
            let use_24h = s.display.time_format == "24h";
            let tz = s.location.timezone.clone();
            let next_str = format_time_in_tz(next_time_ms, &tz, use_24h);
            let hijri = civil_to_hijri(today, s.display.hijri_offset);
            let (bearing, _, wind) = qibla_bearing_and_distance(s.location.latitude, s.location.longitude);

            let sr_ms = day_times.get(PrayerKey::Sunrise);
            let ss_ms = day_times.get(PrayerKey::Maghrib);

            // Compute SVG path across 0..24h (viewBox 0 0 560 88, horizon y = 64).
            let day_base_ms = crate::prayers::tz::epoch_ms_for_civil_minutes(today, 0.0, &tz);
            let mut path_d = String::new();
            for step in 0..=48 {
                let frac = step as f64 / 48.0;
                let t_ms = day_base_ms + frac * 86_400_000.0;
                let (xf, yn) = sun_arc_xy(t_ms, &tz, sr_ms, ss_ms);
                let sx = 14.0 + xf * 532.0;
                let sy = 64.0 - yn * 46.0;
                if step == 0 {
                    path_d.push_str(&format!("M{sx:.1} {sy:.1}"));
                } else {
                    path_d.push_str(&format!(" L{sx:.1} {sy:.1}"));
                }
            }
            let tz_sun = tz.clone();
            let sun_pos = Signal::derive(move || {
                let (xf, yn) = sun_arc_xy(now_ms.get(), &tz_sun, sr_ms, ss_ms);
                (format!("{:.1}", 14.0 + xf * 532.0), format!("{:.1}", 64.0 - yn * 46.0))
            });
            let s_loc = s.location.clone();
            let s_calc = s.calculation.clone();
            let live_cd = Signal::derive(move || {
                let win = compute_current_window(now_ms.get(), &s_loc, &s_calc);
                (
                    format_countdown(win.remaining_ms),
                    format!("{:.0}", (win.progress * 100.0).round()),
                )
            });

            let head_sub = if s.display.show_hijri {
                format!(
                    "{} \u{00b7} {} \u{00b7} {}, {} {} \u{00b7} {}",
                    s.location.label,
                    method_short_label(&s.calculation.method),
                    today.short_weekday_name(),
                    today.short_month_name(),
                    today.day,
                    hijri.format_full()
                )
            } else {
                format!(
                    "{} \u{00b7} {} \u{00b7} {}, {} {} {}",
                    s.location.label,
                    method_short_label(&s.calculation.method),
                    today.short_weekday_name(),
                    today.short_month_name(),
                    today.day,
                    today.year
                )
            };

            let tom_fajr_str = tom_times
                .get(PrayerKey::Fajr)
                .map(|ms| format_time_in_tz(ms, &tz, use_24h))
                .unwrap_or_else(|| "\u{2014}".into());
            let (on_t, late, _) = day_log.counts();
            let logged_count = on_t + late;

            view! {
                <header class="prayers-sheet-head">
                    <div class="prayers-head-title">
                        <h1>{format!("{} at {}", next_prayer.label(), next_str)}</h1>
                        <div class="prayers-head-sub">{head_sub}</div>
                    </div>
                    <div class="prayers-head-actions">
                        <button
                            type="button"
                            class="prayers-pill-btn"
                            on:click=move |_| { on_copy_day.try_run(today); }
                        >
                            <Icon name="copy"/><span>"Copy times"</span>
                        </button>
                    </div>
                </header>

                <div class="prayers-sheet-body">
                    <section class="prayers-solar-stage" aria-label="Solar horizon timeline">
                        <div class="prayers-hero-top">
                            <div>
                                <div class="prayers-phase-pill">
                                    <span class="prayers-phase-dot" aria-hidden="true"></span>
                                    <span>{format!("Now \u{00b7} {}", current_prayer.label())}</span>
                                </div>
                                <p class="prayers-hero-title">{format!("Next: {} at {}", next_prayer.label(), next_str)}</p>
                                <p class="prayers-hero-note">{format!("{} \u{00b7} Qibla {:.0}\u{00b0} {}", s.location.label, bearing, wind)}</p>
                            </div>
                            {s.display.show_countdown.then(|| view! {
                                <div class="prayers-hero-cd">
                                    <small>{if is_tomorrow_fajr { "Tomorrow in" } else { "Next in" }}</small>
                                    <strong class="prayers-num">{move || live_cd.get().0}</strong>
                                    <progress class="prayers-window-bar" max="100" value=move || live_cd.get().1></progress>
                                </div>
                            })}
                        </div>

                        <svg class="prayers-arc-svg" viewBox="0 0 560 88" aria-hidden="true">
                            <line class="prayers-arc-horizon" x1="8" y1="64" x2="552" y2="64"/>
                            <text class="prayers-arc-horizon-lbl" x="12" y="60">"HORIZON"</text>
                            <path d=path_d fill="none" stroke="var(--accent)" stroke-width="1.8" stroke-opacity="0.48"/>
                            {ALL_PRAYERS.into_iter().map(|p| {
                                let Some(ms) = day_times.get(p) else {
                                    return ().into_any();
                                };
                                let (_xf, yn) = sun_arc_xy(ms, &tz, sr_ms, ss_ms);
                                let xm = local_minutes_of_day(ms, &tz) / 1440.0;
                                let px = 14.0 + xm * 532.0;
                                let py = 64.0 - yn * 46.0;
                                let is_now = current_prayer == p && !is_tomorrow_fajr;
                                let is_next = next_prayer == p && !is_tomorrow_fajr;
                                let drop_cls = format!(
                                    "prayers-arc-drop{}{}",
                                    if is_now { " is-now" } else { "" },
                                    if is_next { " is-next" } else { "" }
                                );
                                let pt_cls = format!(
                                    "prayers-arc-pt{}{}{}",
                                    if !p.is_obligatory() { " is-solar" } else { "" },
                                    if is_now { " is-now" } else { "" },
                                    if is_next { " is-next" } else { "" }
                                );
                                let r = if is_next { "4.6" } else { "3.4" };
                                view! {
                                    <g>
                                        <line class=drop_cls x1=format!("{px:.1}") y1=format!("{py:.1}") x2=format!("{px:.1}") y2="64"/>
                                        <circle class=pt_cls cx=format!("{px:.1}") cy=format!("{py:.1}") r=r/>
                                    </g>
                                }.into_any()
                            }).collect_view()}
                            <circle class="prayers-arc-sun-halo" cx=move || sun_pos.get().0 cy=move || sun_pos.get().1 r="7"/>
                            <circle class="prayers-arc-sun-core" cx=move || sun_pos.get().0 cy=move || sun_pos.get().1 r="3.4"/>
                        </svg>

                        <div class="prayers-arc-grid">
                            {ALL_PRAYERS.into_iter().map(|p| {
                                let is_now = current_prayer == p && !is_tomorrow_fajr;
                                let is_next = next_prayer == p && !is_tomorrow_fajr;
                                let cls = format!(
                                    "prayers-arc-cell{}{}",
                                    if is_now { " is-now" } else { "" },
                                    if is_next { " is-next" } else { "" }
                                );
                                let val = day_times
                                    .get(p)
                                    .map(|ms| format_time_in_tz(ms, &tz, use_24h))
                                    .unwrap_or_else(|| "\u{2014}".into());
                                let st = day_log.get(p);
                                view! {
                                    <div class=cls>
                                        <span>
                                            {p.short_label()}
                                            {(s.display.show_tracking && p.is_obligatory() && st != PrayerStatus::Unprayed).then(|| view! {
                                                <span class=format!("prayers-st-dot is-{}", st.as_str()) aria-hidden="true"></span>
                                            })}
                                        </span>
                                        <strong class="prayers-num">{val}</strong>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    </section>

                    <div class="prayers-list" role="list" aria-label="Today's prayer schedule">
                        {ALL_PRAYERS.into_iter().filter(|p| s.display.show_sunrise || *p != PrayerKey::Sunrise).map(|p| {
                            let ms = day_times.get(p);
                            let formatted = ms
                                .map(|m| format_time_in_tz(m, &tz, use_24h))
                                .unwrap_or_else(|| "\u{2014}".into());
                            let is_now = current_prayer == p && !is_tomorrow_fajr;
                            let is_next = next_prayer == p && !is_tomorrow_fajr;
                            let is_past = !is_now && !is_next && (p.index() < next_prayer.index() || is_tomorrow_fajr);
                            let st = day_log.get(p);
                            let mut cls = String::from("prayers-row");
                            if !s.display.show_tracking {
                                cls.push_str(" no-tracking");
                            }
                            if !p.is_obligatory() {
                                cls.push_str(" is-solar");
                            }
                            if is_now {
                                cls.push_str(" is-now");
                            } else if is_next {
                                cls.push_str(" is-next");
                            } else if is_past {
                                cls.push_str(" is-past");
                            }
                            let badge = if is_now {
                                Some("Now")
                            } else if is_next {
                                Some("Next")
                            } else {
                                None
                            };
                            let next_st = st.next_cycle();
                            let aria_lbl = format!(
                                "{}: {}. Click to mark {}.",
                                p.label(),
                                st.label(),
                                next_st.label().to_lowercase()
                            );
                            view! {
                                <div class=cls role="listitem">
                                    <span class="prayers-row-ico" aria-hidden="true"><Icon name=p.icon_name()/></span>
                                    <div class="prayers-row-name">
                                        <strong>
                                            {p.label()}
                                            {badge.map(|b| view! { <span class="prayers-row-badge">{b}</span> })}
                                        </strong>
                                        <small>{p.subtitle()}</small>
                                    </div>
                                    <time class="prayers-row-time prayers-num">{formatted}</time>
                                    {s.display.show_tracking.then(|| {
                                        if p.is_obligatory() {
                                            view! {
                                                <button
                                                    type="button"
                                                    class=format!("prayers-track-ico is-{}", st.as_str())
                                                    data-prayer=p.as_str()
                                                    data-status=st.as_str()
                                                    aria-label=aria_lbl
                                                    title=st.label()
                                                    on:click=move |_| { on_cycle_prayer.try_run((today, p)); }
                                                >
                                                    <StatusIcon status=st/>
                                                </button>
                                            }.into_any()
                                        } else {
                                            view! { <span class="prayers-track-spacer" aria-hidden="true"></span> }.into_any()
                                        }
                                    })}
                                </div>
                            }
                        }).collect_view()}
                    </div>

                    <div class="prayers-foot-cue">
                        <span>{format!(
                            "Tomorrow\u{2019}s Fajr at {tom_fajr_str}{}",
                            if s.display.show_tracking { format!(" \u{00b7} {logged_count}/5 logged") } else { String::new() }
                        )}</span>
                        <button
                            type="button"
                            class="prayers-text-link"
                            on:click=move |_| { on_open_week.try_run(today); }
                        >
                            "View week \u{2192}"
                        </button>
                    </div>
                </div>
            }
        }}
    }
}

#[component]
pub fn TodayRightSheet(
    settings: Signal<PrayerSettings>,
    logs: RwSignal<BTreeMap<u32, YearLog>>,
    now_ms: Signal<f64>,
    on_cycle_prayer: Callback<(CivilDate, PrayerKey)>,
    on_open_day_inspector: Callback<CivilDate>,
    on_open_qibla: Callback<()>,
) -> impl IntoView {
    view! {
        {move || {
            let s = settings.get();
            let now = now_ms.get();
            let win = compute_current_window(now, &s.location, &s.calculation);
            let today = win.today;
            let day_times = compute_day_times(today, &s.location, &s.calculation);
            let use_24h = s.display.time_format == "24h";
            let tz = &s.location.timezone;
            let iso = today.format_iso();
            let today_log = logs
                .with(|m| m.get(&(today.year as u32)).and_then(|yl| yl.days.get(&iso)).copied())
                .unwrap_or_default();
            let (on_t, late, _) = today_log.counts();
            let total_done = on_t + late;
            let (bearing, distance_km, wind) = qibla_bearing_and_distance(s.location.latitude, s.location.longitude);

            let week_days: Vec<CivilDate> = (-6..=0).map(|d| today.add_days(d)).collect();

            view! {
                <header class="prayers-sheet-head">
                    <div class="prayers-head-title">
                        <h2>"Daily log & Sun"</h2>
                        <div class="prayers-head-sub">{format!("{} \u{00b7} {} {}", today.short_weekday_name(), today.short_month_name(), today.day)}</div>
                    </div>
                </header>

                <div class="prayers-right-body">
                    {s.display.show_tracking.then(|| view! {
                        <section class="prayers-ctx-card" aria-label="Daily prayer log">
                            <div class="prayers-ctx-head">
                                <h3>"Today\u{2019}s prayers"</h3>
                                <span class="prayers-ctx-stat">{format!("{on_t} on time \u{00b7} {late} late")}</span>
                            </div>
                            <div class="prayers-log-summary">
                                <strong class="prayers-num">{format!("{total_done}/5")}</strong>
                                <span>{if total_done == 5 { "All prayers logged" } else { "Tap icon to log" }}</span>
                            </div>
                            <div class="prayers-log-strip">
                                {OBLIGATORY_PRAYERS.into_iter().map(|p| {
                                    let st = today_log.get(p);
                                    view! {
                                        <button
                                            type="button"
                                            class=format!("prayers-log-seg is-{}", st.as_str())
                                            aria-label=format!("{}: {}", p.label(), st.label())
                                            on:click=move |_| { on_cycle_prayer.try_run((today, p)); }
                                        >
                                            <span class="prayers-log-seg-bar"></span>
                                            <span>{p.short_label()}</span>
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                            <div class="prayers-wk-dots" aria-label="Last 7 days">
                                {week_days.into_iter().map(|d| {
                                    let d_iso = d.format_iso();
                                    let d_log = logs.with(|m| {
                                        m.get(&(d.year as u32)).and_then(|yl| yl.days.get(&d_iso)).copied()
                                    }).unwrap_or_default();
                                    let is_today = d == today;
                                    view! {
                                        <button
                                            type="button"
                                            class=if is_today { "prayers-wk-col is-today" } else { "prayers-wk-col" }
                                            aria-label=format!("Open {} {} {}", d.month_name(), d.day, d.year)
                                            on:click=move |_| { on_open_day_inspector.try_run(d); }
                                        >
                                            <small>{d.short_weekday_name().chars().next().unwrap_or('S')}</small>
                                            <span class="prayers-wk-pips">
                                                {OBLIGATORY_PRAYERS.into_iter().map(|p| {
                                                    let st = d_log.get(p);
                                                    view! { <span class=format!("prayers-pip is-{}", st.as_str())></span> }
                                                }).collect_view()}
                                            </span>
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                            <StatusLegend/>
                        </section>
                    })}

                    <section class="prayers-ctx-card" aria-label="Qibla summary">
                        <div class="prayers-ctx-head">
                            <h3>"Qibla direction"</h3>
                            <button type="button" class="prayers-text-link" on:click=move |_| { on_open_qibla.try_run(()); }>
                                "Open compass"
                            </button>
                        </div>
                        <div class="prayers-qibla-mini">
                            <svg class="prayers-qibla-dial-sm" viewBox="0 0 56 56" aria-hidden="true">
                                <circle cx="28" cy="28" r="25" class="prayers-qibla-ring"/>
                                <g transform=format!("rotate({bearing:.1} 28 28)")>
                                    <line x1="28" y1="28" x2="28" y2="8" class="prayers-qibla-needle"/>
                                    <polygon points="28,5 24,13 32,13" class="prayers-qibla-arrow"/>
                                </g>
                                <circle cx="28" cy="28" r="3" class="prayers-qibla-hub"/>
                            </svg>
                            <div class="prayers-qibla-meta">
                                <strong class="prayers-num">{format!("{:.1}\u{00b0} {wind}", bearing)}</strong>
                                <span>{format!("{:.0} km to Makkah", distance_km)}</span>
                                <small>{s.location.label.clone()}</small>
                            </div>
                        </div>
                    </section>

                    {s.display.show_sunna.then(|| {
                        let daylight = day_times
                            .daylight_minutes
                            .map(format_duration_minutes)
                            .unwrap_or_else(|| "\u{2014}".into());
                        let noon_s = format_time_in_tz(day_times.solar_noon_ms, tz, use_24h);
                        let mid_s = day_times
                            .midnight_ms
                            .map(|m| format_time_in_tz(m, tz, use_24h))
                            .unwrap_or_else(|| "\u{2014}".into());
                        let last_third_s = day_times
                            .last_third_ms
                            .map(|m| format_time_in_tz(m, tz, use_24h))
                            .unwrap_or_else(|| "\u{2014}".into());
                        view! {
                            <section class="prayers-ctx-card" aria-label="Sunna and night windows">
                                <div class="prayers-ctx-head"><h3>"Sunna & night"</h3></div>
                                <div class="prayers-kv-list">
                                    <div class="prayers-kv-row"><span class="prayers-kv-k">"Daylight"</span><span class="prayers-kv-v prayers-num">{daylight}</span></div>
                                    <div class="prayers-kv-row"><span class="prayers-kv-k">"Solar noon"</span><span class="prayers-kv-v prayers-num">{noon_s}</span></div>
                                    <div class="prayers-kv-row"><span class="prayers-kv-k">"Islamic midnight"</span><span class="prayers-kv-v prayers-num">{mid_s}</span></div>
                                    <div class="prayers-kv-row"><span class="prayers-kv-k">"Last third of night"</span><span class="prayers-kv-v prayers-num">{last_third_s}</span></div>
                                </div>
                            </section>
                        }
                    })}
                </div>
            }
        }}
    }
}
