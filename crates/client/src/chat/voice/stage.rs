//! Voice channel stage (pre-join preview and active call tiles) and E2EE call-code verification dialog.

use std::collections::BTreeSet;
use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::super::model::{ChatVoiceCall, ChatVoiceMember};
use super::{
    Controls, Marks, MicState, VoiceHub, format_duration, state_label, voice_initials,
    voice_supported,
};
use crate::icons::Icon;
use crate::pim::avatar_hue;
use crate::ui::Dialog;

/// A voice channel opened from the tree: who is in it, and joining or the call's controls (`VoiceStage`).
#[component]
pub fn VoiceStage(
    thread_name: String,
    thread_jid: String,
    server: Signal<String>,
    members: Signal<Vec<ChatVoiceMember>>,
    call: Signal<Option<ChatVoiceCall>>,
    in_other: Signal<String>,
    speaking: Signal<BTreeSet<String>>,
    me: Signal<String>,
    mic: Signal<MicState>,
    here: Signal<bool>,
    on_join: Callback<(bool, Callback<Result<(), String>>)>,
    on_set: Callback<(bool, bool)>,
    on_leave: Callback<()>,
    on_code: Callback<()>,
    on_use_here: Callback<()>,
    on_close: Callback<()>,
) -> impl IntoView {
    let join_muted = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let supported = voice_supported();
    let thread_name = StoredValue::new(thread_name);
    let thread_jid = StoredValue::new(thread_jid);

    let in_this_call = Memo::new(move |_| {
        let jid = thread_jid.get_value();
        call.get().filter(|c| c.channel_id == jid)
    });

    let connected_since_secs = RwSignal::new(0u32);
    let voice_hub = use_context::<VoiceHub>();
    let call_connected_key = Memo::new(move |_| {
        in_this_call
            .get()
            .and_then(|c| (c.state == "connected").then_some((c.server_id, c.channel_id)))
    });
    Effect::new(move |_| {
        let Some((server_id, channel_id)) = call_connected_key.get() else {
            let _ = connected_since_secs.try_set(0);
            return;
        };
        let started_ms = voice_hub
            .and_then(|hub| hub.call_started_ms(&server_id, &channel_id))
            .unwrap_or_else(js_sys::Date::now);
        let initial = ((js_sys::Date::now() - started_ms).max(0.0) / 1000.0) as u32;
        let _ = connected_since_secs.try_set(initial);
        if let Ok(handle) = set_interval_with_handle(
            move || {
                let secs = ((js_sys::Date::now() - started_ms).max(0.0) / 1000.0) as u32;
                let _ = connected_since_secs.try_set(secs);
            },
            Duration::from_secs(1),
        ) {
            on_cleanup(move || handle.clear());
        }
    });

    let do_join = move |_| {
        busy.set(true);
        error.set(String::new());
        on_join.try_run((
            join_muted.get_untracked(),
            Callback::new(move |res: Result<(), String>| {
                let _ = busy.try_set(false);
                if let Err(msg) = res {
                    let _ = error.try_set(msg);
                }
            }),
        ));
    };

    view! {
        <section class="chat-voice-stage" aria-label=format!("Voice channel {}", thread_name.get_value())>
            <header class="chat-conversation-head">
                <button type="button" class="icon-button chat-back" aria-label="Back to chats" on:click=move |_| { on_close.try_run(()); }><Icon name="back"/></button>
                <span class=move || if in_this_call.get().is_some() { "chat-channel-mark is-live" } else { "chat-channel-mark" } aria-hidden="true"><Icon name="speaker"/></span>
                <div class="chat-who">
                    <h2 tabindex="-1">{thread_name.get_value()}</h2>
                    <small>{move || {
                        let count = members.with(Vec::len);
                        let srv = server.get();
                        if count > 0 { format!("{srv} \u{00b7} {count} in voice") } else { format!("{srv} \u{00b7} nobody in voice") }
                    }}</small>
                </div>
                {move || in_this_call.get().filter(|c| c.e2ee).map(|_| view! {
                    <button type="button" class="chat-voice-e2ee" on:click=move |_| { on_code.try_run(()); }><Icon name="lock"/><span>"End-to-end encrypted"</span></button>
                })}
                <button type="button" class="icon-button chat-close" aria-label="Close voice channel" on:click=move |_| { on_close.try_run(()); }><Icon name="close"/></button>
            </header>
            {move || match in_this_call.get() {
                Some(active_call) => {
                    let is_here = here.get();
                    let cur_mic = mic.get();
                    let cur_me = me.get();
                    let spk = speaking.get();
                    let list = members.get();
                    let empty = list.is_empty();
                    let is_connected = active_call.state == "connected";
                    let show_e2ee = active_call.e2ee;
                    let status_class = format!("chat-voice-status is-{}", active_call.state);
                    let status_text = state_label(&active_call);
                    view! {
                        <div class="chat-voice-grid" role="list" aria-label="In the call">
                            {list.into_iter().map(|m| {
                                let talking = spk.contains(&m.id);
                                let is_me = m.id == cur_me;
                                let hue = avatar_hue(&m.id);
                                let init = voice_initials(&m.name);
                                let name = m.name.clone();
                                let badge_icon = if m.deafened {
                                    Some("headphonesOff")
                                } else if m.muted {
                                    Some("micOff")
                                } else {
                                    None
                                };
                                view! {
                                    <div role="listitem" class=if talking { "chat-voice-tile is-speaking" } else { "chat-voice-tile" }>
                                        {if talking {
                                            view! { <div class="chat-voice-tile-eq" aria-hidden="true"><i></i><i></i><i></i></div> }.into_any()
                                        } else if let Some(icon) = badge_icon {
                                            view! { <span class="chat-voice-tile-badge" aria-hidden="true"><Icon name=icon/></span> }.into_any()
                                        } else {
                                            ().into_any()
                                        }}
                                        <span class=format!("chat-voice-tile-avatar contact-initials {hue}") aria-hidden="true">{init}</span>
                                        <span class="chat-voice-tile-name">{name}{if is_me { " (you)" } else { "" }}<Marks member=m/>{talking.then(|| view! { <span class="sr-only">", speaking"</span> })}</span>
                                    </div>
                                }
                            }).collect_view()}
                            {empty.then(|| view! { <p class="chat-hint">"Waiting for Discord\u{2026}"</p> })}
                        </div>
                        {(!is_here).then(|| view! {
                            <div class="chat-banner" role="status">
                                <Icon name="info"/>
                                <span><b>"The call\u{2019}s audio is not playing here"</b>"It is in another tab, or this page was reloaded."</span>
                                <button type="button" class="secondary-button" on:click=move |_| { on_use_here.try_run(()); }>"Use this tab"</button>
                            </div>
                        })}
                        {(is_here && cur_mic == MicState::Blocked).then(|| view! {
                            <div class="chat-banner is-error" role="alert">
                                <Icon name="micOff"/>
                                <span><b>"Microphone blocked"</b>"Allow the microphone for this site in the browser, then unmute. You can still listen."</span>
                            </div>
                        })}
                        <div class="chat-voice-bar">
                            <span class=status_class>
                                <Icon name="signal"/>
                                <b>{move || if is_connected {
                                    format!("{status_text} \u{00b7} {}", format_duration(connected_since_secs.get()))
                                } else {
                                    status_text.to_owned()
                                }}</b>
                            </span>
                            <Controls call=active_call mic=if is_here { cur_mic } else { MicState::On } big=true on_set=on_set on_leave=on_leave/>
                            {show_e2ee.then(|| view! {
                                <button type="button" class="chat-voice-button chat-voice-verify" on:click=move |_| { on_code.try_run(()); }>
                                    <Icon name="lock"/>
                                    <span>"Verify code"</span>
                                </button>
                            })}
                        </div>
                    }
                    .into_any()
                }
                None => {
                    let list = members.get();
                    let count = list.len();
                    let spk = speaking.get();
                    let name = thread_name.get_value();
                    let heading = if count > 0 {
                        if count == 1 { format!("1 person is in {name}") } else { format!("{count} people are in {name}") }
                    } else {
                        format!("{name} is empty")
                    };
                    let other = in_other.get();
                    view! {
                        <div class="chat-voice-join">
                            <span class="chat-empty-icon"><Icon name="speaker"/></span>
                            <h3>{heading}</h3>
                            {(count > 0).then(|| view! {
                                <ul class="chat-voice-preview" aria-label="In voice">
                                    {list.into_iter().take(8).map(|m| {
                                        let hue = avatar_hue(&m.id);
                                        let init = voice_initials(&m.name);
                                        let talking = spk.contains(&m.id);
                                        let status = if m.deafened {
                                            Some("deafened")
                                        } else if m.muted {
                                            Some("muted")
                                        } else if talking {
                                            Some("speaking")
                                        } else {
                                            None
                                        };
                                        view! {
                                            <li class=if talking { "chat-voice-preview-chip is-speaking" } else { "chat-voice-preview-chip" }>
                                                <span class=format!("chat-voice-avatar contact-initials {hue}") aria-hidden="true">{init}</span>
                                                <span class="chat-voice-preview-name">{m.name}</span>
                                                {status.map(|s| view! { <span class="chat-voice-preview-status">{" \u{00b7} "}{s}</span> })}
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            })}
                            {if supported {
                                view! {
                                    <p>{if !other.is_empty() { format!("Joining leaves {other}.") } else { "Join to talk. Your browser asks for the microphone the first time.".into() }}</p>
                                    <label class="chat-voice-option">
                                        <input type="checkbox" prop:checked=move || join_muted.get() on:change=move |e| join_muted.set(event_target_checked(&e))/>
                                        "Join muted"
                                    </label>
                                    <button type="button" class="primary-button chat-voice-join-button" disabled=move || busy.get() on:click=do_join>
                                        <Icon name="headphones"/>
                                        {move || if busy.get() { "Joining\u{2026}" } else { "Join voice" }}
                                    </button>
                                }
                                .into_any()
                            } else {
                                view! { <p>"This browser cannot take part in voice calls. Use a current version of Chrome, Edge, Firefox or Safari."</p> }.into_any()
                            }}
                            {move || (!error.get().is_empty()).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                        </div>
                    }
                    .into_any()
                }
            }}
        </section>
    }
}

/// The 30-digit code everyone in an end-to-end encrypted call sees (`CallCodeDialog`).
#[component]
pub fn CallCodeDialog(hub: VoiceHub, account_id: String, on_close: Callback<()>) -> impl IntoView {
    let code = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    spawn_local(async move {
        match hub.code(account_id).await {
            Ok(c) => {
                let _ = code.try_set(c);
            }
            Err(e) => {
                let _ = error.try_set(e);
            }
        }
    });
    view! {
        <Dialog title="Verify the call is private" on_close=on_close>
            <p>"Everyone in the call sees the same code. Compare it with someone over another channel; if it matches, nobody else can listen."</p>
            {move || {
                let c = code.get();
                if !c.is_empty() {
                    let digits: Vec<char> = c.chars().filter(|ch| !ch.is_whitespace()).collect();
                    let groups: Vec<String> = digits.chunks(5).map(|ch| ch.iter().collect()).collect();
                    let aria = format!("Call code {}", groups.join(" "));
                    view! {
                        <p class="chat-call-code" aria-label=aria>
                            {groups.into_iter().map(|g| view! { <span>{g}</span> }).collect_view()}
                        </p>
                    }
                    .into_any()
                } else {
                    let err = error.get();
                    if !err.is_empty() {
                        view! { <p class="error" role="alert">{err}</p> }.into_any()
                    } else {
                        view! { <p role="status">"Loading the code\u{2026}"</p> }.into_any()
                    }
                }
            }}
            <div class="dialog-actions">
                <button type="button" class="secondary-button" on:click=move |_| { on_close.try_run(()); }>"Close"</button>
            </div>
        </Dialog>
    }
}
