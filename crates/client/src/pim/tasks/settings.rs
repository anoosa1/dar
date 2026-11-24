//! Tasks settings sheet (SettingsSheet B with General, Import and export, Keyboard shortcuts)
//! and `?` keyboard shortcuts cheat sheet dialog.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use super::model::{
    CollectionList, ITEMS_BASE, LISTS_BASE, SmartViewVis, TaskListResponse, export_markdown,
    sort_label, view_label,
};
use crate::api::{self, BrowserSession};
use crate::pim::{Collection, PimLive, send};
use crate::settings::{SaveProblems, Saver, Segmented, SettingRow, SettingSwitch, SettingsSheet};
use crate::ui::{Dialog, document, window};
use crate::workspace::ReactiveSelect;

fn trigger_download(filename: &str, content: &str, mime: &str) {
    let parts = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(content));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    if let Ok(a) = document().create_element("a")
        && let Ok(anchor) = a.dyn_into::<web_sys::HtmlAnchorElement>()
    {
        anchor.set_href(&url);
        anchor.set_download(filename);
        anchor.click();
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

#[component]
pub fn TasksSettings(
    session: BrowserSession,
    pim: PimLive,
    on_close: Callback<()>,
) -> impl IntoView {
    let saver = Saver::new(session.clone());
    let section = RwSignal::new("General");
    let collections: RwSignal<Vec<Collection>> = RwSignal::new(Vec::new());
    let collections_loaded = RwSignal::new(false);

    spawn_local(async move {
        if let Ok(list) = api::get::<CollectionList>(LISTS_BASE).await {
            collections.try_set(list.items);
        }
        collections_loaded.try_set(true);
    });

    let text = move |key: &'static str, default: &'static str| {
        Signal::derive(move || saver.value(key).as_str().unwrap_or(default).to_owned())
    };
    let flag = move |key: &'static str, default: bool| {
        Signal::derive(move || saver.value(key).as_bool().unwrap_or(default))
    };
    let set = move |key: &'static str, value: Value, message: String| {
        saver.change(key, value, message, true)
    };

    let view_sig = RwSignal::new("today".to_owned());
    let sort_sig = RwSignal::new("due".to_owned());
    let default_list_sig = RwSignal::new(String::new());

    Effect::new(move |_| {
        let v = saver
            .value("tasksView")
            .as_str()
            .unwrap_or("today")
            .to_owned();
        view_sig.set(if v == "important" {
            "starred".into()
        } else {
            v
        });
        let s = saver
            .value("tasksSort")
            .as_str()
            .unwrap_or("due")
            .to_owned();
        sort_sig.set(s);
        let dl = saver
            .value("tasksDefaultList")
            .as_str()
            .unwrap_or("")
            .to_owned();
        default_list_sig.set(dl);
    });

    let view_options = Signal::derive(|| {
        [
            ("today", "Today"),
            ("week", "Next 7 days"),
            ("inbox", "Inbox"),
            ("starred", "Starred"),
            ("all", "All tasks"),
            ("upcoming", "Upcoming"),
            ("completed", "Completed"),
        ]
        .into_iter()
        .map(|(v, l)| (v.to_owned(), l.to_owned()))
        .collect::<Vec<_>>()
    });

    let sort_options = Signal::derive(|| {
        [
            ("due", "Due date"),
            ("priority", "Priority"),
            ("manual", "My order"),
            ("start", "Start date"),
            ("title", "Title"),
            ("modified", "Recently edited"),
        ]
        .into_iter()
        .map(|(v, l)| (v.to_owned(), l.to_owned()))
        .collect::<Vec<_>>()
    });

    let list_options = Signal::derive(move || {
        let mut opts = vec![(String::new(), "Active list (or first writable)".to_owned())];
        for c in collections.get() {
            if c.role != "read" {
                opts.push((c.id, c.name));
            }
        }
        opts
    });

    let smart_vis = Signal::derive(move || {
        let v = saver.value("tasksSmartViews");
        SmartViewVis::from_value((!v.is_null()).then_some(&v))
    });

    let update_smart_view = move |key: &'static str, mode: String| {
        let mut vis = smart_vis.get_untracked();
        match key {
            "week" => vis.week = mode.clone(),
            "inbox" => vis.inbox = mode.clone(),
            "starred" => vis.starred = mode.clone(),
            "completed" => vis.completed = mode.clone(),
            _ => {}
        }
        let body = json!({
            "week": vis.week,
            "inbox": vis.inbox,
            "starred": vis.starred,
            "completed": vis.completed,
        });
        set(
            "tasksSmartViews",
            body,
            format!("{} visibility set to {mode}", view_label(key)),
        );
    };

    let dav_url_sv = StoredValue::new(
        window()
            .location()
            .origin()
            .map(|o| format!("{o}/dav/"))
            .unwrap_or_else(|_| "/dav/".to_owned()),
    );
    let dav_copied = RwSignal::new(false);
    let session_sv = StoredValue::new(session);

    view! {
        <SettingsSheet
            title="Tasks settings"
            icon="check"
            sections=vec!["General", "Import and export", "Keyboard shortcuts"]
            section=section
            on_close=on_close
            notice=saver.notice
        >
            <SaveProblems saver=saver/>
            {move || match section.get() {
                "Import and export" => view! {
                    <TasksTransfers session=session_sv.get_value() pim=pim/>
                }.into_any(),
                "Keyboard shortcuts" => view! {
                    <TasksShortcutsList/>
                }.into_any(),
                _ if saver.preferences.with(Option::is_none) => view! {
                    <p role="status">"Loading settings\u{2026}"</p>
                }.into_any(),
                _ => {
                    let dav_copy = dav_url_sv.get_value();
                    let dav_show = dav_url_sv.get_value();
                    let zone = text("timezone", "UTC");
                    view! {
                        <section class="settings-group" aria-label="Defaults">
                            <h3>"Defaults"</h3>
                            <SettingRow label="Default layout" hint="How Tasks opens" state=saver.state("tasksLayout") stacked=true>
                                <Segmented
                                    label="Default layout"
                                    value=text("tasksLayout", "list")
                                    options=vec![("list", "List"), ("board", "Board"), ("kanban", "Status")]
                                    on_change=Callback::new(move |v: String| {
                                        let lbl = match v.as_str() { "board" => "Board", "kanban" => "Status", _ => "List" };
                                        set("tasksLayout", v.into(), format!("Default layout set to {lbl}"));
                                    })
                                />
                            </SettingRow>
                            <SettingRow label="Default view" hint="The smart view Tasks opens on" state=saver.state("tasksView")>
                                <ReactiveSelect
                                    aria_label="Default view"
                                    options=view_options
                                    value=view_sig
                                    on_change=Callback::new(move |v: String| {
                                        let lbl = view_label(&v);
                                        set("tasksView", v.into(), format!("Tasks opens on {lbl}"));
                                    })
                                />
                            </SettingRow>
                            <SettingRow label="Default sorting" hint="Order of tasks inside lists and views" state=saver.state("tasksSort")>
                                <ReactiveSelect
                                    aria_label="Default sorting"
                                    options=sort_options
                                    value=sort_sig
                                    on_change=Callback::new(move |v: String| {
                                        let lbl = sort_label(&v);
                                        set("tasksSort", v.into(), format!("Tasks sorted by {lbl}"));
                                    })
                                />
                            </SettingRow>
                            <SettingRow label="Quick-add list" hint="Where quick-added tasks go from smart views" state=saver.state("tasksDefaultList")>
                                <ReactiveSelect
                                    aria_label="Quick-add list"
                                    options=list_options
                                    value=default_list_sig
                                    disabled=Signal::derive(move || !collections_loaded.get())
                                    on_change=Callback::new(move |v: String| {
                                        set("tasksDefaultList", v.into(), "Default quick-add list updated".into());
                                    })
                                />
                            </SettingRow>
                        </section>

                        <section class="settings-group" aria-label="Display and behavior">
                            <h3>"Display and behavior"</h3>
                            <SettingSwitch
                                label="Show completed tasks inline"
                                hint="Keep a collapsible Completed drawer at the bottom of lists and board columns"
                                state=saver.state("tasksShowCompleted")
                                checked=flag("tasksShowCompleted", true)
                                on_change=Callback::new(move |v: bool| {
                                    set("tasksShowCompleted", v.into(), if v { "Completed drawer shown" } else { "Completed drawer hidden" }.into());
                                })
                            />
                            <SettingSwitch
                                label="Expand subtasks by default"
                                hint="Show nested subtask trees open when loading lists"
                                state=saver.state("tasksExpandSubtasks")
                                checked=flag("tasksExpandSubtasks", false)
                                on_change=Callback::new(move |v: bool| {
                                    set("tasksExpandSubtasks", v.into(), if v { "Subtasks expanded by default" } else { "Subtasks collapsed by default" }.into());
                                })
                            />
                            <SettingSwitch
                                label="Show Today\u{2019}s pace"
                                hint="Display daily completion progress in the sidebar"
                                state=saver.state("tasksShowPace")
                                checked=flag("tasksShowPace", true)
                                on_change=Callback::new(move |v: bool| {
                                    set("tasksShowPace", v.into(), if v { "Today\u{2019}s pace shown" } else { "Today\u{2019}s pace hidden" }.into());
                                })
                            />
                            <SettingSwitch
                                label="Show dated tasks in Calendar"
                                hint="Tasks with a due date appear on Calendar grids"
                                state=saver.state("calendarShowTasks")
                                checked=flag("calendarShowTasks", true)
                                on_change=Callback::new(move |v: bool| {
                                    set("calendarShowTasks", v.into(), if v { "Tasks shown in Calendar" } else { "Tasks hidden in Calendar" }.into());
                                })
                            />
                            <SettingSwitch
                                label="Confirm before deleting tasks"
                                hint="Ask before permanently removing a task (deleting parents with subtasks always asks)"
                                state=saver.state("tasksConfirmDelete")
                                checked=flag("tasksConfirmDelete", true)
                                on_change=Callback::new(move |v: bool| {
                                    set("tasksConfirmDelete", v.into(), if v { "Delete confirmation on" } else { "Delete confirmation off" }.into());
                                })
                            />
                        </section>

                        <section class="settings-group" aria-label="Sidebar smart views">
                            <h3>"Sidebar smart views"</h3>
                            {([
                                ("week", "Next 7 days", "Tasks due in the next week"),
                                ("inbox", "Inbox", "Unscheduled top-level tasks"),
                                ("starred", "Starred", "Starred and high-priority tasks"),
                                ("completed", "Completed", "Finished tasks archive"),
                            ]).into_iter().map(|(key, label, hint)| {
                                let sig = Signal::derive(move || {
                                    let vis = smart_vis.get();
                                    match key {
                                        "week" => vis.week,
                                        "inbox" => vis.inbox,
                                        "starred" => vis.starred,
                                        _ => vis.completed,
                                    }
                                });
                                view! {
                                    <SettingRow label=label hint=hint state=saver.state("tasksSmartViews")>
                                        <Segmented
                                            label=format!("{label} visibility")
                                            value=sig
                                            options=vec![("show", "Show"), ("auto", "Auto"), ("hide", "Hide")]
                                            on_change=Callback::new(move |mode: String| update_smart_view(key, mode))
                                        />
                                    </SettingRow>
                                }
                            }).collect_view()}
                        </section>

                        <section class="settings-group" aria-label="CalDAV sync">
                            <h3>"CalDAV sync"</h3>
                            <div class="contacts-dav-box">
                                <p class="field-hint">"Sync task lists with Tasks.org, DAVx\u{2075}, Thunderbird, or Apple Reminders using an app password."</p>
                                <div class="contacts-dav-row">
                                    <code class="contacts-dav-url">{dav_show}</code>
                                    <button
                                        type="button"
                                        class="secondary-button"
                                        on:click=move |_| {
                                            let url = dav_copy.clone();
                                            let cb = window().navigator().clipboard();
                                            if !cb.is_undefined() && !cb.is_null() {
                                                let p = cb.write_text(&url);
                                                spawn_local(async move {
                                                    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
                                                    dav_copied.try_set(true);
                                                    crate::sleep(1800).await;
                                                    dav_copied.try_set(false);
                                                });
                                            }
                                        }
                                    >
                                        {move || if dav_copied.get() { "Copied" } else { "Copy URL" }}
                                    </button>
                                </div>
                                <a class="text-button" href="/settings?section=security">"Manage app passwords"</a>
                            </div>
                            <SettingRow label="Timezone" hint=format!("{} \u{00b7} used for Today and Overdue dates", zone.get()) state=Signal::derive(|| None)>
                                <a class="secondary-button" href="/settings?section=regional">"Regional settings"</a>
                            </SettingRow>
                        </section>
                    }.into_any()
                }
            }}
        </SettingsSheet>
    }
}

#[component]
fn TasksTransfers(session: BrowserSession, pim: PimLive) -> impl IntoView {
    let collections: RwSignal<Vec<Collection>> = RwSignal::new(Vec::new());
    let selected = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let loaded = RwSignal::new(false);
    let results: RwSignal<Option<Vec<Value>>> = RwSignal::new(None);
    let content = RwSignal::new(String::new());
    let file = NodeRef::<html::Input>::new();
    let csrf = StoredValue::new(session.csrf_token);

    spawn_local(async move {
        match api::get::<CollectionList>(LISTS_BASE).await {
            Ok(list) => {
                selected.try_set(list.items.first().map(|c| c.id.clone()).unwrap_or_default());
                collections.try_set(list.items);
            }
            Err(e) => {
                error.try_set(e.message);
            }
        }
        loaded.try_set(true);
    });

    let collection = Memo::new(move |_| {
        collections.with(|c| c.iter().find(|x| x.id == selected.get()).cloned())
    });
    let read_only =
        Memo::new(move |_| collection.with(|c| c.as_ref().is_some_and(|c| c.role == "read")));
    let options = Signal::derive(move || {
        collections
            .get()
            .into_iter()
            .map(|c| (c.id, c.name))
            .collect::<Vec<_>>()
    });

    let export_md = move || {
        let Some(c) = collection.get_untracked() else {
            return;
        };
        busy.set(true);
        error.set(String::new());
        spawn_local(async move {
            let url = format!("{ITEMS_BASE}?view=all&collection={}&limit=200", c.id);
            match api::get::<TaskListResponse>(&url).await {
                Ok(resp) => {
                    let md = export_markdown(&c.name, &resp.items);
                    let slug: String = c
                        .name
                        .to_lowercase()
                        .chars()
                        .map(|ch| if ch.is_alphanumeric() { ch } else { '-' })
                        .collect();
                    let fname = format!("{}.md", slug.trim_matches('-'));
                    trigger_download(
                        if fname == ".md" { "tasks.md" } else { &fname },
                        &md,
                        "text/markdown;charset=utf-8",
                    );
                }
                Err(e) => {
                    error.try_set(e.message);
                }
            }
            busy.try_set(false);
        });
    };

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
                let value = match &source {
                    Some(f) => wasm_bindgen_futures::JsFuture::from(f.text())
                        .await
                        .ok()
                        .and_then(|v| v.as_string())
                        .unwrap_or_default(),
                    None => pasted,
                };
                let result: Value = send(
                    &format!("{LISTS_BASE}/{}/import", c.id),
                    "POST",
                    Some(json!({ "content": value })),
                    &csrf.get_value(),
                    None,
                )
                .await
                .map_err(|e| e.message)?;
                Ok(result["items"].as_array().cloned().unwrap_or_default())
            }
            .await;
            match outcome {
                Ok(items) => {
                    results.try_set(Some(items));
                    pim.refresh();
                }
                Err(e) => {
                    error.try_set(e);
                }
            }
            busy.try_set(false);
        });
    };

    view! {
        <section class="settings-group" aria-label="Import and export">
            <h3>"Import and export"</h3>
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            {move || if !loaded.get() {
                view! { <p role="status">"Loading task lists\u{2026}"</p> }.into_any()
            } else if collections.with(Vec::is_empty) {
                view! { <p>"No task lists available."</p> }.into_any()
            } else {
                view! {
                    <label>"Task list"
                        <ReactiveSelect
                            aria_label="Task list for import or export"
                            options=options
                            value=selected
                            disabled=Signal::derive(move || busy.get())
                            on_change=Callback::new(move |_| results.set(None))
                        />
                    </label>
                    {move || collection.get().map(|c| view! {
                        <div class="tasks-export-actions">
                            <a class="secondary-button" href=format!("{LISTS_BASE}/{}/export", c.id)>"Export ICS"</a>
                            <button type="button" class="secondary-button" disabled=move || busy.get() on:click=move |_| export_md()>
                                "Export Markdown"
                            </button>
                        </div>
                    })}
                    <h4>"Import ICS"</h4>
                    <label>"ICS file"
                        <input type="file" node_ref=file accept=".ics,text/calendar" disabled=move || busy.get() || read_only.get()/>
                    </label>
                    <label>"Or paste content"
                        <textarea
                            rows="6"
                            prop:value=move || content.get()
                            disabled=move || busy.get() || read_only.get()
                            on:input=move |e| content.set(event_target_value(&e))
                        ></textarea>
                    </label>
                    <button
                        type="button"
                        class="primary-button"
                        disabled=move || busy.get() || collection.with(Option::is_none) || read_only.get()
                        on:click=move |_| import()
                    >
                        {move || if busy.get() { "Importing\u{2026}" } else { "Import ICS" }}
                    </button>
                    <p class="field-hint">"Task lists share calendars with Calendar. ICS transfers include both events and tasks."</p>
                    {move || read_only.get().then(|| view! {
                        <p class="field-hint">"Import requires write access to this task list."</p>
                    })}
                    {move || results.get().map(|items| {
                        let failed: Vec<(usize, String)> = items
                            .iter()
                            .enumerate()
                            .filter_map(|(i, r)| r["error"].as_str().map(|e| (i, e.to_owned())))
                            .collect();
                        view! {
                            <div role="status">
                                <p>{format!("{} imported; {} failed.", items.len() - failed.len(), failed.len())}</p>
                                {failed.into_iter().map(|(i, e)| view! { <p>{format!("Item {}: {e}", i + 1)}</p> }).collect_view()}
                            </div>
                        }
                    })}
                }.into_any()
            }}
        </section>
    }
}

#[component]
pub fn TasksShortcutsList() -> impl IntoView {
    view! {
        <dl class="ui-shortcuts">
            <dt>"C"</dt><dd>"Create a new task"</dd>
            <dt>"J / K"</dt><dd>"Next or previous task"</dd>
            <dt>"Space"</dt><dd>"Complete or reopen the selected task"</dd>
            <dt>"S"</dt><dd>"Star or unstar the selected task"</dd>
            <dt>"E"</dt><dd>"Open full editor for the selected task"</dd>
            <dt>"X"</dt><dd>"Toggle bulk selection on the current task"</dd>
            <dt>"1 / 2 / 3"</dt><dd>"Switch between List, Board, and Status views"</dd>
            <dt>"Ctrl / Cmd K"</dt><dd>"Search all apps outside editors and dialogs"</dd>
            <dt>"Escape"</dt><dd>"Clear selection, close details, dialogs, or settings"</dd>
            <dt>"?"</dt><dd>"Show keyboard shortcuts"</dd>
        </dl>
    }
}

#[component]
pub fn ShortcutsDialog(on_close: Callback<()>) -> impl IntoView {
    view! {
        <Dialog title="Keyboard shortcuts" on_close=on_close>
            <TasksShortcutsList/>
            <div class="dialog-actions">
                <button type="button" class="primary-button" on:click=move |_| { on_close.try_run(()); }>"Done"</button>
            </div>
        </Dialog>
    }
}
