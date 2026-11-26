//! Book details right sheet: cover, progress, single-row quick actions, clamped description,
//! interactive series strip, Highlights-to-Notes bridge, metadata facts, and clickable tags.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::api::format_bytes;
use crate::books::cover::Cover;
use crate::books::highlights::{self, BookHighlightsNote};
use crate::books::model::{
    BookItem, BookLibrary, authors, book_format_label, language_display_name, locale_cmp, percent,
    progress_caption, status_label,
};
use crate::dates::edited;
use crate::icons::Icon;
use crate::ui::{ActionMenu, MenuAction};

#[component]
pub fn BookDetails(
    book_id: String,
    library: RwSignal<Option<BookLibrary>>,
    on_close: Callback<()>,
    on_read: Callback<()>,
    on_open_page: Callback<usize>,
    on_status: Callback<&'static str>,
    on_select: Callback<String>,
    on_filter_author: Callback<String>,
    on_filter_tag: Callback<String>,
    on_filter_series: Callback<String>,
    actions: Callback<(), Vec<MenuAction>>,
) -> impl IntoView {
    let more = RwSignal::new(false);
    let hl_note: RwSignal<Option<BookHighlightsNote>> = RwSignal::new(None);
    let root = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        if let Some(node) = root.get() {
            let opts = web_sys::FocusOptions::new();
            opts.set_prevent_scroll(true);
            let _ = node.focus_with_options(&opts);
        }
    });

    let book_id = StoredValue::new(book_id);
    let current_book = Memo::new(move |_| {
        let id = book_id.get_value();
        library.with(|l| {
            l.as_ref()
                .and_then(|l| l.books.iter().find(|b| b.id == id).cloned())
        })
    });

    // Load saved Highlights note for PDFs so Details bridges directly into Notes.
    Effect::new(move |_| {
        let Some(book) = current_book.get() else {
            return;
        };
        if book.format != "pdf" {
            hl_note.set(None);
            return;
        }
        let bid = book.id.clone();
        let title = book.title.clone();
        spawn_local(async move {
            let loaded = highlights::load_detailed(&bid, &title).await;
            let _ = hl_note.try_set(Some(loaded));
        });
    });

    let on_keydown = move |event: web_sys::KeyboardEvent| {
        if event.key() == "Escape" {
            let in_menu = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .and_then(|el| el.closest("[role=menu]").ok().flatten())
                .is_some();
            if !in_menu {
                event.prevent_default();
                let _ = on_close.try_run(());
            }
        }
    };

    view! {
        <div class="books-detail" node_ref=root tabindex="-1" on:keydown=on_keydown>
            {move || {
                let Some(book) = current_book.get() else {
                    return ().into_any();
                };
                let series: Vec<BookItem> = if !book.series.is_empty() {
                    let mut list: Vec<BookItem> = library.with(|l| {
                        l.as_ref()
                            .map(|l| l.books.iter().filter(|b| b.series == book.series).cloned().collect())
                            .unwrap_or_default()
                    });
                    list.sort_by(|a, b| {
                        a.series_index
                            .unwrap_or(1e9)
                            .total_cmp(&b.series_index.unwrap_or(1e9))
                            .then_with(|| locale_cmp(&a.title, &b.title))
                    });
                    list
                } else {
                    Vec::new()
                };
                let progress = book.state.as_ref().map_or(0.0, |s| s.progress);
                let status = book.state.as_ref().map(|s| s.status.as_str()).unwrap_or("");
                let has_position = book.state.as_ref().and_then(|s| s.position.as_ref()).is_some();
                let problem = match book.error.as_str() {
                    "drm" => "This book is protected with DRM by the store it came from, so Dar cannot open it. Download it to read it in that store\u{2019}s app.".to_owned(),
                    "unsupported" => "This archive type could not be opened on this server. Download it to read it elsewhere.".to_owned(),
                    "damaged" => format!("This file could not be read. It may be damaged or not a real {} file.", book_format_label(&book.format)),
                    _ => String::new(),
                };
                let series_line = if !book.series.is_empty() {
                    let mut s = book.series.clone();
                    if let Some(idx) = book.series_index {
                        s.push_str(&format!(" \u{00b7} Book {idx}"));
                    }
                    if series.len() > 1 {
                        s.push_str(&format!(" of {}", series.len()));
                    }
                    Some(s)
                } else {
                    None
                };
                let series_name_click = book.series.clone();
                let status_icon = match status {
                    "finished" => "check",
                    "want" => "bookmark",
                    _ => "reading",
                };
                let is_want = status == "want";
                let is_finished = status == "finished";
                let is_reading = status == "reading";
                let read_label = if is_finished {
                    "Read again"
                } else if has_position {
                    "Continue reading"
                } else {
                    "Read"
                };
                let want_next: &'static str = if is_want { "" } else { "want" };
                let fin_next: &'static str = if is_finished { "" } else { "finished" };
                let fin_label = if is_finished { "Mark as unread" } else { "Mark as finished" };
                let more_actions_label = format!("More actions for {}", book.title);
                let desc = book.description.clone();
                let desc_long = desc.chars().count() > 170;
                let desc_lines: Vec<String> = desc.split('\n').filter(|l| !l.trim().is_empty()).map(str::to_owned).collect();
                let current_id = book.id.clone();
                let first_author = book.authors.first().cloned();
                let auth_display = authors(&book);
                let prog_text = progress_caption(&book);

                view! {
                    <div class="books-detail-head">
                        <button class="icon-button books-detail-back" aria-label="Close details" title="Close (Escape)" on:click=move |_| { on_close.try_run(()); }><Icon name="back"/></button>
                        <h2>"Details"</h2>
                        <ActionMenu label=more_actions_label actions=actions><Icon name="more"/></ActionMenu>
                        <button class="icon-button books-detail-close" aria-label="Close details" on:click=move |_| { on_close.try_run(()); }><Icon name="close"/></button>
                    </div>
                    <div class="books-detail-body">
                        <div class="books-hero">
                            <Cover book=book.clone() size="l"/>
                            <div class="books-hero-meta">
                                <h3>{book.title.clone()}</h3>
                                {match first_author {
                                    Some(a) => view! {
                                        <button
                                            type="button"
                                            class="books-author-link"
                                            title="Filter library by this author"
                                            on:click=move |_| { on_filter_author.try_run(a.clone()); }
                                        >
                                            {auth_display}
                                        </button>
                                    }.into_any(),
                                    None => view! { <p class="books-hero-sub">{auth_display}</p> }.into_any(),
                                }}
                                {series_line.map(|s| {
                                    let sn = series_name_click.clone();
                                    view! {
                                        <button
                                            type="button"
                                            class="books-series-link"
                                            on:click=move |_| { on_filter_series.try_run(sn.clone()); }
                                        >
                                            {s}
                                        </button>
                                    }
                                })}
                                {(!status.is_empty()).then(|| view! {
                                    <p class="books-status-chip"><Icon name=status_icon/>{status_label(status)}</p>
                                })}
                                {is_reading.then(|| view! {
                                    <progress max="1" value=progress.to_string() aria-label=format!("{} read", percent(progress))/>
                                    <small>{prog_text}</small>
                                })}
                            </div>
                        </div>
                        {if !problem.is_empty() {
                            view! { <p class="error" role="alert">{problem}</p> }.into_any()
                        } else {
                            view! {
                                <div class="books-quick">
                                    <button class="primary-button" on:click=move |_| { on_read.try_run(()); }><Icon name="reading"/>{read_label}</button>
                                    <button
                                        class=if is_want { "icon-button books-round is-on" } else { "icon-button books-round" }
                                        aria-pressed=is_want.to_string()
                                        aria-label="Want to read"
                                        title="Want to read"
                                        on:click=move |_| { on_status.try_run(want_next); }
                                    ><Icon name="bookmark"/></button>
                                    <button
                                        class=if is_finished { "icon-button books-round is-on" } else { "icon-button books-round" }
                                        aria-pressed=is_finished.to_string()
                                        aria-label=fin_label
                                        title=fin_label
                                        on:click=move |_| { on_status.try_run(fin_next); }
                                    ><Icon name="check"/></button>
                                    <a
                                        class="icon-button books-round"
                                        href=format!("/api/v1/files/{}/content", book.id)
                                        download=""
                                        aria-label="Download"
                                        title="Download"
                                    ><Icon name="download"/></a>
                                </div>
                            }.into_any()
                        }}
                        {(!desc_lines.is_empty()).then(|| view! {
                            <div class=move || if more.get() || !desc_long { "books-description is-open" } else { "books-description is-clamped" }>
                                <div class="books-description-text">
                                    {desc_lines.into_iter().map(|line| view! { <p>{line}</p> }).collect_view()}
                                </div>
                                {desc_long.then(|| view! {
                                    <button
                                        type="button"
                                        class="text-button books-desc-toggle"
                                        aria-expanded=move || more.get().to_string()
                                        on:click=move |_| more.update(|m| *m = !*m)
                                    >
                                        {move || if more.get() { "Show less" } else { "Show more" }}
                                    </button>
                                })}
                            </div>
                        })}
                        {(series.len() > 1).then(|| {
                            let sname = book.series.clone();
                            let count = series.len();
                            view! {
                                <section aria-label=format!("{sname} series") class="books-series-strip">
                                    <div class="books-sec-head">
                                        <h4>{sname.clone()}</h4>
                                        <span>{format!("{count} books")}</span>
                                    </div>
                                    <div class="books-series-thumbs">
                                        {series.into_iter().map(|b| {
                                            let is_cur = b.id == current_id;
                                            let bid = b.id.clone();
                                            let lbl = match b.series_index {
                                                Some(idx) => format!("{}, book {idx}", b.title),
                                                None => b.title.clone(),
                                            };
                                            view! {
                                                <button
                                                    type="button"
                                                    aria-current=is_cur.then_some("true")
                                                    aria-label=lbl
                                                    on:click=move |_| { on_select.try_run(bid.clone()); }
                                                >
                                                    <Cover book=b size="s"/>
                                                </button>
                                            }
                                        }).collect_view()}
                                    </div>
                                </section>
                            }
                        })}
                        {move || {
                            let note = hl_note.get()?;
                            if note.highlights.is_empty() {
                                return None;
                            }
                            let count = note.highlights.len();
                            let latest = note.highlights.last()?.clone();
                            let page = latest.page;
                            let quote = if latest.quote.is_empty() {
                                format!("Highlighted passage on page {}", page + 1)
                            } else {
                                latest.quote
                            };
                            let notes_link = note.note_id.map(|nid| format!("/notes?note={nid}"));
                            Some(view! {
                                <section class="books-hl-bridge" aria-label="Highlights and notes">
                                    <div class="books-sec-head">
                                        <h4>"Highlights & notes"</h4>
                                        <span>{format!("{count} saved")}</span>
                                    </div>
                                    <button
                                        type="button"
                                        class="books-hl-preview"
                                        on:click=move |_| { on_open_page.try_run(page); }
                                    >
                                        <blockquote>{quote}</blockquote>
                                    </button>
                                    <div class="books-hl-bridge-foot">
                                        <span>{format!("Page {} \u{00b7} {}", page + 1, note.note_name)}</span>
                                        {notes_link.map(|href| view! { <a href=href>"Open in Notes"</a> })}
                                    </div>
                                </section>
                            })
                        }}
                        <div class="card books-facts">
                            <div>
                                <Icon name="file"/>
                                <strong>{format!("{} \u{00b7} {}", book_format_label(&book.format), format_bytes(book.size))}</strong>
                                {(book.pages > 0).then(|| view! {
                                    <span>{if book.format == "epub" { format!("\u{2248} {} pages", book.pages) } else { format!("{} pages", book.pages) }}</span>
                                })}
                            </div>
                            {(!book.published.is_empty() || !book.publisher.is_empty()).then(|| view! {
                                <div>
                                    <Icon name="calendar"/>
                                    <strong>{if !book.published.is_empty() { format!("Published {}", book.published) } else { "Publisher".into() }}</strong>
                                    <span>{book.publisher.clone()}</span>
                                </div>
                            })}
                            {(!book.language.is_empty()).then(|| view! {
                                <div>
                                    <Icon name="globe"/>
                                    <strong>{language_display_name(&book.language)}</strong>
                                </div>
                            })}
                            {book.state.as_ref().filter(|s| s.position.is_some()).map(|st| view! {
                                <div>
                                    <Icon name="clock"/>
                                    <strong>{format!("Last read {}", edited(st.updated_at))}</strong>
                                    {(!st.device.is_empty()).then(|| view! { <span>{st.device.clone()}</span> })}
                                </div>
                            })}
                            <div>
                                <Icon name="folder"/>
                                <strong class="books-path">{book.path.clone()}</strong>
                                <a href=format!("/?folder={}&file={}", book.parent_id, book.id)>"Show in Files"</a>
                            </div>
                        </div>
                        {(!book.subjects.is_empty()).then(|| view! {
                            <div class="books-tags">
                                {book.subjects.iter().map(|s| {
                                    let tag = s.clone();
                                    view! {
                                        <button
                                            type="button"
                                            class="books-tag"
                                            on:click=move |_| { on_filter_tag.try_run(tag.clone()); }
                                        >
                                            {s.clone()}
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        })}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
