//! Shared controls (`controls.tsx`, `dialog.tsx`, `pane.tsx`, `body-class.ts`,
//! `connection-status.tsx`): the same markup and classes, so Dar's CSS applies unchanged.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use leptos::html;
use leptos::portal::Portal;
use leptos::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::icons::Icon;

pub fn window() -> web_sys::Window {
    web_sys::window().expect("window")
}

pub fn document() -> web_sys::Document {
    window().document().expect("document")
}

thread_local! {
    static NEXT_ID: Cell<u32> = const { Cell::new(0) };
    static BODY_CLASSES: RefCell<HashMap<&'static str, u32>> = RefCell::new(HashMap::new());
}

/// A page-unique element id (React's `useId`).
pub fn unique_id() -> String {
    NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        format!("r{}", n.get())
    })
}

/// Adds classes to `<body>` while the calling component is mounted.
pub fn use_body_class(names: &'static [&'static str]) {
    let body = document().body().expect("body");
    BODY_CLASSES.with(|counts| {
        let mut counts = counts.borrow_mut();
        for name in names {
            *counts.entry(name).or_default() += 1;
            let _ = body.class_list().add_1(name);
        }
    });
    on_cleanup(move || {
        let body = document().body().expect("body");
        BODY_CLASSES.with(|counts| {
            let mut counts = counts.borrow_mut();
            for name in names {
                let left = counts.get(name).copied().unwrap_or(1) - 1;
                if left > 0 {
                    counts.insert(name, left);
                } else {
                    counts.remove(name);
                    let _ = body.class_list().remove_1(name);
                }
            }
        });
    });
}

/// An event listener removed when dropped.
pub struct Listener {
    target: web_sys::EventTarget,
    name: &'static str,
    capture: bool,
    callback: Closure<dyn FnMut(web_sys::Event)>,
}

impl Listener {
    pub fn new(
        target: &web_sys::EventTarget,
        name: &'static str,
        capture: bool,
        f: impl FnMut(web_sys::Event) + 'static,
    ) -> Self {
        let callback = Closure::<dyn FnMut(web_sys::Event)>::new(f);
        let _ = target.add_event_listener_with_callback_and_bool(
            name,
            callback.as_ref().unchecked_ref(),
            capture,
        );
        Self {
            target: target.clone(),
            name,
            capture,
            callback,
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.target.remove_event_listener_with_callback_and_bool(
            self.name,
            self.callback.as_ref().unchecked_ref(),
            self.capture,
        );
    }
}

/// Keeps a listener for as long as the current reactive owner lives.
pub fn listen(
    target: &web_sys::EventTarget,
    name: &'static str,
    capture: bool,
    f: impl FnMut(web_sys::Event) + 'static,
) {
    let listener = send_wrapper::SendWrapper::new(Listener::new(target, name, capture, f));
    on_cleanup(move || drop(listener));
}

pub fn call_method(target: &JsValue, name: &str) {
    if let Ok(function) = js_sys::Reflect::get(target, &JsValue::from_str(name))
        && let Some(function) = function.dyn_ref::<js_sys::Function>()
    {
        let _ = function.call0(target);
    }
}

pub fn active_element() -> Option<web_sys::HtmlElement> {
    document().active_element().and_then(|e| e.dyn_into().ok())
}

/// Menus, listboxes and pickers open in the top layer (Popover API) at a viewport-clamped
/// position next to their anchor (`useFloating`).
pub fn use_floating(
    open: Signal<bool>,
    anchor: NodeRef<html::Button>,
    panel: NodeRef<html::Div>,
    align_end: bool,
    gap: f64,
) {
    Effect::new(move |_| {
        let (true, Some(element)) = (open.get(), panel.get()) else {
            return;
        };
        let element: web_sys::HtmlElement = element.unchecked_into();
        if !element.has_attribute("popover") {
            let _ = element.set_attribute("popover", "manual");
        }
        let place = std::rc::Rc::new({
            let element = element.clone();
            move || {
                let Some(anchor) = anchor.get_untracked() else {
                    return;
                };
                let rect = anchor.get_bounding_client_rect();
                let (inner_w, inner_h) = (
                    window()
                        .inner_width()
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0),
                    window()
                        .inner_height()
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0),
                );
                let margin = 8.0;
                let below = inner_h - rect.bottom() - gap - margin;
                let above = rect.top() - gap - margin;
                let style = element.style();
                let _ = style.set_property(
                    "--float-anchor-width",
                    &format!("{}px", rect.width().round()),
                );
                let (height, width) = (
                    element.offset_height() as f64,
                    element.offset_width() as f64,
                );
                let up = height > below && above > below;
                let _ = style.set_property(
                    "--float-max-height",
                    &format!("{}px", 120f64.max((if up { above } else { below }).floor())),
                );
                let top = margin.max(
                    (if up {
                        rect.top() - gap - height.min(above)
                    } else {
                        rect.bottom() + gap
                    })
                    .min(inner_h - margin - height.min(inner_h - 2.0 * margin)),
                );
                let mut left = if align_end {
                    rect.right() - width
                } else {
                    rect.left()
                };
                if left < margin {
                    left = rect.left();
                } else if left + width > inner_w - margin {
                    left = rect.right() - width;
                }
                left = margin.max(left.min(inner_w - margin - width));
                let _ = style.set_property("--float-top", &format!("{}px", top.round()));
                let _ = style.set_property("--float-left", &format!("{}px", left.round()));
                let _ = element.set_attribute("data-side", if up { "up" } else { "down" });
            }
        });
        let ensure_open = std::rc::Rc::new({
            let element = element.clone();
            let place = place.clone();
            move || {
                if open.try_get_untracked() != Some(true) || !element.is_connected() {
                    return;
                }
                if !element.matches(":popover-open").unwrap_or(true) {
                    call_method(&element, "showPopover");
                }
                place();
            }
        });
        ensure_open();
        {
            let ensure = ensure_open.clone();
            request_animation_frame(move || ensure());
        }
        let observer_callback = {
            let element = element.clone();
            let place = place.clone();
            let ensure = ensure_open.clone();
            Closure::<dyn FnMut()>::new(move || {
                // Moving an ancestor (such as a Leptos `<For>` row reorder) closes an open
                // popover silently (`fireEvents = false`), collapsing it to 0x0 and triggering
                // ResizeObserver. Reopen and re-anchor on the next frame if still open.
                if open.try_get_untracked() == Some(true)
                    && !element.matches(":popover-open").unwrap_or(true)
                {
                    let ensure = ensure.clone();
                    request_animation_frame(move || ensure());
                } else {
                    place();
                }
            })
        };
        let observer = web_sys::ResizeObserver::new(observer_callback.as_ref().unchecked_ref())
            .expect("ResizeObserver");
        observer.observe(&element);
        let resize = Listener::new(&window(), "resize", false, {
            let place = place.clone();
            move |_| place()
        });
        let scroll = Listener::new(&window(), "scroll", true, {
            let place = place.clone();
            move |_| place()
        });
        let toggle = Listener::new(&element, "toggle", false, {
            let element = element.clone();
            let ensure = ensure_open.clone();
            move |_| {
                if open.try_get_untracked() == Some(true)
                    && !element.matches(":popover-open").unwrap_or(true)
                {
                    let ensure = ensure.clone();
                    request_animation_frame(move || ensure());
                }
            }
        });
        let kept = send_wrapper::SendWrapper::new((
            observer,
            observer_callback,
            resize,
            scroll,
            toggle,
            element,
        ));
        on_cleanup(move || {
            let (observer, _callback, _resize, _scroll, _toggle, element) = kept.take();
            observer.disconnect();
            if element.matches(":popover-open").unwrap_or(false) {
                call_method(&element, "hidePopover");
            }
        });
    });
}

/// A menu command. `section` starts a labelled group; `shortcut` is a visible hint only.
#[derive(Clone, Default)]
pub struct MenuAction {
    pub label: String,
    pub icon: Option<&'static str>,
    pub disabled: bool,
    pub danger: bool,
    pub href: Option<String>,
    pub run: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    pub separator: bool,
    pub current: Option<bool>,
    pub items: Option<Vec<MenuAction>>,
    pub shortcut: Option<String>,
    pub section: Option<String>,
    pub download: bool,
}

impl MenuAction {
    pub fn new(label: impl Into<String>, icon: &'static str) -> Self {
        Self {
            label: label.into(),
            icon: Some(icon),
            ..Default::default()
        }
    }
    pub fn plain(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ..Default::default()
        }
    }
    pub fn run(mut self, f: impl Fn() + Send + Sync + 'static) -> Self {
        self.run = Some(std::sync::Arc::new(f));
        self
    }
    pub fn href(mut self, href: impl Into<String>) -> Self {
        self.href = Some(href.into());
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    pub fn separator(mut self) -> Self {
        self.separator = true;
        self
    }
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }
    pub fn section(mut self, section: impl Into<String>) -> Self {
        self.section = Some(section.into());
        self
    }
    pub fn current(mut self, current: bool) -> Self {
        self.current = Some(current);
        self
    }
    pub fn items(mut self, items: Vec<MenuAction>) -> Self {
        self.items = Some(items);
        self
    }
    pub fn download(mut self) -> Self {
        self.download = true;
        self
    }
}

fn menu_items(panel: &web_sys::HtmlElement, selector: &str) -> Vec<web_sys::HtmlElement> {
    let list = panel.query_selector_all(selector).expect("selector");
    (0..list.length())
        .filter_map(|i| list.item(i)?.dyn_into().ok())
        .collect()
}

fn focus_by_key(items: &[web_sys::HtmlElement], key: &str) -> bool {
    if !["ArrowDown", "ArrowUp", "Home", "End"].contains(&key) || items.is_empty() {
        return false;
    }
    let active = document().active_element();
    let index = items
        .iter()
        .position(|item| Some(item.unchecked_ref::<web_sys::Element>()) == active.as_ref())
        .map(|i| i as i64)
        .unwrap_or(-1);
    let len = items.len() as i64;
    let next = match key {
        "Home" => 0,
        "End" => len - 1,
        "ArrowDown" => (index + 1 + len) % len,
        _ => (index - 1 + len) % len,
    };
    let _ = items[next as usize].focus();
    true
}

/// Commands share dismissal, arrow-key navigation and focus restoration (`ActionMenu`).
#[component]
pub fn ActionMenu(
    #[prop(into)] label: Signal<String>,
    children: Children,
    /// Called each time the menu opens or changes page, so items reflect current state.
    #[prop(into)]
    actions: Callback<(), Vec<MenuAction>>,
    #[prop(optional, into)] class: String,
    #[prop(optional, into)] on_open: Option<Callback<()>>,
    #[prop(optional)] heading: Option<ChildrenFn>,
    #[prop(optional)] align_start: bool,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let page: RwSignal<Option<Vec<MenuAction>>> = RwSignal::new(None);
    let side = RwSignal::new("down");
    let root = NodeRef::<html::Div>::new();
    let trigger = NodeRef::<html::Button>::new();
    let menu = NodeRef::<html::Div>::new();
    let id = unique_id();
    use_floating(open.into(), trigger, menu, !align_start, 6.0);
    let toggle = move || {
        if !open.get_untracked() {
            page.set(None);
            if let Some(on_open) = on_open {
                {
                    on_open.try_run(());
                };
            }
            let bottom = trigger
                .get_untracked()
                .map(|t| t.get_bounding_client_rect().bottom())
                .unwrap_or(0.0);
            let half = window()
                .inner_height()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                / 2.0;
            side.set(if bottom > half { "up" } else { "down" });
        }
        open.update(|o| *o = !*o);
    };
    let close = move || {
        open.set(false);
        if let Some(t) = trigger.get_untracked() {
            let _ = t.focus();
        }
    };
    Effect::new(move |_| {
        page.track();
        if !open.get() {
            return;
        }
        let Some(panel) = menu.get() else { return };
        listen(&document(), "pointerdown", false, move |event| {
            let target = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Node>().ok());
            if let Some(root) = root.get_untracked()
                && !root.contains(target.as_ref())
            {
                open.set(false);
            }
        });
        if let Some(first) = menu_items(&panel, "[role^=menuitem]:not(:disabled)").first() {
            let _ = first.focus();
        }
    });
    let shown = move || page.get().unwrap_or_else(|| actions.run(()));
    let heading = StoredValue::new(heading);
    view! {
        <div class=format!("ui-menu {class}") node_ref=root data-side=move || side.get()>
            <button type="button" class="ui-menu-trigger" aria-label=move || label.get() aria-expanded=move || open.get().to_string() aria-haspopup="menu" aria-controls=id.clone() node_ref=trigger
                on:click=move |_| toggle()
                on:keydown=move |e| {
                    if e.key() == "ArrowDown" { e.prevent_default(); if !open.get_untracked() { toggle() } }
                    // Focus stays on the trigger when every item is disabled.
                    else if e.key() == "Escape" && open.get_untracked() { e.prevent_default(); e.stop_propagation(); close(); }
                }>
                {children()}
            </button>
            <Show when=move || open.get()>
                <div id=id.clone() node_ref=menu role="menu" aria-label=move || label.get() class="ui-menu-panel"
                    on:keydown=move |e| {
                        let Some(panel) = menu.get_untracked() else { return };
                        let items = menu_items(&panel, "[role^=menuitem]:not(:disabled)");
                        let key = e.key();
                        if key == "Escape" { e.prevent_default(); e.stop_propagation(); close(); }
                        else if focus_by_key(&items, &key) { e.prevent_default(); }
                        else if key == "Tab" { open.set(false); }
                    }>
                    {move || heading.with_value(|h| h.as_ref().map(|h| view! { <div class="ui-account-heading">{h()}</div> }))}
                    {move || page.get().map(|_| view! {
                        <button type="button" role="menuitem" class="ui-menu-item ui-menu-back" on:click=move |_| page.set(None)><Icon name="back"/><span>"Back"</span></button><hr/>
                    })}
                    {move || shown().into_iter().map(|action| menu_row(action, page, close)).collect_view()}
                </div>
            </Show>
        </div>
    }
}

fn menu_row(
    action: MenuAction,
    page: RwSignal<Option<Vec<MenuAction>>>,
    close: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let MenuAction {
        label,
        icon,
        disabled,
        danger,
        href,
        run,
        separator,
        current,
        items,
        shortcut,
        section,
        download,
    } = action;
    let class = format!("ui-menu-item {}", if danger { "danger" } else { "" });
    let item = if let Some(href) = href {
        view! {
            <a role="menuitem" class=class aria-current=current.filter(|c| *c).map(|_| "page") href=href download=download.then_some("") on:click=move |_| close()>
                {icon.map(|i| view! { <Icon name=i/> })}<span>{label}</span>
                {current.filter(|c| *c).map(|_| view! { <Icon name="check" class="ui-menu-current"/> })}
                {shortcut.map(|s| view! { <kbd class="ui-menu-shortcut" aria-hidden="true">{s}</kbd> })}
            </a>
        }
        .into_any()
    } else {
        let has_items = items.is_some();
        let items = StoredValue::new(items);
        view! {
            <button type="button" role=if current.is_none() { "menuitem" } else { "menuitemradio" } aria-checked=current.map(|c| c.to_string()) disabled=disabled
                aria-haspopup=has_items.then_some("menu") class=class
                on:click=move |_| {
                    if let Some(items) = items.get_value() { page.set(Some(items)); return; }
                    close();
                    if let Some(run) = &run { run(); }
                }>
                {icon.map(|i| view! { <Icon name=i/> })}<span>{label}</span>
                {shortcut.map(|s| view! { <kbd class="ui-menu-shortcut" aria-hidden="true">{s}</kbd> })}
                {current.filter(|c| *c).map(|_| view! { <Icon name="check" class="ui-menu-current"/> })}
                {has_items.then(|| view! { <Icon name="chevron" class="ui-menu-submenu"/> })}
            </button>
        }
        .into_any()
    };
    view! {
        <div>
            {separator.then(|| view! { <hr/> })}
            {section.map(|s| view! { <div class="ui-menu-section" aria-hidden="true">{s}</div> })}
            {item}
        </div>
    }
}

/// The hidden field supplies form data; visible choices use a themed listbox (`Select`).
#[component]
pub fn Select(
    #[prop(into)] options: Vec<(String, String)>,
    #[prop(optional, into)] name: Option<String>,
    #[prop(optional, into)] value: Option<RwSignal<String>>,
    #[prop(optional, into)] aria_label: Option<String>,
    #[prop(optional, into)] on_change: Option<Callback<String>>,
) -> impl IntoView {
    let current = value
        .unwrap_or_else(|| RwSignal::new(options.first().map(|o| o.0.clone()).unwrap_or_default()));
    let open = RwSignal::new(false);
    let side = RwSignal::new("down");
    let root = NodeRef::<html::Div>::new();
    let button = NodeRef::<html::Button>::new();
    let list = NodeRef::<html::Div>::new();
    let id = unique_id();
    let implicit = RwSignal::new(String::new());
    let options = StoredValue::new(options);
    use_floating(open.into(), button, list, false, 5.0);
    Effect::new(move |_| {
        let Some(b) = button.get() else { return };
        if let Some(label) = b.closest("label").ok().flatten() {
            let nodes = label.child_nodes();
            let text: Vec<String> = (0..nodes.length())
                .filter_map(|i| nodes.item(i))
                .filter(|n| n.node_type() == web_sys::Node::TEXT_NODE)
                .filter_map(|n| n.text_content())
                .collect();
            implicit.set(text.join(" ").trim().to_owned());
        }
    });
    let choose = move |next: String| {
        current.set(next.clone());
        open.set(false);
        if let Some(b) = button.get_untracked() {
            let _ = b.focus();
        }
        if let Some(f) = on_change {
            f.run(next);
        }
    };
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let Some(panel) = list.get() else { return };
        listen(&document(), "pointerdown", false, move |event| {
            let target = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Node>().ok());
            if let Some(root) = root.get_untracked()
                && !root.contains(target.as_ref())
            {
                open.set(false);
            }
        });
        let selected = panel
            .query_selector("[aria-selected=\"true\"]")
            .ok()
            .flatten()
            .or_else(|| {
                panel
                    .query_selector("[role=\"option\"]:not(:disabled)")
                    .ok()
                    .flatten()
            });
        if let Some(el) = selected.and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
            let _ = el.focus();
        }
    });
    let label = {
        let aria = aria_label.clone();
        move || aria.clone().unwrap_or_else(|| implicit.get())
    };
    let list_label = aria_label.clone().unwrap_or_else(|| "Options".into());
    let options_id = format!("{id}-options");
    view! {
        <div node_ref=root class="ui-select " data-side=move || side.get()>
            <input type="hidden" name=name prop:value=move || current.get()/>
            <button id=id.clone() node_ref=button type="button" role="combobox" aria-label=label aria-expanded=move || open.get().to_string() aria-haspopup="listbox" aria-controls=options_id.clone() class="ui-select-trigger"
                on:click=move |_| {
                    let bottom = button.get_untracked().map(|b| b.get_bounding_client_rect().bottom()).unwrap_or(0.0);
                    let limit = window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0) - 270.0;
                    side.set(if bottom > limit { "up" } else { "down" });
                    open.update(|o| *o = !*o);
                }
                on:keydown=move |e| { if ["ArrowDown", "ArrowUp", " "].contains(&e.key().as_str()) { e.prevent_default(); open.set(true); } else if e.key() == "Escape" { open.set(false); } }>
                {move || options.with_value(|o| o.iter().find(|(v, _)| *v == current.get()).map(|(_, l)| l.clone())).unwrap_or_else(|| "Choose…".into())}
                <Icon name="chevron"/>
            </button>
            <Show when=move || open.get()>
                <div id=options_id.clone() node_ref=list role="listbox" aria-label=list_label.clone() class="ui-select-options"
                    on:keydown=move |e| {
                        let Some(panel) = list.get_untracked() else { return };
                        let items = menu_items(&panel, "[role=\"option\"]:not(:disabled)");
                        let key = e.key();
                        if key == "Escape" { e.prevent_default(); e.stop_propagation(); open.set(false); if let Some(b) = button.get_untracked() { let _ = b.focus(); } }
                        else if focus_by_key(&items, &key) { e.prevent_default(); }
                        else if key == "Tab" { open.set(false); }
                        else if key.chars().count() == 1 {
                            let lower = key.to_lowercase();
                            if let Some(item) = items.iter().find(|i| i.text_content().unwrap_or_default().to_lowercase().starts_with(&lower)) { let _ = item.focus(); }
                        }
                    }>
                    {move || options.get_value().into_iter().map(|(value, text)| {
                        let chosen = value.clone();
                        let selected = Memo::new(move |_| current.get() == chosen);
                        let pick = value.clone();
                        view! {
                            <button type="button" role="option" data-value=value aria-selected=move || selected.get().to_string() on:click=move |_| choose(pick.clone())>
                                {text}{move || selected.get().then(|| view! { <Icon name="check"/> })}
                            </button>
                        }
                    }).collect_view()}
                </div>
            </Show>
        </div>
    }
}

/// Restores focus to `previous` if it is still visible, else to the page's main region.
fn restore_focus(previous: Option<web_sys::HtmlElement>) {
    if let Some(previous) =
        previous.filter(|p| p.is_connected() && p.get_bounding_client_rect().width() > 0.0)
    {
        let _ = previous.focus();
    } else if let Some(main) = document()
        .query_selector("main")
        .ok()
        .flatten()
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = main.focus();
    }
}

/// A modal `<dialog>` in `document.body` (`Dialog`).
#[component]
pub fn Dialog(
    #[prop(into)] title: Signal<String>,
    #[prop(into)] on_close: Callback<()>,
    children: ChildrenFn,
) -> impl IntoView {
    let dialog = NodeRef::<html::Dialog>::new();
    let title_id = unique_id();
    // As in React, the focus to restore is read just before the dialog opens, after any
    // dialog it replaces has closed and restored its own.
    let previous = StoredValue::new_local(None::<web_sys::HtmlElement>);
    on_cleanup(move || restore_focus(previous.try_get_value().flatten()));
    Effect::new(move |_| {
        if let Some(dialog) = dialog.get()
            && !dialog.open()
        {
            previous.set_value(active_element());
            let _ = dialog.show_modal();
        }
    });
    let children = StoredValue::new(children);
    view! {
        <Portal>
            <dialog node_ref=dialog class="dialog" aria-labelledby=title_id.clone() on:cancel=move |e: web_sys::Event| { e.prevent_default(); { on_close.try_run(()); }; }>
                <div class="dialog-heading"><h2 id=title_id.clone()>{move || title.get()}</h2><button class="icon-button" aria-label="Close dialog" on:click=move |_| { on_close.try_run(()); }><Icon name="close"/></button></div>
                {children.with_value(|c| c())}
            </dialog>
        </Portal>
    }
}

/// The non-modal right-hand editing pane (`EditorPane`). A button labelled Cancel closes it,
/// asking first when `dirty` reports unsaved changes.
#[component]
pub fn EditorPane(
    #[prop(into)] title: Signal<String>,
    #[prop(into)] on_close: Callback<()>,
    #[prop(optional, into)] dirty: Option<Signal<bool>>,
    children: ChildrenFn,
) -> impl IntoView {
    use_body_class(&["has-editing-pane"]);
    let pane = NodeRef::<html::Aside>::new();
    let id = unique_id();
    let changed = RwSignal::new(false);
    let discard = RwSignal::new(false);
    let is_dirty = move || {
        dirty
            .map(|d| d.get_untracked())
            .unwrap_or_else(|| changed.get_untracked())
    };
    let close = move || {
        if is_dirty() {
            discard.set(true)
        } else {
            on_close.try_run(());
        }
    };
    let previous = send_wrapper::SendWrapper::new(active_element());
    on_cleanup(move || restore_focus(previous.take()));
    Effect::new(move |_| {
        let Some(pane) = pane.get() else { return };
        let target = pane
            .query_selector("[autofocus],input:not([type=hidden]),textarea")
            .ok()
            .flatten()
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok());
        let _ = target
            .unwrap_or_else(|| pane.clone().unchecked_into())
            .focus();
        listen(&window(), "beforeunload", false, move |event| {
            if is_dirty() {
                event.prevent_default()
            }
        });
    });
    let children = StoredValue::new(children);
    view! {
        <Portal>
            <aside node_ref=pane tabindex="-1" data-editor-pane="" class="editing-pane" aria-labelledby=id.clone()
                on:change=move |_| changed.set(true)
                on:keydown=move |e| if e.key() == "Escape" && !e.default_prevented() { e.prevent_default(); close(); }>
                <div class="editing-pane-heading"><h2 id=id.clone()>{move || title.get()}</h2><button type="button" class="icon-button" aria-label="Close editor" on:click=move |_| close()><Icon name="close"/></button></div>
                <Show when=move || discard.get()>
                    <div class="draft-banner" role="alert"><p>"Discard unsaved changes?"</p>
                        <button type="button" class="text-button" on:click=move |_| discard.set(false)>"Keep editing"</button>
                        <button type="button" class="text-button danger" on:click=move |_| { on_close.try_run(()); }>"Discard changes"</button>
                    </div>
                </Show>
                <div class="editing-pane-content" on:click=move |e| {
                    let button = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).and_then(|t| t.closest("button").ok().flatten());
                    if button.and_then(|b| b.text_content()).is_some_and(|t| t.trim() == "Cancel") { e.prevent_default(); close(); }
                }>{children.with_value(|c| c())}</div>
            </aside>
        </Portal>
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnectionState {
    Connecting,
    Ready,
    Reconnecting,
    Error,
}

/// One status and recovery action for every app's live connection.
#[component]
pub fn ConnectionStatus(#[prop(into)] state: Signal<ConnectionState>) -> impl IntoView {
    move || {
        let state = state.get();
        (state != ConnectionState::Ready).then(|| {
            let failed = state == ConnectionState::Error;
            view! {
                <div class=if failed { "error-banner" } else { "connection-state" } role=if failed { "alert" } else { "status" }>
                    <Icon name="refresh"/>
                    <span>{if failed { "Unable to connect. Refresh to try again." } else if state == ConnectionState::Reconnecting { "Connection lost. Reconnecting…" } else { "Connecting…" }}</span>
                    {failed.then(|| view! { <button class="text-button" on:click=|_| { let _ = window().location().reload(); }>"Try again"</button> })}
                </div>
            }
        })
    }
}
