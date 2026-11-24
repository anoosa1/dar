//! Repeat (with visual recurrence builder) and reminder fields (`pim/schedule-controls.tsx`).

use leptos::prelude::*;

use super::presentation::recurrence_label;
use crate::ui::Select;

const REPEATS: [&str; 4] = ["DAILY", "WEEKLY", "MONTHLY", "YEARLY"];
const WEEKDAYS: [(&str, &str); 7] = [
    ("MO", "M"),
    ("TU", "T"),
    ("WE", "W"),
    ("TH", "T"),
    ("FR", "F"),
    ("SA", "S"),
    ("SU", "S"),
];

fn parse_parts(rule: &str) -> Vec<(String, String)> {
    rule.trim()
        .trim_start_matches("RRULE:")
        .to_uppercase()
        .split(';')
        .filter_map(|p| p.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned())))
        .collect()
}

fn part<'a>(parts: &'a [(String, String)], key: &str) -> Option<&'a str> {
    parts
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn build_rrule(
    freq: &str,
    interval: u32,
    byday: &[String],
    end_mode: &str,
    count: u32,
    until: &str,
) -> String {
    let mut out = vec![format!("FREQ={freq}")];
    if interval > 1 {
        out.push(format!("INTERVAL={interval}"));
    }
    if freq == "WEEKLY" && !byday.is_empty() {
        out.push(format!("BYDAY={}", byday.join(",")));
    }
    match end_mode {
        "count" => out.push(format!("COUNT={}", count.max(1))),
        "until" if until.len() == 10 => {
            out.push(format!("UNTIL={}", until.replace('-', "")));
        }
        _ => {}
    }
    out.join(";")
}

#[component]
pub fn RepeatField(value: RwSignal<String>) -> impl IntoView {
    let custom = RwSignal::new(false);
    let mode = Memo::new(move |_| {
        let v = value.get();
        let simple = v.is_empty()
            || v == "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"
            || REPEATS.iter().any(|f| v == format!("FREQ={f}"));
        if custom.get() || !simple {
            "custom".to_owned()
        } else if v == "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR" {
            "weekdays".to_owned()
        } else {
            v.trim_start_matches("FREQ=").to_lowercase()
        }
    });
    let selected = RwSignal::new(mode.get_untracked());
    Effect::new(move |_| selected.set(mode.get()));

    // Visual builder state synced from `value` when custom opens or edits happen.
    let init_parts = parse_parts(&value.get_untracked());
    let freq = RwSignal::new(
        part(&init_parts, "FREQ")
            .filter(|f| REPEATS.contains(f))
            .unwrap_or("WEEKLY")
            .to_owned(),
    );
    let interval = RwSignal::new(
        part(&init_parts, "INTERVAL")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(1)
            .clamp(1, 99),
    );
    let byday = RwSignal::new(
        part(&init_parts, "BYDAY")
            .map(|d| {
                d.split(',')
                    .map(|x| x.trim().to_owned())
                    .filter(|x| WEEKDAYS.iter().any(|(code, _)| code == x))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );
    let end_mode = RwSignal::new(if part(&init_parts, "COUNT").is_some() {
        "count".to_owned()
    } else if part(&init_parts, "UNTIL").is_some() {
        "until".to_owned()
    } else {
        "never".to_owned()
    });
    let count = RwSignal::new(
        part(&init_parts, "COUNT")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(10)
            .clamp(1, 999),
    );
    let until = RwSignal::new(
        part(&init_parts, "UNTIL")
            .filter(|u| u.len() >= 8 && u[..8].bytes().all(|b| b.is_ascii_digit()))
            .map(|u| format!("{}-{}-{}", &u[0..4], &u[4..6], &u[6..8]))
            .unwrap_or_default(),
    );

    let sync_from_visual = move || {
        let built = build_rrule(
            &freq.get_untracked(),
            interval.get_untracked(),
            &byday.get_untracked(),
            &end_mode.get_untracked(),
            count.get_untracked(),
            &until.get_untracked(),
        );
        value.set(built);
    };

    let options: Vec<(String, String)> = [
        ("", "Does not repeat"),
        ("daily", "Daily"),
        ("weekdays", "Every weekday (Mon – Fri)"),
        ("weekly", "Weekly"),
        ("monthly", "Monthly"),
        ("yearly", "Yearly"),
        ("custom", "Custom schedule…"),
    ]
    .iter()
    .map(|(v, l)| ((*v).into(), (*l).into()))
    .collect();
    let freq_options: Vec<(String, String)> = [
        ("DAILY", "day(s)"),
        ("WEEKLY", "week(s)"),
        ("MONTHLY", "month(s)"),
        ("YEARLY", "year(s)"),
    ]
    .iter()
    .map(|(v, l)| ((*v).into(), (*l).into()))
    .collect();
    let end_options: Vec<(String, String)> = [
        ("never", "Never"),
        ("count", "After occurrences"),
        ("until", "On date"),
    ]
    .iter()
    .map(|(v, l)| ((*v).into(), (*l).into()))
    .collect();

    view! {
        <div class="repeat-field">
            <input type="hidden" name="recurrence" prop:value=move || value.get()/>
            <label>"Repeats"<Select aria_label="Repeats" options=options value=selected on_change=Callback::new(move |next: String| {
                custom.set(next == "custom");
                match next.as_str() {
                    "" => value.set(String::new()),
                    "weekdays" => value.set("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".into()),
                    "custom" => {
                        if value.get_untracked().is_empty() {
                            freq.set("WEEKLY".into());
                            interval.set(1);
                            byday.set(Vec::new());
                            end_mode.set("count".into());
                            count.set(10);
                            value.set("FREQ=WEEKLY;COUNT=10".into());
                        }
                    }
                    other => value.set(format!("FREQ={}", other.to_uppercase())),
                }
            })/></label>
            {move || (!value.get().is_empty()).then(|| view! { <p class="field-hint">{recurrence_label(&value.get(), false)}</p> })}
            {move || (mode.get() == "custom").then(|| view! {
                <div class="repeat-builder">
                    <div class="repeat-interval-row">
                        <span>"Every"</span>
                        <input type="number" min="1" max="99" aria-label="Repeat interval" prop:value=move || interval.get().to_string()
                            on:input=move |e| {
                                if let Ok(n) = event_target_value(&e).parse::<u32>() {
                                    interval.set(n.clamp(1, 99));
                                    sync_from_visual();
                                }
                            }/>
                        <Select aria_label="Repeat frequency" options=freq_options.clone() value=freq on_change=Callback::new(move |_| sync_from_visual())/>
                    </div>
                    {move || (freq.get() == "WEEKLY").then(|| view! {
                        <div class="repeat-weekdays" role="group" aria-label="Repeat on weekdays">
                            {WEEKDAYS.into_iter().map(|(code, short)| {
                                let code_str = code.to_owned();
                                let c2 = code_str.clone();
                                let active = Signal::derive(move || byday.with(|list| list.contains(&code_str)));
                                view! {
                                    <button type="button" class=move || if active.get() { "repeat-day on" } else { "repeat-day" } aria-pressed=move || active.get().to_string()
                                        on:click=move |_| {
                                            byday.update(|list| {
                                                if let Some(pos) = list.iter().position(|d| *d == c2) {
                                                    list.remove(pos);
                                                } else {
                                                    list.push(c2.clone());
                                                    list.sort_by_key(|d| WEEKDAYS.iter().position(|(k, _)| k == d).unwrap_or(7));
                                                }
                                            });
                                            sync_from_visual();
                                        }>{short}</button>
                                }
                            }).collect_view()}
                        </div>
                    })}
                    <div class="repeat-end-row">
                        <label>"Ends"<Select aria_label="Repeat ends" options=end_options.clone() value=end_mode on_change=Callback::new(move |_| sync_from_visual())/></label>
                        {move || match end_mode.get().as_str() {
                            "count" => view! {
                                <label>"Times"<input type="number" min="1" max="999" aria-label="Repeat occurrences" prop:value=move || count.get().to_string()
                                    on:input=move |e| {
                                        if let Ok(n) = event_target_value(&e).parse::<u32>() {
                                            count.set(n.clamp(1, 999));
                                            sync_from_visual();
                                        }
                                    }/></label>
                            }.into_any(),
                            "until" => view! {
                                <label>"End date"<input type="date" aria-label="Repeat until date" prop:value=move || until.get()
                                    on:input=move |e| {
                                        until.set(event_target_value(&e));
                                        sync_from_visual();
                                    }/></label>
                            }.into_any(),
                            _ => ().into_any(),
                        }}
                    </div>
                    <details class="repeat-advanced" open=true>
                        <summary>"Advanced rule (RRULE)"</summary>
                        <label>"Recurrence rule"<input aria-label="Recurrence rule" prop:value=move || value.get() maxlength="2048" on:input=move |e| value.set(event_target_value(&e))/><small>"RFC 5545 rule, e.g. FREQ=WEEKLY;BYDAY=MO,WE;COUNT=12."</small></label>
                    </details>
                </div>
            })}
        </div>
    }
}

const REMINDERS: [(&str, &str); 9] = [
    ("", "No reminder"),
    ("0", "At start"),
    ("5", "5 minutes before"),
    ("10", "10 minutes before"),
    ("15", "15 minutes before"),
    ("30", "30 minutes before"),
    ("60", "1 hour before"),
    ("1440", "1 day before"),
    ("10080", "1 week before"),
];

/// `value` is minutes before the start; empty means no reminder.
#[component]
pub fn ReminderField(value: RwSignal<String>) -> impl IntoView {
    let custom = RwSignal::new(false);
    let mode = Memo::new(move |_| {
        let v = value.get();
        if custom.get() || !REMINDERS.iter().any(|(o, _)| *o == v) {
            "custom".to_owned()
        } else {
            v
        }
    });
    let selected = RwSignal::new(mode.get_untracked());
    Effect::new(move |_| selected.set(mode.get()));
    let mut options: Vec<(String, String)> = REMINDERS
        .iter()
        .map(|(v, l)| ((*v).into(), (*l).into()))
        .collect();
    options.push(("custom".into(), "Custom reminder…".into()));
    view! {
        <div>
            <input type="hidden" name="reminderMinutes" prop:value=move || value.get()/>
            <label>"Reminder"<Select aria_label="Reminder" options=options value=selected on_change=Callback::new(move |next: String| {
                custom.set(next == "custom");
                if next != "custom" { value.set(next); } else if value.get_untracked().is_empty() { value.set("20".into()); }
            })/></label>
            {move || (mode.get() == "custom").then(|| view! { <label>"Minutes before"<input type="number" min="0" max="10080" prop:value=move || value.get() on:input=move |e| value.set(event_target_value(&e))/></label> })}
        </div>
    }
}
