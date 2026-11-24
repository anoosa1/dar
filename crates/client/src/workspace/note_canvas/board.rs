//! The canvas being edited: the document, the view, the selection and undo history, as signals
//! the cards, the toolbar and the gestures share (`Board` is `Copy`).

use std::sync::Arc;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::model::{self, Canvas, Rect, Side};
use crate::markdown::NoteLinks;
use crate::workspace::note_links::NoteIndex;

const HISTORY: usize = 200;

#[derive(Clone, Default)]
pub(super) struct History {
    undo: Vec<Canvas>,
    redo: Vec<Canvas>,
}

/// A dialog the editor shows: the note and file picker, or the link form (for a new card, or
/// the card being edited).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum CanvasDialog {
    Pick,
    Link(Option<String>),
}

/// What a file card shows.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum FileTarget {
    /// A note: its ID, the wikilink target naming it, and the section (`Heading`, `^block`).
    Note {
        id: String,
        target: String,
        fragment: Option<String>,
    },
    File {
        id: String,
        name: String,
        media_type: String,
    },
    Missing,
}

pub(super) fn resolve_file(index: &NoteIndex, path: &str) -> FileTarget {
    let path = path.trim();
    if path.is_empty() {
        return FileTarget::Missing;
    }
    if dar_notes::is_note_target(path)
        && let Some(i) = index.vault.find(path)
    {
        return FileTarget::Note {
            id: index.notes[i].id.clone(),
            target: index.vault.wiki_target(i),
            fragment: None,
        };
    }
    match index.file_vault.find(path) {
        Some(i) => {
            let file = &index.files[i];
            FileTarget::File {
                id: file.id.clone(),
                name: file.name.clone(),
                media_type: file.media_type.clone(),
            }
        }
        None => FileTarget::Missing,
    }
}

pub(super) fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    u64::from_le_bytes(bytes)
}

fn attribute_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Clone, Copy)]
pub(super) struct Board {
    pub doc: RwSignal<Canvas>,
    pub view: RwSignal<model::View>,
    /// The stage's size in CSS pixels.
    pub size: RwSignal<(f64, f64)>,
    pub selected: RwSignal<Vec<String>>,
    pub selected_edge: RwSignal<Option<String>>,
    /// The text card or group whose text or label is being written.
    pub editing: RwSignal<Option<String>>,
    pub editing_edge: RwSignal<Option<String>>,
    edit_before: StoredValue<Option<Canvas>>,
    history: StoredValue<History>,
    pub can_undo: RwSignal<bool>,
    pub can_redo: RwSignal<bool>,
    /// Read only, still loading, or waiting on a recovery draft or conflict.
    pub locked: Signal<bool>,
    pub index: RwSignal<Arc<NoteIndex>>,
    pub links: NoteLinks,
    pub on_open: Callback<String>,
    pub stage: NodeRef<html::Div>,
    /// An edge being drawn: start, side, pointer (world coordinates).
    pub connecting: RwSignal<Option<((f64, f64), Side, (f64, f64))>>,
    pub marquee: RwSignal<Option<Rect>>,
    pub dialog: RwSignal<Option<CanvasDialog>>,
    /// Set by a pointer press, so the focus it causes does not change the selection again.
    pub from_pointer: StoredValue<bool>,
}

impl Board {
    pub fn new(
        locked: Signal<bool>,
        index: RwSignal<Arc<NoteIndex>>,
        links: NoteLinks,
        on_open: Callback<String>,
    ) -> Self {
        Self {
            doc: RwSignal::new(Canvas::default()),
            view: RwSignal::new(model::View::default()),
            size: RwSignal::new((0.0, 0.0)),
            selected: RwSignal::new(Vec::new()),
            selected_edge: RwSignal::new(None),
            editing: RwSignal::new(None),
            editing_edge: RwSignal::new(None),
            edit_before: StoredValue::new(None),
            history: StoredValue::new(History::default()),
            can_undo: RwSignal::new(false),
            can_redo: RwSignal::new(false),
            locked,
            index,
            links,
            on_open,
            stage: NodeRef::new(),
            connecting: RwSignal::new(None),
            marquee: RwSignal::new(None),
            dialog: RwSignal::new(None),
            from_pointer: StoredValue::new(false),
        }
    }

    /// Replaces the document (a load, a reload or a recovery draft), forgetting the history.
    pub fn load(self, canvas: Canvas) {
        self.editing.set(None);
        self.editing_edge.set(None);
        self.edit_before.set_value(None);
        self.history.set_value(History::default());
        self.doc.set(canvas);
        self.prune();
        self.sync_history();
    }

    fn sync_history(self) {
        let (undo, redo) = self
            .history
            .with_value(|h| (!h.undo.is_empty(), !h.redo.is_empty()));
        self.can_undo.set(undo);
        self.can_redo.set(redo);
    }

    fn record(self, before: Canvas) {
        self.history.update_value(|h| {
            h.undo.push(before);
            if h.undo.len() > HISTORY {
                h.undo.remove(0);
            }
            h.redo.clear();
        });
        self.sync_history();
    }

    /// Records `before` as an undo step if the document changed since.
    pub fn record_if_changed(self, before: Canvas) {
        if self.doc.with_untracked(|c| *c != before) {
            self.record(before);
        }
    }

    /// Applies an edit as one undo step.
    pub fn change(self, edit: impl FnOnce(&mut Canvas)) -> bool {
        if self.locked.get_untracked() {
            return false;
        }
        let before = self.doc.get_untracked();
        let mut next = before.clone();
        edit(&mut next);
        if next == before {
            return false;
        }
        self.record(before);
        self.doc.set(next);
        true
    }

    /// An edit during a gesture or while writing, whose undo step is recorded when it ends.
    pub fn live(self, edit: impl FnOnce(&mut Canvas)) {
        if !self.locked.get_untracked() {
            self.doc.update(edit);
        }
    }

    pub fn undo(self) {
        self.finish_edit();
        let Some(previous) = self.history.try_update_value(|h| h.undo.pop()).flatten() else {
            return;
        };
        let current = self.doc.get_untracked();
        self.history.update_value(|h| h.redo.push(current));
        self.doc.set(previous);
        self.prune();
        self.sync_history();
    }

    pub fn redo(self) {
        self.finish_edit();
        let Some(next) = self.history.try_update_value(|h| h.redo.pop()).flatten() else {
            return;
        };
        let current = self.doc.get_untracked();
        self.history.update_value(|h| h.undo.push(current));
        self.doc.set(next);
        self.prune();
        self.sync_history();
    }

    /// Drops selected cards and edges that no longer exist.
    fn prune(self) {
        let (nodes, edges): (Vec<String>, Vec<String>) = self.doc.with_untracked(|c| {
            (
                c.nodes.iter().map(|n| n.id.clone()).collect(),
                c.edges.iter().map(|e| e.id.clone()).collect(),
            )
        });
        if self
            .selected
            .with_untracked(|s| s.iter().any(|id| !nodes.contains(id)))
        {
            self.selected.update(|s| s.retain(|id| nodes.contains(id)));
        }
        if self
            .selected_edge
            .with_untracked(|e| e.as_ref().is_some_and(|id| !edges.contains(id)))
        {
            self.selected_edge.set(None);
        }
    }

    pub fn begin_edit(self, id: &str) {
        if self.locked.get_untracked() || self.doc.with_untracked(|c| c.node(id).is_none()) {
            return;
        }
        self.finish_edit();
        self.edit_before.set_value(Some(self.doc.get_untracked()));
        self.select_only(id);
        self.editing.set(Some(id.to_owned()));
    }

    pub fn begin_edge_edit(self, id: &str) {
        if self.locked.get_untracked() {
            return;
        }
        self.finish_edit();
        self.edit_before.set_value(Some(self.doc.get_untracked()));
        self.select_edge(id);
        self.editing_edge.set(Some(id.to_owned()));
    }

    /// Ends writing in a card or an edge label, as one undo step.
    pub fn finish_edit(self) {
        if let Some(before) = self.edit_before.try_update_value(Option::take).flatten() {
            self.record_if_changed(before);
        }
        if self.editing.with_untracked(Option::is_some) {
            self.editing.set(None);
        }
        if self.editing_edge.with_untracked(Option::is_some) {
            self.editing_edge.set(None);
        }
    }

    pub fn is_selected(self, id: &str) -> bool {
        self.selected.with_untracked(|s| s.iter().any(|x| x == id))
    }

    pub fn select_only(self, id: &str) {
        if self.selected.with_untracked(|s| s.len() != 1 || s[0] != id) {
            self.selected.set(vec![id.to_owned()]);
        }
        if self.selected_edge.with_untracked(Option::is_some) {
            self.selected_edge.set(None);
        }
    }

    pub fn toggle(self, id: &str) {
        self.selected
            .update(|s| match s.iter().position(|x| x == id) {
                Some(i) => {
                    s.remove(i);
                }
                None => s.push(id.to_owned()),
            });
        self.selected_edge.set(None);
    }

    pub fn select_edge(self, id: &str) {
        self.selected.set(Vec::new());
        self.selected_edge.set(Some(id.to_owned()));
    }

    pub fn select_all(self) {
        self.selected.set(
            self.doc
                .with_untracked(|c| c.nodes.iter().map(|n| n.id.clone()).collect()),
        );
        self.selected_edge.set(None);
    }

    pub fn has_selection(self) -> bool {
        self.selected.with_untracked(|s| !s.is_empty())
            || self.selected_edge.with_untracked(Option::is_some)
    }

    pub fn clear_selection(self) {
        if self.selected.with_untracked(|s| !s.is_empty()) {
            self.selected.set(Vec::new());
        }
        if self.selected_edge.with_untracked(Option::is_some) {
            self.selected_edge.set(None);
        }
    }

    /// The world point at the middle of the stage.
    pub fn center(self) -> (f64, f64) {
        let (w, h) = self.size.get_untracked();
        self.view.get_untracked().to_world((w / 2.0, h / 2.0))
    }

    /// Adds a card centred on `at`; groups go below the other cards. Returns its ID.
    pub fn add_card(
        self,
        kind: &str,
        media_type: &str,
        at: (f64, f64),
        setup: impl FnOnce(&mut model::Node),
    ) -> Option<String> {
        let (width, height) = model::default_size(kind, media_type);
        let mut made = None;
        self.change(|c| {
            let id = model::fresh_id(c, random_u64);
            let rect = Rect::new(
                (at.0 - width / 2.0).round(),
                (at.1 - height / 2.0).round(),
                width,
                height,
            );
            let mut node = model::Node::new(id.clone(), kind, rect);
            setup(&mut node);
            if node.is_group() {
                c.nodes.insert(0, node);
            } else {
                c.nodes.push(node);
            }
            made = Some(id);
        });
        if let Some(id) = &made {
            self.select_only(id);
        }
        made
    }

    /// Deletes the selected cards (with their edges) or the selected edge.
    pub fn delete_selection(self) -> bool {
        let nodes = self.selected.get_untracked();
        let edges: Vec<String> = self.selected_edge.get_untracked().into_iter().collect();
        if nodes.is_empty() && edges.is_empty() {
            return false;
        }
        self.finish_edit();
        let removed = self.change(|c| c.remove(&nodes, &edges));
        if removed {
            self.clear_selection();
            self.focus_stage();
        }
        removed
    }

    /// Moves the selection (and what its groups hold) by a world offset.
    pub fn nudge(self, dx: f64, dy: f64) {
        let ids = self
            .doc
            .with_untracked(|c| c.carried(&self.selected.get_untracked()));
        if ids.is_empty() {
            return;
        }
        self.change(|c| {
            for id in &ids {
                if let Some(node) = c.node_mut(id) {
                    node.x += dx;
                    node.y += dy;
                }
            }
        });
        if let Some(first) = self.selected.with_untracked(|s| s.first().cloned()) {
            self.reveal(&first);
        }
    }

    pub fn set_color(self, color: Option<String>) {
        let nodes = self.selected.get_untracked();
        let edge = self.selected_edge.get_untracked();
        self.change(|c| {
            for id in &nodes {
                if let Some(node) = c.node_mut(id) {
                    node.color = color.clone();
                }
            }
            if let Some(edge) = edge.as_deref().and_then(|id| c.edge_mut(id)) {
                edge.color = color.clone();
            }
        });
    }

    pub fn set_ends(self, from: model::End, to: model::End) {
        let Some(id) = self.selected_edge.get_untracked() else {
            return;
        };
        self.change(|c| {
            if let Some(edge) = c.edge_mut(&id) {
                edge.from_end = from;
                edge.to_end = to;
            }
        });
    }

    /// Connects the two selected cards along their facing sides.
    pub fn connect_selected(self) -> bool {
        let selected = self.selected.get_untracked();
        let [a, b] = selected.as_slice() else {
            return false;
        };
        let (a, b) = (a.clone(), b.clone());
        self.change(|c| {
            let (Some(from), Some(to)) = (
                c.node(&a).map(model::Node::rect),
                c.node(&b).map(model::Node::rect),
            ) else {
                return;
            };
            let (fs, ts) = model::facing_sides(&from, &to);
            let id = model::fresh_id(c, random_u64);
            c.connect(id, (&a, fs), (&b, ts));
        })
    }

    pub fn zoom_by(self, factor: f64) {
        let (w, h) = self.size.get_untracked();
        self.view
            .update(|v| *v = v.zoom_at(v.zoom * factor, (w / 2.0, h / 2.0)));
    }

    /// Shows every card, at most at 100%.
    pub fn fit(self) {
        let (w, h) = self.size.get_untracked();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let bounds = self.doc.with_untracked(|c| model::bounds(&c.nodes));
        self.view.set(match bounds {
            Some(rect) => model::View::fit(rect, w, h, 48.0),
            None => model::View {
                x: w / 2.0,
                y: h / 2.0,
                zoom: 1.0,
            },
        });
    }

    /// Pans so the card is in view, when it is not already.
    pub fn reveal(self, id: &str) {
        let (w, h) = self.size.get_untracked();
        let Some(rect) = self
            .doc
            .with_untracked(|c| c.node(id).map(model::Node::rect))
        else {
            return;
        };
        let view = self.view.get_untracked();
        if w <= 0.0 || view.visible(w, h).contains(&rect) {
            return;
        }
        let (cx, cy) = rect.center();
        self.view.set(model::View {
            x: w / 2.0 - cx * view.zoom,
            y: h / 2.0 - cy * view.zoom,
            zoom: view.zoom,
        });
    }

    pub fn card_element(self, id: &str) -> Option<web_sys::HtmlElement> {
        let stage = self.stage.get_untracked()?;
        stage
            .query_selector(&format!("[data-node-id=\"{}\"]", attribute_value(id)))
            .ok()
            .flatten()
            .and_then(|e| e.dyn_into().ok())
    }

    pub fn focus_card(self, id: &str) {
        if let Some(card) = self.card_element(id) {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = card.focus_with_options(&options);
        }
    }

    pub fn focus_stage(self) {
        if let Some(stage) = self.stage.get_untracked() {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = stage.focus_with_options(&options);
        }
    }

    /// Enter or a double click on a card: write in text cards and group labels, edit a link,
    /// open a note.
    pub fn activate(self, id: &str) {
        let Some((kind, file)) = self
            .doc
            .with_untracked(|c| c.node(id).map(|n| (n.kind.clone(), n.file.clone())))
        else {
            return;
        };
        match kind.as_str() {
            "text" | "group" => self.begin_edit(id),
            "link" if !self.locked.get_untracked() => self
                .dialog
                .set(Some(CanvasDialog::Link(Some(id.to_owned())))),
            "file" => match self.index.with_untracked(|ix| resolve_file(ix, &file)) {
                FileTarget::Note { id, .. } => {
                    self.on_open.try_run(id);
                }
                FileTarget::File { id, .. } => {
                    let _ = crate::ui::window()
                        .open_with_url_and_target(&format!("/api/v1/files/{id}/content"), "_blank");
                }
                FileTarget::Missing => {}
            },
            _ => {}
        }
    }
}

/// Sets a positioned element's place (world coordinates) as CSS custom properties.
pub(super) fn place(element: &web_sys::HtmlElement, rect: &Rect) {
    let style = element.style();
    let _ = style.set_property("--x", &format!("{}px", rect.x));
    let _ = style.set_property("--y", &format!("{}px", rect.y));
    let _ = style.set_property("--w", &format!("{}px", rect.width));
    let _ = style.set_property("--h", &format!("{}px", rect.height));
}
