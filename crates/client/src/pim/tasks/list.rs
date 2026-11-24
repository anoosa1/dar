//! Tasks header bar (title, count pill, List/Board/Kanban switcher, Sort menu, Shortcuts),
//! quick filter / bulk selection strip, Smart Quick-Add bar, due-date grouped list rows with
//! expandable nested subtask trees, inline star toggle, and collapsible Completed drawer.

use std::collections::HashSet;

use leptos::prelude::*;

use super::model::{
    DisplayTaskRow, ITEMS_BASE, TaskFilterChip, TaskItem, TaskLayout, due_label, priority_class,
    sort_label,
};
use crate::date_popover::add_days;
use crate::icons::Icon;
use crate::pim::presentation::recurrence_label;
use crate::pim::{Collection, CollectionDot};
use crate::ui::{ActionMenu, MenuAction};

#[component]
pub fn TasksHeader(
    heading_title: Signal<String>,
    total_count: Signal<usize>,
    overdue_count: Signal<usize>,
    active_tag: RwSignal<Option<String>>,
    layout: RwSignal<TaskLayout>,
    sort: RwSignal<String>,
    filter_chip: RwSignal<TaskFilterChip>,
    selected_ids: RwSignal<HashSet<String>>,
    busy: RwSignal<bool>,
    on_set_layout: Callback<TaskLayout>,
    on_set_sort: Callback<String>,
    on_open_shortcuts: Callback<()>,
    on_clear_tag: Callback<()>,
    on_bulk_complete: Callback<()>,
    on_bulk_today: Callback<()>,
    on_bulk_star: Callback<()>,
    on_bulk_move: Callback<()>,
    on_bulk_delete: Callback<()>,
    on_clear_selection: Callback<()>,
) -> impl IntoView {
    let sort_options: [(&'static str, &'static str); 6] = [
        ("due", "Due date"),
        ("priority", "Priority"),
        ("manual", "My order"),
        ("start", "Start date"),
        ("title", "Title"),
        ("modified", "Recently edited"),
    ];

    view! {
        <header class="tasks-heading">
            <div class="tasks-heading-title">
                <h1>{move || heading_title.get()}</h1>
                <span class="tasks-count-pill" aria-label="Task count">{move || total_count.get()}</span>
                {move || active_tag.get().map(|tg| view! {
                    <span class="pim-filter-chip">
                        <Icon name="tag"/>
                        <span>{format!("#{tg}")}</span>
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="Clear tag filter"
                            on:click=move |_| { on_clear_tag.try_run(()); }
                        >
                            <Icon name="close"/>
                        </button>
                    </span>
                })}
            </div>

            <div class="tasks-heading-controls">
                <div class="tasks-layout-switch" role="radiogroup" aria-label="Tasks layout">
                    <button
                        type="button"
                        role="radio"
                        aria-label="List layout"
                        aria-checked=move || (layout.get() == TaskLayout::List).to_string()
                        title="List (1)"
                        on:click=move |_| { on_set_layout.try_run(TaskLayout::List); }
                    >
                        <Icon name="list"/>
                        <span>"List"</span>
                    </button>
                    <button
                        type="button"
                        role="radio"
                        aria-label="Board layout"
                        aria-checked=move || (layout.get() == TaskLayout::Board).to_string()
                        title="List Board (2)"
                        on:click=move |_| { on_set_layout.try_run(TaskLayout::Board); }
                    >
                        <Icon name="board"/>
                        <span>"Board"</span>
                    </button>
                    <button
                        type="button"
                        role="radio"
                        aria-label="Status board layout"
                        aria-checked=move || (layout.get() == TaskLayout::Kanban).to_string()
                        title="Status Kanban (3)"
                        on:click=move |_| { on_set_layout.try_run(TaskLayout::Kanban); }
                    >
                        <Icon name="progress"/>
                        <span>"Status"</span>
                    </button>
                </div>

                <ActionMenu
                    label="Sort tasks"
                    class="workspace-sort tasks-sort-menu"
                    actions=move |_| {
                        let cur = sort.get_untracked();
                        sort_options
                            .into_iter()
                            .map(|(val, label)| {
                                let v = val.to_owned();
                                MenuAction::plain(label)
                                    .current(cur == val)
                                    .run(move || { on_set_sort.try_run(v.clone()); })
                            })
                            .collect()
                    }
                >
                    <Icon name="sort"/>
                    <span>{move || sort_label(&sort.get())}</span>
                </ActionMenu>

                <button
                    type="button"
                    class="icon-button tasks-shortcuts-btn"
                    aria-label="Keyboard shortcuts"
                    title="Keyboard shortcuts (?)"
                    on:click=move |_| { on_open_shortcuts.try_run(()); }
                >
                    <Icon name="keyboard"/>
                </button>
            </div>
        </header>

        {move || {
            let sel_count = selected_ids.with(HashSet::len);
            if sel_count > 0 {
                view! {
                    <div class="tasks-bulk-bar" role="region" aria-label="Bulk actions">
                        <span class="tasks-bulk-count">{format!("{sel_count} selected")}</span>
                        <div class="tasks-bulk-actions">
                            <button type="button" class="text-button" disabled=move || busy.get() on:click=move |_| { on_bulk_complete.try_run(()); }>
                                <Icon name="check"/><span>"Complete"</span>
                            </button>
                            <button type="button" class="text-button" disabled=move || busy.get() on:click=move |_| { on_bulk_today.try_run(()); }>
                                <Icon name="calendar"/><span>"Today"</span>
                            </button>
                            <button type="button" class="text-button" disabled=move || busy.get() on:click=move |_| { on_bulk_star.try_run(()); }>
                                <Icon name="star"/><span>"Star"</span>
                            </button>
                            <button type="button" class="text-button" disabled=move || busy.get() on:click=move |_| { on_bulk_move.try_run(()); }>
                                <Icon name="move"/><span>"Move"</span>
                            </button>
                            <button type="button" class="text-button is-danger" disabled=move || busy.get() on:click=move |_| { on_bulk_delete.try_run(()); }>
                                <Icon name="trash"/><span>"Delete"</span>
                            </button>
                            <button type="button" class="icon-button" aria-label="Clear selection" title="Clear selection" on:click=move |_| { on_clear_selection.try_run(()); }>
                                <Icon name="close"/>
                            </button>
                        </div>
                    </div>
                }.into_any()
            } else {
                let ov = overdue_count.get();
                let cur = filter_chip.get();
                view! {
                    <div class="tasks-filter-strip" role="group" aria-label="Quick filters">
                        <button
                            type="button"
                            class=if cur == TaskFilterChip::All { "tasks-filter-chip is-active" } else { "tasks-filter-chip" }
                            aria-pressed=(cur == TaskFilterChip::All).to_string()
                            on:click=move |_| filter_chip.set(TaskFilterChip::All)
                        >"All"</button>
                        <button
                            type="button"
                            class=if cur == TaskFilterChip::Overdue { "tasks-filter-chip is-active is-overdue" } else { "tasks-filter-chip" }
                            aria-pressed=(cur == TaskFilterChip::Overdue).to_string()
                            on:click=move |_| filter_chip.set(if cur == TaskFilterChip::Overdue { TaskFilterChip::All } else { TaskFilterChip::Overdue })
                        >
                            <span>"Overdue"</span>
                            {(ov > 0).then(|| view! { <span class="tasks-chip-badge">{ov}</span> })}
                        </button>
                        <button
                            type="button"
                            class=if cur == TaskFilterChip::High { "tasks-filter-chip is-active" } else { "tasks-filter-chip" }
                            aria-pressed=(cur == TaskFilterChip::High).to_string()
                            on:click=move |_| filter_chip.set(if cur == TaskFilterChip::High { TaskFilterChip::All } else { TaskFilterChip::High })
                        >"High priority"</button>
                        <button
                            type="button"
                            class=if cur == TaskFilterChip::Subtasks { "tasks-filter-chip is-active" } else { "tasks-filter-chip" }
                            aria-pressed=(cur == TaskFilterChip::Subtasks).to_string()
                            on:click=move |_| filter_chip.set(if cur == TaskFilterChip::Subtasks { TaskFilterChip::All } else { TaskFilterChip::Subtasks })
                        >"Has subtasks"</button>
                        <button
                            type="button"
                            class=if cur == TaskFilterChip::Recurring { "tasks-filter-chip is-active" } else { "tasks-filter-chip" }
                            aria-pressed=(cur == TaskFilterChip::Recurring).to_string()
                            on:click=move |_| filter_chip.set(if cur == TaskFilterChip::Recurring { TaskFilterChip::All } else { TaskFilterChip::Recurring })
                        >"Repeating"</button>
                    </div>
                }.into_any()
            }
        }}
    }
}

#[component]
pub fn TasksList(
    collections: Signal<Vec<Collection>>,
    collection: RwSignal<String>,
    view: RwSignal<String>,
    today: Memo<String>,
    display_rows: Memo<Vec<DisplayTaskRow>>,
    completed_rows: Memo<Vec<TaskItem>>,
    selected_id: Memo<Option<String>>,
    selected_ids: RwSignal<HashSet<String>>,
    expanded_subtasks: RwSignal<HashSet<String>>,
    show_completed_inline: Signal<bool>,
    quick_target: Memo<Option<Collection>>,
    busy: RwSignal<bool>,
    loading: RwSignal<bool>,
    on_quick_add: Callback<(String, Option<String>, Option<i32>, Option<String>)>,
    on_open_task: Callback<String>,
    on_complete_task: Callback<TaskItem>,
    on_toggle_star: Callback<TaskItem>,
    on_add_subtask: Callback<TaskItem>,
    on_duplicate_task: Callback<TaskItem>,
    on_move_task: Callback<(TaskItem, bool)>,
    on_delete_task: Callback<TaskItem>,
    on_toggle_select: Callback<String>,
    on_toggle_expand: Callback<String>,
    on_move_overdue_today: Callback<()>,
    on_pick_tag: Callback<String>,
) -> impl IntoView {
    let quick_text = RwSignal::new(String::new());
    // Quick-add option pills: None / Some("today") / Some("tomorrow")
    let quick_due = RwSignal::new(Option::<&'static str>::None);
    let quick_high = RwSignal::new(false);
    let quick_list_override = RwSignal::new(Option::<String>::None);
    let completed_open = RwSignal::new(false);

    let effective_quick_list = Memo::new(move |_| {
        if let Some(ov) = quick_list_override.get()
            && let Some(c) = collections.with(|cols| {
                cols.iter()
                    .find(|c| c.id == ov && c.role != "read")
                    .cloned()
            })
        {
            return Some(c);
        }
        quick_target.get()
    });

    let submit_quick = move || {
        let title = quick_text.get_untracked().trim().to_owned();
        if title.is_empty() {
            return;
        }
        let t = today.get_untracked();
        let due_val = match quick_due.get_untracked() {
            Some("today") => Some(t),
            Some("tomorrow") => Some(add_days(&t, 1)),
            _ => None,
        };
        let pri = if quick_high.get_untracked() {
            Some(1)
        } else {
            None
        };
        let list_id = effective_quick_list.get_untracked().map(|c| c.id);
        quick_text.set(String::new());
        quick_due.set(None);
        quick_high.set(false);
        on_quick_add.try_run((title, due_val, pri, list_id));
    };

    let cycle_quick_list = move || {
        let writable: Vec<Collection> = collections
            .get_untracked()
            .into_iter()
            .filter(|c| c.role != "read")
            .collect();
        if writable.len() <= 1 {
            return;
        }
        let cur_id = effective_quick_list
            .get_untracked()
            .map(|c| c.id)
            .unwrap_or_default();
        let idx = writable.iter().position(|c| c.id == cur_id).unwrap_or(0);
        let next = &writable[(idx + 1) % writable.len()];
        quick_list_override.set(Some(next.id.clone()));
    };

    view! {
        <div class="tasks-list-scroll">
            {move || (view.get() != "completed" || !collection.with(String::is_empty)).then(|| view! {
                <form class="task-quick-add tasks-smart-add" on:submit=move |e: web_sys::SubmitEvent| {
                    e.prevent_default();
                    submit_quick();
                }>
                    <Icon name="plus"/>
                    <label class="sr-only" for="task-quick-add">"Add a task"</label>
                    <input
                        id="task-quick-add"
                        prop:value=move || quick_text.get()
                        maxlength="1000"
                        disabled=move || busy.get() || effective_quick_list.with(Option::is_none)
                        placeholder=move || match effective_quick_list.get() {
                            Some(t) => format!(
                                "Add a task to {}{}",
                                t.name,
                                if view.get() == "today" && collection.with(String::is_empty) { " for today" } else { "" }
                            ),
                            None => "Create a task list to add tasks".to_owned(),
                        }
                        on:input=move |e| quick_text.set(event_target_value(&e))
                    />
                    <div class="tasks-quick-pills">
                        <button
                            type="button"
                            class=move || if quick_due.get() == Some("today") { "tasks-quick-pill is-active" } else { "tasks-quick-pill" }
                            aria-label="Quick add due today"
                            aria-pressed=move || (quick_due.get() == Some("today")).to_string()
                            on:click=move |_| quick_due.update(|d| *d = if *d == Some("today") { None } else { Some("today") })
                        >"Today"</button>
                        <button
                            type="button"
                            class=move || if quick_due.get() == Some("tomorrow") { "tasks-quick-pill is-active" } else { "tasks-quick-pill" }
                            aria-label="Quick add due tomorrow"
                            aria-pressed=move || (quick_due.get() == Some("tomorrow")).to_string()
                            on:click=move |_| quick_due.update(|d| *d = if *d == Some("tomorrow") { None } else { Some("tomorrow") })
                        >"Tomorrow"</button>
                        <button
                            type="button"
                            class=move || if quick_high.get() { "tasks-quick-pill is-active is-high" } else { "tasks-quick-pill" }
                            aria-label="Quick add high priority"
                            aria-pressed=move || quick_high.get().to_string()
                            on:click=move |_| quick_high.update(|h| *h = !*h)
                        >"High"</button>
                        {move || effective_quick_list.get().map(|list| {
                            let multi = collections.with(|cols| cols.iter().filter(|c| c.role != "read").count() > 1);
                            let list_aria = format!("Quick add list: {}", list.name);
                            view! {
                                <button
                                    type="button"
                                    class="tasks-quick-pill tasks-quick-list-pill"
                                    aria-label=list_aria
                                    disabled=!multi
                                    title=if multi { "Cycle destination list" } else { "Destination list" }
                                    on:click=move |_| cycle_quick_list()
                                >
                                    <CollectionDot color=list.color/>
                                    <span>{list.name}</span>
                                </button>
                            }
                        })}
                    </div>
                    {move || (!quick_text.get().trim().is_empty()).then(|| view! {
                        <button type="submit" class="primary-button tasks-quick-submit" disabled=move || busy.get()>"Add"</button>
                    })}
                </form>
            })}

            {move || loading.get().then(|| view! { <p class="tasks-status" role="status">"Loading tasks\u{2026}"</p> })}

            <div class=move || if selected_ids.with(|s| !s.is_empty()) { "tasks-rows is-selecting" } else { "tasks-rows" }>
                <For
                    each=move || display_rows.get()
                    key=|row| (
                        row.item.id.clone(),
                        row.item.revision,
                        row.item.data.status.clone(),
                        row.item.data.title.clone(),
                        row.item.data.starred,
                        row.item.data.priority,
                        row.item.data.progress,
                        row.item.data.due.clone(),
                        row.item.next_occurrence.as_ref().map(|o| o.recurrence_id.clone()),
                        row.section,
                        row.section_count,
                        row.subtasks.iter().map(|s| (s.id.clone(), s.revision, s.data.status.clone(), s.data.title.clone())).collect::<Vec<_>>(),
                    )
                    children=move |drow| {
                        let row = drow.item;
                        let section = drow.section;
                        let section_count = drow.section_count;
                        let subtasks = drow.subtasks;
                        let parent_title = drow.parent_title;

                        let task = row.data.clone();
                        let next_due = row.effective_due().to_owned();
                        let all_day = row.effective_all_day();
                        let is_done = row.is_done();
                        let is_starred = row.is_starred();
                        let read_only = row.role == "read";
                        let today_now = today.get_untracked();
                        let overdue = row.is_overdue(&today_now);

                        let row_id_sel = row.id.clone();
                        let row_id_chk = row.id.clone();
                        let row_id_chk2 = row.id.clone();
                        let row_id_open = row.id.clone();
                        let row_id_exp = row.id.clone();
                        let row_id_exp2 = row.id.clone();
                        let row_id_exp3 = row.id.clone();
                        let row_col_id = row.collection_id.clone();

                        let row_complete = row.clone();
                        let row_star = row.clone();
                        let row_actions = row.clone();

                        let title_display = if task.title.is_empty() {
                            "Untitled task".to_owned()
                        } else {
                            task.title.clone()
                        };
                        let check_label = format!("{} {}", if is_done { "Reopen" } else { "Complete" }, title_display);
                        let star_label = format!("{} {}", if is_starred { "Unstar" } else { "Star" }, title_display);
                        let select_label = format!("Select {}", title_display);
                        let action_label = format!("Actions for {}", title_display);
                        let desc_preview = task
                            .description
                            .lines()
                            .map(str::trim)
                            .find(|l| !l.is_empty())
                            .map(str::to_owned);

                        let sub_total = subtasks.len();
                        let sub_done = subtasks.iter().filter(|s| s.is_done()).count();
                        let subtasks_sv = StoredValue::new(subtasks);

                        view! {
                            <div class="workspace-entry">
                                {section.map(|sec| {
                                    let sec_slug = sec.to_lowercase().replace(' ', "-");
                                    view! {
                                        <div class=format!("tasks-section-bar task-section-{sec_slug}")>
                                            <h2 class=format!("task-section task-section-{sec_slug}")>
                                                <span>{sec}</span>
                                                <span class="tasks-section-count">{section_count}</span>
                                            </h2>
                                            {(sec == "Overdue").then(|| view! {
                                                <button
                                                    type="button"
                                                    class="text-button tasks-reschedule-btn"
                                                    disabled=move || busy.get()
                                                    on:click=move |_| { on_move_overdue_today.try_run(()); }
                                                >
                                                    "Move to Today"
                                                </button>
                                            })}
                                        </div>
                                    }
                                })}

                                <article class=move || {
                                    let is_sel = selected_id.with(|s| s.as_deref() == Some(row_id_sel.as_str()));
                                    let is_checked = selected_ids.with(|s| s.contains(&row_id_sel));
                                    format!(
                                        "workspace-row tasks-row{}{}{}",
                                        if is_sel { " selected" } else { "" },
                                        if is_done { " is-done" } else { "" },
                                        if is_checked { " is-bulk-checked" } else { "" },
                                    )
                                }>
                                    <label class="tasks-row-select">
                                        <input
                                            type="checkbox"
                                            aria-label=select_label
                                            prop:checked=move || selected_ids.with(|s| s.contains(&row_id_chk))
                                            on:change=move |_| { on_toggle_select.try_run(row_id_chk2.clone()); }
                                        />
                                    </label>

                                    <button
                                        type="button"
                                        class=format!("task-check priority-{}", priority_class(task.priority))
                                        aria-pressed=is_done.to_string()
                                        aria-label=check_label
                                        disabled=move || busy.get() || read_only
                                        on:click=move |_| { on_complete_task.try_run(row_complete.clone()); }
                                    >
                                        {is_done.then(|| view! { <Icon name="check"/> })}
                                    </button>

                                    <button
                                        type="button"
                                        class="workspace-row-content"
                                        on:click=move |_| {
                                            if selected_id.with_untracked(|s| s.as_deref() != Some(row_id_open.as_str())) {
                                                on_open_task.try_run(row_id_open.clone());
                                            }
                                        }
                                    >
                                        <strong class="tasks-row-title">{title_display}</strong>
                                        {desc_preview.map(|d| view! { <span class="tasks-row-desc">{d}</span> })}
                                        <span class="row-meta">
                                            {(!next_due.is_empty()).then(|| {
                                                let due_val = next_due.clone();
                                                view! {
                                                    <span class=if overdue { "tasks-meta-pill is-overdue" } else { "tasks-meta-pill" }>
                                                        <Icon name="calendar"/>
                                                        {move || due_label(&due_val, all_day, &today.get())}
                                                        {overdue.then(|| view! { <span class="sr-only">" (overdue)"</span> })}
                                                    </span>
                                                }
                                            })}
                                            {(!task.recurrence.is_empty() || task.recurring).then(|| view! {
                                                <span><Icon name="repeat"/>{recurrence_label(&task.recurrence, task.recurring)}</span>
                                            })}
                                            {(task.status == "IN-PROCESS" || (task.progress > 0 && task.progress < 100)).then(|| view! {
                                                <span class="tasks-meta-progress"><Icon name="progress"/>{format!("{}%", task.progress)}</span>
                                            })}
                                            {parent_title.map(|pt| view! {
                                                <span class="tasks-parent-crumb"><Icon name="subtask"/>{pt}</span>
                                            })}
                                            {move || {
                                                let cid = row_col_id.clone();
                                                collection.with(String::is_empty).then(|| {
                                                    collections.with(|cols| cols.iter().find(|c| c.id == cid).cloned()).map(|book| view! {
                                                        <span class="tasks-list-pill"><CollectionDot color=book.color/>{book.name}</span>
                                                    })
                                                }).flatten()
                                            }}
                                            {task.categories.iter().cloned().map(|tag| {
                                                let t_click = tag.clone();
                                                view! {
                                                    <span
                                                        class="row-tag tasks-row-tag"
                                                        on:click=move |e: web_sys::MouseEvent| {
                                                            e.stop_propagation();
                                                            on_pick_tag.try_run(t_click.clone());
                                                        }
                                                    >{format!("#{tag}")}</span>
                                                }
                                            }).collect_view()}
                                        </span>
                                    </button>

                                    {(sub_total > 0).then(|| view! {
                                        <button
                                            type="button"
                                            class=move || if expanded_subtasks.with(|e| e.contains(&row_id_exp)) { "tasks-sub-toggle is-open" } else { "tasks-sub-toggle" }
                                            aria-expanded=move || expanded_subtasks.with(|e| e.contains(&row_id_exp2)).to_string()
                                            title="Toggle subtasks"
                                            on:click=move |_| { on_toggle_expand.try_run(row_id_exp3.clone()); }
                                        >
                                            <Icon name="subtask"/>
                                            <span>{format!("{sub_done}/{sub_total}")}</span>
                                        </button>
                                    })}

                                    <button
                                        type="button"
                                        class=if is_starred { "icon-button tasks-star-btn is-starred" } else { "icon-button tasks-star-btn" }
                                        aria-pressed=is_starred.to_string()
                                        aria-label=star_label
                                        disabled=move || busy.get() || read_only
                                        on:click=move |_| { on_toggle_star.try_run(row_star.clone()); }
                                    >
                                        <Icon name="star"/>
                                    </button>

                                    <div class="workspace-row-tools">
                                        {(!read_only).then(|| view! {
                                            <ActionMenu
                                                label=action_label
                                                class="ui-file-actions"
                                                actions=move |_| {
                                                    let is_busy = busy.get_untracked();
                                                    let cols = collections.get_untracked();
                                                    let source_owner = cols
                                                        .iter()
                                                        .find(|c| c.id == row_actions.collection_id)
                                                        .map(|c| c.owner_id.as_str());
                                                    let can_move = !is_busy
                                                        && cols.iter().any(|c| {
                                                            c.id != row_actions.collection_id
                                                                && c.role != "read"
                                                                && Some(c.owner_id.as_str()) == source_owner
                                                        });
                                                    let r_sub = row_actions.clone();
                                                    let r_dup = row_actions.clone();
                                                    let r_move = row_actions.clone();
                                                    let r_copy = row_actions.clone();
                                                    let r_del = row_actions.clone();
                                                    vec![
                                                        MenuAction::new("Add subtask", "subtask")
                                                            .disabled(is_busy)
                                                            .run(move || { on_add_subtask.try_run(r_sub.clone()); }),
                                                        MenuAction::new("Duplicate task", "copy")
                                                            .disabled(is_busy)
                                                            .run(move || { on_duplicate_task.try_run(r_dup.clone()); }),
                                                        MenuAction::new("Move to\u{2026}", "move")
                                                            .disabled(!can_move)
                                                            .separator()
                                                            .run(move || { on_move_task.try_run((r_move.clone(), false)); }),
                                                        MenuAction::new("Copy to\u{2026}", "copy")
                                                            .disabled(is_busy)
                                                            .run(move || { on_move_task.try_run((r_copy.clone(), true)); }),
                                                        MenuAction::new("Export task", "download")
                                                            .href(format!("{ITEMS_BASE}/{}/export", row_actions.id)),
                                                        MenuAction::new("Delete task", "trash")
                                                            .danger()
                                                            .separator()
                                                            .disabled(is_busy)
                                                            .run(move || { on_delete_task.try_run(r_del.clone()); }),
                                                    ]
                                                }
                                            >
                                                <Icon name="more"/>
                                            </ActionMenu>
                                        })}
                                    </div>
                                </article>

                                {move || {
                                    let is_open = sub_total > 0
                                        && expanded_subtasks.with(|e| e.contains(&row.id));
                                    is_open.then(|| {
                                        let kids = subtasks_sv.get_value();
                                        view! {
                                            <div class="tasks-sub-tree" role="list" aria-label="Subtasks">
                                                {kids.into_iter().map(|kid| {
                                                    let k_done = kid.is_done();
                                                    let k_ro = kid.role == "read";
                                                    let k_id = kid.id.clone();
                                                    let k_id_sel = kid.id.clone();
                                                    let k_title = if kid.data.title.is_empty() {
                                                        "Untitled subtask".to_owned()
                                                    } else {
                                                        kid.data.title.clone()
                                                    };
                                                    let k_check_label = format!("{} {}", if k_done { "Reopen" } else { "Complete" }, k_title);
                                                    let k_comp = kid.clone();
                                                    let k_due = kid.effective_due().to_owned();
                                                    let k_all_day = kid.effective_all_day();
                                                    view! {
                                                        <div
                                                            role="listitem"
                                                            class=move || {
                                                                let sel = selected_id.with(|s| s.as_deref() == Some(k_id_sel.as_str()));
                                                                format!(
                                                                    "tasks-sub-row{}{}",
                                                                    if k_done { " is-done" } else { "" },
                                                                    if sel { " selected" } else { "" }
                                                                )
                                                            }
                                                        >
                                                            <button
                                                                type="button"
                                                                class=format!("task-check is-sub priority-{}", priority_class(kid.data.priority))
                                                                aria-pressed=k_done.to_string()
                                                                aria-label=k_check_label
                                                                disabled=move || busy.get() || k_ro
                                                                on:click=move |_| { on_complete_task.try_run(k_comp.clone()); }
                                                            >
                                                                {k_done.then(|| view! { <Icon name="check"/> })}
                                                            </button>
                                                            <button
                                                                type="button"
                                                                class="tasks-sub-title"
                                                                on:click=move |_| { on_open_task.try_run(k_id.clone()); }
                                                            >
                                                                <span>{k_title}</span>
                                                                {(!k_due.is_empty()).then(|| {
                                                                    let d = k_due.clone();
                                                                    view! { <small>{move || due_label(&d, k_all_day, &today.get())}</small> }
                                                                })}
                                                            </button>
                                                        </div>
                                                    }
                                                }).collect_view()}
                                            </div>
                                        }
                                    })
                                }}
                            </div>
                        }
                    }
                />
            </div>

            {move || {
                let open_empty = display_rows.with(Vec::is_empty);
                let comp_empty = completed_rows.with(Vec::is_empty);
                (!loading.get() && open_empty && comp_empty).then(|| {
                    view! {
                        <div class="workspace-empty">
                            <h2>"No tasks in this view"</h2>
                            <p>"Use the quick-add bar above, press C to create a task, or choose another view."</p>
                        </div>
                    }
                })
            }}

            {move || {
                let comp = completed_rows.get();
                let show_inline = show_completed_inline.get() && view.get() != "completed";
                (show_inline && !comp.is_empty()).then(|| {
                    let count = comp.len();
                    let comp_sv = StoredValue::new(comp);
                    view! {
                        <section class="tasks-completed-drawer" aria-label="Completed tasks">
                            <button
                                type="button"
                                class="tasks-completed-toggle"
                                aria-expanded=move || completed_open.get().to_string()
                                on:click=move |_| completed_open.update(|o| *o = !*o)
                            >
                                <Icon name="chevron"/>
                                <span>{format!("Completed ({count})")}</span>
                            </button>
                            {move || completed_open.get().then(|| view! {
                                <div class="tasks-completed-list">
                                    {comp_sv.get_value().into_iter().map(|row| {
                                        let id_open = row.id.clone();
                                        let id_sel = row.id.clone();
                                        let title = if row.data.title.is_empty() { "Untitled task".to_owned() } else { row.data.title.clone() };
                                        let check_lbl = format!("Reopen {title}");
                                        let ro = row.role == "read";
                                        let row_c = row.clone();
                                        view! {
                                            <article class=move || {
                                                let sel = selected_id.with(|s| s.as_deref() == Some(id_sel.as_str()));
                                                format!("workspace-row tasks-row is-done{}", if sel { " selected" } else { "" })
                                            }>
                                                <button
                                                    type="button"
                                                    class="task-check"
                                                    aria-pressed="true"
                                                    aria-label=check_lbl
                                                    disabled=move || busy.get() || ro
                                                    on:click=move |_| { on_complete_task.try_run(row_c.clone()); }
                                                >
                                                    <Icon name="check"/>
                                                </button>
                                                <button
                                                    type="button"
                                                    class="workspace-row-content"
                                                    on:click=move |_| { on_open_task.try_run(id_open.clone()); }
                                                >
                                                    <strong class="tasks-row-title">{title}</strong>
                                                </button>
                                            </article>
                                        }
                                    }).collect_view()}
                                </div>
                            })}
                        </section>
                    }
                })
            }}
        </div>
    }
}
