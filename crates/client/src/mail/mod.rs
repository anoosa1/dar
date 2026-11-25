//! Mail workspace: coordinates sidebar, conversation list, thread reader, floating/minimizable
//! compose sheet, settings/account panes, global keyboard shortcuts, and confirmation/toast UI.

mod compose;
mod list;
mod model;
mod panes;
mod reader;
mod shortcuts;
mod sidebar;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use wasm_bindgen::JsCast;

use compose::ComposeSheet;
use list::MailList;
use model::{
    MailDensity, MailFilter, MailLayout, MailSort, MailState, Pane, UNIFIED, View, read_url,
    write_url,
};
use panes::{AddAccountPane, MailSettings};
use reader::MailReader;
use shortcuts::ShortcutsModal;
use sidebar::MailSidebar;

use crate::api::BrowserSession;
use crate::shared::context;
use crate::shell::WorkspaceHeader;
use crate::ui::{Dialog, document, listen, unique_id};

#[component]
pub fn MailWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let (start_view, start_message, start_extra) = read_url();
    let prefs = context().preferences;

    let images = Memo::new(move |_| {
        prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("mailImages"))
                .and_then(Value::as_str)
                .unwrap_or("block")
                .to_owned()
        })
    });
    let mark_read = Memo::new(move |_| {
        prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("mailMarkRead"))
                .and_then(Value::as_str)
                .unwrap_or("open")
                .to_owned()
        })
    });
    let group_threads = Memo::new(move |_| {
        prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("mailThreads"))
                .and_then(Value::as_str)
                .map(|v| v != "off")
                .unwrap_or(true)
        })
    });
    let allowed_senders = Memo::new(move |_| {
        prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("mailAllowedSenders"))
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.to_lowercase())
                        .collect()
                })
                .unwrap_or_default()
        })
    });

    let init_layout = prefs.value.with_untracked(|p| {
        if p.as_ref()
            .and_then(|p| p.rest.get("mailLayout"))
            .and_then(Value::as_str)
            == Some("list")
        {
            MailLayout::List
        } else {
            MailLayout::Split
        }
    });
    let init_density = prefs.value.with_untracked(|p| {
        if p.as_ref()
            .and_then(|p| p.rest.get("mailDensity"))
            .and_then(Value::as_str)
            == Some("compact")
        {
            MailDensity::Compact
        } else {
            MailDensity::Comfortable
        }
    });

    let state = MailState {
        overview: RwSignal::new(None),
        load_error: RwSignal::new(String::new()),
        view: RwSignal::new(start_view.clone()),
        items: RwSignal::new(Vec::new()),
        cursor: RwSignal::new(None),
        list_state: RwSignal::new("loading"),
        list_error: RwSignal::new(String::new()),
        selected: RwSignal::new(None),
        detail: RwSignal::new(None),
        detail_error: RwSignal::new(String::new()),
        checked: RwSignal::new(HashSet::new()),
        last_checked_id: RwSignal::new(None),
        filter: RwSignal::new(MailFilter::All),
        account_filter: RwSignal::new(None),
        sort: RwSignal::new(MailSort::Newest),
        layout: RwSignal::new(init_layout),
        density: RwSignal::new(init_density),
        shortcuts_open: RwSignal::new(false),
        expand_all_signal: RwSignal::new(0),
        pane: RwSignal::new(None),
        compose: RwSignal::new(None),
        compose_minimized: RwSignal::new(false),
        toast: RwSignal::new(None),
        confirm: RwSignal::new(None),
        syncing: RwSignal::new(false),
        collapsed: RwSignal::new(HashSet::new()),
        navigation: RwSignal::new(false),
        generation: StoredValue::new(0),
        session: StoredValue::new(session.clone()),
        images,
        mark_read,
        group_threads,
        allowed_senders,
        sending: StoredValue::new(0),
    };

    // Sync layout & density when preferences finish loading initially
    Effect::new(move |_| {
        prefs.value.with(|p| {
            if let Some(p) = p {
                if let Some(l) = p.rest.get("mailLayout").and_then(Value::as_str) {
                    state.layout.set(if l == "list" {
                        MailLayout::List
                    } else {
                        MailLayout::Split
                    });
                }
                if let Some(d) = p.rest.get("mailDensity").and_then(Value::as_str) {
                    state.density.set(if d == "compact" {
                        MailDensity::Compact
                    } else {
                        MailDensity::Comfortable
                    });
                }
            }
        });
    });

    // After OAuth: say whether the account was added.
    if let Some(error) = start_extra.get("oauthError") {
        state.fail(format!("The account was not added: {error}"));
    } else if start_extra.contains_key("account") {
        state.notify("Account added; its mail is being mirrored");
    }
    if start_extra.is_empty() {
        write_url(&start_view, start_message.as_deref());
    } else {
        write_url(&start_view, None);
    }

    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || alive.store(false, Ordering::Relaxed));
    }
    spawn_local(async move {
        state.fetch_overview().await;
        state.fetch_items(true).await;
        while alive.load(Ordering::Relaxed) {
            crate::sleep(30_000).await;
            if !alive.load(Ordering::Relaxed) {
                break;
            }
            state.fetch_overview().await;
            if state.checked.try_with_untracked(HashSet::is_empty) == Some(true) {
                state.fetch_items(true).await;
            }
        }
    });
    if let Some(id) = start_message {
        state.open(id);
    }

    // Images: reload the open message when the preference changes.
    Effect::new(move |prev: Option<String>| {
        let current = state.images.get();
        if prev.is_some_and(|p| p != current)
            && let Some(id) = state.selected.get_untracked()
        {
            state.open(id);
        }
        current
    });

    // Toasts leave after eight seconds (ten while a send can be undone).
    let toast_seq = StoredValue::new(0u32);
    Effect::new(move |_| {
        let Some(t) = state.toast.get() else { return };
        let id = toast_seq.get_value() + 1;
        toast_seq.set_value(id);
        let wait = if t.action.is_some() { 10_000 } else { 8000 };
        spawn_local(async move {
            crate::sleep(wait).await;
            if toast_seq.try_get_value() == Some(id) {
                let _ = state.toast.try_set(None);
            }
        });
    });

    // Global keyboard navigation & triage shortcuts
    Effect::new(move |_| {
        listen(&document(), "keydown", false, move |event| {
            let event: web_sys::KeyboardEvent = event.unchecked_into();
            if event.default_prevented() || event.ctrl_key() || event.meta_key() || event.alt_key()
            {
                return;
            }
            let typing = document().active_element().is_some_and(|e| {
                matches!(e.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                    || e.closest("[contenteditable],[role=dialog],.editing-pane,.mail-compose")
                        .ok()
                        .flatten()
                        .is_some()
            });
            if typing
                || state.pane.get_untracked().is_some()
                || (state.compose.with_untracked(Option::is_some)
                    && !state.compose_minimized.get_untracked())
            {
                return;
            }
            if state.shortcuts_open.get_untracked() {
                if event.key() == "Escape" || event.key() == "?" {
                    event.prevent_default();
                    state.shortcuts_open.set(false);
                }
                return;
            }
            let selected = state.selection();
            let handled = match event.key().as_str() {
                "j" | "ArrowDown" => {
                    state.step(1);
                    true
                }
                "k" | "ArrowUp" => {
                    state.step(-1);
                    true
                }
                "x" => {
                    if let Some(id) = state.selected.get_untracked() {
                        state.toggle_check(id, false);
                        true
                    } else {
                        false
                    }
                }
                "c" => {
                    state.new_message();
                    true
                }
                "r" => {
                    state.reply("reply");
                    true
                }
                "a" => {
                    state.reply("all");
                    true
                }
                "f" => {
                    state.reply("forward");
                    true
                }
                "e" if !selected.is_empty() => {
                    state.move_to(selected, Some("archive"), None, "archived".into());
                    true
                }
                "#" if !selected.is_empty() => {
                    state.move_to(selected, Some("trash"), None, "moved to Trash".into());
                    true
                }
                "s" if !selected.is_empty() => {
                    let flagged = state
                        .detail
                        .with_untracked(|d| d.as_ref().is_some_and(|d| d.summary.flagged));
                    state.set_flags(selected, None, Some(!flagged), true);
                    true
                }
                "u" if !selected.is_empty() => {
                    state.set_flags(selected, Some(false), None, true);
                    true
                }
                ";" if state.selected.get_untracked().is_some() => {
                    state.expand_all_signal.update(|n| *n += 1);
                    true
                }
                "?" => {
                    state.shortcuts_open.set(true);
                    true
                }
                "Escape" if state.selected.get_untracked().is_some() => {
                    state.close_reader();
                    true
                }
                _ => false,
            };
            if handled {
                event.prevent_default();
            }
        });
    });

    let title = Memo::new(move |_| match state.view.get() {
        View::Unified(v) => UNIFIED
            .iter()
            .find(|(id, ..)| *id == v)
            .map(|(_, l, _)| (*l).to_owned())
            .unwrap_or_default(),
        View::Mailbox(id) => state
            .overview
            .with(|o| {
                o.as_ref().and_then(|o| {
                    o.accounts.iter().find_map(|a| {
                        a.mailboxes
                            .iter()
                            .find(|m| m.id == id)
                            .map(|m| format!("{} \u{00b7} {}", a.name, m.name))
                    })
                })
            })
            .unwrap_or_else(|| "Mailbox".into()),
    });

    let has_selection = Memo::new(move |_| state.selected.get().is_some());
    let has_accounts = Memo::new(move |_| {
        state
            .overview
            .with(|o| o.as_ref().is_some_and(|o| !o.accounts.is_empty()))
    });
    let show_reader_sheet = Memo::new(move |_| {
        has_selection.get() || (state.layout.get() == MailLayout::Split && has_accounts.get())
    });
    let navigation_id = unique_id();

    view! {
        <div class=move || format!(
            "app-shell pim-shell mail-shell{}{}",
            if has_selection.get() { " has-pim-details" } else { "" },
            if show_reader_sheet.get() { " has-mail-split" } else { "" }
        )>
            <WorkspaceHeader
                app="mail"
                session=session.clone()
                on_signed_out=on_signed_out
                on_settings=Callback::new(move |_| state.pane.set(Some(Pane::Settings("Accounts"))))
            />
            <div class="pim-workspace mail-workspace">
                {move || state.navigation.get().then(|| view! {
                    <button class="nav-scrim" aria-label="Close navigation" on:click=move |_| state.navigation.set(false)></button>
                })}
                <MailSidebar state=state navigation_id=navigation_id/>
                <MailList state=state title=title/>
                <Show when=move || show_reader_sheet.get()>
                    <aside
                        class=move || if has_selection.get() { "pim-details mail-details" } else { "pim-details mail-details is-empty" }
                        tabindex="-1"
                        aria-label="Message"
                    >
                        <MailReader state=state/>
                    </aside>
                </Show>
            </div>

            {move || state.pane.get().map(|p| match p {
                Pane::AddAccount(account) => view! { <AddAccountPane state=state account=account/> }.into_any(),
                Pane::Settings(section) => view! { <MailSettings state=state section=section/> }.into_any(),
            })}

            {move || state.compose.get().map(|spec| view! { <ComposeSheet state=state spec=spec/> })}

            <ShortcutsModal state=state/>

            {move || state.confirm.get().map(|c| {
                let run = StoredValue::new(c.run.clone());
                view! {
                    <Dialog title=c.title.clone() on_close=Callback::new(move |_| state.confirm.set(None))>
                        <p>{c.message.clone()}</p>
                        <div class="dialog-actions">
                            <button type="button" class="text-button" on:click=move |_| state.confirm.set(None)>"Cancel"</button>
                            <button type="button" class="primary-button danger" on:click=move |_| { state.confirm.set(None); run.with_value(|r| r()); }>{c.action}</button>
                        </div>
                    </Dialog>
                }
            })}

            <div class="mail-toast-region" role="status" aria-live="polite">
                {move || state.toast.get().map(|t| view! {
                    <div class=if t.error { "mail-toast is-error" } else { "mail-toast" }>
                        <span class="mail-toast-text">{t.message.clone()}</span>
                        {t.action.map(|(label, run)| view! {
                            <button type="button" class="mail-toast-action" on:click=move |_| { state.toast.set(None); run(); }>{label}</button>
                        })}
                    </div>
                })}
            </div>
        </div>
    }
}
