//! Notes (`notes.tsx`) and Tasks (`pim/workspace.tsx`, `pim/task-occurrences.tsx`) apps,
//! along with their shared shell, settings sheet, transfers and reactive select control.

mod logseq_import;
pub mod note_canvas;
pub mod note_cards;
pub mod note_editor;
pub mod note_graph;
pub mod note_links;
pub mod note_query;
pub mod note_tabs;
pub mod notes;
pub mod task_occurrences;
pub mod tasks;

pub use notes::Notes;
pub use tasks::TasksWorkspace;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use crate::api::{self, BrowserSession};
use crate::icons::Icon;
use crate::pim::{Collection, PimLive, send};
use crate::settings::{AppSettingsButton, SaveProblems, SaveStatus, Saver, Segmented, SettingRow, SettingsSheet};
use crate::shell::WorkspaceHeader;
use crate::ui::{ConnectionState, ConnectionStatus, document, listen, unique_id, use_floating, window};
use logseq_import::LogseqImport;

pub(super) const DEFAULT_NOTES_FOLDER: &str = "docs/notes";

pub(super) fn browser_timezone() -> String {
    let fmt = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new());
    let opts = fmt.resolved_options();
    js_sys::Reflect::get(&opts, &"timeZone".into())
        .ok()
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "UTC".into())
}

pub(super) fn search_params(pairs: &[(&str, String)]) -> String {
    let params = web_sys::UrlSearchParams::new().expect("params");
    for (k, v) in pairs {
        params.append(k, v);
    }
    params.to_string().as_string().unwrap_or_default()
}

pub(super) fn capitalized(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

fn menu_items(panel: &web_sys::HtmlElement, selector: &str) -> Vec<web_sys::HtmlElement> {
    let list = panel.query_selector_all(selector).expect("selector");
    (0..list.length()).filter_map(|i| list.item(i)?.dyn_into().ok()).collect()
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

/// Like `crate::ui::Select`, with reactive `options` and optional `disabled` / `required`.
#[component]
pub(super) fn ReactiveSelect(
    #[prop(into)] options: Signal<Vec<(String, String)>>,
    #[prop(optional, into)] name: Option<String>,
    #[prop(optional, into)] value: Option<RwSignal<String>>,
    #[prop(optional, into)] aria_label: Option<String>,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional)] required: bool,
    #[prop(optional, into)] on_change: Option<Callback<String>>,
) -> impl IntoView {
    let current = value.unwrap_or_else(|| {
        RwSignal::new(options.with_untracked(|o| o.first().map(|(v, _)| v.clone()).unwrap_or_default()))
    });
    let open = RwSignal::new(false);
    let side = RwSignal::new("down");
    let root = NodeRef::<html::Div>::new();
    let button = NodeRef::<html::Button>::new();
    let list = NodeRef::<html::Div>::new();
    let id = unique_id();
    let implicit = RwSignal::new(String::new());
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
            let target = event.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
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
            .or_else(|| panel.query_selector("[role=\"option\"]:not(:disabled)").ok().flatten());
        if let Some(el) = selected.and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
            let _ = el.focus();
        }
    });
    let label = {
        let aria = aria_label.clone();
        move || aria.clone().unwrap_or_else(|| implicit.get())
    };
    let list_label = aria_label.unwrap_or_else(|| "Options".into());
    let options_id = format!("{id}-options");
    view! {
        <div node_ref=root class="ui-select " data-side=move || side.get()>
            <input type="hidden" name=name prop:value=move || current.get() disabled=move || disabled.get()/>
            <button id=id.clone() node_ref=button type="button" role="combobox" aria-label=label aria-expanded=move || open.get().to_string() aria-haspopup="listbox" aria-controls=options_id.clone() aria-required=required.then_some("true") disabled=move || disabled.get() class="ui-select-trigger"
                on:click=move |_| {
                    let bottom = button.get_untracked().map(|b| b.get_bounding_client_rect().bottom()).unwrap_or(0.0);
                    let limit = window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0) - 270.0;
                    side.set(if bottom > limit { "up" } else { "down" });
                    open.update(|o| *o = !*o);
                }
                on:keydown=move |e| {
                    if ["ArrowDown", "ArrowUp", " "].contains(&e.key().as_str()) {
                        e.prevent_default();
                        open.set(true);
                    } else if e.key() == "Escape" {
                        open.set(false);
                    }
                }>
                {move || {
                    let cur = current.get();
                    options.with(|o| o.iter().find(|(v, _)| *v == cur).map(|(_, l)| l.clone())).unwrap_or_else(|| "Choose\u{2026}".into())
                }}
                <Icon name="chevron"/>
            </button>
            <Show when=move || open.get()>
                <div id=options_id.clone() node_ref=list role="listbox" aria-label=list_label.clone() class="ui-select-options"
                    on:keydown=move |e| {
                        let Some(panel) = list.get_untracked() else { return };
                        let items = menu_items(&panel, "[role=\"option\"]:not(:disabled)");
                        let key = e.key();
                        if key == "Escape" {
                            e.prevent_default();
                            e.stop_propagation();
                            open.set(false);
                            if let Some(b) = button.get_untracked() {
                                let _ = b.focus();
                            }
                        } else if focus_by_key(&items, &key) {
                            e.prevent_default();
                        } else if key == "Tab" {
                            open.set(false);
                        } else if key.chars().count() == 1 {
                            let lower = key.to_lowercase();
                            if let Some(item) = items.iter().find(|i| i.text_content().unwrap_or_default().to_lowercase().starts_with(&lower)) {
                                let _ = item.focus();
                            }
                        }
                    }>
                    {move || options.get().into_iter().map(|(val, text)| {
                        let chosen = val.clone();
                        let selected = Memo::new(move |_| current.get() == chosen);
                        let pick = val.clone();
                        view! {
                            <button type="button" role="option" data-value=val aria-selected=move || selected.get().to_string() on:click=move |_| choose(pick.clone())>
                                {text}{move || selected.get().then(|| view! { <Icon name="check"/> })}
                            </button>
                        }
                    }).collect_view()}
                </div>
            </Show>
        </div>
    }
}

/// Shared PIM shell for Notes and Tasks (`pim/common.tsx`'s `PimShell`), adding
/// `has-note-editor`, `has-workspace-list`, phone focus restoration, and the Notes/Tasks settings sheet.
#[component]
pub(super) fn WorkspaceShell(
    kind: &'static str,
    session: BrowserSession,
    on_signed_out: Callback<()>,
    sidebar: ChildrenFn,
    #[prop(into)] has_details: Signal<bool>,
    #[prop(optional, into)] note_editor: Signal<bool>,
    /// The note sheet shows two panes side by side, so the list column narrows.
    #[prop(optional, into)] split_details: Signal<bool>,
    details: ChildrenFn,
    pim: PimLive,
    children: ChildrenFn,
) -> impl IntoView {
    let navigation = RwSignal::new(false);
    let settings = RwSignal::new(false);
    let connected = RwSignal::new(false);
    Effect::new(move |_| {
        if pim.state.get() == ConnectionState::Ready {
            connected.set(true);
        }
    });
    let detail_label = if kind == "notes" { "Note details" } else { "Task details" };
    let panel = NodeRef::<html::Aside>::new();
    let main = NodeRef::<html::Main>::new();
    let origin: StoredValue<Option<send_wrapper::SendWrapper<web_sys::HtmlElement>>> = StoredValue::new(None);
    let had_details = RwSignal::new(false);

    // Record the control that opened details so focus returns to it on close.
    Effect::new(move |_| {
        let Some(m) = main.get() else { return };
        listen(&m, "click", true, move |event| {
            if !has_details.get_untracked() {
                let target = event
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .and_then(|el| el.closest("button,a").ok().flatten())
                    .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
                    .map(send_wrapper::SendWrapper::new);
                origin.set_value(target);
            }
        });
    });

    // On phones the details sheet takes focus when it opens; closing restores focus to origin.
    Effect::new(move |_| {
        let open = has_details.get();
        let panel_el = panel.get();
        if open && !had_details.get_untracked() {
            if let Some(p) = panel_el {
                had_details.set(true);
                if window().match_media("(max-width:800px)").ok().flatten().is_some_and(|m| m.matches()) {
                    // The details load after the sheet opens; focus their Close button once it is there.
                    fn focus_close(p: web_sys::HtmlElement, tries: u32) {
                        let close = p.query_selector("[aria-label^=\"Close\"]:not([data-tab-close])").ok().flatten().and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok());
                        match close {
                            Some(c) => { let _ = c.focus(); }
                            None if tries > 0 && p.is_connected() => request_animation_frame(move || focus_close(p, tries - 1)),
                            None => {
                                let target = p.query_selector("button,a,input").ok().flatten().and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok());
                                let _ = target.unwrap_or(p).focus();
                            }
                        }
                    }
                    let p: web_sys::HtmlElement = p.clone().into();
                    let _ = p.focus();
                    focus_close(p, 60);
                }
            }
        } else if !open && had_details.get_untracked() {
            had_details.set(false);
            // The list is shown again in the same pass; restore focus once it has its size.
            request_animation_frame(move || {
            let active = document().active_element();
            let body: Option<web_sys::Element> = document().body().map(|b| b.unchecked_into());
            if active.is_none() || active == body {
                let target = origin.try_with_value(|o| o.as_ref().map(|el| (**el).clone())).flatten();
                if let Some(t) = target.filter(|t| {
                    let rect = t.get_bounding_client_rect();
                    t.is_connected() && rect.width() > 0.0 && rect.height() > 0.0
                }) {
                    let _ = t.focus();
                } else if let Some(m) = main.try_get_untracked().flatten() {
                    let _ = m.focus();
                }
            }
            });
        }
    });

    let (session_header, session_settings) = (session.clone(), StoredValue::new(session));
    let sidebar = StoredValue::new(sidebar);
    let details = StoredValue::new(details);
    let children = StoredValue::new(children);
    view! {
        <div class=move || {
            let layout = if connected.get() { "has-workspace-list" } else { "" };
            let details_cls = if has_details.get() { "has-pim-details" } else { "" };
            let editor_cls = if has_details.get() && note_editor.get() { " has-note-editor" } else { "" };
            let split_cls = if has_details.get() && split_details.get() { " has-note-split" } else { "" };
            format!("app-shell pim-shell {details_cls} {layout}{editor_cls}{split_cls}")
        }>
            <WorkspaceHeader app=kind session=session_header on_signed_out=on_signed_out on_settings=Callback::new(move |_| settings.set(true))/>
            <div class=move || format!("pim-workspace {}", if connected.get() { "has-workspace-list" } else { "" })>
                {move || navigation.get().then(|| view! { <button class="nav-scrim" aria-label="Close navigation" on:click=move |_| navigation.set(false)></button> })}
                <aside class=move || if navigation.get() { "sidebar is-open" } else { "sidebar " }
                    on:click=move |e| {
                        let target = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok());
                        let Some(t) = target else { return };
                        let in_button = t.closest("button").ok().flatten().is_some();
                        let excluded = t.closest(".ui-select,.contacts-book-pages,.calendar-list-pages").ok().flatten().is_some();
                        let menu = t.closest(".ui-menu").ok().flatten().is_some() && t.closest("[role=\"menuitem\"]").ok().flatten().is_none();
                        if in_button && !excluded && !menu {
                            navigation.set(false);
                        }
                    }>
                    {sidebar.with_value(|s| s())}
                    <AppSettingsButton app=kind on_click=Callback::new(move |_| settings.set(true))/>
                </aside>
                <main node_ref=main tabindex="-1" class=move || format!("pim-main {}", if connected.get() { "has-workspace-list" } else { "" })>
                    <ConnectionStatus state=pim.state/>
                    {move || connected.get().then(|| children.with_value(|c| c()))}
                </main>
                {move || has_details.get().then(|| view! {
                    <aside node_ref=panel tabindex="-1" class=move || format!("pim-details{}", if has_details.get() && note_editor.get() { " has-note-editor" } else { "" }) aria-label=detail_label>
                        {details.with_value(|d| d())}
                    </aside>
                })}
            </div>
            {move || settings.get().then(|| view! {
                <WorkspaceSettings app=kind session=session_settings.get_value() pim=pim on_close=Callback::new(move |_| settings.set(false))/>
            })}
        </div>
    }
}

/// Validates a Files folder path like `"docs/notes"` (`photoFolderParts`).
fn folder_parts(value: &str) -> Option<Vec<String>> {
    if value.len() > 500 {
        return None;
    }
    let parts: Vec<String> = value
        .split('/')
        .map(|part| {
            let nfc: String = js_sys::JsString::from(part).normalize("NFC").into();
            nfc.trim().to_owned()
        })
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty()
        || parts.len() > 8
        || parts.iter().any(|part| {
            part.len() > 255 || part == "." || part == ".." || part.chars().any(|c| (c as u32) < 0x20 || c == '\\')
        })
    {
        return None;
    }
    Some(parts)
}

#[component]
fn NotesFolderSetting(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] state: Signal<Option<crate::settings::SaveState>>,
    on_save: Callback<String>,
) -> impl IntoView {
    let draft: RwSignal<Option<String>> = RwSignal::new(None);
    let current = Memo::new(move |_| draft.get().unwrap_or_else(|| value.get()));
    let valid = Memo::new(move |_| folder_parts(&current.get()).is_some());
    view! {
        <div class="settings-row is-stacked">
            <div class="settings-row-text">
                <span class="settings-row-label">"Notes folder"</span>
                <small>{move || format!("Notes are Markdown files in Files \u{203a} {}. Folders inside it are notebooks.", value.get())}</small>
            </div>
            <SaveStatus state=state/>
            <div class="settings-row-control">
                <form class="photos-folder-form" on:submit=move |e: web_sys::SubmitEvent| {
                    e.prevent_default();
                    if let Some(parts) = folder_parts(&current.get_untracked()) {
                        draft.set(None);
                        { on_save.try_run(parts.join("/")); };
                    }
                }>
                    <label>
                        <span class="sr-only">"Notes folder"</span>
                        <input prop:value=move || current.get() maxlength="500" aria-invalid=move || (!valid.get()).to_string() on:input=move |e| draft.set(Some(event_target_value(&e)))/>
                    </label>
                    <button class="secondary-button" disabled=move || !valid.get() || current.get() == value.get()>"Save"</button>
                </form>
                {move || (!valid.get()).then(|| view! {
                    <p class="field-error" role="alert">"Use folder names separated by /, like docs/notes."</p>
                })}
            </div>
        </div>
    }
}

/// Local Notes and Tasks settings sheet (`app-settings.tsx`), since `crate::settings::AppSettings`
/// currently handles Files, Contacts and Calendar only.
#[component]
fn WorkspaceSettings(app: &'static str, session: BrowserSession, pim: PimLive, on_close: Callback<()>) -> impl IntoView {
    let saver = Saver::new(session.clone());
    let section = RwSignal::new("General");
    let is_notes = app == "notes";
    let (name, icon) = if is_notes { ("Notes", "file") } else { ("Tasks", "check") };
    let text = move |key: &'static str, default: &'static str| {
        Signal::derive(move || saver.value(key).as_str().unwrap_or(default).to_owned())
    };
    let set = move |key: &'static str, value: Value, message: String| saver.change(key, value, message, true);
    let session = StoredValue::new(session);
    let tasks_view_sig = RwSignal::new("today".to_owned());
    Effect::new(move |_| {
        let v = saver.value("tasksView").as_str().unwrap_or("today").to_owned();
        tasks_view_sig.set(v);
    });
    let view_options = Signal::derive(|| {
        ["all", "inbox", "today", "upcoming", "important", "completed"]
            .into_iter()
            .map(|v| (v.to_owned(), capitalized(v)))
            .collect::<Vec<_>>()
    });
    view! {
        <SettingsSheet title=format!("{name} settings") icon=icon sections=vec!["General", "Import and export", "Keyboard shortcuts"] section=section on_close=on_close notice=saver.notice>
            <SaveProblems saver=saver/>
            {move || match section.get() {
                "Import and export" => view! {
                    <WorkspaceTransfers session=session.get_value() app=app/>
                    {is_notes.then(|| view! { <LogseqImport session=session.get_value() pim=pim/> })}
                }.into_any(),
                "Keyboard shortcuts" => view! {
                    <dl class="ui-shortcuts">
                        <dt>"Ctrl / Cmd K"</dt><dd>"Search all apps outside editors and dialogs"</dd>
                        <dt>"Tab / Shift Tab"</dt><dd>"Move between controls"</dd>
                        <dt>"Arrow keys"</dt><dd>"Move through menus and choices"</dd>
                        <dt>"Enter / Space"</dt><dd>"Activate the focused control"</dd>
                        <dt>"Escape"</dt><dd>"Close menus, then settings"</dd>
                        {is_notes.then(|| view! { <dt>"Ctrl / Cmd S"</dt><dd>"Save the active editor"</dd> })}
                    </dl>
                }.into_any(),
                _ if saver.preferences.with(Option::is_none) => view! { <p role="status">"Loading settings\u{2026}"</p> }.into_any(),
                _ => {
                    let sort_key = if is_notes { "notesSort" } else { "tasksSort" };
                    let default_sort = if is_notes { "modified" } else { "due" };
                    let sort_options = if is_notes {
                        vec![("modified", "Recently edited"), ("title", "Title")]
                    } else {
                        vec![("modified", "Recently edited"), ("title", "Title"), ("due", "Due date")]
                    };
                    view! {
                        {is_notes.then(|| view! {
                            <section class="settings-group" aria-label="Storage">
                                <h3>"Storage"</h3>
                                <NotesFolderSetting value=text("notesFolder", DEFAULT_NOTES_FOLDER) state=saver.state("notesFolder") on_save=Callback::new(move |v: String| {
                                    set("notesFolder", v.clone().into(), format!("Notes now live in {v}"));
                                })/>
                            </section>
                        })}
                        <section class="settings-group" aria-label="Lists">
                            <h3>"Lists"</h3>
                            <SettingRow label="Default sorting" hint=format!("Order of the {app} list") state=saver.state(sort_key) stacked=!is_notes>
                                <Segmented label="Default sorting" value=text(sort_key, default_sort) options=sort_options on_change=Callback::new(move |v: String| {
                                    let label = match v.as_str() { "title" => "title", "due" => "due date", _ => "recently edited" };
                                    set(sort_key, v.into(), format!("Sorted by {label}"));
                                })/>
                            </SettingRow>
                            {is_notes.then(|| view! {
                                <SettingRow label="Open notes in" hint="Live preview or reading view" state=saver.state("notesEditorMode")>
                                    <Segmented label="Open notes in" value=text("notesEditorMode", "write") options=vec![("write", "Live"), ("preview", "Reading")] on_change=Callback::new(move |v: String| {
                                        let label = if v == "write" { "Live" } else { "Reading" };
                                        set("notesEditorMode", v.into(), format!("Notes open in {label}"));
                                    })/>
                                </SettingRow>
                            })}
                            {(!is_notes).then(|| view! {
                                <SettingRow label="Default view" hint="The list Tasks opens on" state=saver.state("tasksView")>
                                    <ReactiveSelect aria_label="Default view" options=view_options value=tasks_view_sig on_change=Callback::new(move |v: String| {
                                        let label = capitalized(&v);
                                        set("tasksView", v.into(), format!("Tasks opens on {label}"));
                                    })/>
                                </SettingRow>
                            })}
                        </section>
                    }.into_any()
                }
            }}
        </SettingsSheet>
    }
}

#[derive(Deserialize)]
struct CollectionList {
    items: Vec<Collection>,
}

/// Import and export for Notes notebooks and Tasks lists (`collection-transfers.tsx`).
#[component]
fn WorkspaceTransfers(session: BrowserSession, app: &'static str) -> impl IntoView {
    let is_notes = app == "notes";
    let (endpoint, noun, plural, format, accept) = if is_notes {
        ("/api/v1/notes/notebooks", "Notebook", "notebooks", "Markdown or notebook JSON", ".md,.markdown,.json")
    } else {
        ("/api/v1/tasks/lists", "Task list", "task lists", "ICS", ".ics,text/calendar")
    };
    let collections: RwSignal<Vec<Collection>> = RwSignal::new(Vec::new());
    let selected = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let loaded = RwSignal::new(false);
    let results: RwSignal<Option<Vec<Value>>> = RwSignal::new(None);
    let content = RwSignal::new(String::new());
    let file = NodeRef::<html::Input>::new();
    let csrf = StoredValue::new(session.csrf_token.clone());

    spawn_local(async move {
        match api::get::<CollectionList>(endpoint).await {
            Ok(list) => {
                selected.set(list.items.first().map(|c| c.id.clone()).unwrap_or_default());
                collections.set(list.items);
            }
            Err(e) => error.set(e.message),
        }
        loaded.set(true);
    });

    let collection = Memo::new(move |_| collections.with(|c| c.iter().find(|x| x.id == selected.get()).cloned()));
    let read_only = Memo::new(move |_| collection.with(|c| c.as_ref().is_some_and(|c| c.role == "read")));
    let options = Signal::derive(move || collections.get().into_iter().map(|c| (c.id, c.name)).collect::<Vec<_>>());

    let import = move || {
        let Some(c) = collection.get_untracked() else { return };
        let source = file.get_untracked().and_then(|f| f.files()).and_then(|l| l.item(0));
        let pasted = content.get_untracked();
        if source.is_none() && pasted.is_empty() {
            return;
        }
        busy.set(true);
        error.set(String::new());
        results.set(None);
        let source = send_wrapper::SendWrapper::new(source);
        spawn_local(async move {
            let source = source.take();
            let outcome: Result<Vec<Value>, String> = async {
                let size = source.as_ref().map_or(pasted.len() as f64, |f| f.size());
                if size > 2.0 * 1024.0 * 1024.0 {
                    return Err("Import exceeds 2 MiB.".to_owned());
                }
                let source_name = source.as_ref().map(|f| f.name());
                let value = match &source {
                    Some(f) => wasm_bindgen_futures::JsFuture::from(f.text()).await.ok().and_then(|v| v.as_string()).unwrap_or_default(),
                    None => pasted,
                };
                if is_notes && !source_name.as_deref().is_some_and(|n| n.to_lowercase().ends_with(".json")) {
                    let title = source_name
                        .as_deref()
                        .map(|n| notes::strip_md_ext(n))
                        .unwrap_or_else(|| "Imported note".into());
                    let _: Value = send(
                        "/api/v1/notes/items",
                        "POST",
                        Some(json!({ "collectionId": c.id, "title": title, "content": value })),
                        &csrf.get_value(),
                        None,
                    )
                    .await
                    .map_err(|e| e.message)?;
                    Ok(vec![json!({})])
                } else {
                    let body = if is_notes {
                        serde_json::from_str::<Value>(&value).map_err(|e| e.to_string())?
                    } else {
                        json!({ "content": value })
                    };
                    let result: Value = send(&format!("{endpoint}/{}/import", c.id), "POST", Some(body), &csrf.get_value(), None)
                        .await
                        .map_err(|e| e.message)?;
                    Ok(result["items"].as_array().cloned().unwrap_or_default())
                }
            }
            .await;
            match outcome {
                Ok(items) => results.set(Some(items)),
                Err(e) => error.set(e),
            }
            busy.set(false);
        });
    };

    view! {
        <section>
            <h3>"Import and export"</h3>
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            {move || if !loaded.get() {
                view! { <p role="status">{format!("Loading {plural}\u{2026}")}</p> }.into_any()
            } else if collections.with(Vec::is_empty) {
                view! { <p>{format!("No {plural} available.")}</p> }.into_any()
            } else {
                view! {
                    <label>{noun}<ReactiveSelect aria_label=format!("{noun} for import or export") options=options value=selected disabled=Signal::derive(move || busy.get()) on_change=Callback::new(move |_| results.set(None))/></label>
                    {move || collection.get().map(|c| view! {
                        <a class="secondary-button" href=format!("{endpoint}/{}/export", c.id)>{format!("Export {}", if is_notes { "notebook" } else { format })}</a>
                    })}
                    <h4>{format!("Import {format}")}</h4>
                    <label>{format!("{format} file")}<input type="file" node_ref=file accept=accept disabled=move || busy.get() || read_only.get()/></label>
                    <label>{if is_notes { "Or paste Markdown" } else { "Or paste content" }}<textarea rows="6" prop:value=move || content.get() disabled=move || busy.get() || read_only.get() on:input=move |e| content.set(event_target_value(&e))></textarea></label>
                    <button type="button" class="primary-button" disabled=move || busy.get() || collection.with(Option::is_none) || read_only.get() on:click=move |_| import()>
                        {move || if busy.get() { "Importing\u{2026}".to_owned() } else { format!("Import {format}") }}
                    </button>
                    {(!is_notes).then(|| view! { <p class="field-hint">"Task lists share calendars with Calendar. ICS transfers include both events and tasks."</p> })}
                    {move || read_only.get().then(|| view! { <p class="field-hint">{format!("Import requires write access to this {}.", noun.to_lowercase())}</p> })}
                    {move || results.get().map(|items| {
                        let failed: Vec<(usize, String)> = items.iter().enumerate().filter_map(|(i, r)| r["error"].as_str().map(|e| (i, e.to_owned()))).collect();
                        view! {
                            <div role="status">
                                <p>{format!("{} imported; {} failed.", items.len() - failed.len(), failed.len())}</p>
                                {failed.into_iter().map(|(i, e)| view! { <p>{format!("Item {}: {e}", i + 1)}</p> }).collect_view()}
                            </div>
                        }
                    })}
                }.into_any()
            }}
        </section>
    }
}
