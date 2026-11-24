//! Tasks sidebar: + New task, smart views with live counts & overdue badge (respecting
//! visibility settings), Task lists (or visibility checkboxes in List Board mode) with live
//! counts and three-dot menus, clickable Tags cloud, compact Today's pace progress card, and
//! Reminders footer.

use std::collections::HashSet;

use leptos::prelude::*;

use super::model::{SidebarCounts, TaskLayout, TasksPrefs};
use crate::icons::Icon;
use crate::pim::{Collection, CollectionDot};
use crate::ui::{ActionMenu, MenuAction};

#[component]
pub fn TasksSidebar(
    collections: Signal<Vec<Collection>>,
    counts: Memo<SidebarCounts>,
    prefs: Signal<TasksPrefs>,
    layout: RwSignal<TaskLayout>,
    view: RwSignal<String>,
    collection: RwSignal<String>,
    active_tag: RwSignal<Option<String>>,
    hidden_board_lists: RwSignal<HashSet<String>>,
    zone: RwSignal<String>,
    reminder_count: RwSignal<usize>,
    busy: RwSignal<bool>,
    on_new_task: Callback<()>,
    on_new_list: Callback<()>,
    on_select_view: Callback<String>,
    on_select_list: Callback<Collection>,
    on_toggle_board_list: Callback<String>,
    on_toggle_tag: Callback<String>,
    on_edit_list: Callback<Collection>,
    on_share_list: Callback<Collection>,
    on_delete_list: Callback<Collection>,
    on_open_reminders: Callback<()>,
) -> impl IntoView {
    let smart_views: [(&'static str, &'static str, &'static str); 6] = [
        ("today", "calendar", "Today"),
        ("week", "clock", "Next 7 days"),
        ("inbox", "folder", "Inbox"),
        ("starred", "star", "Starred"),
        ("all", "list", "All tasks"),
        ("completed", "check", "Completed"),
    ];

    view! {
        <button
            type="button"
            class="new-button ui-new"
            disabled=move || busy.get()
            on:click=move |_| { on_new_task.try_run(()); }
        >
            <Icon name="plus"/>"New task"
        </button>

        <div class="tasks-sidebar-scroll">
            <nav class="tasks-nav-views" aria-label="tasks views">
                {smart_views.into_iter().map(|(val, icon, label)| {
                    view! {
                        {move || {
                            let c = counts.get();
                            let p = prefs.get();
                            let active = layout.get() != TaskLayout::Board
                                && view.get() == val
                                && collection.with(String::is_empty)
                                && active_tag.with(Option::is_none);
                            let count = match val {
                                "today" => c.today,
                                "week" => c.week,
                                "inbox" => c.inbox,
                                "starred" => c.starred,
                                "all" => c.all,
                                "completed" => c.completed,
                                _ => 0,
                            };
                            let show = p.smart_views.visible(val, count, active);
                            show.then(|| {
                                let overdue_badge = val == "today" && c.overdue > 0;
                                view! {
                                    <button
                                        type="button"
                                        class=if active { "nav-item active" } else { "nav-item" }
                                        aria-label=label
                                        aria-current=active.then_some("page")
                                        on:click=move |_| { on_select_view.try_run(val.to_owned()); }
                                    >
                                        <Icon name=icon/>
                                        <span class="tasks-nav-label">{label}</span>
                                        {if overdue_badge {
                                            view! {
                                                <span class="tasks-nav-overdue" title=format!("{} overdue", c.overdue) aria-hidden="true">
                                                    {c.overdue}
                                                </span>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <span class="tasks-nav-count" aria-hidden="true">{count}</span>
                                            }.into_any()
                                        }}
                                    </button>
                                }
                            })
                        }}
                    }
                }).collect_view()}
            </nav>

            <div class="pim-sidebar-heading">
                <strong>"Task lists"</strong>
                <button
                    type="button"
                    class="icon-button"
                    aria-label="New task list"
                    title="New task list"
                    on:click=move |_| { on_new_list.try_run(()); }
                >
                    <Icon name="plus"/>
                </button>
            </div>

            <div class="tasks-lists-nav">
                {move || {
                    let cols = collections.get();
                    let c = counts.get();
                    let is_board = layout.get() == TaskLayout::Board;
                    let selected_col = collection.get();
                    let hidden = hidden_board_lists.get();

                    if cols.is_empty() {
                        return view! { <p class="tasks-side-empty">"No task lists yet."</p> }.into_any();
                    }

                    cols.into_iter().map(|col| {
                        let id = col.id.clone();
                        let id_toggle = col.id.clone();
                        let name = col.name.clone();
                        let aria_name = col.name.clone();
                        let color = col.color.clone();
                        let count = c.per_list.get(&id).copied().unwrap_or(0);
                        let active = !is_board && selected_col == id;
                        let board_checked = !hidden.contains(&id);
                        let is_owner = col.role == "owner";
                        let is_shared = col.role != "owner";
                        let c_sel = col.clone();
                        let c_edit = col.clone();
                        let c_share = col.clone();
                        let c_del = col.clone();
                        let action_label = format!("Actions for task list {name}");

                        view! {
                            <div class="tasks-list-row">
                                {if is_board {
                                    view! {
                                        <button
                                            type="button"
                                            class=if board_checked { "nav-item tasks-board-toggle is-visible" } else { "nav-item tasks-board-toggle" }
                                            aria-label=aria_name
                                            aria-pressed=board_checked.to_string()
                                            on:click=move |_| { on_toggle_board_list.try_run(id_toggle.clone()); }
                                        >
                                            <span class=if board_checked { "tasks-board-check is-checked" } else { "tasks-board-check" } aria-hidden="true">
                                                {board_checked.then(|| view! { <Icon name="check"/> })}
                                            </span>
                                            <CollectionDot color=color.clone()/>
                                            <span class="tasks-nav-label">{name.clone()}</span>
                                            <span class="tasks-nav-count" aria-hidden="true">{count}</span>
                                        </button>
                                    }.into_any()
                                } else {
                                    view! {
                                        <button
                                            type="button"
                                            class=if active { "nav-item active" } else { "nav-item" }
                                            aria-label=aria_name
                                            aria-current=active.then_some("page")
                                            on:click=move |_| { on_select_list.try_run(c_sel.clone()); }
                                        >
                                            <CollectionDot color=color.clone()/>
                                            <span class="tasks-nav-label">{name.clone()}</span>
                                            {is_shared.then(|| view! { <small class="tasks-shared-tag" aria-hidden="true">"Shared"</small> })}
                                            <span class="tasks-nav-count" aria-hidden="true">{count}</span>
                                        </button>
                                    }.into_any()
                                }}
                                {is_owner.then(|| view! {
                                    <ActionMenu
                                        label=action_label
                                        class="tasks-list-actions"
                                        actions=move |_| {
                                            let (ce, cs, cd) = (c_edit.clone(), c_share.clone(), c_del.clone());
                                            vec![
                                                MenuAction::new("Edit task list", "pencil").run(move || { on_edit_list.try_run(ce.clone()); }),
                                                MenuAction::new("Share task list", "share").run(move || { on_share_list.try_run(cs.clone()); }),
                                                MenuAction::new("Delete task list", "trash").danger().separator().run(move || { on_delete_list.try_run(cd.clone()); }),
                                            ]
                                        }
                                    >
                                        <Icon name="more"/>
                                    </ActionMenu>
                                })}
                            </div>
                        }
                    }).collect_view().into_any()
                }}
            </div>

            {move || {
                let tags = counts.with(|c| c.tags.clone());
                let cur_tag = active_tag.get();
                (!tags.is_empty()).then(|| view! {
                    <div class="pim-sidebar-heading tasks-tags-heading">
                        <strong>"Tags"</strong>
                    </div>
                    <div class="tasks-sidebar-tags" role="group" aria-label="Filter by tag">
                        {tags.into_iter().take(12).map(|(tag, _cnt)| {
                            let is_active = cur_tag.as_deref() == Some(tag.as_str());
                            let t_click = tag.clone();
                            view! {
                                <button
                                    type="button"
                                    class=if is_active { "tasks-side-tag is-active" } else { "tasks-side-tag" }
                                    aria-pressed=is_active.to_string()
                                    on:click=move |_| { on_toggle_tag.try_run(t_click.clone()); }
                                >
                                    {format!("#{tag}")}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                })
            }}
        </div>

        <div class="tasks-sidebar-footer">
            {move || {
                let p = prefs.get();
                let c = counts.get();
                let total_today = c.today + c.done_today;
                p.show_pace.then(|| view! {
                    <div class="tasks-pace-card" aria-label="Today's pace">
                        <div class="tasks-pace-head">
                            <span>"Today\u{2019}s pace"</span>
                            <span class="tasks-pace-num">{format!("{}/{}", c.done_today, total_today)}</span>
                        </div>
                        <progress class="tasks-pace-bar" max=total_today.max(1).to_string() value=c.done_today.to_string() aria-label="Today's completion progress"></progress>
                    </div>
                })
            }}
            <div class="sidebar-footer-row">
                <button
                    type="button"
                    class="nav-item"
                    on:click=move |_| { on_open_reminders.try_run(()); }
                >
                    <Icon name="bell"/>
                    <span>"Reminders"</span>
                    {move || {
                        let n = reminder_count.get();
                        if n > 0 {
                            view! { <span class="tasks-rem-badge">{n}</span> }.into_any()
                        } else {
                            view! { <small>{zone.get()}</small> }.into_any()
                        }
                    }}
                </button>
            </div>
        </div>
    }
}
