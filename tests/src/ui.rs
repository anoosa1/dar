//! Browser steps the suites share (`ui-helpers.mjs`), and signing in through the page.

use crate::Result;
use crate::browser::{Locator, Page, State};

/// Operates a themed select (a combobox with a listbox) by an option's label or value.
pub enum Choice<'a> {
    Label(&'a str),
    Value(&'a str),
}

/// Opens the themed choice and picks an option, rather than a hidden form field.
pub async fn choose(control: &Locator, choice: Choice<'_>) -> Result<()> {
    let id = control
        .get_attribute("aria-controls")
        .await?
        .unwrap_or_default();
    control.click().await?;
    let list = control
        .page()
        .locator(&format!("[id={}]", serde_json::to_string(&id)?));
    match choice {
        Choice::Label(label) => list.get_by_role("option", label).click().await,
        Choice::Value(value) => {
            list.locator(&format!(
                "[role=option][data-value={}]",
                serde_json::to_string(value)?
            ))
            .click()
            .await
        }
    }
}

/// Switches app from the app rail (desktop) or the More menu (phones), and waits for it.
pub async fn switch_app(page: &Page, id: &str) -> Result<()> {
    let name = match id {
        "files" => "Files",
        "contacts" => "Contacts",
        "calendar" => "Calendar",
        "notes" => "Notes",
        "tasks" => "Tasks",
        other => other,
    };
    let link = page
        .get_by_role("navigation", "Applications")
        .get_by_role("link", name);
    if link.is_visible().await? {
        link.click().await?;
    } else {
        page.get_by_role("button", "More apps").click().await?;
        page.get_by_role("menu", "More apps")
            .get_by_role("menuitem", name)
            .click()
            .await?;
    }
    let path = if id == "files" {
        "/".to_owned()
    } else {
        format!("/{id}")
    };
    page.wait_for_url(|url| url.path() == path).await?;
    // Optional apps briefly render a loading header before mounting the workspace.
    let main = match id {
        "files" => ".files-main",
        "contacts" => ".contacts-main",
        "calendar" => ".calendar-main",
        "notes" => ".notes-list",
        "tasks" => ".workspace-list",
        _ => "main",
    };
    page.locator(main).wait_for().await
}

pub async fn file_details(page: &Page, name: &str) -> Result<()> {
    item_action(page, name, "Details").await
}

pub async fn item_action(page: &Page, name: &str, action: &str) -> Result<()> {
    page.get_by_role("button", format!("Actions for {name}"))
        .click()
        .await?;
    page.get_by_role("menuitem", action).click().await
}

/// Picks a date (`YYYY-MM-DD`) in the calendar's date popover.
pub async fn calendar_date(page: &Page, date: &str) -> Result<()> {
    page.get_by_role("button", "Choose date").click().await?;
    let picker = page.get_by_role("dialog", "Choose calendar date");
    let year: u32 = date[..4].parse().unwrap();
    let month: u32 = date[5..7].parse().unwrap();
    choose(
        &picker.get_by_role("combobox", "Choose year"),
        Choice::Value(&year.to_string()),
    )
    .await?;
    choose(
        &picker.get_by_role("combobox", "Choose month"),
        Choice::Value(&(month - 1).to_string()),
    )
    .await?;
    picker
        .locator(&format!("[role=gridcell][data-date=\"{date}\"]"))
        .click()
        .await
}

pub async fn note_details(page: &Page) -> Result<Locator> {
    page.get_by_role("button", "Note options").click().await?;
    page.get_by_role("menuitem", "Note details").click().await?;
    Ok(page.get_by_role("dialog", "Note details"))
}

/// Calendar views are a segmented radio group (Day, Week, Month, Agenda).
pub async fn calendar_view(page: &Page, view: &str) -> Result<()> {
    page.get_by_role("radiogroup", "Calendar view")
        .locator(&format!(
            "[role=radio][data-value={}]",
            serde_json::to_string(view)?
        ))
        .click()
        .await
}

/// Opens the spotlight search from the top bar (if needed) and returns its field.
pub async fn open_search(page: &Page) -> Result<Locator> {
    let field = page.get_by_role("combobox", "Search all apps");
    if !field.is_visible().await? {
        page.get_by_role("button", "Open search").click().await?;
    }
    field.wait_for().await?;
    Ok(field)
}

/// Escape clears the query, then closes the palette.
pub async fn close_search(page: &Page) -> Result<()> {
    let palette = page.locator("dialog.spotlight[open]");
    for _ in 0..2 {
        if palette.count().await? == 0 {
            break;
        }
        page.keyboard().press("Escape").await?;
    }
    palette.wait_for_state(State::Detached).await
}

/// Signs in through the page as `username` and waits for the app to connect.
pub async fn sign_in(page: &Page, base: &str, username: &str, password: &str) -> Result<()> {
    page.goto(base).await?;
    page.get_by_label("Username").fill(username).await?;
    page.get_by_label("Password").fill(password).await?;
    page.get_by_role("button", "Sign in").click().await?;
    signed_in(page).await
}

/// Waits until the app shown after signing in is up and live: the sign-in page and the startup
/// screens are gone, the app frame is shown and no live connection is still starting or coming
/// back. (Only checking that "Connecting…" is absent passes on the sign-in page itself, before
/// the sign-in request has returned.)
pub async fn signed_in(page: &Page) -> Result<()> {
    page.wait_for_function("() => !!document.querySelector('nav.app-rail') && !document.querySelector('.auth-page, .startup-state, .connection-state')").await?;
    Ok(())
}

/// Opens a menu with `button` until `item` is listed and enabled. Most menus take their items'
/// state when they open, so one opened just before a change lands (a settled thread) shows the
/// old state until it is opened again; the trigger toggles it closed for the next try.
pub async fn open_menu_for(_page: &Page, button: &Locator, item: &Locator) -> Result<()> {
    let deadline = std::time::Instant::now() + crate::browser::DEFAULT_TIMEOUT;
    let short = std::time::Duration::from_secs(2);
    loop {
        if !menu_expanded(button).await? {
            button.click().await?;
        }
        // An open menu may still enable the item (Files' New menu follows the connection).
        let ready = item.clone().timeout(short);
        if ready.wait_for().await.is_ok()
            && ready
                .wait_until("(el) => !el.disabled && el.getAttribute('aria-disabled') !== 'true'")
                .await
                .is_ok()
        {
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            crate::bail!(
                "{} did not offer an enabled {}",
                button.describe(),
                item.describe()
            );
        }
        // Not Escape: with every item disabled the focus is on no item, and a key the menu
        // does not see leaves it open.
        if menu_expanded(button).await? {
            button.click().await?;
        }
        item.clone()
            .timeout(short)
            .wait_for_state(State::Detached)
            .await
            .ok();
    }
}

async fn menu_expanded(button: &Locator) -> Result<bool> {
    Ok(button.get_attribute("aria-expanded").await?.as_deref() == Some("true"))
}

/// Clicks `button` until `dialog` is visible. An app still mounting (a lazily loaded app right
/// after switching to it) can drop the first click on its header's controls.
pub async fn open_dialog(button: &Locator, dialog: &Locator) -> Result<()> {
    let deadline = std::time::Instant::now() + crate::browser::DEFAULT_TIMEOUT;
    loop {
        button.click().await?;
        if dialog
            .clone()
            .timeout(std::time::Duration::from_secs(3))
            .wait_for()
            .await
            .is_ok()
        {
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            crate::bail!("{} did not open {}", button.describe(), dialog.describe());
        }
    }
}

/// The Notes editor's text field (CodeMirror's content element, a textbox named "File
/// contents") once the note has loaded and can be edited.
pub async fn note_source(editor: &Locator) -> Result<Locator> {
    editor
        .locator(r#".cm-content[aria-label="File contents"]:not([aria-readonly="true"])"#)
        .wait_for()
        .await?;
    Ok(editor.get_by_role("textbox", "File contents"))
}

/// The whole text in the Notes editor, through the hook the editor puts on its content element
/// for tests (`darEditor`, in `crates/client/editor/notes-editor.js`).
pub async fn note_text(source: &Locator) -> Result<String> {
    let text = source
        .evaluate("el => el.darEditor.text()", serde_json::Value::Null)
        .await?;
    Ok(text.as_str().unwrap_or_default().to_owned())
}

/// Focuses the Notes editor and selects from `anchor` to `head` (UTF-16 offsets, as
/// `setSelectionRange` takes them); `None` means the end of the text.
pub async fn note_select(
    source: &Locator,
    anchor: Option<usize>,
    head: Option<usize>,
) -> Result<()> {
    source
        .evaluate(
            "(el, [a, h]) => { const ed = el.darEditor, end = ed.text().length; ed.focus(); ed.select(a ?? end, h ?? end); }",
            serde_json::json!([anchor, head]),
        )
        .await?;
    Ok(())
}

/// The offset of `needle` in the Notes editor's text (UTF-16, as JavaScript's `indexOf`).
pub async fn note_offset(source: &Locator, needle: &str) -> Result<usize> {
    let at = source
        .evaluate(
            "(el, needle) => el.darEditor.text().indexOf(needle)",
            serde_json::json!(needle),
        )
        .await?;
    let at = at.as_i64().unwrap_or(-1);
    if at < 0 {
        return Err(crate::Error::new(format!("{needle:?} is not in the note")));
    }
    Ok(at as usize)
}
