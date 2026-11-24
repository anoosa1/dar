//! What the canvas draws: cards (text, notes and files, links, groups), edges with their
//! labels, the edge being drawn and the marquee. Positions are CSS custom properties set from
//! effects (Dar's CSP forbids style attributes); edges are SVG in world coordinates.

use leptos::html;
use leptos::prelude::*;

use super::board::{Board, FileTarget, place, resolve_file};
use super::model::{self, Rect, Side};
use crate::files::types::FileTile;
use crate::icons::Icon;
use crate::markdown::{Markdown, note_embed};

const KINDS: [&str; 4] = ["text", "file", "link", "group"];

/// Classes for a card's or edge's colour, and the hex colour to set as `--canvas-color`.
fn color_parts(color: Option<&str>) -> (&'static str, Option<String>) {
    model::color_class(color)
}

#[component]
pub(super) fn CanvasCard(board: Board, id: String) -> impl IntoView {
    let key = StoredValue::new(id.clone());
    let node = Memo::new(move |_| board.doc.with(|c| key.with_value(|id| c.node(id).cloned())));
    let kind = Memo::new(move |_| node.with(|n| n.as_ref().map(|n| n.kind.clone()).unwrap_or_default()));
    let color = Memo::new(move |_| node.with(|n| n.as_ref().and_then(|n| n.color.clone())));
    let selected = Memo::new(move |_| key.with_value(|id| board.selected.with(|s| s.contains(id))));
    let editing = Memo::new(move |_| key.with_value(|id| board.editing.with(|e| e.as_deref() == Some(id.as_str()))));
    let label = Memo::new(move |_| node.with(|n| n.as_ref().map(|n| format!("{}: {}", model::kind_label(&n.kind), model::summary(n))).unwrap_or_default()));
    let rect = Memo::new(move |_| node.with(|n| n.as_ref().map(model::Node::rect)));

    let element = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        let Some(el) = element.get() else { return };
        if let Some(rect) = rect.get() {
            place(&el, &rect);
        }
        let style = el.style();
        match color_parts(color.get().as_deref()).1 {
            Some(hex) => {
                let _ = style.set_property("--canvas-color", &hex);
            }
            None => {
                let _ = style.remove_property("--canvas-color");
            }
        }
    });

    let class = move || {
        let kind = kind.get();
        let kind = if KINDS.contains(&kind.as_str()) { kind } else { "unknown".to_owned() };
        format!(
            "canvas-card is-{kind} {}{}{}",
            color_parts(color.get().as_deref()).0,
            if selected.get() { " is-selected" } else { "" },
            if editing.get() { " is-editing" } else { "" }
        )
    };

    let body = move || {
        let id = key.get_value();
        match kind.get().as_str() {
            "text" => view! { <TextBody board=board id=id node=node/> }.into_any(),
            "file" => view! { <FileBody node=node board=board/> }.into_any(),
            "link" => view! { <LinkBody node=node/> }.into_any(),
            "group" => view! { <GroupLabel board=board id=id node=node/> }.into_any(),
            _ => view! {
                <div class="canvas-card-body canvas-unknown">
                    <Icon name="info"/><span>"This card type is kept as it is but cannot be shown here."</span>
                </div>
            }
            .into_any(),
        }
    };

    let id_focus = id.clone();
    view! {
        <div node_ref=element class=class data-node-id=id.clone() tabindex="0" role="group" aria-roledescription="card" aria-label=move || label.get()
            on:focusin=move |_| {
                if !board.from_pointer.get_value() && !board.is_selected(&id_focus) {
                    board.select_only(&id_focus);
                    board.reveal(&id_focus);
                }
                board.from_pointer.set_value(false);
            }>
            {body}
            {move || (!board.locked.get()).then(|| {
                let id = key.get_value();
                view! {
                    {Side::ALL.into_iter().map(|side| view! {
                        <span class=format!("canvas-handle is-{}", side.name()) data-handle=side.name() data-node=id.clone() aria-hidden="true"></span>
                    }).collect_view()}
                    <span class="canvas-resize" data-resize=id.clone() aria-hidden="true"></span>
                }
            })}
        </div>
    }
}

#[component]
fn TextBody(board: Board, id: String, node: Memo<Option<model::Node>>) -> impl IntoView {
    let text = Memo::new(move |_| node.with(|n| n.as_ref().map(|n| n.text.clone()).unwrap_or_default()));
    let key = StoredValue::new(id);
    let editing = Memo::new(move |_| key.with_value(|id| board.editing.with(|e| e.as_deref() == Some(id.as_str()))));
    move || {
        if editing.get() {
            view! { <CardInput board=board id=key.get_value() value=text.get_untracked() group=false/> }.into_any()
        } else {
            let source = text.get();
            if source.trim().is_empty() {
                view! { <div class="canvas-card-body canvas-text is-empty"><span>"Double-click to write"</span></div> }.into_any()
            } else {
                view! { <div class="canvas-card-body canvas-text"><Markdown source=source links=board.links/></div> }.into_any()
            }
        }
    }
}

/// A textarea for a text card, an input for a group's label.
#[component]
fn CardInput(board: Board, id: String, value: String, group: bool) -> impl IntoView {
    let area = NodeRef::<html::Textarea>::new();
    let line = NodeRef::<html::Input>::new();
    Effect::new(move |_| {
        let options = web_sys::FocusOptions::new();
        options.set_prevent_scroll(true);
        if let Some(el) = area.get() {
            let _ = el.focus_with_options(&options);
            let end = el.value().encode_utf16().count() as u32;
            let _ = el.set_selection_range(end, end);
        }
        if let Some(el) = line.get() {
            let _ = el.focus_with_options(&options);
            el.select();
        }
    });
    let key = StoredValue::new(id);
    let write = move |value: String| {
        key.with_value(|id| {
            board.live(|c| {
                if let Some(node) = c.node_mut(id) {
                    if group {
                        node.label = (!value.is_empty()).then_some(value);
                    } else {
                        node.text = value;
                    }
                }
            })
        })
    };
    let finish = move || {
        if key.with_value(|id| board.editing.with_untracked(|e| e.as_deref() == Some(id.as_str()))) {
            board.finish_edit();
        }
    };
    let keys = move |e: web_sys::KeyboardEvent| {
        let done = e.key() == "Escape" || (group && e.key() == "Enter") || (e.key() == "Enter" && (e.ctrl_key() || e.meta_key()));
        if done {
            e.prevent_default();
            e.stop_propagation();
            finish();
            key.with_value(|id| board.focus_card(id));
        }
    };
    if group {
        view! {
            <input node_ref=line class="canvas-label-input" aria-label="Group name" prop:value=value
                on:input=move |e| write(event_target_value(&e)) on:blur=move |_| finish() on:keydown=keys/>
        }
        .into_any()
    } else {
        view! {
            <textarea node_ref=area class="canvas-card-input" aria-label="Card text" prop:value=value
                on:input=move |e| write(event_target_value(&e)) on:blur=move |_| finish() on:keydown=keys></textarea>
        }
        .into_any()
    }
}

#[component]
fn GroupLabel(board: Board, id: String, node: Memo<Option<model::Node>>) -> impl IntoView {
    let label = Memo::new(move |_| node.with(|n| n.as_ref().and_then(|n| n.label.clone()).unwrap_or_default()));
    let key = StoredValue::new(id);
    let editing = Memo::new(move |_| key.with_value(|id| board.editing.with(|e| e.as_deref() == Some(id.as_str()))));
    view! {
        <div class="canvas-group-label">
            {move || if editing.get() {
                view! { <CardInput board=board id=key.get_value() value=label.get_untracked() group=true/> }.into_any()
            } else {
                let text = label.get();
                let class = if text.is_empty() { "is-empty" } else { "" };
                let shown = if text.is_empty() { "Group".to_owned() } else { text };
                view! { <span class=class>{shown}</span> }.into_any()
            }}
        </div>
    }
}

#[component]
fn FileBody(board: Board, node: Memo<Option<model::Node>>) -> impl IntoView {
    let target = Memo::new(move |_| {
        let (file, subpath) = node.with(|n| n.as_ref().map(|n| (n.file.clone(), n.subpath.clone())).unwrap_or_default());
        let found = board.index.with(|ix| resolve_file(ix, &file));
        let found = match found {
            FileTarget::Note { id, target, .. } => {
                let fragment = subpath.map(|s| s.trim_start_matches('#').to_owned()).filter(|s| !s.is_empty());
                FileTarget::Note { id, target, fragment }
            }
            other => other,
        };
        (found, file)
    });
    move || {
        let (found, path) = target.get();
        let name = path.rsplit('/').next().unwrap_or_default().to_owned();
        match found {
            FileTarget::Note { id, target, fragment } => view! {
                <div class="canvas-card-body canvas-note">{note_embed(id, target, fragment, String::new(), board.links)}</div>
            }
            .into_any(),
            FileTarget::File { id, name, media_type } if media_type.starts_with("image/") => view! {
                <div class="canvas-card-body canvas-image">
                    <img src=format!("/api/v1/files/{id}/content") alt=name draggable="false" loading="lazy"/>
                </div>
            }
            .into_any(),
            FileTarget::File { id, name, media_type } => view! {
                <div class="canvas-card-body canvas-file">
                    <FileTile kind="file" name=name.clone() media_type=media_type large=true/>
                    <span class="canvas-file-name">{name}</span>
                    <a class="text-button" href=format!("/api/v1/files/{id}/content") target="_blank" rel="noopener noreferrer">"Open"</a>
                </div>
            }
            .into_any(),
            FileTarget::Missing => view! {
                <div class="canvas-card-body canvas-file is-missing">
                    <Icon name="alert"/>
                    <span class="canvas-file-name">{if name.is_empty() { "No file".to_owned() } else { name }}</span>
                    <small>{if path.is_empty() { String::new() } else { format!("{path} is not in the Notes folder.") }}</small>
                </div>
            }
            .into_any(),
        }
    }
}

#[component]
fn LinkBody(node: Memo<Option<model::Node>>) -> impl IntoView {
    let url = Memo::new(move |_| node.with(|n| n.as_ref().map(|n| n.url.clone()).unwrap_or_default()));
    move || {
        let url = url.get();
        let host = model::link_host(&url);
        let safe = model::safe_url(&url);
        view! {
            <div class="canvas-card-body canvas-link">
                <span class="canvas-link-mark"><Icon name="globe"/></span>
                <strong>{if host.is_empty() { "Link".to_owned() } else { host }}</strong>
                <small>{url.clone()}</small>
                {safe.then(|| view! { <a class="text-button" href=url.clone() target="_blank" rel="noopener noreferrer">"Open link"</a> })}
            </div>
        }
    }
}

/// An SVG box around a curve, so the paths draw (and take pointer events) inside it.
fn curve_box(points: &[(f64, f64)]) -> Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in points {
        x0 = x0.min(*x);
        y0 = y0.min(*y);
        x1 = x1.max(*x);
        y1 = y1.max(*y);
    }
    let pad = model::ARROW_SIZE + 8.0;
    if x0 > x1 {
        return Rect::new(0.0, 0.0, 1.0, 1.0);
    }
    Rect::new((x0 - pad).floor(), (y0 - pad).floor(), (x1 - x0 + 2.0 * pad).ceil(), (y1 - y0 + 2.0 * pad).ceil())
}

fn view_box(rect: &Rect) -> String {
    format!("{} {} {} {}", rect.x, rect.y, rect.width, rect.height)
}

#[derive(Clone, Debug, PartialEq)]
struct EdgeDrawing {
    shape: model::EdgeShape,
    area: Rect,
    color: Option<String>,
    label: Option<String>,
}

#[component]
pub(super) fn CanvasEdge(board: Board, id: String) -> impl IntoView {
    let key = StoredValue::new(id.clone());
    let drawing = Memo::new(move |_| {
        board.doc.with(|c| {
            key.with_value(|id| {
                let edge = c.edges.iter().find(|e| &e.id == id)?;
                let shape = model::edge_shape(c, edge)?;
                let (from, to) = (c.node(&edge.from_node)?.rect(), c.node(&edge.to_node)?.rect());
                let curve = model::edge_curve(&from, edge.from_side, &to, edge.to_side);
                Some(EdgeDrawing { area: curve_box(&curve), shape, color: edge.color.clone(), label: edge.label.clone() })
            })
        })
    });
    let selected = Memo::new(move |_| key.with_value(|id| board.selected_edge.with(|e| e.as_deref() == Some(id.as_str()))));
    let editing = Memo::new(move |_| key.with_value(|id| board.editing_edge.with(|e| e.as_deref() == Some(id.as_str()))));
    let area = Memo::new(move |_| drawing.with(|d| d.as_ref().map(|d| d.area)));
    let middle = Memo::new(move |_| drawing.with(|d| d.as_ref().map(|d| d.shape.middle)));
    let color = Memo::new(move |_| drawing.with(|d| d.as_ref().and_then(|d| d.color.clone())));
    let label = Memo::new(move |_| drawing.with(|d| d.as_ref().and_then(|d| d.label.clone()).unwrap_or_default()));

    let frame = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        if let (Some(el), Some(rect)) = (frame.get(), area.get()) {
            place(&el, &rect);
        }
    });
    let path = move || drawing.with(|d| d.as_ref().map(|d| d.shape.path.clone()).unwrap_or_default());
    let from_arrow = move || drawing.with(|d| d.as_ref().and_then(|d| d.shape.from_arrow.clone()).unwrap_or_default());
    let to_arrow = move || drawing.with(|d| d.as_ref().and_then(|d| d.shape.to_arrow.clone()).unwrap_or_default());
    let class = move || {
        let color_class = color_parts(color.get().as_deref()).0;
        format!("canvas-edge {color_class}{}{}", if selected.get() { " is-selected" } else { "" }, if drawing.with(Option::is_none) { " is-hidden" } else { "" })
    };
    let hex = move || color_parts(color.get().as_deref()).1;
    let label_id = id.clone();

    view! {
        <div node_ref=frame class=class data-edge-id=id.clone()>
            <svg aria-hidden="true" color=hex
                width=move || area.get().map(|r| r.width.to_string()).unwrap_or_default()
                height=move || area.get().map(|r| r.height.to_string()).unwrap_or_default()
                viewBox=move || area.get().map(|r| view_box(&r)).unwrap_or_default()>
                <path class="canvas-edge-hit" d=path/>
                <path class="canvas-edge-line" d=path/>
                <polygon class="canvas-edge-arrow" points=from_arrow/>
                <polygon class="canvas-edge-arrow" points=to_arrow/>
            </svg>
        </div>
        <EdgeLabel board=board id=label_id middle=middle label=label editing=editing selected=selected/>
    }
}

#[component]
fn EdgeLabel(board: Board, id: String, middle: Memo<Option<(f64, f64)>>, label: Memo<String>, editing: Memo<bool>, selected: Memo<bool>) -> impl IntoView {
    let element = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        if let (Some(el), Some((x, y))) = (element.get(), middle.get()) {
            place(&el, &Rect::new(x, y, 0.0, 0.0));
        }
    });
    let key = StoredValue::new(id);
    let input = NodeRef::<html::Input>::new();
    Effect::new(move |_| {
        if let Some(el) = input.get() {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = el.focus_with_options(&options);
            el.select();
        }
    });
    let finish = move || {
        if editing.get_untracked() {
            board.finish_edit();
        }
    };
    let shown = move || editing.get() || !label.with(String::is_empty);
    view! {
        <Show when=shown>
            <div node_ref=element class=move || if selected.get() { "canvas-edge-label is-selected" } else { "canvas-edge-label" } data-edge-id=key.get_value()>
                {move || if editing.get() {
                    view! {
                        <input node_ref=input class="canvas-label-input" aria-label="Connection label" prop:value=label.get_untracked()
                            on:input=move |e| {
                                let value = event_target_value(&e);
                                key.with_value(|id| board.live(|c| {
                                    if let Some(edge) = c.edge_mut(id) {
                                        edge.label = (!value.is_empty()).then_some(value);
                                    }
                                }));
                            }
                            on:blur=move |_| finish()
                            on:keydown=move |e: web_sys::KeyboardEvent| {
                                if matches!(e.key().as_str(), "Escape" | "Enter") {
                                    e.prevent_default();
                                    e.stop_propagation();
                                    finish();
                                    board.focus_stage();
                                }
                            }/>
                    }
                    .into_any()
                } else {
                    view! { <span>{label.get()}</span> }.into_any()
                }}
            </div>
        </Show>
    }
}

/// The edge being drawn from a card's side to the pointer.
#[component]
pub(super) fn ConnectPreview(board: Board) -> impl IntoView {
    let curve = Memo::new(move |_| {
        board.connecting.get().map(|(from, side, to)| {
            let c = model::curve(from, side, to, None);
            (model::path_data(&c), curve_box(&c))
        })
    });
    let frame = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        if let (Some(el), Some((_, rect))) = (frame.get(), curve.get()) {
            place(&el, &rect);
        }
    });
    view! {
        <div node_ref=frame class=move || if curve.with(Option::is_some) { "canvas-edge is-preview" } else { "canvas-edge is-preview is-hidden" } aria-hidden="true">
            <svg aria-hidden="true"
                width=move || curve.get().map(|(_, r)| r.width.to_string()).unwrap_or_default()
                height=move || curve.get().map(|(_, r)| r.height.to_string()).unwrap_or_default()
                viewBox=move || curve.get().map(|(_, r)| view_box(&r)).unwrap_or_default()>
                <path class="canvas-edge-line" d=move || curve.get().map(|(d, _)| d).unwrap_or_default()/>
            </svg>
        </div>
    }
}

#[component]
pub(super) fn Marquee(board: Board) -> impl IntoView {
    let frame = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        if let (Some(el), Some(rect)) = (frame.get(), board.marquee.get()) {
            place(&el, &rect);
        }
    });
    view! { <div node_ref=frame class=move || if board.marquee.with(Option::is_some) { "canvas-marquee" } else { "canvas-marquee is-hidden" } aria-hidden="true"></div> }
}
