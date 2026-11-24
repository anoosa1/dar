//! List Board (side-by-side task list columns, Google Tasks full-screen style) and
//! Status Kanban (To do / In progress / Completed lanes) with HTML5 drag-and-drop.

use std::collections::HashSet;

use leptos::prelude::*;

use super::model::{TaskFilterChip, TaskItem, due_label, filter_tasks, priority_class, sort_tasks};
use crate::icons::Icon;
use crate::pim::{Collection, CollectionDot};

#[component]
pub fn TasksBoard(
    collections: Signal<Vec<Collection>>,
    all_items: Signal<Vec<TaskItem>>,
    hidden_board_lists: RwSignal<HashSet<String>>,
    active_tag: RwSignal<Option<String>>,
    filter_chip: RwSignal<TaskFilterChip>,
    sort: RwSignal<String>,
    today: Memo<String>,
    selected_id: Memo<Option<String>>,
    show_completed_inline: Signal<bool>,
    busy: RwSignal<bool>,
    on_open_task: Callback<String>,
    on_complete_task: Callback<TaskItem>,
    on_toggle_star: Callback<TaskItem>,
    on_quick_add_to_list: Callback<(String, String)>,
    on_drag_move_to_list: Callback<(String, String)>,
    on_new_list: Callback<()>,
) -> impl IntoView {
    let dragging_id = RwSignal::new(Option::<String>::None);
    let drag_over_col = RwSignal::new(Option::<String>::None);

    let visible_cols = Memo::new(move |_| {
        let hidden = hidden_board_lists.get();
        collections
            .get()
            .into_iter()
            .filter(|c| !hidden.contains(&c.id))
            .collect::<Vec<_>>()
    });

    view! {
        <div class="tasks-board-scroll" role="region" aria-label="Task lists board">
            {move || {
                let cols = visible_cols.get();
                if cols.is_empty() {
                    return view! {
                        <div class="workspace-empty">
                            <h2>"No lists shown on the board"</h2>
                            <p>"Toggle lists in the sidebar or create a new task list."</p>
                            <button type="button" class="primary-button" on:click=move |_| { on_new_list.try_run(()); }>
                                <Icon name="plus"/>"New task list"
                            </button>
                        </div>
                    }.into_any();
                }

                let items = all_items.get();
                let tg = active_tag.get();
                let chip = filter_chip.get();
                let s = sort.get();
                let t = today.get();
                let show_comp = show_completed_inline.get();

                view! {
                    <div class="tasks-board-columns">
                        {cols.into_iter().map(|col| {
                            let col_id = col.id.clone();
                            let col_id_drop = col.id.clone();
                            let col_id_over = col.id.clone();
                            let col_id_add = col.id.clone();
                            let col_id_cls = col.id.clone();
                            let read_only = col.role == "read";
                            let list_items = filter_tasks(&items, "all", &col_id, tg.as_deref(), chip, &t);
                            let mut open_tasks: Vec<TaskItem> = Vec::new();
                            let mut done_tasks: Vec<TaskItem> = Vec::new();
                            for item in list_items {
                                if item.is_done() || item.is_cancelled() {
                                    done_tasks.push(item);
                                } else {
                                    open_tasks.push(item);
                                }
                            }
                            sort_tasks(&mut open_tasks, &s, &t);
                            sort_tasks(&mut done_tasks, "modified", &t);

                            let open_count = open_tasks.len();
                            let done_count = done_tasks.len();
                            let quick_val = RwSignal::new(String::new());
                            let comp_open = RwSignal::new(false);
                            let done_sv = StoredValue::new(done_tasks);

                            view! {
                                <section
                                    class=move || if drag_over_col.get().as_deref() == Some(col_id_cls.as_str()) {
                                        "tasks-board-col is-drop-target"
                                    } else {
                                        "tasks-board-col"
                                    }
                                    aria-label=col.name.clone()
                                    on:dragover=move |e: web_sys::DragEvent| {
                                        if !read_only {
                                            e.prevent_default();
                                            drag_over_col.set(Some(col_id_over.clone()));
                                        }
                                    }
                                    on:dragleave=move |_| {
                                        if drag_over_col.get_untracked().as_deref() == Some(col_id.as_str()) {
                                            drag_over_col.set(None);
                                        }
                                    }
                                    on:drop=move |e: web_sys::DragEvent| {
                                        e.prevent_default();
                                        drag_over_col.set(None);
                                        if read_only {
                                            return;
                                        }
                                        let id = e
                                            .data_transfer()
                                            .and_then(|dt| dt.get_data("text/plain").ok())
                                            .filter(|s| !s.is_empty())
                                            .or_else(|| dragging_id.get_untracked());
                                        dragging_id.set(None);
                                        if let Some(task_id) = id {
                                            on_drag_move_to_list.try_run((task_id, col_id_drop.clone()));
                                        }
                                    }
                                >
                                    <div class="tasks-board-col-head">
                                        <CollectionDot color=col.color.clone()/>
                                        <h2>{col.name.clone()}</h2>
                                        <span class="tasks-col-count">{open_count}</span>
                                    </div>

                                    {(!read_only).then(|| view! {
                                        <form class="tasks-col-add" on:submit=move |e: web_sys::SubmitEvent| {
                                            e.prevent_default();
                                            let title = quick_val.get_untracked().trim().to_owned();
                                            if !title.is_empty() {
                                                quick_val.set(String::new());
                                                on_quick_add_to_list.try_run((col_id_add.clone(), title));
                                            }
                                        }>
                                            <Icon name="plus"/>
                                            <input
                                                type="text"
                                                maxlength="1000"
                                                placeholder="Add a task\u{2026}"
                                                aria-label=format!("Add a task to {}", col.name)
                                                disabled=move || busy.get()
                                                prop:value=move || quick_val.get()
                                                on:input=move |e| quick_val.set(event_target_value(&e))
                                            />
                                        </form>
                                    })}

                                    <div class="tasks-board-cards">
                                        {open_tasks.into_iter().map(|row| {
                                            let id = row.id.clone();
                                            let id_sel = row.id.clone();
                                            let id_open = row.id.clone();
                                            let id_drag = row.id.clone();
                                            let title = if row.data.title.is_empty() { "Untitled task".to_owned() } else { row.data.title.clone() };
                                            let desc = row.data.description.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned);
                                            let due = row.effective_due().to_owned();
                                            let all_day = row.effective_all_day();
                                            let overdue = row.is_overdue(&t);
                                            let starred = row.is_starred();
                                            let ro = row.role == "read";
                                            let sub_len = row.children_slice().len();
                                            let sub_done = row.children_slice().iter().filter(|c| c.status == "COMPLETED").count();
                                            let r_comp = row.clone();
                                            let r_star = row.clone();
                                            let pri_cls = priority_class(row.data.priority);
                                            let check_lbl = format!("Complete {title}");
                                            let star_lbl = format!("{} {title}", if starred { "Unstar" } else { "Star" });

                                            view! {
                                                <article
                                                    class=move || if selected_id.with(|s| s.as_deref() == Some(id_sel.as_str())) {
                                                        "tasks-card selected"
                                                    } else {
                                                        "tasks-card"
                                                    }
                                                    draggable=(!ro).to_string()
                                                    on:dragstart=move |e: web_sys::DragEvent| {
                                                        dragging_id.set(Some(id_drag.clone()));
                                                        if let Some(dt) = e.data_transfer() {
                                                            let _ = dt.set_data("text/plain", &id);
                                                        }
                                                    }
                                                    on:dragend=move |_| {
                                                        dragging_id.set(None);
                                                        drag_over_col.set(None);
                                                    }
                                                >
                                                    <div class="tasks-card-top">
                                                        <button
                                                            type="button"
                                                            class=format!("task-check priority-{pri_cls}")
                                                            aria-pressed="false"
                                                            aria-label=check_lbl
                                                            disabled=move || busy.get() || ro
                                                            on:click=move |_| { on_complete_task.try_run(r_comp.clone()); }
                                                        ></button>
                                                        <button
                                                            type="button"
                                                            class="tasks-card-open"
                                                            on:click=move |_| { on_open_task.try_run(id_open.clone()); }
                                                        >
                                                            <strong>{title}</strong>
                                                            {desc.map(|d| view! { <span class="tasks-card-desc">{d}</span> })}
                                                        </button>
                                                        <button
                                                            type="button"
                                                            class=if starred { "icon-button tasks-star-btn is-starred" } else { "icon-button tasks-star-btn" }
                                                            aria-pressed=starred.to_string()
                                                            aria-label=star_lbl
                                                            disabled=move || busy.get() || ro
                                                            on:click=move |_| { on_toggle_star.try_run(r_star.clone()); }
                                                        >
                                                            <Icon name="star"/>
                                                        </button>
                                                    </div>
                                                    {(!due.is_empty() || sub_len > 0 || !row.data.categories.is_empty()).then(|| view! {
                                                        <div class="tasks-card-meta">
                                                            {(!due.is_empty()).then(|| {
                                                                let d = due.clone();
                                                                view! {
                                                                    <span class=if overdue { "tasks-meta-pill is-overdue" } else { "tasks-meta-pill" }>
                                                                        <Icon name="calendar"/>
                                                                        {move || due_label(&d, all_day, &today.get())}
                                                                    </span>
                                                                }
                                                            })}
                                                            {(sub_len > 0).then(|| view! {
                                                                <span class="tasks-meta-pill"><Icon name="subtask"/>{format!("{sub_done}/{sub_len}")}</span>
                                                            })}
                                                            {row.data.categories.iter().take(2).cloned().map(|tg| view! {
                                                                <span class="row-tag">{format!("#{tg}")}</span>
                                                            }).collect_view()}
                                                        </div>
                                                    })}
                                                </article>
                                            }
                                        }).collect_view()}
                                    </div>

                                    {(show_comp && done_count > 0).then(|| view! {
                                        <div class="tasks-col-completed">
                                            <button
                                                type="button"
                                                class="tasks-completed-toggle"
                                                aria-expanded=move || comp_open.get().to_string()
                                                on:click=move |_| comp_open.update(|o| *o = !*o)
                                            >
                                                <Icon name="chevron"/>
                                                <span>{format!("Completed ({done_count})")}</span>
                                            </button>
                                            {move || comp_open.get().then(|| view! {
                                                <div class="tasks-col-done-list">
                                                    {done_sv.get_value().into_iter().map(|row| {
                                                        let id = row.id.clone();
                                                        let title = if row.data.title.is_empty() { "Untitled task".to_owned() } else { row.data.title.clone() };
                                                        let ro = row.role == "read";
                                                        let r_c = row.clone();
                                                        view! {
                                                            <div class="tasks-col-done-row">
                                                                <button
                                                                    type="button"
                                                                    class="task-check is-sub"
                                                                    aria-pressed="true"
                                                                    aria-label=format!("Reopen {title}")
                                                                    disabled=move || busy.get() || ro
                                                                    on:click=move |_| { on_complete_task.try_run(r_c.clone()); }
                                                                >
                                                                    <Icon name="check"/>
                                                                </button>
                                                                <button type="button" class="tasks-sub-title" on:click=move |_| { on_open_task.try_run(id.clone()); }>
                                                                    <span>{title}</span>
                                                                </button>
                                                            </div>
                                                        }
                                                    }).collect_view()}
                                                </div>
                                            })}
                                        </div>
                                    })}
                                </section>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
        </div>
    }
}

#[component]
pub fn TasksKanban(
    collections: Signal<Vec<Collection>>,
    items: Memo<Vec<TaskItem>>,
    today: Memo<String>,
    selected_id: Memo<Option<String>>,
    busy: RwSignal<bool>,
    on_open_task: Callback<String>,
    on_complete_task: Callback<TaskItem>,
    on_toggle_star: Callback<TaskItem>,
    on_drag_set_status: Callback<(String, &'static str)>,
) -> impl IntoView {
    let dragging_id = RwSignal::new(Option::<String>::None);
    let drag_over_lane = RwSignal::new(Option::<&'static str>::None);

    let lanes: [(&'static str, &'static str, &'static str); 3] = [
        ("NEEDS-ACTION", "To do", "todo"),
        ("IN-PROCESS", "In progress", "progress"),
        ("COMPLETED", "Completed", "done"),
    ];

    view! {
        <div class="tasks-kanban-scroll" role="region" aria-label="Status board">
            <div class="tasks-kanban-grid">
                {lanes.into_iter().map(|(status_code, label, slug)| {
                    view! {
                        <section
                            class=move || if drag_over_lane.get() == Some(status_code) {
                                format!("tasks-kanban-lane lane-{slug} is-drop-target")
                            } else {
                                format!("tasks-kanban-lane lane-{slug}")
                            }
                            aria-label=label
                            on:dragover=move |e: web_sys::DragEvent| {
                                e.prevent_default();
                                drag_over_lane.set(Some(status_code));
                            }
                            on:dragleave=move |_| {
                                if drag_over_lane.get_untracked() == Some(status_code) {
                                    drag_over_lane.set(None);
                                }
                            }
                            on:drop=move |e: web_sys::DragEvent| {
                                e.prevent_default();
                                drag_over_lane.set(None);
                                let id = e
                                    .data_transfer()
                                    .and_then(|dt| dt.get_data("text/plain").ok())
                                    .filter(|s| !s.is_empty())
                                    .or_else(|| dragging_id.get_untracked());
                                dragging_id.set(None);
                                if let Some(task_id) = id {
                                    on_drag_set_status.try_run((task_id, status_code));
                                }
                            }
                        >
                            {move || {
                                let t = today.get();
                                let list: Vec<TaskItem> = items
                                    .get()
                                    .into_iter()
                                    .filter(|i| match status_code {
                                        "COMPLETED" => i.is_done(),
                                        "IN-PROCESS" => i.data.status == "IN-PROCESS",
                                        _ => !i.is_done() && !i.is_cancelled() && i.data.status != "IN-PROCESS",
                                    })
                                    .collect();
                                let count = list.len();
                                view! {
                                    <div class="tasks-board-col-head">
                                        <span class=format!("tasks-lane-dot dot-{slug}") aria-hidden="true"></span>
                                        <h2>{label}</h2>
                                        <span class="tasks-col-count">{count}</span>
                                    </div>
                                    <div class="tasks-board-cards">
                                        {if list.is_empty() {
                                            view! { <p class="tasks-lane-empty">"Drop a task here"</p> }.into_any()
                                        } else {
                                            list.into_iter().map(|row| {
                                                let id = row.id.clone();
                                                let id_sel = row.id.clone();
                                                let id_open = row.id.clone();
                                                let id_drag = row.id.clone();
                                                let title = if row.data.title.is_empty() { "Untitled task".to_owned() } else { row.data.title.clone() };
                                                let due = row.effective_due().to_owned();
                                                let all_day = row.effective_all_day();
                                                let overdue = row.is_overdue(&t);
                                                let done = row.is_done();
                                                let starred = row.is_starred();
                                                let ro = row.role == "read";
                                                let prog = row.data.progress;
                                                let pri_cls = priority_class(row.data.priority);
                                                let col_match = collections.with(|cs| cs.iter().find(|c| c.id == row.collection_id).cloned());
                                                let r_comp = row.clone();
                                                let r_star = row.clone();
                                                let check_lbl = format!("{} {title}", if done { "Reopen" } else { "Complete" });
                                                let star_lbl = format!("{} {title}", if starred { "Unstar" } else { "Star" });

                                                view! {
                                                    <article
                                                        class=move || {
                                                            let sel = selected_id.with(|s| s.as_deref() == Some(id_sel.as_str()));
                                                            format!("tasks-card{}{}", if sel { " selected" } else { "" }, if done { " is-done" } else { "" })
                                                        }
                                                        draggable=(!ro).to_string()
                                                        on:dragstart=move |e: web_sys::DragEvent| {
                                                            dragging_id.set(Some(id_drag.clone()));
                                                            if let Some(dt) = e.data_transfer() {
                                                                let _ = dt.set_data("text/plain", &id);
                                                            }
                                                        }
                                                        on:dragend=move |_| {
                                                            dragging_id.set(None);
                                                            drag_over_lane.set(None);
                                                        }
                                                    >
                                                        <div class="tasks-card-top">
                                                            <button
                                                                type="button"
                                                                class=format!("task-check priority-{pri_cls}")
                                                                aria-pressed=done.to_string()
                                                                aria-label=check_lbl
                                                                disabled=move || busy.get() || ro
                                                                on:click=move |_| { on_complete_task.try_run(r_comp.clone()); }
                                                            >
                                                                {done.then(|| view! { <Icon name="check"/> })}
                                                            </button>
                                                            <button
                                                                type="button"
                                                                class="tasks-card-open"
                                                                on:click=move |_| { on_open_task.try_run(id_open.clone()); }
                                                            >
                                                                <strong>{title}</strong>
                                                            </button>
                                                            <button
                                                                type="button"
                                                                class=if starred { "icon-button tasks-star-btn is-starred" } else { "icon-button tasks-star-btn" }
                                                                aria-pressed=starred.to_string()
                                                                aria-label=star_lbl
                                                                disabled=move || busy.get() || ro
                                                                on:click=move |_| { on_toggle_star.try_run(r_star.clone()); }
                                                            >
                                                                <Icon name="star"/>
                                                            </button>
                                                        </div>
                                                        {(status_code == "IN-PROCESS").then(|| view! {
                                                            <div class="tasks-card-progress">
                                                                <progress max="100" value=prog.to_string() aria-label="Task progress"></progress>
                                                                <span>{format!("{prog}%")}</span>
                                                            </div>
                                                        })}
                                                        <div class="tasks-card-meta">
                                                            {(!due.is_empty()).then(|| {
                                                                let d = due.clone();
                                                                view! {
                                                                    <span class=if overdue { "tasks-meta-pill is-overdue" } else { "tasks-meta-pill" }>
                                                                        <Icon name="calendar"/>
                                                                        {move || due_label(&d, all_day, &today.get())}
                                                                    </span>
                                                                }
                                                            })}
                                                            {col_match.map(|c| view! {
                                                                <span class="tasks-list-pill"><CollectionDot color=c.color/>{c.name}</span>
                                                            })}
                                                        </div>
                                                    </article>
                                                }
                                            }).collect_view().into_any()
                                        }}
                                    </div>
                                }
                            }}
                        </section>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}
