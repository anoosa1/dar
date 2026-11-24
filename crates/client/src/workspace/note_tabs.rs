//! Open notes as tabs in the note sheet (Obsidian's tabs and split): one or two panes of tabs,
//! their tab strips, which editors stay mounted while their tab is in the background, and the
//! tab set remembered for the browser tab (sessionStorage).

use std::collections::{HashMap, HashSet};

use leptos::html;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;

use super::notes::strip_md_ext;
use crate::files::FileEntry;
use crate::icons::Icon;
use crate::ui::{ActionMenu, MenuAction, document, window};

/// Editors kept mounted, the shown ones included; beyond this the longest-unseen clean ones
/// unmount and load again when their tab is shown.
const MOUNTED: usize = 8;
/// Tabs remembered for the browser tab.
const REMEMBERED: usize = 20;

/// Where a note opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OpenIn {
    /// In the pane's active tab (a new tab when that one has unsaved changes).
    Current,
    NewTab,
    /// In the right pane, splitting the sheet.
    Right,
}

impl OpenIn {
    /// List rows and Reading's links: Ctrl/Cmd (or the middle button) opens a new tab,
    /// Ctrl/Cmd+Alt opens it to the right.
    pub(super) fn from_click(e: &web_sys::MouseEvent) -> Self {
        let modifier = e.ctrl_key() || e.meta_key();
        if modifier && e.alt_key() {
            Self::Right
        } else if modifier || e.button() == 1 {
            Self::NewTab
        } else {
            Self::Current
        }
    }

    /// Live preview follows links with Ctrl/Cmd+click, so a new tab takes Shift as well.
    pub(super) fn from_live_click(e: &web_sys::MouseEvent) -> Self {
        let modifier = e.ctrl_key() || e.meta_key();
        if modifier && e.alt_key() {
            Self::Right
        } else if (modifier && e.shift_key()) || e.button() == 1 {
            Self::NewTab
        } else {
            Self::Current
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct NoteTab {
    pub entry: FileEntry,
    /// 0 is the left (or only) pane, 1 the right one.
    pub pane: usize,
    /// Whether its editor is in the page.
    pub mounted: bool,
    /// When it was last shown: a closed tab's pane falls back to the latest, and unmounting
    /// takes the oldest.
    pub shown: u64,
    /// When it opened: editors stay in this order in the page, so they never move.
    pub opened: u64,
}

/// The tab set remembered for the browser tab.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedTabs {
    #[serde(default)]
    pub tabs: Vec<(String, usize)>,
    #[serde(default)]
    pub active: [Option<String>; 2],
    #[serde(default)]
    pub focus: usize,
}

pub(super) fn tab_id(note: &str) -> String {
    format!("note-tab-{note}")
}

pub(super) fn panel_id(note: &str) -> String {
    format!("note-panel-{note}")
}

/// Focuses a note's tab in the strip.
pub(super) fn focus_tab(note: &str) {
    if let Some(el) = document().get_element_by_id(&tab_id(note)).and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
        let _ = el.focus();
    }
}

fn storage() -> Option<web_sys::Storage> {
    window().session_storage().ok().flatten()
}

/// The remembered tab set (empty when sessionStorage is unavailable).
pub(super) fn load(key: &str) -> SavedTabs {
    let mut saved: SavedTabs = storage()
        .and_then(|s| s.get_item(key).ok().flatten())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let mut seen = HashSet::new();
    saved.tabs.retain(|(id, _)| seen.insert(id.clone()));
    saved.tabs.truncate(REMEMBERED);
    saved
}

pub(super) fn save(key: &str, saved: &SavedTabs) {
    let Some(storage) = storage() else { return };
    if saved.tabs.is_empty() {
        let _ = storage.remove_item(key);
    } else if let Ok(raw) = serde_json::to_string(saved) {
        let _ = storage.set_item(key, &raw);
    }
}

/// The open notes. Every method reads and writes with `try_`, so deferred code may call it.
#[derive(Clone, Copy)]
pub(super) struct NoteTabs {
    pub tabs: RwSignal<Vec<NoteTab>>,
    /// Each pane's shown tab.
    pub active: RwSignal<[Option<String>; 2]>,
    /// The pane last used: its shown note is the one in the URL and selected in the list.
    pub focus: RwSignal<usize>,
    /// Tabs whose editor has unsaved changes or a save in progress.
    pub dirty: RwSignal<HashSet<String>>,
    /// Close requests per note, counted, so a mounted editor runs its keep/discard flow.
    pub close: RwSignal<HashMap<String, u32>>,
    clock: StoredValue<u64>,
    dragging: StoredValue<Option<String>>,
}

impl NoteTabs {
    pub(super) fn new() -> Self {
        Self {
            tabs: RwSignal::new(Vec::new()),
            active: RwSignal::new([None, None]),
            focus: RwSignal::new(0),
            dirty: RwSignal::new(HashSet::new()),
            close: RwSignal::new(HashMap::new()),
            clock: StoredValue::new(0),
            dragging: StoredValue::new(None),
        }
    }

    fn tick(self) -> u64 {
        self.clock
            .try_update_value(|c| {
                *c += 1;
                *c
            })
            .unwrap_or(0)
    }

    fn find<U>(self, id: &str, f: impl FnOnce(&NoteTab) -> U) -> Option<U> {
        self.tabs.try_with_untracked(|list| list.iter().find(|t| t.entry.id == id).map(f)).flatten()
    }

    pub(super) fn contains(self, id: &str) -> bool {
        self.find(id, |_| ()).is_some()
    }

    pub(super) fn entry(self, id: &str) -> Option<FileEntry> {
        self.find(id, |t| t.entry.clone())
    }

    pub(super) fn mounted(self, id: &str) -> bool {
        self.find(id, |t| t.mounted).unwrap_or(false)
    }

    pub(super) fn is_dirty(self, id: &str) -> bool {
        self.dirty.try_with_untracked(|d| d.contains(id)).unwrap_or(false)
    }

    fn pane_of(self, id: &str) -> Option<usize> {
        self.find(id, |t| t.pane)
    }

    fn count(self, pane: usize) -> usize {
        self.tabs.try_with_untracked(|list| list.iter().filter(|t| t.pane == pane).count()).unwrap_or(0)
    }

    pub(super) fn active_in(self, pane: usize) -> Option<String> {
        self.active.try_with_untracked(|a| a[pane.min(1)].clone()).flatten()
    }

    pub(super) fn focused(self) -> usize {
        self.focus.try_get_untracked().unwrap_or(0)
    }

    /// The focused pane's note (tracked).
    pub(super) fn current(self) -> Option<FileEntry> {
        let pane = self.focus.get();
        let id = self.active.with(|a| a[pane].clone())?;
        self.tabs.with(|list| list.iter().find(|t| t.entry.id == id).map(|t| t.entry.clone()))
    }

    /// Whether the sheet shows two panes (tracked).
    pub(super) fn split(self) -> bool {
        self.tabs.with(|list| list.iter().any(|t| t.pane == 1))
    }

    /// Whether any note is open (tracked).
    pub(super) fn any(self) -> bool {
        self.tabs.with(|list| !list.is_empty())
    }

    /// A pane's tabs in strip order (tracked).
    pub(super) fn ids(self, pane: usize) -> Vec<String> {
        self.tabs.with(|list| list.iter().filter(|t| t.pane == pane).map(|t| t.entry.id.clone()).collect())
    }

    /// The notes whose editor is in the page, in the order they opened (tracked).
    pub(super) fn mounted_ids(self) -> Vec<String> {
        self.tabs.with(|list| {
            let mut open: Vec<(u64, String)> = list.iter().filter(|t| t.mounted).map(|t| (t.opened, t.entry.id.clone())).collect();
            open.sort();
            open.into_iter().map(|(_, id)| id).collect()
        })
    }

    /// What to remember (tracked).
    pub(super) fn snapshot(self) -> SavedTabs {
        SavedTabs {
            tabs: self.tabs.with(|list| list.iter().map(|t| (t.entry.id.clone(), t.pane)).collect()),
            active: self.active.get(),
            focus: self.focus.get(),
        }
    }

    fn set_active(self, pane: usize, id: Option<String>) {
        if self.active.try_with_untracked(|a| a[pane] != id) == Some(true) {
            self.active.try_update(|a| a[pane] = id);
        }
    }

    fn set_focus(self, pane: usize) {
        if self.focus.try_get_untracked().is_some_and(|f| f != pane) {
            self.focus.try_set(pane);
        }
    }

    /// Makes a pane the focused one (the right pane only while it exists).
    pub(super) fn focus_pane(self, pane: usize) {
        if pane == 0 || self.count(1) > 0 {
            self.set_focus(pane.min(1));
        }
    }

    fn mount(self, id: &str) {
        if self.find(id, |t| !t.mounted) == Some(true) {
            self.tabs.try_update(|list| {
                if let Some(t) = list.iter_mut().find(|t| t.entry.id == id) {
                    t.mounted = true;
                }
            });
        }
    }

    fn latest(self, pane: usize) -> Option<String> {
        self.tabs
            .try_with_untracked(|list| list.iter().filter(|t| t.pane == pane).max_by_key(|t| t.shown).map(|t| t.entry.id.clone()))
            .flatten()
    }

    /// Shows a tab in its pane and focuses that pane.
    pub(super) fn show(self, id: &str) {
        let Some(pane) = self.pane_of(id) else { return };
        let now = self.tick();
        self.tabs.try_update(|list| {
            if let Some(t) = list.iter_mut().find(|t| t.entry.id == id) {
                t.shown = now;
                t.mounted = true;
            }
        });
        self.set_active(pane, Some(id.to_owned()));
        self.set_focus(pane);
        self.trim();
    }

    /// Opens a note. One already open is shown where it is (`Right` moves it to the right pane);
    /// `Current` replaces the pane's shown tab unless that one has unsaved changes.
    pub(super) fn open(self, entry: FileEntry, how: OpenIn) {
        let id = entry.id.clone();
        if self.contains(&id) {
            self.update_entry(entry);
            if how == OpenIn::Right && self.pane_of(&id) == Some(0) && self.count(0) > 1 {
                self.move_tab(&id, 1, None);
            }
            self.show(&id);
            return;
        }
        let pane = if how == OpenIn::Right && self.count(0) > 0 { 1 } else { self.focused() };
        let active = self.active_in(pane);
        let replace = how == OpenIn::Current && active.as_deref().is_some_and(|a| !self.is_dirty(a));
        let now = self.tick();
        let tab = NoteTab { entry, pane, mounted: true, shown: now, opened: now };
        self.tabs.try_update(move |list| {
            let at = active.as_deref().and_then(|a| list.iter().position(|t| t.entry.id == a));
            match at {
                Some(i) if replace => list[i] = tab,
                Some(i) => list.insert(i + 1, tab),
                None => list.push(tab),
            }
        });
        self.set_active(pane, Some(id));
        self.set_focus(pane);
        self.trim();
    }

    /// Takes a renamed or moved note's entry.
    pub(super) fn update_entry(self, entry: FileEntry) {
        if self.find(&entry.id, |t| t.entry != entry) == Some(true) {
            let id = entry.id.clone();
            self.tabs.try_update(move |list| {
                if let Some(t) = list.iter_mut().find(|t| t.entry.id == id) {
                    t.entry = entry;
                }
            });
        }
    }

    /// Asks a mounted editor to close its tab (it keeps unsaved text until the person decides).
    pub(super) fn request_close(self, id: &str) {
        self.close.try_update(|requests| *requests.entry(id.to_owned()).or_insert(0) += 1);
    }

    /// Closes a tab now; its pane shows the tab seen last.
    pub(super) fn remove(self, id: &str) {
        let Some(pane) = self.pane_of(id) else { return };
        self.tabs.try_update(|list| list.retain(|t| t.entry.id != id));
        if self.is_dirty(id) {
            self.dirty.try_update(|d| {
                d.remove(id);
            });
        }
        if self.active_in(pane).as_deref() == Some(id) {
            let next = self.latest(pane);
            if let Some(next) = &next {
                self.mount(next);
            }
            self.set_active(pane, next);
        }
        self.normalize();
        self.trim();
    }

    /// The right pane exists only beside a left one.
    fn normalize(self) {
        if self.count(0) == 0 && self.count(1) > 0 {
            self.tabs.try_update(|list| list.iter_mut().for_each(|t| t.pane = 0));
            let right = self.active_in(1);
            self.active.try_set([right, None]);
            self.set_focus(0);
        } else if self.count(1) == 0 {
            self.set_active(1, None);
            self.set_focus(0);
        }
    }

    /// Moves a tab to a pane, before another tab or at the end, and shows it. A lone tab stays.
    pub(super) fn move_tab(self, id: &str, pane: usize, before: Option<&str>) {
        let Some(from) = self.pane_of(id) else { return };
        let pane = pane.min(1);
        if from == 0 && pane == 1 && self.count(0) == 1 {
            return;
        }
        self.tabs.try_update(|list| {
            let Some(i) = list.iter().position(|t| t.entry.id == id) else { return };
            let mut tab = list.remove(i);
            tab.pane = pane;
            let at = before
                .filter(|b| *b != id)
                .and_then(|b| list.iter().position(|t| t.entry.id == b))
                .or_else(|| list.iter().rposition(|t| t.pane == pane).map(|p| p + 1))
                .unwrap_or(list.len());
            list.insert(at, tab);
        });
        if from != pane && self.active_in(from).as_deref() == Some(id) {
            let next = self.latest(from);
            if let Some(next) = &next {
                self.mount(next);
            }
            self.set_active(from, next);
        }
        self.normalize();
        self.show(id);
    }

    /// Shows the next (or previous) tab of a pane, wrapping around; returns it.
    pub(super) fn cycle(self, pane: usize, step: isize) -> Option<String> {
        let ids = self.tabs.try_with_untracked(|list| list.iter().filter(|t| t.pane == pane).map(|t| t.entry.id.clone()).collect::<Vec<_>>())?;
        if ids.len() < 2 {
            return None;
        }
        let at = self.active_in(pane).and_then(|a| ids.iter().position(|i| *i == a)).unwrap_or(0) as isize;
        let next = ids[(at + step).rem_euclid(ids.len() as isize) as usize].clone();
        self.show(&next);
        Some(next)
    }

    pub(super) fn set_dirty(self, id: &str, dirty: bool) {
        if self.is_dirty(id) == dirty || (dirty && !self.contains(id)) {
            return;
        }
        self.dirty.try_update(|d| {
            if dirty {
                d.insert(id.to_owned());
            } else {
                d.remove(id);
            }
        });
        if !dirty {
            self.trim();
        }
    }

    /// Unmounts the longest-unseen clean background editors beyond the limit.
    fn trim(self) {
        let actives = self.active.try_get_untracked().unwrap_or_default();
        let dirty = self.dirty.try_get_untracked().unwrap_or_default();
        let spare: Vec<String> = self
            .tabs
            .try_with_untracked(|list| {
                let mounted = list.iter().filter(|t| t.mounted).count();
                let mut idle: Vec<&NoteTab> = list
                    .iter()
                    .filter(|t| t.mounted && !dirty.contains(&t.entry.id) && !actives.iter().any(|a| a.as_deref() == Some(t.entry.id.as_str())))
                    .collect();
                idle.sort_by_key(|t| t.shown);
                idle.into_iter().take(mounted.saturating_sub(MOUNTED)).map(|t| t.entry.id.clone()).collect::<Vec<String>>()
            })
            .unwrap_or_default();
        if !spare.is_empty() {
            self.tabs.try_update(|list| list.iter_mut().filter(|t| spare.contains(&t.entry.id)).for_each(|t| t.mounted = false));
        }
    }

    /// Brings back a remembered tab set (only into an empty sheet); the shown tabs mount.
    pub(super) fn restore(self, entries: Vec<(FileEntry, usize)>, saved: &SavedTabs) {
        if entries.is_empty() || self.tabs.try_with_untracked(|list| !list.is_empty()).unwrap_or(true) {
            return;
        }
        let mut list: Vec<NoteTab> = entries
            .into_iter()
            .map(|(entry, pane)| {
                let now = self.tick();
                NoteTab { entry, pane: pane.min(1), mounted: false, shown: now, opened: now }
            })
            .collect();
        if !list.iter().any(|t| t.pane == 0) {
            list.iter_mut().for_each(|t| t.pane = 0);
        }
        let mut active: [Option<String>; 2] = [None, None];
        for (pane, slot) in active.iter_mut().enumerate() {
            let wanted = saved.active[pane].as_deref();
            let pick = list
                .iter()
                .position(|t| t.pane == pane && Some(t.entry.id.as_str()) == wanted)
                .or_else(|| list.iter().position(|t| t.pane == pane));
            if let Some(i) = pick {
                let now = self.tick();
                list[i].mounted = true;
                list[i].shown = now;
                *slot = Some(list[i].entry.id.clone());
            }
        }
        let focus = if active[1].is_some() { saved.focus.min(1) } else { 0 };
        self.tabs.try_set(list);
        self.active.try_set(active);
        self.set_focus(focus);
    }

    /// After a tab closed: focus the tab shown in its place, unless focus already moved on.
    pub(super) fn refocus(self) {
        request_animation_frame(move || {
            let active = document().active_element();
            let body: Option<web_sys::Element> = document().body().map(|b| b.unchecked_into());
            if active.is_some() && active != body {
                return;
            }
            if let Some(id) = self.active_in(self.focused()) {
                focus_tab(&id);
            }
        });
    }

    fn drag(self, id: Option<String>) {
        self.dragging.try_update_value(|d| *d = id);
    }

    fn dragged(self) -> Option<String> {
        self.dragging.try_with_value(Clone::clone).flatten()
    }
}

/// A pane's tab strip: the tabs (a roving tablist: arrows, Home/End, Delete closes; middle-click
/// closes), drag to reorder or move between panes, and the shown tab's options.
#[component]
pub(super) fn NoteTabStrip(tabs: NoteTabs, pane: usize, on_close: Callback<String>) -> impl IntoView {
    let bar = NodeRef::<html::Div>::new();
    let ids = Memo::new(move |_| tabs.ids(pane));
    let label = if pane == 0 { "Open notes" } else { "Open notes on the right" };
    let keys = move |e: web_sys::KeyboardEvent| {
        let list = ids.get_untracked();
        let Some(current) = tabs.active_in(pane) else { return };
        let Some(at) = list.iter().position(|id| *id == current) else { return };
        let last = list.len() - 1;
        let next = match e.key().as_str() {
            "ArrowRight" => {
                if at == last {
                    0
                } else {
                    at + 1
                }
            }
            "ArrowLeft" => {
                if at == 0 {
                    last
                } else {
                    at - 1
                }
            }
            "Home" => 0,
            "End" => last,
            "Delete" => {
                e.prevent_default();
                on_close.run(current);
                return;
            }
            _ => return,
        };
        e.prevent_default();
        tabs.show(&list[next]);
        focus_tab(&list[next]);
    };
    let menu = move |_| {
        let Some(id) = tabs.active_in(pane) else { return Vec::new() };
        let here = ids.get_untracked();
        let others: Vec<String> = here.iter().filter(|i| **i != id).cloned().collect();
        let (move_id, close_id) = (id.clone(), id);
        vec![
            if pane == 0 {
                MenuAction::new("Open to the right", "panel").disabled(here.len() < 2).run(move || tabs.move_tab(&move_id, 1, None))
            } else {
                MenuAction::new("Move to the left", "back").run(move || tabs.move_tab(&move_id, 0, None))
            },
            MenuAction::new("Close tab", "close").separator().run(move || on_close.run(close_id.clone())),
            MenuAction::new("Close other tabs", "close").disabled(others.is_empty()).run(move || {
                for other in &others {
                    if !tabs.is_dirty(other) {
                        on_close.run(other.clone());
                    }
                }
            }),
        ]
    };
    view! {
        <div node_ref=bar class=move || if tabs.focus.get() == pane { "note-tabbar is-focused-pane" } else { "note-tabbar" } data-pane=pane.to_string()
            on:dragover=move |e: web_sys::DragEvent| {
                if tabs.dragged().is_some() {
                    e.prevent_default();
                }
            }
            on:drop=move |e: web_sys::DragEvent| {
                if let Some(id) = tabs.dragged() {
                    e.prevent_default();
                    tabs.drag(None);
                    tabs.move_tab(&id, pane, None);
                }
            }>
            <div role="tablist" aria-label=label class="note-tabs" on:keydown=keys>
                <For each=move || ids.get() key=|id| id.clone() let:id>
                    {note_tab(tabs, pane, id, on_close, bar)}
                </For>
            </div>
            <ActionMenu label="Tab options" class="note-tab-menu" actions=menu><Icon name="more"/></ActionMenu>
        </div>
    }
}

fn note_tab(tabs: NoteTabs, pane: usize, id: String, on_close: Callback<String>, bar: NodeRef<html::Div>) -> impl IntoView {
    let title = Memo::new({
        let id = id.clone();
        move |_| tabs.tabs.with(|list| list.iter().find(|t| t.entry.id == id).map(|t| strip_md_ext(&t.entry.name)).unwrap_or_default())
    });
    let selected = Memo::new({
        let id = id.clone();
        move |_| tabs.active.with(|a| a[pane].as_deref() == Some(id.as_str()))
    });
    let dirty = Memo::new({
        let id = id.clone();
        move |_| tabs.dirty.with(|d| d.contains(&id))
    });
    let (show_id, close_id, aux_id, drag_id, drop_id, menu_id) = (id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
    view! {
        <div role="presentation" class=move || if selected.get() { "note-tab is-active" } else { "note-tab" }
            on:dragover=move |e: web_sys::DragEvent| {
                if tabs.dragged().is_some() {
                    e.prevent_default();
                }
            }
            on:drop=move |e: web_sys::DragEvent| {
                if let Some(dragged) = tabs.dragged() {
                    e.prevent_default();
                    e.stop_propagation();
                    tabs.drag(None);
                    tabs.move_tab(&dragged, pane, Some(&drop_id));
                }
            }
            on:contextmenu=move |e: web_sys::MouseEvent| {
                e.prevent_default();
                tabs.show(&menu_id);
                let trigger = bar
                    .get_untracked()
                    .and_then(|b| b.query_selector(".note-tab-menu > .ui-menu-trigger").ok().flatten())
                    .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok());
                if let Some(trigger) = trigger {
                    trigger.click();
                }
            }>
            <button type="button" role="tab" id=tab_id(&id) class="note-tab-button" draggable="true"
                aria-selected=move || selected.get().to_string() aria-controls=panel_id(&id) tabindex=move || if selected.get() { "0" } else { "-1" }
                title=move || title.get()
                on:click=move |_| tabs.show(&show_id)
                on:mousedown=move |e: web_sys::MouseEvent| {
                    if e.button() == 1 {
                        e.prevent_default();
                    }
                }
                on:auxclick=move |e: web_sys::MouseEvent| {
                    if e.button() == 1 {
                        e.prevent_default();
                        on_close.run(aux_id.clone());
                    }
                }
                on:dragstart=move |e: web_sys::DragEvent| {
                    tabs.drag(Some(drag_id.clone()));
                    if let Some(data) = e.data_transfer() {
                        data.set_effect_allowed("move");
                        let _ = data.set_data("text/plain", &title.get_untracked());
                    }
                }
                on:dragend=move |_| tabs.drag(None)>
                <span class="note-tab-title">{move || title.get()}</span>
                {move || dirty.get().then(|| view! { <span class="note-tab-dirty" aria-hidden="true"></span><span class="sr-only">" (unsaved)"</span> })}
            </button>
            <button type="button" class="note-tab-close" tabindex="-1" data-tab-close="" aria-label=move || format!("Close {}", title.get()) title="Close tab"
                on:click=move |_| on_close.run(close_id.clone())><Icon name="close"/></button>
        </div>
    }
}
