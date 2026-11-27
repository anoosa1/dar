//! Month view: full monthly prayer timetable with Print, .ics, .csv, and JSON backup exports.

use std::collections::BTreeMap;

use leptos::prelude::*;

use crate::icons::Icon;
use crate::prayers::calc::{civil_to_hijri, compute_current_window, compute_day_times};
use crate::prayers::model::{
    ALL_PRAYERS, OBLIGATORY_PRAYERS, PrayerKey, PrayerSettings, PrayerStatus, YearLog,
    method_short_label,
};
use crate::prayers::tz::{CivilDate, days_in_month, format_time_in_tz, month_name, trigger_print};

#[component]
pub fn MonthMain(
    settings: Signal<PrayerSettings>,
    logs: RwSignal<BTreeMap<u32, YearLog>>,
    now_ms: Signal<f64>,
    mini_month: RwSignal<(i32, u8)>,
    on_select_day: Callback<CivilDate>,
    on_export_month_ics: Callback<(i32, u8)>,
    on_export_month_csv: Callback<(i32, u8)>,
    on_export_json: Callback<()>,
) -> impl IntoView {
    let shift_month = move |delta: i32| {
        mini_month.update(|(y, m)| {
            let total = *y * 12 + (*m as i32 - 1) + delta;
            *y = total.div_euclid(12);
            *m = (total.rem_euclid(12) + 1) as u8;
        });
    };

    let win_key = Memo::new(move |_| {
        let s = settings.get();
        let w = compute_current_window(now_ms.get(), &s.location, &s.calculation);
        (w.today, w.next_prayer, w.next_date)
    });

    view! {
        {move || {
            let s = settings.get();
            let (win_today, win_next, win_next_date) = win_key.get();
            let (year, month) = mini_month.get();
            let count = days_in_month(year, month);
            let use_24h = s.display.time_format == "24h";
            let tz = &s.location.timezone;
            let first = CivilDate::from_ymd(year, month, 1).unwrap_or(win_today);
            let last = CivilDate::from_ymd(year, month, count).unwrap_or(win_today);
            let h1 = civil_to_hijri(first, s.display.hijri_offset);
            let h2 = civil_to_hijri(last, s.display.hijri_offset);
            let hijri_sub = if h1.month == h2.month {
                format!("{} {} AH", h1.month_name(), h1.year)
            } else {
                format!("{} \u{2013} {} {} AH", h1.short_month_name(), h2.short_month_name(), h2.year)
            };
            let year_days = logs.with(|m| m.get(&(year as u32)).map(|yl| yl.days.clone())).unwrap_or_default();

            view! {
                <header class="prayers-sheet-head">
                    <div class="prayers-head-title">
                        <h1>{format!("{} {}", month_name(month), year)}</h1>
                        <div class="prayers-head-sub">{format!(
                            "{} \u{00b7} {}{}",
                            s.location.label,
                            method_short_label(&s.calculation.method),
                            if s.display.show_hijri { format!(" \u{00b7} {hijri_sub}") } else { String::new() }
                        )}</div>
                    </div>
                    <div class="prayers-head-actions">
                        <button type="button" class="icon-button" aria-label="Previous month" on:click=move |_| shift_month(-1)>
                            <Icon name="back"/>
                        </button>
                        <button
                            type="button"
                            class="prayers-pill-btn"
                            on:click=move |_| mini_month.set((win_today.year, win_today.month))
                        >
                            "This month"
                        </button>
                        <button type="button" class="icon-button is-next-arrow" aria-label="Next month" on:click=move |_| shift_month(1)>
                            <Icon name="chevron"/>
                        </button>
                        <button type="button" class="prayers-pill-btn" on:click=move |_| trigger_print()>
                            <Icon name="print"/><span>"Print"</span>
                        </button>
                        <button type="button" class="prayers-pill-btn" on:click=move |_| { on_export_month_ics.try_run((year, month)); }>
                            <Icon name="download"/><span>".ics"</span>
                        </button>
                        <button type="button" class="prayers-pill-btn" on:click=move |_| { on_export_month_csv.try_run((year, month)); }>
                            <span>".csv"</span>
                        </button>
                        <button type="button" class="prayers-pill-btn" on:click=move |_| { on_export_json.try_run(()); }>
                            <span>"JSON"</span>
                        </button>
                    </div>
                </header>

                <div class="prayers-sheet-body">
                    <div class="prayers-table-wrap">
                        <table class="prayer-week prayers-month-table" aria-label="Monthly prayer timetable">
                            <thead>
                                <tr>
                                    <th scope="col">"Date"</th>
                                    {s.display.show_hijri.then(|| view! { <th scope="col">"Hijri"</th> })}
                                    {ALL_PRAYERS.into_iter().filter(|p| s.display.show_sunrise || *p != PrayerKey::Sunrise).map(|p| {
                                        view! { <th scope="col">{p.label()}</th> }
                                    }).collect_view()}
                                    {s.display.show_tracking.then(|| view! { <th scope="col">"Log"</th> })}
                                </tr>
                            </thead>
                            <tbody>
                                {(1..=count).map(|day_num| {
                                    let d = CivilDate::from_ymd(year, month, day_num).unwrap_or(win_today);
                                    let times = compute_day_times(d, &s.location, &s.calculation);
                                    let iso = d.format_iso();
                                    let d_log = year_days.get(&iso).copied().unwrap_or_default();
                                    let is_today = d == win_today;
                                    let is_jumuah = d.weekday() == 5;
                                    let hijri = civil_to_hijri(d, s.display.hijri_offset);
                                    let mut row_cls = String::new();
                                    if is_today {
                                        row_cls.push_str("is-today ");
                                    }
                                    if is_jumuah {
                                        row_cls.push_str("is-jumuah");
                                    }
                                    view! {
                                        <tr class=row_cls on:click=move |_| { on_select_day.try_run(d); }>
                                            <th scope="row">
                                                <button
                                                    type="button"
                                                    class="prayer-day"
                                                    data-date=iso
                                                    on:click=move |ev| {
                                                        ev.stop_propagation();
                                                        on_select_day.try_run(d);
                                                    }
                                                >
                                                    <strong>{format!("{} {} {}", d.short_weekday_name(), d.short_month_name(), d.day)}</strong>
                                                </button>
                                            </th>
                                            {s.display.show_hijri.then(|| view! {
                                                <td data-label="Hijri">{hijri.format_short()}</td>
                                            })}
                                            {ALL_PRAYERS.into_iter().filter(|p| s.display.show_sunrise || *p != PrayerKey::Sunrise).map(|p| {
                                                let txt = times.get(p).map(|m| format_time_in_tz(m, tz, use_24h)).unwrap_or_else(|| "\u{2014}".into());
                                                let st = d_log.get(p);
                                                let mut cls = String::new();
                                                if !p.is_obligatory() {
                                                    cls.push_str("is-solar ");
                                                }
                                                if d == win_next_date && win_next == p {
                                                    cls.push_str("is-next");
                                                }
                                                view! {
                                                    <td class=cls data-label=p.label()>
                                                        {(s.display.show_tracking && p.is_obligatory() && st != PrayerStatus::Unprayed).then(|| view! {
                                                            <span class=format!("prayers-cell-dot is-{}", st.as_str()) aria-hidden="true"></span>
                                                        })}
                                                        <time>{txt}</time>
                                                    </td>
                                                }
                                            }).collect_view()}
                                            {s.display.show_tracking.then(|| view! {
                                                <td data-label="Log">
                                                    <span class="prayers-pip-row">
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
