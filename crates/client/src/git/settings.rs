//! Git settings sheet: General (diff layout, commits view, wrap, About rail),
//! HTTPS & keys (clone endpoint + app passwords), and Keyboard shortcuts.

use leptos::prelude::*;
use serde_json::json;

use super::model::copy;
use crate::api::BrowserSession;
use crate::icons::Icon;
use crate::settings::{
    Notice, SaveProblems, Saver, Segmented, SettingRow, SettingSwitch, SettingsSheet,
};
use crate::ui::window;

#[component]
pub fn GitSettings(
    session: BrowserSession,
    initial_section: &'static str,
    on_close: Callback<()>,
) -> impl IntoView {
    let username = StoredValue::new(session.user.username.clone());
    let saver = Saver::new(session);
    let section = RwSignal::new(initial_section);

    let text = move |key: &'static str, default: &'static str| {
        Signal::derive(move || saver.value(key).as_str().unwrap_or(default).to_owned())
    };
    let flag = move |key: &'static str, default: bool| {
        Signal::derive(move || saver.value(key).as_bool().unwrap_or(default))
    };

    let address = StoredValue::new(format!(
        "{}/dav/git",
        window().location().origin().unwrap_or_default()
    ));

    let notify_copy = Callback::new(move |msg: String| {
        let _ = saver.notice.try_set(Some(Notice {
            message: msg,
            undo: None,
        }));
    });

    view! {
        <SettingsSheet
            title="Git settings"
            icon="branch"
            sections=vec!["General", "HTTPS & keys", "Keyboard shortcuts"]
            section=section
            on_close=on_close
            notice=saver.notice
        >
            <SaveProblems saver=saver/>
            {move || match section.get() {
                "General" if saver.preferences.with(Option::is_some) => view! {
                    <section class="settings-group" aria-label="Diffs and history">
                        <h3>"Diffs and history"</h3>
                        <SettingRow
                            label="Default diff layout"
                            hint="Used in commit details, comparisons and pull requests"
                            state=saver.state("gitDiffLayout")
                        >
                            <Segmented
                                label="Default diff layout"
                                value=text("gitDiffLayout", "split")
                                options=vec![("split", "Split"), ("unified", "Unified")]
                                on_change=Callback::new(move |value: String| {
                                    let msg = if value == "unified" {
                                        "Diffs open in Unified view"
                                    } else {
                                        "Diffs open in Split view"
                                    };
                                    saver.change("gitDiffLayout", json!(value), msg.into(), true);
                                })
                            />
                        </SettingRow>
                        <SettingRow
                            label="Default commits view"
                            hint="Show branch lane graph or chronological list"
                            state=saver.state("gitCommitsView")
                        >
                            <Segmented
                                label="Default commits view"
                                value=text("gitCommitsView", "graph")
                                options=vec![("graph", "Graph"), ("list", "List")]
                                on_change=Callback::new(move |value: String| {
                                    let msg = if value == "list" {
                                        "Commits open as a day list"
                                    } else {
                                        "Commits open with the branch graph"
                                    };
                                    saver.change("gitCommitsView", json!(value), msg.into(), true);
                                })
                            />
                        </SettingRow>
                        <SettingSwitch
                            label="Wrap long lines"
                            hint="Soft-wrap source lines in diffs by default"
                            state=saver.state("gitWrapLines")
                            checked=flag("gitWrapLines", false)
                            on_change=Callback::new(move |value: bool| {
                                saver.change(
                                    "gitWrapLines",
                                    json!(value),
                                    if value { "Diff line wrapping on" } else { "Diff line wrapping off" }.into(),
                                    true,
                                );
                            })
                        />
                    </section>
                    <section class="settings-group" aria-label="Repository layout">
                        <h3>"Repository layout"</h3>
                        <SettingSwitch
                            label="Show repository About rail"
                            hint="Display description, release tags, counts and quick clone beside Code"
                            state=saver.state("gitShowAbout")
                            checked=flag("gitShowAbout", true)
                            on_change=Callback::new(move |value: bool| {
                                saver.change(
                                    "gitShowAbout",
                                    json!(value),
                                    if value { "About rail shown" } else { "About rail hidden" }.into(),
                                    true,
                                );
                            })
                        />
                    </section>
                }.into_any(),
                "General" => view! { <p role="status">"Loading settings\u{2026}"</p> }.into_any(),
                "HTTPS & keys" => {
                    let addr = address.get_value();
                    let addr_copy = addr.clone();
                    let user = username.get_value();
                    view! {
                        <section class="settings-group" aria-label="Git over HTTPS">
                            <h3>"Git over HTTPS"</h3>
                            <div class="settings-row is-stacked">
                                <div class="settings-row-text">
                                    <span class="settings-row-label">"Clone and push credentials"</span>
                                    <small>
                                        "Sign in as "<code>{user}</code>
                                        " using an app password with Files access (read to clone, write to push). Repositories live under "
                                        <code>{addr}</code>"."
                                    </small>
                                </div>
                                <div class="settings-row-control">
                                    <span class="git-settings-sync">
                                        <button
                                            type="button"
                                            class="secondary-button"
                                            on:click=move |_| copy(addr_copy.clone(), notify_copy, "Git HTTPS base URL")
                                        >
                                            <Icon name="copy"/>"Copy base URL"
                                        </button>
                                        <a class="secondary-button" href="/settings?section=security">
                                            <Icon name="key"/>"App passwords"
                                        </a>
                                    </span>
                                </div>
                            </div>
                        </section>
                    }.into_any()
                }
                "Keyboard shortcuts" => view! {
                    <dl class="ui-shortcuts">
                        <dt>"t"</dt><dd>"Focus file filter on Code tab"</dd>
                        <dt>"1\u{2013}6"</dt><dd>"Switch repository tabs (Code, Issues, Pull requests, Commits, Branches, Tags)"</dd>
                        <dt>"f"</dt><dd>"Toggle focus reading / review mode"</dd>
                        <dt>"d"</dt><dd>"Switch Split / Unified diff layout"</dd>
                        <dt>"w"</dt><dd>"Toggle line wrapping in diff or file reader"</dd>
                        <dt>"?"</dt><dd>"Open Git shortcuts & settings"</dd>
                        <dt>"Escape"</dt><dd>"Close open sheet, pane, or exit focus mode"</dd>
                        <dt>"Ctrl / Cmd K"</dt><dd>"Search all apps"</dd>
                    </dl>
                }.into_any(),
                _ => ().into_any(),
            }}
        </SettingsSheet>
    }
}
