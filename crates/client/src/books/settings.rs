//! Books settings sheet: General library settings, Reading defaults (EPUB, PDF, Comics),
//! and Keyboard shortcuts.

use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api::BrowserSession;
use crate::books::model::{
    BookLibrary, DEFAULT_BOOKS_FOLDER, FONT_SIZES, SORTS, books_folder_parts,
};
use crate::settings::{SaveProblems, Saver, Segmented, SettingRow, SettingsSheet};
use crate::ui::Select;

#[component]
pub fn BooksSettings(
    session: BrowserSession,
    library: RwSignal<Option<BookLibrary>>,
    on_close: Callback<()>,
) -> impl IntoView {
    let saver = Saver::new(session);
    let section = RwSignal::new("General");
    let draft: RwSignal<Option<String>> = RwSignal::new(None);

    let text = move |key: &'static str, default: &'static str| {
        Signal::derive(move || {
            let v = saver.value(key);
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
                .unwrap_or_else(|| default.to_owned())
        })
    };
    let set = move |key: &'static str, value: Value, message: String| {
        saver.change(key, value, message, true);
    };

    let saved_folder = Memo::new(move |_| {
        saver
            .value("booksFolder")
            .as_str()
            .map(str::to_owned)
            .or_else(|| library.with(|l| l.as_ref().map(|l| l.path.clone())))
            .unwrap_or_else(|| DEFAULT_BOOKS_FOLDER.to_owned())
    });
    let folder_value = Memo::new(move |_| draft.get().unwrap_or_else(|| saved_folder.get()));
    let folder_valid = Memo::new(move |_| books_folder_parts(&folder_value.get()).is_some());

    let sort_sig = text("booksSort", "recent");
    let sort_select_val = RwSignal::new(sort_sig.get_untracked());
    Effect::new(move |_| sort_select_val.set(sort_sig.get()));

    let size_sig = text("booksFontSize", "18");
    let size_select_val = RwSignal::new(size_sig.get_untracked());
    Effect::new(move |_| size_select_val.set(size_sig.get()));

    view! {
        <SettingsSheet
            title="Books settings"
            icon="book"
            sections=vec!["General", "Reading", "Keyboard shortcuts"]
            section=section
            on_close=on_close
            notice=saver.notice
        >
            <SaveProblems saver=saver/>
            {move || match section.get() {
                "Keyboard shortcuts" => view! {
                    <dl class="ui-shortcuts">
                        <dt>"Ctrl / Cmd K"</dt><dd>"Search all apps"</dd>
                        <dt>"\u{2192} / Space / Page Down"</dt><dd>"Next page (\u{2190} in right-to-left books)"</dd>
                        <dt>"\u{2190} / Shift Space / Page Up"</dt><dd>"Previous page"</dd>
                        <dt>"Ctrl / Cmd F"</dt><dd>"Find in PDF"</dd>
                        <dt>"Z"</dt><dd>"Focus mode (hide top bar while reading)"</dd>
                        <dt>"F"</dt><dd>"Full screen"</dd>
                        <dt>"Escape"</dt><dd>"Close menus, focus mode, reader, then details"</dd>
                        <dt>"Tab / Shift Tab"</dt><dd>"Move between controls"</dd>
                    </dl>
                }.into_any(),
                _ if saver.preferences.with(Option::is_none) => view! {
                    <p role="status">"Loading settings\u{2026}"</p>
                }.into_any(),
                "General" => {
                    let sort_options: Vec<(String, String)> = SORTS
                        .iter()
                        .map(|&(v, l)| (v.to_owned(), l.to_owned()))
                        .collect();
                    view! {
                        <section class="settings-group" aria-label="Library">
                            <h3>"Library"</h3>
                            <SettingRow
                                label="Books folder"
                                hint=format!("Books finds EPUB, PDF and comic files in Files \u{203a} {}. Its top folders are libraries.", saved_folder.get())
                                state=saver.state("booksFolder")
                                stacked=true
                            >
                                <form
                                    class="photos-folder-form"
                                    on:submit=move |ev| {
                                        ev.prevent_default();
                                        if let Some(parts) = books_folder_parts(&folder_value.get_untracked()) {
                                            let joined = parts.join("/");
                                            draft.set(None);
                                            set("booksFolder", json!(joined.clone()), format!("Books now come from {joined}"));
                                        }
                                    }
                                >
                                    <label>
                                        <span class="sr-only">"Books folder"</span>
                                        <input
                                            prop:value=move || folder_value.get()
                                            maxlength="500"
                                            on:input=move |ev| draft.set(Some(event_target_value(&ev)))
                                            aria-invalid=move || (!folder_valid.get()).to_string()
                                        />
                                    </label>
                                    <button
                                        class="secondary-button"
                                        disabled=move || !folder_valid.get() || folder_value.get() == saved_folder.get()
                                    >"Save"</button>
                                </form>
                                {move || (!folder_valid.get()).then(|| view! {
                                    <p class="field-error" role="alert">"Use folder names separated by /, like docs/books."</p>
                                })}
                            </SettingRow>
                            <SettingRow
                                label="Sort books by"
                                hint="Default order for grids and lists"
                                state=saver.state("booksSort")
                            >
                                <Select
                                    aria_label="Sort books by"
                                    options=sort_options
                                    value=sort_select_val
                                    on_change=Callback::new(move |next: String| {
                                        let lbl = SORTS.iter().find(|(v, _)| *v == next).map(|(_, l)| *l).unwrap_or("Recently read");
                                        set("booksSort", json!(next), format!("Books sorted by {}", lbl.to_lowercase()));
                                    })
                                />
                            </SettingRow>
                            <SettingRow
                                label="Library view"
                                hint="Show books as covers, a compact list, or grouped by series"
                                state=saver.state("booksCatalogView")
                                stacked=true
                            >
                                <Segmented
                                    label="Library view"
                                    value=text("booksCatalogView", "grid")
                                    options=vec![("grid", "Covers"), ("list", "List"), ("series", "Series")]
                                    on_change=Callback::new(move |next: String| {
                                        set("booksCatalogView", json!(next), "Library view changed".into());
                                    })
                                />
                            </SettingRow>
                        </section>
                    }.into_any()
                }
                _ => {
                    let size_options: Vec<(String, String)> = FONT_SIZES
                        .iter()
                        .map(|n| (n.to_string(), format!("{n} px")))
                        .collect();
                    view! {
                        <section class="settings-group" aria-label="Text">
                            <h3>"EPUB & text"</h3>
                            <SettingRow
                                label="Theme"
                                hint="Page colours while reading. Dar follows your appearance setting."
                                state=saver.state("booksTheme")
                                stacked=true
                            >
                                <Segmented
                                    label="Theme"
                                    value=text("booksTheme", "dar")
                                    options=vec![("dar", "Dar"), ("paper", "Paper"), ("sepia", "Sepia"), ("night", "Night")]
                                    on_change=Callback::new(move |next: String| set("booksTheme", json!(next), "Reading theme changed".into()))
                                />
                            </SettingRow>
                            <SettingRow
                                label="Font"
                                hint="Choose the book\u{2019}s own to keep the publisher\u{2019}s fonts"
                                state=saver.state("booksFont")
                                stacked=true
                            >
                                <Segmented
                                    label="Font"
                                    value=text("booksFont", "serif")
                                    options=vec![("serif", "Serif"), ("sans", "Sans serif"), ("book", "The book\u{2019}s own")]
                                    on_change=Callback::new(move |next: String| set("booksFont", json!(next), "Reading font changed".into()))
                                />
                            </SettingRow>
                            <SettingRow label="Text size" hint="" state=saver.state("booksFontSize")>
                                <Select
                                    aria_label="Text size"
                                    options=size_options
                                    value=size_select_val
                                    on_change=Callback::new(move |next: String| {
                                        if let Ok(n) = next.parse::<u32>() {
                                            set("booksFontSize", json!(n), format!("Text size set to {n} px"));
                                        }
                                    })
                                />
                            </SettingRow>
                            <SettingRow label="Line spacing" hint="" state=saver.state("booksLineHeight")>
                                <Segmented
                                    label="Line spacing"
                                    value=text("booksLineHeight", "normal")
                                    options=vec![("tight", "Tight"), ("normal", "Normal"), ("loose", "Loose")]
                                    on_change=Callback::new(move |next: String| set("booksLineHeight", json!(next), "Line spacing changed".into()))
                                />
                            </SettingRow>
                            <SettingRow
                                label="Layout"
                                hint="Turn pages, or scroll through each chapter"
                                state=saver.state("booksLayout")
                            >
                                <Segmented
                                    label="Layout"
                                    value=text("booksLayout", "pages")
                                    options=vec![("pages", "Pages"), ("scroll", "Scroll")]
                                    on_change=Callback::new(move |next: String| {
                                        let label = if next == "pages" { "Pages" } else { "Scroll" };
                                        set("booksLayout", json!(next), format!("Books open in {label} layout"));
                                    })
                                />
                            </SettingRow>
                            <SettingRow
                                label="Columns"
                                hint="Automatic uses two pages side by side on wide displays"
                                state=saver.state("booksColumns")
                            >
                                <Segmented
                                    label="Columns"
                                    value=text("booksColumns", "auto")
                                    options=vec![("auto", "Auto"), ("1", "1-page"), ("2", "2-page")]
                                    on_change=Callback::new(move |next: String| set("booksColumns", json!(next), "Column layout changed".into()))
                                />
                            </SettingRow>
                        </section>
                        <section class="settings-group" aria-label="PDF">
                            <h3>"PDF"</h3>
                            <SettingRow
                                label="Paper tint"
                                hint="Soften white PDF pages for dim rooms"
                                state=saver.state("booksPdfTint")
                            >
                                <Segmented
                                    label="Paper tint"
                                    value=text("booksPdfTint", "paper")
                                    options=vec![("paper", "Paper"), ("sepia", "Sepia"), ("night", "Night")]
                                    on_change=Callback::new(move |next: String| set("booksPdfTint", json!(next), "PDF paper tint changed".into()))
                                />
                            </SettingRow>
                        </section>
                        <section class="settings-group" aria-label="Comics">
                            <h3>"Comics & manga"</h3>
                            <SettingRow
                                label="Page layout"
                                hint="Single page, two-page spread, or continuous vertical strip"
                                state=saver.state("booksComicLayout")
                                stacked=true
                            >
                                <Segmented
                                    label="Page layout"
                                    value=text("booksComicLayout", "single")
                                    options=vec![("single", "Single"), ("spread", "Spread"), ("strip", "Strip")]
                                    on_change=Callback::new(move |next: String| set("booksComicLayout", json!(next), "Comic layout changed".into()))
                                />
                            </SettingRow>
                            <SettingRow
                                label="Reading direction"
                                hint="Automatic uses the comic\u{2019}s own setting (manga read right to left)"
                                state=saver.state("booksComicDirection")
                                stacked=true
                            >
                                <Segmented
                                    label="Reading direction"
                                    value=text("booksComicDirection", "auto")
                                    options=vec![("auto", "Automatic"), ("ltr", "Left to right"), ("rtl", "Right to left")]
                                    on_change=Callback::new(move |next: String| set("booksComicDirection", json!(next), "Comic direction changed".into()))
                                />
                            </SettingRow>
                        </section>
                    }.into_any()
                }
            }}
        </SettingsSheet>
    }
}
