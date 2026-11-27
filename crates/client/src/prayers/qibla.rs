//! Qibla compass view with live solar azimuth alignment cue and saved location comparison.

use leptos::prelude::*;

use crate::icons::Icon;
use crate::prayers::calc::{
    format_coord_lat, format_coord_lon, qibla_bearing_and_distance, sun_azimuth_altitude,
};
use crate::prayers::model::{Location, PrayerSettings, SavedLocation};

#[component]
pub fn QiblaMain(
    settings: Signal<PrayerSettings>,
    now_ms: Signal<f64>,
) -> impl IntoView {
    view! {
        {move || {
            let s = settings.get();
            let now = now_ms.get();
            let loc = &s.location;
            let (bearing, km, wind) = qibla_bearing_and_distance(loc.latitude, loc.longitude);
            let (sun_az, sun_alt) = sun_azimuth_altitude(now, loc.latitude, loc.longitude);
            let sun_visible = sun_alt > -6.0;

            let sun_rad = (sun_az - 90.0).to_radians();
            let sx = 115.0 + 92.0 * sun_rad.cos();
            let sy = 115.0 + 92.0 * sun_rad.sin();

            let signed_diff = ((bearing - sun_az + 540.0) % 360.0) - 180.0;
            let cue_text = if sun_visible {
                if signed_diff.abs() < 4.0 {
                    format!(
                        "The sun is nearly aligned with the Qibla right now (azimuth {:.0}\u{00b0}). Face the sun to face Makkah.",
                        sun_az
                    )
                } else if signed_diff > 0.0 {
                    format!(
                        "Sun azimuth is {:.0}\u{00b0} (altitude {:.0}\u{00b0}). Look toward the sun and turn {:.0}\u{00b0} clockwise (right) to face the Qibla.",
                        sun_az,
                        sun_alt,
                        signed_diff.abs()
                    )
                } else {
                    format!(
                        "Sun azimuth is {:.0}\u{00b0} (altitude {:.0}\u{00b0}). Look toward the sun and turn {:.0}\u{00b0} counterclockwise (left) to face the Qibla.",
                        sun_az,
                        sun_alt,
                        signed_diff.abs()
                    )
                }
            } else {
                format!(
                    "Sun is below the horizon (altitude {:.0}\u{00b0}). Face {:.0}\u{00b0} {wind} from true North.",
                    sun_alt, bearing
                )
            };

            let sun_stat = if sun_visible {
                format!("{:.0}\u{00b0} az \u{00b7} {:.0}\u{00b0} alt", sun_az, sun_alt)
            } else {
                "Below horizon".to_owned()
            };

            view! {
                <header class="prayers-sheet-head">
                    <div class="prayers-head-title">
                        <h1>{format!("Qibla from {}", loc.label)}</h1>
                        <div class="prayers-head-sub">
                            {format!(
                                "{}, {} \u{00b7} Great-circle bearing to al-Masjid al-Haram",
                                format_coord_lat(loc.latitude),
                                format_coord_lon(loc.longitude)
                            )}
                        </div>
                    </div>
                </header>

                <div class="prayers-sheet-body">
                    <div class="prayers-qibla-stage">
                        <svg class="prayers-qibla-big-dial" viewBox="0 0 230 230" role="img" aria-label=format!("Qibla compass pointing {:.1} degrees {}", bearing, wind)>
                            <circle cx="115" cy="115" r="102" class="prayers-qibla-ring"/>
                            <circle cx="115" cy="115" r="74" class="prayers-qibla-inner"/>
                            <circle cx="115" cy="115" r="24" class="prayers-qibla-inner-solid"/>
                            {(0..36).map(|step| {
                                let deg = (step * 10) as f64;
                                let is_major = step % 3 == 0;
                                let rad = (deg - 90.0).to_radians();
                                let r_out = 102.0;
                                let r_in = if is_major { 92.0 } else { 97.0 };
                                view! {
                                    <line
                                        class=if is_major { "prayers-qibla-tick is-major" } else { "prayers-qibla-tick" }
                                        x1=format!("{:.1}", 115.0 + r_in * rad.cos())
                                        y1=format!("{:.1}", 115.0 + r_in * rad.sin())
                                        x2=format!("{:.1}", 115.0 + r_out * rad.cos())
                                        y2=format!("{:.1}", 115.0 + r_out * rad.sin())
                                    />
                                }
                            }).collect_view()}

                            <text x="115" y="25" text-anchor="middle" class="prayers-qibla-card-n">"N"</text>
                            <text x="206" y="119" text-anchor="middle" class="prayers-qibla-card">"E"</text>
                            <text x="115" y="213" text-anchor="middle" class="prayers-qibla-card">"S"</text>
                            <text x="24" y="119" text-anchor="middle" class="prayers-qibla-card">"W"</text>

                            {sun_visible.then(|| view! {
                                <g>
                                    <line
                                        class="prayers-qibla-sun-ray"
                                        x1="115"
                                        y1="115"
                                        x2=format!("{sx:.1}")
                                        y2=format!("{sy:.1}")
                                    />
                                    <circle class="prayers-qibla-sun-dot" cx=format!("{sx:.1}") cy=format!("{sy:.1}") r="5"/>
                                </g>
                            })}

                            <g transform=format!("rotate({bearing:.1} 115 115)")>
                                <line x1="115" y1="115" x2="115" y2="33" class="prayers-qibla-needle"/>
                                <polygon points="115,24 107,40 123,40" class="prayers-qibla-arrow"/>
                                <rect x="108" y="42" width="14" height="14" rx="2.5" class="prayers-qibla-kaaba"/>
                            </g>
                            <circle cx="115" cy="115" r="5" class="prayers-qibla-hub"/>
                        </svg>

                        <div class="prayers-qibla-readout">
                            <div class="prayers-q-stat">
                                <small>"Qibla bearing"</small>
                                <strong class="prayers-num">{format!("{bearing:.1}\u{00b0} {wind}")}</strong>
                            </div>
                            <div class="prayers-q-stat">
                                <small>"Distance to Kaaba"</small>
                                <strong class="prayers-num">{format!("{km:.0} km")}</strong>
                            </div>
                            <div class="prayers-q-stat">
                                <small>"Sun azimuth"</small>
                                <strong class="prayers-num">{sun_stat}</strong>
                            </div>
                        </div>

                        <div class="prayers-sun-cue">
                            <Icon name="sun"/>
                            <span>{cue_text}</span>
                        </div>
                    </div>
                </div>
            }
        }}
    }
}

#[component]
pub fn QiblaRightSheet(
    settings: Signal<PrayerSettings>,
    on_switch_location: Callback<Location>,
) -> impl IntoView {
    view! {
        {move || {
            let s = settings.get();
            let mut list = s.locations.clone();
            if list.is_empty() {
                list.push(SavedLocation {
                    id: "active".into(),
                    tag: String::new(),
                    location: s.location.clone(),
                });
            }
            let active = &s.location;
            view! {
                <header class="prayers-sheet-head">
                    <div class="prayers-head-title">
                        <h2>"Saved bearings"</h2>
                        <div class="prayers-head-sub">"True North bearings"</div>
                    </div>
                </header>

                <div class="prayers-right-body">
                    <section class="prayers-ctx-card" aria-label="Locations comparison">
                        <div class="prayers-ctx-head"><h3>"Compare locations"</h3></div>
                        <div class="prayers-kv-list">
                            {list.into_iter().map(|item| {
                                let (deg, dist, wind) = qibla_bearing_and_distance(item.location.latitude, item.location.longitude);
                                let is_cur = item.location.label.eq_ignore_ascii_case(&active.label)
                                    && (item.location.latitude - active.latitude).abs() < 0.01;
                                let target = item.location.clone();
                                view! {
                                    <button
                                        type="button"
                                        class="prayers-kv-row is-btn"
                                        on:click=move |_| { on_switch_location.try_run(target.clone()); }
                                    >
                                        <span class=if is_cur { "prayers-kv-k is-active" } else { "prayers-kv-k" }>
                                            {item.location.label}
                                        </span>
                                        <span class="prayers-kv-v prayers-num">
                                            {format!("{deg:.1}\u{00b0} {wind} \u{00b7} {dist:.0} km")}
                                        </span>
                                    </button>
                                }
                            }).collect_view()}
                        </div>
                    </section>

                    <section class="prayers-ctx-card" aria-label="Reference coordinates">
                        <div class="prayers-ctx-head"><h3>"Reference"</h3></div>
                        <div class="prayers-kv-list">
                            <div class="prayers-kv-row">
                                <span class="prayers-kv-k">"Active"</span>
                                <span class="prayers-kv-v prayers-num">{format!("{}, {}", format_coord_lat(active.latitude), format_coord_lon(active.longitude))}</span>
                            </div>
                            <div class="prayers-kv-row">
                                <span class="prayers-kv-k">"Kaaba"</span>
                                <span class="prayers-kv-v prayers-num">"21.4225\u{00b0} N, 39.8262\u{00b0} E"</span>
                            </div>
                            <div class="prayers-kv-row">
                                <span class="prayers-kv-k">"Timezone"</span>
                                <span class="prayers-kv-v">{active.timezone.clone()}</span>
                            </div>
                        </div>
                    </section>
                </div>
            }
        }}
    }
}
