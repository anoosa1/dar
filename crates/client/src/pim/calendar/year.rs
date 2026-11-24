//! Year overview: 12 months with Twilight density heat shading; clicking a month opens
//! Month view and clicking any day jumps straight to Day view.

use std::collections::HashMap;

use leptos::prelude::*;
use serde_json::Value;

use super::model::{day, event_day_bounds, weekday};
use crate::date_popover::add_days;

#[component]
pub fn YearView(
    anchor: RwSignal<String>,
    today: Memo<String>,
    week_start: Signal<u32>,
    events: RwSignal<Vec<Value>>,
    zone: Memo<String>,
    on_pick_day: Callback<String>,
    on_pick_month: Callback<String>,
) -> impl IntoView {
    let year_num = Memo::new(move |_| anchor.get()[..4].to_owned());
    let density = Memo::new(move |_| {
        let z = zone.get();
        let y = year_num.get();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for ev in events.get() {
            let (from, to) = event_day_bounds(&ev, &z);
            let mut cur = if from[..4] < y[..] {
                format!("{y}-01-01")
            } else {
                from
            };
            let end = if to[..4] > y[..] {
                format!("{y}-12-31")
            } else {
                to
            };
            let mut guard = 0;
            while cur <= end && guard < 366 {
                *counts.entry(cur.clone()).or_insert(0) += 1;
                cur = add_days(&cur, 1);
                guard += 1;
            }
        }
        counts
    });

    view! {
        <div class="calendar-year" aria-label="Year calendar">
            {move || {
                let y = year_num.get();
                let ws = week_start.get();
                let today_now = today.get();
                let anchor_month = anchor.get()[..7].to_owned();
                let counts = density.get();
                (1..=12u32).map(|m| {
                    let month_prefix = format!("{y}-{m:02}");
                    let first = format!("{month_prefix}-01");
                    let month_name = day(&first, &[("month", "long")]);
                    let offset = (weekday(&first) + 7 - ws) % 7;
                    let is_current_month = month_prefix == anchor_month;
                    let month_click = first.clone();

                    // Build days of this month
                    let mut cells: Vec<Option<String>> = vec![None; offset as usize];
                    let mut d = first.clone();
                    while d.starts_with(&month_prefix) {
                        cells.push(Some(d.clone()));
                        d = add_days(&d, 1);
                    }
                    let weekday_headers: Vec<String> = (0..7)
                        .map(|i| {
                            let sample = add_days(&first, i - offset as i64);
                            day(&sample, &[("weekday", "narrow")])
                        })
                        .collect();

                    view! {
                        <section class="calendar-year-month">
                            <button
                                type="button"
                                class=if is_current_month { "calendar-year-title is-current" } else { "calendar-year-title" }
                                on:click=move |_| on_pick_month.run(month_click.clone())
                            >
                                {month_name}
                            </button>
                            <div class="calendar-year-grid">
                                {weekday_headers.into_iter().map(|w| view! { <span class="calendar-year-wd">{w}</span> }).collect_view()}
                                {cells.into_iter().map(|cell| match cell {
                                    None => view! { <span class="calendar-year-blank"></span> }.into_any(),
                                    Some(date) => {
                                        let n = counts.get(&date).copied().unwrap_or(0);
                                        let heat = match n {
                                            0 => "",
                                            1 => " heat-1",
                                            2..=3 => " heat-2",
                                            _ => " heat-3",
                                        };
                                        let is_today = date == today_now;
                                        let cls = format!(
                                            "calendar-year-day{heat}{}",
                                            if is_today { " is-today" } else { "" }
                                        );
                                        let label = if n == 0 {
                                            day(&date, &[("month", "long"), ("day", "numeric"), ("year", "numeric")])
                                        } else {
                                            format!(
                                                "{} ({n} event{})",
                                                day(&date, &[("month", "long"), ("day", "numeric"), ("year", "numeric")]),
                                                if n == 1 { "" } else { "s" }
                                            )
                                        };
                                        let pick = date.clone();
                                        view! {
                                            <button
                                                type="button"
                                                class=cls
                                                aria-label=label.clone()
                                                title=label
                                                on:click=move |_| on_pick_day.run(pick.clone())
                                            >
                                                {date[8..].trim_start_matches('0').to_owned()}
                                            </button>
                                        }.into_any()
                                    }
                                }).collect_view()}
                            </div>
                        </section>
                    }
                }).collect_view()
            }}
        </div>
    }
}
