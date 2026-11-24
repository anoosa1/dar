//! Graph view for Notes: force-directed layout over `/api/v1/notes/graph`, drawn on a `<canvas>`
//! with pan/zoom/hover/click, text & `#tag` filtering, local graph toggle, and an accessible note list.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::api;
use crate::icons::Icon;
use crate::ui::{Listener, window};

const MAX_NODES: usize = 1000;
const ANIMATION_MS: f64 = 1500.0;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    pub path: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct GraphData {
    #[serde(default)]
    pub nodes: Vec<GraphNode>,
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ActiveGraph {
    nodes: Vec<GraphNode>,
    edges: Vec<(usize, usize)>,
    neighbors: Vec<Vec<usize>>,
    truncated: bool,
}

impl ActiveGraph {
    fn degree(&self, index: usize) -> usize {
        self.neighbors.get(index).map_or(0, Vec::len)
    }

    fn is_neighbor(&self, a: usize, b: usize) -> bool {
        self.neighbors
            .get(a)
            .is_some_and(|list| list.binary_search(&b).is_ok())
    }
}

fn build_active_graph(data: &GraphData, open_id: Option<&str>, local_only: bool) -> ActiveGraph {
    let mut selected: Vec<GraphNode>;
    let mut truncated = false;

    if local_only
        && let Some(cur) = open_id
        && data.nodes.iter().any(|n| n.id == cur)
    {
        let mut keep: HashSet<&str> = HashSet::new();
        keep.insert(cur);
        for edge in &data.edges {
            if edge.from == cur {
                keep.insert(edge.to.as_str());
            } else if edge.to == cur {
                keep.insert(edge.from.as_str());
            }
        }
        selected = data
            .nodes
            .iter()
            .filter(|n| keep.contains(n.id.as_str()))
            .cloned()
            .collect();
        if selected.len() > MAX_NODES {
            selected.truncate(MAX_NODES);
            truncated = true;
        }
    } else if data.nodes.len() > MAX_NODES {
        let mut deg: HashMap<&str, usize> = HashMap::new();
        for edge in &data.edges {
            if edge.from != edge.to {
                *deg.entry(edge.from.as_str()).or_default() += 1;
                *deg.entry(edge.to.as_str()).or_default() += 1;
            }
        }
        let mut ranked: Vec<&GraphNode> = data.nodes.iter().collect();
        ranked.sort_by(|a, b| {
            deg.get(b.id.as_str())
                .copied()
                .unwrap_or(0)
                .cmp(&deg.get(a.id.as_str()).copied().unwrap_or(0))
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        selected = ranked.into_iter().take(MAX_NODES).cloned().collect();
        truncated = true;
    } else {
        selected = data.nodes.clone();
    }

    let id_to_idx: HashMap<&str, usize> = selected
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let mut edges: Vec<(usize, usize)> = Vec::new();
    let mut seen_edges: HashSet<(usize, usize)> = HashSet::new();
    let mut neighbors: Vec<Vec<usize>> = vec![Vec::new(); selected.len()];
    for edge in &data.edges {
        if let (Some(&from), Some(&to)) = (
            id_to_idx.get(edge.from.as_str()),
            id_to_idx.get(edge.to.as_str()),
        ) && from != to
            && seen_edges.insert((from, to))
        {
            edges.push((from, to));
            neighbors[from].push(to);
            neighbors[to].push(from);
        }
    }
    for list in &mut neighbors {
        list.sort_unstable();
        list.dedup();
    }
    ActiveGraph {
        nodes: selected,
        edges,
        neighbors,
        truncated,
    }
}

fn matches_filter(node: &GraphNode, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    if let Some(tag_q) = q.strip_prefix('#') {
        if tag_q.is_empty() {
            return !node.tags.is_empty();
        }
        return node.tags.iter().any(|t| t.to_lowercase().contains(tag_q));
    }
    node.title.to_lowercase().contains(&q)
        || node.path.to_lowercase().contains(&q)
        || node.tags.iter().any(|t| t.to_lowercase().contains(&q))
}

fn node_radius(degree: usize) -> f64 {
    (5.0 + (degree as f64).sqrt() * 2.5).min(18.0)
}

fn resolve_css_color(raw: &str, dark: bool, fallback: &str) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return fallback.to_owned();
    }
    if let Some(inner) = s
        .strip_prefix("light-dark(")
        .and_then(|v| v.strip_suffix(')'))
    {
        let mut depth = 0i32;
        let mut split_at = None;
        for (i, ch) in inner.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    split_at = Some(i);
                    break;
                }
                _ => {}
            }
        }
        if let Some(comma) = split_at {
            let chosen = if dark {
                &inner[comma + 1..]
            } else {
                &inner[..comma]
            }
            .trim();
            if !chosen.is_empty() {
                return chosen.to_owned();
            }
        }
    }
    s.to_owned()
}

fn truncate_label(title: &str, max_chars: usize) -> String {
    let label = if title.is_empty() { "Untitled" } else { title };
    if label.chars().count() <= max_chars {
        return label.to_owned();
    }
    let mut out: String = label.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

#[derive(Clone, Debug)]
struct Simulation {
    x: Vec<f64>,
    y: Vec<f64>,
    vx: Vec<f64>,
    vy: Vec<f64>,
    fx: Vec<f64>,
    fy: Vec<f64>,
    alpha: f64,
}

impl Simulation {
    fn new(n: usize) -> Self {
        let mut x = Vec::with_capacity(n);
        let mut y = Vec::with_capacity(n);
        let golden = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
        for i in 0..n {
            let r = 24.0 * ((i + 1) as f64).sqrt();
            let theta = (i as f64) * golden;
            x.push(r * theta.cos());
            y.push(r * theta.sin());
        }
        Self {
            x,
            y,
            vx: vec![0.0; n],
            vy: vec![0.0; n],
            fx: vec![0.0; n],
            fy: vec![0.0; n],
            alpha: 1.0,
        }
    }

    fn step(&mut self, edges: &[(usize, usize)]) -> f64 {
        let n = self.x.len();
        if n == 0 {
            return 0.0;
        }
        self.fx.fill(0.0);
        self.fy.fill(0.0);

        // O(n²) repulsion (capped at MAX_NODES).
        let limit = n.min(MAX_NODES);
        for i in 0..limit {
            let (xi, yi) = (self.x[i], self.y[i]);
            for j in (i + 1)..limit {
                let dx = self.x[j] - xi;
                let dy = self.y[j] - yi;
                let dist2 = (dx * dx + dy * dy).max(144.0);
                let dist = dist2.sqrt();
                let force = (2800.0 / dist2).min(40.0);
                let ux = dx / dist;
                let uy = dy / dist;
                self.fx[i] -= force * ux;
                self.fy[i] -= force * uy;
                self.fx[j] += force * ux;
                self.fy[j] += force * uy;
            }
        }

        // Springs on edges.
        for &(a, b) in edges {
            if a >= n || b >= n {
                continue;
            }
            let dx = self.x[b] - self.x[a];
            let dy = self.y[b] - self.y[a];
            let dist = (dx * dx + dy * dy).sqrt().max(1.0);
            let force = 0.035 * (dist - 85.0);
            let ux = dx / dist;
            let uy = dy / dist;
            self.fx[a] += force * ux;
            self.fy[a] += force * uy;
            self.fx[b] -= force * ux;
            self.fy[b] -= force * uy;
        }

        // Centering force.
        for i in 0..n {
            self.fx[i] -= self.x[i] * 0.015;
            self.fy[i] -= self.y[i] * 0.015;
        }

        // Velocity integration with damping.
        let mut energy = 0.0;
        for i in 0..n {
            let mut vx = (self.vx[i] + self.fx[i] * self.alpha) * 0.82;
            let mut vy = (self.vy[i] + self.fy[i] * self.alpha) * 0.82;
            let speed = vx.hypot(vy);
            if speed > 40.0 {
                let scale = 40.0 / speed;
                vx *= scale;
                vy *= scale;
            }
            self.vx[i] = vx;
            self.vy[i] = vy;
            self.x[i] += vx;
            self.y[i] += vy;
            energy += speed;
        }
        self.alpha = (self.alpha * 0.985).max(0.05);
        energy / (n as f64)
    }

    fn settle(&mut self, edges: &[(usize, usize)], max_steps: usize) {
        for _ in 0..max_steps {
            if self.step(edges) < 0.05 {
                break;
            }
        }
    }

    fn bounds(&self) -> (f64, f64, f64, f64) {
        if self.x.is_empty() {
            return (0.0, 0.0, 0.0, 0.0);
        }
        let (mut min_x, mut max_x) = (self.x[0], self.x[0]);
        let (mut min_y, mut max_y) = (self.y[0], self.y[0]);
        for i in 1..self.x.len() {
            min_x = min_x.min(self.x[i]);
            max_x = max_x.max(self.x[i]);
            min_y = min_y.min(self.y[i]);
            max_y = max_y.max(self.y[i]);
        }
        (min_x, max_x, min_y, max_y)
    }
}

struct Palette {
    accent: String,
    muted: String,
    border: String,
    text: String,
}

fn read_palette(canvas: &web_sys::HtmlCanvasElement) -> Palette {
    let style = window().get_computed_style(canvas).ok().flatten();
    let scheme = style
        .as_ref()
        .and_then(|s| s.get_property_value("color-scheme").ok())
        .unwrap_or_default();
    let dark = if scheme.contains("dark") && !scheme.contains("light") {
        true
    } else if scheme.contains("light") && !scheme.contains("dark") {
        false
    } else {
        window()
            .match_media("(prefers-color-scheme: dark)")
            .ok()
            .flatten()
            .is_some_and(|m| m.matches())
    };
    let get = |name: &str, light_fb: &str, dark_fb: &str| {
        let fb = if dark { dark_fb } else { light_fb };
        let raw = style
            .as_ref()
            .and_then(|s| s.get_property_value(name).ok())
            .unwrap_or_default();
        resolve_css_color(&raw, dark, fb)
    };
    Palette {
        accent: get("--accent", "#73396f", "#f2a2a9"),
        muted: get("--muted", "#755d74", "#cfb4ca"),
        border: get("--border", "#c9b4c7", "#80617a"),
        text: get("--text", "#352138", "#faeaf3"),
    }
}

fn prefers_reduced_motion() -> bool {
    window()
        .match_media("(prefers-reduced-motion: reduce)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches())
}

struct WheelListener {
    target: web_sys::EventTarget,
    callback: Closure<dyn FnMut(web_sys::Event)>,
}

impl WheelListener {
    fn new(target: &web_sys::EventTarget, f: impl FnMut(web_sys::Event) + 'static) -> Self {
        let callback = Closure::<dyn FnMut(web_sys::Event)>::new(f);
        let opts = web_sys::AddEventListenerOptions::new();
        opts.set_passive(false);
        let _ = target.add_event_listener_with_callback_and_add_event_listener_options(
            "wheel",
            callback.as_ref().unchecked_ref(),
            &opts,
        );
        Self {
            target: target.clone(),
            callback,
        }
    }
}

impl Drop for WheelListener {
    fn drop(&mut self) {
        let _ = self
            .target
            .remove_event_listener_with_callback("wheel", self.callback.as_ref().unchecked_ref());
    }
}

struct CanvasState {
    graph: ActiveGraph,
    sim: Simulation,
    matched: Vec<bool>,
    open_idx: Option<usize>,
    hovered: Option<usize>,
    pan_x: f64,
    pan_y: f64,
    zoom: f64,
    user_moved: bool,
    fitted: bool,
    width: f64,
    height: f64,
    dpr: f64,
    dragging: bool,
    drag_moved: bool,
    drag_start_x: f64,
    drag_start_y: f64,
    pan_start_x: f64,
    pan_start_y: f64,
    raf_id: Option<i32>,
    raf_cb: Option<Closure<dyn FnMut(f64)>>,
    canvas: Option<web_sys::HtmlCanvasElement>,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            graph: ActiveGraph::default(),
            sim: Simulation::new(0),
            matched: Vec::new(),
            open_idx: None,
            hovered: None,
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
            user_moved: false,
            fitted: false,
            width: 0.0,
            height: 0.0,
            dpr: 1.0,
            dragging: false,
            drag_moved: false,
            drag_start_x: 0.0,
            drag_start_y: 0.0,
            pan_start_x: 0.0,
            pan_start_y: 0.0,
            raf_id: None,
            raf_cb: None,
            canvas: None,
        }
    }
}

impl CanvasState {
    fn cancel_raf(&mut self) {
        if let Some(id) = self.raf_id.take() {
            let _ = window().cancel_animation_frame(id);
        }
        self.raf_cb = None;
    }

    fn world_to_screen(&self, index: usize) -> (f64, f64) {
        (
            self.width * 0.5 + self.pan_x + self.sim.x[index] * self.zoom,
            self.height * 0.5 + self.pan_y + self.sim.y[index] * self.zoom,
        )
    }

    fn fit_view(&mut self) {
        if self.graph.nodes.is_empty() || self.width <= 1.0 || self.height <= 1.0 {
            self.pan_x = 0.0;
            self.pan_y = 0.0;
            self.zoom = 1.0;
            return;
        }
        let (min_x, max_x, min_y, max_y) = self.sim.bounds();
        let cx = (min_x + max_x) * 0.5;
        let cy = (min_y + max_y) * 0.5;
        let span_x = (max_x - min_x + 120.0).max(160.0);
        let span_y = (max_y - min_y + 120.0).max(160.0);
        self.zoom = (self.width / span_x)
            .min(self.height / span_y)
            .clamp(0.4, 1.25);
        self.pan_x = -cx * self.zoom;
        self.pan_y = -cy * self.zoom;
        self.fitted = true;
    }

    fn hit_test(&self, sx: f64, sy: f64) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        let r_scale = self.zoom.clamp(0.65, 1.6);
        for i in 0..self.graph.nodes.len() {
            let (nx, ny) = self.world_to_screen(i);
            let r = node_radius(self.graph.degree(i)) * r_scale + 5.0;
            let d = (sx - nx).hypot(sy - ny);
            if d <= r && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    fn draw(&self) {
        let Some(canvas) = self.canvas.as_ref() else {
            return;
        };
        if self.width <= 0.0 || self.height <= 0.0 {
            return;
        }
        let Ok(Some(ctx)) = canvas.get_context("2d") else {
            return;
        };
        let Ok(ctx) = ctx.dyn_into::<web_sys::CanvasRenderingContext2d>() else {
            return;
        };

        let palette = read_palette(canvas);
        let _ = ctx.set_transform(self.dpr, 0.0, 0.0, self.dpr, 0.0, 0.0);
        ctx.clear_rect(0.0, 0.0, self.width, self.height);

        let n = self.graph.nodes.len();
        if n == 0 {
            return;
        }

        // Edges.
        for &(a, b) in &self.graph.edges {
            if a >= n || b >= n {
                continue;
            }
            let (ax, ay) = self.world_to_screen(a);
            let (bx, by) = self.world_to_screen(b);
            let (color, width, alpha) = if let Some(h) = self.hovered {
                if a == h || b == h {
                    (palette.accent.as_str(), 1.6, 0.85)
                } else {
                    (palette.border.as_str(), 1.0, 0.14)
                }
            } else {
                let both_match = self.matched.get(a).copied().unwrap_or(true)
                    && self.matched.get(b).copied().unwrap_or(true);
                (
                    palette.border.as_str(),
                    1.0,
                    if both_match { 0.7 } else { 0.14 },
                )
            };
            ctx.set_stroke_style_str(color);
            ctx.set_line_width(width);
            ctx.set_global_alpha(alpha);
            ctx.begin_path();
            ctx.move_to(ax, ay);
            ctx.line_to(bx, by);
            ctx.stroke();
        }

        // Nodes.
        let r_scale = self.zoom.clamp(0.65, 1.6);
        let two_pi = std::f64::consts::TAU;
        for i in 0..n {
            let (nx, ny) = self.world_to_screen(i);
            let deg = self.graph.degree(i);
            let r = node_radius(deg) * r_scale;
            let is_open = self.open_idx == Some(i);
            let is_hovered = self.hovered == Some(i);
            let is_neighbor = self.hovered.is_some_and(|h| self.graph.is_neighbor(h, i));
            let is_match = self.matched.get(i).copied().unwrap_or(true);

            let (fill, alpha, ring) = if self.hovered.is_some() {
                if is_hovered {
                    (palette.accent.as_str(), 1.0, true)
                } else if is_neighbor {
                    (palette.accent.as_str(), 0.85, is_open)
                } else {
                    (palette.muted.as_str(), 0.18, false)
                }
            } else if is_open {
                (
                    palette.accent.as_str(),
                    if is_match { 1.0 } else { 0.25 },
                    true,
                )
            } else if deg > 0 {
                (
                    palette.accent.as_str(),
                    if is_match { 0.82 } else { 0.18 },
                    false,
                )
            } else {
                (
                    palette.muted.as_str(),
                    if is_match { 0.8 } else { 0.18 },
                    false,
                )
            };

            if ring {
                ctx.set_stroke_style_str(&palette.accent);
                ctx.set_line_width(2.0);
                ctx.set_global_alpha((alpha * 0.45_f64).min(0.6));
                ctx.begin_path();
                let _ = ctx.arc(nx, ny, r + 4.0, 0.0, two_pi);
                ctx.stroke();
            }

            ctx.set_fill_style_str(fill);
            ctx.set_global_alpha(alpha);
            ctx.begin_path();
            let _ = ctx.arc(nx, ny, r, 0.0, two_pi);
            ctx.fill();
        }

        // Labels (shown when zoomed in, hovered/neighbor, or open).
        ctx.set_font("12px Inter, ui-sans-serif, system-ui, sans-serif");
        ctx.set_text_align("center");
        ctx.set_text_baseline("top");
        ctx.set_fill_style_str(&palette.text);
        let zoomed_in = self.zoom >= 1.1;
        for i in 0..n {
            let is_hovered = self.hovered == Some(i);
            let is_neighbor = self.hovered.is_some_and(|h| self.graph.is_neighbor(h, i));
            let is_open = self.open_idx == Some(i);
            let is_match = self.matched.get(i).copied().unwrap_or(true);
            let show = is_hovered
                || is_neighbor
                || (zoomed_in && (is_match || is_open))
                || (is_open && self.hovered.is_none());
            if !show {
                continue;
            }
            let alpha = if self.hovered.is_some() {
                if is_hovered {
                    1.0
                } else if is_neighbor {
                    0.9
                } else {
                    0.18
                }
            } else if is_match || is_open {
                0.92
            } else {
                0.2
            };
            let (nx, ny) = self.world_to_screen(i);
            let r = node_radius(self.graph.degree(i)) * r_scale;
            let label = truncate_label(&self.graph.nodes[i].title, 28);
            ctx.set_global_alpha(alpha);
            let _ = ctx.fill_text(&label, nx, ny + r + 5.0);
        }
        ctx.set_global_alpha(1.0);
    }
}

fn start_layout(state: Rc<RefCell<CanvasState>>) {
    {
        let mut st = state.borrow_mut();
        st.cancel_raf();
        let n = st.graph.nodes.len();
        if n == 0 {
            st.draw();
            return;
        }
        let edges = st.graph.edges.clone();
        if prefers_reduced_motion() || n <= 1 {
            let steps = if n > 400 { 40 } else { 90 };
            st.sim.settle(&edges, steps);
            if !st.user_moved {
                st.fit_view();
            }
            st.draw();
            return;
        }
        // Short warm-up so frame 1 is framed neatly, then animate for up to ~1.5s.
        let warmup = if n > 400 { 5 } else { 15 };
        st.sim.settle(&edges, warmup);
        if !st.user_moved {
            st.fit_view();
        }
        st.draw();
    }

    let start_ms = window().performance().map(|p| p.now()).unwrap_or(0.0);
    schedule_raf(state, start_ms);
}

fn schedule_raf(state: Rc<RefCell<CanvasState>>, start_ms: f64) {
    let weak = Rc::downgrade(&state);
    let closure = Closure::<dyn FnMut(f64)>::new(move |now: f64| {
        let Some(state_tick) = weak.upgrade() else {
            return;
        };
        let keep_going = {
            let mut st = state_tick.borrow_mut();
            st.raf_id = None;
            let edges = st.graph.edges.clone();
            let energy = st.sim.step(&edges);
            st.draw();
            now - start_ms < ANIMATION_MS && energy > 0.04
        };
        if keep_going {
            let mut st = state_tick.borrow_mut();
            let id = st.raf_cb.as_ref().and_then(|c| {
                window()
                    .request_animation_frame(c.as_ref().unchecked_ref())
                    .ok()
            });
            st.raf_id = id;
        }
    });
    let id = window()
        .request_animation_frame(closure.as_ref().unchecked_ref())
        .ok();
    let mut st = state.borrow_mut();
    if let Some(id) = id {
        st.raf_id = Some(id);
        st.raf_cb = Some(closure);
    }
}

fn sync_canvas_size(
    st: &mut CanvasState,
    stage: &web_sys::HtmlDivElement,
    canvas: &web_sys::HtmlCanvasElement,
) {
    let rect = stage.get_bounding_client_rect();
    let w = rect.width().max(1.0);
    let h = rect.height().max(1.0);
    let dpr = window().device_pixel_ratio().clamp(1.0, 3.0);
    let pw = (w * dpr).round().max(1.0) as u32;
    let ph = (h * dpr).round().max(1.0) as u32;
    if canvas.width() != pw {
        canvas.set_width(pw);
    }
    if canvas.height() != ph {
        canvas.set_height(ph);
    }
    st.width = w;
    st.height = h;
    st.dpr = dpr;
    if !st.fitted && !st.user_moved && !st.graph.nodes.is_empty() {
        st.fit_view();
    }
    st.draw();
}

#[component]
pub fn NoteGraph(
    #[prop(into)] open_id: Signal<Option<String>>,
    #[prop(into)] version: Signal<u32>,
    on_open: Callback<String>,
) -> impl IntoView {
    let raw: RwSignal<Option<GraphData>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let filter = RwSignal::new(String::new());
    let local_only = RwSignal::new(false);
    let retry_tick = RwSignal::new(0u32);
    let seq = RwSignal::new(0u32);

    let stage_ref = NodeRef::<html::Div>::new();
    let canvas_ref = NodeRef::<html::Canvas>::new();

    let alive = Arc::new(AtomicBool::new(true));
    let state = send_wrapper::SendWrapper::new(Rc::new(RefCell::new(CanvasState::default())));
    {
        let alive = alive.clone();
        let state = state.clone();
        on_cleanup(move || {
            alive.store(false, Ordering::Relaxed);
            state.borrow_mut().cancel_raf();
        });
    }

    // Fetch `/api/v1/notes/graph` whenever notes version or retry changes.
    {
        let alive = alive.clone();
        Effect::new(move |_| {
            let _v = version.get();
            let _r = retry_tick.get();
            let ticket = seq.get_untracked() + 1;
            seq.set(ticket);
            let alive = alive.clone();
            spawn_local(async move {
                match api::get::<GraphData>("/api/v1/notes/graph").await {
                    Ok(data)
                        if alive.load(Ordering::Relaxed)
                            && seq.try_get_untracked() == Some(ticket) =>
                    {
                        let _ = raw.try_set(Some(data));
                        let _ = error.try_set(String::new());
                    }
                    Err(e)
                        if alive.load(Ordering::Relaxed)
                            && seq.try_get_untracked() == Some(ticket) =>
                    {
                        let msg = if e.message.is_empty() {
                            "Graph could not be loaded.".into()
                        } else {
                            e.message
                        };
                        let _ = error.try_set(msg);
                    }
                    _ => {}
                }
            });
        });
    }

    // Active subgraph: full graph, or local neighborhood of the open note when Local graph is on.
    let active = Memo::new(move |_| {
        let local = local_only.get();
        let current = if local { open_id.get() } else { None };
        raw.with(|data| {
            data.as_ref()
                .map(|d| build_active_graph(d, current.as_deref(), local))
                .unwrap_or_default()
        })
    });

    // Rebuild simulation when active graph changes.
    {
        let state = state.clone();
        Effect::new(move |_| {
            let graph = active.get();
            let q = filter.get_untracked();
            let cur_open = open_id.get_untracked();
            let matched: Vec<bool> = graph.nodes.iter().map(|n| matches_filter(n, &q)).collect();
            let open_idx = cur_open
                .as_deref()
                .and_then(|id| graph.nodes.iter().position(|n| n.id == id));
            {
                let mut st = state.borrow_mut();
                st.cancel_raf();
                st.sim = Simulation::new(graph.nodes.len());
                st.graph = graph;
                st.matched = matched;
                st.open_idx = open_idx;
                st.hovered = None;
                st.user_moved = false;
                st.fitted = false;
            }
            start_layout((*state).clone());
        });
    }

    // Repaint when text filter or open note changes without restarting the simulation.
    {
        let state = state.clone();
        Effect::new(move |_| {
            let q = filter.get();
            let cur_open = open_id.get();
            let mut st = state.borrow_mut();
            st.matched = st
                .graph
                .nodes
                .iter()
                .map(|n| matches_filter(n, &q))
                .collect();
            st.open_idx = cur_open
                .as_deref()
                .and_then(|id| st.graph.nodes.iter().position(|n| n.id == id));
            st.draw();
        });
    }

    // Mount ResizeObserver and canvas pointer/wheel interactions; clean up deterministically.
    {
        let state = state.clone();
        Effect::new(move |_| {
            let (Some(stage), Some(canvas)) = (stage_ref.get(), canvas_ref.get()) else {
                return;
            };
            let stage_el: web_sys::HtmlDivElement = stage.clone();
            let canvas_el: web_sys::HtmlCanvasElement = canvas.clone();
            {
                let mut st = state.borrow_mut();
                st.canvas = Some(canvas_el.clone());
                sync_canvas_size(&mut st, &stage_el, &canvas_el);
            }

            let resize_state = (*state).clone();
            let resize_stage = stage_el.clone();
            let resize_canvas = canvas_el.clone();
            let observer_cb = Closure::<dyn FnMut()>::new(move || {
                let mut st = resize_state.borrow_mut();
                sync_canvas_size(&mut st, &resize_stage, &resize_canvas);
            });
            let observer = web_sys::ResizeObserver::new(observer_cb.as_ref().unchecked_ref()).ok();
            if let Some(ref obs) = observer {
                obs.observe(&stage_el);
            }

            let target: web_sys::EventTarget = canvas_el.clone().into();

            let down_state = (*state).clone();
            let down_canvas = canvas_el.clone();
            let on_down = Listener::new(&target, "pointerdown", false, move |event| {
                let Ok(e) = event.dyn_into::<web_sys::PointerEvent>() else {
                    return;
                };
                if e.button() != 0 {
                    return;
                }
                let mut st = down_state.borrow_mut();
                st.dragging = true;
                st.drag_moved = false;
                st.drag_start_x = e.client_x() as f64;
                st.drag_start_y = e.client_y() as f64;
                st.pan_start_x = st.pan_x;
                st.pan_start_y = st.pan_y;
                let _ = down_canvas.set_pointer_capture(e.pointer_id());
                let _ = down_canvas.class_list().add_1("is-dragging");
            });

            let move_state = (*state).clone();
            let move_canvas = canvas_el.clone();
            let on_move = Listener::new(&target, "pointermove", false, move |event| {
                let Ok(e) = event.dyn_into::<web_sys::PointerEvent>() else {
                    return;
                };
                let mut st = move_state.borrow_mut();
                if st.dragging {
                    let dx = e.client_x() as f64 - st.drag_start_x;
                    let dy = e.client_y() as f64 - st.drag_start_y;
                    if dx.hypot(dy) > 4.0 {
                        st.drag_moved = true;
                        st.user_moved = true;
                    }
                    if st.drag_moved {
                        st.pan_x = st.pan_start_x + dx;
                        st.pan_y = st.pan_start_y + dy;
                        st.draw();
                    }
                } else {
                    let rect = move_canvas.get_bounding_client_rect();
                    let sx = e.client_x() as f64 - rect.left();
                    let sy = e.client_y() as f64 - rect.top();
                    let hit = st.hit_test(sx, sy);
                    if hit != st.hovered {
                        st.hovered = hit;
                        let classes = move_canvas.class_list();
                        if hit.is_some() {
                            let _ = classes.add_1("is-hovering");
                        } else {
                            let _ = classes.remove_1("is-hovering");
                        }
                        st.draw();
                    }
                }
            });

            let up_state = (*state).clone();
            let up_canvas = canvas_el.clone();
            let on_up = Listener::new(&target, "pointerup", false, move |event| {
                let Ok(e) = event.dyn_into::<web_sys::PointerEvent>() else {
                    return;
                };
                let clicked_id = {
                    let mut st = up_state.borrow_mut();
                    if !st.dragging {
                        return;
                    }
                    st.dragging = false;
                    let _ = up_canvas.release_pointer_capture(e.pointer_id());
                    let _ = up_canvas.class_list().remove_1("is-dragging");
                    if !st.drag_moved && e.button() == 0 {
                        let rect = up_canvas.get_bounding_client_rect();
                        let sx = e.client_x() as f64 - rect.left();
                        let sy = e.client_y() as f64 - rect.top();
                        st.hit_test(sx, sy).map(|i| st.graph.nodes[i].id.clone())
                    } else {
                        None
                    }
                };
                if let Some(id) = clicked_id {
                    on_open.try_run(id);
                }
            });

            let cancel_state = (*state).clone();
            let cancel_canvas = canvas_el.clone();
            let on_cancel = Listener::new(&target, "pointercancel", false, move |event| {
                if let Ok(e) = event.dyn_into::<web_sys::PointerEvent>() {
                    let _ = cancel_canvas.release_pointer_capture(e.pointer_id());
                }
                cancel_state.borrow_mut().dragging = false;
                let _ = cancel_canvas.class_list().remove_1("is-dragging");
            });

            let leave_state = (*state).clone();
            let leave_canvas = canvas_el.clone();
            let on_leave = Listener::new(&target, "pointerleave", false, move |_| {
                let mut st = leave_state.borrow_mut();
                if st.hovered.take().is_some() {
                    let _ = leave_canvas.class_list().remove_1("is-hovering");
                    st.draw();
                }
            });

            let wheel_state = (*state).clone();
            let wheel_canvas = canvas_el.clone();
            let on_wheel = WheelListener::new(&target, move |event| {
                let Ok(e) = event.dyn_into::<web_sys::WheelEvent>() else {
                    return;
                };
                e.prevent_default();
                let mut st = wheel_state.borrow_mut();
                let rect = wheel_canvas.get_bounding_client_rect();
                let mx = e.client_x() as f64 - rect.left();
                let my = e.client_y() as f64 - rect.top();
                let factor = (-e.delta_y() * 0.0015).exp().clamp(0.75, 1.35);
                let next_zoom = (st.zoom * factor).clamp(0.25, 4.0);
                let ratio = next_zoom / st.zoom;
                let cx = mx - st.width * 0.5;
                let cy = my - st.height * 0.5;
                st.pan_x = cx - (cx - st.pan_x) * ratio;
                st.pan_y = cy - (cy - st.pan_y) * ratio;
                st.zoom = next_zoom;
                st.user_moved = true;
                st.draw();
            });

            let cleanup_state = state.clone();
            let kept = send_wrapper::SendWrapper::new((
                observer,
                observer_cb,
                on_down,
                on_move,
                on_up,
                on_cancel,
                on_leave,
                on_wheel,
            ));
            on_cleanup(move || {
                let (observer, _cb, _down, _move, _up, _cancel, _leave, _wheel) = kept.take();
                if let Some(obs) = observer {
                    obs.disconnect();
                }
                let mut st = cleanup_state.borrow_mut();
                st.cancel_raf();
                st.canvas = None;
            });
        });
    }

    // Accessible list: matching notes from the active graph, sorted by link count descending.
    let list_items = Memo::new(move |_| {
        let q = filter.get();
        active.with(|g| {
            let mut items: Vec<(String, String, usize)> = g
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| matches_filter(n, &q))
                .map(|(i, n)| (n.id.clone(), n.title.clone(), g.degree(i)))
                .collect();
            items.sort_by(|a, b| {
                b.2.cmp(&a.2)
                    .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
                    .then_with(|| a.1.cmp(&b.1))
            });
            items
        })
    });

    view! {
        <div class="note-graph">
            {move || (!error.get().is_empty()).then(|| view! {
                <div role="alert" class="error">
                    {error.get()}
                    <button type="button" class="text-button" on:click=move |_| { error.set(String::new()); retry_tick.update(|n| *n += 1); }>"Retry"</button>
                </div>
            })}
            {move || (raw.with(Option::is_none) && error.with(String::is_empty)).then(|| view! {
                <p role="status" class="muted small">"Loading graph\u{2026}"</p>
            })}
            {move || raw.with(|r| r.as_ref().is_some_and(|g| g.nodes.is_empty())).then(|| view! {
                <div class="empty-state">
                    <span class="empty-icon"><Icon name="share"/></span>
                    <h2>"No notes yet"</h2>
                    <p>"Create notes and link them with [[note]] to see their connections here."</p>
                </div>
            })}
            <Show when=move || raw.with(|r| r.as_ref().is_some_and(|g| !g.nodes.is_empty()))>
                <div class="note-graph-toolbar">
                    <input
                        type="search"
                        class="note-graph-filter"
                        placeholder="Filter by title, path or #tag"
                        aria-label="Filter graph"
                        prop:value=move || filter.get()
                        on:input=move |e| filter.set(event_target_value(&e))
                    />
                    <label class="note-graph-local">
                        <input
                            type="checkbox"
                            prop:checked=move || local_only.get()
                            on:change=move |e| local_only.set(event_target_checked(&e))
                        />
                        <span>"Local graph"</span>
                    </label>
                </div>
                {move || (local_only.get() && open_id.with(Option::is_none)).then(|| view! {
                    <p class="muted small note-graph-hint">"Open a note to focus the local graph on its direct links."</p>
                })}
                {move || active.with(|g| g.truncated).then(|| view! {
                    <p class="muted small note-graph-hint">{format!("Showing the {MAX_NODES} most connected notes.")}</p>
                })}
                <div class="note-graph-body">
                    <div node_ref=stage_ref class="note-graph-stage">
                        <canvas
                            node_ref=canvas_ref
                            class="note-graph-canvas"
                            role="img"
                            aria-label="Notes link graph"
                        />
                    </div>
                    <div class="note-graph-list-panel">
                        <h2 class="note-graph-list-heading">
                            {move || {
                                let n = list_items.with(Vec::len);
                                format!("{n} {}", if n == 1 { "note" } else { "notes" })
                            }}
                        </h2>
                        <div class="note-graph-list" role="list" aria-label="Graph notes">
                            {move || list_items.get().into_iter().map(|(id, title, links)| {
                                let click_id = id.clone();
                                let sel_id = id;
                                let display = if title.is_empty() { "Untitled".to_owned() } else { title };
                                let count = format!("{links} {}", if links == 1 { "link" } else { "links" });
                                view! {
                                    <div role="listitem" class="note-graph-row">
                                        <button
                                            type="button"
                                            class=move || if open_id.with(|o| o.as_deref() == Some(sel_id.as_str())) { "note-graph-item is-selected" } else { "note-graph-item" }
                                            on:click=move |_| { on_open.try_run(click_id.clone()); }
                                        >
                                            <span class="note-graph-item-title">{display}</span>
                                            <small class="note-graph-item-count">{count}</small>
                                        </button>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                        {move || list_items.with(Vec::is_empty).then(|| view! {
                            <p class="muted small">"No matching notes."</p>
                        })}
                    </div>
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_graph() -> GraphData {
        GraphData {
            nodes: vec![
                GraphNode {
                    id: "a".into(),
                    title: "Alpha".into(),
                    path: "Alpha.md".into(),
                    tags: vec!["work".into(), "work/space".into()],
                },
                GraphNode {
                    id: "b".into(),
                    title: "Beta".into(),
                    path: "Projects/Beta.md".into(),
                    tags: vec!["personal".into()],
                },
                GraphNode {
                    id: "c".into(),
                    title: "Gamma".into(),
                    path: "Gamma.md".into(),
                    tags: vec![],
                },
                GraphNode {
                    id: "d".into(),
                    title: "Delta".into(),
                    path: "Delta.md".into(),
                    tags: vec![],
                },
            ],
            edges: vec![
                GraphEdge {
                    from: "a".into(),
                    to: "b".into(),
                },
                GraphEdge {
                    from: "c".into(),
                    to: "a".into(),
                },
            ],
        }
    }

    #[test]
    fn active_graph_degrees_and_local_filter() {
        let g = sample_graph();
        let full = build_active_graph(&g, None, false);
        assert_eq!(full.nodes.len(), 4);
        assert_eq!(full.edges.len(), 2);
        assert_eq!(full.degree(0), 2);
        assert_eq!(full.degree(1), 1);
        assert_eq!(full.degree(2), 1);
        assert_eq!(full.degree(3), 0);
        assert!(full.is_neighbor(0, 1));
        assert!(full.is_neighbor(0, 2));
        assert!(!full.is_neighbor(1, 2));

        // Local graph around Beta includes Beta and its direct neighbor Alpha, excluding Gamma and Delta.
        let local = build_active_graph(&g, Some("b"), true);
        let ids: Vec<&str> = local.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(local.edges.len(), 1);
    }

    #[test]
    fn filter_and_colors() {
        let g = sample_graph();
        assert!(matches_filter(&g.nodes[0], ""));
        assert!(matches_filter(&g.nodes[0], "alp"));
        assert!(matches_filter(&g.nodes[1], "projects/"));
        assert!(matches_filter(&g.nodes[0], "#work"));
        assert!(matches_filter(&g.nodes[0], "#space"));
        assert!(!matches_filter(&g.nodes[1], "#work"));
        assert!(!matches_filter(&g.nodes[2], "#"));
        assert!(matches_filter(&g.nodes[0], "#"));

        assert_eq!(
            resolve_css_color("light-dark(#73396f, #f2a2a9)", false, "#000"),
            "#73396f"
        );
        assert_eq!(
            resolve_css_color("light-dark(#73396f, #f2a2a9)", true, "#000"),
            "#f2a2a9"
        );
        assert_eq!(
            resolve_css_color("light-dark(rgb(1, 2, 3), rgb(4, 5, 6))", true, "#000"),
            "rgb(4, 5, 6)"
        );
        assert_eq!(resolve_css_color("", false, "#123"), "#123");
    }

    #[test]
    fn simulation_settles_with_finite_positions() {
        let g = sample_graph();
        let active = build_active_graph(&g, None, false);
        let mut sim = Simulation::new(active.nodes.len());
        sim.settle(&active.edges, 90);
        for i in 0..active.nodes.len() {
            assert!(sim.x[i].is_finite() && sim.y[i].is_finite());
        }
        assert!(node_radius(4) > node_radius(0));
    }
}
