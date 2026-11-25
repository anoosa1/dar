//! Keyboard shortcuts modal (`?`) for Mail.

use leptos::prelude::*;

use super::model::MailState;
use crate::ui::Dialog;

#[component]
pub(crate) fn ShortcutsModal(state: MailState) -> impl IntoView {
    view! {
        <Show when=move || state.shortcuts_open.get()>
            <Dialog title="Mail keyboard shortcuts" on_close=Callback::new(move |_| state.shortcuts_open.set(false))>
                <div class="mail-shortcuts-grid">
                    <section>
                        <h4>"Navigation & triage"</h4>
                        <dl class="mail-keys">
                            <dt><kbd>"j"</kbd><kbd>"\u{2193}"</kbd></dt><dd>"Next conversation"</dd>
                            <dt><kbd>"k"</kbd><kbd>"\u{2191}"</kbd></dt><dd>"Previous conversation"</dd>
                            <dt><kbd>"x"</kbd></dt><dd>"Select / deselect row"</dd>
                            <dt><kbd>"e"</kbd></dt><dd>"Archive"</dd>
                            <dt><kbd>"#"</kbd></dt><dd>"Move to Trash"</dd>
                            <dt><kbd>"s"</kbd></dt><dd>"Toggle flag"</dd>
                            <dt><kbd>"u"</kbd></dt><dd>"Mark as unread"</dd>
                        </dl>
                    </section>
                    <section>
                        <h4>"Reading & writing"</h4>
                        <dl class="mail-keys">
                            <dt><kbd>"c"</kbd></dt><dd>"Compose new message"</dd>
                            <dt><kbd>"r"</kbd></dt><dd>"Reply"</dd>
                            <dt><kbd>"a"</kbd></dt><dd>"Reply all"</dd>
                            <dt><kbd>"f"</kbd></dt><dd>"Forward"</dd>
                            <dt><kbd>";"</kbd></dt><dd>"Expand / collapse all in thread"</dd>
                            <dt><kbd>"Ctrl"</kbd><kbd>"\u{21b5}"</kbd></dt><dd>"Send message"</dd>
                            <dt><kbd>"Esc"</kbd></dt><dd>"Close reader or compose"</dd>
                        </dl>
                    </section>
                </div>
                <div class="dialog-actions">
                    <button type="button" class="primary-button" on:click=move |_| state.shortcuts_open.set(false)>"Done"</button>
                </div>
            </Dialog>
        </Show>
    }
}
