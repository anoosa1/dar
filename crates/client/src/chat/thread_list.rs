//! Chat thread list sheet: mobile account tabs, title and shortcuts trigger,
//! WhatsApp quick filter chips, Pinned/Recent sectioning, row hover triage,
//! zero-account onboarding, and phone voice dock.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::model::{
    CHAT_MUTE_CHOICES, ChatAccount, ChatAvatar, ChatOverview, ChatSetting, ChatThread, ChatView,
    QuickFilter, Ticks, iso, kind_icon, list_time, muted, now, preview, thread_display_name,
};
use super::onboarding::EmptyOnboarding;
use super::sidebar::AccountTabs;
use crate::icons::Icon;

/// Middle sheet showing the selected account's threads (or zero-account onboarding).
#[component]
pub fn ThreadList(
    #[prop(into)] title: Signal<String>,
    #[prop(into)] account: Signal<Option<ChatAccount>>,
    #[prop(into)] accounts: Signal<Vec<ChatAccount>>,
    #[prop(into)] account_key: Signal<String>,
    #[prop(into)] overview: Signal<Option<ChatOverview>>,
    #[prop(into)] load_error: Signal<String>,
    #[prop(into)] list_state: Signal<&'static str>,
    #[prop(into)] list_error: Signal<String>,
    #[prop(into)] view: Signal<ChatView>,
    quick_filter: RwSignal<QuickFilter>,
    #[prop(into)] discord: Signal<bool>,
    #[prop(into)] has_account: Signal<bool>,
    #[prop(into)] selected: Signal<String>,
    #[prop(into)] threads: Signal<Vec<ChatThread>>,
    #[prop(into)] has_more: Signal<bool>,
    #[prop(into)] loading_more: Signal<bool>,
    empty_text: Callback<(), Option<(&'static str, &'static str)>>,
    render_dock: Callback<&'static str, AnyView>,
    on_choose_account: Callback<String>,
    on_link: Callback<Option<String>>,
    on_open: Callback<(String, web_sys::HtmlElement)>,
    on_load_more: Callback<()>,
    on_retry_overview: Callback<()>,
    on_retry_threads: Callback<()>,
    on_setting: Callback<(String, ChatSetting)>,
    on_open_shortcuts: Callback<()>,
) -> impl IntoView {
    let can_write = Memo::new(move |_| {
        account.with(|a| {
            a.as_ref().is_some_and(|a| {
                matches!(
                    a.state.as_str(),
                    "connected" | "connecting" | "disconnected"
                )
            })
        })
    });
    let show_chips = Memo::new(move |_| {
        !discord.get() && has_account.get() && view.get() == ChatView::Chats
    });
    let filtered = Memo::new(move |_| {
        let active = show_chips.get();
        let f = quick_filter.get();
        threads.with(|list| {
            if !active || f == QuickFilter::All {
                list.clone()
            } else {
                list.iter().filter(|t| f.matches(t)).cloned().collect()
            }
        })
    });
    let has_pinned = Memo::new(move |_| {
        show_chips.get() && filtered.with(|list| list.iter().any(|t| t.pinned))
    });
    let pinned_threads = Memo::new(move |_| {
        if has_pinned.get() {
            filtered.with(|list| list.iter().filter(|t| t.pinned).cloned().collect::<Vec<_>>())
        } else {
            Vec::new()
        }
    });
    let regular_threads = Memo::new(move |_| {
        if has_pinned.get() {
            filtered.with(|list| list.iter().filter(|t| !t.pinned).cloned().collect::<Vec<_>>())
        } else {
            filtered.get()
        }
    });

    let render_row = move |thread: ChatThread| {
        let id = thread.id.clone();
        let open_id = id.clone();
        let set_id = id.clone();
        view! {
            <ThreadRow
                thread=thread
                selected=Signal::derive(move || selected.with(|s| *s == id))
                discord=discord
                can_write=can_write
                on_open=Callback::new(move |el| { on_open.try_run((open_id.clone(), el)); })
                on_setting=Callback::new(move |change| { on_setting.try_run((set_id.clone(), change)); })
            />
        }
    };

    let row_key = |t: &ChatThread| {
        (
            t.id.clone(),
            t.name.clone(),
            t.preview.clone(),
            t.preview_kind.clone(),
            t.preview_from_me,
            t.preview_sender.clone(),
            t.preview_status.clone(),
            t.at.to_bits(),
            t.unread,
            t.pinned,
            t.archived,
            t.muted_until.to_bits(),
        )
    };

    view! {
        <main class="pim-main chat-main" tabindex="-1" aria-label=move || title.get()>
            <AccountTabs
                accounts=accounts
                account_key=account_key
                class="chat-accounts chat-main-accounts"
                on_choose=on_choose_account
                on_link=Callback::new(move |_| { on_link.try_run(None); })
            />
            <div class="chat-list-heading">
                <h1>{move || title.get()}</h1>
                {move || account.get().map(|a| view! { <span class="chat-heading-count">{a.counts.for_view(view.get())}</span> })}
                <button
                    type="button"
                    class="icon-button chat-shortcuts-trigger"
                    aria-label="Keyboard shortcuts"
                    title="Keyboard shortcuts"
                    on:click=move |_| { on_open_shortcuts.try_run(()); }
                >
                    <Icon name="keyboard"/>
                </button>
            </div>
            {move || show_chips.get().then(|| view! {
                <div class="chat-chip-bar" role="radiogroup" aria-label="Filter chats">
                    {QuickFilter::ALL.into_iter().map(|f| {
                        let active = move || quick_filter.get() == f;
                        let active2 = active;
                        view! {
                            <button
                                type="button"
                                role="radio"
                                aria-checked=move || active().to_string()
                                class=move || if active2() { "chat-chip is-active" } else { "chat-chip" }
                                on:click=move |_| quick_filter.set(f)
                            >
                                {f.label()}
                            </button>
                        }
                    }).collect_view()}
                </div>
            })}
            {move || (!load_error.with(String::is_empty)).then(|| view! {
                <div class="chat-banner is-error" role="alert">
                    <Icon name="alert"/>
                    <span><b>"Chat could not be loaded"</b>{load_error.get()}</span>
                    <button type="button" class="secondary-button" on:click=move |_| { on_retry_overview.try_run(()); }>"Retry"</button>
                </div>
            })}
            {move || overview.with(|o| o.as_ref().filter(|o| !o.bridge.available && !o.bridge.detail.is_empty()).map(|o| o.bridge.detail.clone())).map(|detail| view! {
                <div class="chat-banner" role="status"><Icon name="alert"/><span><b>"Chat is unavailable"</b>{detail}</span></div>
            })}
            <div class="chat-list" role="list" aria-label=move || title.get()>
                {move || (overview.with(Option::is_none) && load_error.with(String::is_empty)).then(|| view! {
                    <p class="chat-empty" role="status">"Loading chats\u{2026}"</p>
                })}
                {move || overview.with(|o| o.as_ref().is_some_and(|o| o.accounts.is_empty())).then(|| view! {
                    <EmptyOnboarding
                        available=Signal::derive(move || overview.with(|o| o.as_ref().is_some_and(|o| o.bridge.available)))
                        on_link=on_link
                    />
                })}
                {move || (has_account.get() && list_state.get() == "loading" && threads.with(Vec::is_empty)).then(|| view! {
                    <p class="chat-empty" role="status">"Loading chats\u{2026}"</p>
                })}
                {move || (has_account.get() && list_state.get() == "error").then(|| view! {
                    <div class="chat-empty" role="alert">
                        <h2>"Chats could not be loaded"</h2>
                        <p>{list_error.get()}</p>
                        <button type="button" class="secondary-button" on:click=move |_| { on_retry_threads.try_run(()); }>"Retry"</button>
                    </div>
                })}
                {move || (has_account.get() && list_state.get() == "ready" && filtered.with(Vec::is_empty)).then(|| {
                    if threads.with(Vec::is_empty) {
                        empty_text.run(()).map(|(heading, text)| view! {
                            <div class="chat-empty"><h2>{heading}</h2><p>{text}</p></div>
                        }.into_any())
                    } else {
                        let filter_name = quick_filter.get().label().to_lowercase();
                        Some(view! {
                            <div class="chat-empty">
                                <h2>{format!("No {filter_name} chats")}</h2>
                                <button type="button" class="text-button" on:click=move |_| quick_filter.set(QuickFilter::All)>"Show all chats"</button>
                            </div>
                        }.into_any())
                    }
                }).flatten()}
                {move || has_pinned.get().then(|| view! { <div class="chat-list-section" role="presentation">"Pinned"</div> })}
                <For each=move || pinned_threads.get() key=row_key let:thread>
                    {render_row(thread)}
                </For>
                {move || (has_pinned.get() && !regular_threads.with(Vec::is_empty)).then(|| view! {
                    <div class="chat-list-section" role="presentation">"Recent"</div>
                })}
                <For each=move || regular_threads.get() key=row_key let:thread>
                    {render_row(thread)}
                </For>
                {move || has_more.get().then(|| view! {
                    <div class="chat-more">
                        <button type="button" class="text-button" disabled=move || loading_more.get() on:click=move |_| { on_load_more.try_run(()); }>
                            {move || if loading_more.get() { "Loading\u{2026}" } else { "Load more" }}
                        </button>
                    </div>
                })}
            </div>
            {move || render_dock.run(" is-phone")}
        </main>
    }
}

#[component]
fn ThreadRow(
    thread: ChatThread,
    #[prop(into)] selected: Signal<bool>,
    #[prop(into)] discord: Signal<bool>,
    #[prop(into)] can_write: Signal<bool>,
    on_open: Callback<web_sys::HtmlElement>,
    on_setting: Callback<ChatSetting>,
) -> impl IntoView {
    let name = thread_display_name(&thread);
    let text = preview(&thread);
    let quiet = muted(&thread);
    let pinned = thread.pinned;
    let archived = thread.archived;
    let unread = thread.unread;
    let class = move || {
        format!(
            "chat-row{}{}",
            if selected.get() { " is-selected" } else { "" },
            if unread > 0 { " is-unread" } else { "" }
        )
    };
    let at = thread.at;
    let icon = if thread.preview_kind.is_empty() {
        None
    } else {
        kind_icon(&thread.preview_kind)
    };

    view! {
        <div role="listitem" class=class>
            <button
                type="button"
                class="chat-row-btn"
                aria-current=move || selected.get().then_some("true")
                on:click=move |event| {
                    if let Some(el) = event.current_target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()) {
                        on_open.try_run(el);
                    }
                }
            >
                <ChatAvatar name=name.clone() group=thread.kind == "group"/>
                <span class="chat-row-name">{name}</span>
                <time class="chat-row-time" datetime=(at != 0.0).then(|| iso(at))>{list_time(at)}</time>
                <span class="chat-row-preview">
                    {thread.preview_from_me.then(|| view! { <Ticks status=thread.preview_status.clone()/> })}
                    {(!thread.preview_sender.is_empty()).then(|| view! { <b>{thread.preview_sender.clone()}": "</b> })}
                    {icon.map(|icon| view! { <Icon name=icon/> })}
                    <span class="chat-row-text">{text}</span>
                </span>
                <span class="chat-row-marks">
                    {quiet.then(|| view! { <span title="Muted"><Icon name="bellOff"/><span class="sr-only">"Muted"</span></span> })}
                    {pinned.then(|| view! { <span title="Pinned"><Icon name="pin"/><span class="sr-only">"Pinned"</span></span> })}
                    {(unread > 0).then(|| view! {
                        <span class=if quiet { "chat-unread is-muted" } else { "chat-unread" }>{unread}<span class="sr-only">" unread"</span></span>
                    })}
                </span>
            </button>
            {move || (!discord.get() && can_write.get()).then(|| {
                let pin_label = if pinned { "Unpin" } else { "Pin" };
                let mute_label = if quiet { "Unmute" } else { "Mute" };
                let mute_title = if quiet { "Unmute" } else { "Mute for 8 hours" };
                let arch_label = if archived { "Unarchive" } else { "Archive" };
                view! {
                    <div class="chat-row-triage">
                        <button
                            type="button"
                            class=if pinned { "chat-triage-btn is-active" } else { "chat-triage-btn" }
                            aria-label=pin_label
                            title=if archived { "Unarchive to pin" } else { pin_label }
                            disabled=archived
                            on:click=move |ev| {
                                ev.stop_propagation();
                                on_setting.try_run(ChatSetting { pinned: Some(!pinned), ..Default::default() });
                            }
                        >
                            <Icon name="pin"/>
                        </button>
                        <button
                            type="button"
                            class=if quiet { "chat-triage-btn is-active" } else { "chat-triage-btn" }
                            aria-label=mute_label
                            title=mute_title
                            on:click=move |ev| {
                                ev.stop_propagation();
                                let until = if quiet { 0 } else { now() as i64 + CHAT_MUTE_CHOICES[0].1 };
                                on_setting.try_run(ChatSetting { muted_until: Some(until), ..Default::default() });
                            }
                        >
                            <Icon name=if quiet { "bellOff" } else { "bell" }/>
                        </button>
                        <button
                            type="button"
                            class=if archived { "chat-triage-btn is-active" } else { "chat-triage-btn" }
                            aria-label=arch_label
                            title=arch_label
                            on:click=move |ev| {
                                ev.stop_propagation();
                                on_setting.try_run(ChatSetting { archived: Some(!archived), ..Default::default() });
                            }
                        >
                            <Icon name="archive"/>
                        </button>
                    </div>
                }
            })}
        </div>
    }
}
