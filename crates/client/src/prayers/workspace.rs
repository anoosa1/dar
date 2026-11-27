//! Prayers workspace coordinator: loads settings and yearly prayer logs, runs 1-second
//! countdown tick, manages URL/view/date state, optimistic + revisioned log mutations,
//! responsive right sheets / mobile views, and SettingsSheet integration.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use futures::lock::Mutex;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::api::BrowserSession;
use crate::prayers::api::{
    fetch_settings, fetch_year_log, patch_day_prayer, put_settings,
};
use crate::prayers::calc::{compute_current_window, compute_day_times};
use crate::prayers::export::{
    format_day_clipboard, generate_csv, generate_ics, generate_json_backup,
};
use crate::prayers::model::{
    Location, PrayerKey, PrayerSettings, PrayerStatus, PrayersView, StoredSettings, YearLog,
    ensure_location_listed,
};
use crate::prayers::month::MonthMain;
use crate::prayers::qibla::{QiblaMain, QiblaRightSheet};
use crate::prayers::settings::PrayersSettings;
use crate::prayers::setup::PrayersSetup;
use crate::prayers::sidebar::PrayersSidebar;
use crate::prayers::today::{TodayMain, TodayRightSheet};
use crate::prayers::tz::{
    self, CivilDate, copy_to_clipboard, days_in_month, trigger_download,
};
use crate::prayers::week::{WeekDayInspector, WeekMain};
use crate::settings::Notice;
use crate::shell::WorkspaceHeader;
use crate::ui::{document, listen, window};

fn read_url_state() -> (PrayersView, Option<CivilDate>) {
    let search = window().location().search().unwrap_or_default();
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok();
    let view = params
        .as_ref()
        .and_then(|p| p.get("view"))
        .map(|v| PrayersView::from_str(&v))
        .unwrap_or(PrayersView::Today);
    let date = params
        .as_ref()
        .and_then(|p| p.get("date"))
        .and_then(|d| CivilDate::parse_iso(&d));
    (view, date)
}

fn replace_url_state(view: PrayersView) {
    let url = if view == PrayersView::Today {
        "/prayers".to_owned()
    } else {
        format!("/prayers?view={}", view.as_str())
    };
    if let Ok(history) = window().history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
    }
}

#[component]
pub fn PrayersWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let (start_view, start_date) = read_url_state();
    let stored: RwSignal<Option<StoredSettings>> = RwSignal::new(None);
    let loaded = RwSignal::new(false);
    let load_error = RwSignal::new(String::new());
    let logs: RwSignal<BTreeMap<u32, YearLog>> = RwSignal::new(BTreeMap::new());
    let loaded_years = StoredValue::new(HashSet::<u32>::new());

    let view = RwSignal::new(start_view);
    let now_ms = RwSignal::new(tz::now_ms());
    let settings_open = RwSignal::new(false);
    let mobile_details_open = RwSignal::new(false);
    let desktop_details_hidden = RwSignal::new(false);
    let previous_settings: RwSignal<Option<PrayerSettings>> = RwSignal::new(None);
    let toast: RwSignal<Option<Notice>> = RwSignal::new(None);

    let today_now = Memo::new(move |_| {
        let now = now_ms.get();
        let tz_name = stored
            .with(|s| s.as_ref().map(|x| x.value.location.timezone.clone()))
            .unwrap_or_else(tz::browser_timezone);
        tz::civil_date_in_tz(now, &tz_name)
    });

    let init_today = today_now.get_untracked();
    let initial_sel = start_date.unwrap_or(init_today);
    let selected_date = RwSignal::new(initial_sel);
    let week_start = RwSignal::new(initial_sel.add_days(-(initial_sel.weekday() as i64)));
    let mini_month = RwSignal::new((initial_sel.year, initial_sel.month));

    let session_sv = StoredValue::new(session);
    let save_queue = StoredValue::new(Arc::new(Mutex::new(())));
    let log_queue = StoredValue::new(Arc::new(Mutex::new(())));

    let load_year = move |year: u32| {
        if !(2000..=2099).contains(&year) {
            return;
        }
        let already = loaded_years.with_value(|s| s.contains(&year));
        if already {
            return;
        }
        loaded_years.update_value(|s| {
            s.insert(year);
        });
        spawn_local(async move {
            match fetch_year_log(year).await {
                Ok(yl) => {
                    let _ = logs.try_update(|map| {
                        map.insert(year, yl);
                    });
                }
                Err(_) => {
                    let _ = loaded_years.try_update_value(|s| {
                        s.remove(&year);
                    });
                }
            }
        });
    };

    // Initial settings + current year log fetch.
    let reload_all = Callback::new(move |()| {
        load_error.set(String::new());
        spawn_local(async move {
            match fetch_settings().await {
                Ok(val) => {
                    let _ = stored.try_set(val);
                    let _ = loaded.try_set(true);
                    let _ = load_error.try_set(String::new());
                }
                Err(e) => {
                    let _ = load_error.try_set(e.message);
                    let _ = loaded.try_set(true);
                }
            }
        });
    });
    reload_all.run(());

    // Fetch year logs for active date/month/year whenever they change.
    Effect::new(move |_| {
        if stored.with(Option::is_some) {
            let t = today_now.get();
            let s = selected_date.get();
            let (my, _) = mini_month.get();
            load_year(t.year as u32);
            load_year((t.year - 1) as u32);
            load_year(s.year as u32);
            load_year(my as u32);
        }
    });

    // Live 1-second clock tick, cleaned up deterministically on unmount.
    let interval_handle: StoredValue<Option<i32>> = StoredValue::new(None);
    let interval_cb: StoredValue<Option<SendWrapper<Closure<dyn Fn()>>>> = StoredValue::new(None);
    let cb = Closure::<dyn Fn()>::new(move || {
        let _ = now_ms.try_set(tz::now_ms());
    });
    if let Ok(id) = window().set_interval_with_callback_and_timeout_and_arguments_0(
        cb.as_ref().unchecked_ref(),
        1000,
    ) {
        interval_handle.set_value(Some(id));
        interval_cb.set_value(Some(SendWrapper::new(cb)));
    }
    on_cleanup(move || {
        if let Some(id) = interval_handle.try_get_value().flatten() {
            window().clear_interval_with_handle(id);
        }
        interval_cb.try_update_value(|slot| {
            slot.take();
        });
    });

    // Clear toast cleanly after 4s.
    let toast_gen = RwSignal::new(0u32);
    Effect::new(move |_| {
        if toast.with(Option::is_none) {
            return;
        }
        let g = toast_gen.get_untracked() + 1;
        toast_gen.set(g);
        spawn_local(async move {
            crate::sleep(4000).await;
            if toast_gen.try_get_untracked() == Some(g) {
                let _ = toast.try_set(None);
            }
        });
    });

    let choose_view = Callback::new(move |next: PrayersView| {
        view.set(next);
        mobile_details_open.set(false);
        desktop_details_hidden.set(false);
        replace_url_state(next);
    });

    let select_day = Callback::new(move |d: CivilDate| {
        selected_date.set(d);
        week_start.set(d.add_days(-(d.weekday() as i64)));
        mini_month.set((d.year, d.month));
        if view.get_untracked() != PrayersView::Week {
            view.set(PrayersView::Week);
            replace_url_state(PrayersView::Week);
        }
        desktop_details_hidden.set(false);
        mobile_details_open.set(true);
    });

    let set_prayer_status =
        Callback::new(move |(date, prayer, next_status): (CivilDate, PrayerKey, PrayerStatus)| {
            if !prayer.is_obligatory() {
                return;
            }
            let year = date.year as u32;
            let iso = date.format_iso();
            // Optimistic local state update.
            logs.update(|map| {
                let entry = map.entry(year).or_insert_with(|| YearLog {
                    year,
                    revision: 0,
                    days: BTreeMap::new(),
                });
                let mut dl = entry.days.get(&iso).copied().unwrap_or_default();
                dl.set(prayer, next_status);
                if dl.is_all_unprayed() {
                    entry.days.remove(&iso);
                } else {
                    entry.days.insert(iso.clone(), dl);
                }
            });

            let csrf = session_sv.with_value(|s| s.csrf_token.clone());
            let q = log_queue.get_value();
            spawn_local(async move {
                let _guard = q.lock().await;
                let rev = logs
                    .try_with_untracked(|m| m.get(&year).map_or(0, |yl| yl.revision))
                    .unwrap_or(0);
                let mut res = patch_day_prayer(&csrf, rev, &iso, prayer, next_status).await;
                if let Err(ref e) = res
                    && e.status == 409
                    && let Ok(fresh) = fetch_year_log(year).await
                {
                    let fresh_rev = fresh.revision;
                    let _ = logs.try_update(|map| {
                        map.insert(year, fresh);
                    });
                    res = patch_day_prayer(&csrf, fresh_rev, &iso, prayer, next_status).await;
                }
                match res {
                    Ok(resp) => {
                        let _ = logs.try_update(|map| {
                            map.insert(
                                resp.year,
                                YearLog {
                                    year: resp.year,
                                    revision: resp.revision,
                                    days: resp.days,
                                },
                            );
                        });
                    }
                    Err(e) => {
                        if let Ok(fresh) = fetch_year_log(year).await {
                            let _ = logs.try_update(|map| {
                                map.insert(year, fresh);
                            });
                        }
                        let _ = toast.try_set(Some(Notice {
                            message: e.message,
                            undo: None,
                        }));
                    }
                }
            });
        });

    let cycle_prayer = Callback::new(move |(date, prayer): (CivilDate, PrayerKey)| {
        let iso = date.format_iso();
        let cur = logs
            .with_untracked(|m| {
                m.get(&(date.year as u32))
                    .and_then(|yl| yl.days.get(&iso))
                    .map(|dl| dl.get(prayer))
            })
            .unwrap_or(PrayerStatus::Unprayed);
        set_prayer_status.run((date, prayer, cur.next_cycle()));
    });

    let save_settings_direct = Callback::new(move |mut next: PrayerSettings| {
        ensure_location_listed(&mut next);
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        let q = save_queue.get_value();
        spawn_local(async move {
            let _guard = q.lock().await;
            let rev = stored
                .try_get_untracked()
                .flatten()
                .map_or(0, |s| s.revision);
            match put_settings(&csrf, rev, &next).await {
                Ok(saved) => {
                    let _ = stored.try_set(Some(saved));
                    let _ = previous_settings.try_set(None);
                }
                Err(e) => {
                    let _ = toast.try_set(Some(Notice {
                        message: e.message,
                        undo: None,
                    }));
                }
            }
        });
    });

    let switch_location = Callback::new(move |loc: Location| {
        if let Some(cur) = stored.get_untracked() {
            let mut next = cur.value;
            next.location = loc;
            save_settings_direct.run(next);
        }
    });

    let copy_day = Callback::new(move |d: CivilDate| {
        let Some(cur) = stored.get_untracked() else { return };
        let times = compute_day_times(d, &cur.value.location, &cur.value.calculation);
        let use_24h = cur.value.display.time_format == "24h";
        let txt = format_day_clipboard(
            d,
            &times,
            &cur.value.location,
            use_24h,
            cur.value.display.hijri_offset,
        );
        spawn_local(async move {
            let _ = copy_to_clipboard(&txt).await;
            let _ = toast.try_set(Some(Notice {
                message: "Prayer times copied".into(),
                undo: None,
            }));
        });
    });

    let export_day_ics = Callback::new(move |d: CivilDate| {
        let Some(cur) = stored.get_untracked() else { return };
        let times = compute_day_times(d, &cur.value.location, &cur.value.calculation);
        let title = format!("Prayer Times \u{2014} {}", cur.value.location.label);
        let ics = generate_ics(&[times], &cur.value.location, &title);
        trigger_download(&format!("prayers-{}.ics", d.format_iso()), "text/calendar", &ics);
    });

    let export_month_ics = Callback::new(move |(year, month): (i32, u8)| {
        let Some(cur) = stored.get_untracked() else { return };
        let count = days_in_month(year, month);
        let days: Vec<_> = (1..=count)
            .filter_map(|day| CivilDate::from_ymd(year, month, day))
            .map(|d| compute_day_times(d, &cur.value.location, &cur.value.calculation))
            .collect();
        let title = format!(
            "Prayer Times \u{2014} {} {:04}-{:02}",
            cur.value.location.label, year, month
        );
        let ics = generate_ics(&days, &cur.value.location, &title);
        trigger_download(&format!("prayers-{year:04}-{month:02}.ics"), "text/calendar", &ics);
    });

    let export_month_csv = Callback::new(move |(year, month): (i32, u8)| {
        let Some(cur) = stored.get_untracked() else { return };
        let count = days_in_month(year, month);
        let days: Vec<_> = (1..=count)
            .filter_map(|day| CivilDate::from_ymd(year, month, day))
            .map(|d| compute_day_times(d, &cur.value.location, &cur.value.calculation))
            .collect();
        let year_days = logs
            .with_untracked(|m| m.get(&(year as u32)).map(|yl| yl.days.clone()))
            .unwrap_or_default();
        let csv = generate_csv(
            &days,
            &cur.value.location,
            &year_days,
            cur.value.display.time_format == "24h",
            cur.value.display.hijri_offset,
        );
        trigger_download(&format!("prayers-{year:04}-{month:02}.csv"), "text/csv", &csv);
    });

    let export_json = Callback::new(move |()| {
        let Some(cur) = stored.get_untracked() else { return };
        let all_logs = logs.get_untracked();
        let json_str = generate_json_backup(&cur.value, &all_logs);
        trigger_download("prayer-times-backup.json", "application/json", &json_str);
    });

    // Keyboard shortcuts: 1/2/3/4 views, t Today, s Settings.
    listen(&document(), "keydown", false, move |event| {
        if settings_open.try_get_untracked() == Some(true) {
            return;
        }
        let ke: web_sys::KeyboardEvent = event.unchecked_into();
        if ke.meta_key() || ke.ctrl_key() || ke.alt_key() {
            return;
        }
        if let Some(el) = ke
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            && el.closest("input,textarea,select,[role=\"dialog\"]").ok().flatten().is_some()
        {
            return;
        }
        match ke.key().as_str() {
            "1" => {
                ke.prevent_default();
                choose_view.try_run(PrayersView::Today);
            }
            "2" => {
                ke.prevent_default();
                choose_view.try_run(PrayersView::Week);
            }
            "3" => {
                ke.prevent_default();
                choose_view.try_run(PrayersView::Month);
            }
            "4" => {
                ke.prevent_default();
                choose_view.try_run(PrayersView::Qibla);
            }
            "t" | "T" => {
                ke.prevent_default();
                let _ = selected_date.try_set(today_now.get_untracked());
                choose_view.try_run(PrayersView::Today);
            }
            "s" | "S" => {
                ke.prevent_default();
                let _ = settings_open.try_set(true);
            }
            _ => {}
        }
    });

    let settings_val = Signal::derive(move || stored.get().map(|s| s.value));
    let configured_settings = Signal::derive(move || {
        stored
            .get()
            .map(|s| s.value)
            .unwrap_or_else(PrayerSettings::default)
    });
    let now_sig = Signal::derive(move || now_ms.get());
    let today_sig = Signal::derive(move || today_now.get());

    // Keep browser title concise with current next prayer when configured.
    Effect::new(move |_| {
        if let Some(s) = settings_val.get() {
            let win = compute_current_window(now_ms.get(), &s.location, &s.calculation);
            let next_str = tz::format_time_in_tz(
                win.next_time_ms,
                &s.location.timezone,
                s.display.time_format == "24h",
            );
            document().set_title(&format!(
                "{} at {next_str} \u{2014} Prayer Times \u{2014} Dar",
                win.next_prayer.label()
            ));
        } else {
            document().set_title("Prayer Times \u{2014} Dar");
        }
    });

    let shell_class = move || {
        let configured = stored.with(Option::is_some);
        let cur_v = view.get();
        let right_open = configured
            && matches!(cur_v, PrayersView::Today | PrayersView::Qibla | PrayersView::Week)
            && (cur_v != PrayersView::Week || !desktop_details_hidden.get());
        let mut cls = String::from("app-shell pim-shell prayers-shell");
        if right_open {
            cls.push_str(" has-prayers-right");
        }
        if configured && mobile_details_open.get() {
            cls.push_str(" has-pim-details");
        }
        cls
    };

    view! {
        <div class=shell_class>
            <WorkspaceHeader
                app="prayers"
                session=session_sv.get_value()
                on_signed_out=on_signed_out
                on_settings=Callback::new(move |_| settings_open.set(true))
            />

            <div class="pim-workspace">
                <PrayersSidebar
                    view=view
                    settings=settings_val
                    logs=logs
                    today=today_sig
                    selected_date=selected_date
                    mini_month=mini_month
                    on_choose_view=choose_view
                    on_select_date=select_day
                    on_switch_location=switch_location
                    on_add_location=Callback::new(move |_| settings_open.set(true))
                    on_settings=Callback::new(move |_| settings_open.set(true))
                />

                <main class="pim-main prayers-main" tabindex="-1">
                    {move || (!load_error.get().is_empty()).then(|| view! {
                        <div class="prayers-banner" role="alert">
                            <span>{load_error.get()}</span>
                            <button type="button" class="secondary-button" on:click=move |_| reload_all.run(())>
                                "Try again"
                            </button>
                        </div>
                    })}

                    {move || {
                        if !loaded.get() {
                            return view! {
                                <div class="prayers-sheet-body">
                                    <p class="prayers-inline-status" role="status">"Loading Prayer Times\u{2026}"</p>
                                </div>
                            }.into_any();
                        }
                        if stored.with(Option::is_none) {
                            return view! {
                                <PrayersSetup
                                    on_pick_settings=save_settings_direct
                                    on_open_settings=Callback::new(move |_| settings_open.set(true))
                                />
                            }.into_any();
                        }
                        view! {
                            <div class="prayers-mobile-tabs" role="tablist" aria-label="Views">
                                {[PrayersView::Today, PrayersView::Week, PrayersView::Month, PrayersView::Qibla].into_iter().map(|tab| {
                                    view! {
                                        <button
                                            type="button"
                                            role="tab"
                                            class=move || if view.get() == tab { "prayers-mobile-tab is-active" } else { "prayers-mobile-tab" }
                                            aria-selected=move || (view.get() == tab).to_string()
                                            on:click=move |_| choose_view.run(tab)
                                        >
                                            {tab.label()}
                                        </button>
                                    }
                                }).collect_view()}
                            </div>

                            {move || match view.get() {
                                PrayersView::Today => view! {
                                    <TodayMain
                                        settings=configured_settings
                                        logs=logs
                                        now_ms=now_sig
                                        on_cycle_prayer=cycle_prayer
                                        on_copy_day=copy_day
                                        on_open_week=Callback::new(move |d| {
                                            selected_date.set(d);
                                            week_start.set(d.add_days(-(d.weekday() as i64)));
                                            choose_view.run(PrayersView::Week);
                                        })
                                    />
                                }.into_any(),
                                PrayersView::Week => view! {
                                    <WeekMain
                                        settings=configured_settings
                                        previous_settings=previous_settings
                                        logs=logs
                                        now_ms=now_sig
                                        selected_date=selected_date
                                        week_start=week_start
                                        on_select_day=select_day
                                    />
                                }.into_any(),
                                PrayersView::Month => view! {
                                    <MonthMain
                                        settings=configured_settings
                                        logs=logs
                                        now_ms=now_sig
                                        mini_month=mini_month
                                        on_select_day=select_day
                                        on_export_month_ics=export_month_ics
                                        on_export_month_csv=export_month_csv
                                        on_export_json=export_json
                                    />
                                }.into_any(),
                                PrayersView::Qibla => view! {
                                    <QiblaMain
                                        settings=configured_settings
                                        now_ms=now_sig
                                    />
                                }.into_any(),
                            }}
                        }.into_any()
                    }}
                </main>

                <Show when=move || stored.with(Option::is_some)>
                    <aside
                        class="pim-details prayers-details"
                        role="complementary"
                        aria-label="Day details"
                    >
                        {move || match view.get() {
                            PrayersView::Week => view! {
                                <WeekDayInspector
                                    settings=configured_settings
                                    logs=logs
                                    now_ms=now_sig
                                    selected_date=selected_date
                                    mobile_details_open=mobile_details_open
                                    on_set_status=set_prayer_status
                                    on_copy_day=copy_day
                                    on_export_day_ics=export_day_ics
                                    on_open_settings=Callback::new(move |_| settings_open.set(true))
                                    on_close_desktop=Callback::new(move |_| desktop_details_hidden.set(true))
                                />
                            }.into_any(),
                            PrayersView::Qibla => view! {
                                <QiblaRightSheet
                                    settings=configured_settings
                                    on_switch_location=switch_location
                                />
                            }.into_any(),
                            PrayersView::Today => view! {
                                <TodayRightSheet
                                    settings=configured_settings
                                    logs=logs
                                    now_ms=now_sig
                                    on_cycle_prayer=cycle_prayer
                                    on_open_day_inspector=select_day
                                    on_open_qibla=Callback::new(move |_| choose_view.run(PrayersView::Qibla))
                                />
                            }.into_any(),
                            PrayersView::Month => ().into_any(),
                        }}
                    </aside>
                </Show>
            </div>

            {move || settings_open.get().then(|| view! {
                <PrayersSettings
                    session=session_sv.get_value()
                    stored=stored
                    save_queue=save_queue.get_value()
                    on_saved=Callback::new(move |(saved, prev): (StoredSettings, Option<PrayerSettings>)| {
                        if let Some(p) = prev {
                            previous_settings.set(Some(p));
                        }
                        let _ = saved;
                    })
                    on_undo_cleared=Callback::new(move |_| previous_settings.set(None))
                    on_close=Callback::new(move |_| settings_open.set(false))
                />
            })}

            <div class="settings-toast-region" role="status" aria-live="polite">
                {move || toast.get().map(|n| view! {
                    <div class="settings-toast"><span>{n.message}</span></div>
                })}
            </div>
        </div>
    }
}
