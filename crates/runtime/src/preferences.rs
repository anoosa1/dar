//! Account preferences (`runtime/pim/http.ts`, `/api/v1/pim/preferences`): a PUT merges the
//! supplied keys into the saved ones and checks every value, so stored preferences are
//! always complete and valid.

use serde_json::{Map, Value, json};

use crate::error::{AppError, Result};

fn invalid(message: &str) -> AppError {
    AppError::with("invalidInput", 400, message)
}

fn is_uuid(value: &Value) -> bool {
    value.as_str().is_some_and(crate::auth::is_uuid)
}

/// Folder names separated by `/`, each NFC-trimmed (`photoFolderParts`).
pub fn folder_parts(value: &str) -> Option<Vec<String>> {
    let parts: Vec<String> = value
        .split('/')
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect();
    let bad = |p: &String| {
        p.encode_utf16().count() > 255
            || p == "."
            || p == ".."
            || p.chars().any(|c| (c as u32) < 0x20 || c == '\\')
    };
    if parts.is_empty()
        || parts.len() > 8
        || value.encode_utf16().count() > 500
        || parts.iter().any(bad)
    {
        return None;
    }
    Some(parts)
}

/// The merged, checked preferences for a PUT body over the saved `previous`.
pub fn merge(
    previous: &Map<String, Value>,
    body: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let get = |key: &str, fallback: Value| -> Value {
        body.get(key)
            .cloned()
            .unwrap_or_else(|| previous.get(key).cloned().unwrap_or(fallback))
    };
    let choice = |key: &str, allowed: &[&str], fallback: &str| -> Result<Value> {
        let value = get(key, json!(fallback));
        match value.as_str() {
            Some(v) if allowed.contains(&v) => Ok(value),
            _ => Err(invalid(&format!("Invalid {key}."))),
        }
    };
    let flag = |key: &str, fallback: bool| -> Result<Value> {
        let value = get(key, json!(fallback));
        if value.is_boolean() {
            Ok(value)
        } else {
            Err(invalid(&format!("Invalid {key}.")))
        }
    };
    let number = |key: &str, allowed: &[i64], fallback: i64| -> Result<Value> {
        let value = get(key, json!(fallback));
        match whole(&value) {
            Some(v) if allowed.contains(&v) && value.is_number() => Ok(value),
            _ => Err(invalid(&format!("Invalid {key}."))),
        }
    };
    let zone = get("timezone", json!("UTC"));
    let zone = zone
        .as_str()
        .filter(|z| z.len() <= 100 && z.parse::<chrono_tz::Tz>().is_ok())
        .ok_or_else(|| invalid("Choose a valid IANA timezone."))?
        .to_owned();
    let week_start = get("weekStart", json!(1));
    if week_start != json!(0) && week_start != json!(1) {
        return Err(invalid("Week starts on Sunday or Monday."));
    }
    if let Some(hidden) = body.get("hiddenCalendars") {
        let ok = hidden
            .as_array()
            .is_some_and(|list| list.len() <= 200 && list.iter().all(Value::is_string));
        if !ok {
            return Err(invalid("Invalid hidden calendar list."));
        }
    }
    let settled = get("agentSettledThreads", json!([]));
    let settled_ok = settled.as_array().is_some_and(|list| {
        list.len() <= 128
            && list
                .iter()
                .all(|row| is_uuid(&row["id"]) && whole(&row["revision"]).is_some_and(|r| r >= 1))
    });
    if !settled_ok {
        return Err(invalid("Invalid settled thread list."));
    }
    let thread_ids = |key: &str| -> Result<Value> {
        let value = get(key, json!([]));
        let list = value
            .as_array()
            .filter(|l| l.len() <= 128 && l.iter().all(is_uuid))
            .ok_or_else(|| invalid(&format!("Invalid {key}.")))?;
        let mut unique: Vec<Value> = Vec::new();
        for item in list {
            if !unique.contains(item) {
                unique.push(item.clone());
            }
        }
        Ok(Value::Array(unique))
    };
    let (pinned, archived) = (
        thread_ids("agentPinnedThreads")?,
        thread_ids("agentArchivedThreads")?,
    );
    let snoozed = get("agentSnoozed", json!([]));
    let snoozed_ok = snoozed.as_array().is_some_and(|list| {
        list.len() <= 128
            && list.iter().all(|row| {
                row.as_object()
                    .is_some_and(|o| o.keys().all(|k| k == "id" || k == "until"))
                    && is_uuid(&row["id"])
                    && whole(&row["until"]).is_some_and(|u| u >= 0)
            })
    });
    if !snoozed_ok {
        return Err(invalid("Invalid snoozed thread list."));
    }
    let folder = |key: &str, fallback: &str, message: &str| -> Result<Value> {
        let value = get(key, json!(fallback));
        let parts = value
            .as_str()
            .and_then(folder_parts)
            .ok_or_else(|| invalid(message))?;
        Ok(json!(parts.join("/")))
    };
    let mut out = Map::new();
    let mut put = |key: &str, value: Value| {
        out.insert(key.to_owned(), value);
    };
    put(
        "calendarShowCancelled",
        flag("calendarShowCancelled", false)?,
    );
    put("calendarShowTasks", flag("calendarShowTasks", true)?);
    put(
        "calendarTimeFormat",
        choice("calendarTimeFormat", &["24h", "12h"], "24h")?,
    );
    put(
        "appearance",
        choice("appearance", &["system", "latte", "mocha"], "system")?,
    );
    put("timezone", json!(zone));
    put("weekStart", week_start);
    put(
        "contactSort",
        choice(
            "contactSort",
            &["firstName", "lastName", "organization"],
            "firstName",
        )?,
    );
    put(
        "contactNameOrder",
        choice("contactNameOrder", &["firstLast", "lastFirst"], "firstLast")?,
    );
    put(
        "contactView",
        choice("contactView", &["list", "table", "cards"], "list")?,
    );
    put("contactShowBirthdays", flag("contactShowBirthdays", true)?);
    let default_book = get("contactDefaultBook", json!(""));
    let default_book_ok = default_book
        .as_str()
        .is_some_and(|v| v.is_empty() || crate::auth::is_uuid(v));
    if !default_book_ok {
        return Err(invalid("Invalid contactDefaultBook."));
    }
    put("contactDefaultBook", default_book);
    put("hiddenCalendars", get("hiddenCalendars", json!([])));
    put(
        "notesSort",
        choice("notesSort", &["modified", "title"], "modified")?,
    );
    put(
        "tasksSort",
        choice(
            "tasksSort",
            &["modified", "title", "due", "priority", "manual", "start"],
            "due",
        )?,
    );
    put("filesView", choice("filesView", &["list", "grid"], "list")?);
    put("filesShowHidden", flag("filesShowHidden", false)?);
    put("filesConfirmTrash", flag("filesConfirmTrash", false)?);
    put(
        "filesDetailsTab",
        choice(
            "filesDetailsTab",
            &["details", "versions", "sharing"],
            "details",
        )?,
    );
    put(
        "notesEditorMode",
        choice("notesEditorMode", &["write", "preview"], "write")?,
    );
    put(
        "tasksView",
        choice(
            "tasksView",
            &[
                "all",
                "inbox",
                "today",
                "week",
                "upcoming",
                "important",
                "starred",
                "completed",
            ],
            "today",
        )?,
    );
    put(
        "tasksLayout",
        choice("tasksLayout", &["list", "board", "kanban"], "list")?,
    );
    let default_task_list = get("tasksDefaultList", json!(""));
    let default_task_list_ok = default_task_list
        .as_str()
        .is_some_and(|v| v.is_empty() || crate::auth::is_uuid(v));
    if !default_task_list_ok {
        return Err(invalid("Invalid tasksDefaultList."));
    }
    put("tasksDefaultList", default_task_list);
    put("tasksShowCompleted", flag("tasksShowCompleted", true)?);
    put("tasksExpandSubtasks", flag("tasksExpandSubtasks", false)?);
    put("tasksShowPace", flag("tasksShowPace", true)?);
    put("tasksConfirmDelete", flag("tasksConfirmDelete", true)?);
    let smart_views_default = json!({
        "week": "show",
        "inbox": "show",
        "starred": "show",
        "completed": "show",
    });
    let smart_views_in = get("tasksSmartViews", smart_views_default.clone());
    let smart_obj = smart_views_in
        .as_object()
        .ok_or_else(|| invalid("Invalid tasksSmartViews."))?;
    let mut smart_out = Map::new();
    for key in ["week", "inbox", "starred", "completed"] {
        let val = smart_obj.get(key).and_then(Value::as_str).unwrap_or("show");
        if !["show", "auto", "hide"].contains(&val) {
            return Err(invalid("Invalid tasksSmartViews."));
        }
        smart_out.insert(key.to_owned(), json!(val));
    }
    put("tasksSmartViews", Value::Object(smart_out));
    put(
        "newsMarkRead",
        choice("newsMarkRead", &["open", "scroll"], "open")?,
    );
    put(
        "newsOrder",
        choice("newsOrder", &["newest", "oldest"], "newest")?,
    );
    put("newsShowRead", flag("newsShowRead", false)?);
    put(
        "newsImages",
        choice("newsImages", &["proxy", "block"], "proxy")?,
    );
    put(
        "newsRefresh",
        number("newsRefresh", &[15, 30, 60, 180, 720, 1440], 30)?,
    );
    put(
        "newsKeepDays",
        number("newsKeepDays", &[7, 30, 90, 365], 30)?,
    );
    put(
        "mailImages",
        choice("mailImages", &["block", "load"], "block")?,
    );
    put(
        "mailMarkRead",
        choice("mailMarkRead", &["open", "manual"], "open")?,
    );
    put(
        "mailFolder",
        folder(
            "mailFolder",
            "docs/mail",
            "Choose a Files folder like docs/mail, without control characters.",
        )?,
    );
    put(
        "booksFolder",
        folder(
            "booksFolder",
            "docs/books",
            "Choose a Files folder like docs/books, without control characters.",
        )?,
    );
    put(
        "booksTheme",
        choice("booksTheme", &["dar", "paper", "sepia", "night"], "dar")?,
    );
    put(
        "booksFont",
        choice("booksFont", &["serif", "sans", "book"], "serif")?,
    );
    put(
        "booksFontSize",
        number(
            "booksFontSize",
            &[14, 15, 16, 17, 18, 19, 20, 21, 22, 24, 26, 28],
            18,
        )?,
    );
    put(
        "booksLineHeight",
        choice("booksLineHeight", &["tight", "normal", "loose"], "normal")?,
    );
    put(
        "booksLayout",
        choice("booksLayout", &["pages", "scroll"], "pages")?,
    );
    put(
        "booksComicDirection",
        choice("booksComicDirection", &["auto", "ltr", "rtl"], "auto")?,
    );
    put(
        "booksSort",
        choice(
            "booksSort",
            &["recent", "title", "author", "added"],
            "recent",
        )?,
    );
    put(
        "photosUploadFolder",
        folder(
            "photosUploadFolder",
            "pics",
            "Choose a Files folder like pics or Media/Camera, without control characters.",
        )?,
    );
    put(
        "notesFolder",
        folder(
            "notesFolder",
            "docs/notes",
            "Choose a Files folder like docs/notes, without control characters.",
        )?,
    );
    put("notesImported", flag("notesImported", false)?);
    put("agentSettledThreads", settled);
    put("agentPinnedThreads", pinned);
    put("agentArchivedThreads", archived);
    put(
        "agentSendKey",
        choice("agentSendKey", &["enter", "modEnter"], "enter")?,
    );
    put("agentNotify", flag("agentNotify", false)?);
    put(
        "agentGrouping",
        choice("agentGrouping", &["recent", "project"], "recent")?,
    );
    put("agentSnoozed", snoozed);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_over_saved_values_and_checks_them() {
        let previous: Map<String, Value> =
            serde_json::from_value(json!({ "filesView": "grid", "timezone": "Asia/Riyadh" }))
                .unwrap();
        let body: Map<String, Value> =
            serde_json::from_value(json!({ "filesConfirmTrash": true })).unwrap();
        let merged = merge(&previous, &body).unwrap();
        assert_eq!(merged["filesView"], "grid");
        assert_eq!(merged["filesConfirmTrash"], true);
        assert_eq!(merged["timezone"], "Asia/Riyadh");
        assert_eq!(merged["notesFolder"], "docs/notes");
        let bad: Map<String, Value> =
            serde_json::from_value(json!({ "filesView": "table" })).unwrap();
        assert_eq!(
            merge(&previous, &bad).unwrap_err().message,
            "Invalid filesView."
        );
        let zone: Map<String, Value> =
            serde_json::from_value(json!({ "timezone": "Mars/Base" })).unwrap();
        assert!(merge(&previous, &zone).is_err());
        assert_eq!(folder_parts(" a / b /"), Some(vec!["a".into(), "b".into()]));
        assert_eq!(folder_parts("../x"), None);
    }

    #[test]
    fn validates_tasks_redesign_preferences() {
        let empty = Map::new();
        let body: Map<String, Value> = serde_json::from_value(json!({
            "tasksView": "week",
            "tasksSort": "priority",
            "tasksLayout": "board",
            "tasksShowCompleted": false,
            "tasksExpandSubtasks": true,
            "tasksShowPace": false,
            "tasksConfirmDelete": false,
            "tasksSmartViews": { "week": "auto", "inbox": "hide", "starred": "show", "completed": "auto" }
        }))
        .unwrap();
        let merged = merge(&empty, &body).unwrap();
        assert_eq!(merged["tasksView"], "week");
        assert_eq!(merged["tasksSort"], "priority");
        assert_eq!(merged["tasksLayout"], "board");
        assert_eq!(merged["tasksShowCompleted"], false);
        assert_eq!(merged["tasksExpandSubtasks"], true);
        assert_eq!(merged["tasksShowPace"], false);
        assert_eq!(merged["tasksConfirmDelete"], false);
        assert_eq!(merged["tasksSmartViews"]["inbox"], "hide");

        let bad_layout: Map<String, Value> =
            serde_json::from_value(json!({ "tasksLayout": "timeline" })).unwrap();
        assert!(merge(&empty, &bad_layout).is_err());
        let bad_vis: Map<String, Value> =
            serde_json::from_value(json!({ "tasksSmartViews": { "inbox": "maybe" } })).unwrap();
        assert!(merge(&empty, &bad_vis).is_err());
        let bad_default_list: Map<String, Value> =
            serde_json::from_value(json!({ "tasksDefaultList": "not-a-uuid" })).unwrap();
        assert!(merge(&empty, &bad_default_list).is_err());
    }
}

/// A whole number as JavaScript sees one: `5` and `5.0` are the same value.
fn whole(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|f| f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15)
            .map(|f| f as i64)
    })
}
