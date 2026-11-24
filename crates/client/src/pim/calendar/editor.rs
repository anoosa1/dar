//! Event & Subscription editing panes (`EventEditor` and `SubscriptionEditor`).
//! `EventEditor` supports moving master events between calendars, styled All-day switch,
//! visual recurrence builder, Contacts-assisted attendee autocomplete, and duration checks.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue};

use super::model::{Editor, parse_ms, s};
use crate::api;
use crate::date_popover::add_days;
use crate::icons::Icon;
use crate::pim::schedule::{ReminderField, RepeatField};
use crate::pim::{Collection, Done};
use crate::ui::{EditorPane, Select};

const REFRESH: [(&str, &str); 5] = [
    ("60", "Every hour"),
    ("180", "Every 3 hours"),
    ("360", "Every 6 hours"),
    ("720", "Every 12 hours"),
    ("1440", "Every day"),
];

/// Subscribe by link, or edit a subscription (`SubscriptionEditor`).
#[component]
pub fn SubscriptionEditor(
    calendar: Option<Collection>,
    on_save: Callback<(Value, Done)>,
    on_close: Callback<()>,
    on_refresh: Callback<()>,
) -> impl IntoView {
    let status = calendar.as_ref().and_then(|c| c.subscription.clone());
    let editing = calendar.is_some();
    let url = RwSignal::new(
        status
            .as_ref()
            .map(|s| s["url"].as_str().unwrap_or_default().to_owned())
            .unwrap_or_default(),
    );
    let name = RwSignal::new(
        calendar
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default(),
    );
    let refresh = RwSignal::new(
        status
            .as_ref()
            .and_then(|s| s["refreshMinutes"].as_u64())
            .unwrap_or(360)
            .to_string(),
    );
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let color = calendar
        .as_ref()
        .map(|c| c.color.clone())
        .unwrap_or_else(|| "#2f7d8c".into());

    let suggested = Memo::new(move |_| {
        let raw = url.get().trim().to_owned();
        let lower = raw.to_lowercase();
        let normalized = if lower.starts_with("webcals://") {
            format!("https://{}", &raw[10..])
        } else if lower.starts_with("webcal://") {
            format!("https://{}", &raw[9..])
        } else {
            raw
        };
        let Ok(parsed) = web_sys::Url::new(&normalized) else {
            return String::new();
        };
        let last = parsed
            .pathname()
            .split('/')
            .rfind(|p| !p.is_empty())
            .map(|p| {
                js_sys::decode_uri_component(p)
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let last = last
            .trim_end_matches(".ics")
            .trim_end_matches(".ICS")
            .replace(['-', '_'], " ")
            .trim()
            .to_owned();
        let opaque = last.len() >= 24 && last.chars().all(|c| c.is_ascii_alphanumeric());
        if !last.is_empty() && !opaque {
            last
        } else {
            parsed.hostname().trim_start_matches("www.").to_owned()
        }
    });
    let options: Vec<(String, String)> = REFRESH
        .iter()
        .map(|(v, l)| ((*v).into(), (*l).into()))
        .collect();
    let dirty = Signal::derive(move || !editing && !url.with(String::is_empty));
    view! {
        <EditorPane title=if editing { "Edit subscription" } else { "Subscribe to a calendar" } on_close=on_close dirty=dirty>
            <form class="calendar-subscription-form" on:submit=move |e: web_sys::SubmitEvent| {
                e.prevent_default();
                let color = e
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok())
                    .and_then(|f| web_sys::FormData::new_with_form(&f).ok())
                    .and_then(|d| d.get("color").as_string())
                    .unwrap_or_default();
                let typed = name.get_untracked().trim().to_owned();
                let chosen: String = if !typed.is_empty() {
                    typed
                } else if !suggested.get_untracked().is_empty() {
                    suggested.get_untracked()
                } else {
                    "Subscribed calendar".into()
                };
                busy.set(true);
                error.set(String::new());
                on_save.try_run((
                    json!({
                        "url": url.get_untracked().trim(),
                        "name": chosen.chars().take(255).collect::<String>(),
                        "color": color,
                        "refreshMinutes": refresh.get_untracked().parse::<u32>().unwrap_or(360),
                    }),
                    Callback::new(move |result: Result<(), String>| {
                        busy.set(false);
                        match result {
                            Ok(()) => {
                                on_close.try_run(());
                            }
                            Err(e) => error.set(e),
                        }
                    }),
                ));
            }>
                <label>
                    "Calendar link"
                    <input type="url" inputmode="url" required autofocus=!editing autocapitalize="none" spellcheck="false" maxlength="2048" placeholder="https://… or webcal://…" prop:value=move || url.get() on:input=move |e| url.set(event_target_value(&e)) pattern="(https?|webcals?)://.+"/>
                    <small class="field-hint">"An iCalendar (.ics) address. Events from it are read-only here and stay in sync."</small>
                </label>
                <label>
                    "Name"
                    <input maxlength="255" prop:value=move || name.get() placeholder=move || {
                        let s = suggested.get();
                        if s.is_empty() { "Subscribed calendar".to_owned() } else { s }
                    } on:input=move |e| name.set(event_target_value(&e))/>
                </label>
                <div class="pim-form-grid">
                    <label>"Color"<input name="color" type="color" value=color.clone()/></label>
                    <label>"Refresh"<Select aria_label="Refresh interval" options=options.clone() value=refresh/></label>
                </div>
                {status.clone().map(|sub| {
                    let failed = sub["lastError"].as_str().unwrap_or_default().to_owned();
                    let synced = sub["lastSyncedAt"].as_f64().unwrap_or(0.0);
                    view! {
                        <div class="property-card">
                            <div class="property">
                                <Icon name=if failed.is_empty() { "check" } else { "alert" }/>
                                <div>
                                    <span>{if !failed.is_empty() { failed.clone() } else if synced > 0.0 { format!("Updated {}", crate::dates::local_date_time(synced, &[])) } else { "Waiting for the first refresh".into() }}</span>
                                    <small>{if failed.is_empty() { "Sync status" } else { "Last refresh failed; Dar retries within the hour" }}</small>
                                </div>
                            </div>
                            <button type="button" class="property property-action" disabled=move || busy.get() on:click=move |_| { on_refresh.try_run(()); }>
                                <Icon name="refresh"/>
                                <div><span>"Refresh now"</span><small>"Fetch the calendar again"</small></div>
                            </button>
                        </div>
                    }
                })}
                {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions">
                    <button type="button" class="text-button">"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get() || url.with(|u| u.trim().is_empty())>
                        {move || if busy.get() { "Saving…" } else if editing { "Save" } else { "Subscribe" }}
                    </button>
                </div>
            </form>
        </EditorPane>
    }
}

fn minutes_of(value: &str) -> f64 {
    if value.len() < 16 {
        return f64::NAN;
    }
    parse_ms(&format!("{}:00Z", &value[..16]))
}

fn stamp(ms: f64) -> String {
    js_sys::Date::new(&JsValue::from_f64(ms))
        .to_iso_string()
        .as_string()
        .unwrap_or_default()[..16]
        .to_owned()
}

fn time_zones() -> Vec<String> {
    let intl = js_sys::Reflect::get(&js_sys::global(), &"Intl".into()).ok();
    let supported = intl
        .as_ref()
        .and_then(|i| js_sys::Reflect::get(i, &"supportedValuesOf".into()).ok())
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok());
    let mut zones = vec!["UTC".to_owned(), "floating".to_owned()];
    if let (Some(intl), Some(f)) = (intl, supported)
        && let Ok(list) = f.call1(&intl, &"timeZone".into())
    {
        zones.extend(
            js_sys::Array::from(&list)
                .iter()
                .filter_map(|v| v.as_string()),
        );
    }
    zones
}

/// The full event form (`EventEditor`).
#[component]
pub fn EventEditor(
    initial: Editor,
    calendars: Vec<Collection>,
    on_close: Callback<()>,
    on_save: Callback<(Value, String, Done)>,
) -> impl IntoView {
    let data = RwSignal::new(initial.data.clone());
    let collection = RwSignal::new(initial.collection_id.clone());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let recurrence = RwSignal::new(s(&initial.data, "recurrence"));
    let reminder = RwSignal::new(
        initial.data["reminderMinutes"]
            .as_i64()
            .map(|m| m.to_string())
            .unwrap_or_default(),
    );
    let status = RwSignal::new({
        let st = s(&initial.data, "status");
        if st.is_empty() {
            "CONFIRMED".to_owned()
        } else {
            st
        }
    });
    let contact_query = RwSignal::new(String::new());
    let contact_hits: RwSignal<Vec<(String, String)>> = RwSignal::new(Vec::new());

    Effect::new(move |_| {
        let r = recurrence.get();
        data.update(|d| d["recurrence"] = json!(r));
    });
    Effect::new(move |_| {
        let r = reminder.get();
        data.update(|d| {
            d["reminderMinutes"] = if r.is_empty() {
                Value::Null
            } else {
                r.parse::<i64>().map(|m| json!(m)).unwrap_or(Value::Null)
            }
        });
    });
    Effect::new(move |_| {
        let st = status.get();
        data.update(|d| d["status"] = json!(st));
    });
    Effect::new(move |_| {
        let q = contact_query.get().trim().to_owned();
        if q.len() < 2 {
            contact_hits.set(Vec::new());
            return;
        }
        spawn_local(async move {
            let enc = js_sys::encode_uri_component(&q)
                .as_string()
                .unwrap_or_default();
            if let Ok(res) = api::get::<Value>(&format!("/api/v1/contacts/people?q={enc}")).await {
                let mut pairs = Vec::new();
                if let Some(items) = res["items"].as_array() {
                    for p in items.iter().take(6) {
                        let name = p["data"]["displayName"]
                            .as_str()
                            .or_else(|| p["displayName"].as_str())
                            .unwrap_or_default()
                            .to_owned();
                        let emails = p["data"]["emails"]
                            .as_array()
                            .or_else(|| p["emails"].as_array());
                        if let Some(arr) = emails {
                            for em in arr {
                                let addr = em["value"]
                                    .as_str()
                                    .or_else(|| em.as_str())
                                    .unwrap_or_default();
                                if !addr.is_empty() {
                                    pairs.push((name.clone(), addr.to_owned()));
                                    break;
                                }
                            }
                        }
                    }
                }
                contact_hits.try_set(pairs);
            }
        });
    });

    let get = move |k: &'static str| Signal::derive(move || data.with(|d| s(d, k)));
    let all_day = Signal::derive(move || data.with(|d| d["allDay"] == json!(true)));
    let set = move |k: &'static str, v: Value| data.update(|d| d[k] = v);
    let set_start = move |next: String| {
        data.update(|d| {
            if d["allDay"] == json!(true) || next.len() < 16 || minutes_of(&next).is_nan() {
                d["start"] = json!(next);
                return;
            }
            let duration = (minutes_of(&s(d, "end")) - minutes_of(&s(d, "start"))).max(0.0);
            let end = if duration.is_nan() {
                s(d, "end")
            } else {
                stamp(minutes_of(&next) + duration)
            };
            d["start"] = json!(next);
            d["end"] = json!(end);
        })
    };

    let start_date =
        Signal::derive(move || get("start").get().chars().take(10).collect::<String>());
    let start_time = Signal::derive(move || {
        get("start")
            .get()
            .chars()
            .skip(11)
            .take(5)
            .collect::<String>()
    });
    let end_date = Signal::derive(move || {
        let e = get("end").get();
        if all_day.get() && e.len() >= 10 {
            add_days(&e[..10], -1)
        } else {
            e.chars().take(10).collect()
        }
    });
    let end_time = Signal::derive(move || {
        get("end")
            .get()
            .chars()
            .skip(11)
            .take(5)
            .collect::<String>()
    });
    let duration = Memo::new(move |_| {
        if all_day.get() {
            let (s_d, e_d) = (start_date.get(), end_date.get());
            if s_d.len() == 10 && e_d.len() == 10 {
                (parse_ms(&format!("{e_d}T12:00:00Z")) - parse_ms(&format!("{s_d}T12:00:00Z")))
                    / 86_400_000.0
                    + 1.0
            } else {
                f64::NAN
            }
        } else {
            (minutes_of(&get("end").get()) - minutes_of(&get("start").get())) / 60_000.0
        }
    });
    let duration_text = move || {
        let d = duration.get();
        if d.is_nan() {
            return String::new();
        }
        if all_day.get() {
            if d < 1.0 {
                "End date must be on or after the start date.".to_owned()
            } else if d == 1.0 {
                "1 day".to_owned()
            } else {
                format!("{} days", d.round() as i64)
            }
        } else if d < 0.0 {
            "End time must be after start time.".to_owned()
        } else {
            let mins = d.round() as i64;
            let (h, m) = (mins / 60, mins % 60);
            match (h, m) {
                (0, m) => format!("{m} minutes"),
                (h, 0) => format!("{h} hour{}", if h == 1 { "" } else { "s" }),
                (h, m) => format!("{h}h {m}m"),
            }
        }
    };

    let creating = initial.resource.is_none();
    let occurrence = initial.occurrence.is_some();
    let show_calendar_picker = !occurrence && !calendars.is_empty();
    let heading = if creating {
        "New event"
    } else if occurrence {
        "Edit occurrence"
    } else {
        "Edit event"
    };
    let options: Vec<(String, String)> = calendars.into_iter().map(|c| (c.id, c.name)).collect();
    let statuses: Vec<(String, String)> = [
        ("CONFIRMED", "Confirmed"),
        ("TENTATIVE", "Tentative"),
        ("CANCELLED", "Cancelled"),
    ]
    .iter()
    .map(|(v, l)| ((*v).into(), (*l).into()))
    .collect();
    let more_open = initial.data["attendees"]
        .as_array()
        .is_some_and(|a| !a.is_empty())
        || s(&initial.data, "status") != "CONFIRMED" && !s(&initial.data, "status").is_empty();
    let initial_data = StoredValue::new(initial.data);
    let dirty = Signal::derive(move || data.get() != initial_data.get_value());
    let zones = StoredValue::new(time_zones());

    let add_attendee = move |email: String| {
        data.update(|d| {
            let mut list: Vec<String> = d["attendees"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if !list.iter().any(|x| x.eq_ignore_ascii_case(&email)) {
                list.push(email);
            }
            d["attendees"] = json!(list);
        });
        contact_query.set(String::new());
        contact_hits.set(Vec::new());
    };

    view! {
        <EditorPane title=heading on_close=on_close dirty=dirty>
            <form class="event-editor" on:submit=move |e: web_sys::SubmitEvent| {
                e.prevent_default();
                busy.set(true);
                error.set(String::new());
                on_save.try_run((
                    data.get_untracked(),
                    collection.get_untracked(),
                    Callback::new(move |result: Result<(), String>| {
                        busy.set(false);
                        if let Err(e) = result {
                            error.set(e);
                        }
                    }),
                ));
            }>
                {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <label class="event-title-field">
                    <span class="sr-only">"Title"</span>
                    <input
                        prop:value=get("title")
                        autofocus
                        required
                        maxlength="1024"
                        placeholder="Add title"
                        on:input=move |e| set("title", json!(event_target_value(&e)))
                    />
                </label>

                {show_calendar_picker.then(|| view! {
                    <label>
                        "Calendar"
                        <Select options=options.clone() value=collection/>
                    </label>
                })}

                <label class="pim-checkbox calendar-all-day-switch">
                    <input
                        type="checkbox"
                        prop:checked=all_day
                        on:change=move |e| {
                            let on = event_target_checked(&e);
                            data.update(|d| {
                                let day_str = s(d, "start").chars().take(10).collect::<String>();
                                d["allDay"] = json!(on);
                                d["start"] = json!(if on { day_str.clone() } else { format!("{day_str}T09:00") });
                                d["end"] = json!(if on { add_days(&day_str, 1) } else { format!("{day_str}T10:00") });
                            });
                        }
                    />
                    <span class="calendar-switch-track" aria-hidden="true"></span>
                    <span>"All day"</span>
                </label>

                <div class="event-when">
                    <label>
                        "Starts"
                        <input
                            type="date"
                            prop:value=start_date
                            required
                            on:input=move |e| {
                                let v = event_target_value(&e);
                                if all_day.get_untracked() {
                                    set("start", json!(v));
                                } else {
                                    let t = start_time.get_untracked();
                                    set_start(format!("{v}T{}", if t.is_empty() { "09:00".to_owned() } else { t }));
                                }
                            }
                        />
                    </label>
                    {move || (!all_day.get()).then(|| view! {
                        <label>
                            "Start time"
                            <input
                                type="time"
                                prop:value=start_time
                                required
                                on:input=move |e| set_start(format!("{}T{}", start_date.get_untracked(), event_target_value(&e)))
                            />
                        </label>
                    })}
                </div>

                <div class="event-when">
                    <label>
                        "Ends"
                        <input
                            type="date"
                            prop:value=end_date
                            required
                            on:input=move |e| {
                                let v = event_target_value(&e);
                                if all_day.get_untracked() && !v.is_empty() {
                                    set("end", json!(add_days(&v, 1)));
                                } else {
                                    let t = end_time.get_untracked();
                                    set("end", json!(format!("{v}T{}", if t.is_empty() { "10:00".to_owned() } else { t })));
                                }
                            }
                        />
                    </label>
                    {move || (!all_day.get()).then(|| view! {
                        <label>
                            "End time"
                            <input
                                type="time"
                                prop:value=end_time
                                required
                                on:input=move |e| set("end", json!(format!("{}T{}", end_date.get_untracked(), event_target_value(&e))))
                            />
                        </label>
                    })}
                </div>

                {move || (!duration.get().is_nan()).then(|| view! {
                    <p class=move || if duration.get() < 0.0 || (all_day.get() && duration.get() < 1.0) { "field-hint error" } else { "field-hint" }>
                        {duration_text}
                    </p>
                })}

                {move || (!all_day.get()).then(|| view! {
                    <label>
                        "Timezone"
                        <input
                            aria-label="Timezone"
                            list="common-timezones"
                            prop:value=get("timezone")
                            on:input=move |e| set("timezone", json!(event_target_value(&e)))
                            required
                        />
                        <datalist id="common-timezones">
                            {zones.get_value().into_iter().map(|z| view! { <option value=z></option> }).collect_view()}
                        </datalist>
                        <small>"Use floating to keep the same wall time in each viewer's timezone."</small>
                    </label>
                })}

                {(!occurrence).then(|| view! { <RepeatField value=recurrence/> })}
                <ReminderField value=reminder/>

                <label>
                    "Location"
                    <input
                        prop:value=get("location")
                        maxlength="2048"
                        placeholder="Add location or meeting link"
                        on:input=move |e| set("location", json!(event_target_value(&e)))
                    />
                </label>

                <label>
                    "Description"
                    <textarea
                        rows="4"
                        prop:value=get("description")
                        placeholder="Add notes"
                        on:input=move |e| set("description", json!(event_target_value(&e)))
                    ></textarea>
                </label>

                <details class="form-more" open=more_open>
                    <summary>"Attendees and status"</summary>
                    <label class="calendar-contact-lookup">
                        "Find from Contacts"
                        <input
                            type="search"
                            placeholder="Type a name or email…"
                            prop:value=move || contact_query.get()
                            on:input=move |e| contact_query.set(event_target_value(&e))
                        />
                    </label>
                    {move || {
                        let hits = contact_hits.get();
                        (!hits.is_empty()).then(|| view! {
                            <div class="calendar-contact-hits">
                                {hits.into_iter().map(|(cname, email)| {
                                    let em = email.clone();
                                    view! {
                                        <button type="button" class="calendar-contact-hit" on:click=move |_| add_attendee(em.clone())>
                                            <strong>{if cname.is_empty() { email.clone() } else { cname }}</strong>
                                            <span>{email}</span>
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        })
                    }}
                    <label>
                        "Attendee emails (one per line)"
                        <textarea
                            prop:value=move || data.with(|d| d["attendees"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default())
                            on:input=move |e| set("attendees", json!(event_target_value(&e).split('\n').map(str::trim).filter(|v| !v.is_empty()).collect::<Vec<_>>()))
                        ></textarea>
                        <small>"Attendees are saved. Invitation emails are not sent."</small>
                    </label>
                    <label>
                        "Status"
                        <Select aria_label="Status" options=statuses.clone() value=status/>
                    </label>
                </details>

                <div class="dialog-actions">
                    <button class="text-button" type="button">"Cancel"</button>
                    <button
                        class="primary-button"
                        disabled=move || busy.get() || duration.get() < 0.0 || (all_day.get() && duration.get() < 1.0)
                    >
                        "Save event"
                    </button>
                </div>
            </form>
        </EditorPane>
    }
}
