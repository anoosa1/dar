//! The PDF viewer's text: the invisible, selectable text layer over each drawn page, search
//! in a document's text, selections turned into page boxes, and the marks (highlights and
//! search matches) drawn under the text.
//!
//! Text runs come from `dar-pdf` through the worker, in PDF points with the origin at the top
//! left of the page. The text layer places one span per run at its baseline with
//! `--x/--y/--size` in points (the page box multiplies by `--pdf-scale`), turned by its angle and
//! stretched (`--sx`, measured once with a canvas at 100 px) so the browser's sans serif covers
//! the run's real length. Marks are fractions of the page box, so zoom needs no rebuild.

use std::cell::RefCell;

use js_sys::{Array, Function, Reflect};
use wasm_bindgen::{JsCast, JsValue};

use crate::ui::{document, window};

/// Share of the em above the baseline in a run's box (as `dar-pdf` reports it).
const ASCENT: f64 = 0.8;
/// Share of the em below the baseline.
const DESCENT: f64 = 0.2;

#[derive(Clone, Debug, PartialEq)]
pub struct PdfTextChar {
    pub text: String,
    pub offset: f64,
    pub advance: f64,
}

/// A run of text on one baseline (see `dar_pdf::TextRun`), in PDF points.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfTextRun {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub font_size: f64,
    pub baseline_x: f64,
    pub baseline_y: f64,
    pub angle: f64,
    pub length: f64,
    pub chars: Vec<PdfTextChar>,
    pub ends_line: bool,
}

/// A box on page `page` (zero-based) as fractions of the page's width and height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfRect {
    pub page: usize,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A saved highlight drawn over its pages.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfHighlight {
    pub id: String,
    pub rects: Vec<PdfRect>,
}

/// Selected text in the viewer: its text with line breaks, its first page and its boxes.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfSelection {
    pub text: String,
    pub page: usize,
    pub rects: Vec<PdfRect>,
}

/// One search match: its page and boxes.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfMatch {
    pub page: usize,
    /// Which match on its page (0 for the first), to keep the current match across updates.
    pub ordinal: usize,
    pub rects: Vec<PdfRect>,
}

/// Where Find is: matches so far, the current one (from 1), and whether every page was read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PdfFindState {
    pub matches: usize,
    pub current: Option<usize>,
    pub complete: bool,
}

// ---- Reading the worker's reply ---------------------------------------------------------

fn floats(item: &JsValue, key: &str) -> Vec<f64> {
    Reflect::get(item, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Float32Array>().ok())
        .map(|a| a.to_vec().into_iter().map(f64::from).collect())
        .unwrap_or_default()
}

/// The runs of a `{type: "text"}` message; malformed entries are skipped.
pub(super) fn parse_runs(data: &JsValue) -> Vec<PdfTextRun> {
    let Ok(raw) = Reflect::get(data, &JsValue::from_str("runs")) else { return Vec::new() };
    if !Array::is_array(&raw) {
        return Vec::new();
    }
    let mut runs = Vec::new();
    for item in Array::from(&raw).iter() {
        let geometry = floats(&item, "box");
        if geometry.len() < 9 || geometry.iter().any(|v| !v.is_finite()) || geometry[4] <= 0.0 {
            continue;
        }
        let text = Reflect::get(&item, &JsValue::from_str("text")).ok().and_then(|v| v.as_string()).unwrap_or_default();
        let names: Vec<String> = Reflect::get(&item, &JsValue::from_str("chars"))
            .ok()
            .filter(Array::is_array)
            .map(|v| Array::from(&v).iter().map(|c| c.as_string().unwrap_or_default()).collect())
            .unwrap_or_default();
        let offsets = floats(&item, "offsets");
        let advances = floats(&item, "advances");
        let chars = names
            .into_iter()
            .zip(offsets)
            .zip(advances)
            .map(|((text, offset), advance)| PdfTextChar { text, offset, advance })
            .collect();
        let ends_line = Reflect::get(&item, &JsValue::from_str("endsLine")).ok().and_then(|v| v.as_bool()).unwrap_or(true);
        runs.push(PdfTextRun {
            text,
            x: geometry[0],
            y: geometry[1],
            width: geometry[2],
            height: geometry[3],
            font_size: geometry[4],
            baseline_x: geometry[5],
            baseline_y: geometry[6],
            angle: geometry[7],
            length: geometry[8],
            chars,
            ends_line,
        });
    }
    runs
}

// ---- Search -------------------------------------------------------------------------------

/// Folds a character for matching: lower case, any whitespace as one space.
fn fold(c: char, out: &mut Vec<char>) {
    if c.is_whitespace() {
        out.push(' ');
    } else {
        out.extend(c.to_lowercase());
    }
}

/// The query as matched: folded, with runs of spaces collapsed and the ends trimmed.
pub fn fold_query(query: &str) -> Vec<char> {
    let mut folded = Vec::new();
    for c in query.trim().chars() {
        let mut one = Vec::new();
        fold(c, &mut one);
        for c in one {
            if c == ' ' && folded.last() == Some(&' ') {
                continue;
            }
            folded.push(c);
        }
    }
    folded
}

/// A page's text folded for search, with where each character came from (run, char), or
/// `None` for the separators between runs.
fn page_index(runs: &[PdfTextRun]) -> (Vec<char>, Vec<Option<(usize, usize)>>) {
    let mut text = Vec::new();
    let mut from = Vec::new();
    for (r, run) in runs.iter().enumerate() {
        for (c, ch) in run.chars.iter().enumerate() {
            let mut folded = Vec::new();
            for scalar in ch.text.chars() {
                fold(scalar, &mut folded);
            }
            for f in folded {
                if f == ' ' && text.last() == Some(&' ') {
                    continue;
                }
                text.push(f);
                from.push(Some((r, c)));
            }
        }
        if text.last() != Some(&' ') {
            text.push(' ');
            from.push(None);
        }
    }
    (text, from)
}

/// The box of characters `first..=last` of `run`, in points (axis-aligned).
fn run_span_box(run: &PdfTextRun, first: usize, last: usize) -> (f64, f64, f64, f64) {
    let start = run.chars.get(first).map_or(0.0, |c| c.offset);
    let end = run.chars.get(last).map_or(run.length, |c| c.offset + c.advance).max(start);
    let (sin, cos) = run.angle.sin_cos();
    let (dx, dy) = (cos, sin);
    let (nx, ny) = (-sin, cos);
    let size = run.font_size;
    let mut corners = Vec::with_capacity(4);
    for along in [start, end] {
        for across in [-ASCENT * size, DESCENT * size] {
            corners.push((run.baseline_x + dx * along + nx * across, run.baseline_y + dy * along + ny * across));
        }
    }
    let x0 = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min);
    let y0 = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min);
    let x1 = corners.iter().map(|c| c.0).fold(f64::MIN, f64::max);
    let y1 = corners.iter().map(|c| c.1).fold(f64::MIN, f64::max);
    (x0, y0, x1 - x0, y1 - y0)
}

/// Every match of `query` (from [`fold_query`]) on one page of `width × height` points.
pub fn find_on_page(page: usize, runs: &[PdfTextRun], query: &[char], size: (f64, f64)) -> Vec<PdfMatch> {
    if query.is_empty() || runs.is_empty() {
        return Vec::new();
    }
    let (text, from) = page_index(runs);
    let (pw, ph) = (size.0.max(1.0), size.1.max(1.0));
    let mut found = Vec::new();
    let mut at = 0;
    while at + query.len() <= text.len() {
        if text[at..at + query.len()] != *query {
            at += 1;
            continue;
        }
        // Group the matched characters by run: one box per run.
        let mut spans: Vec<(usize, usize, usize)> = Vec::new();
        for (r, c) in from[at..at + query.len()].iter().flatten() {
            match spans.last_mut() {
                Some((run, _, last)) if run == r => *last = *c,
                _ => spans.push((*r, *c, *c)),
            }
        }
        let rects = spans
            .into_iter()
            .map(|(r, first, last)| {
                let (x, y, w, h) = run_span_box(&runs[r], first, last);
                PdfRect { page, x: x / pw, y: y / ph, width: w / pw, height: h / ph }
            })
            .collect::<Vec<_>>();
        if !rects.is_empty() {
            found.push(PdfMatch { page, ordinal: found.len(), rects });
        }
        at += query.len();
    }
    found
}

// ---- The text layer -----------------------------------------------------------------------

thread_local! {
    static MEASURE: RefCell<Option<web_sys::CanvasRenderingContext2d>> = const { RefCell::new(None) };
}

/// The width of `text` in the layer's font at 100 px.
fn measure(text: &str) -> f64 {
    MEASURE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = document()
                .create_element("canvas")
                .ok()
                .and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok())
                .and_then(|c| c.get_context("2d").ok().flatten())
                .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok());
            if let Some(ctx) = slot.as_ref() {
                ctx.set_font("100px sans-serif");
            }
        }
        let Some(ctx) = slot.as_ref() else { return 0.0 };
        Reflect::get(ctx, &JsValue::from_str("measureText"))
            .ok()
            .and_then(|f| f.dyn_into::<Function>().ok())
            .and_then(|f| f.call1(ctx, &JsValue::from_str(text)).ok())
            .and_then(|m| Reflect::get(&m, &JsValue::from_str("width")).ok())
            .and_then(|w| w.as_f64())
            .unwrap_or(0.0)
    })
}

fn set_vars(el: &web_sys::Element, vars: &[(&str, String)]) {
    if let Some(el) = el.dyn_ref::<web_sys::HtmlElement>() {
        let style = el.style();
        for (name, value) in vars {
            let _ = style.set_property(name, value);
        }
    }
}

/// Fills `layer` with one span per run (and line breaks), replacing what it held.
pub(super) fn build_text_layer(layer: &web_sys::Element, runs: &[PdfTextRun]) {
    layer.set_text_content(None);
    let doc = document();
    let fragment = doc.create_document_fragment();
    for (i, run) in runs.iter().enumerate() {
        if run.text.trim().is_empty() && run.ends_line {
            continue;
        }
        let Ok(span) = doc.create_element("span") else { continue };
        let natural = measure(&run.text) * run.font_size / 100.0;
        let stretch = if natural > 0.01 { (run.length / natural).clamp(0.05, 20.0) } else { 1.0 };
        let next_starts_blank = runs.get(i + 1).is_some_and(|n| n.text.starts_with(char::is_whitespace));
        let text = if !run.ends_line && !run.text.ends_with(char::is_whitespace) && !next_starts_blank {
            format!("{} ", run.text)
        } else {
            run.text.clone()
        };
        span.set_text_content(Some(&text));
        set_vars(
            &span,
            &[
                ("--x", format!("{:.3}", run.baseline_x)),
                ("--y", format!("{:.3}", run.baseline_y - ASCENT * run.font_size)),
                ("--size", format!("{:.3}", run.font_size)),
                ("--angle", format!("{:.5}rad", run.angle)),
                ("--sx", format!("{stretch:.5}")),
            ],
        );
        let _ = fragment.append_child(&span);
        if run.ends_line
            && let Ok(br) = doc.create_element("br")
        {
            let _ = fragment.append_child(&br);
        }
    }
    let _ = layer.append_child(&fragment);
}

/// Draws `marks` (box, class) into `layer`, replacing what it held.
pub(super) fn paint_marks(layer: &web_sys::Element, marks: &[(PdfRect, &'static str)]) {
    layer.set_text_content(None);
    let doc = document();
    for (rect, class) in marks {
        let Ok(mark) = doc.create_element("i") else { continue };
        mark.set_class_name(class);
        set_vars(
            &mark,
            &[
                ("--x", format!("{:.5}", rect.x)),
                ("--y", format!("{:.5}", rect.y)),
                ("--w", format!("{:.5}", rect.width)),
                ("--h", format!("{:.5}", rect.height)),
            ],
        );
        let _ = layer.append_child(&mark);
    }
}

// ---- Selections ---------------------------------------------------------------------------

fn element_of(node: Option<web_sys::Node>) -> Option<web_sys::Element> {
    let node = node?;
    match node.dyn_ref::<web_sys::Element>() {
        Some(el) => Some(el.clone()),
        None => node.parent_element(),
    }
}

fn client_rects(range: &web_sys::Range) -> Vec<web_sys::DomRect> {
    Reflect::get(range, &JsValue::from_str("getClientRects"))
        .ok()
        .and_then(|f| f.dyn_into::<Function>().ok())
        .and_then(|f| f.call0(range).ok())
        .map(|list| Array::from(&list).iter().map(|r| r.unchecked_into::<web_sys::DomRect>()).collect())
        .unwrap_or_default()
}

/// Merges boxes on one line that touch, so overlapping marks do not darken.
pub fn merge_rects(mut rects: Vec<PdfRect>) -> Vec<PdfRect> {
    rects.sort_by(|a, b| (a.page, a.y, a.x).partial_cmp(&(b.page, b.y, b.x)).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<PdfRect> = Vec::new();
    for r in rects {
        if let Some(last) = merged.last_mut() {
            let overlap = (last.y + last.height).min(r.y + r.height) - last.y.max(r.y);
            let same_line = last.page == r.page && overlap > 0.5 * last.height.min(r.height);
            if same_line && r.x <= last.x + last.width + 0.004 {
                let x1 = (last.x + last.width).max(r.x + r.width);
                let y0 = last.y.min(r.y);
                let y1 = (last.y + last.height).max(r.y + r.height);
                last.width = x1 - last.x;
                last.y = y0;
                last.height = y1 - y0;
                continue;
            }
        }
        merged.push(r);
    }
    merged
}

/// Tidies selected text for a quote: each line trimmed with its spaces collapsed, blank lines
/// dropped.
pub fn tidy_text(text: &str) -> String {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The window's selection when it lies in `scroller`'s text layers: its text, first page and
/// boxes (from the selected text of each span, so whole page boxes never count).
pub(super) fn current_selection(scroller: &web_sys::Element) -> Option<PdfSelection> {
    let selection = window().get_selection().ok().flatten()?;
    if selection.is_collapsed() || selection.range_count() == 0 {
        return None;
    }
    let in_layer = |node: Option<web_sys::Node>| {
        element_of(node)
            .and_then(|el| el.closest(".pdf-view-text").ok().flatten())
            .is_some_and(|layer| scroller.contains(Some(&layer)))
    };
    if !in_layer(selection.anchor_node()) || !in_layer(selection.focus_node()) {
        return None;
    }
    let range = selection.get_range_at(0).ok()?;
    let text = tidy_text(&String::from(selection.to_string()));
    if text.is_empty() {
        return None;
    }
    let spans = scroller.query_selector_all(".pdf-view-text span").ok()?;
    let start = range.start_container().ok()?;
    let end = range.end_container().ok()?;
    let mut rects = Vec::new();
    for i in 0..spans.length() {
        let Some(span) = spans.item(i) else { continue };
        if !range.intersects_node(&span).unwrap_or(false) {
            continue;
        }
        let Ok(part) = document().create_range() else { continue };
        if part.select_node_contents(&span).is_err() {
            continue;
        }
        if span.contains(Some(&start)) {
            let _ = part.set_start(&start, range.start_offset().unwrap_or(0));
        }
        if span.contains(Some(&end)) {
            let _ = part.set_end(&end, range.end_offset().unwrap_or(0));
        }
        let Some(page_box) = span
            .dyn_ref::<web_sys::Element>()
            .and_then(|el| el.closest(".pdf-view-page").ok().flatten())
        else {
            continue;
        };
        let Some(page) = page_box.get_attribute("data-page").and_then(|p| p.parse::<usize>().ok()) else { continue };
        let frame = page_box.get_bounding_client_rect();
        if frame.width() <= 0.0 || frame.height() <= 0.0 {
            continue;
        }
        for r in client_rects(&part) {
            if r.width() < 0.5 || r.height() < 0.5 {
                continue;
            }
            let x0 = ((r.left() - frame.left()) / frame.width()).clamp(0.0, 1.0);
            let y0 = ((r.top() - frame.top()) / frame.height()).clamp(0.0, 1.0);
            let x1 = ((r.right() - frame.left()) / frame.width()).clamp(0.0, 1.0);
            let y1 = ((r.bottom() - frame.top()) / frame.height()).clamp(0.0, 1.0);
            if x1 > x0 && y1 > y0 {
                rects.push(PdfRect { page, x: x0, y: y0, width: x1 - x0, height: y1 - y0 });
            }
        }
    }
    let rects = merge_rects(rects);
    let page = rects.iter().map(|r| r.page).min()?;
    Some(PdfSelection { text, page, rects })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, x: f64, y: f64, ends_line: bool) -> PdfTextRun {
        let chars = text
            .chars()
            .enumerate()
            .map(|(i, c)| PdfTextChar { text: c.to_string(), offset: i as f64 * 6.0, advance: 6.0 })
            .collect::<Vec<_>>();
        PdfTextRun {
            text: text.into(),
            x,
            y: y - 8.0,
            width: chars.len() as f64 * 6.0,
            height: 10.0,
            font_size: 10.0,
            baseline_x: x,
            baseline_y: y,
            angle: 0.0,
            length: chars.len() as f64 * 6.0,
            chars,
            ends_line,
        }
    }

    #[test]
    fn find_matches_across_runs_and_case() {
        let runs = vec![run("The quick", 10.0, 20.0, false), run("Brown fox", 100.0, 20.0, true), run("brown  bear", 10.0, 40.0, true)];
        let query = fold_query("  BROWN ");
        let found = find_on_page(2, &runs, &query, (200.0, 100.0));
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].ordinal, 1);
        let r = found[0].rects[0];
        assert_eq!(r.page, 2);
        assert!((r.x - 0.5).abs() < 1e-9 && (r.width - 0.15).abs() < 1e-9, "{r:?}");
        assert!((r.y - 0.12).abs() < 1e-9 && (r.height - 0.1).abs() < 1e-9, "{r:?}");

        // A phrase over two runs on one line gets a box in each.
        let across = find_on_page(0, &runs, &fold_query("quick brown"), (200.0, 100.0));
        assert_eq!(across.len(), 1);
        assert_eq!(across[0].rects.len(), 2);
        // Spaces collapse in the text and in the query.
        assert_eq!(find_on_page(0, &runs, &fold_query("brown bear"), (200.0, 100.0)).len(), 1);
        assert!(find_on_page(0, &runs, &fold_query(""), (200.0, 100.0)).is_empty());
    }

    #[test]
    fn rotated_run_boxes_turn_with_the_text() {
        let mut down = run("ab", 50.0, 10.0, true);
        down.angle = std::f64::consts::FRAC_PI_2;
        let (x, y, w, h) = run_span_box(&down, 0, 1);
        assert!((x - 48.0).abs() < 1e-9 && (w - 10.0).abs() < 1e-9, "{x} {w}");
        assert!((y - 10.0).abs() < 1e-9 && (h - 12.0).abs() < 1e-9, "{y} {h}");
    }

    #[test]
    fn selections_merge_and_tidy() {
        let a = PdfRect { page: 0, x: 0.1, y: 0.1, width: 0.2, height: 0.02 };
        let b = PdfRect { page: 0, x: 0.3, y: 0.101, width: 0.1, height: 0.02 };
        let c = PdfRect { page: 0, x: 0.1, y: 0.2, width: 0.2, height: 0.02 };
        let merged = merge_rects(vec![c, b, a]);
        assert_eq!(merged.len(), 2);
        assert!((merged[0].width - 0.3).abs() < 1e-9);
        assert_eq!(tidy_text("  One   line \n\n two\tlines "), "One line\ntwo lines");
    }
}
