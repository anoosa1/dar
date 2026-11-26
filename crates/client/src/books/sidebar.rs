//! Books sidebar: Add books CTA, primary views, Series & Highlights, Libraries,
//! background indexing indicator, and quiet Library Pulse summary card.

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::books::model::{ACCEPT, BookLibrary, VIEWS, View, group_series};
use crate::icons::Icon;
use crate::settings::AppSettingsButton;

#[component]
pub fn BooksSidebar(
    navigation_id: String,
    navigation_ref: NodeRef<html::Aside>,
    file_input: NodeRef<html::Input>,
    navigation: RwSignal<bool>,
    library: RwSignal<Option<BookLibrary>>,
    view: RwSignal<View>,
    highlights_count: Signal<usize>,
    on_choose: Callback<View>,
    on_trigger_upload: Callback<()>,
    on_upload_files: Callback<web_sys::FileList>,
    on_settings: Callback<()>,
) -> impl IntoView {
    let counts = Memo::new(move |_| {
        library.with(|l| {
            let Some(lib) = l else {
                return (0usize, 0usize, 0usize, 0usize, 0usize);
            };
            let reading = lib
                .books
                .iter()
                .filter(|b| b.state.as_ref().is_some_and(|s| s.status == "reading"))
                .count();
            let want = lib
                .books
                .iter()
                .filter(|b| b.state.as_ref().is_some_and(|s| s.status == "want"))
                .count();
            let finished = lib
                .books
                .iter()
                .filter(|b| b.state.as_ref().is_some_and(|s| s.status == "finished"))
                .count();
            let series = group_series(&lib.books).len();
            (reading, want, finished, lib.books.len(), series)
        })
    });

    view! {
        <aside
            id=navigation_id
            node_ref=navigation_ref
            class=move || if navigation.get() { "sidebar books-sidebar is-open" } else { "sidebar books-sidebar" }
            aria-label="Books navigation"
        >
            <button
                type="button"
                class="new-button ui-new"
                disabled=move || library.with(Option::is_none)
                on:click=move |_| { on_trigger_upload.try_run(()); }
            >
                <Icon name="plus"/>"Add books"
            </button>
            <input
                node_ref=file_input
                type="file"
                multiple
                hidden
                accept=ACCEPT
                aria-label="Add books"
                on:change=move |ev| {
                    if let Some(target) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                        if let Some(files) = target.files() {
                            on_upload_files.try_run(files);
                        }
                        target.set_value("");
                    }
                }
            />
            <div class="books-side-scroll">
                <nav aria-label="Books views">
                    {VIEWS.iter().map(|&(id, label, icon)| {
                        let target_view = View::from_list_id(id).unwrap_or(View::Home);
                        let tv_class = target_view.clone();
                        let tv_curr = target_view.clone();
                        let tv_click = target_view;
                        let count = move || {
                            let (reading, want, finished, all, series) = counts.get();
                            match id {
                                "reading" => reading,
                                "want" => want,
                                "finished" => finished,
                                "all" => all,
                                "series" => series,
                                "highlights" => highlights_count.get(),
                                _ => 0,
                            }
                        };
                        view! {
                            <button
                                class=move || if view.get() == tv_class { "nav-item active" } else { "nav-item" }
                                aria-current=move || (view.get() == tv_curr).then_some("page")
                                on:click=move |_| { on_choose.try_run(tv_click.clone()); }
                            >
                                <Icon name=icon/>
                                <span class="books-row-title">{label}</span>
                                {move || {
                                    let c = count();
                                    (c > 0).then(|| view! { <span class="books-count">{c}</span> })
                                }}
                            </button>
                        }
                    }).collect_view()}
                </nav>
                <div class="sidebar-heading books-heading"><h2>"Libraries"</h2></div>
                <nav aria-label="Libraries" class="books-libraries">
                    {move || {
                        let Some(lib) = library.get() else {
                            return ().into_any();
                        };
                        if lib.libraries.is_empty() {
                            view! { <p class="books-hint">{format!("Folders in Files \u{203a} {} become libraries.", lib.path)}</p> }.into_any()
                        } else {
                            lib.libraries.into_iter().map(|folder| {
                                let fid1 = folder.id.clone();
                                let fid2 = folder.id.clone();
                                let fid3 = folder.id.clone();
                                let count = folder.count;
                                view! {
                                    <button
                                        class=move || if view.get() == View::Library(fid1.clone()) { "nav-item active" } else { "nav-item" }
                                        aria-current=move || (view.get() == View::Library(fid2.clone())).then_some("page")
                                        on:click=move |_| { on_choose.try_run(View::Library(fid3.clone())); }
                                    >
                                        <Icon name="folder"/>
                                        <span class="books-row-title">{folder.name}</span>
                                        {(count > 0).then(|| view! { <span class="books-count">{count}</span> })}
                                    </button>
                                }
                            }).collect_view().into_any()
                        }
                    }}
                </nav>
            </div>
            {move || {
                let n = library.with(|l| l.as_ref().map_or(0, |l| l.indexing));
                (n > 0).then(|| view! {
                    <p class="books-indexing" role="status">
                        <span class="books-spinner" aria-hidden="true"></span>
                        {format!("Reading {n} new {}\u{2026}", if n == 1 { "book" } else { "books" })}
                    </p>
                })
            }}
            {move || {
                let (reading, want, finished, all, _) = counts.get();
                (all > 0).then(|| {
                    let denom = (all as f64).max(1.0);
                    let r_pct = ((reading as f64 / denom) * 100.0).round() as u32;
                    let f_pct = ((finished as f64 / denom) * 100.0).round() as u32;
                    let w_pct = ((want as f64 / denom) * 100.0).round() as u32;
                    let bar_ref = NodeRef::<html::Div>::new();
                    Effect::new(move |_| {
                        if let Some(el) = bar_ref.get() {
                            let s = web_sys::HtmlElement::style(&el);
                            let _ = s.set_property("--pulse-read", &format!("{r_pct}%"));
                            let _ = s.set_property("--pulse-done", &format!("{f_pct}%"));
                            let _ = s.set_property("--pulse-want", &format!("{w_pct}%"));
                        }
                    });
                    view! {
                        <div class="books-pulse" aria-label="Library overview">
                            <div class="books-pulse-top">
                                <strong>"Library"</strong>
                                <span>{format!("{all} {}", if all == 1 { "book" } else { "books" })}</span>
                            </div>
                            <div node_ref=bar_ref class="books-pulse-bar" aria-hidden="true">
                                <span class="is-read"></span>
                                <span class="is-done"></span>
                                <span class="is-want"></span>
                            </div>
                            <div class="books-pulse-legend">
                                {(reading > 0).then(|| view! { <span><i class="is-read"></i>{format!("{reading} reading")}</span> })}
                                {(finished > 0).then(|| view! { <span><i class="is-done"></i>{format!("{finished} finished")}</span> })}
                                {(want > 0 && reading == 0).then(|| view! { <span><i class="is-want"></i>{format!("{want} want")}</span> })}
                            </div>
                        </div>
                    }
                })
            }}
            <AppSettingsButton app="books" on_click=on_settings/>
        </aside>
    }
}
