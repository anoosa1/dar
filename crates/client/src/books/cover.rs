//! Book cover component: image covers with spine depth, literary typographic fallback covers,
//! format badges, DRM/error indicators, progress bars, and finished checkmarks.

use leptos::prelude::*;

use crate::books::model::{API, BookItem, book_format_label, hue, percent};
use crate::icons::Icon;

/// The cover image, or a literary typographic cover when the book has none or it cannot load.
#[component]
pub fn Cover(book: BookItem, #[prop(default = "m")] size: &'static str) -> impl IntoView {
    let failed = RwSignal::new(!book.cover);
    let done = book.state.as_ref().is_some_and(|s| s.status == "finished");
    let progress = book
        .state
        .as_ref()
        .filter(|s| s.status == "reading")
        .map_or(0.0, |s| s.progress);
    let title = book.title.clone();
    let title_hue = hue(&book.title);
    let first_author = book.authors.first().cloned().unwrap_or_default();
    let cover_src = format!("{API}/{}/cover?r={}", book.id, book.revision);
    let format = book.format.clone();
    let has_error = !book.error.is_empty();
    let error_label = match book.error.as_str() {
        "drm" => "DRM",
        "unsupported" => "UNSUPPORTED",
        "damaged" => "DAMAGED",
        _ => "",
    };
    let show_format = !has_error && format != "epub" && size != "s";
    let show_error_badge = has_error && size != "s";
    let has_badge = show_format || show_error_badge;
    let format_label = book_format_label(&format);

    view! {
        <span class=move || {
            let mut cls = format!("books-cover is-{size}");
            if has_badge {
                cls.push_str(" has-badge");
            }
            if failed.get() {
                cls.push_str(&format!(" is-generated hue-{title_hue}"));
            }
            if has_error {
                cls.push_str(" has-error");
            }
            cls
        }>
            {move || {
                if failed.get() {
                    view! {
                        <b>{title.clone()}</b>
                        <span class="books-cover-rule" aria-hidden="true"></span>
                        <i>{first_author.clone()}</i>
                    }
                    .into_any()
                } else {
                    view! {
                        <img
                            src=cover_src.clone()
                            alt=""
                            loading="lazy"
                            decoding="async"
                            on:error=move |_| failed.set(true)
                        />
                    }
                    .into_any()
                }
            }}
            {show_error_badge.then(|| view! {
                <em class="books-format is-error" title="This book cannot be opened">{error_label}</em>
            })}
            {show_format.then(|| view! { <em class="books-format">{format_label.clone()}</em> })}
            {(progress > 0.0 && progress < 1.0).then(|| view! {
                <span class="books-progress">
                    <progress max="1" value=progress.to_string() aria-label=format!("{} read", percent(progress))/>
                </span>
            })}
            {done.then(|| view! {
                <span class="books-done" title="Finished">
                    <Icon name="check"/>
                    <span class="sr-only">"Finished"</span>
                </span>
            })}
        </span>
    }
}
