//! First-run Prayer Times setup: offline world-city picker, browser geolocation,
//! and manual setup launch in Dar's settings sheet.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::icons::Icon;
use crate::prayers::cities::{City, QUICK_CITIES, find_city, search_cities};
use crate::prayers::model::{
    CalculationSettings, DisplaySettings, PrayerSettings, SavedLocation, ensure_location_listed,
};
use crate::prayers::tz;

#[component]
pub fn PrayersSetup(
    on_pick_settings: Callback<PrayerSettings>,
    on_open_settings: Callback<()>,
) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let geo_error = RwSignal::new(String::new());
    let locating = RwSignal::new(false);

    let pick_city = move |c: &'static City| {
        let mut settings = PrayerSettings {
            location: c.to_location(),
            locations: vec![SavedLocation {
                id: crate::prayers::model::slug_id(c.name),
                tag: "Home".into(),
                location: c.to_location(),
            }],
            calculation: CalculationSettings {
                method: c.recommended_method.to_owned(),
                ..Default::default()
            },
            display: DisplaySettings::default(),
        };
        ensure_location_listed(&mut settings);
        on_pick_settings.try_run(settings);
    };

    let on_locate = move |_| {
        geo_error.set(String::new());
        locating.set(true);
        spawn_local(async move {
            match tz::request_geolocation().await {
                Ok((lat, lon)) => {
                    let mut settings = PrayerSettings {
                        location: crate::prayers::model::Location {
                            label: "My location".into(),
                            latitude: (lat * 10000.0).round() / 10000.0,
                            longitude: (lon * 10000.0).round() / 10000.0,
                            timezone: tz::browser_timezone(),
                        },
                        locations: Vec::new(),
                        calculation: CalculationSettings::default(),
                        display: DisplaySettings::default(),
                    };
                    ensure_location_listed(&mut settings);
                    let _ = locating.try_set(false);
                    let _ = on_pick_settings.try_run(settings);
                }
                Err(msg) => {
                    let _ = geo_error.try_set(msg);
                    let _ = locating.try_set(false);
                }
            }
        });
    };

    view! {
        <header class="prayers-sheet-head">
            <div class="prayers-head-title">
                <h1>"Prayer Times"</h1>
                <div class="prayers-head-sub">"Solar prayer timetable, daily prayer log, and Qibla compass"</div>
            </div>
        </header>

        <div class="prayers-sheet-body">
            <section class="prayers-setup-card" aria-label="First-run Prayer Times setup">
                <h2>"Choose your city to start"</h2>
                <p>
                    "Times are calculated locally in your browser from astronomical coordinates with zero external lookups."
                </p>

                <label>
                    <span class="sr-only">"Search world cities"</span>
                    <input
                        type="search"
                        aria-label="Search world cities"
                        placeholder="Search city (e.g. Toronto, London, Makkah)\u{2026}"
                        prop:value=move || query.get()
                        on:input=move |ev| query.set(event_target_value(&ev))
                    />
                </label>

                {move || {
                    let q = query.get();
                    (!q.trim().is_empty()).then(|| {
                        let matches = search_cities(&q, 8);
                        if matches.is_empty() {
                            view! {
                                <p class="prayers-inline-status">"No city match found. Use Set up Prayer Times to enter coordinates manually."</p>
                            }.into_any()
                        } else {
                            view! {
                                <div class="prayers-city-results" role="list" aria-label="Matching cities">
                                    {matches.into_iter().map(|c| {
                                        view! {
                                            <button
                                                type="button"
                                                class="prayers-city-opt"
                                                on:click=move |_| pick_city(c)
                                            >
                                                <span>{format!("{}, {}", c.name, c.country)}</span>
                                                <small>{format!("{} \u{00b7} {}", c.tz, c.recommended_method)}</small>
                                            </button>
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_any()
                        }
                    })
                }}

                <div class="prayers-city-chips" role="group" aria-label="Quick city presets">
                    {QUICK_CITIES.iter().filter_map(|name| find_city(name)).map(|c| {
                        view! {
                            <button
                                type="button"
                                class="prayers-city-chip"
                                on:click=move |_| pick_city(c)
                            >
                                {c.name}
                            </button>
                        }
                    }).collect_view()}
                </div>

                {move || {
                    let msg = geo_error.get();
                    (!msg.is_empty()).then(|| view! {
                        <p class="prayers-inline-status" role="status">{msg}</p>
                    })
                }}

                <div class="prayers-setup-actions">
                    <button
                        type="button"
                        class="primary-button"
                        on:click=move |_| { on_open_settings.try_run(()); }
                    >
                        "Set up Prayer Times"
                    </button>
                    <button
                        type="button"
                        class="secondary-button prayer-locate"
                        disabled=move || locating.get()
                        on:click=on_locate
                    >
                        <Icon name="location"/>
                        {move || if locating.get() { "Locating\u{2026}" } else { "Use my location" }}
                    </button>
                </div>
            </section>
        </div>
    }
}
