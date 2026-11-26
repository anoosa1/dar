//! Comic reader: single page, two-page spread, or vertical webtoon strip.
//! Taps on the outer thirds and arrow keys turn pages in the reading direction (right-to-left
//! for manga); upcoming pages preload.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::ui::window;

const HINT_KEY: &str = "dar:books:comic-hint";

#[derive(Clone)]
pub struct ComicHandle {
    next: Callback<()>,
    previous: Callback<()>,
    go_to: Callback<usize>,
}

impl ComicHandle {
    pub fn next(&self) {
        self.next.run(());
    }
    pub fn previous(&self) {
        self.previous.run(());
    }
    pub fn go_to(&self, page: usize) {
        self.go_to.run(page);
    }
}

#[component]
pub fn ComicView(
    #[prop(into)] base: String,
    pages: usize,
    start: usize,
    #[prop(into)] right_to_left: Signal<bool>,
    #[prop(into)] layout: Signal<String>,
    #[prop(into)] fit: Signal<String>,
    #[prop(into)] label: Signal<String>,
    handle: RwSignal<Option<ComicHandle>>,
    on_page: Callback<usize>,
) -> impl IntoView {
    let root = NodeRef::<html::Div>::new();
    let max_page = pages.saturating_sub(1);
    let index = RwSignal::new(start.min(max_page));
    let failed = RwSignal::new(false);
    let hint = RwSignal::new(
        window()
            .local_storage()
            .ok()
            .flatten()
            .and_then(|s| s.get_item(HINT_KEY).ok().flatten())
            .is_none(),
    );
    let base = StoredValue::new(base);

    let step_size = move || {
        if layout.get_untracked() == "spread" {
            2isize
        } else {
            1isize
        }
    };

    let go = move |value: isize| {
        let clamped = value.clamp(0, max_page as isize) as usize;
        index.set(clamped);
    };

    handle.set(Some(ComicHandle {
        next: Callback::new(move |_| go(index.get_untracked() as isize + step_size())),
        previous: Callback::new(move |_| go(index.get_untracked() as isize - step_size())),
        go_to: Callback::new(move |page: usize| go(page as isize)),
    }));
    on_cleanup(move || {
        let _ = handle.try_set(None);
    });

    // Report page and preload upcoming pages so turning feels instant.
    Effect::new(move |_| {
        let idx = index.get();
        failed.set(false);
        let _ = on_page.try_run(idx);
        let prefix = base.get_value();
        for n in [idx + 1, idx + 2, idx + 3] {
            if n < pages
                && let Ok(image) = web_sys::HtmlImageElement::new()
            {
                image.set_src(&format!("{prefix}{n}"));
            }
        }
    });

    // Dismiss the subtle bottom hint pill after 4 seconds.
    Effect::new(move |_| {
        if !hint.get() {
            return;
        }
        spawn_local(async move {
            crate::sleep(4000).await;
            if hint.try_get_untracked() == Some(true) {
                let _ = hint.try_set(false);
                if let Some(storage) = window().local_storage().ok().flatten() {
                    let _ = storage.set_item(HINT_KEY, "1");
                }
            }
        });
    });

    let on_click = move |event: web_sys::MouseEvent| {
        if layout.get_untracked() == "strip" {
            return;
        }
        let Some(node) = root.get_untracked() else {
            return;
        };
        let rect = node.get_bounding_client_rect();
        if rect.width() <= 0.0 {
            return;
        }
        let x = (event.client_x() as f64 - rect.left()) / rect.width();
        let rtl = right_to_left.get_untracked();
        let cur = index.get_untracked() as isize;
        let delta = step_size();
        if x < 0.33 {
            go(if rtl { cur + delta } else { cur - delta });
        } else if x > 0.67 {
            go(if rtl { cur - delta } else { cur + delta });
        }
    };

    let on_strip_scroll = move |_| {
        if layout.get_untracked() != "strip" || pages == 0 {
            return;
        }
        let Some(node) = root.get_untracked() else {
            return;
        };
        let max_scroll = (node.scroll_height() - node.client_height()).max(1) as f64;
        let ratio = ((node.scroll_top() as f64) / max_scroll).clamp(0.0, 1.0);
        let page = (ratio * max_page as f64).round() as usize;
        if page != index.get_untracked() {
            index.set(page);
        }
    };

    let stage_class = move || format!("comic-view is-{} fit-{}", layout.get(), fit.get(),);

    view! {
        <div
            node_ref=root
            class=stage_class
            role="group"
            aria-label=move || label.get()
            on:click=on_click
            on:scroll=on_strip_scroll
        >
            {move || {
                let mode = layout.get();
                if mode == "strip" {
                    let prefix = base.get_value();
                    return view! {
                        <div class="comic-strip">
                            {(0..pages).map(|i| view! {
                                <img
                                    src=format!("{prefix}{i}")
                                    alt=format!("Page {} of {pages}", i + 1)
                                    loading="lazy"
                                    draggable="false"
                                />
                            }).collect_view()}
                        </div>
                    }
                    .into_any();
                }

                let idx = index.get();
                if failed.get() {
                    return view! {
                        <p class="pdf-view-message" role="alert">
                            {format!("Page {} could not be shown.", idx + 1)}
                        </p>
                    }
                    .into_any();
                }

                let prefix = base.get_value();
                if mode == "spread" && idx + 1 < pages {
                    let rtl = right_to_left.get();
                    let (left_idx, right_idx) = if rtl { (idx + 1, idx) } else { (idx, idx + 1) };
                    return view! {
                        <div class="comic-spread">
                            <img
                                src=format!("{prefix}{left_idx}")
                                alt=format!("Page {} of {pages}", left_idx + 1)
                                on:error=move |_| failed.set(true)
                                draggable="false"
                            />
                            <img
                                src=format!("{prefix}{right_idx}")
                                alt=format!("Page {} of {pages}", right_idx + 1)
                                on:error=move |_| failed.set(true)
                                draggable="false"
                            />
                        </div>
                    }
                    .into_any();
                }

                view! {
                    <img
                        src=format!("{prefix}{idx}")
                        alt=format!("Page {} of {pages}", idx + 1)
                        on:error=move |_| failed.set(true)
                        draggable="false"
                    />
                }
                .into_any()
            }}
            {move || {
                (hint.get() && layout.get() != "strip").then(|| {
                    let rtl = right_to_left.get();
                    view! {
                        <div class="comic-hint-pill" aria-hidden="true">
                            {if rtl {
                                "Reading right to left \u{00b7} Left side turns forward"
                            } else {
                                "Tap the sides or use arrow keys to turn pages"
                            }}
                        </div>
                    }
                })
            }}
        </div>
    }
}
