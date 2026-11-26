//! Books Home view: Now Reading spotlight, compact Also Reading queue, horizontal scrollable
//! shelves (Recently added, Want to read, Series), and reusable BookCard.

use leptos::html;
use leptos::prelude::*;

use crate::books::cover::Cover;
use crate::books::model::{
    BookItem, BookLibrary, View, authors, book_format_label, group_series, percent, place,
    sort_books, time_left,
};
use crate::dates::edited;
use crate::icons::Icon;
use crate::ui::{ActionMenu, MenuAction};

#[component]
pub fn BookCard(
    book: BookItem,
    #[prop(into)] selected: Signal<bool>,
    #[prop(optional)] selectable: Option<Signal<bool>>,
    #[prop(optional)] checked: Option<Signal<bool>>,
    #[prop(optional)] on_toggle_check: Option<Callback<()>>,
    on_select: Callback<()>,
    actions: Callback<(), Vec<MenuAction>>,
) -> impl IntoView {
    let title = book.title.clone();
    let subtitle = if !book.error.is_empty() {
        match book.error.as_str() {
            "drm" => "DRM protected".into(),
            _ => "Cannot open".into(),
        }
    } else if !book.series.is_empty() {
        match book.series_index {
            Some(idx) => format!("{} \u{00b7} {idx}", book.series),
            None => book.series.clone(),
        }
    } else {
        authors(&book)
    };
    let menu_label = format!("Actions for {}", book.title);
    let check_label = format!("Select {}", book.title);

    let card_class = move || {
        let mut cls = String::from("books-card");
        if selected.get() {
            cls.push_str(" is-selected");
        }
        if checked.is_some_and(|c| c.get()) {
            cls.push_str(" is-checked");
        }
        if selectable.is_some_and(|s| s.get()) {
            cls.push_str(" is-selectable");
        }
        cls
    };

    view! {
        <div class=card_class>
            {on_toggle_check.map(|toggle| view! {
                <button
                    type="button"
                    class="books-card-check"
                    role="checkbox"
                    aria-checked=move || checked.is_some_and(|c| c.get()).to_string()
                    aria-label=check_label.clone()
                    on:click=move |ev| {
                        ev.stop_propagation();
                        let _ = toggle.try_run(());
                    }
                >
                    <Icon name="check"/>
                </button>
            })}
            <button
                type="button"
                class="books-card-open"
                aria-pressed=move || selected.get().to_string()
                on:click=move |_| {
                    if selectable.is_some_and(|s| s.get())
                        && let Some(toggle) = on_toggle_check
                    {
                        let _ = toggle.try_run(());
                    } else {
                        let _ = on_select.try_run(());
                    }
                }
            >
                <Cover book=book/>
                <strong>{title}</strong>
                <small>{subtitle}</small>
            </button>
            <ActionMenu label=menu_label class="books-card-menu" actions=actions>
                <Icon name="more"/>
            </ActionMenu>
        </div>
    }
}

#[component]
fn Shelf(
    #[prop(into)] title: String,
    hint: Option<String>,
    books: Vec<BookItem>,
    selected: RwSignal<Option<String>>,
    on_select: Callback<String>,
    on_all: Callback<()>,
    actions: Callback<BookItem, Vec<MenuAction>>,
) -> impl IntoView {
    let scroller = NodeRef::<html::Ul>::new();
    let scroll_by = move |dir: f64| {
        if let Some(el) = scroller.get_untracked() {
            let delta = (el.client_width() as f64 * 0.75).max(220.0) * dir;
            let opts = web_sys::ScrollToOptions::new();
            opts.set_left(delta);
            opts.set_behavior(web_sys::ScrollBehavior::Smooth);
            el.scroll_by_with_scroll_to_options(&opts);
        }
    };
    let show_arrows = books.len() > 4;

    view! {
        <section class="books-shelf-section" aria-label=title.clone()>
            <div class="books-shelf-head">
                <h2>{title.clone()}</h2>
                {hint.map(|h| view! { <small>{h}</small> })}
                <div class="books-shelf-controls">
                    {show_arrows.then(|| view! {
                        <span class="books-shelf-arrows">
                            <button
                                type="button"
                                class="icon-button books-shelf-arrow"
                                aria-label=format!("Scroll {title} left")
                                on:click=move |_| scroll_by(-1.0)
                            >
                                <Icon name="back"/>
                            </button>
                            <button
                                type="button"
                                class="icon-button books-shelf-arrow is-next"
                                aria-label=format!("Scroll {title} right")
                                on:click=move |_| scroll_by(1.0)
                            >
                                <Icon name="chevron"/>
                            </button>
                        </span>
                    })}
                    <button type="button" class="text-button" on:click=move |_| { on_all.try_run(()); }>"See all"</button>
                </div>
            </div>
            <ul node_ref=scroller class="books-shelf">
                {books.into_iter().map(|b| {
                    let bid = b.id.clone();
                    let bid_sel = bid.clone();
                    let bid_click = bid.clone();
                    let b_act = b.clone();
                    view! {
                        <li>
                            <BookCard
                                book=b
                                selected=Signal::derive(move || selected.get().as_deref() == Some(bid_sel.as_str()))
                                on_select=Callback::new(move |_| { on_select.try_run(bid_click.clone()); })
                                actions=Callback::new(move |_| actions.run(b_act.clone()))
                            />
                        </li>
                    }
                }).collect_view()}
            </ul>
        </section>
    }
}

#[component]
fn NowReadingSpotlight(
    book: BookItem,
    #[prop(into)] selected: Signal<bool>,
    on_open: Callback<()>,
    on_details: Callback<()>,
) -> impl IntoView {
    let progress = book.state.as_ref().map_or(0.0, |s| s.progress);
    let title = book.title.clone();
    let auth = authors(&book);
    let series_note = if !book.series.is_empty() {
        match book.series_index {
            Some(idx) => format!("{auth} \u{00b7} {}, Book {idx}", book.series),
            None => format!("{auth} \u{00b7} {}", book.series),
        }
    } else {
        auth
    };
    let place_str = place(&book);
    let est = time_left(&book, progress);
    let updated_text = book
        .state
        .as_ref()
        .filter(|s| s.updated_at > 0.0)
        .map(|s| edited(s.updated_at))
        .unwrap_or_default();
    let details_label = format!("Details for {}", book.title);
    let fmt = book_format_label(&book.format);

    view! {
        <div class=move || if selected.get() { "books-spotlight is-selected" } else { "books-spotlight" }>
            <button
                type="button"
                class="books-spotlight-cover"
                aria-label=details_label
                on:click=move |_| { on_details.try_run(()); }
            >
                <Cover book=book/>
            </button>
            <div class="books-spotlight-body">
                <div class="books-spotlight-meta">
                    <span class="books-live-dot" aria-hidden="true"></span>
                    <span>"Now reading"</span>
                    {(!updated_text.is_empty()).then(|| view! {
                        <span>{format!("\u{00b7} {updated_text}")}</span>
                    })}
                    <span class="books-spotlight-fmt">{fmt}</span>
                </div>
                <h3 class="books-spotlight-title">{title}</h3>
                <p class="books-spotlight-sub">{series_note}</p>
                <div class="books-spotlight-progress">
                    <progress max="1" value=progress.to_string() aria-label=format!("{} read", percent(progress))/>
                    <span class="books-spotlight-stat">
                        {match est {
                            Some(e) => format!("{place_str} \u{00b7} {e}"),
                            None => place_str,
                        }}
                    </span>
                </div>
                <div class="books-spotlight-actions">
                    <button
                        type="button"
                        class="primary-button"
                        on:click=move |_| { on_open.try_run(()); }
                    >
                        <Icon name="reading"/>"Continue reading"
                    </button>
                    <button
                        type="button"
                        class="secondary-button"
                        on:click=move |_| { on_details.try_run(()); }
                    >
                        "Details"
                    </button>
                </div>
            </div>
        </div>
    }
}

#[component]
fn AlsoReadingCard(
    book: BookItem,
    #[prop(into)] selected: Signal<bool>,
    on_open: Callback<()>,
    on_details: Callback<()>,
) -> impl IntoView {
    let progress = book.state.as_ref().map_or(0.0, |s| s.progress);
    let title = book.title.clone();
    let place_text = place(&book);
    let fmt = book_format_label(&book.format);
    let details_label = format!("Details for {}", book.title);

    view! {
        <div class=move || if selected.get() { "books-continue-card is-selected" } else { "books-continue-card" }>
            <button
                type="button"
                class="books-continue-cover"
                aria-label=details_label
                on:click=move |_| { on_details.try_run(()); }
            >
                <Cover book=book size="s"/>
            </button>
            <button
                type="button"
                class="books-continue-body"
                on:click=move |_| { on_open.try_run(()); }
            >
                <strong>{title}</strong>
                <span class="books-continue-place">{format!("{place_text} \u{00b7} {fmt}")}</span>
                <progress max="1" value=progress.to_string() aria-label=format!("{} read", percent(progress))/>
            </button>
        </div>
    }
}

#[component]
pub fn BooksHome(
    lib: BookLibrary,
    sort: String,
    selected: RwSignal<Option<String>>,
    on_select: Callback<String>,
    on_open: Callback<String>,
    on_choose: Callback<View>,
    on_open_series: Callback<String>,
    actions: Callback<BookItem, Vec<MenuAction>>,
) -> impl IntoView {
    let reading_books: Vec<BookItem> = lib
        .books
        .iter()
        .filter(|b| b.state.as_ref().is_some_and(|s| s.status == "reading"))
        .cloned()
        .collect();
    let mut continuing = sort_books(&reading_books, "recent");
    continuing.truncate(7);
    let featured = if !continuing.is_empty() {
        Some(continuing.remove(0))
    } else {
        None
    };

    let mut added = lib.books.clone();
    added.sort_by(|a, b| b.modified_at.total_cmp(&a.modified_at));
    added.truncate(16);

    let want_books: Vec<BookItem> = lib
        .books
        .iter()
        .filter(|b| b.state.as_ref().is_some_and(|s| s.status == "want"))
        .cloned()
        .collect();
    let wanted: Vec<BookItem> = sort_books(&want_books, &sort)
        .into_iter()
        .take(16)
        .collect();
    let series_groups = group_series(&lib.books);
    let path_hint = format!("in {}", lib.path);

    view! {
        <div class="books-scroll">
            {featured.map(|first| {
                let fid = first.id.clone();
                let fid_sel = fid.clone();
                let fid_open = fid.clone();
                let fid_det = fid.clone();
                view! {
                    <section class="books-continue-section" aria-label="Continue reading">
                        <div class="books-shelf-head">
                            <h2>"Continue reading"</h2>
                            <button type="button" class="text-button" on:click=move |_| { on_choose.try_run(View::Reading); }>"See all"</button>
                        </div>
                        <NowReadingSpotlight
                            book=first
                            selected=Signal::derive(move || selected.get().as_deref() == Some(fid_sel.as_str()))
                            on_open=Callback::new(move |_| { on_open.try_run(fid_open.clone()); })
                            on_details=Callback::new(move |_| { on_select.try_run(fid_det.clone()); })
                        />
                        {(!continuing.is_empty()).then(|| view! {
                            <div class="books-continue">
                                {continuing.into_iter().map(|b| {
                                    let bid = b.id.clone();
                                    let bid_sel = bid.clone();
                                    let bid_open = bid.clone();
                                    let bid_det = bid.clone();
                                    view! {
                                        <AlsoReadingCard
                                            book=b
                                            selected=Signal::derive(move || selected.get().as_deref() == Some(bid_sel.as_str()))
                                            on_open=Callback::new(move |_| { on_open.try_run(bid_open.clone()); })
                                            on_details=Callback::new(move |_| { on_select.try_run(bid_det.clone()); })
                                        />
                                    }
                                }).collect_view()}
                            </div>
                        })}
                    </section>
                }
            })}

            <Shelf
                title="Recently added"
                hint=Some(path_hint)
                books=added
                selected=selected
                on_select=on_select
                on_all=Callback::new(move |_| { on_choose.try_run(View::All); })
                actions=actions
            />

            {(!wanted.is_empty()).then(|| view! {
                <Shelf
                    title="Want to read"
                    hint=None
                    books=wanted
                    selected=selected
                    on_select=on_select
                    on_all=Callback::new(move |_| { on_choose.try_run(View::Want); })
                    actions=actions
                />
            })}

            {(!series_groups.is_empty()).then(|| view! {
                <section class="books-shelf-section" aria-label="Series">
                    <div class="books-shelf-head">
                        <h2>"Series"</h2>
                        <button type="button" class="text-button" on:click=move |_| { on_choose.try_run(View::Series); }>"See all"</button>
                    </div>
                    <div class="books-series-home">
                        {series_groups.into_iter().take(6).map(|grp| {
                            let sname = grp.name.clone();
                            let count = grp.books.len();
                            let cover_book = grp.books[0].clone();
                            view! {
                                <button
                                    type="button"
                                    class="books-series-chip-card"
                                    on:click=move |_| { on_open_series.try_run(sname.clone()); }
                                >
                                    <Cover book=cover_book size="s"/>
                                    <div class="books-series-chip-info">
                                        <strong>{grp.name}</strong>
                                        <small>{format!("{count} {} \u{00b7} {}", if count == 1 { "book" } else { "books" }, grp.status_summary)}</small>
                                    </div>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </section>
            })}
        </div>
    }
}
