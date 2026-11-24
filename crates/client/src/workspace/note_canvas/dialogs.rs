//! Adding a note or file card (a picker over the library) and a link card (a web address).

use leptos::html;
use leptos::prelude::*;

use super::board::{Board, CanvasDialog};
use super::model;
use crate::files::types::FileTile;
use crate::icons::Icon;
use crate::ui::Dialog;

const SHOWN: usize = 60;

#[derive(Clone, Debug, PartialEq)]
struct Choice {
    title: String,
    path: String,
    media_type: String,
    note: bool,
}

#[component]
pub(super) fn CanvasDialogs(board: Board) -> impl IntoView {
    move || match board.dialog.get() {
        Some(CanvasDialog::Pick) => view! { <PickFile board=board/> }.into_any(),
        Some(CanvasDialog::Link(editing)) => {
            view! { <LinkForm board=board editing=editing/> }.into_any()
        }
        None => ().into_any(),
    }
}

#[component]
fn PickFile(board: Board) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let field = NodeRef::<html::Input>::new();
    Effect::new(move |_| {
        if let Some(el) = field.get() {
            let _ = el.focus();
        }
    });
    let choices = Memo::new(move |_| {
        let q = query.get().trim().to_lowercase();
        board.index.with(|ix| {
            let notes = ix.notes.iter().map(|n| Choice {
                title: if n.title.is_empty() {
                    n.name.clone()
                } else {
                    n.title.clone()
                },
                path: n.path.clone(),
                media_type: "text/markdown".into(),
                note: true,
            });
            let files = ix
                .files
                .iter()
                .filter(|f| !model::is_canvas(&f.name))
                .map(|f| Choice {
                    title: f.name.clone(),
                    path: f.path.clone(),
                    media_type: f.media_type.clone(),
                    note: false,
                });
            let mut all: Vec<Choice> = notes
                .chain(files)
                .filter(|c| {
                    q.is_empty()
                        || c.title.to_lowercase().contains(&q)
                        || c.path.to_lowercase().contains(&q)
                })
                .collect();
            all.sort_by(|a, b| {
                b.note
                    .cmp(&a.note)
                    .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            });
            all
        })
    });
    let close = Callback::new(move |()| board.dialog.set(None));
    let pick = move |choice: Choice| {
        board.dialog.set(None);
        let at = board.center();
        let path = choice.path.clone();
        if let Some(id) = board.add_card("file", &choice.media_type, at, |node| node.file = path) {
            request_animation_frame(move || board.focus_card(&id));
        }
    };
    view! {
        <Dialog title="Add a note or file" on_close=close>
            <div class="canvas-picker">
                <input node_ref=field autofocus type="search" class="canvas-picker-filter" aria-label="Find a note or file" placeholder="Find a note or file"
                    prop:value=move || query.get() on:input=move |e| query.set(event_target_value(&e))/>
                <ul class="canvas-picker-list" role="list" aria-label="Notes and files">
                    {move || choices.get().into_iter().take(SHOWN).map(|choice| {
                        let (title, path, media_type) = (choice.title.clone(), choice.path.clone(), choice.media_type.clone());
                        let name = path.rsplit('/').next().unwrap_or_default().to_owned();
                        view! {
                            <li>
                                <button type="button" class="canvas-picker-item" on:click=move |_| pick(choice.clone())>
                                    <FileTile kind="file" name=name media_type=media_type/>
                                    <span><strong>{title}</strong><small>{path}</small></span>
                                </button>
                            </li>
                        }
                    }).collect_view()}
                </ul>
                {move || choices.with(Vec::is_empty).then(|| view! { <p class="muted small">"No matching notes or files."</p> })}
                {move || (choices.with(Vec::len) > SHOWN).then(|| view! { <p class="muted small">"Type to narrow the list."</p> })}
            </div>
            <div class="dialog-actions">
                <button type="button" class="text-button" on:click=move |_| close.run(())>"Cancel"</button>
            </div>
        </Dialog>
    }
}

#[component]
fn LinkForm(board: Board, editing: Option<String>) -> impl IntoView {
    let initial = editing
        .as_deref()
        .and_then(|id| {
            board
                .doc
                .with_untracked(|c| c.node(id).map(|n| n.url.clone()))
        })
        .unwrap_or_default();
    let value = RwSignal::new(initial);
    let problem = RwSignal::new(String::new());
    let editing = StoredValue::new(editing);
    let close = Callback::new(move |()| board.dialog.set(None));
    let field = NodeRef::<html::Input>::new();
    Effect::new(move |_| {
        if let Some(el) = field.get() {
            let _ = el.focus();
        }
    });
    let submit = move || {
        let url = value.get_untracked().trim().to_owned();
        if !model::safe_url(&url) {
            problem.set("Enter a web address that starts with https:// or http://.".into());
            return;
        }
        board.dialog.set(None);
        match editing.get_value() {
            Some(id) => {
                board.change(|c| {
                    if let Some(node) = c.node_mut(&id) {
                        node.url = url;
                    }
                });
                request_animation_frame(move || board.focus_card(&id));
            }
            None => {
                let at = board.center();
                if let Some(id) = board.add_card("link", "", at, |node| node.url = url) {
                    request_animation_frame(move || board.focus_card(&id));
                }
            }
        }
    };
    let title = if editing.with_value(Option::is_some) {
        "Edit link"
    } else {
        "Add a link"
    };
    view! {
        <Dialog title=title on_close=close>
            <form class="canvas-link-form" on:submit=move |e: web_sys::SubmitEvent| { e.prevent_default(); submit(); }>
                <label>"Web address"
                    <input node_ref=field autofocus type="url" inputmode="url" placeholder="https://" prop:value=move || value.get()
                        aria-invalid=move || (!problem.with(String::is_empty)).to_string()
                        on:input=move |e| { value.set(event_target_value(&e)); problem.set(String::new()); }/>
                </label>
                {move || (!problem.with(String::is_empty)).then(|| view! { <p class="field-error" role="alert"><Icon name="alert"/>{problem.get()}</p> })}
                <div class="dialog-actions">
                    <button type="button" class="text-button" on:click=move |_| close.run(())>"Cancel"</button>
                    <button class="primary-button" disabled=move || value.with(|v| v.trim().is_empty())>{if editing.with_value(Option::is_some) { "Save" } else { "Add link" }}</button>
                </div>
            </form>
        </Dialog>
    }
}
