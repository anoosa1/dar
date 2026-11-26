//! Chat settings sheet (`ChatSettings`): linked accounts, display and composer preferences
//! (`chatDensity`, `chatSendKey`, `chatFormatMarkdown`, `chatShowEmoji`), and keyboard shortcuts.

use leptos::prelude::*;
use serde_json::{Value, json};

use super::model::{CHAT_LIMITS_ACCOUNTS, ChatAccount, phone_of, platform_name};
use crate::api::BrowserSession;
use crate::icons::Icon;
use crate::settings::{
    SaveProblems, Saver, Segmented, SettingRow, SettingSwitch, SettingsSheet,
};
use crate::ui::{ActionMenu, MenuAction};

/// Chat settings sheet with Accounts, Display, and Keyboard shortcuts sections.
#[component]
pub fn ChatSettings(
    session: BrowserSession,
    initial_section: &'static str,
    #[prop(into)] accounts: Signal<Vec<ChatAccount>>,
    on_close: Callback<()>,
    on_link: Callback<()>,
    actions: Callback<ChatAccount, Vec<MenuAction>>,
) -> impl IntoView {
    let saver = Saver::new(session);
    let section = RwSignal::new(initial_section);

    let text = move |key: &'static str, default: &'static str| {
        Signal::derive(move || saver.value(key).as_str().unwrap_or(default).to_owned())
    };
    let flag = move |key: &'static str, default: bool| {
        Signal::derive(move || saver.value(key).as_bool().unwrap_or(default))
    };
    let set = move |key: &'static str, value: Value, message: String| {
        saver.change(key, value, message, true);
    };

    view! {
        <SettingsSheet
            title="Chat settings"
            icon="chat"
            sections=vec!["Accounts", "Display", "Keyboard shortcuts"]
            section=section
            on_close=on_close
            notice=saver.notice
            footer="Each account keeps its own chats."
        >
            <SaveProblems saver=saver/>
            {move || match section.get() {
                "Accounts" => view! {
                    <section class="settings-group" aria-label="Accounts">
                        <h3>"Linked accounts"</h3>
                        {move || accounts.get().into_iter().map(|a| {
                            let phone = phone_of(&a.jid);
                            let (state, badge_class) = match a.state.as_str() {
                                "connected" => ("Connected", "chat-account-badge is-online"),
                                "loggedOut" => (
                                    if a.platform == "discord" { "Signed out" } else { "Unlinked" },
                                    "chat-account-badge is-warn",
                                ),
                                "linking" => ("Not linked yet", "chat-account-badge is-warn"),
                                "failed" => ("Stopped", "chat-account-badge is-err"),
                                _ => ("Reconnecting", "chat-account-badge is-warn"),
                            };
                            let hint = format!(
                                "{}{}",
                                platform_name(&a.platform),
                                if a.platform == "whatsapp" && !phone.is_empty() {
                                    format!(" \u{b7} {phone}")
                                } else {
                                    String::new()
                                }
                            );
                            let label = format!("Actions for {}", a.name);
                            let account = a.clone();
                            view! {
                                <div class="settings-row">
                                    <div class="settings-row-text">
                                        <span class="settings-row-label">{a.name.clone()}</span>
                                        <small>{hint}</small>
                                    </div>
                                    <span class=badge_class>{state}</span>
                                    <div class="settings-row-control">
                                        <ActionMenu label=label actions=Callback::new(move |_| actions.run(account.clone()))>
                                            <Icon name="more"/>
                                        </ActionMenu>
                                    </div>
                                </div>
                            }
                        }).collect_view()}
                        <SettingRow
                            label="Link an account"
                            hint=format!("WhatsApp or Discord, up to {CHAT_LIMITS_ACCOUNTS}.")
                            state=Signal::stored(None)
                        >
                            <button type="button" class="secondary-button" on:click=move |_| { on_link.try_run(()); }>
                                <Icon name="plus"/>"Link an account"
                            </button>
                        </SettingRow>
                    </section>
                }.into_any(),
                "Keyboard shortcuts" => view! {
                    <dl class="ui-shortcuts">
                        <dt>"Enter / Ctrl+Enter"</dt><dd>"Send the message"</dd>
                        <dt>"Shift + Enter"</dt><dd>"New line"</dd>
                        <dt>"J / K"</dt><dd>"Next or previous chat"</dd>
                        <dt>"Alt + 1\u{2013}9"</dt><dd>"Switch account tab"</dd>
                        <dt>"N"</dt><dd>"New chat or message"</dd>
                        <dt>"I"</dt><dd>"Toggle chat info"</dd>
                        <dt>"Ctrl / Cmd F"</dt><dd>"Find in conversation"</dd>
                        <dt>"Escape"</dt><dd>"Cancel reply, close find, pane, or chat"</dd>
                        <dt>"Ctrl / Cmd K"</dt><dd>"Search all apps"</dd>
                    </dl>
                }.into_any(),
                _ if saver.preferences.with(Option::is_none) => view! {
                    <p role="status">"Loading settings\u{2026}"</p>
                }.into_any(),
                _ => view! {
                    <section class="settings-group" aria-label="Appearance">
                        <h3>"Appearance"</h3>
                        <SettingRow
                            label="Message density"
                            hint="Applies to chat list and message stream"
                            state=saver.state("chatDensity")
                        >
                            <Segmented
                                label="Message density"
                                value=text("chatDensity", "comfortable")
                                options=vec![("comfortable", "Comfortable"), ("compact", "Compact")]
                                on_change=Callback::new(move |v: String| {
                                    let msg = if v == "compact" {
                                        "Message density set to Compact"
                                    } else {
                                        "Message density set to Comfortable"
                                    };
                                    set("chatDensity", json!(v), msg.into());
                                })
                            />
                        </SettingRow>
                        <SettingRow
                            label="Send message with"
                            hint="Shift+Enter always adds a new line"
                            state=saver.state("chatSendKey")
                        >
                            <Segmented
                                label="Send message with"
                                value=text("chatSendKey", "enter")
                                options=vec![("enter", "Enter"), ("modEnter", "Ctrl+Enter")]
                                on_change=Callback::new(move |v: String| {
                                    let msg = if v == "modEnter" {
                                        "Send message with Ctrl+Enter"
                                    } else {
                                        "Send message with Enter"
                                    };
                                    set("chatSendKey", json!(v), msg.into());
                                })
                            />
                        </SettingRow>
                        <SettingSwitch
                            label="Format inline Markdown"
                            hint="Render bold, italic, code, and links"
                            state=saver.state("chatFormatMarkdown")
                            checked=flag("chatFormatMarkdown", true)
                            on_change=Callback::new(move |v: bool| {
                                let msg = if v {
                                    "Inline Markdown formatting on"
                                } else {
                                    "Inline Markdown formatting off"
                                };
                                set("chatFormatMarkdown", json!(v), msg.into());
                            })
                        />
                        <SettingSwitch
                            label="Show emoji button"
                            hint="Quick emoji picker in the composer"
                            state=saver.state("chatShowEmoji")
                            checked=flag("chatShowEmoji", true)
                            on_change=Callback::new(move |v: bool| {
                                let msg = if v {
                                    "Emoji button shown"
                                } else {
                                    "Emoji button hidden"
                                };
                                set("chatShowEmoji", json!(v), msg.into());
                            })
                        />
                    </section>
                }.into_any(),
            }}
        </SettingsSheet>
    }
}
