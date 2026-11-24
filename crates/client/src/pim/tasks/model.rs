//! Tasks data model, smart-view filtering, subtask tree grouping, sorting, date labels,
//! preference helpers, and Markdown export.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;
use wasm_bindgen::JsValue;

use crate::date_popover::add_days;
use crate::pim::Collection;
use crate::ui::window;
pub use crate::workspace::task_occurrences::{TaskData, TaskOccurrence};

pub const LISTS_BASE: &str = "/api/v1/tasks/lists";
pub const ITEMS_BASE: &str = "/api/v1/tasks/items";

pub const SECTION_ORDER: [&str; 6] = [
    "Overdue",
    "Today",
    "Tomorrow",
    "Upcoming",
    "No date",
    "Completed",
];

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskChild {
    pub id: String,
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub id: String,
    pub collection_id: String,
    #[serde(default)]
    pub uid: String,
    pub revision: u64,
    #[serde(default)]
    pub modified_at: String,
    pub data: TaskData,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub collection_name: Option<String>,
    #[serde(default)]
    pub overdue: Option<bool>,
    #[serde(default)]
    pub next_occurrence: Option<TaskOccurrence>,
    #[serde(skip)]
    pub editing_occurrence_id: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub children: Option<Vec<TaskChild>>,
}

impl TaskItem {
    pub fn is_done(&self) -> bool {
        self.data.status == "COMPLETED"
    }

    pub fn is_cancelled(&self) -> bool {
        self.data.status == "CANCELLED"
    }

    pub fn is_starred(&self) -> bool {
        self.data.starred || (1..=4).contains(&self.data.priority)
    }

    pub fn effective_due(&self) -> &str {
        self.next_occurrence
            .as_ref()
            .map(|o| o.data.due.as_str())
            .unwrap_or(self.data.due.as_str())
    }

    pub fn effective_all_day(&self) -> bool {
        self.next_occurrence
            .as_ref()
            .map(|o| o.data.all_day)
            .unwrap_or(self.data.all_day)
    }

    pub fn is_overdue(&self, today: &str) -> bool {
        if self.is_done() || self.is_cancelled() {
            return false;
        }
        if self.overdue == Some(true) {
            return true;
        }
        let due = self.effective_due();
        let day: String = due.chars().take(10).collect();
        !day.is_empty() && day.as_str() < today
    }

    pub fn children_slice(&self) -> &[TaskChild] {
        self.children.as_deref().unwrap_or(&[])
    }
}

#[derive(Deserialize)]
pub struct CollectionList {
    pub items: Vec<Collection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskListResponse {
    pub items: Vec<TaskItem>,
    #[serde(default)]
    pub next_cursor: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskLayout {
    List,
    Board,
    Kanban,
}

impl TaskLayout {
    pub fn from_str(s: &str) -> Self {
        match s {
            "board" => Self::Board,
            "kanban" => Self::Kanban,
            _ => Self::List,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Board => "board",
            Self::Kanban => "kanban",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskFilterChip {
    All,
    Overdue,
    High,
    Subtasks,
    Recurring,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SmartViewVis {
    pub week: String,
    pub inbox: String,
    pub starred: String,
    pub completed: String,
}

impl Default for SmartViewVis {
    fn default() -> Self {
        Self {
            week: "show".into(),
            inbox: "show".into(),
            starred: "show".into(),
            completed: "show".into(),
        }
    }
}

impl SmartViewVis {
    pub fn from_value(v: Option<&Value>) -> Self {
        let mut out = Self::default();
        if let Some(obj) = v.and_then(Value::as_object) {
            if let Some(s) = obj.get("week").and_then(Value::as_str) {
                out.week = s.to_owned();
            }
            if let Some(s) = obj.get("inbox").and_then(Value::as_str) {
                out.inbox = s.to_owned();
            }
            if let Some(s) = obj.get("starred").and_then(Value::as_str) {
                out.starred = s.to_owned();
            }
            if let Some(s) = obj.get("completed").and_then(Value::as_str) {
                out.completed = s.to_owned();
            }
        }
        out
    }

    pub fn visible(&self, key: &str, count: usize, active: bool) -> bool {
        if active {
            return true;
        }
        let mode = match key {
            "week" => self.week.as_str(),
            "inbox" => self.inbox.as_str(),
            "starred" => self.starred.as_str(),
            "completed" => self.completed.as_str(),
            _ => "show",
        };
        match mode {
            "hide" => false,
            "auto" => count > 0,
            _ => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TasksPrefs {
    pub default_view: String,
    pub default_sort: String,
    pub default_layout: TaskLayout,
    pub default_list: String,
    pub show_completed_inline: bool,
    pub expand_subtasks: bool,
    pub show_pace: bool,
    pub confirm_delete: bool,
    pub smart_views: SmartViewVis,
}

impl Default for TasksPrefs {
    fn default() -> Self {
        Self {
            default_view: "today".into(),
            default_sort: "due".into(),
            default_layout: TaskLayout::List,
            default_list: String::new(),
            show_completed_inline: true,
            expand_subtasks: false,
            show_pace: true,
            confirm_delete: true,
            smart_views: SmartViewVis::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarCounts {
    pub all: usize,
    pub inbox: usize,
    pub today: usize,
    pub overdue: usize,
    pub week: usize,
    pub upcoming: usize,
    pub starred: usize,
    pub completed: usize,
    pub done_today: usize,
    pub per_list: BTreeMap<String, usize>,
    pub tags: Vec<(String, usize)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TaskEditorMode {
    New {
        collection_id: Option<String>,
        data: Option<TaskData>,
    },
    Edit(TaskItem),
}

#[derive(Clone)]
pub enum ConfirmAction {
    DeleteTask {
        id: String,
        revision: u64,
        children: Option<&'static str>,
    },
    CompleteWithSubtasks {
        id: String,
        revision: u64,
    },
    DeleteList {
        id: String,
        revision: u64,
    },
    BulkDelete {
        rows: Vec<(String, u64)>,
    },
}

#[derive(Clone)]
pub struct ConfirmState {
    pub title: String,
    pub text: String,
    pub destructive: bool,
    pub action: ConfirmAction,
    pub alternative: Option<(String, ConfirmAction)>,
}

#[derive(Clone)]
pub struct MoveState {
    pub rows: Vec<TaskItem>,
    pub copy: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UndoSnapshot {
    RestoreTask {
        id: String,
        collection_id: String,
        data: TaskData,
    },
    RestoreMany {
        items: Vec<(String, TaskData)>,
    },
    RecreateTask {
        collection_id: String,
        data: TaskData,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct UndoToast {
    pub message: String,
    pub snapshot: Option<UndoSnapshot>,
}

pub fn normalize_view(v: &str) -> String {
    match v {
        "important" => "starred".into(),
        "all" | "inbox" | "today" | "week" | "upcoming" | "starred" | "completed" => v.to_owned(),
        _ => "today".into(),
    }
}

pub fn view_label(view: &str) -> &'static str {
    match view {
        "all" => "All tasks",
        "inbox" => "Inbox",
        "today" => "Today",
        "week" => "Next 7 days",
        "upcoming" => "Upcoming",
        "starred" | "important" => "Starred",
        "completed" => "Completed",
        _ => "Tasks",
    }
}

pub fn sort_label(sort: &str) -> &'static str {
    match sort {
        "modified" => "Edited",
        "title" => "Title",
        "due" => "Due date",
        "priority" => "Priority",
        "manual" => "My order",
        "start" => "Start date",
        _ => "Sort",
    }
}

pub fn task_status_label(status: &str) -> &'static str {
    match status {
        "NEEDS-ACTION" => "To do",
        "IN-PROCESS" => "In progress",
        "COMPLETED" => "Completed",
        "CANCELLED" => "Cancelled",
        _ => "To do",
    }
}

pub fn task_priority_label(priority: i32) -> &'static str {
    if priority == 0 {
        "None"
    } else if priority < 5 {
        "High"
    } else if priority == 5 {
        "Normal"
    } else {
        "Low"
    }
}

pub fn priority_class(priority: i32) -> &'static str {
    if (1..=4).contains(&priority) {
        "high"
    } else if priority == 5 {
        "medium"
    } else if priority >= 6 {
        "low"
    } else {
        "none"
    }
}

pub fn date_slice(value: &str, all_day: bool) -> String {
    value.chars().take(if all_day { 10 } else { 16 }).collect()
}

pub fn encode_uri(value: &str) -> String {
    js_sys::encode_uri_component(value)
        .as_string()
        .unwrap_or_else(|| value.to_owned())
}

pub fn today_in_zone(zone: &str) -> String {
    let locales = js_sys::Array::of1(&JsValue::from_str("en-CA"));
    let opts = js_sys::Object::new();
    for (k, v) in [
        ("timeZone", zone),
        ("year", "numeric"),
        ("month", "2-digit"),
        ("day", "2-digit"),
    ] {
        let _ = js_sys::Reflect::set(&opts, &JsValue::from_str(k), &JsValue::from_str(v));
    }
    let fmt = js_sys::Intl::DateTimeFormat::new(&locales, &opts);
    let now = js_sys::Date::new_0();
    fmt.format()
        .call1(&JsValue::NULL, &now)
        .ok()
        .and_then(|v| v.as_string())
        .filter(|s| s.len() >= 10)
        .unwrap_or_else(|| {
            now.to_iso_string()
                .as_string()
                .unwrap_or_else(|| "2026-01-01".into())
                .chars()
                .take(10)
                .collect()
        })
}

pub fn section_rank(section: &str) -> usize {
    SECTION_ORDER
        .iter()
        .position(|s| *s == section)
        .unwrap_or(SECTION_ORDER.len())
}

pub fn task_section(row: &TaskItem, today: &str) -> &'static str {
    if row.is_done() || row.is_cancelled() {
        return "Completed";
    }
    if row.is_overdue(today) {
        return "Overdue";
    }
    let due: String = row.effective_due().chars().take(10).collect();
    if due.is_empty() {
        return "No date";
    }
    if due.as_str() < today {
        return "Overdue";
    }
    if due == today {
        return "Today";
    }
    let tomorrow = add_days(today, 1);
    if due == tomorrow {
        "Tomorrow"
    } else {
        "Upcoming"
    }
}

pub fn due_label(value: &str, all_day: bool, today: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let date: String = value.chars().take(10).collect();
    let tomorrow = add_days(today, 1);
    let yesterday = add_days(today, -1);
    let time: String = if !all_day && value.chars().count() >= 16 {
        value.chars().skip(11).take(5).collect()
    } else {
        String::new()
    };
    let day = if date == today {
        "Today".to_owned()
    } else if date == tomorrow {
        "Tomorrow".to_owned()
    } else if date == yesterday {
        "Yesterday".to_owned()
    } else {
        let date_ms = js_sys::Date::parse(&date);
        let today_ms = js_sys::Date::parse(today);
        let near = !date_ms.is_nan()
            && !today_ms.is_nan()
            && (date_ms - today_ms).abs() < 6.0 * 86_400_000.0;
        let noon_ms = js_sys::Date::parse(&format!("{date}T12:00:00Z"));
        if noon_ms.is_nan() {
            date
        } else if near {
            crate::dates::local_date(
                noon_ms,
                &[
                    ("weekday", "short"),
                    ("month", "short"),
                    ("day", "numeric"),
                    ("timeZone", "UTC"),
                ],
            )
        } else {
            crate::dates::local_date(
                noon_ms,
                &[("month", "short"), ("day", "numeric"), ("timeZone", "UTC")],
            )
        }
    };
    if time.is_empty() {
        day
    } else {
        format!("{day}, {time}")
    }
}

pub fn set_url_item(id: Option<&str>) {
    let Ok(url) = web_sys::Url::new(&window().location().href().unwrap_or_default()) else {
        return;
    };
    let sp = url.search_params();
    match id {
        Some(id) => sp.set("item", id),
        None => sp.delete("item"),
    }
    let search = sp.to_string().as_string().unwrap_or_default();
    let next = if search.is_empty() {
        url.pathname()
    } else {
        format!("{}?{search}", url.pathname())
    };
    if let Ok(history) = window().history() {
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&next));
    }
}

/// Computes live counts across all loaded tasks for sidebar badges, tags, and Today's pace.
pub fn compute_counts(all_items: &[TaskItem], today: &str) -> SidebarCounts {
    let week_end = add_days(today, 7);
    let mut c = SidebarCounts::default();
    let mut tag_map: BTreeMap<String, usize> = BTreeMap::new();

    for item in all_items {
        let done = item.is_done();
        let cancelled = item.is_cancelled();
        let due_day: String = item.effective_due().chars().take(10).collect();
        let has_due = !due_day.is_empty();

        if done {
            c.completed += 1;
            if item.data.completed_at.starts_with(today) || due_day == today {
                c.done_today += 1;
            }
            continue;
        }
        if cancelled {
            continue;
        }
        c.all += 1;
        *c.per_list.entry(item.collection_id.clone()).or_insert(0) += 1;

        if item.is_overdue(today) {
            c.overdue += 1;
        }
        if has_due && due_day.as_str() <= today {
            c.today += 1;
        }
        if has_due && due_day.as_str() <= week_end.as_str() {
            c.week += 1;
        }
        if has_due && due_day.as_str() > today {
            c.upcoming += 1;
        }
        if !has_due && item.data.parent_uid.is_empty() {
            c.inbox += 1;
        }
        if item.is_starred() {
            c.starred += 1;
        }
        for tag in &item.data.categories {
            let trimmed = tag.trim();
            if !trimmed.is_empty() {
                *tag_map.entry(trimmed.to_owned()).or_insert(0) += 1;
            }
        }
    }
    let mut tags: Vec<(String, usize)> = tag_map.into_iter().collect();
    tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    c.tags = tags;
    c
}

/// Filters `all_items` by active scope (collection or smart view), optional tag, and filter chip.
pub fn filter_tasks(
    all_items: &[TaskItem],
    view: &str,
    collection: &str,
    tag_filter: Option<&str>,
    chip: TaskFilterChip,
    today: &str,
) -> Vec<TaskItem> {
    let week_end = add_days(today, 7);
    all_items
        .iter()
        .filter(|item| {
            if !collection.is_empty() {
                if item.collection_id != collection {
                    return false;
                }
            } else {
                let done = item.is_done();
                let cancelled = item.is_cancelled();
                let due_day: String = item.effective_due().chars().take(10).collect();
                let has_due = !due_day.is_empty();
                let keep = match view {
                    "completed" => done,
                    "all" => true,
                    other => {
                        !done
                            && !cancelled
                            && match other {
                                "today" => has_due && due_day.as_str() <= today,
                                "week" => has_due && due_day.as_str() <= week_end.as_str(),
                                "upcoming" => has_due && due_day.as_str() > today,
                                "starred" | "important" => item.is_starred(),
                                "inbox" => !has_due && item.data.parent_uid.is_empty(),
                                _ => true,
                            }
                    }
                };
                if !keep {
                    return false;
                }
            }
            if let Some(tg) = tag_filter
                && !item.data.categories.iter().any(|c| c == tg)
            {
                return false;
            }
            match chip {
                TaskFilterChip::All => true,
                TaskFilterChip::Overdue => item.is_overdue(today),
                TaskFilterChip::High => (1..=4).contains(&item.data.priority),
                TaskFilterChip::Subtasks => !item.children_slice().is_empty(),
                TaskFilterChip::Recurring => {
                    !item.data.recurrence.is_empty() || item.data.recurring
                }
            }
        })
        .cloned()
        .collect()
}

/// Sorts tasks in-place according to the active sort key.
pub fn sort_tasks(list: &mut [TaskItem], sort: &str, today: &str) {
    match sort {
        "title" => list.sort_by(|a, b| {
            a.data
                .title
                .to_lowercase()
                .cmp(&b.data.title.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        }),
        "priority" => list.sort_by(|a, b| {
            let star_a = if a.is_starred() { 0 } else { 1 };
            let star_b = if b.is_starred() { 0 } else { 1 };
            let pri_a = if (1..=9).contains(&a.data.priority) {
                a.data.priority
            } else {
                99
            };
            let pri_b = if (1..=9).contains(&b.data.priority) {
                b.data.priority
            } else {
                99
            };
            star_a
                .cmp(&star_b)
                .then_with(|| pri_a.cmp(&pri_b))
                .then_with(|| {
                    section_rank(task_section(a, today)).cmp(&section_rank(task_section(b, today)))
                })
                .then_with(|| a.effective_due().cmp(b.effective_due()))
        }),
        "start" => list.sort_by(|a, b| {
            let sa = if a.data.start.is_empty() {
                "9999"
            } else {
                a.data.start.as_str()
            };
            let sb = if b.data.start.is_empty() {
                "9999"
            } else {
                b.data.start.as_str()
            };
            sa.cmp(sb).then_with(|| b.modified_at.cmp(&a.modified_at))
        }),
        "manual" => list.sort_by(|a, b| {
            a.data
                .sort_order
                .cmp(&b.data.sort_order)
                .then_with(|| b.modified_at.cmp(&a.modified_at))
        }),
        "modified" => list.sort_by(|a, b| b.modified_at.cmp(&a.modified_at)),
        _ => list.sort_by(|a, b| {
            section_rank(task_section(a, today))
                .cmp(&section_rank(task_section(b, today)))
                .then_with(|| {
                    let da = a.effective_due();
                    let db = b.effective_due();
                    let ka = if da.is_empty() { "9999" } else { da };
                    let kb = if db.is_empty() { "9999" } else { db };
                    ka.cmp(kb)
                })
                .then_with(|| b.modified_at.cmp(&a.modified_at))
        }),
    }
}

/// A display row for the List view, pairing a task with its optional section header,
/// nested subtasks (when the parent is visible in this view), and parent title (when orphaned).
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayTaskRow {
    pub item: TaskItem,
    pub section: Option<&'static str>,
    pub section_count: usize,
    pub subtasks: Vec<TaskItem>,
    pub parent_title: Option<String>,
}

/// Builds top-level rows with nested subtasks attached underneath their parent when both live
/// in the same view; subtasks whose parent is NOT in the filtered view stay visible with a
/// subtle parent-title breadcrumb so nothing is ever lost.
pub fn build_display_rows(
    filtered: Vec<TaskItem>,
    all_items: &[TaskItem],
    sort: &str,
    today: &str,
    separate_completed: bool,
) -> (Vec<DisplayTaskRow>, Vec<TaskItem>) {
    let mut open: Vec<TaskItem> = Vec::new();
    let mut completed: Vec<TaskItem> = Vec::new();

    for item in filtered {
        if separate_completed && (item.is_done() || item.is_cancelled()) {
            completed.push(item);
        } else {
            open.push(item);
        }
    }

    // Index all tasks by (collection_id, uid) so subtasks can be nested under visible parents.
    let mut all_by_uid: HashMap<(&str, &str), &TaskItem> = HashMap::new();
    let mut kids_by_parent: HashMap<(&str, &str), Vec<TaskItem>> = HashMap::new();
    for item in all_items {
        if !item.uid.is_empty() {
            all_by_uid.insert((item.collection_id.as_str(), item.uid.as_str()), item);
        }
        if !item.data.parent_uid.is_empty() {
            kids_by_parent
                .entry((item.collection_id.as_str(), item.data.parent_uid.as_str()))
                .or_default()
                .push(item.clone());
        }
    }

    let visible_open_uids: HashSet<(String, String)> = open
        .iter()
        .filter(|t| !t.uid.is_empty())
        .map(|t| (t.collection_id.clone(), t.uid.clone()))
        .collect();

    // Hide subtasks from top-level open list ONLY when their parent is already visible in `open`.
    let mut top_open: Vec<TaskItem> = open
        .into_iter()
        .filter(|t| {
            t.data.parent_uid.is_empty()
                || !visible_open_uids
                    .contains(&(t.collection_id.clone(), t.data.parent_uid.clone()))
        })
        .collect();

    sort_tasks(&mut top_open, sort, today);
    sort_tasks(&mut completed, "modified", today);

    // Count tasks per section when sorted by due date.
    let mut section_counts: HashMap<&'static str, usize> = HashMap::new();
    if sort == "due" {
        for row in &top_open {
            *section_counts.entry(task_section(row, today)).or_insert(0) += 1;
        }
    }

    let mut prev_section: Option<&'static str> = None;
    let display: Vec<DisplayTaskRow> = top_open
        .into_iter()
        .map(|item| {
            let section = if sort == "due" {
                let sec = task_section(&item, today);
                let show = prev_section != Some(sec);
                prev_section = Some(sec);
                show.then_some(sec)
            } else {
                None
            };
            let sec_cnt = section
                .and_then(|s| section_counts.get(s).copied())
                .unwrap_or(0);
            let mut subtasks = kids_by_parent
                .get(&(item.collection_id.as_str(), item.uid.as_str()))
                .cloned()
                .unwrap_or_default();
            subtasks.sort_by(|a, b| {
                a.is_done()
                    .cmp(&b.is_done())
                    .then_with(|| a.data.sort_order.cmp(&b.data.sort_order))
                    .then_with(|| a.data.title.cmp(&b.data.title))
            });
            let parent_title = if !item.data.parent_uid.is_empty() {
                all_by_uid
                    .get(&(item.collection_id.as_str(), item.data.parent_uid.as_str()))
                    .map(|p| p.data.title.clone())
                    .filter(|t| !t.is_empty())
            } else {
                None
            };
            DisplayTaskRow {
                item,
                section,
                section_count: sec_cnt,
                subtasks,
                parent_title,
            }
        })
        .collect();

    // Filter out completed subtasks whose parent is also in `completed`? Keep completed flat so
    // every completed item is easy to reopen.
    (display, completed)
}

/// Generates a clean Markdown checklist export for a task list (compatible with Dar Notes).
pub fn export_markdown(list_name: &str, items: &[TaskItem]) -> String {
    let mut out = format!("# {list_name}\n\n");
    let mut kids: BTreeMap<String, Vec<&TaskItem>> = BTreeMap::new();
    let mut roots: Vec<&TaskItem> = Vec::new();
    let uids: HashSet<&str> = items.iter().map(|i| i.uid.as_str()).collect();
    for item in items {
        if !item.data.parent_uid.is_empty() && uids.contains(item.data.parent_uid.as_str()) {
            kids.entry(item.data.parent_uid.clone())
                .or_default()
                .push(item);
        } else {
            roots.push(item);
        }
    }
    let write_line = |buf: &mut String, indent: &str, t: &TaskItem| {
        let box_ch = if t.is_done() { "x" } else { " " };
        let title = if t.data.title.is_empty() {
            "Untitled task"
        } else {
            &t.data.title
        };
        buf.push_str(&format!("{indent}- [{box_ch}] {title}"));
        if !t.data.due.is_empty() {
            buf.push_str(&format!(
                " (due {})",
                &t.data.due[..t.data.due.len().min(10)]
            ));
        }
        for tag in &t.data.categories {
            buf.push_str(&format!(" #{tag}"));
        }
        buf.push('\n');
    };
    for root in roots {
        write_line(&mut out, "", root);
        if let Some(sub) = kids.get(&root.uid) {
            for s in sub {
                write_line(&mut out, "  ", s);
            }
        }
    }
    out
}

pub fn visible_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        Some(trimmed.to_owned())
    } else {
        None
    }
}

pub fn collect_unique_tags(items: &[TaskItem]) -> Vec<String> {
    let mut set = BTreeSet::new();
    for item in items {
        for tag in &item.data.categories {
            let t = tag.trim();
            if !t.is_empty() {
                set.insert(t.to_owned());
            }
        }
    }
    set.into_iter().collect()
}
