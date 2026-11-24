//! JSON Canvas 1.0 (<https://jsoncanvas.org>), the format of Obsidian's `.canvas` files: the
//! document model, parsing and writing (fields Dar does not know are kept as they were), and
//! the geometry the editor draws with. Pure Rust, so it is unit tested natively.

use serde_json::{Map, Value};

/// A side of a card, where edges attach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];

    pub fn name(self) -> &'static str {
        match self {
            Side::Top => "top",
            Side::Right => "right",
            Side::Bottom => "bottom",
            Side::Left => "left",
        }
    }

    pub fn parse(value: &str) -> Option<Side> {
        Side::ALL.into_iter().find(|s| s.name() == value)
    }

    /// The outward direction of this side.
    pub fn normal(self) -> (f64, f64) {
        match self {
            Side::Top => (0.0, -1.0),
            Side::Right => (1.0, 0.0),
            Side::Bottom => (0.0, 1.0),
            Side::Left => (-1.0, 0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// A rectangle spanning two corners, in any order.
    pub fn spanning(a: (f64, f64), b: (f64, f64)) -> Self {
        Self {
            x: a.0.min(b.0),
            y: a.1.min(b.1),
            width: (a.0 - b.0).abs(),
            height: (a.1 - b.1).abs(),
        }
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    pub fn contains(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    pub fn contains_point(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let (x, y) = (self.x.min(other.x), self.y.min(other.y));
        Rect {
            x,
            y,
            width: self.right().max(other.right()) - x,
            height: self.bottom().max(other.bottom()) - y,
        }
    }

    /// The middle of `side`, where an edge attaches.
    pub fn anchor(&self, side: Side) -> (f64, f64) {
        let (cx, cy) = self.center();
        match side {
            Side::Top => (cx, self.y),
            Side::Right => (self.right(), cy),
            Side::Bottom => (cx, self.bottom()),
            Side::Left => (self.x, cy),
        }
    }

    /// The side whose anchor is nearest to `point`.
    pub fn nearest_side(&self, point: (f64, f64)) -> Side {
        let distance = |s: Side| {
            let (ax, ay) = self.anchor(s);
            (ax - point.0).powi(2) + (ay - point.1).powi(2)
        };
        Side::ALL
            .into_iter()
            .min_by(|a, b| distance(*a).total_cmp(&distance(*b)))
            .unwrap_or(Side::Right)
    }
}

/// Card colours: the six presets (`"1"` red … `"6"` purple) or a `#rrggbb` hex colour.
pub const PRESETS: [(&str, &str); 6] = [
    ("1", "Red"),
    ("2", "Orange"),
    ("3", "Yellow"),
    ("4", "Green"),
    ("5", "Cyan"),
    ("6", "Purple"),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Color {
    Preset(u8),
    Hex(String),
}

pub fn color(value: Option<&str>) -> Option<Color> {
    let value = value?.trim();
    if let Ok(n) = value.parse::<u8>()
        && (1..=6).contains(&n)
    {
        return Some(Color::Preset(n));
    }
    let hex = value.strip_prefix('#')?;
    (matches!(hex.len(), 3 | 6) && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| Color::Hex(format!("#{}", hex.to_ascii_lowercase())))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    /// `text`, `file`, `link`, `group`, or a type from a newer version (kept as it was).
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub color: Option<String>,
    pub text: String,
    pub file: String,
    /// A `#heading` or `#^block` inside `file`, with its `#`.
    pub subpath: Option<String>,
    pub url: String,
    pub label: Option<String>,
    /// Every other field (`background`, `backgroundStyle`, fields of newer versions or plugins).
    pub extra: Map<String, Value>,
}

impl Node {
    pub fn new(id: String, kind: &str, rect: Rect) -> Self {
        Self {
            id,
            kind: kind.to_owned(),
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            color: None,
            text: String::new(),
            file: String::new(),
            subpath: None,
            url: String::new(),
            label: None,
            extra: Map::new(),
        }
    }

    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }

    pub fn is_group(&self) -> bool {
        self.kind == "group"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    None,
    Arrow,
}

impl End {
    fn parse(value: Option<&Value>, default: End) -> End {
        match value.and_then(Value::as_str) {
            Some("none") => End::None,
            Some("arrow") => End::Arrow,
            _ => default,
        }
    }

    fn name(self) -> &'static str {
        match self {
            End::None => "none",
            End::Arrow => "arrow",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub id: String,
    pub from_node: String,
    pub from_side: Option<Side>,
    pub to_node: String,
    pub to_side: Option<Side>,
    /// Defaults: no mark at the start, an arrow at the end.
    pub from_end: End,
    pub to_end: End,
    pub color: Option<String>,
    pub label: Option<String>,
    /// The ends as written (kept when unchanged, so files that leave them out stay so).
    written_ends: (Option<End>, Option<End>),
    pub extra: Map<String, Value>,
}

impl Edge {
    pub fn new(id: String, from: (&str, Side), to: (&str, Side)) -> Self {
        Self {
            id,
            from_node: from.0.to_owned(),
            from_side: Some(from.1),
            to_node: to.0.to_owned(),
            to_side: Some(to.1),
            from_end: End::None,
            to_end: End::Arrow,
            color: None,
            label: None,
            written_ends: (None, None),
            extra: Map::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Canvas {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Top-level fields other than `nodes` and `edges`.
    pub extra: Map<String, Value>,
}

fn take_str(map: &mut Map<String, Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(Value::String(_)) => map.remove(key).and_then(|v| v.as_str().map(str::to_owned)),
        _ => None,
    }
}

fn take_number(map: &mut Map<String, Value>, key: &str) -> f64 {
    match map.get(key).and_then(Value::as_f64) {
        Some(n) if n.is_finite() => {
            map.remove(key);
            n
        }
        _ => 0.0,
    }
}

fn parse_node(value: Value) -> Option<Node> {
    let Value::Object(mut map) = value else {
        return None;
    };
    let id = take_str(&mut map, "id")?;
    let kind = take_str(&mut map, "type").unwrap_or_else(|| "text".into());
    let (x, y) = (take_number(&mut map, "x"), take_number(&mut map, "y"));
    let (width, height) = (
        take_number(&mut map, "width"),
        take_number(&mut map, "height"),
    );
    let color = take_str(&mut map, "color");
    let mut node = Node {
        id,
        kind,
        x,
        y,
        width,
        height,
        color,
        text: String::new(),
        file: String::new(),
        subpath: None,
        url: String::new(),
        label: None,
        extra: Map::new(),
    };
    match node.kind.as_str() {
        "text" => node.text = take_str(&mut map, "text").unwrap_or_default(),
        "file" => {
            node.file = take_str(&mut map, "file").unwrap_or_default();
            node.subpath = take_str(&mut map, "subpath");
        }
        "link" => node.url = take_str(&mut map, "url").unwrap_or_default(),
        "group" => node.label = take_str(&mut map, "label"),
        _ => {}
    }
    node.extra = map;
    Some(node)
}

fn parse_edge(value: Value) -> Option<Edge> {
    let Value::Object(mut map) = value else {
        return None;
    };
    let id = take_str(&mut map, "id")?;
    let from_node = take_str(&mut map, "fromNode")?;
    let to_node = take_str(&mut map, "toNode")?;
    let side = |map: &mut Map<String, Value>, key: &str| {
        let parsed = map.get(key).and_then(Value::as_str).and_then(Side::parse);
        if parsed.is_some() {
            map.remove(key);
        }
        parsed
    };
    let from_side = side(&mut map, "fromSide");
    let to_side = side(&mut map, "toSide");
    let end = |map: &mut Map<String, Value>, key: &str| {
        let parsed = map.get(key).and_then(|v| {
            matches!(v.as_str(), Some("none" | "arrow")).then(|| End::parse(Some(v), End::None))
        });
        if parsed.is_some() {
            map.remove(key);
        }
        parsed
    };
    let written = (end(&mut map, "fromEnd"), end(&mut map, "toEnd"));
    let color = take_str(&mut map, "color");
    let label = take_str(&mut map, "label");
    Some(Edge {
        id,
        from_node,
        from_side,
        to_node,
        to_side,
        from_end: written.0.unwrap_or(End::None),
        to_end: written.1.unwrap_or(End::Arrow),
        color,
        label,
        written_ends: written,
        extra: map,
    })
}

/// Reads a canvas. An empty file is an empty canvas; nodes and edges without the required
/// fields are skipped, and edges to missing nodes are kept (Obsidian keeps them too).
pub fn parse(source: &str) -> Result<Canvas, String> {
    if source.trim().is_empty() {
        return Ok(Canvas::default());
    }
    let value: Value = serde_json::from_str(source)
        .map_err(|e| format!("This canvas is not valid JSON ({e})."))?;
    let Value::Object(mut map) = value else {
        return Err("This canvas is not a JSON Canvas object.".into());
    };
    let list = |v: Option<Value>| match v {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    };
    let nodes = list(map.remove("nodes"))
        .into_iter()
        .filter_map(parse_node)
        .collect();
    let edges = list(map.remove("edges"))
        .into_iter()
        .filter_map(parse_edge)
        .collect();
    Ok(Canvas {
        nodes,
        edges,
        extra: map,
    })
}

/// Coordinates are integers in JSON Canvas.
fn int(value: f64) -> Value {
    Value::from(value.round() as i64)
}

fn object(fields: Vec<(&str, Value)>, extra: &Map<String, Value>) -> String {
    let mut out = String::from("{");
    let mut first = true;
    let mut push = |key: &str, value: &Value| {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&serde_json::to_string(key).unwrap_or_default());
        out.push(':');
        out.push_str(&serde_json::to_string(value).unwrap_or_default());
    };
    for (key, value) in &fields {
        push(key, value);
    }
    for (key, value) in extra {
        if !fields.iter().any(|(k, _)| k == key) {
            push(key, value);
        }
    }
    out.push('}');
    out
}

fn node_json(node: &Node) -> String {
    let mut fields: Vec<(&str, Value)> = vec![
        ("id", node.id.clone().into()),
        ("type", node.kind.clone().into()),
    ];
    match node.kind.as_str() {
        "text" => fields.push(("text", node.text.clone().into())),
        "file" => {
            fields.push(("file", node.file.clone().into()));
            if let Some(sub) = &node.subpath {
                fields.push(("subpath", sub.clone().into()));
            }
        }
        "link" => fields.push(("url", node.url.clone().into())),
        "group" => {
            if let Some(label) = &node.label {
                fields.push(("label", label.clone().into()));
            }
        }
        _ => {}
    }
    fields.extend([
        ("x", int(node.x)),
        ("y", int(node.y)),
        ("width", int(node.width)),
        ("height", int(node.height)),
    ]);
    if let Some(color) = &node.color {
        fields.push(("color", color.clone().into()));
    }
    object(fields, &node.extra)
}

fn edge_json(edge: &Edge) -> String {
    let mut fields: Vec<(&str, Value)> = vec![
        ("id", edge.id.clone().into()),
        ("fromNode", edge.from_node.clone().into()),
    ];
    if let Some(side) = edge.from_side {
        fields.push(("fromSide", side.name().into()));
    }
    if edge.written_ends.0.is_some() || edge.from_end != End::None {
        fields.push(("fromEnd", edge.from_end.name().into()));
    }
    fields.push(("toNode", edge.to_node.clone().into()));
    if let Some(side) = edge.to_side {
        fields.push(("toSide", side.name().into()));
    }
    if edge.written_ends.1.is_some() || edge.to_end != End::Arrow {
        fields.push(("toEnd", edge.to_end.name().into()));
    }
    if let Some(color) = &edge.color {
        fields.push(("color", color.clone().into()));
    }
    if let Some(label) = &edge.label {
        fields.push(("label", label.clone().into()));
    }
    object(fields, &edge.extra)
}

/// Writes a canvas as Obsidian does: tab-indented, one node or edge per line.
pub fn serialize(canvas: &Canvas) -> String {
    let list = |items: Vec<String>| {
        if items.is_empty() {
            "[]".to_owned()
        } else {
            format!("[\n\t\t{}\n\t]", items.join(",\n\t\t"))
        }
    };
    let mut parts = vec![
        format!(
            "\t\"nodes\":{}",
            list(canvas.nodes.iter().map(node_json).collect())
        ),
        format!(
            "\t\"edges\":{}",
            list(canvas.edges.iter().map(edge_json).collect())
        ),
    ];
    for (key, value) in &canvas.extra {
        parts.push(format!(
            "\t{}:{}",
            serde_json::to_string(key).unwrap_or_default(),
            serde_json::to_string(value).unwrap_or_default()
        ));
    }
    format!("{{\n{}\n}}", parts.join(",\n"))
}

/// A fresh 16-hex-digit ID (Obsidian's form) not used by any node or edge.
pub fn fresh_id(canvas: &Canvas, mut random: impl FnMut() -> u64) -> String {
    loop {
        let id = format!("{:016x}", random());
        if !canvas.nodes.iter().any(|n| n.id == id) && !canvas.edges.iter().any(|e| e.id == id) {
            return id;
        }
    }
}

/// The bounding box of the nodes, if any.
pub fn bounds<'a>(nodes: impl IntoIterator<Item = &'a Node>) -> Option<Rect> {
    nodes.into_iter().map(Node::rect).reduce(|a, b| a.union(&b))
}

/// The view: a world point `p` shows at `p * zoom + pan` in the stage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

pub const MIN_ZOOM: f64 = 0.1;
pub const MAX_ZOOM: f64 = 4.0;

impl Default for View {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        }
    }
}

impl View {
    pub fn to_world(&self, (sx, sy): (f64, f64)) -> (f64, f64) {
        ((sx - self.x) / self.zoom, (sy - self.y) / self.zoom)
    }

    pub fn to_screen(&self, (wx, wy): (f64, f64)) -> (f64, f64) {
        (wx * self.zoom + self.x, wy * self.zoom + self.y)
    }

    /// Zooms to `zoom` (clamped) keeping the world point under the stage point `at` in place.
    pub fn zoom_at(&self, zoom: f64, at: (f64, f64)) -> View {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let world = self.to_world(at);
        View {
            x: at.0 - world.0 * zoom,
            y: at.1 - world.1 * zoom,
            zoom,
        }
    }

    /// Shows `rect` centred in a `width` × `height` stage with `margin` around it, at most at
    /// 100%.
    pub fn fit(rect: Rect, width: f64, height: f64, margin: f64) -> View {
        let room = (
            (width - 2.0 * margin).max(1.0),
            (height - 2.0 * margin).max(1.0),
        );
        let zoom = (room.0 / rect.width.max(1.0))
            .min(room.1 / rect.height.max(1.0))
            .clamp(MIN_ZOOM, 1.0);
        let (cx, cy) = rect.center();
        View {
            x: width / 2.0 - cx * zoom,
            y: height / 2.0 - cy * zoom,
            zoom,
        }
    }

    /// The world rectangle the stage shows.
    pub fn visible(&self, width: f64, height: f64) -> Rect {
        let (x, y) = self.to_world((0.0, 0.0));
        Rect::new(x, y, width / self.zoom, height / self.zoom)
    }
}

/// The sides an edge without stored sides attaches to: those facing each other.
pub fn facing_sides(from: &Rect, to: &Rect) -> (Side, Side) {
    let (a, b) = (from.center(), to.center());
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    if dx.abs() >= dy.abs() {
        if dx >= 0.0 {
            (Side::Right, Side::Left)
        } else {
            (Side::Left, Side::Right)
        }
    } else if dy >= 0.0 {
        (Side::Bottom, Side::Top)
    } else {
        (Side::Top, Side::Bottom)
    }
}

/// An edge's curve: start, two control points, end (a cubic Bézier leaving and entering the
/// cards along their sides' normals).
pub fn curve(
    from: (f64, f64),
    from_side: Side,
    to: (f64, f64),
    to_side: Option<Side>,
) -> [(f64, f64); 4] {
    let distance = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
    let reach = (distance * 0.4).clamp(30.0, 160.0);
    let (nx, ny) = from_side.normal();
    let c1 = (from.0 + nx * reach, from.1 + ny * reach);
    let c2 = match to_side {
        Some(side) => {
            let (mx, my) = side.normal();
            (to.0 + mx * reach, to.1 + my * reach)
        }
        None => to,
    };
    [from, c1, c2, to]
}

pub fn path_data(c: &[(f64, f64); 4]) -> String {
    let f = |v: f64| format!("{:.1}", v);
    format!(
        "M{} {} C{} {} {} {} {} {}",
        f(c[0].0),
        f(c[0].1),
        f(c[1].0),
        f(c[1].1),
        f(c[2].0),
        f(c[2].1),
        f(c[3].0),
        f(c[3].1)
    )
}

/// The point at `t` on the curve.
pub fn curve_point(c: &[(f64, f64); 4], t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    let (a, b, cc, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (
        a * c[0].0 + b * c[1].0 + cc * c[2].0 + d * c[3].0,
        a * c[0].1 + b * c[1].1 + cc * c[2].1 + d * c[3].1,
    )
}

/// An arrowhead (three points) with its tip at `tip`, pointing along `direction`.
pub fn arrow(tip: (f64, f64), direction: (f64, f64), size: f64) -> [(f64, f64); 3] {
    let length = (direction.0.powi(2) + direction.1.powi(2))
        .sqrt()
        .max(f64::EPSILON);
    let (dx, dy) = (direction.0 / length, direction.1 / length);
    let base = (tip.0 - dx * size, tip.1 - dy * size);
    let (px, py) = (-dy * size * 0.55, dx * size * 0.55);
    [tip, (base.0 + px, base.1 + py), (base.0 - px, base.1 - py)]
}

pub fn polygon_points(points: &[(f64, f64)]) -> String {
    points
        .iter()
        .map(|(x, y)| format!("{x:.1},{y:.1}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The nodes a group carries when it moves: those lying inside it (and inside those, for
/// nested groups), by geometry as Obsidian does.
pub fn group_members(canvas: &Canvas, group_ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut queue: Vec<Rect> = canvas
        .nodes
        .iter()
        .filter(|n| n.is_group() && group_ids.contains(&n.id))
        .map(Node::rect)
        .collect();
    while let Some(area) = queue.pop() {
        for node in &canvas.nodes {
            if group_ids.contains(&node.id)
                || out.contains(&node.id)
                || !area.contains(&node.rect())
                || node.rect() == area
            {
                continue;
            }
            out.push(node.id.clone());
            if node.is_group() {
                queue.push(node.rect());
            }
        }
    }
    out
}

/// The text a card is known by in lists and labels.
pub fn summary(node: &Node) -> String {
    let first_line = |s: &str| {
        s.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or_default()
            .trim_start_matches('#')
            .trim()
            .to_owned()
    };
    let text = match node.kind.as_str() {
        "text" => first_line(&node.text),
        "file" => node.file.rsplit('/').next().unwrap_or_default().to_owned(),
        "link" => node.url.clone(),
        "group" => node.label.clone().unwrap_or_default(),
        _ => String::new(),
    };
    let text: String = text.chars().take(80).collect();
    if text.is_empty() {
        "Empty".into()
    } else {
        text
    }
}

pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "text" => "Text card",
        "file" => "File card",
        "link" => "Link card",
        "group" => "Group",
        _ => "Card",
    }
}

/// A link card's host name, shown as its title.
pub fn link_host(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or(host);
    host.strip_prefix("www.").unwrap_or(host).to_owned()
}

/// Only web links open from a link card.
pub fn safe_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// Canvases are `*.canvas` files in the Notes folder.
pub fn is_canvas(name: &str) -> bool {
    name.to_lowercase().ends_with(".canvas")
}

/// A canvas file's name without `.canvas`.
pub fn stem(name: &str) -> &str {
    if is_canvas(name) {
        &name[..name.len() - ".canvas".len()]
    } else {
        name
    }
}

/// `Untitled.canvas`, or `Untitled 2.canvas` and so on when the name is taken (`taken` holds
/// lower-case names).
pub fn unique_name(base: &str, taken: &[String]) -> String {
    let free = |name: &str| !taken.iter().any(|t| t.eq_ignore_ascii_case(name));
    let first = format!("{base}.canvas");
    if free(&first) {
        return first;
    }
    (2..1000)
        .map(|n| format!("{base} {n}.canvas"))
        .find(|name| free(name))
        .unwrap_or_else(|| format!("{base} {}.canvas", taken.len() + 1))
}

/// Default card sizes, as Obsidian makes them.
pub fn default_size(kind: &str, media_type: &str) -> (f64, f64) {
    match kind {
        "text" => (250.0, 60.0),
        "link" => (400.0, 120.0),
        "group" => (480.0, 320.0),
        _ if media_type.starts_with("image/") => (400.0, 300.0),
        _ if media_type.is_empty() || media_type.starts_with("text/markdown") => (400.0, 400.0),
        _ => (400.0, 120.0),
    }
}

pub const MIN_WIDTH: f64 = 60.0;
pub const MIN_HEIGHT: f64 = 40.0;

impl Canvas {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn edge_mut(&mut self, id: &str) -> Option<&mut Edge> {
        self.edges.iter_mut().find(|e| e.id == id)
    }

    /// The card at a world point: the topmost card, else the topmost group (cards lie above
    /// groups). `skip` lists cards to look through.
    pub fn hit(&self, point: (f64, f64), skip: &[String]) -> Option<&Node> {
        let under: Vec<&Node> = self
            .nodes
            .iter()
            .rev()
            .filter(|n| !skip.contains(&n.id) && n.rect().contains_point(point))
            .collect();
        under
            .iter()
            .copied()
            .find(|n| !n.is_group())
            .or_else(|| under.first().copied())
    }

    /// The cards a move of `ids` carries: those cards and everything inside the groups among them.
    pub fn carried(&self, ids: &[String]) -> Vec<String> {
        let mut out: Vec<String> = ids
            .iter()
            .filter(|id| self.node(id).is_some())
            .cloned()
            .collect();
        for member in group_members(self, ids) {
            if !out.contains(&member) {
                out.push(member);
            }
        }
        out
    }

    /// Removes cards (with the edges attached to them) and edges.
    pub fn remove(&mut self, nodes: &[String], edges: &[String]) {
        self.nodes.retain(|n| !nodes.contains(&n.id));
        self.edges.retain(|e| {
            !edges.contains(&e.id) && !nodes.contains(&e.from_node) && !nodes.contains(&e.to_node)
        });
    }

    /// Adds an edge between two different cards, unless the same connection exists.
    pub fn connect(&mut self, id: String, from: (&str, Side), to: (&str, Side)) -> bool {
        if from.0 == to.0 || self.node(from.0).is_none() || self.node(to.0).is_none() {
            return false;
        }
        let exists = self.edges.iter().any(|e| {
            e.from_node == from.0
                && e.to_node == to.0
                && e.from_side.unwrap_or(from.1) == from.1
                && e.to_side.unwrap_or(to.1) == to.1
        });
        if exists {
            return false;
        }
        self.edges.push(Edge::new(id, from, to));
        true
    }

    /// The cards a marquee touches.
    pub fn within(&self, area: &Rect) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|n| area.intersects(&n.rect()))
            .map(|n| n.id.clone())
            .collect()
    }
}

/// What the editor draws for an edge, in world coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeShape {
    pub path: String,
    pub from_arrow: Option<String>,
    pub to_arrow: Option<String>,
    /// Where the label sits: the middle of the curve.
    pub middle: (f64, f64),
}

pub const ARROW_SIZE: f64 = 12.0;

/// The curve between two cards, leaving `from_side` and entering `to_side` (or the sides facing
/// each other when an edge does not store them).
pub fn edge_curve(
    from: &Rect,
    from_side: Option<Side>,
    to: &Rect,
    to_side: Option<Side>,
) -> [(f64, f64); 4] {
    let (facing_from, facing_to) = facing_sides(from, to);
    let (fs, ts) = (
        from_side.unwrap_or(facing_from),
        to_side.unwrap_or(facing_to),
    );
    curve(from.anchor(fs), fs, to.anchor(ts), Some(ts))
}

pub fn edge_shape(canvas: &Canvas, edge: &Edge) -> Option<EdgeShape> {
    let (from, to) = (
        canvas.node(&edge.from_node)?.rect(),
        canvas.node(&edge.to_node)?.rect(),
    );
    let c = edge_curve(&from, edge.from_side, &to, edge.to_side);
    let head = |tip: (f64, f64), back: (f64, f64)| {
        let direction = (tip.0 - back.0, tip.1 - back.1);
        polygon_points(&arrow(tip, direction, ARROW_SIZE))
    };
    Some(EdgeShape {
        path: path_data(&c),
        from_arrow: (edge.from_end == End::Arrow).then(|| head(c[0], c[1])),
        to_arrow: (edge.to_end == End::Arrow).then(|| head(c[3], c[2])),
        middle: curve_point(&c, 0.5),
    })
}

/// The CSS class for a colour (`is-color-1` … `is-color-6`, `has-hex`) and the hex value to set.
pub fn color_class(value: Option<&str>) -> (&'static str, Option<String>) {
    match color(value) {
        Some(Color::Preset(n)) => (
            [
                "",
                "is-color-1",
                "is-color-2",
                "is-color-3",
                "is-color-4",
                "is-color-5",
                "is-color-6",
            ][usize::from(n)],
            None,
        ),
        Some(Color::Hex(hex)) => ("has-hex", Some(hex)),
        None => ("", None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"{
	"nodes":[
		{"id":"a1","type":"text","text":"# Hello\nworld","x":-40,"y":20,"width":250,"height":120,"color":"4","plugin":{"k":1}},
		{"id":"b2","type":"file","file":"Folder/Note.md","subpath":"#Part","x":400,"y":0,"width":300,"height":200},
		{"id":"c3","type":"group","label":"Ideas","background":"bg.png","backgroundStyle":"cover","x":-100,"y":-100,"width":900,"height":500},
		{"id":"d4","type":"link","url":"https://example.org/x","x":0,"y":350,"width":200,"height":80},
		{"id":"e5","type":"future","shape":"star","x":1,"y":2,"width":3,"height":4}
	],
	"edges":[
		{"id":"e1","fromNode":"a1","fromSide":"right","toNode":"b2","toSide":"left","label":"see","custom":true},
		{"id":"e2","fromNode":"b2","toNode":"d4","fromEnd":"arrow","toEnd":"none","color":"#FF0000"}
	],
	"metadata":{"version":"1.0"}
}"##;

    #[test]
    fn parses_every_node_type() {
        let canvas = parse(SAMPLE).unwrap();
        assert_eq!(canvas.nodes.len(), 5);
        assert_eq!(canvas.nodes[0].text, "# Hello\nworld");
        assert_eq!(canvas.nodes[0].color.as_deref(), Some("4"));
        assert_eq!(canvas.nodes[1].file, "Folder/Note.md");
        assert_eq!(canvas.nodes[1].subpath.as_deref(), Some("#Part"));
        assert_eq!(canvas.nodes[2].label.as_deref(), Some("Ideas"));
        assert_eq!(canvas.nodes[2].extra["background"], "bg.png");
        assert_eq!(canvas.nodes[3].url, "https://example.org/x");
        assert_eq!(canvas.nodes[4].kind, "future");
        assert_eq!(canvas.edges[0].from_side, Some(Side::Right));
        assert_eq!(canvas.edges[0].to_end, End::Arrow);
        assert_eq!(canvas.edges[1].from_end, End::Arrow);
        assert_eq!(canvas.edges[1].to_end, End::None);
        assert_eq!(canvas.extra["metadata"]["version"], "1.0");
    }

    #[test]
    fn round_trip_keeps_unknown_fields() {
        let canvas = parse(SAMPLE).unwrap();
        let written = serialize(&canvas);
        let again: Value = serde_json::from_str(&written).unwrap();
        let original: Value = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(again, original, "{written}");
        assert_eq!(parse(&written).unwrap(), canvas);
        // Writing is stable.
        assert_eq!(serialize(&parse(&written).unwrap()), written);
        assert!(
            written.starts_with("{\n\t\"nodes\":[\n\t\t{\"id\":\"a1\",\"type\":\"text\""),
            "{written}"
        );
    }

    #[test]
    fn edits_keep_other_fields() {
        let mut canvas = parse(SAMPLE).unwrap();
        canvas.nodes[0].x = 10.4;
        canvas.nodes[0].text = "Changed".into();
        canvas.edges[0].to_end = End::None;
        let value: Value = serde_json::from_str(&serialize(&canvas)).unwrap();
        assert_eq!(value["nodes"][0]["x"], 10);
        assert_eq!(value["nodes"][0]["text"], "Changed");
        assert_eq!(value["nodes"][0]["plugin"]["k"], 1);
        assert_eq!(value["edges"][0]["toEnd"], "none");
        assert_eq!(value["edges"][0]["custom"], true);
        assert!(value["edges"][0].get("fromEnd").is_none());
    }

    #[test]
    fn empty_and_invalid_files() {
        assert_eq!(parse("").unwrap(), Canvas::default());
        assert_eq!(
            serialize(&Canvas::default()),
            "{\n\t\"nodes\":[],\n\t\"edges\":[]\n}"
        );
        assert!(parse("[1]").is_err());
        assert!(parse("{nope").is_err());
        // Nodes without an ID and edges without ends are skipped.
        let canvas = parse(r#"{"nodes":[{"type":"text"},{"id":"x","type":"text","text":"t","x":0,"y":0,"width":1,"height":1}],"edges":[{"id":"e"}]}"#).unwrap();
        assert_eq!(canvas.nodes.len(), 1);
        assert!(canvas.edges.is_empty());
    }

    #[test]
    fn colors() {
        assert_eq!(color(Some("3")), Some(Color::Preset(3)));
        assert_eq!(color(Some("#A0B1C2")), Some(Color::Hex("#a0b1c2".into())));
        assert_eq!(color(Some("#abc")), Some(Color::Hex("#abc".into())));
        assert_eq!(color(Some("7")), None);
        assert_eq!(color(Some("red")), None);
        assert_eq!(color(Some("#12345")), None);
        assert_eq!(color(None), None);
    }

    #[test]
    fn view_math() {
        let view = View {
            x: 100.0,
            y: 50.0,
            zoom: 2.0,
        };
        assert_eq!(view.to_world((300.0, 250.0)), (100.0, 100.0));
        assert_eq!(view.to_screen((100.0, 100.0)), (300.0, 250.0));
        // Zooming keeps the point under the pointer in place.
        let zoomed = view.zoom_at(1.0, (300.0, 250.0));
        assert_eq!(zoomed.to_screen((100.0, 100.0)), (300.0, 250.0));
        assert_eq!(view.zoom_at(100.0, (0.0, 0.0)).zoom, MAX_ZOOM);
        // Fitting centres the content, never above 100%.
        let fit = View::fit(Rect::new(0.0, 0.0, 200.0, 100.0), 1000.0, 600.0, 40.0);
        assert_eq!(fit.zoom, 1.0);
        assert_eq!(fit.to_screen((100.0, 50.0)), (500.0, 300.0));
        let fit = View::fit(Rect::new(-1000.0, 0.0, 2000.0, 400.0), 1040.0, 600.0, 20.0);
        assert!((fit.zoom - 0.5).abs() < 1e-9);
        let visible = fit.visible(1040.0, 600.0);
        assert!(visible.contains(&Rect::new(-1000.0, 0.0, 2000.0, 400.0)));
    }

    #[test]
    fn rect_geometry() {
        let a = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert_eq!(a.anchor(Side::Top), (50.0, 0.0));
        assert_eq!(a.anchor(Side::Right), (100.0, 25.0));
        assert_eq!(a.anchor(Side::Bottom), (50.0, 50.0));
        assert_eq!(a.anchor(Side::Left), (0.0, 25.0));
        assert_eq!(a.nearest_side((110.0, 20.0)), Side::Right);
        assert_eq!(a.nearest_side((40.0, -5.0)), Side::Top);
        assert_eq!(
            Rect::spanning((10.0, 5.0), (-10.0, 25.0)),
            Rect::new(-10.0, 5.0, 20.0, 20.0)
        );
        assert!(a.intersects(&Rect::new(90.0, 40.0, 50.0, 50.0)));
        assert!(!a.intersects(&Rect::new(100.0, 0.0, 10.0, 10.0)));
        assert!(a.contains(&Rect::new(10.0, 10.0, 20.0, 20.0)));
        assert_eq!(
            a.union(&Rect::new(150.0, -10.0, 10.0, 10.0)),
            Rect::new(0.0, -10.0, 160.0, 60.0)
        );
        let b = Rect::new(300.0, 0.0, 100.0, 50.0);
        assert_eq!(facing_sides(&a, &b), (Side::Right, Side::Left));
        assert_eq!(facing_sides(&b, &a), (Side::Left, Side::Right));
        assert_eq!(
            facing_sides(&a, &Rect::new(0.0, 300.0, 100.0, 50.0)),
            (Side::Bottom, Side::Top)
        );
    }

    #[test]
    fn curves_and_arrows() {
        let c = curve((0.0, 0.0), Side::Right, (200.0, 0.0), Some(Side::Left));
        assert_eq!(c[1], (80.0, 0.0));
        assert_eq!(c[2], (120.0, 0.0));
        assert_eq!(curve_point(&c, 0.0), (0.0, 0.0));
        assert_eq!(curve_point(&c, 1.0), (200.0, 0.0));
        assert_eq!(curve_point(&c, 0.5), (100.0, 0.0));
        assert_eq!(path_data(&c), "M0.0 0.0 C80.0 0.0 120.0 0.0 200.0 0.0");
        let head = arrow((10.0, 0.0), (1.0, 0.0), 10.0);
        assert_eq!(head[0], (10.0, 0.0));
        assert!((head[1].0 - 0.0).abs() < 1e-9 && (head[1].1 - 5.5).abs() < 1e-9);
    }

    #[test]
    fn groups_carry_their_members() {
        let canvas = parse(SAMPLE).unwrap();
        let members = group_members(&canvas, &["c3".to_owned()]);
        assert!(members.contains(&"a1".to_owned()));
        assert!(members.contains(&"b2".to_owned()));
        assert!(
            !members.contains(&"d4".to_owned()),
            "d4 reaches below the group"
        );
    }

    #[test]
    fn editing_helpers() {
        let mut canvas = parse(SAMPLE).unwrap();
        // Cards lie above groups; skipped cards are looked through.
        assert_eq!(
            canvas.hit((0.0, 50.0), &[]).map(|n| n.id.as_str()),
            Some("a1")
        );
        assert_eq!(
            canvas.hit((300.0, 300.0), &[]).map(|n| n.id.as_str()),
            Some("c3")
        );
        assert_eq!(
            canvas
                .hit((0.0, 50.0), &["a1".to_owned()])
                .map(|n| n.id.as_str()),
            Some("c3")
        );
        assert!(canvas.hit((5000.0, 0.0), &[]).is_none());
        // A group carries what lies inside it.
        let carried = canvas.carried(&["c3".to_owned()]);
        assert!(
            carried.contains(&"c3".to_owned())
                && carried.contains(&"a1".to_owned())
                && !carried.contains(&"d4".to_owned())
        );
        // Connecting: not to itself, not twice.
        assert!(canvas.connect("n1".into(), ("a1", Side::Bottom), ("d4", Side::Top)));
        assert!(!canvas.connect("n2".into(), ("a1", Side::Bottom), ("d4", Side::Top)));
        assert!(!canvas.connect("n3".into(), ("a1", Side::Bottom), ("a1", Side::Top)));
        assert!(!canvas.connect("n4".into(), ("a1", Side::Bottom), ("zz", Side::Top)));
        let value: Value = serde_json::from_str(&serialize(&canvas)).unwrap();
        let added = &value["edges"][2];
        assert_eq!(
            (
                added["fromNode"].as_str(),
                added["fromSide"].as_str(),
                added["toNode"].as_str(),
                added["toSide"].as_str()
            ),
            (Some("a1"), Some("bottom"), Some("d4"), Some("top"))
        );
        assert!(
            added.get("fromEnd").is_none() && added.get("toEnd").is_none(),
            "{added}"
        );
        // Removing a card drops its edges.
        canvas.remove(&["b2".to_owned()], &[]);
        assert!(canvas.node("b2").is_none());
        assert_eq!(
            canvas
                .edges
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            vec!["n1"]
        );
        canvas.remove(&[], &["n1".to_owned()]);
        assert!(canvas.edges.is_empty());
        // A marquee selects the cards it touches.
        let mut touched = canvas.within(&Rect::new(-50.0, 0.0, 20.0, 400.0));
        touched.sort();
        assert_eq!(touched, vec!["a1", "c3"]);
    }

    #[test]
    fn edge_shapes_and_colors() {
        let canvas = parse(SAMPLE).unwrap();
        let shape = edge_shape(&canvas, &canvas.edges[0]).unwrap();
        // a1's right side (210, 80) to b2's left side (400, 100).
        assert!(shape.path.starts_with("M210.0 80.0 C"), "{}", shape.path);
        assert!(shape.path.ends_with("400.0 100.0"), "{}", shape.path);
        assert!(shape.from_arrow.is_none());
        assert!(
            shape
                .to_arrow
                .as_deref()
                .is_some_and(|p| p.starts_with("400.0,100.0 "))
        );
        assert!((shape.middle.0 - 305.0).abs() < 1e-9 && (shape.middle.1 - 90.0).abs() < 1e-9);
        // Without stored sides the edge takes the facing ones: d4 lies mostly left of b2.
        let shape = edge_shape(&canvas, &canvas.edges[1]).unwrap();
        assert!(shape.path.starts_with("M400.0 100.0 C"), "{}", shape.path);
        assert!(shape.from_arrow.is_some() && shape.to_arrow.is_none());
        let mut dangling = canvas.edges[0].clone();
        dangling.to_node = "missing".into();
        assert!(edge_shape(&canvas, &dangling).is_none());
        assert_eq!(color_class(Some("2")), ("is-color-2", None));
        assert_eq!(
            color_class(Some("#FF0000")),
            ("has-hex", Some("#ff0000".to_owned()))
        );
        assert_eq!(color_class(Some("bad")), ("", None));
    }

    #[test]
    fn canvas_names() {
        assert!(is_canvas("Board.canvas") && is_canvas("B.CANVAS") && !is_canvas("Board.md"));
        assert_eq!(stem("Plan.canvas"), "Plan");
        assert_eq!(unique_name("Untitled", &[]), "Untitled.canvas");
        assert_eq!(
            unique_name(
                "Untitled",
                &["untitled.canvas".into(), "Untitled 2.canvas".into()]
            ),
            "Untitled 3.canvas"
        );
        assert_eq!(default_size("text", ""), (250.0, 60.0));
        assert_eq!(default_size("file", "image/png"), (400.0, 300.0));
    }

    #[test]
    fn ids_summaries_and_links() {
        let canvas = parse(SAMPLE).unwrap();
        let mut n = 0u64;
        let id = fresh_id(&canvas, || {
            n += 1;
            0xa1 * (n % 2) + n
        });
        assert_eq!(id.len(), 16);
        assert!(!canvas.nodes.iter().any(|node| node.id == id));
        assert_eq!(summary(&canvas.nodes[0]), "Hello");
        assert_eq!(summary(&canvas.nodes[1]), "Note.md");
        assert_eq!(link_host("https://www.example.org/x?y"), "example.org");
        assert_eq!(link_host("http://user@host:8080/"), "host:8080");
        assert!(safe_url("https://a.b") && !safe_url("javascript:alert(1)"));
    }
}
