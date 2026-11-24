//! Switching between apps without reloading (`navigation.ts`). Addresses this port
//! renders open in place; everything else (apps still served by the React client, shared
//! links, downloads) is a normal page load.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::ui::{document, listen, window};

/// The current address; `visit` changes on each in-place navigation so the page remounts.
#[derive(Clone, Copy)]
pub struct Route {
    pub path: RwSignal<String>,
    pub visit: RwSignal<u32>,
}

/// Paths this port renders. Keep in step with `App`'s routing.
pub fn handles(path: &str) -> bool {
    matches!(
        path,
        "/" | "/contacts"
            | "/calendar"
            | "/notes"
            | "/tasks"
            | "/news"
            | "/mail"
            | "/chat"
            | "/photos"
            | "/books"
            | "/git"
            | "/agent"
            | "/settings"
            | "/identity"
    ) || path
        .strip_prefix("/apps/")
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

thread_local! {
    static ROUTE: std::cell::Cell<Option<Route>> = const { std::cell::Cell::new(None) };
}

/// Registers the app root's route, which in-place navigation updates.
pub fn set_route(route: Route) {
    ROUTE.with(|r| r.set(Some(route)));
}

/// Whether an open editor would warn before the page unloads.
fn unsaved_changes() -> bool {
    let init = web_sys::EventInit::new();
    init.set_cancelable(true);
    let check = web_sys::Event::new_with_event_init_dict("beforeunload", &init).expect("event");
    let _ = window().dispatch_event(&check);
    check.default_prevented()
}

/// Opens a Dar address, in place when this port renders it.
pub fn navigate(href: &str) {
    let location = window().location();
    let Ok(url) = web_sys::Url::new_with_base(href, &location.href().unwrap_or_default()) else {
        return;
    };
    let same_origin = url.origin() == location.origin().unwrap_or_default();
    if same_origin
        && handles(&url.pathname())
        && !unsaved_changes()
        && let Some(route) = ROUTE.with(|r| r.get())
    {
        let _ = window().history().expect("history").push_state_with_url(
            &wasm_bindgen::JsValue::NULL,
            "",
            Some(&format!("{}{}{}", url.pathname(), url.search(), url.hash())),
        );
        route.path.set(url.pathname());
        route.visit.update(|v| *v += 1);
        window().scroll_to_with_x_and_y(0.0, 0.0);
        return;
    }
    let _ = location.assign(&url.href());
}

/// Follows links to ported apps in place for as long as the calling owner lives.
pub fn intercept_links() {
    listen(&document(), "click", false, move |event| {
        let event: web_sys::MouseEvent = event.unchecked_into();
        if event.default_prevented()
            || event.button() != 0
            || event.meta_key()
            || event.ctrl_key()
            || event.shift_key()
            || event.alt_key()
        {
            return;
        }
        let link = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|t| t.closest("a[href]").ok().flatten());
        let Some(link) = link.and_then(|l| l.dyn_into::<web_sys::HtmlAnchorElement>().ok()) else {
            return;
        };
        if link.has_attribute("download") || !(link.target().is_empty() || link.target() == "_self")
        {
            return;
        }
        let Ok(url) = web_sys::Url::new(&link.href()) else {
            return;
        };
        if url.origin() != window().location().origin().unwrap_or_default()
            || !handles(&url.pathname())
        {
            return;
        }
        event.prevent_default();
        navigate(&url.href());
    });
}

/// The route registered by the app root.
pub fn route_now() -> Route {
    ROUTE
        .with(|r| r.get())
        .expect("the app root registers its route")
}
