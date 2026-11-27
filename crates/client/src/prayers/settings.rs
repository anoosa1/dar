//! Prayer Times settings sheet: Location (offline city lookup + geolocation + saved list),
//! Calculation (10 methods + Asr + High-latitude + Custom), Offsets, and Display.
//! Implements save-as-you-go with If-Match concurrency, Undo toast, save-on-close, and 409 conflict reload.

use std::sync::Arc;

use futures::lock::Mutex;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::BrowserSession;
use crate::icons::Icon;
use crate::prayers::api::{fetch_settings, put_settings};
use crate::prayers::cities::search_cities;
use crate::prayers::model::{
    ALL_PRAYERS, Location, METHODS, OffsetMinutes, PrayerSettings, SavedLocation, StoredSettings,
    ensure_location_listed, method_short_label, slug_id, validate_label, validate_latitude,
    validate_longitude, validate_offset, validate_timezone,
};
use crate::prayers::tz;
use crate::settings::{Notice, SaveState, Segmented, SettingRow, SettingSwitch, SettingsSheet};
use crate::ui::Select;

#[component]
pub fn PrayersSettings(
    session: BrowserSession,
    stored: RwSignal<Option<StoredSettings>>,
    save_queue: Arc<Mutex<()>>,
    on_saved: Callback<(StoredSettings, Option<PrayerSettings>)>,
    on_undo_cleared: Callback<()>,
    on_close: Callback<()>,
) -> impl IntoView {
    let section = RwSignal::new("Location");
    let notice: RwSignal<Option<Notice>> = RwSignal::new(None);
    let conflict = RwSignal::new(false);
    let geo_status = RwSignal::new(String::new());
    let city_query = RwSignal::new(String::new());

    let initial_loc = stored
        .get_untracked()
        .map(|s| s.value.location)
        .unwrap_or_default();
    let label_draft = RwSignal::new(initial_loc.label.clone());
    let lat_draft = RwSignal::new(if initial_loc.label.is_empty() && initial_loc.latitude == 0.0 {
        String::new()
    } else {
        format!("{}", initial_loc.latitude)
    });
    let lon_draft = RwSignal::new(if initial_loc.label.is_empty() && initial_loc.longitude == 0.0 {
        String::new()
    } else {
        format!("{}", initial_loc.longitude)
    });
    let tz_draft = RwSignal::new(initial_loc.timezone);

    let label_err: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let lat_err: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let lon_err: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let tz_err: RwSignal<Option<&'static str>> = RwSignal::new(None);

    let loc_state: RwSignal<Option<SaveState>> = RwSignal::new(None);
    let calc_state: RwSignal<Option<SaveState>> = RwSignal::new(None);
    let offsets_state: RwSignal<Option<SaveState>> = RwSignal::new(None);
    let display_state: RwSignal<Option<SaveState>> = RwSignal::new(None);

    // Local draft calculation & display before the first setup save lands.
    let draft_calc = RwSignal::new(
        stored
            .get_untracked()
            .map(|s| s.value.calculation)
            .unwrap_or_default(),
    );
    let draft_display = RwSignal::new(
        stored
            .get_untracked()
            .map(|s| s.value.display)
            .unwrap_or_default(),
    );
    let draft_locations = RwSignal::new(
        stored
            .get_untracked()
            .map(|s| s.value.locations)
            .unwrap_or_default(),
    );

    let sync_location_drafts = move |loc: &Location| {
        label_draft.set(loc.label.clone());
        lat_draft.set(format!("{}", loc.latitude));
        lon_draft.set(format!("{}", loc.longitude));
        tz_draft.set(loc.timezone.clone());
        label_err.set(None);
        lat_err.set(None);
        lon_err.set(None);
        tz_err.set(None);
    };

    let validate_all_location = move || -> Option<Location> {
        let l = validate_label(&label_draft.get_untracked());
        let la = validate_latitude(&lat_draft.get_untracked());
        let lo = validate_longitude(&lon_draft.get_untracked());
        let t = validate_timezone(&tz_draft.get_untracked());
        label_err.set(l.as_ref().err().copied());
        lat_err.set(la.as_ref().err().copied());
        lon_err.set(lo.as_ref().err().copied());
        tz_err.set(t.as_ref().err().copied());
        match (l, la, lo, t) {
            (Ok(label), Ok(latitude), Ok(longitude), Ok(timezone)) => Some(Location {
                label,
                latitude,
                longitude,
                timezone,
            }),
            _ => None,
        }
    };

    let session_sv = StoredValue::new(session);
    let queue_sv = StoredValue::new(save_queue);

    let commit_settings = Callback::new(
        move |(mut next, msg, state_sig, allow_undo, highlight_diff): (
            PrayerSettings,
            String,
            RwSignal<Option<SaveState>>,
            bool,
            bool,
        )| {
            ensure_location_listed(&mut next);
            let csrf = session_sv.with_value(|s| s.csrf_token.clone());
            let q = queue_sv.get_value();
            let _ = state_sig.try_set(Some(SaveState::Saving));
            spawn_local(async move {
                let _guard = q.lock().await;
                let prev = stored.try_get_untracked().flatten();
                let rev = prev.as_ref().map_or(0, |s| s.revision);
                match put_settings(&csrf, rev, &next).await {
                    Ok(saved) => {
                        let _ = conflict.try_set(false);
                        let _ = stored.try_set(Some(saved.clone()));
                        let _ = draft_calc.try_set(saved.value.calculation.clone());
                        let _ = draft_display.try_set(saved.value.display.clone());
                        let _ = draft_locations.try_set(saved.value.locations.clone());
                        let _ = state_sig.try_set(Some(SaveState::Saved));
                        let diff_prev = if highlight_diff {
                            prev.as_ref().map(|p| p.value.clone())
                        } else {
                            None
                        };
                        let _ = on_saved.try_run((saved.clone(), diff_prev));

                        if allow_undo && let Some(prev_rec) = prev {
                            let prev_val = prev_rec.value;
                            let csrf_undo = csrf.clone();
                            let q_undo = q.clone();
                            let undo_cb: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                                let pv = prev_val.clone();
                                let c = csrf_undo.clone();
                                let qu = q_undo.clone();
                                spawn_local(async move {
                                    let _ug = qu.lock().await;
                                    let cur_rev = stored
                                        .try_get_untracked()
                                        .flatten()
                                        .map_or(0, |s| s.revision);
                                    if let Ok(restored) = put_settings(&c, cur_rev, &pv).await {
                                        let _ = stored.try_set(Some(restored.clone()));
                                        let _ = draft_calc.try_set(restored.value.calculation.clone());
                                        let _ = draft_display.try_set(restored.value.display.clone());
                                        let _ = draft_locations.try_set(restored.value.locations.clone());
                                        sync_location_drafts(&restored.value.location);
                                        let _ = on_saved.try_run((restored, None));
                                        let _ = on_undo_cleared.try_run(());
                                        let _ = notice.try_set(Some(Notice {
                                            message: "Change undone".into(),
                                            undo: None,
                                        }));
                                    }
                                });
                            });
                            let _ = notice.try_set(Some(Notice {
                                message: msg,
                                undo: Some(undo_cb),
                            }));
                        } else {
                            let _ = notice.try_set(Some(Notice {
                                message: msg,
                                undo: None,
                            }));
                        }
                    }
                    Err(e) if e.status == 409 => {
                        let _ = state_sig.try_set(Some(SaveState::Error));
                        let _ = conflict.try_set(true);
                    }
                    Err(_) => {
                        let _ = state_sig.try_set(Some(SaveState::Error));
                    }
                }
            });
        },
    );

    let save_location_if_configured = move |silent_if_invalid: bool| {
        let Some(loc) = validate_all_location() else {
            if !silent_if_invalid {
                let _ = loc_state.try_set(Some(SaveState::Error));
            }
            return;
        };
        let Some(cur) = stored.get_untracked() else {
            // Setup mode: keep draft ready until Calculation method is chosen.
            return;
        };
        let mut next = cur.value.clone();
        if next.location == loc {
            return;
        }
        next.location = loc;
        commit_settings.run((next, "Location saved".into(), loc_state, true, false));
    };

    let handle_close = Callback::new(move |()| {
        if !conflict.get_untracked()
            && stored.get_untracked().is_some()
            && let Some(loc) = validate_all_location()
        {
            let cur_loc = stored.get_untracked().map(|s| s.value.location);
            if cur_loc.as_ref() != Some(&loc) {
                save_location_if_configured(true);
            }
        }
        on_close.try_run(());
    });

    let reload_from_server = move |_| {
        spawn_local(async move {
            if let Ok(Some(fresh)) = fetch_settings().await {
                let _ = stored.try_set(Some(fresh.clone()));
                let _ = draft_calc.try_set(fresh.value.calculation.clone());
                let _ = draft_display.try_set(fresh.value.display.clone());
                let _ = draft_locations.try_set(fresh.value.locations.clone());
                sync_location_drafts(&fresh.value.location);
                let _ = conflict.try_set(false);
                let _ = loc_state.try_set(Some(SaveState::Saved));
                let _ = calc_state.try_set(Some(SaveState::Saved));
                let _ = on_saved.try_run((fresh, None));
            }
        });
    };

    let on_use_location = move |_| {
        geo_status.set(String::new());
        spawn_local(async move {
            match tz::request_geolocation().await {
                Ok((lat, lon)) => {
                    let lat_r = (lat * 10000.0).round() / 10000.0;
                    let lon_r = (lon * 10000.0).round() / 10000.0;
                    let _ = lat_draft.try_set(format!("{lat_r}"));
                    let _ = lon_draft.try_set(format!("{lon_r}"));
                    let _ = tz_draft.try_set(tz::browser_timezone());
                    if label_draft.with_untracked(|s| s.trim().is_empty()) {
                        let _ = label_draft.try_set("My location".into());
                    }
                    save_location_if_configured(true);
                }
                Err(msg) => {
                    let _ = geo_status.try_set(msg);
                }
            }
        });
    };

    let current_settings_or_draft = move || -> Option<PrayerSettings> {
        if let Some(s) = stored.get_untracked() {
            Some(s.value)
        } else {
            validate_all_location().map(|loc| PrayerSettings {
                location: loc,
                locations: draft_locations.get_untracked(),
                calculation: draft_calc.get_untracked(),
                display: draft_display.get_untracked(),
            })
        }
    };

    let high_lat_val = RwSignal::new(draft_calc.get_untracked().high_latitude_rule);
    Effect::new(move |_| {
        let rule = stored
            .get()
            .map(|s| s.value.calculation.high_latitude_rule)
            .unwrap_or_else(|| draft_calc.get().high_latitude_rule);
        if high_lat_val.get_untracked() != rule {
            high_lat_val.set(rule);
        }
    });

    view! {
        <SettingsSheet
            title="Prayer Times settings"
            icon="clock"
            sections=vec!["Location", "Calculation", "Offsets", "Display"]
            section=section
            on_close=handle_close
            notice=notice
        >
            {move || conflict.get().then(|| view! {
                <div class="settings-conflict" role="alert">
                    <p>"Prayer Times settings changed elsewhere."</p>
                    <button type="button" class="secondary-button" on:click=reload_from_server>
                        "Reload saved settings"
                    </button>
                </div>
            })}

            {move || match section.get() {
                "Location" => view! {
                    <section class="settings-group" aria-label="Active location">
                        <h3>"Location"</h3>
                        <form on:submit=move |ev| {
                            ev.prevent_default();
                            save_location_if_configured(false);
                        }>
                            <SettingRow label="Coordinates & zone" hint="Stored with your account" state=loc_state stacked=true>
                                <div class="prayers-location-fields">
                                    <label>
                                        <span>"Location name"</span>
                                        <input
                                            type="text"
                                            prop:value=move || label_draft.get()
                                            aria-invalid=move || label_err.get().is_some().to_string()
                                            on:input=move |ev| {
                                                label_draft.set(event_target_value(&ev));
                                                label_err.set(None);
                                            }
                                            on:blur=move |_| if !conflict.get_untracked() { save_location_if_configured(false); }
                                        />
                                    </label>
                                    {move || label_err.get().map(|m| view! { <p class="field-error" role="alert">{m}</p> })}

                                    <div class="prayer-coordinate-grid">
                                        <div>
                                            <label>
                                                <span>"Latitude"</span>
                                                <input
                                                    type="text"
                                                    inputmode="decimal"
                                                    prop:value=move || lat_draft.get()
                                                    aria-invalid=move || lat_err.get().is_some().to_string()
                                                    on:input=move |ev| {
                                                        lat_draft.set(event_target_value(&ev));
                                                        lat_err.set(None);
                                                    }
                                                    on:blur=move |_| if !conflict.get_untracked() { save_location_if_configured(false); }
                                                />
                                            </label>
                                            {move || lat_err.get().map(|m| view! { <p class="field-error" role="alert">{m}</p> })}
                                        </div>
                                        <div>
                                            <label>
                                                <span>"Longitude"</span>
                                                <input
                                                    type="text"
                                                    inputmode="decimal"
                                                    prop:value=move || lon_draft.get()
                                                    aria-invalid=move || lon_err.get().is_some().to_string()
                                                    on:input=move |ev| {
                                                        lon_draft.set(event_target_value(&ev));
                                                        lon_err.set(None);
                                                    }
                                                    on:blur=move |_| if !conflict.get_untracked() { save_location_if_configured(false); }
                                                />
                                            </label>
                                            {move || lon_err.get().map(|m| view! { <p class="field-error" role="alert">{m}</p> })}
                                        </div>
                                    </div>

                                    <label>
                                        <span>"Timezone"</span>
                                        <input
                                            type="text"
                                            prop:value=move || tz_draft.get()
                                            aria-invalid=move || tz_err.get().is_some().to_string()
                                            on:input=move |ev| {
                                                tz_draft.set(event_target_value(&ev));
                                                tz_err.set(None);
                                            }
                                            on:blur=move |_| if !conflict.get_untracked() { save_location_if_configured(false); }
                                        />
                                    </label>
                                    {move || tz_err.get().map(|m| view! { <p class="field-error" role="alert">{m}</p> })}

                                    <button type="button" class="secondary-button prayer-locate" on:click=on_use_location>
                                        <Icon name="location"/>"Use my location"
                                    </button>
                                    {move || {
                                        let msg = geo_status.get();
                                        (!msg.is_empty()).then(|| view! { <p class="prayers-inline-status" role="status">{msg}</p> })
                                    }}
                                    <button type="submit" hidden></button>
                                </div>
                            </SettingRow>
                        </form>

                        <SettingRow label="City quick-fill" hint="Offline world city lookup" state=Signal::stored(None) stacked=true>
                            <div class="prayers-location-fields">
                                <input
                                    type="search"
                                    aria-label="Quick-fill city"
                                    placeholder="Search city to fill coordinates\u{2026}"
                                    prop:value=move || city_query.get()
                                    on:input=move |ev| city_query.set(event_target_value(&ev))
                                />
                                {move || {
                                    let q = city_query.get();
                                    (!q.trim().is_empty()).then(|| {
                                        let matches = search_cities(&q, 6);
                                        view! {
                                            <div class="prayers-city-results" role="list" aria-label="City matches">
                                                {matches.into_iter().map(|c| {
                                                    view! {
                                                        <button
                                                            type="button"
                                                            class="prayers-city-opt"
                                                            on:click=move |_| {
                                                                city_query.set(String::new());
                                                                sync_location_drafts(&c.to_location());
                                                                save_location_if_configured(false);
                                                            }
                                                        >
                                                            <span>{format!("{}, {}", c.name, c.country)}</span>
                                                            <small>{c.tz}</small>
                                                        </button>
                                                    }
                                                }).collect_view()}
                                            </div>
                                        }
                                    })
                                }}
                            </div>
                        </SettingRow>

                        {move || {
                            let list = stored
                                .get()
                                .map(|s| s.value.locations)
                                .unwrap_or_else(|| draft_locations.get());
                            (!list.is_empty()).then(|| {
                                let active_label = stored.get().map(|s| s.value.location.label).unwrap_or_default();
                                view! {
                                    <SettingRow label="Saved locations" hint="Up to 24 locations" state=Signal::stored(None) stacked=true>
                                        <div class="prayers-location-fields">
                                            {list.into_iter().map(|entry| {
                                                let is_active = entry.location.label.eq_ignore_ascii_case(&active_label);
                                                let target = entry.location.clone();
                                                let rm_id = entry.id.clone();
                                                view! {
                                                    <div class="prayers-saved-loc-row">
                                                        <strong>{entry.location.label}</strong>
                                                        <small>{entry.location.timezone}</small>
                                                        {(!is_active).then(|| view! {
                                                            <button
                                                                type="button"
                                                                class="text-button"
                                                                on:click=move |_| {
                                                                    if let Some(mut next) = current_settings_or_draft() {
                                                                        next.location = target.clone();
                                                                        sync_location_drafts(&target);
                                                                        commit_settings.run((next, format!("Switched to {}", target.label), loc_state, true, false));
                                                                    }
                                                                }
                                                            >
                                                                "Use"
                                                            </button>
                                                        })}
                                                        {(!is_active).then(|| view! {
                                                            <button
                                                                type="button"
                                                                class="text-button"
                                                                aria-label="Remove location"
                                                                on:click=move |_| {
                                                                    if let Some(mut next) = current_settings_or_draft() {
                                                                        next.locations.retain(|l| l.id != rm_id);
                                                                        commit_settings.run((next, "Location removed".into(), loc_state, true, false));
                                                                    }
                                                                }
                                                            >
                                                                "Remove"
                                                            </button>
                                                        })}
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </SettingRow>
                                }
                            })
                        }}
                    </section>
                }.into_any(),

                "Calculation" => view! {
                    <section class="settings-group" aria-label="Calculation settings">
                        <h3>"Calculation"</h3>
                        <SettingRow label="Calculation method" hint="Fajr and Isha twilight angles" state=calc_state stacked=true>
                            <div class="prayer-methods" role="radiogroup" aria-label="Calculation method">
                                {METHODS.iter().map(|preset| {
                                    let pid = preset.id;
                                    let is_selected = move || {
                                        stored
                                            .get()
                                            .map(|s| s.value.calculation.method == pid)
                                            .unwrap_or_else(|| draft_calc.get().method == pid)
                                    };
                                    view! {
                                        <label class=move || if is_selected() { "is-checked" } else { "" }>
                                            <input
                                                type="radio"
                                                name="prayerMethod"
                                                prop:checked=is_selected
                                                on:change=move |_| {
                                                    if stored.get_untracked().is_none() {
                                                        let Some(loc) = validate_all_location() else {
                                                            section.set("Location");
                                                            return;
                                                        };
                                                        let mut calc = draft_calc.get_untracked();
                                                        calc.method = pid.to_owned();
                                                        let next = PrayerSettings {
                                                            locations: vec![SavedLocation {
                                                                id: slug_id(&loc.label),
                                                                tag: "Home".into(),
                                                                location: loc.clone(),
                                                            }],
                                                            location: loc,
                                                            calculation: calc,
                                                            display: draft_display.get_untracked(),
                                                        };
                                                        commit_settings.run((
                                                            next,
                                                            format!("Method set to {}", method_short_label(pid)),
                                                            calc_state,
                                                            false,
                                                            false,
                                                        ));
                                                    } else if let Some(mut next) = current_settings_or_draft() {
                                                        next.calculation.method = pid.to_owned();
                                                        commit_settings.run((
                                                            next,
                                                            format!("Method set to {}", method_short_label(pid)),
                                                            calc_state,
                                                            true,
                                                            true,
                                                        ));
                                                    }
                                                }
                                            />
                                            <span>{preset.name}</span>
                                            <small>{preset.summary}</small>
                                        </label>
                                    }
                                }).collect_view()}
                            </div>
                        </SettingRow>

                        {move || {
                            let is_custom = stored
                                .get()
                                .map(|s| s.value.calculation.method == "Custom")
                                .unwrap_or_else(|| draft_calc.get().method == "Custom");
                            is_custom.then(|| {
                                let cur_c = stored.get().map(|s| s.value.calculation).unwrap_or_else(|| draft_calc.get());
                                let fajr_a = RwSignal::new(format!("{}", cur_c.custom_fajr_angle.unwrap_or(18.0)));
                                let isha_a = RwSignal::new(format!("{}", cur_c.custom_isha_angle.unwrap_or(17.0)));
                                view! {
                                    <SettingRow label="Custom angles" hint="Degrees below horizon (0\u{2013}30)" state=calc_state stacked=true>
                                        <div class="prayer-coordinate-grid">
                                            <label>
                                                <span>"Fajr angle"</span>
                                                <input
                                                    type="text"
                                                    inputmode="decimal"
                                                    prop:value=move || fajr_a.get()
                                                    on:input=move |ev| fajr_a.set(event_target_value(&ev))
                                                    on:blur=move |_| {
                                                        if let Ok(a) = fajr_a.get_untracked().trim().parse::<f64>()
                                                            && (0.0..=30.0).contains(&a)
                                                            && let Some(mut next) = current_settings_or_draft()
                                                        {
                                                            next.calculation.custom_fajr_angle = Some(a);
                                                            commit_settings.run((next, "Custom Fajr angle saved".into(), calc_state, true, true));
                                                        }
                                                    }
                                                />
                                            </label>
                                            <label>
                                                <span>"Isha angle"</span>
                                                <input
                                                    type="text"
                                                    inputmode="decimal"
                                                    prop:value=move || isha_a.get()
                                                    on:input=move |ev| isha_a.set(event_target_value(&ev))
                                                    on:blur=move |_| {
                                                        if let Ok(a) = isha_a.get_untracked().trim().parse::<f64>()
                                                            && (0.0..=30.0).contains(&a)
                                                            && let Some(mut next) = current_settings_or_draft()
                                                        {
                                                            next.calculation.custom_isha_angle = Some(a);
                                                            next.calculation.custom_isha_minutes = None;
                                                            commit_settings.run((next, "Custom Isha angle saved".into(), calc_state, true, true));
                                                        }
                                                    }
                                                />
                                            </label>
                                        </div>
                                    </SettingRow>
                                }
                            })
                        }}

                        <SettingRow
                            label="Asr convention"
                            hint="Standard (1x shadow) or Hanafi (2x shadow)"
                            state=calc_state
                        >
                            <Segmented
                                label="Asr convention"
                                value=Signal::derive(move || {
                                    stored
                                        .get()
                                        .map(|s| s.value.calculation.asr_rule)
                                        .unwrap_or_else(|| draft_calc.get().asr_rule)
                                })
                                options=vec![("standard", "Standard"), ("hanafi", "Hanafi")]
                                on_change=Callback::new(move |next_asr: String| {
                                    draft_calc.update(|c| c.asr_rule = next_asr.clone());
                                    if let Some(mut next) = current_settings_or_draft() {
                                        next.calculation.asr_rule = next_asr.clone();
                                        let lbl = if next_asr == "hanafi" { "Hanafi" } else { "Standard" };
                                        commit_settings.run((next, format!("Asr set to {lbl}"), calc_state, true, true));
                                    }
                                })
                            />
                        </SettingRow>

                        <SettingRow
                            label="High-latitude rule"
                            hint="Used during long summer nights above 48\u{00b0}"
                            state=calc_state
                        >
                            <Select
                                aria_label="High-latitude rule"
                                options=vec![
                                    ("middle".to_owned(), "Middle of the night".to_owned()),
                                    ("seventh".to_owned(), "One-seventh of the night".to_owned()),
                                    ("angle".to_owned(), "Angle-based".to_owned()),
                                    ("none".to_owned(), "None".to_owned()),
                                ]
                                value=high_lat_val
                                on_change=Callback::new(move |next_rule: String| {
                                    draft_calc.update(|c| c.high_latitude_rule = next_rule.clone());
                                    if let Some(mut next) = current_settings_or_draft() {
                                        next.calculation.high_latitude_rule = next_rule;
                                        commit_settings.run((next, "High-latitude rule saved".into(), calc_state, true, true));
                                    }
                                })
                            />
                        </SettingRow>
                    </section>
                }.into_any(),

                "Offsets" => view! {
                    <section class="settings-group" aria-label="Minute adjustments">
                        <h3>"Adjustments"</h3>
                        {ALL_PRAYERS.into_iter().map(|p| {
                            let step = move |delta: i32| {
                                if let Some(mut next) = current_settings_or_draft() {
                                    let cur = next.calculation.offset_minutes.get(p);
                                    next.calculation.offset_minutes.set(p, (cur + delta).clamp(-120, 120));
                                    commit_settings.run((next, format!("{} offset updated", p.label()), offsets_state, true, true));
                                } else {
                                    draft_calc.update(|c| {
                                        let cur = c.offset_minutes.get(p);
                                        c.offset_minutes.set(p, (cur + delta).clamp(-120, 120));
                                    });
                                }
                            };
                            let val_str = move || {
                                stored
                                    .get()
                                    .map(|s| s.value.calculation.offset_minutes.get(p))
                                    .unwrap_or_else(|| draft_calc.get().offset_minutes.get(p))
                                    .to_string()
                            };
                            view! {
                                <SettingRow label=format!("{} adjustment", p.label()) hint="Range \u{2212}120 to +120 minutes" state=offsets_state>
                                    <div class="prayer-stepper">
                                        <button type="button" class="icon-button" aria-label=format!("Decrease {} offset", p.label()) on:click=move |_| step(-1)>
                                            "\u{2212}"
                                        </button>
                                        <input
                                            type="text"
                                            inputmode="numeric"
                                            aria-label=format!("{} offset minutes", p.label())
                                            prop:value=val_str
                                            on:change=move |ev| {
                                                if let Ok(v) = validate_offset(&event_target_value(&ev))
                                                    && let Some(mut next) = current_settings_or_draft()
                                                {
                                                    next.calculation.offset_minutes.set(p, v);
                                                    commit_settings.run((next, format!("{} offset updated", p.label()), offsets_state, true, true));
                                                }
                                            }
                                        />
                                        <button type="button" class="icon-button" aria-label=format!("Increase {} offset", p.label()) on:click=move |_| step(1)>
                                            "+"
                                        </button>
                                        <small>"min"</small>
                                    </div>
                                </SettingRow>
                            }
                        }).collect_view()}
                        <button
                            type="button"
                            class="secondary-button"
                            on:click=move |_| {
                                if let Some(mut next) = current_settings_or_draft() {
                                    next.calculation.offset_minutes = OffsetMinutes::default();
                                    commit_settings.run((next, "All offsets reset".into(), offsets_state, true, true));
                                }
                            }
                        >
                            "Reset all offsets"
                        </button>
                    </section>
                }.into_any(),

                _ => view! {
                    <section class="settings-group" aria-label="Display options">
                        <h3>"Display"</h3>
                        <SettingRow label="Clock format" hint="Shown across all timetables" state=display_state>
                            <Segmented
                                label="Clock format"
                                value=Signal::derive(move || {
                                    stored
                                        .get()
                                        .map(|s| s.value.display.time_format)
                                        .unwrap_or_else(|| draft_display.get().time_format)
                                })
                                options=vec![("12h", "12-hour"), ("24h", "24-hour")]
                                on_change=Callback::new(move |fmt: String| {
                                    draft_display.update(|d| d.time_format = fmt.clone());
                                    if let Some(mut next) = current_settings_or_draft() {
                                        next.display.time_format = fmt;
                                        commit_settings.run((next, "Clock format saved".into(), display_state, true, false));
                                    }
                                })
                            />
                        </SettingRow>
                        <SettingSwitch
                            label="Daily prayer tracking"
                            hint="Show on-time / late / unprayed status icons"
                            state=display_state
                            checked=Signal::derive(move || stored.get().map_or_else(|| draft_display.get().show_tracking, |s| s.value.display.show_tracking))
                            on_change=Callback::new(move |v: bool| {
                                draft_display.update(|d| d.show_tracking = v);
                                if let Some(mut next) = current_settings_or_draft() {
                                    next.display.show_tracking = v;
                                    commit_settings.run((next, "Tracking visibility updated".into(), display_state, true, false));
                                }
                            })
                        />
                        <SettingSwitch
                            label="Show Sunrise"
                            hint="Include Sunrise marker in Today and Week tables"
                            state=display_state
                            checked=Signal::derive(move || stored.get().map_or_else(|| draft_display.get().show_sunrise, |s| s.value.display.show_sunrise))
                            on_change=Callback::new(move |v: bool| {
                                draft_display.update(|d| d.show_sunrise = v);
                                if let Some(mut next) = current_settings_or_draft() {
                                    next.display.show_sunrise = v;
                                    commit_settings.run((next, "Sunrise setting saved".into(), display_state, true, false));
                                }
                            })
                        />
                        <SettingSwitch
                            label="Show Hijri calendar"
                            hint="Display Islamic dates alongside Gregorian dates"
                            state=display_state
                            checked=Signal::derive(move || stored.get().map_or_else(|| draft_display.get().show_hijri, |s| s.value.display.show_hijri))
                            on_change=Callback::new(move |v: bool| {
                                draft_display.update(|d| d.show_hijri = v);
                                if let Some(mut next) = current_settings_or_draft() {
                                    next.display.show_hijri = v;
                                    commit_settings.run((next, "Hijri calendar setting saved".into(), display_state, true, false));
                                }
                            })
                        />
                        <SettingSwitch
                            label="Show Sunna & night times"
                            hint="Show Islamic midnight and last third of night"
                            state=display_state
                            checked=Signal::derive(move || stored.get().map_or_else(|| draft_display.get().show_sunna, |s| s.value.display.show_sunna))
                            on_change=Callback::new(move |v: bool| {
                                draft_display.update(|d| d.show_sunna = v);
                                if let Some(mut next) = current_settings_or_draft() {
                                    next.display.show_sunna = v;
                                    commit_settings.run((next, "Sunna display saved".into(), display_state, true, false));
                                }
                            })
                        />
                        <SettingSwitch
                            label="Show live countdown"
                            hint="Display countdown bar on Today and Week views"
                            state=display_state
                            checked=Signal::derive(move || stored.get().map_or_else(|| draft_display.get().show_countdown, |s| s.value.display.show_countdown))
                            on_change=Callback::new(move |v: bool| {
                                draft_display.update(|d| d.show_countdown = v);
                                if let Some(mut next) = current_settings_or_draft() {
                                    next.display.show_countdown = v;
                                    commit_settings.run((next, "Countdown setting saved".into(), display_state, true, false));
                                }
                            })
                        />
                    </section>
                }.into_any(),
            }}
        </SettingsSheet>
    }
}
