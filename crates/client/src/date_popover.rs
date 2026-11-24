//! The calendar date controls (`date-popover.tsx`): a popover month picker and the sidebar
//! month, on civil dates with roving focus and arrow, Home, End and Page keys.

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::{JsCast, JsValue};

use crate::dates::local_date;
use crate::icons::Icon;
use crate::ui::{Select, unique_id};

/// `date` plus `days`, as civil dates (`add`).
pub fn add_days(date: &str, days: i64) -> String {
    let ms = js_sys::Date::parse(&format!("{date}T12:00:00Z")) + days as f64 * 86_400_000.0;
    js_sys::Date::new(&JsValue::from_f64(ms))
        .to_iso_string()
        .as_string()
        .unwrap_or_default()[..10]
        .to_owned()
}

/// A civil date formatted in UTC (`format`).
pub fn format_date(date: &str, options: &[(&str, &str)]) -> String {
    let mut all = options.to_vec();
    all.push(("timeZone", "UTC"));
    local_date(js_sys::Date::parse(&format!("{date}T12:00:00Z")), &all)
}

fn weekday(date: &str) -> u32 {
    js_sys::Date::new(&JsValue::from_f64(js_sys::Date::parse(&format!(
        "{date}T12:00:00Z"
    ))))
    .get_utc_day()
}

/// Moves a date by a view's period (`shiftCalendarDate`).
pub fn shift(anchor: &str, view: &str, direction: i64) -> String {
    let parts: Vec<u32> = anchor.split('-').filter_map(|p| p.parse().ok()).collect();
    if parts.len() != 3 {
        return anchor.to_owned();
    }
    if view == "month" {
        let total = parts[0] as i64 * 12 + parts[1] as i64 - 1 + direction;
        let (year, month) = (total.div_euclid(12), total.rem_euclid(12) + 1);
        let last = js_sys::Date::new(&JsValue::from_f64(js_sys::Date::utc(
            year as f64,
            month as f64,
        )))
        .get_time()
            - 86_400_000.0;
        let last_day = js_sys::Date::new(&JsValue::from_f64(last)).get_utc_date();
        return format!("{year:04}-{month:02}-{:02}", parts[2].min(last_day));
    }
    add_days(
        anchor,
        direction
            * if view == "day" {
                1
            } else if view == "agenda" {
                30
            } else {
                7
            },
    )
}

/// A six-week grid of civil dates with roving focus (`MonthGrid`).
#[component]
fn MonthGrid(
    #[prop(into)] month: Signal<String>,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] today: Signal<String>,
    #[prop(into)] week_start: Signal<u32>,
    focused: RwSignal<String>,
    label: &'static str,
    #[prop(optional, into)] week: Option<Signal<Option<Vec<String>>>>,
    on_move: Callback<String>,
    on_select: Callback<String>,
) -> impl IntoView {
    let dates = Memo::new(move |_| {
        let m = month.get();
        let offset = (weekday(&m) + 7 - week_start.get()) % 7;
        (0..42)
            .map(|i| add_days(&m, i - offset as i64))
            .collect::<Vec<_>>()
    });
    let column = move |date: &str| (weekday(date) + 7 - week_start.get_untracked()) % 7;
    view! {
        <div class="date-popover-grid" role="grid" aria-label=label on:keydown=move |e| {
            let f = focused.get_untracked();
            let next = match e.key().as_str() {
                "ArrowLeft" => Some(add_days(&f, -1)),
                "ArrowRight" => Some(add_days(&f, 1)),
                "ArrowUp" => Some(add_days(&f, -7)),
                "ArrowDown" => Some(add_days(&f, 7)),
                "Home" => Some(add_days(&f, -(column(&f) as i64))),
                "End" => Some(add_days(&f, 6 - column(&f) as i64)),
                "PageUp" => Some(shift(&f, "month", -1)),
                "PageDown" => Some(shift(&f, "month", 1)),
                _ => None,
            };
            if let Some(next) = next { e.prevent_default(); { on_move.try_run(next); }; }
        }>
            <div role="row">{move || dates.get().into_iter().take(7).map(|d| view! { <span role="columnheader">{format_date(&d, &[("weekday", "narrow")])}</span> }).collect_view()}</div>
            {move || {
                let all = dates.get();
                let range = week.and_then(|w| w.get());
                (0..6).map(|row| view! {
                    <div role="row">
                        {all[row * 7..row * 7 + 7].iter().map(|date| {
                            let d = date.clone();
                            let in_week = range.as_ref().is_some_and(|w| w.contains(&d));
                            let classes = [
                                (date[..7] != month.get()[..7]).then_some("outside-month"),
                                in_week.then_some("in-range"),
                                (in_week && range.as_ref().and_then(|w| w.first()) == Some(&d)).then_some("range-start"),
                                (in_week && range.as_ref().and_then(|w| w.last()) == Some(&d)).then_some("range-end"),
                            ].into_iter().flatten().collect::<Vec<_>>().join(" ");
                            let (pick, focus) = (d.clone(), d.clone());
                            view! {
                                <button type="button" role="gridcell" data-date=d.clone() aria-label=format_date(&d, &[("weekday", "long"), ("month", "long"), ("day", "numeric"), ("year", "numeric")])
                                    aria-selected=(value.get() == d).to_string() aria-current=(today.get() == d).then_some("date") tabindex=if focused.get() == d { "0" } else { "-1" }
                                    class=(!classes.is_empty()).then_some(classes)
                                    on:click=move |_| { on_select.try_run(pick.clone()); } on:focus=move |_| focused.set(focus.clone())>
                                    {d[8..].trim_start_matches('0').to_owned()}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                }).collect_view()
            }}
        </div>
    }
}

fn focus_date(root: Option<web_sys::Element>, date: &str) {
    if let Some(button) = root
        .and_then(|r| {
            r.query_selector(&format!("[data-date=\"{date}\"]"))
                .ok()
                .flatten()
        })
        .and_then(|b| b.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = button.focus();
    }
}

/// The themed date navigation control (`DatePopover`).
#[component]
pub fn DatePopover(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] today: Signal<String>,
    #[prop(into)] week_start: Signal<u32>,
    on_change: Callback<String>,
) -> impl IntoView {
    let id = unique_id();
    let panel = NodeRef::<html::Div>::new();
    let trigger = NodeRef::<html::Button>::new();
    let open = RwSignal::new(false);
    let month = RwSignal::new(format!("{}-01", &value.get_untracked()[..7]));
    let focused = RwSignal::new(value.get_untracked());
    Effect::new(move |_| {
        if open.get() {
            month.track();
            let f = focused.get();
            focus_date(panel.get().map(Into::into), &f);
        }
    });
    let close = move || {
        if let Some(p) = panel.get_untracked() {
            crate::ui::call_method(&p, "hidePopover");
        }
        if let Some(t) = trigger.get_untracked() {
            let _ = t.focus();
        }
    };
    let select = move |date: String| {
        {
            on_change.try_run(date);
        };
        close();
    };
    let move_to = Callback::new(move |date: String| {
        month.set(format!("{}-01", &date[..7]));
        focused.set(date);
    });
    let label = move || {
        let (v, t) = (value.get(), today.get());
        if v == t {
            return "Today".to_owned();
        }
        if v[..4] != t[..4] {
            format_date(
                &v,
                &[("month", "short"), ("day", "numeric"), ("year", "numeric")],
            )
        } else {
            format_date(&v, &[("month", "short"), ("day", "numeric")])
        }
    };
    let months: Vec<(String, String)> = (0..12)
        .map(|i| {
            (
                i.to_string(),
                local_date(
                    js_sys::Date::utc(2026.0, i as f64),
                    &[("month", "long"), ("timeZone", "UTC")],
                ),
            )
        })
        .collect();
    let month_index = RwSignal::new(String::new());
    let year = RwSignal::new(String::new());
    Effect::new(move |_| {
        let m = month.get();
        month_index.set((m[5..7].parse::<u32>().unwrap_or(1) - 1).to_string());
        year.set(m[..4].trim_start_matches('0').to_owned());
    });
    let base_year: i32 = value.get_untracked()[..4].parse().unwrap_or(2026);
    let years: Vec<(String, String)> = (base_year - 100..=base_year + 100)
        .filter(|y| (1..=9999).contains(y))
        .map(|y| (y.to_string(), y.to_string()))
        .collect();
    view! {
        <button node_ref=trigger type="button" class="text-button calendar-anchor date-popover-trigger" aria-label="Choose date" aria-haspopup="dialog" aria-expanded=move || open.get().to_string() aria-controls=id.clone() popovertarget=id.clone() data-date=move || value.get()>{label}<Icon name="chevron"/></button>
        <div id=id.clone() node_ref=panel popover="auto" role="dialog" aria-label="Choose calendar date" class="date-popover"
            on:toggle=move |e: web_sys::Event| {
                let visible = js_sys::Reflect::get(&e, &"newState".into()).ok().and_then(|v| v.as_string()).as_deref() == Some("open");
                open.set(visible);
                if visible { let v = value.get_untracked(); month.set(format!("{}-01", &v[..7])); focused.set(v); }
            }
            on:keydown=move |e| if e.key() == "Escape" { e.prevent_default(); e.stop_propagation(); close(); }>
            <div class="date-popover-heading">
                <button class="icon-button" aria-label="Previous month" on:click=move |_| move_to.run(shift(&focused.get_untracked(), "month", -1))><Icon name="back"/></button>
                <strong aria-live="polite">{move || format_date(&month.get(), &[("month", "long"), ("year", "numeric")])}</strong>
                <button class="icon-button" aria-label="Next month" on:click=move |_| move_to.run(shift(&focused.get_untracked(), "month", 1))><Icon name="chevron"/></button>
            </div>
            <div class="date-popover-selectors">
                <Select aria_label="Choose month" options=months value=month_index on_change=Callback::new(move |v: String| { let m = month.get_untracked(); move_to.run(format!("{}-{:02}-01", &m[..4], v.parse::<u32>().unwrap_or(0) + 1)); })/>
                <Select aria_label="Choose year" options=years value=year on_change=Callback::new(move |v: String| { let m = month.get_untracked(); move_to.run(format!("{:0>4}-{}-01", v, &m[5..7])); })/>
            </div>
            <MonthGrid month=month value=value today=today week_start=week_start focused=focused label="Pick a day" on_move=move_to on_select=Callback::new(select)/>
            <div class="date-popover-actions"><button class="text-button" on:click=move |_| select(today.get_untracked())>"Go to today"</button></div>
        </div>
    }
}

/// The sidebar month: follows the anchor and highlights the visible range (`MiniMonth`).
#[component]
pub fn MiniMonth(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] today: Signal<String>,
    #[prop(into)] week_start: Signal<u32>,
    #[prop(into)] range: Signal<Option<Vec<String>>>,
    on_change: Callback<String>,
) -> impl IntoView {
    let month = RwSignal::new(format!("{}-01", &value.get_untracked()[..7]));
    let focused = RwSignal::new(value.get_untracked());
    let root = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        let v = value.get();
        month.set(format!("{}-01", &v[..7]));
        focused.set(v);
    });
    let move_to = Callback::new(move |date: String| {
        focused.set(date.clone());
        month.set(format!("{}-01", &date[..7]));
        request_animation_frame(move || focus_date(root.get_untracked().map(Into::into), &date));
    });
    let step = move |direction: i64| {
        let next = shift(&month.get_untracked(), "month", direction);
        month.set(next.clone());
        focused.set(next);
    };
    view! {
        <div class="mini-month" node_ref=root>
            <div class="mini-month-heading"><strong aria-live="polite">{move || format_date(&month.get(), &[("month", "long"), ("year", "numeric")])}</strong>
                <button type="button" class="icon-button" aria-label="Previous month in sidebar" on:click=move |_| step(-1)><Icon name="back"/></button>
                <button type="button" class="icon-button" aria-label="Next month in sidebar" on:click=move |_| step(1)><Icon name="chevron"/></button>
            </div>
            <MonthGrid month=month value=value today=today week_start=week_start focused=focused label="Month overview" week=range on_move=move_to on_select=on_change/>
        </div>
    }
}
