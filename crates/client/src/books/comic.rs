//! Comic reader (`next/client/books/comic.tsx`): one page at a time, fitted to the sheet.
//! Taps on the outer thirds and arrow keys turn pages in the reading direction (right-to-left
//! for manga); the next pages preload.

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

    let go = move |value: isize| {
        let clamped = value.clamp(0, max_page as isize) as usize;
        index.set(clamped);
    };

    handle.set(Some(ComicHandle {
        next: Callback::new(move |_| go(index.get_untracked() as isize + 1)),
        previous: Callback::new(move |_| go(index.get_untracked() as isize - 1)),
        go_to: Callback::new(move |page: usize| go(page as isize)),
    }));
    on_cleanup(move || {
        let _ = handle.try_set(None);
    });

    // Report page and preload the next two pages so turning feels instant.
    Effect::new(move |_| {
        let idx = index.get();
        failed.set(false);
        {
            on_page.try_run(idx);
        };
        let prefix = base.get_value();
        for n in [idx + 1, idx + 2] {
            if n < pages
                && let Ok(image) = web_sys::HtmlImageElement::new()
            {
                image.set_src(&format!("{prefix}{n}"));
            }
        }
    });

    // Dismiss the tap-zone hint after 3.5 seconds.
    Effect::new(move |_| {
        if !hint.get() {
            return;
        }
        spawn_local(async move {
            crate::sleep(3500).await;
            if hint.try_get_untracked() == Some(true) {
                hint.set(false);
                if let Some(storage) = window().local_storage().ok().flatten() {
                    let _ = storage.set_item(HINT_KEY, "1");
                }
            }
        });
    });

    let on_click = move |event: web_sys::MouseEvent| {
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
        if x < 0.33 {
            go(if rtl { cur + 1 } else { cur - 1 });
        } else if x > 0.67 {
            go(if rtl { cur - 1 } else { cur + 1 });
        }
    };

    view! {
        <div
            node_ref=root
            class="comic-view"
            role="group"
            aria-label=move || label.get()
            on:click=on_click
        >
            {move || {
                let idx = index.get();
                if failed.get() {
                    view! {
                        <p class="pdf-view-message" role="alert">
                            {format!("Page {} could not be shown.", idx + 1)}
                        </p>
                    }
                    .into_any()
                } else {
                    view! {
                        <img
                            src=format!("{}{idx}", base.get_value())
                            alt=format!("Page {} of {pages}", idx + 1)
                            on:error=move |_| failed.set(true)
                            draggable="false"
                        />
                    }
                    .into_any()
                }
            }}
            {move || {
                hint.get().then(|| {
                    let rtl = right_to_left.get();
                    view! {
                        <div class="comic-zones" aria-hidden="true">
                            <span>{if rtl { "Next page" } else { "Previous page" }}</span>
                            <span>"Tap the sides to turn pages"</span>
                            <span>{if rtl { "Previous page" } else { "Next page" }}</span>
                        </div>
                    }
                })
            }}
        </div>
    }
}
