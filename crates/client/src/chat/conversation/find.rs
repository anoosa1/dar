//! Inline in-conversation search strip (`FindBar`).

use leptos::html;
use leptos::prelude::*;

use crate::icons::Icon;

/// In-conversation find bar with match stepping and keyboard navigation.
#[component]
pub fn FindBar(
    query: RwSignal<String>,
    #[prop(into)] match_count: Signal<usize>,
    #[prop(into)] current_idx: Signal<usize>,
    on_step: Callback<isize>,
    on_close: Callback<()>,
) -> impl IntoView {
    let input_ref = NodeRef::<html::Input>::new();
    Effect::new(move |_| {
        if let Some(el) = input_ref.get() {
            let _ = el.focus();
            if !query.get_untracked().is_empty() {
                el.select();
            }
        }
    });

    view! {
        <div class="chat-find-bar" role="search" aria-label="Find in conversation">
            <Icon name="search"/>
            <input
                node_ref=input_ref
                type="search"
                autofocus
                placeholder="Find in conversation\u{2026}"
                aria-label="Find in conversation"
                prop:value=move || query.get()
                on:input=move |e| query.set(event_target_value(&e))
                on:keydown=move |e: web_sys::KeyboardEvent| {
                    if e.key() == "Enter" && !e.is_composing() {
                        e.prevent_default();
                        on_step.try_run(if e.shift_key() { -1 } else { 1 });
                    } else if e.key() == "Escape" {
                        e.prevent_default();
                        e.stop_propagation();
                        on_close.try_run(());
                    }
                }
            />
            <span class="chat-find-count">
                {move || {
                    let count = match_count.get();
                    if count == 0 {
                        if query.with(|q| q.trim().is_empty()) {
                            String::new()
                        } else {
                            "0 matches".to_owned()
                        }
                    } else {
                        let shown = current_idx.get().min(count - 1) + 1;
                        format!("{shown} / {count}")
                    }
                }}
            </span>
            <button
                type="button"
                class="icon-button"
                aria-label="Previous match"
                title="Previous match"
                disabled=move || match_count.get() == 0
                on:click=move |_| { on_step.try_run(-1); }
            >
                <Icon name="arrowUp"/>
            </button>
            <button
                type="button"
                class="icon-button"
                aria-label="Next match"
                title="Next match"
                disabled=move || match_count.get() == 0
                on:click=move |_| { on_step.try_run(1); }
            >
                <Icon name="arrowDown"/>
            </button>
            <button
                type="button"
                class="icon-button"
                aria-label="Close find"
                title="Close find"
                on:click=move |_| { on_close.try_run(()); }
            >
                <Icon name="close"/>
            </button>
        </div>
    }
}
