//! Settings sheet B (`settings-sheet.tsx`) and the per-app preference saver
//! (`usePreferenceSaver` and Files' part of `AppSettings` in `app-settings.tsx`).

use std::collections::HashMap;

use leptos::html;
use leptos::portal::Portal;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{self, BrowserSession};
use crate::icons::Icon;
use crate::shared::{PREFERENCES, Preferences, context, save_preferences};
use crate::ui::{active_element, unique_id, use_body_class};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SaveState {
    Saving,
    Saved,
    Error,
}

#[derive(Clone)]
pub struct Notice {
    pub message: String,
    pub undo: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

/// A right sheet in place of the details pane (a bottom sheet on phones).
#[component]
pub fn SettingsSheet(
    #[prop(into)] title: String,
    #[prop(into)] icon: &'static str,
    sections: Vec<&'static str>,
    section: RwSignal<&'static str>,
    on_close: Callback<()>,
    notice: RwSignal<Option<Notice>>,
    #[prop(optional, into)] footer: Option<String>,
    children: ChildrenFn,
) -> impl IntoView {
    use_body_class(&["has-editing-pane", "has-settings-sheet"]);
    let footer = footer.unwrap_or_else(|| "Changes save to your account as you make them.".into());
    let sheet = NodeRef::<html::Aside>::new();
    let tabs = NodeRef::<html::Div>::new();
    let (id, panel) = (unique_id(), unique_id());
    let previous = send_wrapper::SendWrapper::new(active_element());
    on_cleanup(move || {
        if let Some(p) = previous
            .take()
            .filter(|p| p.is_connected() && p.get_bounding_client_rect().width() > 0.0)
        {
            let _ = p.focus();
        }
    });
    Effect::new(move |_| {
        let Some(sheet) = sheet.get() else { return };
        let selected = tabs
            .get_untracked()
            .and_then(|t| t.query_selector("[aria-selected=\"true\"]").ok().flatten());
        let target: web_sys::HtmlElement = selected
            .map(wasm_bindgen::JsCast::unchecked_into)
            .unwrap_or_else(|| sheet);
        let _ = target.focus();
    });
    // A notice disappears after eight seconds.
    let shown = RwSignal::new(0u32);
    Effect::new(move |_| {
        if notice.with(Option::is_none) {
            return;
        }
        let id = shown.get_untracked() + 1;
        shown.set(id);
        spawn_local(async move {
            crate::sleep(8000).await;
            if shown.try_get_untracked() == Some(id) {
                notice.set(None);
            }
        });
    });
    let sections = StoredValue::new(sections);
    let many = sections.with_value(|s| s.len() > 1);
    let tabs_label = format!("{title} sections");
    let children = StoredValue::new(children);
    view! {
        <Portal>
            <aside node_ref=sheet role="dialog" aria-modal="false" aria-labelledby=id.clone() tabindex="-1" data-settings-sheet="" class="editing-pane settings-sheet"
                on:keydown=move |e| {
                    if e.key() != "Escape" || e.default_prevented() { return; }
                    let inside_open = e.target().and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok()).and_then(|t| t.closest("[aria-expanded=\"true\"],[role=\"listbox\"],[role=\"menu\"]").ok().flatten());
                    if inside_open.is_some() { return; }
                    e.prevent_default();
                    { on_close.try_run(()); };
                }>
                <span class="settings-sheet-handle" aria-hidden="true"></span>
                <div class="settings-sheet-heading"><span class="settings-sheet-icon" aria-hidden="true"><Icon name=icon/></span><h2 id=id.clone()>{title.clone()}</h2><button type="button" class="icon-button" aria-label="Close settings" on:click=move |_| { on_close.try_run(()); }><Icon name="close"/></button></div>
                {many.then(|| view! {
                    <div node_ref=tabs role="tablist" aria-label=tabs_label.clone() class="settings-tabs"
                        on:keydown=move |e| {
                            let list = sections.get_value();
                            let index = list.iter().position(|s| *s == section.get_untracked()).unwrap_or(0) as i64;
                            let last = list.len() as i64 - 1;
                            let next = match e.key().as_str() { "ArrowRight" => index + 1, "ArrowLeft" => index - 1, "Home" => 0, "End" => last, _ => return };
                            e.prevent_default();
                            let target = list[((next + list.len() as i64) % list.len() as i64) as usize];
                            section.set(target);
                            if let Some(tabs) = tabs.get_untracked()
                                && let Some(el) = tabs.query_selector(&format!("[data-section=\"{target}\"]")).ok().flatten() {
                                    let _ = wasm_bindgen::JsCast::unchecked_into::<web_sys::HtmlElement>(el).focus();
                                }
                        }>
                        {sections.get_value().into_iter().map(|item| view! {
                            <button type="button" role="tab" data-section=item aria-selected=move || (section.get() == item).to_string() aria-controls=panel.clone() tabindex=move || if section.get() == item { "0" } else { "-1" } on:click=move |_| section.set(item)>{item}</button>
                        }).collect_view()}
                    </div>
                })}
                <div id=panel.clone() role=many.then_some("tabpanel") aria-label=move || section.get() class="settings-sheet-content">{children.with_value(|c| c())}</div>
                <div class="settings-sheet-footer"><span>{footer.clone()}</span></div>
                <div class="settings-toast-region" role="status" aria-live="polite">
                    {move || notice.get().map(|n| view! {
                        <div class="settings-toast"><span>{n.message.clone()}</span>
                            {n.undo.map(|undo| view! { <button type="button" class="text-button" on:click=move |_| { notice.set(None); undo(); if let Some(el) = sheet.get_untracked() { let _ = el.focus(); } }>"Undo"</button> })}
                        </div>
                    })}
                </div>
            </aside>
        </Portal>
    }
}

#[component]
pub fn SaveStatus(#[prop(into)] state: Signal<Option<SaveState>>) -> impl IntoView {
    move || {
        state.get().map(|s| {
            let class = format!("settings-save-state is-{}", match s { SaveState::Saving => "saving", SaveState::Saved => "saved", SaveState::Error => "error" });
            view! {
                <span class=class aria-hidden="true">{match s {
                    SaveState::Saving => view! { <span class="settings-spinner"></span>"Saving…" }.into_any(),
                    SaveState::Saved => view! { <Icon name="check"/>"Saved" }.into_any(),
                    SaveState::Error => view! { "Not saved" }.into_any(),
                }}</span>
            }
        })
    }
}

/// Label and explanation on the left, the control on the right.
#[component]
pub fn SettingRow(
    #[prop(into)] label: String,
    #[prop(into)] hint: String,
    #[prop(into)] state: Signal<Option<SaveState>>,
    #[prop(optional)] stacked: bool,
    children: Children,
) -> impl IntoView {
    view! {
        <div class=format!("settings-row {}", if stacked { "is-stacked" } else { "" })>
            <div class="settings-row-text"><span class="settings-row-label">{label}</span><small>{hint}</small></div>
            <SaveStatus state=state/>
            <div class="settings-row-control">{children()}</div>
        </div>
    }
}

/// An on/off row: the whole row is the switch's label.
#[component]
pub fn SettingSwitch(
    #[prop(into)] label: String,
    #[prop(into)] hint: String,
    #[prop(into)] state: Signal<Option<SaveState>>,
    #[prop(into)] checked: Signal<bool>,
    on_change: Callback<bool>,
) -> impl IntoView {
    let id = unique_id();
    view! {
        <label class="settings-row settings-switch">
            <span class="settings-row-text"><span id=format!("{id}label") class="settings-row-label">{label}</span><small id=format!("{id}hint")>{hint}</small></span>
            <SaveStatus state=state/>
            <span class="ui-switch"><input type="checkbox" role="switch" aria-labelledby=format!("{id}label") aria-describedby=format!("{id}hint") prop:checked=move || checked.get() on:change=move |e| { on_change.try_run(event_target_checked(&e)); }/></span>
        </label>
    }
}

/// Two or three choices as a segmented radio group.
#[component]
pub fn Segmented(
    #[prop(into)] label: String,
    #[prop(into)] value: Signal<String>,
    options: Vec<(&'static str, &'static str)>,
    on_change: Callback<String>,
) -> impl IntoView {
    let name = unique_id();
    view! {
        <div role="radiogroup" aria-label=label class="ui-segmented">
            {options.into_iter().map(|(option, text)| {
                let name = name.clone();
                view! {
                    <label class=move || if value.get() == option { "is-checked" } else { "" }>
                        <input type="radio" name=name prop:checked=move || value.get() == option on:change=move |_| { on_change.try_run(option.to_owned()); }/><span>{text}</span>
                    </label>
                }
            }).collect_view()}
        </div>
    }
}

/// Saves one preference at a time, in order, with the account revision. A newer revision
/// only conflicts when the same value changed elsewhere; otherwise the save is retried.
#[derive(Clone, Copy)]
pub struct Saver {
    pub preferences: RwSignal<Option<Preferences>>,
    pub states: RwSignal<HashMap<&'static str, SaveState>>,
    pub error: RwSignal<String>,
    pub conflict: RwSignal<bool>,
    pub notice: RwSignal<Option<Notice>>,
    saved: RwSignal<Option<Preferences>>,
    session: StoredValue<BrowserSession>,
    queue: StoredValue<std::sync::Arc<futures::lock::Mutex<()>>>,
    shared: crate::shared::Shared<Preferences>,
}

fn field(p: &Preferences, key: &str) -> serde_json::Value {
    serde_json::to_value(p)
        .ok()
        .and_then(|v| v.get(key).cloned())
        .unwrap_or(serde_json::Value::Null)
}

impl Saver {
    pub fn new(session: BrowserSession) -> Self {
        let shared = context().preferences;
        let saver = Self {
            shared,
            preferences: RwSignal::new(None),
            states: RwSignal::new(HashMap::new()),
            error: RwSignal::new(String::new()),
            conflict: RwSignal::new(false),
            notice: RwSignal::new(None),
            saved: RwSignal::new(None),
            session: StoredValue::new(session),
            queue: StoredValue::new(Default::default()),
        };
        // The page has usually just loaded these preferences; reuse them rather than asking again.
        match shared
            .value
            .get_untracked()
            .filter(|_| shared.age() < 2000.0)
        {
            Some(fresh) => {
                saver.saved.set(Some(fresh.clone()));
                saver.preferences.set(Some(fresh));
            }
            None => saver.reload(),
        }
        saver
    }

    pub fn reload(self) {
        let queue = self.queue.get_value();
        spawn_local(async move {
            let _turn = queue.lock().await;
            match api::get::<Preferences>(PREFERENCES).await {
                Ok(value) => {
                    self.saved.try_set(Some(value.clone()));
                    self.preferences.try_set(Some(value));
                    self.conflict.try_set(false);
                    self.error.try_set(String::new());
                    self.states.try_set(HashMap::new());
                }
                Err(e) => {
                    self.error.try_set(e.message);
                }
            }
        });
    }

    pub fn state(self, key: &'static str) -> Signal<Option<SaveState>> {
        Signal::derive(move || self.states.with(|s| s.get(key).copied()))
    }

    pub fn value(self, key: &'static str) -> serde_json::Value {
        self.preferences.with(|p| {
            p.as_ref()
                .map(|p| field(p, key))
                .unwrap_or(serde_json::Value::Null)
        })
    }

    pub fn change(
        self,
        key: &'static str,
        value: serde_json::Value,
        message: String,
        undoable: bool,
    ) {
        let set_local = move |v: serde_json::Value| {
            self.preferences.try_update(|p| {
                if let Some(p) = p
                    && let Ok(serde_json::Value::Object(mut map)) = serde_json::to_value(&*p)
                {
                    map.insert(key.into(), v);
                    if let Ok(next) = serde_json::from_value(serde_json::Value::Object(map)) {
                        *p = next;
                    }
                }
            });
        };
        if let Some(saved) = self.saved.get_untracked() {
            if field(&saved, key) == value {
                return;
            }
            set_local(value.clone());
        }
        self.states.try_update(|s| {
            s.insert(key, SaveState::Saving);
        });
        self.error.try_set(String::new());
        self.notice.try_set(None);
        let queue = self.queue.get_value();
        spawn_local(async move {
            // Saves run in order: each waits for any initial load and previous save to finish.
            let _turn = queue.lock().await;
            let Some(session) = self.session.try_get_value() else {
                return;
            };
            let saved = match self.saved.try_get_untracked().flatten() {
                Some(s) => s,
                None => match api::get::<Preferences>(PREFERENCES).await {
                    Ok(latest) => {
                        self.saved.try_set(Some(latest.clone()));
                        self.preferences.try_set(Some(latest.clone()));
                        latest
                    }
                    Err(e) => {
                        self.states.try_update(|s| {
                            s.insert(key, SaveState::Error);
                        });
                        self.error.try_set(e.message);
                        return;
                    }
                },
            };
            let before = field(&saved, key);
            if before == value {
                self.states.try_update(|s| {
                    s.remove(key);
                });
                return;
            }
            set_local(value.clone());
            self.states.try_update(|s| {
                s.insert(key, SaveState::Saving);
            });
            // A missing value (undoing a first change) is left out, as JSON.stringify drops `undefined`.
            let body = |revision: u64| {
                if value.is_null() {
                    serde_json::json!({ "revision": revision })
                } else {
                    serde_json::json!({ "revision": revision, key: value.clone() })
                }
            };
            let revision = saved.revision;
            let mut result = save_preferences(body(revision), revision, &session).await;
            if let Err(e) = &result
                && e.status == 412
            {
                match api::get::<Preferences>(PREFERENCES).await {
                    Ok(latest) if field(&latest, key) == before => {
                        result =
                            save_preferences(body(latest.revision), latest.revision, &session).await
                    }
                    Ok(latest) => {
                        self.saved.try_set(Some(latest));
                    }
                    Err(_) => {}
                }
            }
            match result {
                Ok(next) => {
                    self.saved.try_set(Some(next.clone()));
                    // The rest of the page (the theme, other apps) follows the saved preferences at once.
                    self.shared.value.try_set(Some(next.clone()));
                    self.preferences.try_update(|p| {
                        if let Some(p) = p {
                            p.revision = next.revision;
                        }
                    });
                    self.states.try_update(|s| {
                        s.insert(key, SaveState::Saved);
                    });
                    let undo = undoable.then(|| {
                        std::sync::Arc::new(move || {
                            self.change(key, before.clone(), "Change undone".into(), false)
                        }) as std::sync::Arc<dyn Fn() + Send + Sync>
                    });
                    self.notice.try_set(Some(Notice { message, undo }));
                }
                Err(e) => {
                    let previous = self
                        .saved
                        .try_with_untracked(|s| s.as_ref().map(|s| field(s, key)))
                        .flatten()
                        .unwrap_or(serde_json::Value::Null);
                    set_local(previous);
                    self.states.try_update(|s| {
                        s.insert(key, SaveState::Error);
                    });
                    if e.status == 412 {
                        self.conflict.try_set(true);
                    } else {
                        self.error.try_set(e.message);
                    }
                }
            }
        });
    }
}

/// Conflict and error banners shared by every settings sheet.
#[component]
pub fn SaveProblems(saver: Saver) -> impl IntoView {
    view! {
        {move || (!saver.error.get().is_empty()).then(|| view! { <p role="alert" class="error settings-problem">{saver.error.get()}</p> })}
        {move || saver.conflict.get().then(|| view! {
            <div role="alert" class="settings-problem settings-conflict"><p><strong>"Changed in another tab"</strong>"This setting was saved elsewhere. Reload to see it before changing it again."</p>
                <button type="button" class="text-button" on:click=move |_| saver.reload()>"Reload settings"</button></div>
        })}
    }
}

#[component]
pub fn AppSettingsButton(#[prop(into)] app: String, on_click: Callback<()>) -> impl IntoView {
    let name = if app == "prayers" {
        "Prayer Times settings".to_owned()
    } else {
        format!("{}{} settings", app[..1].to_uppercase(), &app[1..])
    };
    view! { <button type="button" class="nav-item ui-app-settings-entry" on:click=move |_| { on_click.try_run(()); }><Icon name="settings"/>{name}</button> }
}

/// Files settings: browsing, safety and details.
#[component]
pub fn FilesSettings(session: BrowserSession, on_close: Callback<()>) -> impl IntoView {
    let saver = Saver::new(session);
    let section = RwSignal::new("General");
    let text = move |key: &'static str, default: &'static str| {
        Signal::derive(move || saver.value(key).as_str().unwrap_or(default).to_owned())
    };
    let flag = move |key: &'static str, default: bool| {
        Signal::derive(move || saver.value(key).as_bool().unwrap_or(default))
    };
    let set = move |key: &'static str, value: serde_json::Value, message: String| {
        saver.change(key, value, message, true)
    };
    let capital = |v: &str| format!("{}{}", v[..1].to_uppercase(), &v[1..]);
    view! {
        <SettingsSheet title="Files settings" icon="folder" sections=vec!["General", "Keyboard shortcuts"] section=section on_close=on_close notice=saver.notice>
            <SaveProblems saver=saver/>
            {move || if section.get() == "Keyboard shortcuts" {
                view! {
                    <dl class="ui-shortcuts"><dt>"Ctrl / Cmd K"</dt><dd>"Search all apps outside editors and dialogs"</dd><dt>"Tab / Shift Tab"</dt><dd>"Move between controls"</dd><dt>"Arrow keys"</dt><dd>"Move through menus and choices"</dd><dt>"Enter / Space"</dt><dd>"Activate the focused control"</dd><dt>"Escape"</dt><dd>"Close menus, then settings"</dd>
                        <dt>"Ctrl / Cmd S"</dt><dd>"Save the active editor"</dd><dt>"Ctrl / Cmd C, X, V"</dt><dd>"Copy, cut and paste the selected file"</dd><dt>"F2"</dt><dd>"Rename the selected file"</dd></dl>
                }.into_any()
            } else if saver.preferences.with(Option::is_none) {
                view! { <p role="status">"Loading settings…"</p> }.into_any()
            } else {
                view! {
                    <section class="settings-group" aria-label="Browsing"><h3>"Browsing"</h3>
                        <SettingRow label="Default view" hint="How folders open" state=saver.state("filesView")>
                            <Segmented label="Default view" value=text("filesView", "list") options=vec![("list", "List"), ("grid", "Grid")] on_change=Callback::new(move |v: String| set("filesView", v.clone().into(), format!("Default view set to {}", if v == "grid" { "Grid" } else { "List" })))/>
                        </SettingRow>
                        <SettingSwitch label="Show hidden files" hint="Names starting with a dot" state=saver.state("filesShowHidden") checked=flag("filesShowHidden", false) on_change=Callback::new(move |v: bool| set("filesShowHidden", v.into(), if v { "Hidden files shown" } else { "Hidden files hidden" }.into()))/>
                    </section>
                    <section class="settings-group" aria-label="Safety"><h3>"Safety"</h3>
                        <SettingSwitch label="Confirm before moving files to trash" hint="Deleted files can always be restored" state=saver.state("filesConfirmTrash") checked=flag("filesConfirmTrash", false) on_change=Callback::new(move |v: bool| set("filesConfirmTrash", v.into(), if v { "Trash confirmation on" } else { "Trash confirmation off" }.into()))/>
                    </section>
                    <section class="settings-group" aria-label="Details"><h3>"Details"</h3>
                        <SettingRow label="Default details tab" hint="Opens first when you choose Details. Folders show History in place of Versions." state=saver.state("filesDetailsTab") stacked=true>
                            <Segmented label="Default details tab" value=text("filesDetailsTab", "details") options=vec![("details", "Details"), ("versions", "Versions"), ("sharing", "Sharing")] on_change=Callback::new(move |v: String| set("filesDetailsTab", v.clone().into(), format!("Details open on {}", capital(&v))))/>
                        </SettingRow>
                    </section>
                }.into_any()
            }}
        </SettingsSheet>
    }
}

/// The settings sheet of an app (`AppSettings`).
#[component]
pub fn AppSettings(
    app: &'static str,
    session: BrowserSession,
    on_close: Callback<()>,
) -> impl IntoView {
    if app == "files" {
        return view! { <FilesSettings session=session on_close=on_close/> }.into_any();
    }
    let saver = Saver::new(session.clone());
    let section = RwSignal::new("General");
    let (name, icon) = if app == "contacts" {
        ("Contacts", "contacts")
    } else {
        ("Calendar", "calendar")
    };
    let text = move |key: &'static str, default: &'static str| {
        Signal::derive(move || {
            let v = saver.value(key);
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
                .unwrap_or_else(|| default.to_owned())
        })
    };
    let flag = move |key: &'static str, default: bool| {
        Signal::derive(move || saver.value(key).as_bool().unwrap_or(default))
    };
    let set = move |key: &'static str, value: serde_json::Value, message: String| {
        saver.change(key, value, message, true)
    };
    let session = StoredValue::new(session);
    view! {
        <SettingsSheet title=format!("{name} settings") icon=icon sections=vec!["General", "Import and export", "Keyboard shortcuts"] section=section on_close=on_close notice=saver.notice>
            <SaveProblems saver=saver/>
            {move || match section.get() {
                "Import and export" => view! { <crate::pim::transfers::CollectionTransfers session=session.get_value() app=app/> }.into_any(),
                "Keyboard shortcuts" => {
                    if app == "calendar" {
                        view! {
                            <dl class="ui-shortcuts">
                                <dt>"C"</dt><dd>"Create a new event"</dd>
                                <dt>"T"</dt><dd>"Jump to today"</dd>
                                <dt>"D / W / M / Y / A"</dt><dd>"Switch to Day, Week, Month, Year or Agenda view"</dd>
                                <dt>"J / K"</dt><dd>"Next or previous period"</dd>
                                <dt>"Ctrl / Cmd K"</dt><dd>"Search all apps outside editors and dialogs"</dd>
                                <dt>"Escape"</dt><dd>"Close popovers, editors or settings"</dd>
                            </dl>
                        }.into_any()
                    } else if app == "contacts" {
                        view! {
                            <dl class="ui-shortcuts">
                                <dt>"C"</dt><dd>"Create a new contact"</dd>
                                <dt>"J / K"</dt><dd>"Next or previous contact"</dd>
                                <dt>"E"</dt><dd>"Edit the selected contact"</dd>
                                <dt>"F"</dt><dd>"Toggle favorite on the selected contact"</dd>
                                <dt>"X"</dt><dd>"Select or deselect the current contact"</dd>
                                <dt>"Ctrl / Cmd K"</dt><dd>"Search all apps outside editors and dialogs"</dd>
                                <dt>"Escape"</dt><dd>"Clear selection, close details or settings"</dd>
                            </dl>
                        }.into_any()
                    } else {
                        view! {
                            <dl class="ui-shortcuts"><dt>"Ctrl / Cmd K"</dt><dd>"Search all apps outside editors and dialogs"</dd><dt>"Tab / Shift Tab"</dt><dd>"Move between controls"</dd><dt>"Arrow keys"</dt><dd>"Move through menus and choices"</dd><dt>"Enter / Space"</dt><dd>"Activate the focused control"</dd><dt>"Escape"</dt><dd>"Close menus, then settings"</dd></dl>
                        }.into_any()
                    }
                }
                _ if saver.preferences.with(Option::is_none) => view! { <p role="status">"Loading settings…"</p> }.into_any(),
                _ if app == "contacts" => {
                    let dav_url = crate::ui::window()
                        .location()
                        .origin()
                        .map(|o| format!("{o}/dav/"))
                        .unwrap_or_else(|_| "/dav/".to_owned());
                    let dav_copied = RwSignal::new(false);
                    let dav_copy_url = dav_url.clone();
                    view! {
                        <section class="settings-group" aria-label="Display"><h3>"Display"</h3>
                            <SettingRow label="Sort contacts by" hint="Order of the contact list" state=saver.state("contactSort") stacked=true>
                                <Segmented label="Sort contacts by" value=text("contactSort", "firstName") options=vec![("firstName", "First name"), ("lastName", "Last name"), ("organization", "Organization")] on_change=Callback::new(move |v: String| set("contactSort", v.clone().into(), match v.as_str() { "lastName" => "Contacts sorted by last name".into(), "organization" => "Contacts sorted by organization".into(), _ => "Contacts sorted by first name".into() }))/>
                            </SettingRow>
                            <SettingRow label="Display format" hint="How names appear in lists and headers" state=saver.state("contactNameOrder")>
                                <Segmented label="Display format" value=text("contactNameOrder", "firstLast") options=vec![("firstLast", "First Last"), ("lastFirst", "Last, First")] on_change=Callback::new(move |v: String| set("contactNameOrder", v.clone().into(), if v == "lastFirst" { "Names shown as Last, First" } else { "Names shown as First Last" }.into()))/>
                            </SettingRow>
                            <SettingRow label="Default view" hint="Layout used when opening Contacts" state=saver.state("contactView") stacked=true>
                                <Segmented label="Default view" value=text("contactView", "list") options=vec![("list", "List"), ("table", "Table"), ("cards", "Cards")] on_change=Callback::new(move |v: String| set("contactView", v.clone().into(), format!("Default view set to {}", match v.as_str() { "table" => "Table", "cards" => "Cards", _ => "List" })))/>
                            </SettingRow>
                            <SettingSwitch label="Show upcoming birthdays" hint="Display upcoming birthdays at the bottom of the sidebar" state=saver.state("contactShowBirthdays") checked=flag("contactShowBirthdays", true) on_change=Callback::new(move |v: bool| set("contactShowBirthdays", v.into(), if v { "Upcoming birthdays shown" } else { "Upcoming birthdays hidden" }.into()))/>
                        </section>
                        <section class="settings-group" aria-label="Sync"><h3>"CardDAV sync"</h3>
                            <div class="contacts-dav-box">
                                <p class="field-hint">"Use this server address with DAVx⁵, Apple Contacts or Thunderbird, together with an app password."</p>
                                <div class="contacts-dav-row">
                                    <code class="contacts-dav-url">{dav_url}</code>
                                    <button type="button" class="secondary-button" on:click=move |_| {
                                        let url = dav_copy_url.clone();
                                        let cb = crate::ui::window().navigator().clipboard();
                                        if !cb.is_undefined() && !cb.is_null() {
                                            let p = cb.write_text(&url);
                                            leptos::task::spawn_local(async move {
                                                let _ = wasm_bindgen_futures::JsFuture::from(p).await;
                                                dav_copied.try_set(true);
                                                crate::sleep(1800).await;
                                                dav_copied.try_set(false);
                                            });
                                        }
                                    }>{move || if dav_copied.get() { "Copied" } else { "Copy URL" }}</button>
                                </div>
                                <a class="text-button" href="/settings?section=security">"Manage app passwords"</a>
                            </div>
                        </section>
                    }.into_any()
                }
                _ => {
                    let zone = text("timezone", "UTC");
                    view! {
                        <section class="settings-group" aria-label="Display"><h3>"Display"</h3>
                            <SettingRow label="Week begins" hint="Month and Week views and the date picker" state=saver.state("weekStart")>
                                <Segmented label="Week begins" value=text("weekStart", "1") options=vec![("1", "Monday"), ("0", "Sunday")] on_change=Callback::new(move |v: String| set("weekStart", serde_json::json!(if v == "0" { 0 } else { 1 }), format!("Week begins on {}", if v == "0" { "Sunday" } else { "Monday" })))/>
                            </SettingRow>
                            <SettingSwitch label="Show cancelled events" hint="Shown struck through instead of hidden" state=saver.state("calendarShowCancelled") checked=flag("calendarShowCancelled", false) on_change=Callback::new(move |v: bool| set("calendarShowCancelled", v.into(), if v { "Cancelled events shown" } else { "Cancelled events hidden" }.into()))/>
                            <SettingSwitch label="Show tasks" hint="Dated tasks appear at their due time" state=saver.state("calendarShowTasks") checked=flag("calendarShowTasks", true) on_change=Callback::new(move |v: bool| set("calendarShowTasks", v.into(), if v { "Tasks shown" } else { "Tasks hidden" }.into()))/>
                        </section>
                        <section class="settings-group" aria-label="Time"><h3>"Time"</h3>
                            <SettingRow label="Time format" hint="How hours appear on grids and event cards" state=saver.state("calendarTimeFormat")>
                                <Segmented label="Time format" value=text("calendarTimeFormat", "24h") options=vec![("24h", "24-hour"), ("12h", "12-hour")] on_change=Callback::new(move |v: String| set("calendarTimeFormat", v.clone().into(), format!("Time format set to {}", if v == "12h" { "12-hour" } else { "24-hour" })))/>
                            </SettingRow>
                            <SettingRow label="Timezone" hint=format!("{} · events with their own timezone keep it", zone.get()) state=Signal::derive(|| None)>
                                <a class="secondary-button" href="/settings?section=regional">"Regional settings"</a>
                            </SettingRow>
                        </section>
                    }.into_any()
                }
            }}
        </SettingsSheet>
    }
    .into_any()
}
