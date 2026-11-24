//! Import and export of address books and calendars (`collection-transfers.tsx`).

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Collection, send};
use crate::api::{self, BrowserSession};
use crate::ui::Select;

#[derive(Deserialize)]
struct List {
    items: Vec<Collection>,
}

#[component]
pub fn CollectionTransfers(session: BrowserSession, app: &'static str) -> impl IntoView {
    let (endpoint, noun, plural, format, accept) = if app == "contacts" {
        (
            "/api/v1/contacts/books",
            "Address book",
            "address books",
            "vCard",
            ".vcf,.csv,text/vcard,text/csv",
        )
    } else {
        (
            "/api/v1/calendar/calendars",
            "Calendar",
            "calendars",
            "ICS",
            ".ics,text/calendar",
        )
    };
    let collections: RwSignal<Vec<Collection>> = RwSignal::new(Vec::new());
    let selected = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let loaded = RwSignal::new(false);
    let results: RwSignal<Option<Vec<Value>>> = RwSignal::new(None);
    let content = RwSignal::new(String::new());
    let file = NodeRef::<html::Input>::new();
    let csrf = StoredValue::new(session.csrf_token.clone());
    spawn_local(async move {
        match api::get::<List>(endpoint).await {
            Ok(list) => {
                selected.set(list.items.first().map(|c| c.id.clone()).unwrap_or_default());
                collections.set(list.items);
            }
            Err(e) => error.set(e.message),
        }
        loaded.set(true);
    });
    let collection = Memo::new(move |_| {
        collections.with(|c| c.iter().find(|x| x.id == selected.get()).cloned())
    });
    let read_only = move || collection.with(|c| c.as_ref().is_some_and(|c| c.role == "read"));
    let import = move || {
        let Some(c) = collection.get_untracked() else {
            return;
        };
        let source = file
            .get_untracked()
            .and_then(|f| f.files())
            .and_then(|l| l.item(0));
        let pasted = content.get_untracked();
        if source.is_none() && pasted.is_empty() {
            return;
        }
        busy.set(true);
        error.set(String::new());
        results.set(None);
        let source = send_wrapper::SendWrapper::new(source);
        spawn_local(async move {
            let source = source.take();
            let outcome: Result<Vec<Value>, String> = async {
                let size = source.as_ref().map_or(pasted.len() as f64, |f| f.size());
                if size > 2.0 * 1024.0 * 1024.0 {
                    return Err("Import exceeds 2 MiB.".to_owned());
                }
                let file_name = source.as_ref().map(|f| f.name().to_ascii_lowercase());
                let value = match &source {
                    Some(f) => wasm_bindgen_futures::JsFuture::from(f.text())
                        .await
                        .ok()
                        .and_then(|v| v.as_string())
                        .unwrap_or_default(),
                    None => pasted,
                };
                let is_csv = app == "contacts"
                    && (file_name.as_deref().is_some_and(|n| n.ends_with(".csv"))
                        || (!value.to_ascii_uppercase().contains("BEGIN:VCARD")
                            && value.lines().next().is_some_and(|l| l.contains(','))));
                let payload = if is_csv {
                    json!({ "content": value, "format": "csv" })
                } else {
                    json!({ "content": value })
                };
                let result: Value = send(
                    &format!("{endpoint}/{}/import", c.id),
                    "POST",
                    Some(payload),
                    &csrf.get_value(),
                    None,
                )
                .await
                .map_err(|e| e.message)?;
                Ok(result["items"].as_array().cloned().unwrap_or_default())
            }
            .await;
            match outcome {
                Ok(items) => results.set(Some(items)),
                Err(e) => error.set(e),
            }
            busy.set(false);
        });
    };
    view! {
        <section><h3>"Import and export"</h3>
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            {move || if !loaded.get() {
                view! { <p role="status">{format!("Loading {plural}…")}</p> }.into_any()
            } else if collections.with(Vec::is_empty) {
                view! { <p>{format!("No {plural} available.")}</p> }.into_any()
            } else {
                let options: Vec<(String, String)> = collections.get().into_iter().map(|c| (c.id, c.name)).collect();
                view! {
                    <label>{noun}<Select aria_label=format!("{noun} for import or export") options=options value=selected on_change=Callback::new(move |_| results.set(None))/></label>
                    {move || collection.get().map(|c| view! { <a class="secondary-button" href=format!("{endpoint}/{}/export", c.id)>{format!("Export {format}")}</a> })}
                    <h4>{format!("Import {format}")}</h4>
                    {(app == "contacts").then(|| view! { <p class="field-hint">"Supports vCard (.vcf) and Google / Outlook CSV (.csv)."</p> })}
                    <label>{format!("{format} file")}<input type="file" node_ref=file accept=accept disabled=move || busy.get() || read_only()/></label>
                    <label>"Or paste content"<textarea rows="6" prop:value=move || content.get() disabled=move || busy.get() || read_only() on:input=move |e| content.set(event_target_value(&e))></textarea></label>
                    <button type="button" class="primary-button" disabled=move || busy.get() || collection.with(Option::is_none) || read_only() on:click=move |_| import()>{move || if busy.get() { "Importing…".to_owned() } else { format!("Import {format}") }}</button>
                    {move || read_only().then(|| view! { <p class="field-hint">{format!("Import requires write access to this {}.", noun.to_lowercase())}</p> })}
                    {move || results.get().map(|items| {
                        let failed: Vec<(usize, String)> = items.iter().enumerate().filter_map(|(i, r)| r["error"].as_str().map(|e| (i, e.to_owned()))).collect();
                        view! {
                            <div role="status"><p>{format!("{} imported; {} failed.", items.len() - failed.len(), failed.len())}</p>
                                {failed.into_iter().map(|(i, e)| view! { <p>{format!("Item {}: {e}", i + 1)}</p> }).collect_view()}
                            </div>
                        }
                    })}
                }.into_any()
            }}
        </section>
    }
}
