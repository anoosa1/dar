//! The inline Files folder picker shared by app forms (`file-picker.tsx`, folder mode).

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;

use super::types::FileTile;
use crate::api;
use crate::icons::Icon;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Selection {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct Crumb {
    id: String,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    folder: Option<Selection>,
    ancestors: Vec<Crumb>,
    items: Vec<Selection>,
    next_cursor: Option<String>,
}

/// Folder mode: the destination is the highlighted folder row, otherwise the folder being
/// viewed; a row's chevron (or double-click) opens it.
#[component]
pub fn FolderPicker(
    #[prop(into)] initial_folder_id: String,
    exclude_ids: Vec<String>,
    #[prop(into)] disabled: Signal<bool>,
    on_change: Callback<Option<Selection>>,
) -> impl IntoView {
    let parent = RwSignal::new(initial_folder_id);
    let cursor = RwSignal::new(String::new());
    let previous: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let page: RwSignal<Option<Page>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let attempt = RwSignal::new(0u32);
    let query = RwSignal::new(String::new());
    let selected: RwSignal<Option<Selection>> = RwSignal::new(None);
    let sequence = RwSignal::new(0u32);
    let here = |value: &Option<Page>| -> Option<Selection> {
        Some(match value.as_ref().and_then(|p| p.folder.clone()) {
            Some(folder) => Selection {
                kind: "folder".into(),
                ..folder
            },
            None => Selection {
                id: String::new(),
                name: "All files".into(),
                kind: "folder".into(),
            },
        })
    };
    Effect::new(move |_| {
        attempt.track();
        let params = format!(
            "parentId={}&cursor={}&kind=folder",
            js_sys::encode_uri_component(&parent.get()),
            js_sys::encode_uri_component(&cursor.get())
        );
        let id = sequence.get_untracked() + 1;
        sequence.set(id);
        page.set(None);
        error.set(String::new());
        selected.set(None);
        {
            on_change.try_run(None);
        };
        spawn_local(async move {
            let value = api::get::<Page>(&format!("/api/v1/files/picker?{params}")).await;
            if sequence.try_get_untracked() != Some(id) {
                return;
            }
            match value {
                Ok(value) => {
                    let value = Some(value);
                    {
                        on_change.try_run(here(&value));
                    };
                    page.set(value);
                }
                Err(e) => error.set(e.message),
            }
        });
    });
    let browse = move |id: String| {
        parent.set(id);
        cursor.set(String::new());
        previous.set(Vec::new());
        query.set(String::new());
    };
    let choose = move |item: Selection| {
        let next = if selected.with_untracked(|s| s.as_ref().is_some_and(|s| s.id == item.id)) {
            None
        } else {
            Some(item)
        };
        selected.set(next.clone());
        {
            on_change.try_run(next.or_else(|| page.with_untracked(here)));
        };
    };
    let exclude = StoredValue::new(exclude_ids);
    let items = Memo::new(move |_| {
        let q = query.get().trim().to_lowercase();
        page.with(|p| {
            p.as_ref()
                .map(|p| {
                    p.items
                        .iter()
                        .filter(|i| {
                            !exclude.with_value(|e| e.contains(&i.id))
                                && i.name.to_lowercase().contains(&q)
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let up = move || {
        page.with(|p| {
            p.as_ref().and_then(|p| {
                p.folder
                    .as_ref()
                    .map(|_| p.ancestors.last().map(|a| a.id.clone()).unwrap_or_default())
            })
        })
    };
    let destination = move || selected.get().or_else(|| page.with(here));
    view! {
        <section class="dar-file-picker" aria-label="Dar folder picker" aria-busy=move || (page.with(Option::is_none) && error.with(String::is_empty)).to_string()>
            <div class="dar-picker-head">
                <button type="button" class="icon-button" aria-label="Up one folder" title="Up one folder" disabled=move || disabled.get() || up().is_none() on:click=move |_| browse(up().unwrap_or_default())><Icon name="back"/></button>
                <nav class="dar-picker-path" aria-label="Folder path">
                    {move || {
                        let p = page.get();
                        let ancestors = p.as_ref().map(|p| p.ancestors.clone()).unwrap_or_default();
                        let folder = p.and_then(|p| p.folder);
                        view! {
                            {if folder.is_some() { view! { <button type="button" disabled=move || disabled.get() on:click=move |_| browse(String::new())>"All files"</button> }.into_any() } else { view! { <strong aria-current="location">"All files"</strong> }.into_any() }}
                            {ancestors.into_iter().map(|part| { let id = part.id.clone(); view! { <span><Icon name="chevron"/><button type="button" disabled=move || disabled.get() on:click=move |_| browse(id.clone())>{part.name}</button></span> } }).collect_view()}
                            {folder.map(|f| view! { <span><Icon name="chevron"/><strong aria-current="location">{f.name}</strong></span> })}
                        }
                    }}
                </nav>
            </div>
            <div class="dar-picker-search"><Icon name="search"/><input type="search" aria-label="Filter folders in this folder" placeholder="Filter folders" prop:value=move || query.get() disabled=move || disabled.get() || page.with(Option::is_none)
                on:input=move |e| query.set(event_target_value(&e)) on:keydown=move |e| if e.key() == "Enter" { e.prevent_default() }/></div>
            {move || if !error.get().is_empty() {
                view! { <div class="dar-picker-state" role="alert"><p class="error">{error.get()}</p><button type="button" class="secondary-button" on:click=move |_| attempt.update(|n| *n += 1)>"Retry"</button></div> }.into_any()
            } else if page.with(Option::is_none) {
                view! { <p class="dar-picker-state muted" role="status">"Loading folders…"</p> }.into_any()
            } else {
                let label = page.with(|p| p.as_ref().and_then(|p| p.folder.as_ref().map(|f| f.name.clone())).unwrap_or_else(|| "All files".into()));
                let next_cursor = page.with(|p| p.as_ref().and_then(|p| p.next_cursor.clone()));
                view! {
                    <ul class="dar-picker-list" aria-label=label>
                        {items.get().into_iter().map(|item| {
                            let pressed = selected.with(|s| s.as_ref().is_some_and(|s| s.id == item.id));
                            let folder = item.kind == "folder";
                            let (open_id, open_id2, open_id3, chosen) = (item.id.clone(), item.id.clone(), item.id.clone(), item.clone());
                            let name = item.name.clone();
                            view! {
                                <li class=if pressed { "is-selected" } else { "" }>
                                    <button type="button" class="dar-picker-row" disabled=move || disabled.get() aria-pressed=pressed.to_string()
                                        on:click=move |_| choose(chosen.clone())
                                        on:dblclick=move |_| if folder { browse(open_id.clone()) }
                                        on:keydown=move |e| if folder && (e.key() == "ArrowRight" || (e.key() == "Enter" && e.shift_key())) { e.prevent_default(); browse(open_id2.clone()); }>
                                        <FileTile kind=item.kind.clone() name=item.name.clone() media_type=""/><span>{item.name.clone()}</span>{pressed.then(|| view! { <Icon name="check"/> })}
                                    </button>
                                    {folder.then(|| view! { <button type="button" class="icon-button dar-picker-open" aria-label=format!("Open {name}") title="Open folder" disabled=move || disabled.get() on:click=move |_| browse(open_id3.clone())><Icon name="chevron"/></button> })}
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                    {items.with(Vec::is_empty).then(|| view! { <p class="dar-picker-state muted">{if query.with(String::is_empty) { "No folders here." } else { "No matching folders here." }}</p> })}
                    {(!previous.with(Vec::is_empty) || next_cursor.is_some()).then(|| {
                        let next = next_cursor.clone();
                        view! {
                            <div class="dar-picker-pages">
                                <button type="button" class="text-button" disabled=move || disabled.get() || previous.with(Vec::is_empty) on:click=move |_| { cursor.set(previous.with_untracked(|p| p.last().cloned()).unwrap_or_default()); previous.update(|p| { p.pop(); }); }>"Previous"</button>
                                <button type="button" class="text-button" disabled=move || disabled.get() || next_cursor.is_none() on:click=move |_| { previous.update(|p| p.push(cursor.get_untracked())); cursor.set(next.clone().unwrap_or_default()); }>"Next"</button>
                            </div>
                        }
                    })}
                }.into_any()
            }}
            <p class="dar-picker-selection" role="status">{move || match destination() {
                Some(d) => view! { <Icon name=if d.kind == "folder" { "folder" } else { "file" }/><span>"Destination: "<strong>{d.name}</strong></span> }.into_any(),
                None => view! { <span>"Open a folder to use it."</span> }.into_any(),
            }}</p>
        </section>
    }
}
