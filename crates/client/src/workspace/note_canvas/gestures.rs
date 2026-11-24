//! Pointer, wheel and keyboard input on the stage: pan (background drag, Space or middle
//! button, wheel, one finger), zoom (Ctrl/Cmd+wheel, pinch), select (click, Shift+click,
//! Shift+drag marquee), move and resize cards, draw edges from side handles, and the keys.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::board::{Board, random_u64};
use super::model::{self, Canvas, Rect, Side};

#[derive(Clone, Debug, Default)]
enum Gesture {
    #[default]
    Idle,
    Pan { start: (f64, f64), view: model::View, moved: bool, clear: bool },
    /// `narrow`: a card of a larger selection pressed without Shift; a click without moving selects only it.
    Move { start: (f64, f64), origins: Vec<(String, f64, f64)>, moved: bool, narrow: Option<String> },
    Resize { start: (f64, f64), id: String, rect: Rect, moved: bool },
    Marquee { start: (f64, f64), base: Vec<String> },
    Connect { from: String, side: Side },
    Pinch { distance: f64, middle: (f64, f64), view: model::View },
}

#[derive(Clone, Copy)]
pub(super) struct Gestures {
    current: StoredValue<Gesture>,
    /// The document when a move or resize began, recorded as one undo step when it ends.
    before: StoredValue<Option<Canvas>>,
    /// Touch points on the stage (pointer ID, stage point).
    touches: StoredValue<Vec<(i32, (f64, f64))>>,
    pub space: StoredValue<bool>,
    /// Set after a drag, so the click that ends it does not also open a link.
    suppress_click: StoredValue<bool>,
}

fn element(event: &web_sys::Event) -> Option<web_sys::Element> {
    event.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok())
}

fn closest(element: &web_sys::Element, selector: &str) -> Option<web_sys::Element> {
    element.closest(selector).ok().flatten()
}

fn in_field(element: &web_sys::Element) -> bool {
    closest(element, "textarea, input, select, [contenteditable=true]").is_some()
}

pub(super) fn stage_point(stage: &web_sys::Element, x: i32, y: i32) -> (f64, f64) {
    let rect = stage.get_bounding_client_rect();
    (f64::from(x) - rect.left(), f64::from(y) - rect.top())
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn middle(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
}

impl Gestures {
    pub fn new() -> Self {
        Self {
            current: StoredValue::new(Gesture::Idle),
            before: StoredValue::new(None),
            touches: StoredValue::new(Vec::new()),
            space: StoredValue::new(false),
            suppress_click: StoredValue::new(false),
        }
    }

    /// Records a move or resize in progress (when a second finger turns it into a pinch).
    fn settle(self, board: Board) {
        if let Some(before) = self.before.try_update_value(Option::take).flatten() {
            board.record_if_changed(before);
        }
        board.connecting.set(None);
        board.marquee.set(None);
        self.current.set_value(Gesture::Idle);
    }

    pub fn down(self, board: Board, stage: &web_sys::Element, event: &web_sys::PointerEvent) {
        let Some(target) = element(event) else { return };
        if in_field(&target) {
            return;
        }
        let touch = event.pointer_type() == "touch";
        let button = event.button();
        if !touch && button != 0 && button != 1 {
            return;
        }
        let point = stage_point(stage, event.client_x(), event.client_y());
        self.suppress_click.set_value(false);
        board.from_pointer.set_value(true);
        if touch {
            let count = self
                .touches
                .try_update_value(|t| {
                    t.retain(|(id, _)| *id != event.pointer_id());
                    t.push((event.pointer_id(), point));
                    t.len()
                })
                .unwrap_or(0);
            if count == 2 {
                self.settle(board);
                let points = self.touches.get_value();
                let (a, b) = (points[0].1, points[1].1);
                self.current.set_value(Gesture::Pinch { distance: distance(a, b).max(1.0), middle: middle(a, b), view: board.view.get_untracked() });
                let _ = stage.set_pointer_capture(event.pointer_id());
                return;
            }
            if count > 2 {
                return;
            }
        }
        let view = board.view.get_untracked();
        let world = view.to_world(point);
        let locked = board.locked.get_untracked();
        let gesture = if button == 1 || self.space.get_value() {
            Gesture::Pan { start: point, view, moved: false, clear: false }
        } else if let Some(handle) = closest(&target, "[data-handle]").filter(|_| !locked) {
            let id = handle.get_attribute("data-node").unwrap_or_default();
            let side = handle.get_attribute("data-handle").and_then(|s| Side::parse(&s)).unwrap_or(Side::Right);
            let Some(anchor) = board.doc.with_untracked(|c| c.node(&id).map(|n| n.rect().anchor(side))) else { return };
            board.finish_edit();
            board.connecting.set(Some((anchor, side, world)));
            Gesture::Connect { from: id, side }
        } else if let Some(handle) = closest(&target, "[data-resize]").filter(|_| !locked) {
            let id = handle.get_attribute("data-resize").unwrap_or_default();
            let Some(rect) = board.doc.with_untracked(|c| c.node(&id).map(model::Node::rect)) else { return };
            board.select_only(&id);
            self.before.set_value(Some(board.doc.get_untracked()));
            Gesture::Resize { start: world, id, rect, moved: false }
        } else if let Some(card) = closest(&target, "[data-node-id]") {
            let id = card.get_attribute("data-node-id").unwrap_or_default();
            let mut narrow = None;
            if event.shift_key() || event.ctrl_key() || event.meta_key() {
                board.toggle(&id);
            } else if !board.is_selected(&id) {
                board.select_only(&id);
            } else {
                board.selected_edge.set(None);
                if board.selected.with_untracked(|s| s.len() > 1) {
                    narrow = Some(id.clone());
                }
            }
            let writing = board.editing.with_untracked(|e| e.as_deref() == Some(id.as_str()));
            if locked || writing || !board.is_selected(&id) {
                Gesture::Pan { start: point, view, moved: false, clear: false }
            } else {
                let ids = board.doc.with_untracked(|c| c.carried(&board.selected.get_untracked()));
                let origins = board.doc.with_untracked(|c| ids.iter().filter_map(|id| c.node(id).map(|n| (id.clone(), n.x, n.y))).collect());
                self.before.set_value(Some(board.doc.get_untracked()));
                Gesture::Move { start: world, origins, moved: false, narrow }
            }
        } else if let Some(edge) = closest(&target, "[data-edge-id]") {
            board.select_edge(&edge.get_attribute("data-edge-id").unwrap_or_default());
            Gesture::Idle
        } else if event.shift_key() && !touch {
            board.marquee.set(Some(Rect::spanning(world, world)));
            Gesture::Marquee { start: world, base: board.selected.get_untracked() }
        } else {
            Gesture::Pan { start: point, view, moved: false, clear: true }
        };
        // Pans and moves capture the pointer once they move, so a press on a card's link or
        // button still clicks it (and focuses the card).
        if matches!(gesture, Gesture::Connect { .. } | Gesture::Resize { .. } | Gesture::Marquee { .. }) {
            let _ = stage.set_pointer_capture(event.pointer_id());
        }
        self.current.set_value(gesture);
    }

    pub fn moved(self, board: Board, stage: &web_sys::Element, event: &web_sys::PointerEvent) {
        let point = stage_point(stage, event.client_x(), event.client_y());
        if event.pointer_type() == "touch" {
            self.touches.update_value(|t| {
                if let Some(entry) = t.iter_mut().find(|(id, _)| *id == event.pointer_id()) {
                    entry.1 = point;
                }
            });
        }
        let mut gesture = self.current.get_value();
        let view = board.view.get_untracked();
        let world = view.to_world(point);
        match &mut gesture {
            Gesture::Idle => return,
            Gesture::Pan { start, view: from, moved, .. } => {
                let (dx, dy) = (point.0 - start.0, point.1 - start.1);
                if !*moved && dx.hypot(dy) > 3.0 {
                    *moved = true;
                    let _ = stage.set_pointer_capture(event.pointer_id());
                }
                if *moved {
                    board.view.set(model::View { x: from.x + dx, y: from.y + dy, zoom: from.zoom });
                }
            }
            Gesture::Move { start, origins, moved, .. } => {
                let (dx, dy) = (world.0 - start.0, world.1 - start.1);
                if !*moved && dx.hypot(dy) * view.zoom > 3.0 {
                    *moved = true;
                    board.finish_edit();
                    let _ = stage.set_pointer_capture(event.pointer_id());
                }
                if *moved {
                    board.live(|c| {
                        for (id, x, y) in origins.iter() {
                            if let Some(node) = c.node_mut(id) {
                                node.x = (x + dx).round();
                                node.y = (y + dy).round();
                            }
                        }
                    });
                }
            }
            Gesture::Resize { start, id, rect, moved } => {
                let (dx, dy) = (world.0 - start.0, world.1 - start.1);
                if !*moved && dx.hypot(dy) * view.zoom > 2.0 {
                    *moved = true;
                }
                if *moved {
                    board.live(|c| {
                        if let Some(node) = c.node_mut(id) {
                            node.width = (rect.width + dx).max(model::MIN_WIDTH).round();
                            node.height = (rect.height + dy).max(model::MIN_HEIGHT).round();
                        }
                    });
                }
            }
            Gesture::Marquee { start, base } => {
                let area = Rect::spanning(*start, world);
                board.marquee.set(Some(area));
                let mut selection = base.clone();
                for id in board.doc.with_untracked(|c| c.within(&area)) {
                    if !selection.contains(&id) {
                        selection.push(id);
                    }
                }
                if board.selected.with_untracked(|s| *s != selection) {
                    board.selected.set(selection);
                }
            }
            Gesture::Connect { .. } => {
                board.connecting.update(|c| {
                    if let Some(c) = c {
                        c.2 = world;
                    }
                });
            }
            Gesture::Pinch { distance: start, middle: center, view: from } => {
                let points = self.touches.get_value();
                if points.len() < 2 {
                    return;
                }
                let (a, b) = (points[0].1, points[1].1);
                let (now, at) = (distance(a, b).max(1.0), middle(a, b));
                let next = from.zoom_at(from.zoom * now / *start, *center);
                board.view.set(model::View { x: next.x + at.0 - center.0, y: next.y + at.1 - center.1, zoom: next.zoom });
            }
        }
        self.current.set_value(gesture);
    }

    pub fn up(self, board: Board, stage: &web_sys::Element, event: &web_sys::PointerEvent, cancelled: bool) {
        if stage.has_pointer_capture(event.pointer_id()) {
            let _ = stage.release_pointer_capture(event.pointer_id());
        }
        if event.pointer_type() == "touch" {
            self.touches.update_value(|t| t.retain(|(id, _)| *id != event.pointer_id()));
        }
        board.from_pointer.set_value(false);
        let gesture = self.current.get_value();
        self.current.set_value(Gesture::Idle);
        let before = self.before.try_update_value(Option::take).flatten();
        match gesture {
            Gesture::Pan { moved, clear, .. } => {
                if moved {
                    self.suppress_click.set_value(true);
                } else if clear && !cancelled {
                    board.finish_edit();
                    board.clear_selection();
                }
            }
            Gesture::Move { moved, narrow, .. } => {
                if let Some(before) = before {
                    board.record_if_changed(before);
                }
                if moved {
                    self.suppress_click.set_value(true);
                } else if let Some(id) = narrow.filter(|_| !cancelled) {
                    board.select_only(&id);
                }
            }
            Gesture::Resize { moved, .. } => {
                if let Some(before) = before {
                    board.record_if_changed(before);
                }
                if moved {
                    self.suppress_click.set_value(true);
                }
            }
            Gesture::Marquee { .. } => {
                board.marquee.set(None);
                self.suppress_click.set_value(true);
            }
            Gesture::Connect { from, side } => {
                board.connecting.set(None);
                self.suppress_click.set_value(true);
                if cancelled {
                    return;
                }
                let world = board.view.get_untracked().to_world(stage_point(stage, event.client_x(), event.client_y()));
                let target = board.doc.with_untracked(|c| c.hit(world, std::slice::from_ref(&from)).map(|n| (n.id.clone(), n.rect().nearest_side(world))));
                if let Some((to, to_side)) = target {
                    let mut made = None;
                    board.change(|c| {
                        let id = model::fresh_id(c, random_u64);
                        if c.connect(id.clone(), (&from, side), (&to, to_side)) {
                            made = Some(id);
                        }
                    });
                    if let Some(id) = made {
                        board.select_edge(&id);
                    }
                }
            }
            Gesture::Pinch { .. } | Gesture::Idle => {}
        }
    }

    /// Ends a drag's click (capture phase), so a card's link does not open after moving it.
    pub fn click(self, event: &web_sys::Event) {
        if self.suppress_click.get_value() {
            self.suppress_click.set_value(false);
            event.prevent_default();
            event.stop_propagation();
        }
    }

    pub fn double_click(self, board: Board, stage: &web_sys::Element, event: &web_sys::MouseEvent) {
        let Some(target) = element(event) else { return };
        if in_field(&target) || board.locked.get_untracked() {
            return;
        }
        if closest(&target, "a, button").is_some() && closest(&target, "[data-node-id]").is_some() {
            return;
        }
        if let Some(card) = closest(&target, "[data-node-id]") {
            board.activate(&card.get_attribute("data-node-id").unwrap_or_default());
        } else if let Some(edge) = closest(&target, "[data-edge-id]") {
            board.begin_edge_edit(&edge.get_attribute("data-edge-id").unwrap_or_default());
        } else {
            let world = board.view.get_untracked().to_world(stage_point(stage, event.client_x(), event.client_y()));
            if let Some(id) = board.add_card("text", "", world, |_| {}) {
                board.begin_edit(&id);
            }
        }
        event.prevent_default();
    }

    pub fn wheel(self, board: Board, stage: &web_sys::Element, event: &web_sys::WheelEvent) {
        if element(event).is_some_and(|t| closest(&t, "textarea").is_some()) {
            return;
        }
        event.prevent_default();
        let scale = match event.delta_mode() {
            1 => 16.0,
            2 => board.size.get_untracked().1.max(1.0),
            _ => 1.0,
        };
        let (mut dx, mut dy) = (event.delta_x() * scale, event.delta_y() * scale);
        if event.ctrl_key() || event.meta_key() {
            let at = stage_point(stage, event.client_x(), event.client_y());
            let factor = (-dy * 0.01).exp().clamp(0.5, 2.0);
            board.view.update(|v| *v = v.zoom_at(v.zoom * factor, at));
            return;
        }
        if event.shift_key() && dx == 0.0 {
            (dx, dy) = (dy, 0.0);
        }
        board.view.update(|v| {
            v.x -= dx;
            v.y -= dy;
        });
    }

    /// Keys on the stage and its cards (fields handle their own).
    pub fn key_down(self, board: Board, stage: &web_sys::Element, event: &web_sys::KeyboardEvent) {
        let Some(target) = element(event) else { return };
        if in_field(&target) {
            return;
        }
        let command = event.ctrl_key() || event.meta_key();
        let key = event.key();
        // Buttons and links inside cards keep Space and Enter.
        if matches!(key.as_str(), " " | "Enter") && closest(&target, "a[href], button").is_some() {
            return;
        }
        let step = if event.shift_key() { 50.0 } else { 10.0 };
        let arrow = match key.as_str() {
            "ArrowLeft" => Some((-step, 0.0)),
            "ArrowRight" => Some((step, 0.0)),
            "ArrowUp" => Some((0.0, -step)),
            "ArrowDown" => Some((0.0, step)),
            _ => None,
        };
        let handled = if let Some((dx, dy)) = arrow.filter(|_| !command && !event.alt_key()) {
            if board.selected.with_untracked(|s| !s.is_empty()) && !board.locked.get_untracked() {
                board.nudge(dx, dy);
            } else {
                board.view.update(|v| {
                    v.x -= dx * 4.0;
                    v.y -= dy * 4.0;
                });
            }
            true
        } else {
            match key.as_str() {
                "z" | "Z" if command && !event.shift_key() => {
                    board.undo();
                    true
                }
                "z" | "Z" if command => {
                    board.redo();
                    true
                }
                "y" | "Y" if command => {
                    board.redo();
                    true
                }
                "a" | "A" if command => {
                    board.select_all();
                    true
                }
                "Delete" | "Backspace" if !command => board.delete_selection(),
                "Enter" if !command => match board.selected_edge.get_untracked() {
                    Some(edge) => {
                        board.begin_edge_edit(&edge);
                        true
                    }
                    None => match board.selected.with_untracked(|s| (s.len() == 1).then(|| s[0].clone())) {
                        Some(id) => {
                            board.activate(&id);
                            true
                        }
                        None => false,
                    },
                },
                "Escape" if board.has_selection() => {
                    board.clear_selection();
                    board.focus_stage();
                    true
                }
                " " if !command => {
                    self.space.set_value(true);
                    let _ = stage.class_list().add_1("is-space");
                    true
                }
                "+" | "=" if !command => {
                    board.zoom_by(1.25);
                    true
                }
                "-" | "_" if !command => {
                    board.zoom_by(0.8);
                    true
                }
                "!" if event.shift_key() => {
                    board.fit();
                    true
                }
                _ => false,
            }
        };
        if handled {
            event.prevent_default();
        }
    }

    pub fn key_up(self, stage: &web_sys::Element, event: &web_sys::KeyboardEvent) {
        if event.key() == " " {
            self.space.set_value(false);
            let _ = stage.class_list().remove_1("is-space");
        }
    }
}
