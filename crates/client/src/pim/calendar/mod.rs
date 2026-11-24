//! Calendar app: Day, Week, Month, Year, and Agenda views with floating Quick-Peek &
//! Quick-Create popovers, direct drag-move/resize with Undo toast, live now-line updates,
//! scrollable calendar sidebar, and single-key keyboard shortcuts.

mod agenda;
mod editor;
mod model;
mod month;
mod popovers;
mod sidebar;
mod time_grid;
mod year;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue};

use self::agenda::AgendaView;
use self::editor::{EventEditor, SubscriptionEditor};
use self::model::{
    BASE, Calendars, Editor, QuickCreate, UndoToast, at, calendar_dots, day, local, occurrence_key,
    parse_ms, period_title, range, s, shift_view,
};
use self::month::MonthView;
use self::popovers::{EventPeekPopover, QuickCreatePopover, ShortcutsDialog};
use self::sidebar::CalendarSidebar;
use self::time_grid::TimeGridView;
use self::year::YearView;
use super::{Collection, CollectionEditor, Done, Item, PimShell, Sharing, etag, send, use_pim};
use crate::api::{self, BrowserSession};
use crate::date_popover::{DatePopover, add_days};
use crate::icons::Icon;
use crate::shared::{self, save_preferences};
use crate::ui::{Dialog, Listener, window};

const VIEWS: [(&str, &str); 5] = [
    ("day", "Day"),
    ("week", "Week"),
    ("month", "Month"),
    ("year", "Year"),
    ("agenda", "Agenda"),
];

#[component]
pub fn Calendar(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let pim = use_pim(on_signed_out);
    let shared = shared::context().preferences;
    let params =
        web_sys::UrlSearchParams::new_with_str(&window().location().search().unwrap_or_default())
            .ok();
    let param = |k: &str| params.as_ref().and_then(|p| p.get(k));
    let browser_zone = StoredValue::new({
        let fmt = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new());
        js_sys::Reflect::get(&fmt.resolved_options(), &"timeZone".into())
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "UTC".into())
    });

    // Live 60s tick for the now-line, today highlight, and Up Next countdown.
    let now_tick = RwSignal::new(js_sys::Date::now());
    {
        let cb = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
            now_tick.try_set(js_sys::Date::now());
        });
        let id = window()
            .set_interval_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                60_000,
            )
            .ok();
        let kept = send_wrapper::SendWrapper::new((id, cb));
        on_cleanup(move || {
            let (id, _cb) = kept.take();
            if let Some(handle) = id {
                window().clear_interval_with_handle(handle);
            }
        });
    }

    let pref_str = move |k: &'static str| -> Option<String> {
        shared.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get(k))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
    };
    let pref_bool = move |k: &'static str, default: bool| -> bool {
        shared.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get(k))
                .and_then(Value::as_bool)
                .unwrap_or(default)
        })
    };

    let zone = Memo::new(move |_| {
        pref_str("timezone")
            .filter(|z| !z.is_empty())
            .unwrap_or_else(|| browser_zone.get_value())
    });
    let week_start = Signal::derive(move || {
        shared.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("weekStart"))
                .and_then(Value::as_u64)
                .unwrap_or(1) as u32
        })
    });
    let time_format = Signal::derive(move || {
        pref_str("calendarTimeFormat")
            .filter(|f| f == "12h" || f == "24h")
            .unwrap_or_else(|| "24h".into())
    });
    let show_cancelled = Signal::derive(move || pref_bool("calendarShowCancelled", false));
    let show_tasks = Signal::derive(move || pref_bool("calendarShowTasks", true));
    let hidden = Memo::new(move |_| {
        shared.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("hiddenCalendars"))
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let today = Memo::new(move |_| local(now_tick.get(), &zone.get())[..10].to_owned());
    let initial_event = StoredValue::new((param("book"), param("event")));

    let anchor = RwSignal::new(
        param("date")
            .filter(|d| d.len() == 10)
            .unwrap_or_else(|| today.get_untracked()),
    );
    let url_view = param("view").filter(|v| VIEWS.iter().any(|(k, _)| k == v));
    let view_mode = RwSignal::new(url_view.clone().unwrap_or_else(|| "month".into()));
    let view_initialized = StoredValue::new(url_view.is_some());
    Effect::new(move |_| {
        if !view_initialized.get_value()
            && let Some(pref_v) =
                pref_str("calendarDefaultView").filter(|v| VIEWS.iter().any(|(k, _)| k == v))
        {
            view_mode.set(pref_v);
            view_initialized.set_value(true);
        }
    });

    let calendars: RwSignal<Vec<Collection>> = RwSignal::new(Vec::new());
    let events: RwSignal<Vec<Value>> = RwSignal::new(Vec::new());
    let reminders: RwSignal<Vec<Value>> = RwSignal::new(Vec::new());
    let managed = RwSignal::new(String::new());
    let action_id = RwSignal::new(String::new());
    let action_sequence = RwSignal::new(0u32);
    let selected: RwSignal<Option<Value>> = RwSignal::new(None);
    let detail: RwSignal<Option<Item>> = RwSignal::new(None);
    let editor: RwSignal<Option<Editor>> = RwSignal::new(None);
    let quick_create: RwSignal<Option<QuickCreate>> = RwSignal::new(None);
    let scope: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let modal: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let delete_count: RwSignal<Option<usize>> = RwSignal::new(None);
    let loading = RwSignal::new(true);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let undo_toast: RwSignal<Option<UndoToast>> = RwSignal::new(None);
    let sequence = RwSignal::new(0u32);
    let loaded_range = StoredValue::new(String::new());
    let grid = NodeRef::<html::Div>::new();
    let grid_period = StoredValue::new(String::new());
    let month_grid = NodeRef::<html::Div>::new();
    let month_limit = RwSignal::new(3usize);
    let dragged = StoredValue::new(None::<(Value, bool)>);

    let session_sv = StoredValue::new(session.clone());
    let csrf = move || session_sv.with_value(|s| s.csrf_token.clone());

    let window_range = Memo::new(move |_| range(&anchor.get(), &view_mode.get(), week_start.get()));
    let days = Memo::new(move |_| {
        let (start, count) = window_range.get();
        (0..count).map(|i| add_days(&start, i)).collect::<Vec<_>>()
    });
    let single_day = Memo::new(move |_| view_mode.get() == "day");
    let title = Memo::new(move |_| {
        let d = days.get();
        period_title(&anchor.get(), &view_mode.get(), &d)
    });
    let ready_to_create = Memo::new(move |_| !loading.get() && shared.value.with(Option::is_some));

    // Dots for sidebar MiniMonth & mobile 7-day strip.
    let mini_days = Memo::new(move |_| {
        let a = anchor.get();
        let (start, count) = range(&a, "month", week_start.get());
        (0..count).map(|i| add_days(&start, i)).collect::<Vec<_>>()
    });
    let dots = Memo::new(move |_| {
        calendar_dots(
            &mini_days.get(),
            &events.get(),
            &calendars.get(),
            &zone.get(),
        )
    });

    // Mobile 7-day week strip around anchor.
    let mobile_week = Memo::new(move |_| {
        let (start, _) = range(&anchor.get(), "week", week_start.get());
        (0..7).map(|i| add_days(&start, i)).collect::<Vec<_>>()
    });

    // Persist preference helper.
    let save_pref = move |patch: Value| {
        let Some(prefs) = shared.value.get_untracked() else {
            return;
        };
        let sess = session_sv.get_value();
        spawn_local(async move {
            match save_preferences(patch, prefs.revision, &sess).await {
                Ok(next) => {
                    shared.value.try_set(Some(next));
                }
                Err(e) => {
                    error.try_set(e.message);
                }
            }
        });
    };

    // Bounded 4-worker occurrence loader with partial-failure retention.
    Effect::new(move |_| {
        pim.epoch.track();
        let (start, count) = window_range.get();
        let z = zone.get();
        let hide = hidden.get();
        let cancelled = show_cancelled.get();
        let tasks = show_tasks.get();
        let key = format!("{start}/{count}/{z}/{cancelled}/{tasks}/{hide:?}");
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
            let pad = if count >= 365 { 0.0 } else { 86_400_000.0 };
            let from = js_sys::Date::new(&JsValue::from_f64(
                parse_ms(&format!("{start}T00:00:00Z")) - pad,
            ))
            .to_iso_string()
            .as_string()
            .unwrap_or_default();
            let to = js_sys::Date::new(&JsValue::from_f64(
                parse_ms(&format!("{}T00:00:00Z", add_days(&start, count))) + pad,
            ))
            .to_iso_string()
            .as_string()
            .unwrap_or_default();
            let visible: Vec<Collection> = list
                .items
                .iter()
                .filter(|c| !hide.contains(&c.id))
                .cloned()
                .collect();
            let query = web_sys::UrlSearchParams::new().expect("params");
            for (k, v) in [
                ("from", from.as_str()),
                ("to", to.as_str()),
                ("timezone", z.as_str()),
                ("includeCancelled", if cancelled { "1" } else { "0" }),
                ("includeTasks", if tasks { "1" } else { "0" }),
            ] {
                query.append(k, v);
            }
            let query = query.to_string().as_string().unwrap_or_default();
            let next = std::rc::Rc::new(std::cell::Cell::new(0usize));
            let results: std::rc::Rc<std::cell::RefCell<Vec<Option<Result<Vec<Value>, ()>>>>> =
                std::rc::Rc::new(std::cell::RefCell::new(vec![None; visible.len()]));
            let workers = (0..visible.len().min(4)).map(|_| {
                let (next, results, visible, query) =
                    (next.clone(), results.clone(), &visible, &query);
                async move {
                    loop {
                        let index = next.get();
                        if index >= visible.len() {
                            break;
                        }
                        next.set(index + 1);
                        let result = api::get::<Value>(&format!(
                            "{BASE}/{}/occurrences?{query}",
                            visible[index].id
                        ))
                        .await;
                        results.borrow_mut()[index] = Some(
                            result
                                .map(|v| v["items"].as_array().cloned().unwrap_or_default())
                                .map_err(|_| ()),
                        );
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
                let kept: Vec<Value> = events
                    .get_untracked()
                    .into_iter()
                    .filter(|e| failed.iter().any(|c| c.id == s(e, "collectionId")))
                    .collect();
                batches.extend(kept);
            }
            if !list.items.iter().any(|c| c.id == managed.get_untracked()) {
                managed.set(list.items.first().map(|c| c.id.clone()).unwrap_or_default());
            }
            calendars.set(list.items);
            events.set(batches);
            reminders.set(inbox["items"].as_array().cloned().unwrap_or_default());
            error.set(if failed.is_empty() {
                String::new()
            } else {
                format!(
                    "Could not refresh {}.{} Try Refresh.",
                    failed
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    if same {
                        " Showing the last loaded events for them."
                    } else {
                        ""
                    }
                )
            });
            loading.set(false);
        });
    });

    // `?book=…&event=…` deep link (e.g. from Global Search) opens that event once loaded.
    Effect::new(move |_| {
        let (book, href) = initial_event.get_value();
        let (Some(book), Some(href)) = (book, href) else {
            return;
        };
        if loading.get() {
            return;
        }
        if let Some(found) = events.with(|list| {
            list.iter()
                .find(|e| s(e, "collectionId") == book && s(e, "href") == href)
                .cloned()
        }) {
            initial_event.set_value((None, None));
            selected.set(Some(found));
        }
    });

    // Fetch full item detail when an event is selected.
    Effect::new(move |_| {
        let Some(sel) = selected.get() else {
            detail.set(None);
            return;
        };
        let key = occurrence_key(&sel);
        spawn_local(async move {
            let result = api::get::<Item>(&format!(
                "{BASE}/{}/items/{}",
                s(&sel, "collectionId"),
                s(&sel, "href")
            ))
            .await;
            if selected
                .try_with_untracked(|cur| cur.as_ref().is_some_and(|x| occurrence_key(x) == key))
                .unwrap_or(false)
            {
                match result {
                    Ok(item) => detail.set(Some(item)),
                    Err(e) => error.set(e.message),
                }
            }
        });
    });

    // Sync URL query params.
    Effect::new(move |_| {
        let params = web_sys::UrlSearchParams::new().expect("params");
        params.append("date", &anchor.get());
        params.append("view", &view_mode.get());
        let _ = window().history().expect("history").replace_state_with_url(
            &JsValue::NULL,
            "",
            Some(&format!(
                "/calendar?{}",
                params.to_string().as_string().unwrap_or_default()
            )),
        );
    });

    // Scroll time grid to 07:00 on both Week and Day once per period!
    Effect::new(move |_| {
        let v = view_mode.get();
        let period = format!("{v}/{}", window_range.get().0);
        pim.state.track();
        let is_time = v == "week" || v == "day";
        if is_time && !loading.get() && grid_period.get_value() != period {
            if let Some(g) = grid.get() {
                let g_clone = g.clone();
                request_animation_frame(move || {
                    g_clone.set_scroll_top(7 * 64);
                });
                grid_period.set_value(period);
            }
        } else if !is_time {
            grid_period.set_value(String::new());
        }
    });

    // Keep selected occurrence fresh after reloads.
    Effect::new(move |_| {
        let list = events.get();
        if loading.get_untracked() {
            return;
        }
        if let Some(current) = selected.get_untracked() {
            selected.set(
                list.into_iter()
                    .find(|e| occurrence_key(e) == occurrence_key(&current)),
            );
        }
    });

    // Dynamically measure Month cell height to show optimal number of rows.
    Effect::new(move |_| {
        let Some(node) = month_grid.get() else { return };
        let measured = node.clone();
        let measure = move || {
            let height = measured.client_height() as f64;
            let limit = (((height - 30.0) / 6.0 - 44.0) / 22.0)
                .floor()
                .clamp(1.0, 4.0) as usize;
            if month_limit
                .try_get_untracked()
                .is_some_and(|current| current != limit)
            {
                month_limit.set(limit);
            }
        };
        let callback = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(measure);
        if let Ok(observer) = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()) {
            observer.observe(&node);
            let kept = send_wrapper::SendWrapper::new((observer, callback));
            on_cleanup(move || kept.take().0.disconnect());
        }
    });

    let run =
        move |work: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>| {
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

    let writable_target = move || -> Option<Collection> {
        let editable = |c: &Collection| c.role != "read" && c.subscription.is_none();
        calendars.with_untracked(|list| {
            list.iter()
                .find(|c| c.id == managed.get_untracked() && editable(c))
                .or_else(|| list.iter().find(|c| editable(c)))
                .cloned()
        })
    };

    let open_full_new_event = move |date: String, hour: u32| {
        if !ready_to_create.get_untracked() {
            return;
        }
        let Some(target) = writable_target() else {
            modal.set(Some("newCalendar"));
            return;
        };
        let end = if hour < 23 {
            format!("{date}T{:02}:00", hour + 1)
        } else {
            format!("{}T00:00", add_days(&date, 1))
        };
        selected.set(None);
        quick_create.set(None);
        editor.set(Some(Editor {
            collection_id: target.id,
            resource: None,
            occurrence: None,
            data: json!({
                "title": "",
                "start": format!("{date}T{hour:02}:00"),
                "end": end,
                "timezone": zone.get_untracked(),
                "allDay": false,
                "location": "",
                "description": "",
                "recurrence": "",
                "attendees": [],
                "reminderMinutes": 10,
                "status": "CONFIRMED",
            }),
        }));
    };

    let open_quick_create = move |date: String, hour: u32| {
        if !ready_to_create.get_untracked() {
            return;
        }
        if writable_target().is_none() {
            modal.set(Some("newCalendar"));
            return;
        }
        selected.set(None);
        quick_create.set(Some(QuickCreate {
            date,
            hour,
            duration_mins: 60,
            all_day: false,
        }));
    };

    let edit_occurrence = move |whole: bool| {
        let (Some(d), Some(sel)) = (detail.get_untracked(), selected.get_untracked()) else {
            return;
        };
        editor.set(Some(Editor {
            collection_id: d.collection_id.clone(),
            occurrence: (!whole).then(|| s(&sel, "recurrenceId")),
            data: if whole { d.data.clone() } else { sel },
            resource: Some(d),
        }));
        scope.set(None);
    };

    let duplicate_selected = move || {
        let Some(d) = detail.get_untracked() else {
            return;
        };
        let mut copy = d.data.clone();
        let orig = s(&copy, "title");
        copy["title"] = json!(format!("{orig} (copy)"));
        selected.set(None);
        editor.set(Some(Editor {
            collection_id: d.collection_id,
            resource: None,
            occurrence: None,
            data: copy,
        }));
    };

    // Save handler for both `EventEditor` and `QuickCreatePopover`.
    let save_event = Callback::new(move |(payload, target_cid, done): (Value, String, Done)| {
        let current_editor = editor.get_untracked();
        spawn_local(async move {
            let outcome: Result<(), String> = async {
                match current_editor {
                    Some(Editor {
                        resource: Some(res),
                        occurrence,
                        ..
                    }) => {
                        if let Some(occ) = occurrence {
                            send::<Item>(
                                &format!("{BASE}/{}/items/{}", res.collection_id, res.href),
                                "PUT",
                                Some(json!({ "occurrenceId": occ, "data": payload })),
                                &csrf(),
                                Some(etag(&res.id, res.revision)),
                            )
                            .await
                            .map_err(|e| e.message)?;
                        } else if target_cid != res.collection_id {
                            // Move event across calendars: create in destination then delete old.
                            send::<Item>(
                                &format!("{BASE}/{target_cid}/items"),
                                "POST",
                                Some(json!({ "data": payload })),
                                &csrf(),
                                None,
                            )
                            .await
                            .map_err(|e| e.message)?;
                            send::<Value>(
                                &format!("{BASE}/{}/items/{}", res.collection_id, res.href),
                                "DELETE",
                                None,
                                &csrf(),
                                Some(etag(&res.id, res.revision)),
                            )
                            .await
                            .map_err(|e| e.message)?;
                            selected.set(None);
                        } else {
                            send::<Item>(
                                &format!("{BASE}/{}/items/{}", res.collection_id, res.href),
                                "PUT",
                                Some(json!({ "data": payload })),
                                &csrf(),
                                Some(etag(&res.id, res.revision)),
                            )
                            .await
                            .map_err(|e| e.message)?;
                        }
                    }
                    _ => {
                        send::<Item>(
                            &format!("{BASE}/{target_cid}/items"),
                            "POST",
                            Some(json!({ "data": payload })),
                            &csrf(),
                            None,
                        )
                        .await
                        .map_err(|e| e.message)?;
                    }
                }
                Ok(())
            }
            .await;
            match outcome {
                Ok(()) => {
                    editor.set(None);
                    selected.set(None);
                    pim.refresh();
                    done.try_run(Ok(()));
                }
                Err(msg) => {
                    done.try_run(Err(msg));
                }
            }
        });
    });

    // Direct drag-to-move / drag-to-resize: saves immediately and shows an Undo toast!
    let drop_at = move |date: String, hour: Option<u32>| {
        let Some((event, resize)) = dragged.get_value() else {
            return;
        };
        dragged.set_value(None);
        let z = zone.get_untracked();
        run(Box::pin(async move {
            let resource: Item = api::get(&format!(
                "{BASE}/{}/items/{}",
                s(&event, "collectionId"),
                s(&event, "href")
            ))
            .await
            .map_err(|e| e.message)?;
            let old_start = local(at(&event, "startAt"), &z);
            let all_day = event["allDay"] == json!(true);
            let new_start = if resize {
                old_start.clone()
            } else if all_day {
                date.clone()
            } else {
                format!(
                    "{date}T{}",
                    hour.map(|h| format!("{h:02}:00"))
                        .unwrap_or_else(|| old_start[11..16].to_owned())
                )
            };
            let duration = at(&event, "endAt") - at(&event, "startAt");
            let naive = parse_ms(&if new_start.len() == 10 {
                format!("{new_start}T00:00:00Z")
            } else {
                format!("{}:00Z", &new_start[..16])
            });
            let new_end = if resize {
                format!("{date}T{:02}:00", hour.unwrap_or(10))
            } else {
                let iso = js_sys::Date::new(&JsValue::from_f64(naive + duration))
                    .to_iso_string()
                    .as_string()
                    .unwrap_or_default();
                iso[..if all_day { 10 } else { 16 }].to_owned()
            };
            if !all_day
                && parse_ms(&format!("{new_end}:00Z")) <= parse_ms(&format!("{new_start}:00Z"))
            {
                return Err("End time must be after start time.".into());
            }
            let recurring = resource.data["recurring"] == json!(true)
                || !s(&resource.data, "recurrence").is_empty();
            let occ = recurring.then(|| s(&event, "recurrenceId"));

            let prev_data = if recurring {
                event.clone()
            } else {
                resource.data.clone()
            };
            let mut next_data = prev_data.clone();
            next_data["start"] = json!(new_start);
            next_data["end"] = json!(new_end);
            if !all_day {
                next_data["timezone"] = json!(z);
            }

            let mut body = json!({ "data": next_data });
            if let Some(ref occ_id) = occ {
                body["occurrenceId"] = json!(occ_id);
            }
            let updated: Item = send(
                &format!("{BASE}/{}/items/{}", resource.collection_id, resource.href),
                "PUT",
                Some(body),
                &csrf(),
                Some(etag(&resource.id, resource.revision)),
            )
            .await
            .map_err(|e| e.message)?;

            let verb = if resize { "Resized" } else { "Moved" };
            let ev_title = s(&event, "title");
            undo_toast.set(Some(UndoToast {
                message: format!("{verb} \"{ev_title}\"."),
                previous_item: updated,
                previous_data: prev_data,
                occurrence: occ,
            }));
            Ok(())
        }));
    };

    let apply_undo = move || {
        let Some(u) = undo_toast.get_untracked() else {
            return;
        };
        undo_toast.set(None);
        run(Box::pin(async move {
            let mut body = json!({ "data": u.previous_data });
            if let Some(occ_id) = u.occurrence {
                body["occurrenceId"] = json!(occ_id);
            }
            send::<Value>(
                &format!(
                    "{BASE}/{}/items/{}",
                    u.previous_item.collection_id, u.previous_item.href
                ),
                "PUT",
                Some(body),
                &csrf(),
                Some(etag(&u.previous_item.id, u.previous_item.revision)),
            )
            .await
            .map_err(|e| e.message)?;
            Ok(())
        }));
    };

    // Clickable task completion directly from Calendar!
    let toggle_task = Callback::new(move |ev: Value| {
        let task_id = s(&ev, "resourceId");
        run(Box::pin(async move {
            let enc = js_sys::encode_uri_component(&task_id)
                .as_string()
                .unwrap_or_default();
            let item: Value = api::get(&format!("/api/v1/tasks/items/{enc}"))
                .await
                .map_err(|e| e.message)?;
            let id = s(&item, "id");
            let rev = item["revision"].as_u64().unwrap_or(0);
            let mut next = item["data"].clone();
            let done = s(&next, "status") == "COMPLETED";
            next["status"] = json!(if done { "NEEDS-ACTION" } else { "COMPLETED" });
            next["progress"] = json!(if done { 0 } else { 100 });
            send::<Value>(
                &format!("/api/v1/tasks/items/{enc}"),
                "PUT",
                Some(json!({ "data": next })),
                &csrf(),
                Some(etag(&id, rev)),
            )
            .await
            .map_err(|e| e.message)?;
            Ok(())
        }));
    });

    let refresh_subscription = Callback::new(move |c: Collection| {
        spawn_local(async move {
            match send::<Value>(
                &format!("{BASE}/{}/refresh", c.id),
                "POST",
                None,
                &csrf(),
                None,
            )
            .await
            {
                Ok(_) => {
                    notice.set(format!("Refreshing {}…", c.name));
                    set_timeout(
                        move || pim.refresh(),
                        std::time::Duration::from_millis(4000),
                    );
                }
                Err(e) => error.set(e.message),
            }
        });
    });

    let calendar_action = Callback::new(move |(c, kind): (Collection, &'static str)| {
        let id = action_sequence.get_untracked() + 1;
        action_sequence.set(id);
        action_id.set(c.id.clone());
        error.set(String::new());
        modal.set(Some(kind));
        if kind == "deleteCalendar" {
            delete_count.set(None);
            spawn_local(async move {
                match api::get::<Value>(&format!("{BASE}/{}", c.id)).await {
                    Ok(v) if action_sequence.try_get_untracked() == Some(id) => {
                        delete_count.set(v["count"].as_u64().map(|c| c as usize))
                    }
                    Err(e) => error.set(e.message),
                    _ => {}
                }
            });
        }
    });

    let current = Memo::new(move |_| {
        let id = {
            let a = action_id.get();
            if a.is_empty() { managed.get() } else { a }
        };
        calendars.with(|c| c.iter().find(|x| x.id == id).cloned())
    });
    let selected_calendar = Memo::new(move |_| {
        selected.with(|sel| {
            sel.as_ref().and_then(|e| {
                calendars.with(|c| c.iter().find(|x| x.id == s(e, "collectionId")).cloned())
            })
        })
    });
    let can_edit = Memo::new(move |_| {
        selected_calendar.with(|c| {
            c.as_ref()
                .is_some_and(|c| c.role != "read" && c.subscription.is_none())
        })
    });

    // Global single-key keyboard shortcuts (C, T, D/W/M/Y/A, J/K, ?, Escape).
    {
        let key_listener = Listener::new(&window(), "keydown", false, move |ev| {
            let Ok(ke) = ev.dyn_into::<web_sys::KeyboardEvent>() else {
                return;
            };
            if ke.ctrl_key() || ke.meta_key() || ke.alt_key() {
                return;
            }
            let key = ke.key();
            if key == "Escape" {
                if quick_create
                    .try_with_untracked(Option::is_some)
                    .unwrap_or(false)
                {
                    quick_create.try_set(None);
                    return;
                }
                if selected
                    .try_with_untracked(Option::is_some)
                    .unwrap_or(false)
                    && editor.try_with_untracked(Option::is_none).unwrap_or(true)
                    && modal.try_with_untracked(Option::is_none).unwrap_or(true)
                {
                    selected.try_set(None);
                    return;
                }
                if modal.try_get_untracked() == Some(Some("shortcuts")) {
                    modal.try_set(None);
                }
                return;
            }
            // Ignore single-key shortcuts when typing in any input/form/dialog.
            let in_editable = ke
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .is_some_and(|el| {
                    el.closest("input,textarea,select,[contenteditable=true],[role=dialog]")
                        .ok()
                        .flatten()
                        .is_some()
                });
            if in_editable
                || editor.try_with_untracked(Option::is_some).unwrap_or(false)
                || modal.try_with_untracked(Option::is_some).unwrap_or(false)
                || quick_create
                    .try_with_untracked(Option::is_some)
                    .unwrap_or(false)
            {
                return;
            }
            match key.as_str() {
                "c" | "C" => {
                    ke.prevent_default();
                    let d = anchor.get_untracked();
                    open_full_new_event(d, 9);
                }
                "t" | "T" => {
                    ke.prevent_default();
                    anchor.set(today.get_untracked());
                }
                "d" | "D" => {
                    ke.prevent_default();
                    view_mode.set("day".into());
                }
                "w" | "W" => {
                    ke.prevent_default();
                    view_mode.set("week".into());
                }
                "m" | "M" => {
                    ke.prevent_default();
                    view_mode.set("month".into());
                }
                "y" | "Y" => {
                    ke.prevent_default();
                    view_mode.set("year".into());
                }
                "a" | "A" => {
                    ke.prevent_default();
                    view_mode.set("agenda".into());
                }
                "j" | "J" => {
                    ke.prevent_default();
                    anchor.set(shift_view(
                        &anchor.get_untracked(),
                        &view_mode.get_untracked(),
                        1,
                    ));
                }
                "k" | "K" => {
                    ke.prevent_default();
                    anchor.set(shift_view(
                        &anchor.get_untracked(),
                        &view_mode.get_untracked(),
                        -1,
                    ));
                }
                "?" => {
                    ke.prevent_default();
                    modal.set(Some("shortcuts"));
                }
                _ => {}
            }
        });
        let kept = send_wrapper::SendWrapper::new(key_listener);
        on_cleanup(move || drop(kept.take()));
    }

    // Sidebar callbacks.
    let on_toggle_calendar = Callback::new(move |(cid, shown): (String, bool)| {
        let mut next = hidden.get_untracked();
        if shown {
            next.retain(|id| id != &cid);
        } else if !next.contains(&cid) {
            next.push(cid);
        }
        save_pref(json!({ "hiddenCalendars": next }));
    });
    let on_solo_calendar = Callback::new(move |cid: String| {
        let next: Vec<String> = calendars
            .get_untracked()
            .into_iter()
            .filter(|c| c.id != cid)
            .map(|c| c.id)
            .collect();
        save_pref(json!({ "hiddenCalendars": next }));
    });
    let on_show_all_calendars = Callback::new(move |()| {
        save_pref(json!({ "hiddenCalendars": [] }));
    });
    let on_quick_color = Callback::new(move |(cal, hex): (Collection, String)| {
        run(Box::pin(async move {
            send::<Value>(
                &format!("{BASE}/{}", cal.id),
                "PUT",
                Some(json!({ "name": cal.name, "color": hex })),
                &csrf(),
                Some(etag(&cal.id, cal.revision)),
            )
            .await
            .map_err(|e| e.message)?;
            Ok(())
        }));
    });
    let on_toggle_tasks = Callback::new(move |on: bool| {
        save_pref(json!({ "calendarShowTasks": on }));
    });
    let on_edit_subscription = Callback::new(move |c: Collection| {
        action_id.set(c.id);
        modal.set(Some("editSubscription"));
    });

    let has_details = Signal::derive(|| false);

    let main_view = move || {
        if loading.get() {
            return view! { <p role="status" class="pim-loading">"Loading calendar…"</p> }
                .into_any();
        }
        if calendars.with(Vec::is_empty) {
            return view! {
                <div class="pim-empty">
                    <h2>"No calendars"</h2>
                    <p>"Create a calendar to start scheduling events."</p>
                    <button type="button" class="primary-button" on:click=move |_| modal.set(Some("newCalendar"))>
                        "Create calendar"
                    </button>
                </div>
            }
            .into_any();
        }
        match view_mode.get().as_str() {
            "month" => view! {
                <MonthView
                    days=days
                    anchor=anchor
                    today=today
                    events=events
                    calendars=calendars
                    zone=zone
                    time_format=time_format
                    selected=selected
                    month_limit=month_limit
                    month_grid=month_grid
                    can_create=ready_to_create
                    on_select=Callback::new(move |ev| {
                        quick_create.set(None);
                        selected.set(Some(ev));
                    })
                    on_pick_day=Callback::new(move |d| {
                        anchor.set(d);
                        view_mode.set("day".into());
                    })
                    on_quick_create=Callback::new(move |(d, h)| open_quick_create(d, h))
                    on_drag_start=Callback::new(move |pair| dragged.set_value(Some(pair)))
                    on_drop=Callback::new(move |d| drop_at(d, None))
                    on_toggle_task=toggle_task
                />
            }
            .into_any(),
            "year" => view! {
                <YearView
                    anchor=anchor
                    today=today
                    week_start=week_start
                    events=events
                    zone=zone
                    on_pick_day=Callback::new(move |d| {
                        anchor.set(d);
                        view_mode.set("day".into());
                    })
                    on_pick_month=Callback::new(move |d| {
                        anchor.set(d);
                        view_mode.set("month".into());
                    })
                />
            }
            .into_any(),
            "agenda" => view! {
                <AgendaView
                    days=days
                    today=today
                    events=events
                    calendars=calendars
                    zone=zone
                    time_format=time_format
                    selected=selected
                    on_select=Callback::new(move |ev| {
                        quick_create.set(None);
                        selected.set(Some(ev));
                    })
                    on_toggle_task=toggle_task
                />
            }
            .into_any(),
            _ => view! {
                <TimeGridView
                    days=days
                    single_day=single_day
                    today=today
                    events=events
                    calendars=calendars
                    zone=zone
                    time_format=time_format
                    selected=selected
                    now_tick=now_tick
                    grid_ref=grid
                    can_create=ready_to_create
                    on_select=Callback::new(move |ev| {
                        quick_create.set(None);
                        selected.set(Some(ev));
                    })
                    on_pick_day=Callback::new(move |d| {
                        anchor.set(d);
                        view_mode.set("day".into());
                    })
                    on_quick_create=Callback::new(move |(d, h)| open_quick_create(d, h))
                    on_drag_start=Callback::new(move |pair| dragged.set_value(Some(pair)))
                    on_drop=Callback::new(move |(d, h)| drop_at(d, h))
                    on_toggle_task=toggle_task
                />
            }
            .into_any(),
        }
    };

    view! {
        <PimShell
            kind="calendar"
            session=session.clone()
            on_signed_out=on_signed_out
            pim=pim
            main_class="calendar-main"
            has_details=has_details
            sidebar=std::sync::Arc::new(move || view! {
                <CalendarSidebar
                    ready_to_create=ready_to_create
                    anchor=anchor
                    today=today
                    week_start=week_start
                    view_mode=view_mode
                    days=days
                    calendars=calendars
                    hidden=hidden
                    managed=managed
                    show_tasks=show_tasks
                    time_format=time_format
                    zone=zone
                    events=events
                    reminders=reminders
                    now_tick=now_tick
                    dots=dots
                    on_new_event=Callback::new(move |()| open_full_new_event(anchor.get_untracked(), 9))
                    on_open_modal=Callback::new(move |m| modal.set(Some(m)))
                    on_toggle_calendar=on_toggle_calendar
                    on_solo_calendar=on_solo_calendar
                    on_show_all_calendars=on_show_all_calendars
                    on_quick_color=on_quick_color
                    on_toggle_tasks=on_toggle_tasks
                    on_calendar_action=calendar_action
                    on_refresh_subscription=refresh_subscription
                    on_edit_subscription=on_edit_subscription
                    on_select_event=Callback::new(move |ev| {
                        quick_create.set(None);
                        selected.set(Some(ev));
                    })
                />
            }.into_any())
            details=std::sync::Arc::new(move || ().into_any())
        >
            <TimeGridClass active=Signal::derive(move || matches!(view_mode.get().as_str(), "day" | "week"))/>

            <div class="calendar-heading">
                <div class="calendar-heading-left">
                    <div class="calendar-period-group">
                        <h1 class="calendar-period-heading">{move || title.get()}</h1>
                        <DatePopover
                            value=anchor
                            today=today
                            week_start=week_start
                            title=Signal::derive(String::new)
                            trigger_class="icon-button calendar-period-trigger"
                            on_change=Callback::new(move |d| anchor.set(d))
                        />
                    </div>
                    <div class="calendar-nav-group">
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="Previous period"
                            title="Previous (K)"
                            on:click=move |_| anchor.set(shift_view(&anchor.get_untracked(), &view_mode.get_untracked(), -1))
                        >
                            <Icon name="back"/>
                        </button>
                        <button
                            type="button"
                            class="text-button calendar-today-btn"
                            on:click=move |_| anchor.set(today.get_untracked())
                        >
                            "Today"
                        </button>
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="Next period"
                            title="Next (J)"
                            on:click=move |_| anchor.set(shift_view(&anchor.get_untracked(), &view_mode.get_untracked(), 1))
                        >
                            <Icon name="chevron"/>
                        </button>
                    </div>
                </div>

                <div class="calendar-controls">
                    <div
                        class="segmented calendar-view"
                        role="radiogroup"
                        aria-label="Calendar view"
                        on:keydown=move |e| {
                            let order = ["day", "week", "month", "year", "agenda"];
                            let index = order.iter().position(|v| *v == view_mode.get_untracked()).unwrap_or(2);
                            let key = e.key();
                            if key == "ArrowRight" || key == "ArrowLeft" {
                                e.prevent_default();
                                let next = order[(index + if key == "ArrowRight" { 1 } else { order.len() - 1 }) % order.len()];
                                view_mode.set(next.into());
                                if let Some(button) = e
                                    .current_target()
                                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                                    .and_then(|t| t.query_selector(&format!("[data-value=\"{next}\"]")).ok().flatten())
                                    .and_then(|b| b.dyn_into::<web_sys::HtmlElement>().ok())
                                {
                                    let _ = button.focus();
                                }
                            }
                        }
                    >
                        {VIEWS.into_iter().map(|(value, label)| view! {
                            <button
                                type="button"
                                role="radio"
                                data-value=value
                                aria-checked=move || (view_mode.get() == value).to_string()
                                tabindex=move || if view_mode.get() == value { "0" } else { "-1" }
                                on:click=move |_| view_mode.set(value.into())
                            >{label}</button>
                        }).collect_view()}
                    </div>
                    <button
                        type="button"
                        class="icon-button calendar-shortcuts-btn"
                        aria-label="Keyboard shortcuts"
                        title="Keyboard shortcuts (?)"
                        on:click=move |_| modal.set(Some("shortcuts"))
                    >
                        <Icon name="keyboard"/>
                    </button>
                </div>
            </div>

            {move || matches!(view_mode.get().as_str(), "day" | "week" | "agenda").then(|| view! {
                <div class="calendar-mobile-strip" aria-label="Week days">
                    {move || {
                        let dot_map = dots.get();
                        let cur = anchor.get();
                        let today_now = today.get();
                        mobile_week.get().into_iter().map(|d| {
                            let wd = day(&d, &[("weekday", "narrow")]);
                            let num = d[8..].trim_start_matches('0').to_owned();
                            let is_sel = d == cur;
                            let is_today = d == today_now;
                            let day_dots = dot_map.get(&d).cloned().unwrap_or_default();
                            let d_pick = d.clone();
                            view! {
                                <button
                                    type="button"
                                    class=format!(
                                        "calendar-mobile-day{}{}",
                                        if is_sel { " is-selected" } else { "" },
                                        if is_today { " is-today" } else { "" }
                                    )
                                    on:click=move |_| anchor.set(d_pick.clone())
                                >
                                    <span class="calendar-mobile-wd">{wd}</span>
                                    <span class="calendar-mobile-num">{num}</span>
                                    <span class="mini-day-dots" aria-hidden="true">
                                        {day_dots.into_iter().take(3).map(|col| view! {
                                            <svg viewBox="0 0 6 6"><circle cx="3" cy="3" r="2.5" fill=col></circle></svg>
                                        }).collect_view()}
                                    </span>
                                </button>
                            }
                        }).collect_view()
                    }}
                </div>
            })}

            {move || (!shared.error.get().is_empty()).then(|| view! { <p role="alert" class="error">{shared.error.get()}</p> })}
            {move || (!error.get().is_empty()).then(|| view! {
                <p role="alert" class="error">
                    {error.get()}
                    <button type="button" class="text-button" on:click=move |_| pim.refresh()>"Refresh"</button>
                </p>
            })}
            {move || (!notice.get().is_empty()).then(|| view! { <p role="status" class="notice">{notice.get()}</p> })}

            {main_view}

            {move || (editor.with(Option::is_none)
                && scope.with(Option::is_none)
                && modal.with(Option::is_none))
                .then(|| selected.get())
                .flatten()
                .map(|ev| {
                view! {
                    <EventPeekPopover
                        event=ev
                        detail=detail.get()
                        calendar=selected_calendar.get()
                        zone=zone.get()
                        time_format=time_format.get()
                        can_edit=can_edit.get()
                        on_close=Callback::new(move |()| selected.set(None))
                        on_edit=Callback::new(move |()| {
                            if let Some(d) = detail.get_untracked() {
                                let recurring = d.data["recurring"] == json!(true)
                                    || !s(&d.data, "recurrence").is_empty();
                                if recurring {
                                    scope.set(Some("edit"));
                                } else {
                                    editor.set(Some(Editor {
                                        collection_id: d.collection_id.clone(),
                                        occurrence: None,
                                        data: d.data.clone(),
                                        resource: Some(d),
                                    }));
                                }
                            }
                        })
                        on_duplicate=Callback::new(move |()| duplicate_selected())
                        on_delete=Callback::new(move |()| {
                            if let Some(d) = detail.get_untracked() {
                                let recurring = d.data["recurring"] == json!(true)
                                    || !s(&d.data, "recurrence").is_empty();
                                if recurring {
                                    scope.set(Some("delete"));
                                } else {
                                    modal.set(Some("deleteEvent"));
                                }
                            }
                        })
                    />
                }
            })}

            {move || quick_create.get().map(|draft| {
                let writable: Vec<Collection> = calendars
                    .get_untracked()
                    .into_iter()
                    .filter(|c| c.role != "read" && c.subscription.is_none())
                    .collect();
                let default_cid = writable_target()
                    .map(|c| c.id)
                    .or_else(|| writable.first().map(|c| c.id.clone()))
                    .unwrap_or_default();
                view! {
                    <QuickCreatePopover
                        draft=draft
                        calendars=writable
                        default_calendar_id=default_cid
                        zone=zone.get_untracked()
                        time_format=time_format.get_untracked()
                        on_close=Callback::new(move |()| quick_create.set(None))
                        on_save=save_event
                        on_more_options=Callback::new(move |(payload, cid)| {
                            quick_create.set(None);
                            editor.set(Some(Editor {
                                collection_id: cid,
                                resource: None,
                                occurrence: None,
                                data: payload,
                            }));
                        })
                    />
                }
            })}

            {move || undo_toast.get().map(|toast| view! {
                <div role="status" class="notice calendar-toast">
                    <Icon name="check"/>
                    <span>{toast.message}</span>
                    <button type="button" class="text-button" on:click=move |_| apply_undo()>"Undo"</button>
                    <button type="button" class="icon-button" aria-label="Dismiss notice" on:click=move |_| undo_toast.set(None)>
                        <Icon name="close"/>
                    </button>
                </div>
            })}

            {move || editor.get().map(|e| {
                let writable: Vec<Collection> = calendars
                    .get_untracked()
                    .into_iter()
                    .filter(|c| c.role != "read" && c.subscription.is_none())
                    .collect();
                view! {
                    <EventEditor
                        initial=e
                        calendars=writable
                        on_close=Callback::new(move |()| editor.set(None))
                        on_save=save_event
                    />
                }
            })}

            {move || scope.get().map(|kind| view! {
                <Dialog
                    title=if kind == "edit" { "Edit repeating event" } else { "Delete repeating event" }
                    on_close=Callback::new(move |()| scope.set(None))
                >
                    <p>"Choose how much of the series to change."</p>
                    {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                    <div class="pim-choice-actions">
                        <button
                            type="button"
                            class="secondary-button"
                            disabled=move || busy.get()
                            on:click=move |_| {
                                if kind == "edit" {
                                    edit_occurrence(false);
                                    return;
                                }
                                let (Some(d), Some(sel)) = (detail.get_untracked(), selected.get_untracked()) else {
                                    return;
                                };
                                run(Box::pin(async move {
                                    send::<Value>(
                                        &format!("{BASE}/{}/items/{}", d.collection_id, d.href),
                                        "PUT",
                                        Some(json!({ "cancelOccurrence": s(&sel, "recurrenceId") })),
                                        &csrf(),
                                        Some(etag(&d.id, d.revision)),
                                    )
                                    .await
                                    .map_err(|e| e.message)?;
                                    selected.set(None);
                                    Ok(())
                                }));
                            }
                        >"This event only"</button>
                        <button
                            type="button"
                            class="secondary-button"
                            disabled=move || busy.get()
                            on:click=move |_| {
                                if kind == "edit" {
                                    edit_occurrence(true);
                                } else {
                                    scope.set(None);
                                    modal.set(Some("deleteEvent"));
                                }
                            }
                        >"Whole series"</button>
                        <button type="button" class="text-button" on:click=move |_| scope.set(None)>"Cancel"</button>
                    </div>
                </Dialog>
            })}

            {move || matches!(modal.get(), Some("newCalendar" | "renameCalendar")).then(|| {
                let rename = modal.get_untracked() == Some("renameCalendar");
                let target = rename.then(|| current.get_untracked()).flatten();
                let target_save = target.clone();
                view! {
                    <CollectionEditor
                        title=if rename { "Edit calendar" } else { "New calendar" }
                        collection=target.unwrap_or_else(|| Collection {
                            id: String::new(),
                            owner_id: String::new(),
                            kind: String::new(),
                            name: String::new(),
                            color: "#73396f".into(),
                            revision: 0,
                            role: String::new(),
                            subscription: None,
                        })
                        on_close=Callback::new(move |()| modal.set(None))
                        on_save=Callback::new(move |(name, color, done): (String, String, Done)| {
                            let target = target_save.clone();
                            spawn_local(async move {
                                let res = match target {
                                    Some(c) => send::<Collection>(
                                        &format!("{BASE}/{}", c.id),
                                        "PUT",
                                        Some(json!({ "name": name, "color": color })),
                                        &csrf(),
                                        Some(etag(&c.id, c.revision)),
                                    ).await,
                                    None => send::<Collection>(
                                        BASE,
                                        "POST",
                                        Some(json!({ "name": name, "color": color })),
                                        &csrf(),
                                        None,
                                    ).await,
                                };
                                match res {
                                    Ok(row) => {
                                        managed.set(row.id);
                                        pim.refresh();
                                        done.try_run(Ok(()));
                                    }
                                    Err(e) => {
                                        done.try_run(Err(e.message));
                                    }
                                }
                            });
                        })
                    />
                }
            })}

            {move || matches!(modal.get(), Some("subscribe" | "editSubscription")).then(|| {
                let editing = modal.get_untracked() == Some("editSubscription");
                let target = editing.then(|| current.get_untracked()).flatten();
                let target_save = target.clone();
                let target_refresh = target.clone();
                view! {
                    <SubscriptionEditor
                        calendar=target
                        on_close=Callback::new(move |()| modal.set(None))
                        on_refresh=Callback::new(move |()| {
                            if let Some(c) = target_refresh.clone() {
                                refresh_subscription.run(c);
                            }
                        })
                        on_save=Callback::new(move |(value, done): (Value, Done)| {
                            let target = target_save.clone();
                            spawn_local(async move {
                                let res: Result<Collection, String> = match target {
                                    Some(c) => send(
                                        &format!("{BASE}/{}", c.id),
                                        "PUT",
                                        Some(value),
                                        &csrf(),
                                        Some(etag(&c.id, c.revision)),
                                    ).await.map_err(|e| e.message),
                                    None => send(
                                        "/api/v1/calendar/subscriptions",
                                        "POST",
                                        Some(value),
                                        &csrf(),
                                        None,
                                    ).await.map_err(|e| e.message),
                                };
                                match res {
                                    Ok(row) => {
                                        managed.set(row.id);
                                        pim.refresh();
                                        done.try_run(Ok(()));
                                    }
                                    Err(e) => {
                                        done.try_run(Err(e));
                                    }
                                }
                            });
                        })
                    />
                }
            })}

            {move || (modal.get() == Some("share")).then(|| current.get().map(|c| {
                let base = format!("{BASE}/{}", c.id);
                view! {
                    <Sharing
                        collection=c
                        base=base
                        session=session_sv.get_value()
                        on_close=Callback::new(move |()| modal.set(None))
                        on_changed=Callback::new(move |()| pim.refresh())
                    />
                }
            }))}

            {move || matches!(modal.get(), Some("deleteCalendar" | "deleteEvent")).then(|| {
                let whole = modal.get_untracked() == Some("deleteCalendar");
                view! {
                    <Dialog
                        title=if whole { "Delete calendar?" } else { "Delete event?" }
                        on_close=Callback::new(move |()| modal.set(None))
                    >
                        <p>{move || if whole {
                            match delete_count.get() {
                                None => "Loading calendar details…".to_owned(),
                                Some(n) => format!(
                                    "Permanently delete {} and its {n} items, including events, tasks and recurring occurrences?",
                                    current.with(|c| c.as_ref().map(|c| c.name.clone()).unwrap_or_default())
                                ),
                            }
                        } else {
                            format!(
                                "Permanently delete {}{}?",
                                selected.with(|e| e.as_ref().map(|e| s(e, "title")).unwrap_or_default()),
                                if detail.with(|d| d.as_ref().is_some_and(|d| !s(&d.data, "recurrence").is_empty())) {
                                    " and its entire recurring series"
                                } else {
                                    ""
                                }
                            )
                        }}</p>
                        {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                        <div class="dialog-actions">
                            <button type="button" class="text-button" on:click=move |_| modal.set(None)>"Cancel"</button>
                            <button
                                type="button"
                                class="danger-button"
                                disabled=move || busy.get() || (whole && delete_count.with(Option::is_none))
                                on:click=move |_| {
                                    let (c, d) = (current.get_untracked(), detail.get_untracked());
                                    run(Box::pin(async move {
                                        let (url, tag) = match (whole, &c, &d) {
                                            (true, Some(c), _) => (format!("{BASE}/{}", c.id), etag(&c.id, c.revision)),
                                            (false, _, Some(d)) => (format!("{BASE}/{}/items/{}", d.collection_id, d.href), etag(&d.id, d.revision)),
                                            _ => return Ok(()),
                                        };
                                        send::<Value>(&url, "DELETE", None, &csrf(), Some(tag)).await.map_err(|e| e.message)?;
                                        selected.set(None);
                                        if whole {
                                            managed.set(String::new());
                                        }
                                        notice.set("Deleted.".into());
                                        Ok(())
                                    }));
                                }
                            >"Delete permanently"</button>
                        </div>
                    </Dialog>
                }
            })}

            {move || (modal.get() == Some("reminders")).then(|| view! {
                <Dialog title="Reminders" on_close=Callback::new(move |()| modal.set(None))>
                    {move || if reminders.with(Vec::is_empty) {
                        view! { <p>"No due reminders."</p> }.into_any()
                    } else {
                        reminders.get().into_iter().map(|r| {
                            let id = s(&r, "id");
                            view! {
                                <div class="grant">
                                    <span>
                                        <strong>{s(&r, "title")}</strong>
                                        <small>{crate::dates::local_date_time(at(&r, "dueAt"), &[])}</small>
                                    </span>
                                    <button type="button" class="text-button" on:click=move |_| {
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

            {move || (modal.get() == Some("shortcuts")).then(|| view! {
                <ShortcutsDialog on_close=Callback::new(move |()| modal.set(None))/>
            })}
        </PimShell>
    }
}

/// Adds `has-time-grid` to the main region while a time grid shows.
#[component]
fn TimeGridClass(active: Signal<bool>) -> impl IntoView {
    Effect::new(move |_| {
        let on = active.get();
        if let Some(main) = crate::ui::document()
            .query_selector(".pim-main.calendar-main")
            .ok()
            .flatten()
        {
            let _ = if on {
                main.class_list().add_1("has-time-grid")
            } else {
                main.class_list().remove_1("has-time-grid")
            };
        }
    });
}
