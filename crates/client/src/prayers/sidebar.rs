//! Prayer Times left sidebar: "+ Add location", primary views, saved locations,
//! interactive MiniMonth calendar with prayer-log completion dots, and app settings link.

use std::collections::BTreeMap;

use leptos::prelude::*;

use crate::icons::Icon;
use crate::prayers::calc::{civil_to_hijri, qibla_bearing_and_distance};
use crate::prayers::model::{DayLog, Location, PrayerSettings, PrayersView, YearLog};
use crate::prayers::tz::{CivilDate, days_in_month, month_name};
use crate::settings::AppSettingsButton;

#[component]
pub fn PrayersSidebar(
    view: RwSignal<PrayersView>,
    settings: Signal<Option<PrayerSettings>>,
    logs: RwSignal<BTreeMap<u32, YearLog>>,
    today: Signal<CivilDate>,
    selected_date: RwSignal<CivilDate>,
    mini_month: RwSignal<(i32, u8)>,
    on_choose_view: Callback<PrayersView>,
    on_select_date: Callback<CivilDate>,
    on_switch_location: Callback<Location>,
    on_add_location: Callback<()>,
    on_settings: Callback<()>,
) -> impl IntoView {
    let shift_month = move |delta: i32| {
        mini_month.update(|(y, m)| {
            let total = *y * 12 + (*m as i32 - 1) + delta;
            let ny = total.div_euclid(12);
            let nm = (total.rem_euclid(12) + 1) as u8;
            *y = ny;
            *m = nm;
        });
    };

    view! {
        <aside class="sidebar prayers-sidebar" aria-label="Prayer Times navigation">
            <button
                type="button"
                class="new-button ui-new"
                on:click=move |_| { on_add_location.try_run(()); }
            >
                <Icon name="plus"/>"Add location"
            </button>

            <div class="prayers-sidebar-scroll">
                <nav aria-label="Prayer views">
                    {[PrayersView::Today, PrayersView::Week, PrayersView::Month, PrayersView::Qibla]
                        .into_iter()
                        .map(|item| {
                            view! {
                                <button
                                    type="button"
                                    class=move || if view.get() == item { "nav-item active" } else { "nav-item" }
                                    aria-current=move || (view.get() == item).then_some("page")
                                    on:click=move |_| { on_choose_view.try_run(item); }
                                >
                                    <Icon name=item.icon_name()/>
                                    <span class="prayers-nav-lbl">{item.label()}</span>
                                    <span class="prayers-nav-meta prayers-num" aria-hidden="true">{move || {
                                        let Some(s) = settings.get() else { return String::new(); };
                                        match item {
                                            PrayersView::Today => {
                                                if !s.display.show_tracking {
                                                    return String::new();
                                                }
                                                let t = today.get();
                                                let iso = t.format_iso();
                                                let day_log = logs.with(|m| {
                                                    m.get(&(t.year as u32))
                                                        .and_then(|yl| yl.days.get(&iso))
                                                        .copied()
                                                }).unwrap_or_default();
                                                let (on_t, late, _) = day_log.counts();
                                                format!("{}/5", on_t + late)
                                            }
                                            PrayersView::Week => "7d".to_owned(),
                                            PrayersView::Month => today.get().short_month_name().to_owned(),
                                            PrayersView::Qibla => {
                                                let (deg, _, wind) = qibla_bearing_and_distance(s.location.latitude, s.location.longitude);
                                                format!("{:.0}\u{00b0} {wind}", deg)
                                            }
                                        }
                                    }}</span>
                                </button>
                            }
                        })
                        .collect_view()}
                </nav>

                {move || {
                    let Some(s) = settings.get() else {
                        return ().into_any();
                    };
                    let mut list = s.locations.clone();
                    if list.is_empty() {
                        list.push(crate::prayers::model::SavedLocation {
                            id: "active".into(),
                            tag: String::new(),
                            location: s.location.clone(),
                        });
                    }
                    let active_label = s.location.label.clone();
                    let active_lat = s.location.latitude;
                    let active_lon = s.location.longitude;
                    view! {
                        <div>
                            <div class="prayers-side-head"><span>"Locations"</span></div>
                            <nav aria-label="Saved locations">
                                {list.into_iter().map(|entry| {
                                    let is_active = entry.location.label.eq_ignore_ascii_case(&active_label)
                                        && (entry.location.latitude - active_lat).abs() < 0.01
                                        && (entry.location.longitude - active_lon).abs() < 0.01;
                                    let target_loc = entry.location.clone();
                                    let meta = if entry.tag.is_empty() {
                                        entry.location.timezone.rsplit('/').next().unwrap_or("").replace('_', " ")
                                    } else {
                                        entry.tag.clone()
                                    };
                                    view! {
                                        <button
                                            type="button"
                                            class=if is_active { "nav-item active" } else { "nav-item" }
                                            aria-current=is_active.then_some("location")
                                            on:click=move |_| { on_switch_location.try_run(target_loc.clone()); }
                                        >
                                            <span class="prayers-loc-dot" aria-hidden="true"></span>
                                            <span class="prayers-nav-lbl">{entry.location.label}</span>
                                            <span class="prayers-nav-meta">{meta}</span>
                                        </button>
                                    }
                                }).collect_view()}
                            </nav>
                        </div>
                    }.into_any()
                }}

                {move || {
                    let Some(s) = settings.get() else {
                        return ().into_any();
                    };
                    let (my, mm) = mini_month.get();
                    let t = today.get();
                    let sel = selected_date.get();
                    let first = CivilDate::from_ymd(my, mm, 1).unwrap_or(t);
                    let leading = first.weekday() as usize;
                    let total_days = days_in_month(my, mm);
                    let show_hijri = s.display.show_hijri;
                    let show_tracking = s.display.show_tracking;
                    let hijri_mid = civil_to_hijri(
                        CivilDate::from_ymd(my, mm, 15.min(total_days)).unwrap_or(first),
                        s.display.hijri_offset,
                    );
                    let year_days = logs.with(|m| m.get(&(my as u32)).map(|y| y.days.clone())).unwrap_or_default();

                    view! {
                        <section class="prayers-mini-month" aria-label="Calendar jump">
                            <div class="prayers-mm-head">
                                <div class="prayers-mm-title">
                                    <strong>{format!("{} {}", month_name(mm), my)}</strong>
                                    {show_hijri.then(|| view! {
                                        <small>{format!("{} {}", hijri_mid.month_name(), hijri_mid.year)}</small>
                                    })}
                                </div>
                                <div class="prayers-mm-nav">
                                    <button
                                        type="button"
                                        class="icon-button"
                                        data-keep-drawer=""
                                        aria-label="Previous month"
                                        on:click=move |_| shift_month(-1)
                                    >
                                        <Icon name="back"/>
                                    </button>
                                    <button
                                        type="button"
                                        class="icon-button is-next-arrow"
                                        data-keep-drawer=""
                                        aria-label="Next month"
                                        on:click=move |_| shift_month(1)
                                    >
                                        <Icon name="chevron"/>
                                    </button>
                                </div>
                            </div>
                            <div class="prayers-mm-grid">
                                {["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"]
                                    .into_iter()
                                    .map(|dow| view! { <span class="prayers-mm-dow">{dow}</span> })
                                    .collect_view()}
                                {(0..leading)
                                    .map(|_| view! { <span></span> })
                                    .collect_view()}
                                {(1..=total_days).map(|day_num| {
                                    let d = CivilDate::from_ymd(my, mm, day_num).unwrap_or(t);
                                    let is_today = d == t;
                                    let is_sel = d == sel;
                                    let day_log = year_days.get(&d.format_iso()).copied().unwrap_or_default();
                                    let dot_cls = log_dot_class(day_log);
                                    let cls = format!(
                                        "prayers-mm-day{}{}",
                                        if is_today { " is-today" } else { "" },
                                        if is_sel { " is-sel" } else { "" },
                                    );
                                    view! {
                                        <button
                                            type="button"
                                            class=cls
                                            aria-label=format!("{} {} {}", d.month_name(), d.day, d.year)
                                            on:click=move |_| { on_select_date.try_run(d); }
                                        >
                                            <span>{day_num}</span>
                                            {(show_tracking && !dot_cls.is_empty()).then(|| view! {
                                                <span class=format!("prayers-mm-dot {dot_cls}") aria-hidden="true"></span>
                                            })}
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        </section>
                    }.into_any()
                }}
            </div>

            <AppSettingsButton app="prayers" on_click=on_settings/>
        </aside>
    }
}

fn log_dot_class(day_log: DayLog) -> &'static str {
    let (on_t, late, _) = day_log.counts();
    if on_t == 5 {
        "is-ok"
    } else if on_t + late > 0 {
        "is-mix"
    } else {
        ""
    }
}
