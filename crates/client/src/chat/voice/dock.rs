//! Sidebar voice member list, voice controls and persistent sidebar/phone call dock.

use std::collections::BTreeSet;

use leptos::prelude::*;

use super::super::model::{ChatVoiceCall, ChatVoiceMember};
use super::{MicState, state_label, voice_initials};
use crate::icons::Icon;
use crate::pim::avatar_hue;

#[component]
pub fn Marks(member: ChatVoiceMember) -> impl IntoView {
    if !member.muted && !member.deafened {
        return ().into_any();
    }
    view! {
        <span class="chat-voice-marks">
            {member.muted.then(|| view! { <span title="Muted"><Icon name="micOff"/><span class="sr-only">", muted"</span></span> })}
            {member.deafened.then(|| view! { <span title="Deafened"><Icon name="headphonesOff"/><span class="sr-only">", deafened"</span></span> })}
        </span>
    }
    .into_any()
}

/// People in a voice channel, under it in the server tree (`VoiceMembers`).
#[component]
pub fn VoiceMembers(
    members: Vec<ChatVoiceMember>,
    speaking: BTreeSet<String>,
    me: String,
) -> impl IntoView {
    view! {
        <ul class="chat-voice-members" aria-label="In voice">
            {members.into_iter().map(|m| {
                let talking = speaking.contains(&m.id);
                let is_me = m.id == me;
                let hue = avatar_hue(&m.id);
                let init = voice_initials(&m.name);
                let name = m.name.clone();
                view! {
                    <li class=if talking { "chat-voice-member is-speaking" } else { "chat-voice-member" }>
                        <span class=format!("chat-voice-avatar contact-initials {hue}") aria-hidden="true">{init}</span>
                        <span class="chat-row-title">{name}{if is_me { " (you)" } else { "" }}{talking.then(|| view! { <span class="sr-only">", speaking"</span> })}</span>
                        <Marks member=m/>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
}

#[component]
pub fn Controls(
    call: ChatVoiceCall,
    mic: MicState,
    #[prop(optional)] big: bool,
    on_set: Callback<(bool, bool)>,
    on_leave: Callback<()>,
) -> impl IntoView {
    let muted = call.self_mute || call.self_deaf || mic != MicState::On;
    let deaf = call.self_deaf;
    let can_speak = call.can_speak;
    let self_mute = call.self_mute;
    view! {
        <div class=if big { "chat-voice-controls is-big" } else { "chat-voice-controls" }>
            <button
                type="button"
                class=if muted { "chat-voice-button is-off" } else { "chat-voice-button" }
                aria-pressed=muted.to_string()
                disabled=!can_speak
                title=if !can_speak { "You cannot speak in this channel" } else if muted { "Unmute" } else { "Mute" }
                aria-label="Mute"
                on:click=move |_| { on_set.try_run((!muted, if muted { false } else { deaf })); }
            >
                <Icon name=if muted { "micOff" } else { "mic" }/>
            </button>
            <button
                type="button"
                class=if deaf { "chat-voice-button is-off" } else { "chat-voice-button" }
                aria-pressed=deaf.to_string()
                aria-label="Deafen"
                title=if deaf { "Undeafen" } else { "Deafen" }
                on:click=move |_| { on_set.try_run((self_mute, !deaf)); }
            >
                <Icon name=if deaf { "headphonesOff" } else { "headphones" }/>
            </button>
            <button
                type="button"
                class=if big { "chat-voice-button is-leave is-wide" } else { "chat-voice-button is-leave" }
                aria-label="Leave voice"
                title="Leave voice"
                on:click=move |_| { on_leave.try_run(()); }
            >
                <Icon name="phoneOff"/>
                {big.then(|| view! { <span>"Leave"</span> })}
            </button>
        </div>
    }
}

/// The call at the bottom of the sidebar while you keep reading and writing (`VoiceDock`).
#[component]
pub fn VoiceDock(
    call: ChatVoiceCall,
    channel: String,
    server: String,
    here: bool,
    elsewhere: bool,
    mic: MicState,
    #[prop(optional, into)] class: String,
    on_set: Callback<(bool, bool)>,
    on_leave: Callback<()>,
    on_use_here: Callback<()>,
    on_open: Callback<()>,
    on_code: Callback<()>,
) -> impl IntoView {
    let live_class = format!("chat-voice-live is-{}", call.state);
    let label = state_label(&call);
    let e2ee = call.e2ee;
    let controls_mic = if here { mic } else { MicState::On };

    view! {
        <section class=format!("chat-voice-dock{class}") aria-label="Voice call">
            <div class="chat-voice-dock-head">
                <button type="button" class="chat-voice-dock-open" on:click=move |_| { on_open.try_run(()); }>
                    <span class=live_class aria-hidden="true"><Icon name="signal"/></span>
                    <span class="chat-voice-dock-text">
                        <b>{label}</b>
                        <small>{channel}" \u{00b7} "{server}</small>
                    </span>
                </button>
                {e2ee.then(|| view! {
                    <button type="button" class="icon-button chat-voice-lock" aria-label="End-to-end encrypted: verify the call code" title="End-to-end encrypted" on:click=move |_| { on_code.try_run(()); }>
                        <Icon name="lock"/>
                    </button>
                })}
            </div>
            {(!here).then(|| view! {
                <div class="chat-voice-away">
                    <span>{if elsewhere { "The call\u{2019}s audio is in another tab." } else { "The call\u{2019}s audio is not playing here." }}</span>
                    <button type="button" class="secondary-button" on:click=move |_| { on_use_here.try_run(()); }>"Use this tab"</button>
                </div>
            })}
            {(here && mic == MicState::Blocked).then(|| view! {
                <p class="chat-voice-note"><Icon name="micOff"/>"Microphone blocked. You can still listen."</p>
            })}
            <Controls call=call mic=controls_mic on_set=on_set on_leave=on_leave/>
        </section>
    }
}
