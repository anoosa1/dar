//! Books catalog views: Covers grid, Compact table list, Series stacks, Highlights browser,
//! lightweight format/status filter chips, and bulk selection actions.

use std::collections::HashSet;

use leptos::prelude::*;

use crate::books::cover::Cover;
use crate::books::highlights::HighlightNoteSummary;
use crate::books::home::BookCard;
use crate::books::model::{
    BookItem, CatalogFilter, SeriesGroup, authors, book_format_label, filter_books, group_series,
    percent, place, sort_books,
};
use crate::icons::Icon;
use crate::ui::{ActionMenu, MenuAction};

#[component]
pub fn BooksFilterBar(
    filter: RwSignal<CatalogFilter>,
    selecting: RwSignal<bool>,
    checked: RwSignal<HashSet<String>>,
    total_visible: Signal<usize>,
    on_bulk_status: Callback<&'static str>,
    on_bulk_trash: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="books-subbar">
            {move || {
                let n = checked.with(HashSet::len);
                if selecting.get() || n > 0 {
                    return view! {
                        <div class="books-bulk-bar" role="region" aria-label="Bulk selection actions">
                            <strong>{if n == 0 { "Select books".into() } else { format!("{n} selected") }}</strong>
                            <button
                                type="button"
                                class="books-bulk-btn"
                                disabled=n == 0
                                on:click=move |_| { on_bulk_status.try_run("want"); }
                            >
                                <Icon name="bookmark"/>"Want to read"
                            </button>
                            <button
                                type="button"
                                class="books-bulk-btn"
                                disabled=n == 0
                                on:click=move |_| { on_bulk_status.try_run("finished"); }
                            >
                                <Icon name="check"/>"Finished"
                            </button>
                            <button
                                type="button"
                                class="books-bulk-btn is-danger"
                                disabled=n == 0
                                on:click=move |_| { on_bulk_trash.try_run(()); }
                            >
                                <Icon name="trash"/>"Delete"
                            </button>
                            <button
                                type="button"
                                class="books-bulk-btn is-clear"
                                on:click=move |_| {
                                    checked.set(HashSet::new());
                                    selecting.set(false);
                                }
                            >
                                "Done"
                            </button>
                        </div>
                    }
                    .into_any();
                }

                let cur = filter.get();
                let active_custom = match &cur {
                    CatalogFilter::Author(a) => Some(format!("Author: {a}")),
                    CatalogFilter::Tag(t) => Some(format!("Tag: {t}")),
                    CatalogFilter::SeriesName(s) => Some(format!("Series: {s}")),
                    _ => None,
                };
                let chips: &[(&str, CatalogFilter)] = &[
                    ("All", CatalogFilter::All),
                    ("EPUB", CatalogFilter::Epub),
                    ("PDF", CatalogFilter::Pdf),
                    ("Comics", CatalogFilter::Comic),
                    ("In progress", CatalogFilter::Reading),
                    ("Finished", CatalogFilter::Finished),
                ];

                view! {
                    <div class="books-filter-chips" role="group" aria-label="Filter books">
                        {chips.iter().map(|(label, target)| {
                            let t_cmp = target.clone();
                            let t_click = target.clone();
                            view! {
                                <button
                                    type="button"
                                    class=move || if filter.get() == t_cmp { "books-chip is-active" } else { "books-chip" }
                                    aria-pressed=move || (filter.get() == target.clone()).to_string()
                                    on:click=move |_| filter.set(t_click.clone())
                                >
                                    {*label}
                                </button>
                            }
                        }).collect_view()}
                        {active_custom.map(|label| view! {
                            <button
                                type="button"
                                class="books-chip is-active is-custom"
                                on:click=move |_| filter.set(CatalogFilter::All)
                            >
                                {label}
                                <Icon name="close"/>
                            </button>
                        })}
                        {(total_visible.get() > 0).then(|| view! {
                            <button
                                type="button"
                                class="books-chip books-select-toggle"
                                on:click=move |_| selecting.set(true)
                            >
                                <Icon name="checks"/>"Select"
                            </button>
                        })}
                    </div>
                }
                .into_any()
            }}
        </div>
    }
}

#[component]
pub fn BooksCatalogGrid(
    items: Vec<BookItem>,
    sort: String,
    filter: CatalogFilter,
    catalog_view: String,
    grid_label: String,
    truncated: bool,
    selected: RwSignal<Option<String>>,
    selecting: RwSignal<bool>,
    checked: RwSignal<HashSet<String>>,
    on_select: Callback<String>,
    on_open_series: Callback<String>,
    on_clear_filter: Callback<()>,
    actions: Callback<BookItem, Vec<MenuAction>>,
) -> impl IntoView {
    let filtered = filter_books(&items, &filter);
    if filtered.is_empty() && catalog_view != "series" {
        return view! {
            <div class="books-empty">
                <span class="empty-icon"><Icon name="book"/></span>
                <h2>"No matching books"</h2>
                <p>"No books in this view match the current filter."</p>
                <button type="button" class="secondary-button" on:click=move |_| { on_clear_filter.try_run(()); }>
                    "Show all"
                </button>
            </div>
        }
        .into_any();
    }

    if catalog_view == "series" {
        let groups = group_series(&filtered);
        if groups.is_empty() {
            return view! {
                <div class="books-empty">
                    <span class="empty-icon"><Icon name="stack"/></span>
                    <h2>"No series found"</h2>
                    <p>"Books with series metadata (EPUB Calibre series or ComicInfo.xml) group together automatically here."</p>
                </div>
            }
            .into_any();
        }
        return view! {
            <div class="books-scroll">
                <div class="books-series-grid" aria-label="Series">
                    {groups.into_iter().map(|grp| view! {
                        <SeriesStackCard group=grp on_open=on_open_series/>
                    }).collect_view()}
                </div>
            </div>
        }
        .into_any();
    }

    let ordered = sort_books(&filtered, &sort);

    if catalog_view == "list" {
        return view! {
            <div class="books-scroll">
                <div class="books-list-table" role="list" aria-label=grid_label>
                    <div class="books-list-head" aria-hidden="true">
                        <span></span>
                        <span></span>
                        <span>"Title"</span>
                        <span>"Author / Series"</span>
                        <span>"Format"</span>
                        <span class="is-right">"Progress"</span>
                        <span></span>
                    </div>
                    {ordered.into_iter().map(|b| {
                        let bid = b.id.clone();
                        let bid_sel = bid.clone();
                        let bid_chk = bid.clone();
                        let bid_chk_btn = bid.clone();
                        let bid_click = bid.clone();
                        let b_act = b.clone();
                        let title = b.title.clone();
                        let auth = authors(&b);
                        let series_sub = if !b.series.is_empty() {
                            match b.series_index {
                                Some(i) => format!("{} \u{00b7} #{i}", b.series),
                                None => b.series.clone(),
                            }
                        } else {
                            String::new()
                        };
                        let fmt = book_format_label(&b.format);
                        let pages_meta = if b.pages > 0 {
                            format!("{} pages", b.pages)
                        } else {
                            String::new()
                        };
                        let status_text = if !b.error.is_empty() {
                            "Cannot open".into()
                        } else if b.state.as_ref().is_some_and(|s| s.status == "finished") {
                            "Finished".into()
                        } else if b.state.as_ref().is_some_and(|s| s.status == "reading") {
                            place(&b)
                        } else if b.state.as_ref().is_some_and(|s| s.status == "want") {
                            "Want to read".into()
                        } else {
                            "Unread".into()
                        };
                        let is_done = b.state.as_ref().is_some_and(|s| s.status == "finished");
                        let is_err = !b.error.is_empty();
                        let menu_label = format!("Actions for {}", b.title);
                        let check_label = format!("Select {}", b.title);

                        view! {
                            <div
                                role="listitem"
                                class=move || {
                                    let mut c = String::from("books-list-row");
                                    if selected.get().as_deref() == Some(bid_sel.as_str()) {
                                        c.push_str(" is-selected");
                                    }
                                    if checked.with(|set| set.contains(&bid_chk)) {
                                        c.push_str(" is-checked");
                                    }
                                    c
                                }
                            >
                                <input
                                    type="checkbox"
                                    class="books-row-checkbox"
                                    aria-label=check_label
                                    prop:checked=move || checked.with(|set| set.contains(&bid_chk_btn))
                                    on:change=move |ev| {
                                        let on = event_target_checked(&ev);
                                        let id = bid.clone();
                                        checked.update(|set| {
                                            if on {
                                                set.insert(id);
                                            } else {
                                                set.remove(&id);
                                            }
                                        });
                                        if on {
                                            selecting.set(true);
                                        }
                                    }
                                />
                                <button
                                    type="button"
                                    class="books-list-row-btn"
                                    on:click=move |_| { on_select.try_run(bid_click.clone()); }
                                >
                                    <span class="books-list-thumb"><Cover book=b size="s"/></span>
                                    <span class="books-list-col">
                                        <strong>{title}</strong>
                                        {(!pages_meta.is_empty()).then(|| view! { <small>{pages_meta}</small> })}
                                    </span>
                                    <span class="books-list-col">
                                        <span>{auth}</span>
                                        {(!series_sub.is_empty()).then(|| view! { <small>{series_sub}</small> })}
                                    </span>
                                    <span class="books-fmt-pill">{fmt}</span>
                                    <span class=if is_err {
                                        "books-list-prog is-error"
                                    } else if is_done {
                                        "books-list-prog is-done"
                                    } else {
                                        "books-list-prog"
                                    }>
                                        {status_text}
                                    </span>
                                </button>
                                <ActionMenu label=menu_label class="books-list-menu" actions=Callback::new(move |_| actions.run(b_act.clone()))>
                                    <Icon name="more"/>
                                </ActionMenu>
                            </div>
                        }
                    }).collect_view()}
                </div>
                {truncated.then(|| view! { <p class="books-hint">"This folder is very large; only the first books are shown."</p> })}
            </div>
        }
        .into_any();
    }

    view! {
        <div class="books-scroll">
            <ul class="books-grid" aria-label=grid_label>
                {ordered.into_iter().map(|b| {
                    let bid = b.id.clone();
                    let bid_sel = bid.clone();
                    let bid_chk = bid.clone();
                    let bid_tog = bid.clone();
                    let bid_click = bid.clone();
                    let b_act = b.clone();
                    view! {
                        <li>
                            <BookCard
                                book=b
                                selected=Signal::derive(move || selected.get().as_deref() == Some(bid_sel.as_str()))
                                selectable=Signal::from(selecting)
                                checked=Signal::derive(move || checked.with(|set| set.contains(&bid_chk)))
                                on_toggle_check=Callback::new(move |_| {
                                    let id = bid_tog.clone();
                                    checked.update(|set| {
                                        if !set.remove(&id) {
                                            set.insert(id);
                                        }
                                    });
                                    if checked.with_untracked(|s| !s.is_empty()) {
                                        selecting.set(true);
                                    }
                                })
                                on_select=Callback::new(move |_| { on_select.try_run(bid_click.clone()); })
                                actions=Callback::new(move |_| actions.run(b_act.clone()))
                            />
                        </li>
                    }
                }).collect_view()}
            </ul>
            {truncated.then(|| view! { <p class="books-hint">"This folder is very large; only the first books are shown."</p> })}
        </div>
    }
    .into_any()
}

#[component]
fn SeriesStackCard(group: SeriesGroup, on_open: Callback<String>) -> impl IntoView {
    let name = group.name.clone();
    let count = group.books.len();
    let front = group.books[0].clone();
    let back1 = group.books.get(1).cloned();
    let back2 = group.books.get(2).cloned();
    let prog = group.progress;

    view! {
        <button
            type="button"
            class="books-series-card"
            on:click=move |_| { on_open.try_run(name.clone()); }
        >
            <div class="books-series-fan" aria-hidden="true">
                {back2.map(|b| view! { <span class="books-fan-slot is-back2"><Cover book=b size="s"/></span> })}
                {back1.map(|b| view! { <span class="books-fan-slot is-back1"><Cover book=b size="s"/></span> })}
                <span class="books-fan-slot is-front"><Cover book=front size="m"/></span>
            </div>
            <div class="books-series-meta">
                <strong>{group.name}</strong>
                <small>{format!("{} \u{00b7} {count} {}", group.authors, if count == 1 { "book" } else { "books" })}</small>
                <progress max="1" value=prog.to_string() aria-label=format!("{} complete", percent(prog))/>
                <small class="books-series-status">{group.status_summary}</small>
            </div>
        </button>
    }
}

#[component]
pub fn BooksHighlightsView(
    summaries: Signal<Vec<HighlightNoteSummary>>,
    loading: Signal<bool>,
    on_open_page: Callback<(String, usize)>,
) -> impl IntoView {
    view! {
        <div class="books-scroll">
            {move || {
                if loading.get() {
                    return view! { <p class="books-empty" role="status">"Loading highlights\u{2026}"</p> }.into_any();
                }
                let list = summaries.get();
                if list.is_empty() {
                    return view! {
                        <div class="books-empty">
                            <span class="empty-icon"><Icon name="quote"/></span>
                            <h2>"No highlights yet"</h2>
                            <p>"Select text while reading a PDF and choose Highlight. Your quotes are saved to Notes \u{203a} Highlights and appear here."</p>
                        </div>
                    }
                    .into_any();
                }
                view! {
                    <div class="books-highlights-list">
                        {list.into_iter().map(|group| {
                            let bid = group.book_id.clone();
                            let note_href = format!("/notes?note={}", group.note_id);
                            let count = group.highlights.len();
                            view! {
                                <section class="books-hl-group">
                                    <div class="books-hl-group-head">
                                        <div>
                                            <h2>{group.book_title}</h2>
                                            <small>{format!("{count} {}", if count == 1 { "highlight" } else { "highlights" })}</small>
                                        </div>
                                        <a class="text-button" href=note_href>"Open in Notes"</a>
                                    </div>
                                    <div class="books-hl-cards">
                                        {group.highlights.into_iter().map(|hl| {
                                            let bid_c = bid.clone();
                                            let page = hl.page;
                                            let quote = if hl.quote.is_empty() {
                                                format!("Highlighted passage on page {}", page + 1)
                                            } else {
                                                hl.quote
                                            };
                                            view! {
                                                <button
                                                    type="button"
                                                    class="books-hl-card"
                                                    disabled=bid_c.is_empty()
                                                    on:click=move |_| {
                                                        if !bid_c.is_empty() {
                                                            let _ = on_open_page.try_run((bid_c.clone(), page));
                                                        }
                                                    }
                                                >
                                                    <blockquote>{quote}</blockquote>
                                                    <span class="books-hl-card-foot">
                                                        <span>{format!("Page {}", page + 1)}</span>
                                                        {(!bid.is_empty()).then(|| view! { <span class="books-hl-jump">"Open page"</span> })}
                                                    </span>
                                                </button>
                                            }
                                        }).collect_view()}
                                    </div>
                                </section>
                            }
                        }).collect_view()}
                    </div>
                }
                .into_any()
            }}
        </div>
    }
}
