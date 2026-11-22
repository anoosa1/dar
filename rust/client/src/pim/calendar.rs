//! Calendar (`pim/calendar.tsx`): month, week, day and agenda views of every visible
//! calendar in the account's time zone, overlapping-event lanes, drag to move or resize,
//! the event card, the event and subscription editors, sharing and reminders.

use std::cell::RefCell;
use std::collections::HashMap;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue};

use super::presentation::recurrence_label;
use super::schedule::{ReminderField, RepeatField};
use super::{Collection, CollectionDot, CollectionEditor, Done, Item, PimShell, Sharing, etag, send, use_pim};
use crate::api::{self, BrowserSession};
use crate::date_popover::{DatePopover, MiniMonth, add_days, format_date, shift};
use crate::icons::Icon;
use crate::shared::{context, save_preferences};
use crate::ui::{ActionMenu, Dialog, EditorPane, MenuAction, Select, window};

const BASE: &str = "/api/v1/calendar/calendars";

thread_local! {
    static FORMATS: RefCell<HashMap<String, js_sys::Intl::DateTimeFormat>> = RefCell::new(HashMap::new());
    static TIMES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// "YYYY-MM-DDTHH:MM" of an instant in a zone, cached (`local`).
fn local(epoch: f64, zone: &str) -> String {
    let key = format!("{zone} {epoch}");
    if let Some(known) = TIMES.with(|t| t.borrow().get(&key).cloned()) {
        return known;
    }
    let value = FORMATS.with(|f| {
        let mut f = f.borrow_mut();
        let format = f.entry(zone.to_owned()).or_insert_with(|| {
            let options = js_sys::Object::new();
            for (k, v) in [("timeZone", zone), ("year", "numeric"), ("month", "2-digit"), ("day", "2-digit"), ("hour", "2-digit"), ("minute", "2-digit"), ("hourCycle", "h23")] {
                let _ = js_sys::Reflect::set(&options, &k.into(), &v.into());
            }
            js_sys::Intl::DateTimeFormat::new(&js_sys::Array::of1(&"en-CA".into()), &options)
        });
        let parts = format.format_to_parts(&js_sys::Date::new(&JsValue::from_f64(epoch)));
        let mut map = HashMap::new();
        for part in parts.iter() {
            let get = |k: &str| js_sys::Reflect::get(&part, &k.into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
            map.insert(get("type"), get("value"));
        }
        let p = |k: &str| map.get(k).cloned().unwrap_or_default();
        format!("{}-{}-{}T{}:{}", p("year"), p("month"), p("day"), p("hour"), p("minute"))
    });
    TIMES.with(|t| {
        let mut t = t.borrow_mut();
        if t.len() >= 10_000 {
            t.clear();
        }
        t.insert(key, value.clone());
    });
    value
}

fn parse_ms(value: &str) -> f64 {
    js_sys::Date::parse(value)
}

fn at(event: &Value, key: &str) -> f64 {
    parse_ms(event[key].as_str().unwrap_or_default())
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_owned()
}

fn weekday(date: &str) -> u32 {
    js_sys::Date::new(&JsValue::from_f64(parse_ms(&format!("{date}T12:00:00Z")))).get_utc_day()
}

/// The visible days of a view (`range`).
fn range(anchor: &str, view: &str, first: u32) -> (String, i64) {
    match view {
        "day" => (anchor.to_owned(), 1),
        "agenda" => (anchor.to_owned(), 30),
        "month" => {
            let start = format!("{}-01", &anchor[..7]);
            let offset = (weekday(&start) + 7 - first) % 7;
            (add_days(&start, -(offset as i64)), 42)
        }
        _ => (add_days(anchor, -(((weekday(anchor) + 7 - first) % 7) as i64)), 7),
    }
}

fn day(date: &str, options: &[(&str, &str)]) -> String {
    format_date(date, options)
}

/// "November 2026", "Nov 9 – 15, 2026" or "Thursday, November 12, 2026" (`periodTitle`).
fn period_title(anchor: &str, view: &str, days: &[String]) -> String {
    match view {
        "day" => day(anchor, &[("weekday", "long"), ("month", "long"), ("day", "numeric"), ("year", "numeric")]),
        "week" => {
            let (first, last) = (&days[0], days.last().expect("week"));
            let month = |d: &str| day(d, &[("month", "short")]);
            let d = |date: &str| date[8..].trim_start_matches('0').to_owned();
            if first[..4] != last[..4] {
                format!("{} {}, {} – {} {}, {}", month(first), d(first), &first[..4], month(last), d(last), &last[..4])
            } else if first[..7] == last[..7] {
                format!("{} {} – {}, {}", month(first), d(first), d(last), &first[..4])
            } else {
                format!("{} {} – {} {}, {}", month(first), d(first), month(last), d(last), &first[..4])
            }
        }
        _ => day(anchor, &[("month", "long"), ("year", "numeric")]),
    }
}

/// The event card's date line; all-day ranges show inclusive end dates (`eventWhen`).
fn event_when(event: &Value, zone: &str) -> String {
    if event["allDay"] == json!(true) {
        let start = &s(event, "start")[..10];
        let last = add_days(&s(event, "end")[..10], -1);
        return if last.as_str() <= start { day(start, &[("weekday", "long"), ("month", "long"), ("day", "numeric")]) } else { format!("{} – {}", day(start, &[("month", "short"), ("day", "numeric")]), day(&last, &[("month", "short"), ("day", "numeric"), ("year", "numeric")])) };
    }
    let (start_at, end_at) = (at(event, "startAt"), at(event, "endAt"));
    let (start, end) = (local(start_at, zone)[..10].to_owned(), local(end_at, zone)[..10].to_owned());
    if start == end || end_at - start_at <= 86_400_000.0 && &local(end_at, zone)[11..] == "00:00" {
        day(&start, &[("weekday", "long"), ("month", "long"), ("day", "numeric")])
    } else {
        format!("{} – {}", day(&start, &[("weekday", "short"), ("month", "short"), ("day", "numeric")]), day(&end, &[("weekday", "short"), ("month", "short"), ("day", "numeric")]))
    }
}

fn reminder_text(minutes: i64) -> String {
    if minutes % 1440 == 0 {
        format!("{} day{}", minutes / 1440, if minutes == 1440 { "" } else { "s" })
    } else if minutes % 60 == 0 {
        format!("{} hour{}", minutes / 60, if minutes == 60 { "" } else { "s" })
    } else {
        format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
    }
}

fn occurrence_key(e: &Value) -> String {
    format!("{}/{}", s(e, "resourceId"), s(e, "recurrenceId"))
}

/// Side-by-side lanes for overlapping timed events, per day (`dayLanes`).
fn day_lanes(days: &[String], events: &[Value], zone: &str) -> HashMap<String, (usize, usize)> {
    let mut result = HashMap::new();
    for date in days {
        let mut rows: Vec<&Value> = events.iter().filter(|e| e["allDay"] != json!(true) && local(at(e, "startAt"), zone)[..10] == *date).collect();
        rows.sort_by(|a, b| at(a, "startAt").total_cmp(&at(b, "startAt")).then(at(b, "endAt").total_cmp(&at(a, "endAt"))));
        let (mut cluster, mut ends, mut cluster_end): (Vec<(String, usize)>, Vec<f64>, f64) = (Vec::new(), Vec::new(), 0.0);
        let mut flush = |cluster: &mut Vec<(String, usize)>, ends: &mut Vec<f64>| {
            let lanes = ends.len().max(1);
            for (key, lane) in cluster.drain(..) {
                result.insert(key, (lane, lanes));
            }
            ends.clear();
        };
        for e in rows {
            let start = at(e, "startAt");
            let end = at(e, "endAt").max(start + 900_000.0);
            if !cluster.is_empty() && start >= cluster_end {
                flush(&mut cluster, &mut ends);
            }
            let lane = match ends.iter().position(|v| *v <= start) {
                Some(l) => {
                    ends[l] = end;
                    l
                }
                None => {
                    ends.push(end);
                    ends.len() - 1
                }
            };
            cluster.push((format!("{date}/{}", occurrence_key(e)), lane));
            cluster_end = cluster_end.max(end);
        }
        flush(&mut cluster, &mut ends);
    }
    result
}

fn on_day(event: &Value, date: &str, zone: &str) -> bool {
    let from = local(at(event, "startAt"), zone)[..10].to_owned();
    let end = local(at(event, "endAt"), zone);
    let to = &end[..10];
    from.as_str() <= date && (to > date || to == date && (&end[11..] != "00:00" || event["startAt"] == event["endAt"]))
}

#[derive(Deserialize)]
struct Calendars {
    items: Vec<Collection>,
}

#[derive(Clone)]
struct Editor {
    data: Value,
    resource: Option<Item>,
    occurrence: Option<String>,
    collection_id: String,
}

#[component]
pub fn Calendar(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let pim = use_pim(on_signed_out);
    let params = web_sys::UrlSearchParams::new_with_str(&window().location().search().unwrap_or_default()).expect("params");
    let guessed = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new()).resolved_options();
    let guessed = js_sys::Reflect::get(&guessed, &"timeZone".into()).ok().and_then(|v| v.as_string()).unwrap_or_else(|| "UTC".into());
    let shared = context().preferences;
    let prefs: RwSignal<Value> = RwSignal::new(json!({ "revision": 0, "timezone": "UTC", "weekStart": 1, "hiddenCalendars": [] }));
    let calendars: RwSignal<Vec<Collection>> = RwSignal::new(Vec::new());
    let events: RwSignal<Vec<Value>> = RwSignal::new(Vec::new());
    let reminders: RwSignal<Vec<Value>> = RwSignal::new(Vec::new());
    let date_param = params.get("date").filter(|d| d.len() == 10 && d.as_bytes()[4] == b'-');
    let initial_date = date_param.is_some();
    let anchor = RwSignal::new(date_param.unwrap_or_else(|| local(js_sys::Date::now(), "UTC")[..10].to_owned()));
    let view_mode = RwSignal::new(params.get("view").filter(|v| ["day", "week", "month", "agenda"].contains(&v.as_str())).unwrap_or_else(|| "month".into()));
    let selected: RwSignal<Option<Value>> = RwSignal::new(None);
    let detail: RwSignal<Option<Item>> = RwSignal::new(None);
    let editor: RwSignal<Option<Editor>> = RwSignal::new(None);
    let scope: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let modal: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let managed = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let loading = RwSignal::new(true);
    let busy = RwSignal::new(false);
    let delete_count: RwSignal<Option<usize>> = RwSignal::new(None);
    let action_id = RwSignal::new(String::new());
    let calendar_page = RwSignal::new(0usize);
    let page_size = RwSignal::new(6usize);
    let month_limit = RwSignal::new(2usize);
    let dragged: StoredValue<Option<(Value, bool)>> = StoredValue::new(None);
    let grid = NodeRef::<html::Div>::new();
    let month_grid = NodeRef::<html::Div>::new();
    let action_sequence = RwSignal::new(0u32);
    let initial_zone = StoredValue::new(false);
    let initial_event = StoredValue::new((params.get("book"), params.get("event")));
    let loaded_range = StoredValue::new(String::new());
    let loaded_detail = StoredValue::new(String::new());
    let sequence = RwSignal::new(0u32);
    let grid_period = StoredValue::new(String::new());
    let session = StoredValue::new(session);
    let csrf = move || session.with_value(|s| s.csrf_token.clone());

    let zone = Memo::new(move |_| prefs.with(|p| p["timezone"].as_str().map(str::to_owned)).unwrap_or_else(|| guessed.clone()));
    let week_start = Signal::derive(move || prefs.with(|p| p["weekStart"].as_u64().unwrap_or(1) as u32));
    let show_cancelled = Signal::derive(move || prefs.with(|p| p["calendarShowCancelled"] == json!(true)));
    let show_tasks = Signal::derive(move || prefs.with(|p| p["calendarShowTasks"] != json!(false)));
    let hidden = Memo::new(move |_| prefs.with(|p| p["hiddenCalendars"].as_array().map(|h| h.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect::<Vec<_>>()).unwrap_or_default()));
    let window_range = Memo::new(move |_| range(&anchor.get(), &view_mode.get(), week_start.get()));
    let days = Memo::new(move |_| {
        let (start, count) = window_range.get();
        (0..count).map(|i| add_days(&start, i)).collect::<Vec<_>>()
    });
    let today = Memo::new(move |_| local(js_sys::Date::now(), &zone.get())[..10].to_owned());
    let loaded_prefs = Memo::new(move |_| shared.value.with(|v| v.as_ref().map(|p| p.revision)));
    let ready_to_create = Memo::new(move |_| !loading.get() && loaded_prefs.get().is_some_and(|r| prefs.with(|p| p["revision"].as_u64() == Some(r))));

    Effect::new(move |_| {
        let Some(p) = shared.value.get() else { return };
        let mut value = serde_json::to_value(&p).unwrap_or_default();
        if value["timezone"].is_null() {
            value["timezone"] = json!("UTC");
        }
        let zone_now = value["timezone"].as_str().unwrap_or("UTC").to_owned();
        prefs.set(value);
        if !initial_zone.get_value() && !initial_date {
            anchor.set(local(js_sys::Date::now(), &zone_now)[..10].to_owned());
            initial_zone.set_value(true);
        }
    });
    // Occurrences of every visible calendar, covering day boundaries in any zone.
    Effect::new(move |_| {
        pim.epoch.track();
        let ((start, count), z, cancelled, tasks, hide) = (window_range.get(), zone.get(), show_cancelled.get(), show_tasks.get(), hidden.get());
        let key = json!([start, count, z, hide]).to_string();
        if loaded_range.get_value() != key {
            loading.set(true);
        }
        let id = sequence.get_untracked() + 1;
        sequence.set(id);
        spawn_local(async move {
            let list = api::get::<Calendars>(BASE).await;
            let inbox = api::get::<Value>("/api/v1/pim/reminders").await;
            if sequence.try_get_untracked() != Some(id) {
                return;
            }
            let (list, inbox) = match (list, inbox) {
                (Ok(l), Ok(i)) => (l, i),
                (Err(e), _) | (_, Err(e)) => {
                    error.set(e.message);
                    events.set(Vec::new());
                    loading.set(false);
                    return;
                }
            };
            let from = js_sys::Date::new(&JsValue::from_f64(parse_ms(&format!("{start}T00:00:00Z")) - 86_400_000.0)).to_iso_string().as_string().unwrap_or_default();
            let to = js_sys::Date::new(&JsValue::from_f64(parse_ms(&format!("{}T00:00:00Z", add_days(&start, count))) + 86_400_000.0)).to_iso_string().as_string().unwrap_or_default();
            // Up to four calendars load at once, as in React; results keep the calendars' order.
            let visible: Vec<Collection> = list.items.iter().filter(|c| !hide.contains(&c.id)).cloned().collect();
            let query = web_sys::UrlSearchParams::new().expect("params");
            for (k, v) in [("from", from.as_str()), ("to", to.as_str()), ("timezone", z.as_str()), ("includeCancelled", if cancelled { "1" } else { "0" }), ("includeTasks", if tasks { "1" } else { "0" })] {
                query.append(k, v);
            }
            let query = query.to_string().as_string().unwrap_or_default();
            let next = std::rc::Rc::new(std::cell::Cell::new(0usize));
            let results: std::rc::Rc<std::cell::RefCell<Vec<Option<Result<Vec<Value>, ()>>>>> = std::rc::Rc::new(std::cell::RefCell::new(vec![None; visible.len()]));
            let workers = (0..visible.len().min(4)).map(|_| {
                let (next, results, visible, query) = (next.clone(), results.clone(), &visible, &query);
                async move {
                    loop {
                        let index = next.get();
                        if index >= visible.len() {
                            break;
                        }
                        next.set(index + 1);
                        let result = api::get::<Value>(&format!("{BASE}/{}/occurrences?{query}", visible[index].id)).await;
                        results.borrow_mut()[index] = Some(result.map(|v| v["items"].as_array().cloned().unwrap_or_default()).map_err(|_| ()));
                    }
                }
            });
            futures::future::join_all(workers).await;
            let mut batches = Vec::new();
            let mut failed: Vec<Collection> = Vec::new();
            for (c, result) in visible.iter().zip(results.take()) {
                match result {
                    Some(Ok(items)) => batches.extend(items),
                    _ => failed.push(c.clone()),
                }
            }
            if sequence.try_get_untracked() != Some(id) {
                return;
            }
            let same = loaded_range.get_value() == key;
            loaded_range.set_value(key);
            if same {
                let kept: Vec<Value> = events.get_untracked().into_iter().filter(|e| failed.iter().any(|c| c.id == s(e, "collectionId"))).collect();
                batches.extend(kept);
            }
            if !list.items.iter().any(|c| c.id == managed.get_untracked()) {
                managed.set(list.items.first().map(|c| c.id.clone()).unwrap_or_default());
            }
            calendars.set(list.items);
            events.set(batches);
            reminders.set(inbox["items"].as_array().cloned().unwrap_or_default());
            error.set(if failed.is_empty() { String::new() } else { format!("Could not refresh {}.{} Try Refresh.", failed.iter().map(|c| c.name.clone()).collect::<Vec<_>>().join(", "), if same { " Previously loaded events in this window have been kept." } else { "" }) });
            loading.set(false);
        });
    });
    // `?book=…&event=…` opens that event once it is loaded.
    Effect::new(move |_| {
        let (book, href) = initial_event.get_value();
        let (Some(book), Some(href)) = (book, href) else { return };
        if loading.get() {
            return;
        }
        if let Some(found) = events.with(|e| e.iter().find(|e| s(e, "collectionId") == book && s(e, "href") == href).cloned()) {
            initial_event.set_value((None, None));
            selected.set(Some(found));
        }
    });
    Effect::new(move |_| {
        pim.epoch.track();
        let current = selected.get();
        let key = current.as_ref().map(|e| format!("{}/{}", s(e, "collectionId"), s(e, "href"))).unwrap_or_default();
        if loaded_detail.get_value() != key {
            detail.set(None);
        }
        loaded_detail.set_value(key.clone());
        let Some(e) = current else { return };
        spawn_local(async move {
            let result = api::get::<Item>(&format!("{BASE}/{}/items/{}", s(&e, "collectionId"), s(&e, "href"))).await;
            if loaded_detail.try_get_value().as_deref() != Some(key.as_str()) {
                return;
            }
            match result {
                Ok(row) => detail.set(Some(row)),
                Err(err) => {
                    selected.set(None);
                    error.set(err.message);
                }
            }
        });
    });
    Effect::new(move |_| {
        let params = web_sys::UrlSearchParams::new().expect("params");
        params.append("date", &anchor.get());
        params.append("view", &view_mode.get());
        let _ = window().history().expect("history").replace_state_with_url(&JsValue::NULL, "", Some(&format!("/calendar?{}", params.to_string().as_string().unwrap_or_default())));
    });
    // The time grid opens at 07:00 once per period.
    Effect::new(move |_| {
        let period = format!("{}/{}", view_mode.get(), window_range.get().0);
        pim.state.track();
        if let (Some(g), false) = (grid.get(), loading.get())
            && grid_period.get_value() != period {
                g.set_scroll_top(7 * 64);
                grid_period.set_value(period);
            }
    });
    Effect::new(move |_| {
        let list = events.get();
        if loading.get_untracked() {
            return;
        }
        if let Some(current) = selected.get_untracked() {
            selected.set(list.into_iter().find(|e| occurrence_key(e) == occurrence_key(&current)));
        }
    });
    let resize_pages = move || {
        let (w, h) = (window().inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1000.0), window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(900.0));
        page_size.set(if w <= 800.0 { 1000 } else { (((h - 56.0 - 560.0) / 42.0).floor() as usize).clamp(1, 8) });
    };
    resize_pages();
    crate::ui::listen(&window(), "resize", false, move |_| resize_pages());
    Effect::new(move |_| {
        let (list, size) = (calendars.get(), page_size.get());
        if let Some(index) = list.iter().position(|c| c.id == managed.get()) {
            calendar_page.set(index / size);
        }
        calendar_page.update(|p| *p = (*p).min(list.len().div_ceil(size).max(1) - 1));
    });
    // Rows per month day follow the grid's height as it lays out and resizes.
    Effect::new(move |_| {
        let Some(node) = month_grid.get() else { return };
        let measured = node.clone();
        let measure = move || {
            let height = measured.client_height() as f64;
            let limit = (((height - 28.0) / 6.0 - 48.0) / 22.0).floor().clamp(0.0, 3.0) as usize;
            if month_limit.try_get_untracked().is_some_and(|current| current != limit) {
                month_limit.set(limit);
            }
        };
        let callback = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(measure);
        let observer = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).expect("ResizeObserver");
        observer.observe(&node);
        let kept = send_wrapper::SendWrapper::new((observer, callback));
        on_cleanup(move || kept.take().0.disconnect());
    });

    let run = move |work: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>| {
        spawn_local(async move {
            busy.set(true);
            error.set(String::new());
            match work.await {
                Ok(()) => {
                    pim.refresh();
                    modal.set(None);
                    scope.set(None);
                }
                Err(e) => error.set(e),
            }
            busy.set(false);
        });
    };
    let new_event = move |date: String, hour: u32| {
        if !ready_to_create.get_untracked() {
            return;
        }
        let editable = |c: &Collection| c.role != "read" && c.subscription.is_none();
        let target = calendars.with_untracked(|list| list.iter().find(|c| c.id == managed.get_untracked() && editable(c)).or_else(|| list.iter().find(|c| editable(c))).cloned());
        let Some(target) = target else {
            modal.set(Some("newCalendar"));
            return;
        };
        let end = if hour == 23 { format!("{}T00:00", add_days(&date, 1)) } else { format!("{date}T{:02}:00", hour + 1) };
        editor.set(Some(Editor {
            collection_id: target.id,
            resource: None,
            occurrence: None,
            data: json!({ "title": "", "start": format!("{date}T{hour:02}:00"), "end": end, "timezone": zone.get_untracked(), "allDay": false, "location": "", "description": "", "recurrence": "", "attendees": [], "reminderMinutes": 10, "status": "CONFIRMED" }),
        }));
    };
    let edit_occurrence = move |whole: bool| {
        let (Some(d), Some(sel)) = (detail.get_untracked(), selected.get_untracked()) else { return };
        editor.set(Some(Editor { collection_id: d.collection_id.clone(), occurrence: (!whole).then(|| s(&sel, "recurrenceId")), data: if whole { d.data.clone() } else { sel }, resource: Some(d) }));
        scope.set(None);
    };
    let refresh_subscription = move |c: Collection| {
        spawn_local(async move {
            match send::<Value>(&format!("{BASE}/{}/refresh", c.id), "POST", None, &csrf(), None).await {
                Ok(_) => {
                    notice.set(format!("Refreshing {}…", c.name));
                    set_timeout(move || pim.refresh(), std::time::Duration::from_millis(4000));
                }
                Err(e) => error.set(e.message),
            }
        });
    };
    let calendar_action = move |c: Collection, kind: &'static str| {
        let id = action_sequence.get_untracked() + 1;
        action_sequence.set(id);
        action_id.set(c.id.clone());
        error.set(String::new());
        modal.set(Some(kind));
        if kind == "deleteCalendar" {
            delete_count.set(None);
            spawn_local(async move {
                match api::get::<Value>(&format!("{BASE}/{}", c.id)).await {
                    Ok(v) if action_sequence.try_get_untracked() == Some(id) => delete_count.set(v["count"].as_u64().map(|c| c as usize)),
                    Err(e) => error.set(e.message),
                    _ => {}
                }
            });
        }
    };
    let current = Memo::new(move |_| {
        let id = { let a = action_id.get(); if a.is_empty() { managed.get() } else { a } };
        calendars.with(|c| c.iter().find(|x| x.id == id).cloned())
    });
    let selected_calendar = Memo::new(move |_| selected.with(|sel| sel.as_ref().and_then(|e| calendars.with(|c| c.iter().find(|x| x.id == s(e, "collectionId")).cloned()))));
    let can_edit = Memo::new(move |_| selected_calendar.with(|c| c.as_ref().is_some_and(|c| c.role != "read" && c.subscription.is_none())));
    let drop_at = move |date: String, hour: Option<u32>| {
        let Some((event, resize)) = dragged.get_value() else { return };
        dragged.set_value(None);
        let z = zone.get_untracked();
        run(Box::pin(async move {
            let resource: Item = api::get(&format!("{BASE}/{}/items/{}", s(&event, "collectionId"), s(&event, "href"))).await.map_err(|e| e.message)?;
            let old_start = local(at(&event, "startAt"), &z);
            let all_day = event["allDay"] == json!(true);
            let new_start = if resize { old_start.clone() } else if all_day { date.clone() } else { format!("{date}T{}", hour.map(|h| format!("{h:02}:00")).unwrap_or_else(|| old_start[11..16].to_owned())) };
            let duration = at(&event, "endAt") - at(&event, "startAt");
            let naive = parse_ms(&if new_start.len() == 10 { format!("{new_start}T00:00:00Z") } else { format!("{}:00Z", &new_start[..16]) });
            let new_end = if resize { format!("{date}T{:02}:00", hour.unwrap_or(10)) } else {
                let iso = js_sys::Date::new(&JsValue::from_f64(naive + duration)).to_iso_string().as_string().unwrap_or_default();
                iso[..if all_day { 10 } else { 16 }].to_owned()
            };
            let recurring = resource.data["recurring"] == json!(true) || !s(&resource.data, "recurrence").is_empty();
            let mut data = event.clone();
            data["start"] = json!(new_start);
            data["end"] = json!(new_end);
            if !all_day {
                data["timezone"] = json!(z);
            }
            editor.set(Some(Editor { collection_id: resource.collection_id.clone(), occurrence: recurring.then(|| s(&event, "recurrenceId")), data, resource: Some(resource) }));
            notice.set("Review the moved or resized event and save. The keyboard editor offers the same controls.".into());
            Ok(())
        }));
    };
    let event_button = move |event: Value, layout: Option<(usize, usize)>| {
        let z = zone.get_untracked();
        let calendar = calendars.with_untracked(|c| c.iter().find(|x| x.id == s(&event, "collectionId")).cloned());
        let color = calendar.as_ref().map(|c| c.color.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| "currentColor".into());
        let start = local(at(&event, "startAt"), &z);
        let minutes: f64 = start[11..13].parse::<f64>().unwrap_or(0.0) * 60.0 + start[14..16].parse::<f64>().unwrap_or(0.0);
        let all_day = event["allDay"] == json!(true);
        let title = s(&event, "title");
        if event["task"] == json!(true) {
            let done = s(&event, "status") == "COMPLETED";
            let block = layout.map(|(lane, lanes)| format!(" ev-at-{} ev-span-2 ev-lane-{} ev-lanes-{}", (minutes as i64 % 60) / 5, lane.min(3), lanes.min(4))).unwrap_or_default();
            let label = format!("Task: {title}{}{} in {}", if done { ", completed" } else { "" }, if all_day { ", due".to_owned() } else { format!(", due at {}", &start[11..16]) }, calendar.as_ref().map(|c| c.name.clone()).unwrap_or_else(|| "a task list".into()));
            return view! {
                <div class=format!("calendar-event-wrap is-task{block}")>
                    <a class=format!("calendar-event calendar-task{}{}", if all_day { " all-day" } else { "" }, if done { " is-done" } else { "" }) href=format!("/tasks?item={}", js_sys::encode_uri_component(&s(&event, "resourceId"))) aria-label=label>
                        <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                        <span class="calendar-task-check" aria-hidden="true"><Icon name=if done { "check" } else { "checklist" }/></span>
                        <span class="calendar-event-time">{if all_day { "Due".to_owned() } else { start[11..16].to_owned() }}</span><strong>{title.clone()}</strong>
                    </a>
                </div>
            }
            .into_any();
        }
        let writable = calendar.as_ref().is_some_and(|c| c.role != "read" && c.subscription.is_none());
        let duration = ((at(&event, "endAt") - at(&event, "startAt")) / 900_000.0).ceil();
        let span = duration.min(96.0 - (minutes / 15.0).floor()).max(1.0) as i64;
        let block = layout.map(|(lane, lanes)| format!(" ev-at-{} ev-span-{span} ev-lane-{} ev-lanes-{}", (minutes as i64 % 60) / 5, lane.min(3), lanes.min(4))).unwrap_or_default();
        let key = occurrence_key(&event);
        let (k1, k2) = (key.clone(), key.clone());
        let is_selected = move || selected.with(|sel| sel.as_ref().is_some_and(|e| occurrence_key(e) == k1));
        let is_selected2 = move || selected.with(|sel| sel.as_ref().is_some_and(|e| occurrence_key(e) == k2));
        let cancelled = s(&event, "status") == "CANCELLED";
        let repeats = event["recurring"] == json!(true) || !s(&event, "recurrence").is_empty();
        let (e1, e2, e3, e4) = (event.clone(), event.clone(), event.clone(), event.clone());
        let resize_label = format!("Resize {title}");
        view! {
            <div class=move || format!("calendar-event-wrap{block}{}", if is_selected() { " is-selected" } else { "" })>
                <button class=format!("calendar-event {} {}", if all_day { "all-day" } else { "" }, if cancelled { "cancelled" } else { "" }) aria-pressed=move || is_selected2().to_string() draggable=writable.to_string()
                    on:dragstart=move |e: web_sys::DragEvent| { dragged.set_value(Some((e1.clone(), false))); if let Some(d) = e.data_transfer() { let _ = d.set_data("text/plain", &s(&e1, "resourceId")); } }
                    on:dragend=move |_| dragged.set_value(None)
                    on:click=move |_| selected.set(Some(e2.clone()))>
                    <svg class="calendar-event-tint" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                    <svg class="calendar-event-edge" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"><rect width="1" height="1" fill=color.clone()></rect></svg>
                    <CollectionDot color=calendar.as_ref().map(|c| c.color.clone()).unwrap_or_default() class="calendar-event-dot"/>
                    <span class="calendar-event-time">{if all_day { "All day".to_owned() } else { start[11..16].to_owned() }}</span><strong>{title.clone()}</strong>
                    {repeats.then(|| view! { <span class="calendar-event-repeat" aria-label="Repeats"><Icon name="repeat"/></span> })}
                </button>
                {(writable && !all_day).then(|| view! {
                    <button class="calendar-resize" draggable="true" aria-label=resize_label.clone() on:click=move |_| selected.set(Some(e3.clone()))
                        on:dragstart=move |e: web_sys::DragEvent| { dragged.set_value(Some((e4.clone(), true))); if let Some(d) = e.data_transfer() { let _ = d.set_data("text/plain", &s(&e4, "resourceId")); } }
                        on:dragend=move |_| dragged.set_value(None)>"↕"</button>
                })}
            </div>
        }
        .into_any()
    };

    let sidebar: ChildrenFn = std::sync::Arc::new(move || {
        let range_days = Signal::derive(move || match view_mode.get().as_str() {
            "week" => Some(days.get()),
            "day" => Some(vec![anchor.get()]),
            _ => None,
        });
        view! {
            <button type="button" class="new-button ui-new" disabled=move || !ready_to_create.get() on:click=move |_| new_event(anchor.get_untracked(), 9)><Icon name="plus"/>"New event"</button>
            <MiniMonth value=anchor today=today week_start=week_start range=range_days on_change=Callback::new(move |d| anchor.set(d))/>
            <div class="pim-sidebar-heading"><strong>"Calendars"</strong>
                <ActionMenu label="Add calendar" class="calendar-add-menu" actions=move |_| vec![MenuAction::new("New calendar", "calendar").run(move || modal.set(Some("newCalendar"))), MenuAction::new("Subscribe by link", "link").run(move || modal.set(Some("subscribe")))]><Icon name="plus"/></ActionMenu>
            </div>
            <div class="calendar-list">
                {move || {
                    let (p, size) = (calendar_page.get(), page_size.get());
                    let hide = hidden.get();
                    calendars.get().into_iter().skip(p * size).take(size).map(|c| {
                        let shown = !hide.contains(&c.id);
                        let id = c.id.clone();
                        let (toggle_id, pick) = (c.id.clone(), c.id.clone());
                        let fill = if c.color.is_empty() { "currentColor".to_owned() } else { c.color.clone() };
                        let subscription = c.subscription.clone();
                        let failed = subscription.as_ref().map(|s| s["lastError"].as_str().unwrap_or_default().to_owned()).filter(|e| !e.is_empty());
                        let (c1, c2, c3, c4, c5) = (c.clone(), c.clone(), c.clone(), c.clone(), c.clone());
                        let subscribed = subscription.is_some();
                        view! {
                            <div class="calendar-nav-row">
                                <label class="calendar-visibility">
                                    <input aria-label=format!("Show {}", c.name) type="checkbox" prop:checked=shown on:change=move |e| {
                                        let checked = event_target_checked(&e);
                                        let mut list = hidden.get_untracked();
                                        if checked { list.retain(|h| *h != toggle_id); } else { list.push(toggle_id.clone()); }
                                        let revision = prefs.with_untracked(|p| p["revision"].as_u64().unwrap_or(0));
                                        let session = session.get_value();
                                        spawn_local(async move {
                                            match save_preferences(json!({ "revision": revision, "hiddenCalendars": list }), revision, &session).await {
                                                Ok(updated) => { prefs.set(serde_json::to_value(updated).unwrap_or_default()); pim.refresh(); }
                                                Err(e) => error.set(e.message),
                                            }
                                        });
                                    }/>
                                    <svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1" y="1" width="14" height="14" rx="3.5" fill=fill></rect><path d="m4.5 8.2 2.3 2.3 4.7-5" fill="none"></path></svg>
                                </label>
                                <button class=move || if managed.get() == id { "nav-item active" } else { "nav-item " } aria-pressed=move || (managed.get() == pick).to_string() on:click=move |_| managed.set(c1.id.clone())>
                                    <span>{c.name.clone()}</span>
                                    {subscription.map(|_| view! {
                                        <span class=if failed.is_some() { "calendar-subscribed has-error" } else { "calendar-subscribed" } title=failed.clone().unwrap_or_else(|| "Subscribed by link".into())>
                                            <Icon name=if failed.is_some() { "alert" } else { "link" }/><span class="sr-only">{failed.clone().map(|e| format!("Refresh failed: {e}")).unwrap_or_else(|| "Subscribed by link".into())}</span>
                                        </span>
                                    })}
                                </button>
                                {(c.role == "owner").then(|| view! {
                                    <ActionMenu label=format!("Actions for calendar {}", c.name) class="calendar-actions" actions=move |_| {
                                        let mut list = if subscribed {
                                            vec![MenuAction::new("Refresh now", "refresh").run({ let c = c2.clone(); move || refresh_subscription(c.clone()) }), MenuAction::new("Edit subscription", "pencil").run({ let c = c2.clone(); move || { action_id.set(c.id.clone()); modal.set(Some("editSubscription")); } })]
                                        } else {
                                            vec![MenuAction::new("Edit calendar", "pencil").run({ let c = c3.clone(); move || calendar_action(c.clone(), "renameCalendar") })]
                                        };
                                        list.push(MenuAction::new("Share calendar", "share").run({ let c = c4.clone(); move || calendar_action(c.clone(), "share") }));
                                        list.push(MenuAction::new(if subscribed { "Unsubscribe" } else { "Delete calendar" }, "trash").danger().run({ let c = c5.clone(); move || calendar_action(c.clone(), "deleteCalendar") }));
                                        list
                                    }><Icon name="more"/></ActionMenu>
                                })}
                            </div>
                        }
                    }).collect_view()
                }}
            </div>
            {move || (calendars.with(Vec::len) > page_size.get()).then(|| view! {
                <nav class="calendar-list-pages" aria-label="Calendar list pages">
                    <button class="icon-button" aria-label="Previous calendars" disabled=move || calendar_page.get() == 0 on:click=move |_| calendar_page.update(|p| *p = p.saturating_sub(1))><Icon name="back"/></button>
                    <span>{move || calendar_page.get() + 1}" / "{move || calendars.with(Vec::len).div_ceil(page_size.get())}</span>
                    <button class="icon-button" aria-label="Next calendars" disabled={move || (calendar_page.get() + 1) * page_size.get() >= calendars.with(Vec::len)} on:click=move |_| calendar_page.update(|p| *p += 1)><Icon name="chevron"/></button>
                </nav>
            })}
            <button class="nav-item calendar-reminders" on:click=move |_| modal.set(Some("reminders"))><Icon name="bell"/>"Reminders "{move || (!reminders.with(Vec::is_empty)).then(|| view! { <span class="reminder-count">{reminders.with(Vec::len)}</span> })}</button>
        }
        .into_any()
    });

    let details: ChildrenFn = std::sync::Arc::new(move || {
        view! {
            <div class="pim-detail-top"><span>"Event"</span>
                {move || (detail.with(Option::is_some) && can_edit.get()).then(|| {
                    let recurring = detail.with(|d| d.as_ref().is_some_and(|d| d.data["recurring"] == json!(true) || !s(&d.data, "recurrence").is_empty()));
                    view! {
                        <button class="icon-button" aria-label="Edit event" title="Edit event" on:click=move |_| if recurring { scope.set(Some("edit")) } else { edit_occurrence(true) }><Icon name="pencil"/></button>
                        <button class="icon-button" aria-label="Delete event" title="Delete event" on:click=move |_| if recurring { scope.set(Some("delete")) } else { modal.set(Some("deleteEvent")) }><Icon name="trash"/></button>
                    }
                })}
                <button class="icon-button" aria-label="Close event details" on:click=move |_| selected.set(None)><Icon name="close"/></button>
            </div>
            {move || match (detail.get(), selected.get()) {
                (Some(d), Some(sel)) => {
                    let z = zone.get();
                    let cal = selected_calendar.get();
                    let color = cal.as_ref().map(|c| c.color.clone()).unwrap_or_default();
                    let all_day = sel["allDay"] == json!(true);
                    let tz = s(&sel, "timezone");
                    let times = if all_day { "All day".to_owned() } else { format!("{} – {} · {z}", &local(at(&sel, "startAt"), &z)[11..16], &local(at(&sel, "endAt"), &z)[11..16]) };
                    let origin = if tz == "floating" { " · Floating event".to_owned() } else if !all_day && tz != z { format!(" · Created in {tz}") } else { String::new() };
                    let state = s(&sel, "status");
                    let reminder = match sel["reminderMinutes"].as_i64() { None => "No reminder".to_owned(), Some(0) => "At start".to_owned(), Some(m) => format!("{} before", reminder_text(m)) };
                    let rule = s(&sel, "recurrence");
                    let attendees: Vec<String> = sel["attendees"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                    let description = s(&sel, "description");
                    let location = s(&sel, "location");
                    view! {
                        <div class="event-title"><CollectionDot color=color.clone() class="event-swatch"/><div><h2>{s(&sel, "title")}</h2><p>{event_when(&sel, &z)}</p><p class="muted small">{times}{origin}</p>
                            {(state != "CONFIRMED").then(|| view! { <span class=format!("status-chip {}", if state == "CANCELLED" { "danger" } else { "warning" })>{match state.as_str() { "CANCELLED" => "Cancelled".to_owned(), "TENTATIVE" => "Tentative".to_owned(), other => other.to_owned() }}</span> })}
                        </div></div>
                        <div class="property-card">
                            {(!location.is_empty()).then(|| view! { <div class="property"><Icon name="location"/><div><span>{location.clone()}</span></div></div> })}
                            <div class="property"><Icon name="repeat"/><div><span>{recurrence_label(&rule, sel["recurring"] == json!(true))}</span>{(!rule.is_empty()).then(|| view! { <details><summary>"Schedule details"</summary><code>{rule.clone()}</code></details> })}</div></div>
                            <div class="property"><Icon name="bell"/><div><span>{reminder}</span></div></div>
                            <div class="property"><Icon name="calendar"/><div><span class="property-value"><CollectionDot color=color.clone()/>{cal.as_ref().map(|c| c.name.clone())}</span></div></div>
                        </div>
                        {(!attendees.is_empty() || !description.is_empty()).then(|| view! {
                            <div class="property-card">
                                {(!attendees.is_empty()).then(|| view! { <div class="property"><Icon name="contacts"/><div>{attendees.iter().map(|a| view! { <span>{a.clone()}</span> }).collect_view()}<small>"Attendees are saved with the event. This app does not send invitation emails."</small></div></div> })}
                                {(!description.is_empty()).then(|| view! { <div class="property"><Icon name="note"/><div><p class="pim-notes">{description.clone()}</p></div></div> })}
                            </div>
                        })}
                        <div class="pim-detail-footer"><span></span><a class="text-button" href=format!("{BASE}/{}/items/{}?export=1", d.collection_id, d.href)><Icon name="download"/>"Export event"</a></div>
                    }
                    .into_any()
                }
                _ => view! { <p role="status">"Loading event…"</p> }.into_any(),
            }}
        }
        .into_any()
    });

    let main_view = move || {
        if loading.get() {
            return view! { <p role="status">"Loading calendar…"</p> }.into_any();
        }
        if calendars.with(Vec::is_empty) {
            return view! { <div class="pim-empty"><h2>"No calendars"</h2><p>"Create a calendar to add events or import an ICS file."</p><button class="primary-button" on:click=move |_| modal.set(Some("newCalendar"))>"Create calendar"</button></div> }.into_any();
        }
        let z = zone.get();
        let list = events.get();
        let all_days = days.get();
        let today_now = today.get();
        match view_mode.get().as_str() {
            "agenda" => {
                let any = list.iter().any(|e| all_days.iter().any(|d| on_day(e, d, &z)));
                view! {
                    <div class="calendar-agenda">
                        {all_days.iter().map(|date| {
                            let rows: Vec<Value> = list.iter().filter(|e| on_day(e, date, &z)).cloned().collect();
                            (!rows.is_empty()).then(|| view! { <section><h2>{day(date, &[("weekday", "long"), ("month", "short"), ("day", "numeric")])}</h2>{rows.into_iter().map(|e| event_button(e, None)).collect_view()}</section> })
                        }).collect_view()}
                        {(!any).then(|| view! { <p class="pim-empty">"No events in this window."</p> })}
                    </div>
                }
                .into_any()
            }
            "month" => {
                let anchor_month = anchor.get()[..7].to_owned();
                let limit = month_limit.get();
                view! {
                    <div node_ref=month_grid class="calendar-month" aria-label="Month calendar">
                        {all_days.iter().take(7).map(|d| view! { <div class="calendar-weekday">{day(d, &[("weekday", "short")])}</div> }).collect_view()}
                        {all_days.iter().map(|date| {
                            let rows: Vec<Value> = list.iter().filter(|e| on_day(e, date, &z)).cloned().collect();
                            let count = rows.len();
                            let (d1, d2, d3) = (date.clone(), date.clone(), date.clone());
                            view! {
                                <div class=format!("calendar-month-day {}", if date[..7] != anchor_month { "outside-month" } else { "" })
                                    on:dragover=|e: web_sys::DragEvent| e.prevent_default() on:drop=move |e: web_sys::DragEvent| { e.prevent_default(); drop_at(d1.clone(), None); }>
                                    <button class=format!("calendar-date {}", if *date == today_now { "today" } else { "" }) aria-label=format!("Create event on {date}") on:click=move |_| new_event(d2.clone(), 9)>{date[8..].trim_start_matches('0').to_owned()}</button>
                                    {rows.into_iter().take(limit).map(|e| event_button(e, None)).collect_view()}
                                    {(count > limit).then(|| view! {
                                        <button class="calendar-more" aria-label=format!("Show all events for {date}") on:click=move |_| { anchor.set(d3.clone()); view_mode.set("day".into()); }>
                                            {if limit > 0 { "+" } else { "" }}{count - limit}" "{if limit > 0 { "more" } else if count == 1 { "event" } else { "events" }}
                                        </button>
                                    })}
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }
                .into_any()
            }
            mode => {
                let lanes = day_lanes(&all_days, &list, &z);
                let now = local(js_sys::Date::now(), &z);
                view! {
                    <div node_ref=grid class=format!("calendar-time-scroll {}", if mode == "day" { "single-day" } else { "" }) tabindex="0" aria-label="Calendar time grid">
                        <div class="calendar-time-grid">
                            <div class="calendar-time-label" title=z.clone()>"Time"</div>
                            {all_days.iter().map(|date| view! {
                                <div class=format!("calendar-day-heading{}", if *date == today_now { " is-today" } else { "" }) aria-label=day(date, &[("weekday", "long"), ("month", "long"), ("day", "numeric")])>
                                    <span aria-hidden="true">{day(date, &[("weekday", "short")])}</span><b aria-hidden="true">{date[8..].trim_start_matches('0').to_owned()}</b>
                                </div>
                            }).collect_view()}
                            <div class="calendar-time-label">"All day"</div>
                            {all_days.iter().map(|date| {
                                let d = date.clone();
                                view! {
                                    <div class="calendar-slot all-day-slot" on:dragover=|e: web_sys::DragEvent| e.prevent_default() on:drop=move |e: web_sys::DragEvent| { e.prevent_default(); drop_at(d.clone(), None); }>
                                        {list.iter().filter(|e| e["allDay"] == json!(true) && on_day(e, date, &z)).cloned().map(|e| event_button(e, None)).collect_view()}
                                    </div>
                                }
                            }).collect_view()}
                            {(0..24u32).map(|hour| {
                                let hh = format!("{hour:02}");
                                view! {
                                    <div class="calendar-time-label calendar-hour-label">{hh.clone()}":00"</div>
                                    {all_days.iter().map(|date| {
                                        let (d1, d2) = (date.clone(), date.clone());
                                        let prefix = format!("{date}T{hh}");
                                        let now_mark = (*date == today_now && now[11..13] == hh).then(|| view! { <span class=format!("calendar-now ev-at-{}", now[14..16].parse::<u32>().unwrap_or(0) / 5) aria-hidden="true"></span> });
                                        view! {
                                            <div class=format!("calendar-slot{}", if *date == today_now { " is-today" } else { "" }) on:dragover=|e: web_sys::DragEvent| e.prevent_default() on:drop=move |e: web_sys::DragEvent| { e.prevent_default(); drop_at(d1.clone(), Some(hour)); }>
                                                <button class="calendar-slot-create" aria-label=format!("New event {date} {hh}:00") on:click=move |_| new_event(d2.clone(), hour)>"+"</button>
                                                {now_mark}
                                                {list.iter().filter(|e| e["allDay"] != json!(true) && local(at(e, "startAt"), &z)[..13] == prefix).cloned().map(|e| {
                                                    let layout = lanes.get(&format!("{date}/{}", occurrence_key(&e))).copied().unwrap_or((0, 1));
                                                    event_button(e, Some(layout))
                                                }).collect_view()}
                                            </div>
                                        }
                                    }).collect_view()}
                                }
                            }).collect_view()}
                        </div>
                    </div>
                }
                .into_any()
            }
        }
    };
    let save = Callback::new(move |(data, collection_id, done): (Value, String, Done)| {
        let Some(e) = editor.get_untracked() else { return };
        spawn_local(async move {
            let (url, method, tag) = match &e.resource {
                Some(r) => (format!("{BASE}/{}/items/{}", r.collection_id, r.href), "PUT", Some(etag(&r.id, r.revision))),
                None => (format!("{BASE}/{collection_id}/items"), "POST", None),
            };
            let mut body = json!({ "data": data });
            if let Some(o) = &e.occurrence {
                body["occurrenceId"] = json!(o);
            }
            match send::<Item>(&url, method, Some(body), &csrf(), tag).await {
                Ok(row) => {
                    editor.set(None);
                    selected.set(None);
                    pim.refresh();
                    notice.set(format!("{} saved.", s(&row.data, "title")));
                    { done.try_run(Ok(())); };
                }
                Err(err) => { done.try_run(Err(err.message)); },
            }
        });
    });
    let time_grid_class = Signal::derive(move || !loading.get() && !calendars.with(Vec::is_empty) && !["agenda", "month"].contains(&view_mode.get().as_str()));
    let shell_session = session.get_value();
    view! {
        <PimShell kind="calendar" main_class="calendar-main" session=shell_session on_signed_out=on_signed_out sidebar=sidebar has_details=Signal::from(Memo::new(move |_| selected.with(Option::is_some))) details=details pim=pim>
            <TimeGridClass active=time_grid_class/>
            <div class="calendar-heading"><h1>{move || period_title(&anchor.get(), &view_mode.get(), &days.get())}</h1>
                <div class="calendar-controls">
                    <button class="icon-button" aria-label="Previous period" title="Previous" on:click=move |_| anchor.set(shift(&anchor.get_untracked(), &view_mode.get_untracked(), -1))><Icon name="back"/></button>
                    <DatePopover value=anchor today=today week_start=week_start on_change=Callback::new(move |d| anchor.set(d))/>
                    <button class="icon-button" aria-label="Next period" title="Next" on:click=move |_| anchor.set(shift(&anchor.get_untracked(), &view_mode.get_untracked(), 1))><Icon name="chevron"/></button>
                    <div class="segmented calendar-view" role="radiogroup" aria-label="Calendar view" on:keydown=move |e| {
                        let order = ["day", "week", "month", "agenda"];
                        let index = order.iter().position(|v| *v == view_mode.get_untracked()).unwrap_or(2);
                        let key = e.key();
                        if key == "ArrowRight" || key == "ArrowLeft" {
                            e.prevent_default();
                            let next = order[(index + if key == "ArrowRight" { 1 } else { 3 }) % 4];
                            view_mode.set(next.into());
                            if let Some(button) = e.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).and_then(|t| t.query_selector(&format!("[data-value=\"{next}\"]")).ok().flatten()).and_then(|b| b.dyn_into::<web_sys::HtmlElement>().ok()) { let _ = button.focus(); }
                        }
                    }>
                        {[("day", "Day"), ("week", "Week"), ("month", "Month"), ("agenda", "Agenda")].into_iter().map(|(value, label)| view! {
                            <button type="button" role="radio" data-value=value aria-checked=move || (view_mode.get() == value).to_string() tabindex=move || if view_mode.get() == value { "0" } else { "-1" } on:click=move |_| view_mode.set(value.into())>{label}</button>
                        }).collect_view()}
                    </div>
                </div>
            </div>
            {move || (!shared.error.get().is_empty()).then(|| view! { <p role="alert" class="error">{shared.error.get()}</p> })}
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}<button class="text-button" on:click=move |_| pim.refresh()>"Refresh"</button></p> })}
            {move || (!notice.get().is_empty()).then(|| view! { <p role="status" class="notice">{notice.get()}</p> })}
            {main_view}
            {move || editor.get().map(|e| {
                let writable: Vec<Collection> = calendars.get_untracked().into_iter().filter(|c| c.role != "read" && c.subscription.is_none()).collect();
                view! { <EventEditor initial=e calendars=writable on_close=Callback::new(move |_| editor.set(None)) on_save=save/> }
            })}
            {move || scope.get().map(|kind| view! {
                <Dialog title=if kind == "edit" { "Edit repeating event" } else { "Delete repeating event" } on_close=Callback::new(move |_| scope.set(None))>
                    <p>"Choose how much of the series to change."</p>
                    {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                    <div class="pim-choice-actions">
                        <button class="secondary-button" disabled=move || busy.get() on:click=move |_| {
                            if kind == "edit" { edit_occurrence(false); return; }
                            let (Some(d), Some(sel)) = (detail.get_untracked(), selected.get_untracked()) else { return };
                            run(Box::pin(async move {
                                send::<Value>(&format!("{BASE}/{}/items/{}", d.collection_id, d.href), "PUT", Some(json!({ "cancelOccurrence": s(&sel, "recurrenceId") })), &csrf(), Some(etag(&d.id, d.revision))).await.map_err(|e| e.message)?;
                                selected.set(None);
                                Ok(())
                            }));
                        }>"This event only"</button>
                        <button class="secondary-button" disabled=move || busy.get() on:click=move |_| if kind == "edit" { edit_occurrence(true) } else { scope.set(None); modal.set(Some("deleteEvent")); }>"Whole series"</button>
                        <button class="text-button" on:click=move |_| scope.set(None)>"Cancel"</button>
                    </div>
                </Dialog>
            })}
            {move || matches!(modal.get(), Some("newCalendar" | "renameCalendar")).then(|| {
                let rename = modal.get_untracked() == Some("renameCalendar");
                let target = if rename { current.get_untracked() } else { None };
                let t2 = target.clone();
                view! {
                    <CollectionEditor title=if rename { "Edit calendar" } else { "New calendar" } collection=target.unwrap_or_else(|| Collection { id: String::new(), owner_id: String::new(), kind: String::new(), name: String::new(), color: "#73396f".into(), revision: 0, role: String::new(), subscription: None }) on_close=Callback::new(move |_| modal.set(None))
                        on_save=Callback::new(move |(name, color, done): (String, String, Done)| {
                            let t = t2.clone();
                            spawn_local(async move {
                                let (url, method, m) = match &t { Some(t) => (format!("{BASE}/{}", t.id), "PUT", Some(etag(&t.id, t.revision))), None => (BASE.to_owned(), "POST", None) };
                                match send::<Collection>(&url, method, Some(json!({ "name": name, "color": color })), &csrf(), m).await {
                                    Ok(row) => { managed.set(row.id); pim.refresh(); { done.try_run(Ok(())); }; }
                                    Err(e) => { done.try_run(Err(e.message)); },
                                }
                            });
                        })/>
                }
            })}
            {move || {
                let m = modal.get();
                let editing = m == Some("editSubscription") && current.with(|c| c.as_ref().is_some_and(|c| c.subscription.is_some()));
                (m == Some("subscribe") || editing).then(|| {
                    let target = if editing { current.get_untracked() } else { None };
                    let t2 = target.clone();
                    view! {
                        <SubscriptionEditor calendar=target on_close=Callback::new(move |_| modal.set(None))
                            on_refresh=Callback::new(move |_| if let Some(c) = current.get_untracked() { refresh_subscription(c) })
                            on_save=Callback::new(move |(value, done): (Value, Done)| {
                                let t = t2.clone();
                                spawn_local(async move {
                                    let result: Result<(), String> = async {
                                        match &t {
                                            None => {
                                                let row: Collection = send("/api/v1/calendar/subscriptions", "POST", Some(value.clone()), &csrf(), None).await.map_err(|e| e.message)?;
                                                managed.set(row.id.clone());
                                                notice.set(format!("Subscribed to {}. Events appear after the first refresh.", row.name));
                                            }
                                            Some(c) => {
                                                if value["name"] != c.name.as_str() || value["color"] != c.color.as_str() {
                                                    send::<Value>(&format!("{BASE}/{}", c.id), "PUT", Some(json!({ "name": value["name"], "color": value["color"] })), &csrf(), Some(etag(&c.id, c.revision))).await.map_err(|e| e.message)?;
                                                }
                                                send::<Value>(&format!("{BASE}/{}/subscription", c.id), "PUT", Some(json!({ "url": value["url"], "refreshMinutes": value["refreshMinutes"] })), &csrf(), None).await.map_err(|e| e.message)?;
                                                notice.set(format!("{} saved.", s(&value, "name")));
                                            }
                                        }
                                        Ok(())
                                    }.await;
                                    if result.is_ok() { pim.refresh(); }
                                    { done.try_run(result); };
                                });
                            })/>
                    }
                })
            }}
            {move || (modal.get() == Some("share")).then(|| current.get_untracked().map(|c| {
                let base = format!("{BASE}/{}", c.id);
                view! { <Sharing collection=c base=base session=session.get_value() on_close=Callback::new(move |_| modal.set(None)) on_changed=Callback::new(move |_| pim.refresh())/> }
            })).flatten()}
            {move || matches!(modal.get(), Some("deleteCalendar" | "deleteEvent")).then(|| {
                let whole = modal.get_untracked() == Some("deleteCalendar");
                view! {
                    <Dialog title=if whole { "Delete calendar?" } else { "Delete event?" } on_close=Callback::new(move |_| modal.set(None))>
                        <p>{move || if whole {
                            match delete_count.get() { None => "Loading calendar details…".to_owned(), Some(n) => format!("Permanently delete {} and its {n} items, including events, tasks and recurring occurrences?", current.with(|c| c.as_ref().map(|c| c.name.clone()).unwrap_or_default())) }
                        } else {
                            format!("Permanently delete {}{}?", selected.with(|e| e.as_ref().map(|e| s(e, "title")).unwrap_or_default()), if detail.with(|d| d.as_ref().is_some_and(|d| !s(&d.data, "recurrence").is_empty())) { " and its entire recurring series" } else { "" })
                        }}</p>
                        {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                        <div class="dialog-actions"><button class="text-button" on:click=move |_| modal.set(None)>"Cancel"</button>
                            <button class="danger-button" disabled=move || busy.get() || (whole && delete_count.with(Option::is_none)) on:click=move |_| {
                                let (c, d) = (current.get_untracked(), detail.get_untracked());
                                run(Box::pin(async move {
                                    let (url, tag) = match (whole, &c, &d) {
                                        (true, Some(c), _) => (format!("{BASE}/{}", c.id), etag(&c.id, c.revision)),
                                        (false, _, Some(d)) => (format!("{BASE}/{}/items/{}", d.collection_id, d.href), etag(&d.id, d.revision)),
                                        _ => return Ok(()),
                                    };
                                    send::<Value>(&url, "DELETE", None, &csrf(), Some(tag)).await.map_err(|e| e.message)?;
                                    selected.set(None);
                                    if whole { managed.set(String::new()); }
                                    notice.set("Deleted.".into());
                                    Ok(())
                                }));
                            }>"Delete permanently"</button></div>
                    </Dialog>
                }
            })}
            {move || (modal.get() == Some("reminders")).then(|| view! {
                <Dialog title="Reminders" on_close=Callback::new(move |_| modal.set(None))>
                    {move || if reminders.with(Vec::is_empty) { view! { <p>"No due reminders."</p> }.into_any() } else {
                        reminders.get().into_iter().map(|r| {
                            let id = s(&r, "id");
                            view! {
                                <div class="grant"><span><strong>{s(&r, "title")}</strong><small>{crate::dates::local_date_time(at(&r, "dueAt"), &[])}</small></span>
                                    <button class="text-button" on:click=move |_| {
                                        let id = id.clone();
                                        run(Box::pin(async move {
                                            send::<Value>(&format!("/api/v1/pim/reminders/{}", js_sys::encode_uri_component(&id)), "DELETE", None, &csrf(), None).await.map_err(|e| e.message)?;
                                            reminders.update(|list| list.retain(|r| s(r, "id") != id));
                                            modal.set(Some("reminders"));
                                            Ok(())
                                        }));
                                    }>"Dismiss"</button>
                                </div>
                            }
                        }).collect_view().into_any()
                    }}
                    <p class="muted small">"Reminders are delivered to the calendar owner. Imported email/audio alarms are retained as data and never send mail or play audio."</p>
                </Dialog>
            })}
        </PimShell>
    }
}

/// Adds `has-time-grid` to the main region while a time grid shows (React's `contentClassName`).
#[component]
fn TimeGridClass(active: Signal<bool>) -> impl IntoView {
    Effect::new(move |_| {
        let on = active.get();
        if let Some(main) = crate::ui::document().query_selector(".pim-main.calendar-main").ok().flatten() {
            let _ = if on { main.class_list().add_1("has-time-grid") } else { main.class_list().remove_1("has-time-grid") };
        }
    });
}

const REFRESH: [(&str, &str); 4] = [("15", "Every 15 minutes"), ("60", "Every hour"), ("360", "Every 6 hours"), ("1440", "Every day")];

/// Subscribe by link, or edit a subscription (`SubscriptionEditor`).
#[component]
fn SubscriptionEditor(calendar: Option<Collection>, on_save: Callback<(Value, Done)>, on_close: Callback<()>, on_refresh: Callback<()>) -> impl IntoView {
    let status = calendar.as_ref().and_then(|c| c.subscription.clone());
    let editing = calendar.is_some();
    let url = RwSignal::new(status.as_ref().map(|s| s["url"].as_str().unwrap_or_default().to_owned()).unwrap_or_default());
    let name = RwSignal::new(calendar.as_ref().map(|c| c.name.clone()).unwrap_or_default());
    let refresh = RwSignal::new(status.as_ref().and_then(|s| s["refreshMinutes"].as_u64()).unwrap_or(360).to_string());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let color = calendar.as_ref().map(|c| c.color.clone()).unwrap_or_else(|| "#2f7d8c".into());
    // A name suggested from the link until the person types one.
    let suggested = Memo::new(move |_| {
        let raw = url.get().trim().to_owned();
        let lower = raw.to_lowercase();
        let normalized = if lower.starts_with("webcals://") { format!("https://{}", &raw[10..]) } else if lower.starts_with("webcal://") { format!("https://{}", &raw[9..]) } else { raw };
        let Ok(parsed) = web_sys::Url::new(&normalized) else { return String::new() };
        let last = parsed.pathname().split('/').rfind(|p| !p.is_empty()).map(|p| js_sys::decode_uri_component(p).ok().and_then(|v| v.as_string()).unwrap_or_default()).unwrap_or_default();
        let last = last.trim_end_matches(".ics").trim_end_matches(".ICS").replace(['-', '_'], " ").trim().to_owned();
        let opaque = last.len() >= 24 && last.chars().all(|c| c.is_ascii_alphanumeric());
        if !last.is_empty() && !opaque { last } else { parsed.hostname().trim_start_matches("www.").to_owned() }
    });
    let options: Vec<(String, String)> = REFRESH.iter().map(|(v, l)| ((*v).into(), (*l).into())).collect();
    let dirty = Signal::derive(move || !editing && !url.with(String::is_empty));
    view! {
        <EditorPane title=if editing { "Edit subscription" } else { "Subscribe to a calendar" } on_close=on_close dirty=dirty>
            <form class="calendar-subscription-form" on:submit=move |e: web_sys::SubmitEvent| {
                e.prevent_default();
                let color = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok()).and_then(|f| web_sys::FormData::new_with_form(&f).ok()).and_then(|d| d.get("color").as_string()).unwrap_or_default();
                let typed = name.get_untracked().trim().to_owned();
                let chosen: String = if !typed.is_empty() { typed } else if !suggested.get_untracked().is_empty() { suggested.get_untracked() } else { "Subscribed calendar".into() };
                busy.set(true);
                error.set(String::new());
                { on_save.try_run((json!({ "url": url.get_untracked().trim(), "name": chosen.chars().take(255).collect::<String>(), "color": color, "refreshMinutes": refresh.get_untracked().parse::<u32>().unwrap_or(360) }), Callback::new(move |result: Result<(), String>| {
                    busy.set(false);
                    match result { Ok(()) => { on_close.try_run(()); }, Err(e) => error.set(e) }
                }))); };
            }>
                <label>"Calendar link"<input type="url" inputmode="url" required autofocus=!editing autocapitalize="none" spellcheck="false" maxlength="2048" placeholder="https://… or webcal://…" prop:value=move || url.get() on:input=move |e| url.set(event_target_value(&e)) pattern="(https?|webcals?)://.+"/><small class="field-hint">"An iCalendar (.ics) address. Events from it are read-only here and stay in sync."</small></label>
                <label>"Name"<input maxlength="255" prop:value=move || name.get() placeholder=move || { let s = suggested.get(); if s.is_empty() { "Subscribed calendar".to_owned() } else { s } } on:input=move |e| name.set(event_target_value(&e))/></label>
                <div class="pim-form-grid"><label>"Color"<input name="color" type="color" value=color.clone()/></label><label>"Refresh"<Select options=options.clone() value=refresh/></label></div>
                {status.clone().map(|st| {
                    let failed = st["lastError"].as_str().unwrap_or_default().to_owned();
                    let synced = st["lastSyncAt"].as_f64().unwrap_or(0.0);
                    view! {
                        <div class="property-card">
                            <div class="property"><Icon name=if failed.is_empty() { "refresh" } else { "alert" }/><div><span>{if !failed.is_empty() { failed.clone() } else if synced > 0.0 { format!("Updated {}", crate::dates::local_date_time(synced, &[])) } else { "Waiting for the first refresh".into() }}</span><small>{if failed.is_empty() { "Sync status" } else { "Last refresh failed; Dar retries within the hour" }}</small></div></div>
                            <button type="button" class="property property-action" disabled=move || busy.get() on:click=move |_| { on_refresh.try_run(()); }><Icon name="refresh"/><div><span>"Refresh now"</span><small>"Fetch the calendar again"</small></div></button>
                        </div>
                    }
                })}
                {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions"><button type="button" class="text-button">"Cancel"</button><button class="primary-button" disabled=move || busy.get() || url.with(|u| u.trim().is_empty())>{move || if busy.get() { "Saving…" } else if editing { "Save" } else { "Subscribe" }}</button></div>
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
    js_sys::Date::new(&JsValue::from_f64(ms)).to_iso_string().as_string().unwrap_or_default()[..16].to_owned()
}

fn time_zones() -> Vec<String> {
    let intl = js_sys::Reflect::get(&js_sys::global(), &"Intl".into()).ok();
    let supported = intl.as_ref().and_then(|i| js_sys::Reflect::get(i, &"supportedValuesOf".into()).ok()).and_then(|f| f.dyn_into::<js_sys::Function>().ok());
    let mut zones = vec!["UTC".to_owned(), "floating".to_owned()];
    if let (Some(intl), Some(f)) = (intl, supported)
        && let Ok(list) = f.call1(&intl, &"timeZone".into()) {
            zones.extend(js_sys::Array::from(&list).iter().filter_map(|v| v.as_string()));
        }
    zones
}

/// The event form (`EventEditor`).
#[component]
fn EventEditor(initial: Editor, calendars: Vec<Collection>, on_close: Callback<()>, on_save: Callback<(Value, String, Done)>) -> impl IntoView {
    let data = RwSignal::new(initial.data.clone());
    let collection = RwSignal::new(initial.collection_id.clone());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let recurrence = RwSignal::new(s(&initial.data, "recurrence"));
    let reminder = RwSignal::new(initial.data["reminderMinutes"].as_i64().map(|m| m.to_string()).unwrap_or_default());
    let status = RwSignal::new({ let st = s(&initial.data, "status"); if st.is_empty() { "CONFIRMED".to_owned() } else { st } });
    Effect::new(move |_| { let r = recurrence.get(); data.update(|d| d["recurrence"] = json!(r)); });
    Effect::new(move |_| { let r = reminder.get(); data.update(|d| d["reminderMinutes"] = if r.is_empty() { Value::Null } else { r.parse::<i64>().map(|m| json!(m)).unwrap_or(Value::Null) }); });
    Effect::new(move |_| { let st = status.get(); data.update(|d| d["status"] = json!(st)); });
    let get = move |k: &'static str| Signal::derive(move || data.with(|d| s(d, k)));
    let all_day = Signal::derive(move || data.with(|d| d["allDay"] == json!(true)));
    let set = move |k: &'static str, v: Value| data.update(|d| d[k] = v);
    // Moving the start keeps the duration, so the end never lands before it.
    let set_start = move |next: String| data.update(|d| {
        if d["allDay"] == json!(true) || next.len() < 16 || minutes_of(&next).is_nan() {
            d["start"] = json!(next);
            return;
        }
        let duration = (minutes_of(&s(d, "end")) - minutes_of(&s(d, "start"))).max(0.0);
        let end = if duration.is_nan() { s(d, "end") } else { stamp(minutes_of(&next) + duration) };
        d["start"] = json!(next);
        d["end"] = json!(end);
    });
    let start_date = Signal::derive(move || get("start").get().chars().take(10).collect::<String>());
    let start_time = Signal::derive(move || get("start").get().chars().skip(11).take(5).collect::<String>());
    let end_date = Signal::derive(move || { let e = get("end").get(); if all_day.get() { if e.len() >= 10 { add_days(&e[..10], -1) } else { String::new() } } else { e.chars().take(10).collect() } });
    let end_time = Signal::derive(move || get("end").get().chars().skip(11).take(5).collect::<String>());
    let duration = Signal::derive(move || {
        let (st, en) = (get("start").get(), get("end").get());
        if !all_day.get() && st.len() >= 16 && en.len() >= 16 { minutes_of(&en) - minutes_of(&st) } else { f64::NAN }
    });
    let duration_text = move || {
        let d = duration.get();
        if d < 0.0 {
            return "The event ends before it starts.".to_owned();
        }
        let text = if d >= 1440.0 * 60_000.0 {
            format!("{} days", (d / 86_400_000.0 * 10.0).round() / 10.0)
        } else if d >= 3_600_000.0 {
            let rest = d % 3_600_000.0;
            format!("{} h{}", (d / 3_600_000.0).floor(), if rest > 0.0 { format!(" {} min", (rest / 60_000.0).round()) } else { String::new() })
        } else {
            format!("{} min", (d / 60_000.0).round())
        };
        format!("Lasts {text}")
    };
    let options: Vec<(String, String)> = calendars.iter().map(|c| (c.id.clone(), c.name.clone())).collect();
    let statuses: Vec<(String, String)> = [("CONFIRMED", "Confirmed"), ("TENTATIVE", "Tentative"), ("CANCELLED", "Cancelled")].iter().map(|(v, l)| ((*v).into(), (*l).into())).collect();
    let creating = initial.resource.is_none();
    let occurrence = initial.occurrence.is_some();
    let more_open = !initial.data["attendees"].as_array().is_none_or(Vec::is_empty) || s(&initial.data, "status") != "CONFIRMED";
    let zones = StoredValue::new(time_zones());
    view! {
        <EditorPane title=if creating { "New event" } else { "Edit event" } on_close=on_close>
            <form class="pim-editor event-editor" on:submit=move |e: web_sys::SubmitEvent| {
                e.prevent_default();
                busy.set(true);
                error.set(String::new());
                { on_save.try_run((data.get_untracked(), collection.get_untracked(), Callback::new(move |result: Result<(), String>| {
                    busy.set(false);
                    if let Err(e) = result { error.set(e) }
                }))); };
            }>
                {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <label class="event-title-field"><span class="sr-only">"Title"</span><input prop:value=get("title") autofocus required maxlength="1024" placeholder="Add title" on:input=move |e| set("title", json!(event_target_value(&e)))/></label>
                {creating.then(|| view! { <label>"Calendar"<Select options=options.clone() value=collection/></label> })}
                <label class="pim-checkbox"><input type="checkbox" prop:checked=all_day on:change=move |e| {
                    let on = event_target_checked(&e);
                    data.update(|d| {
                        let day = s(d, "start").chars().take(10).collect::<String>();
                        d["allDay"] = json!(on);
                        d["start"] = json!(if on { day.clone() } else { format!("{day}T09:00") });
                        d["end"] = json!(if on { add_days(&day, 1) } else { format!("{day}T10:00") });
                    });
                }/>"All day"</label>
                <div class="event-when">
                    <label>"Starts"<input type="date" prop:value=start_date required on:input=move |e| { let v = event_target_value(&e); if all_day.get_untracked() { set("start", json!(v)) } else { let t = start_time.get_untracked(); set_start(format!("{v}T{}", if t.is_empty() { "09:00".to_owned() } else { t })) } }/></label>
                    {move || (!all_day.get()).then(|| view! { <label>"Start time"<input type="time" prop:value=start_time required on:input=move |e| set_start(format!("{}T{}", start_date.get_untracked(), event_target_value(&e)))/></label> })}
                </div>
                <div class="event-when">
                    <label>"Ends"<input type="date" prop:value=end_date required on:input=move |e| {
                        let v = event_target_value(&e);
                        if all_day.get_untracked() && !v.is_empty() { set("end", json!(add_days(&v, 1))) } else { let t = end_time.get_untracked(); set("end", json!(format!("{v}T{}", if t.is_empty() { "10:00".to_owned() } else { t }))) }
                    }/></label>
                    {move || (!all_day.get()).then(|| view! { <label>"End time"<input type="time" prop:value=end_time required on:input=move |e| set("end", json!(format!("{}T{}", end_date.get_untracked(), event_target_value(&e))))/></label> })}
                </div>
                {move || (!duration.get().is_nan()).then(|| view! { <p class=move || if duration.get() < 0.0 { "field-hint error" } else { "field-hint" }>{duration_text}</p> })}
                {move || (!all_day.get()).then(|| view! {
                    <label>"Timezone"<input aria-label="Timezone" list="common-timezones" prop:value=get("timezone") on:input=move |e| set("timezone", json!(event_target_value(&e))) required/>
                        <datalist id="common-timezones">{zones.get_value().into_iter().map(|z| view! { <option value=z></option> }).collect_view()}</datalist>
                        <small>"Use floating to keep the same wall time in each viewer's timezone."</small></label>
                })}
                {(!occurrence).then(|| view! { <RepeatField value=recurrence/> })}
                <ReminderField value=reminder/>
                <label>"Location"<input prop:value=get("location") maxlength="2048" placeholder="Add location" on:input=move |e| set("location", json!(event_target_value(&e)))/></label>
                <label>"Description"<textarea rows="4" prop:value=get("description") placeholder="Add notes" on:input=move |e| set("description", json!(event_target_value(&e)))></textarea></label>
                <details class="form-more" open=more_open><summary>"Attendees and status"</summary>
                    <label>"Attendee emails (one per line)"<textarea prop:value=move || data.with(|d| d["attendees"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default())
                        on:input=move |e| set("attendees", json!(event_target_value(&e).split('\n').map(str::trim).filter(|v| !v.is_empty()).collect::<Vec<_>>()))></textarea><small>"Attendees are saved. Invitation emails are not sent."</small></label>
                    <label>"Status"<Select aria_label="Status" options=statuses.clone() value=status/></label>
                </details>
                <div class="dialog-actions"><button class="text-button" type="button">"Cancel"</button><button class="primary-button" disabled={move || busy.get() || duration.get() < 0.0}>"Save event"</button></div>
            </form>
        </EditorPane>
    }
}
