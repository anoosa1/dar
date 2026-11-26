//! Custom Twilight waveform voice-note player backed by a hidden `<audio>` element.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use super::super::clock;
use crate::icons::Icon;

/// Deterministic 18-bar waveform heights (`vn-h1` .. `vn-h8`).
const WAVEFORM_BARS: [u8; 18] = [3, 5, 8, 4, 6, 3, 7, 5, 8, 4, 6, 3, 5, 4, 7, 3, 6, 4];

fn speed_label(rate: f64) -> &'static str {
    if (rate - 1.5).abs() < 0.01 {
        "1.5\u{00d7}"
    } else if (rate - 2.0).abs() < 0.01 {
        "2\u{00d7}"
    } else {
        "1\u{00d7}"
    }
}

/// Custom voice-note player with deterministic waveform scrub bars and speed toggle.
#[component]
pub fn VoiceNotePlayer(
    url: String,
    seconds: u32,
    label: &'static str,
    on_error: Callback<()>,
) -> impl IntoView {
    let audio_ref = NodeRef::<html::Audio>::new();
    let playing = RwSignal::new(false);
    let current = RwSignal::new(0.0f64);
    let duration = RwSignal::new(seconds as f64);
    let rate = RwSignal::new(1.0f64);

    on_cleanup(move || {
        if let Some(el) = audio_ref.try_get_untracked().flatten() {
            let media: &web_sys::HtmlMediaElement = el.unchecked_ref();
            let _ = media.pause();
        }
    });

    let progress = Memo::new(move |_| {
        let d = duration.get();
        if d > 0.0 {
            (current.get() / d).clamp(0.0, 1.0)
        } else {
            0.0
        }
    });

    let toggle_play = move |_| {
        let Some(el) = audio_ref.get_untracked() else {
            return;
        };
        let media: &web_sys::HtmlMediaElement = el.unchecked_ref();
        if playing.get_untracked() {
            let _ = media.pause();
            playing.set(false);
        } else {
            media.set_playback_rate(rate.get_untracked());
            match media.play() {
                Ok(promise) => {
                    playing.set(true);
                    spawn_local(async move {
                        if wasm_bindgen_futures::JsFuture::from(promise).await.is_err() {
                            playing.try_set(false);
                        }
                    });
                }
                Err(_) => {
                    playing.set(false);
                }
            }
        }
    };

    let cycle_speed = move |_| {
        let next = match rate.get_untracked() {
            r if r < 1.25 => 1.5,
            r if r < 1.75 => 2.0,
            _ => 1.0,
        };
        rate.set(next);
        if let Some(el) = audio_ref.get_untracked() {
            let media: &web_sys::HtmlMediaElement = el.unchecked_ref();
            media.set_playback_rate(next);
        }
    };

    view! {
        <div class="chat-voice-note" role="group" aria-label=label>
            <audio
                node_ref=audio_ref
                preload="none"
                src=url
                hidden=true
                on:loadedmetadata=move |_| {
                    if let Some(el) = audio_ref.try_get_untracked().flatten() {
                        let media: &web_sys::HtmlMediaElement = el.unchecked_ref();
                        let d = media.duration();
                        if d.is_finite() && d > 0.0 {
                            duration.try_set(d);
                        }
                        media.set_playback_rate(rate.try_get_untracked().unwrap_or(1.0));
                    }
                }
                on:timeupdate=move |_| {
                    if let Some(el) = audio_ref.try_get_untracked().flatten() {
                        let media: &web_sys::HtmlMediaElement = el.unchecked_ref();
                        current.try_set(media.current_time());
                    }
                }
                on:ended=move |_| {
                    playing.try_set(false);
                    current.try_set(0.0);
                }
                on:error=move |_| {
                    playing.try_set(false);
                    on_error.try_run(());
                }
            ></audio>
            <button
                type="button"
                class="vn-play"
                aria-label=move || if playing.get() { "Pause" } else { "Play" }
                on:click=toggle_play
            >
                <Icon name=Signal::derive(move || if playing.get() { "pause" } else { "play" })/>
            </button>
            <div class="vn-bars">
                {WAVEFORM_BARS.into_iter().enumerate().map(|(idx, height)| {
                    let ratio = idx as f64 / WAVEFORM_BARS.len() as f64;
                    let pct = (idx * 100) / WAVEFORM_BARS.len();
                    view! {
                        <button
                            type="button"
                            class=move || {
                                if progress.get() > ratio {
                                    format!("vn-bar vn-h{height} is-played")
                                } else {
                                    format!("vn-bar vn-h{height}")
                                }
                            }
                            aria-label=format!("Seek to {pct}%")
                            on:click=move |_| {
                                if let Some(el) = audio_ref.get_untracked() {
                                    let media: &web_sys::HtmlMediaElement = el.unchecked_ref();
                                    let media_dur = media.duration();
                                    let dur = if media_dur.is_finite() && media_dur > 0.0 {
                                        media_dur
                                    } else {
                                        duration.get_untracked()
                                    };
                                    if dur > 0.0 {
                                        let target = dur * ratio;
                                        media.set_current_time(target);
                                        current.set(target);
                                    }
                                }
                            }
                        ></button>
                    }
                }).collect_view()}
            </div>
            <span class="vn-time">
                {move || {
                    if playing.get() {
                        clock(current.get().round().max(0.0) as u32)
                    } else {
                        let total = if seconds > 0 {
                            seconds
                        } else {
                            duration.get().round().max(0.0) as u32
                        };
                        clock(total)
                    }
                }}
            </span>
            <button
                type="button"
                class="vn-speed"
                aria-label="Playback speed"
                on:click=cycle_speed
            >
                {move || speed_label(rate.get())}
            </button>
        </div>
    }
}
