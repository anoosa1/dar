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
    let id = control.get_attribute("aria-controls").await?.unwrap_or_default();
    control.click().await?;
    let list = control.page().locator(&format!("[id={}]", serde_json::to_string(&id)?));
    match choice {
        Choice::Label(label) => list.get_by_role("option", label).click().await,
        Choice::Value(value) => list.locator(&format!("[role=option][data-value={}]", serde_json::to_string(value)?)).click().await,
    }
}

/// Switches app from the app rail (desktop) or the More menu (phones), and waits for it.
pub async fn switch_app(page: &Page, id: &str) -> Result<()> {
    let name = match id { "files" => "Files", "contacts" => "Contacts", "calendar" => "Calendar", "notes" => "Notes", "tasks" => "Tasks", other => other };
    let link = page.get_by_role("navigation", "Applications").get_by_role("link", name);
    if link.is_visible().await? {
        link.click().await?;
    } else {
        page.get_by_role("button", "More apps").click().await?;
        page.get_by_role("menu", "More apps").get_by_role("menuitem", name).click().await?;
    }
    let path = if id == "files" { "/".to_owned() } else { format!("/{id}") };
    page.wait_for_url(|url| url.path() == path).await?;
    // Optional apps briefly render a loading header before mounting the workspace.
    let main = match id { "files" => ".files-main", "contacts" => ".contacts-main", "calendar" => ".calendar-main", "notes" => ".notes-list", "tasks" => ".workspace-list", _ => "main" };
    page.locator(main).wait_for().await
}

pub async fn file_details(page: &Page, name: &str) -> Result<()> {
    item_action(page, name, "Details").await
}

pub async fn item_action(page: &Page, name: &str, action: &str) -> Result<()> {
    page.get_by_role("button", format!("Actions for {name}")).click().await?;
    page.get_by_role("menuitem", action).click().await
}

/// Picks a date (`YYYY-MM-DD`) in the calendar's date popover.
pub async fn calendar_date(page: &Page, date: &str) -> Result<()> {
    page.get_by_role("button", "Choose date").click().await?;
    let picker = page.get_by_role("dialog", "Choose calendar date");
    let year: u32 = date[..4].parse().unwrap();
    let month: u32 = date[5..7].parse().unwrap();
    choose(&picker.get_by_role("combobox", "Choose year"), Choice::Value(&year.to_string())).await?;
    choose(&picker.get_by_role("combobox", "Choose month"), Choice::Value(&(month - 1).to_string())).await?;
    picker.locator(&format!("[role=gridcell][data-date=\"{date}\"]")).click().await
}

pub async fn note_details(page: &Page) -> Result<Locator> {
    page.get_by_role("button", "Note options").click().await?;
    page.get_by_role("menuitem", "Note details").click().await?;
    Ok(page.get_by_role("dialog", "Note details"))
}

/// Calendar views are a segmented radio group (Day, Week, Month, Agenda).
pub async fn calendar_view(page: &Page, view: &str) -> Result<()> {
    page.get_by_role("radiogroup", "Calendar view").locator(&format!("[role=radio][data-value={}]", serde_json::to_string(view)?)).click().await
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
    page.wait_for_function("() => !document.body.textContent.includes(\"Connecting…\")").await?;
    Ok(())
}
