//! Chat zero-account onboarding (`EmptyOnboarding`): introduces WhatsApp and Discord
//! linking inside the shared workspace sheet when no accounts are linked yet.

use leptos::prelude::*;

use crate::icons::Icon;

/// Empty-state onboarding shown when the user has not linked any Chat account yet.
#[component]
pub fn EmptyOnboarding(
    #[prop(into)] available: Signal<bool>,
    on_link: Callback<Option<String>>,
) -> impl IntoView {
    view! {
        <div class="chat-onboarding">
            <div class="chat-onboard-head">
                <span class="chat-empty-icon" aria-hidden="true"><Icon name="chat"/></span>
                <h2>"No accounts linked"</h2>
                <p>"Link WhatsApp or Discord to bring their chats here. Each account keeps its own tab, views, and unread counts."</p>
            </div>
            <div class="chat-onboard-cards">
                <section class="chat-onboard-card" aria-label="WhatsApp">
                    <div class="chat-onboard-card-top">
                        <span class="chat-platform is-whatsapp" aria-hidden="true"><Icon name="chat"/></span>
                        <b>"WhatsApp"</b>
                    </div>
                    <p>"Link as a companion device using a QR code or phone pairing code. Imports chats, groups, reactions, and media."</p>
                    <button
                        type="button"
                        class="primary-button"
                        disabled=move || !available.get()
                        on:click=move |_| { on_link.try_run(Some("whatsapp".to_owned())); }
                    >
                        "Link WhatsApp"
                    </button>
                </section>
                <section class="chat-onboard-card" aria-label="Discord">
                    <div class="chat-onboard-card-top">
                        <span class="chat-platform is-discord" aria-hidden="true"><Icon name="hash"/></span>
                        <b>"Discord"</b>
                    </div>
                    <p>"Servers, text channels, direct messages, and DAVE E2EE voice channels via QR sign-in."</p>
                    <div class="chat-onboard-warn" role="note">
                        <Icon name="alert"/>
                        <span>"Breaks Discord\u{2019}s terms; your password is never seen."</span>
                    </div>
                    <button
                        type="button"
                        class="secondary-button"
                        disabled=move || !available.get()
                        on:click=move |_| { on_link.try_run(Some("discord".to_owned())); }
                    >
                        "Link Discord"
                    </button>
                </section>
            </div>
            <div class="chat-onboard-foot">
                <span><Icon name="lock"/>"Encrypted key store"</span>
                <span><Icon name="contacts"/>"Named by Dar Contacts"</span>
                <span><Icon name="folder"/>"Save media to Files"</span>
            </div>
        </div>
    }
}
