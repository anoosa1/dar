//! Port of `test/contacts-layout-browser.mjs`: independent scrolling of the contact list and
//! details pane, collection action menu targeting & keyboard focus restoration, settings vCard
//! export/import round trip, and desktop/tablet/phone viewport checks.

use dar_acceptance::prelude::*;
use dar_acceptance::ui::Choice;
use std::sync::{Arc, Mutex};

pub const SUITE: Suite = Suite {
    name: "contacts-layout-browser",
    cases: &[case!(contacts_layout_and_menus, 120)],
};

pub async fn contacts_layout_and_menus(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 950))
        .await?;
    let page = context.new_page().await?;
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = errors.clone();
    page.on_page_error(move |message| sink.lock().unwrap().push(message.clone()));

    page.goto(&format!("{base}/contacts")).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.get_by_role("heading", "All contacts")
        .wait_for()
        .await?;

    let fixture = page
        .evaluate(
            r##"async () => {
                const {csrfToken} = await (await fetch("/api/v1/auth/session")).json();
                async function post(path, body) {
                    const response = await fetch(path, {
                        method: "POST",
                        headers: {"content-type": "application/json", "x-csrf-token": csrfToken},
                        body: JSON.stringify(body),
                    });
                    if (response.status !== 201) throw Error(await response.text());
                    return response.json();
                }
                const a = await post("/api/v1/contacts/books", {name: "Scrolling people", color: "#1e66f5"});
                const b = await post("/api/v1/contacts/books", {name: "Menu target book", color: "#1e66f5"});
                for (let i = 0; i < 45; i++) {
                    await post("/api/v1/contacts/books/" + a.id + "/items", {
                        data: {
                            displayName: "Scroll person " + String(i).padStart(2, "0"),
                            notes: "Long contact details\n".repeat(150),
                        },
                    });
                }
                return {a, b};
            }"##,
        )
        .await?;
    let book_a_id = fixture["a"]["id"].as_str().context("book a id")?;
    let book_a_name = fixture["a"]["name"].as_str().context("book a name")?;
    let book_b_name = fixture["b"]["name"].as_str().context("book b name")?;

    page.goto(&format!("{base}/contacts?book={book_a_id}"))
        .await?;
    page.get_by_role("button", regex("Scroll person 00"))
        .click()
        .await?;
    page.get_by_role("heading", "Scroll person 00")
        .wait_for()
        .await?;

    let details = page.get_by_role("complementary", "Contact details");
    let list = page.locator(".contact-list");
    let position = details.bounding_box().await?;

    list.evaluate("el => el.scrollTop = 1000", Value::Null)
        .await?;
    assert!(
        list.evaluate("el => el.scrollTop", Value::Null)
            .await?
            .as_f64()
            .unwrap_or(0.0)
            > 0.0
    );
    assert_eq!(details.bounding_box().await?, position);

    let list_scroll = list.evaluate("el => el.scrollTop", Value::Null).await?;
    details
        .evaluate("el => el.scrollTop = 500", Value::Null)
        .await?;
    assert!(
        details
            .evaluate("el => el.scrollTop", Value::Null)
            .await?
            .as_f64()
            .unwrap_or(0.0)
            > 0.0
    );
    assert_eq!(
        list.evaluate("el => el.scrollTop", Value::Null).await?,
        list_scroll
    );
    assert_eq!(page.evaluate("() => scrollY").await?, json!(0));
    assert_eq!(
        page.evaluate("() => document.documentElement.scrollHeight > innerHeight + 1")
            .await?,
        json!(false)
    );
    assert_eq!(
        page.locator(".sidebar")
            .get_by_role("link", "Export vCard")
            .count()
            .await?,
        0
    );
    assert_eq!(
        page.locator(".sidebar")
            .get_by_role("button", "Import vCard")
            .count()
            .await?,
        0
    );

    let actions = page.get_by_role("button", "Actions for address book Menu target book");
    let previous = page.get_by_role("button", "Previous address books");
    let next = page.get_by_role("button", "Next address books");
    while previous.count().await? > 0 && previous.is_enabled().await? {
        previous.click().await?;
    }
    while !actions.is_visible().await? && next.count().await? > 0 && next.is_enabled().await? {
        next.click().await?;
    }

    actions.click().await?;
    page.get_by_role("menuitem", "Edit address book")
        .click()
        .await?;
    assert_eq!(
        page.locator("[data-editor-pane]")
            .get_by_label("Name")
            .input_value()
            .await?,
        book_b_name
    );
    page.get_by_role("button", "Close editor").click().await?;

    actions.click().await?;
    page.get_by_role("menuitem", "Share address book")
        .click()
        .await?;
    page.get_by_role("dialog", "Share Menu target book")
        .wait_for()
        .await?;
    page.keyboard().press("Escape").await?;
    page.get_by_role("dialog", "Share Menu target book")
        .wait_for_state(State::Hidden)
        .await?;

    actions.click().await?;
    page.get_by_role("menuitem", "Delete address book")
        .click()
        .await?;
    let deletion = page.get_by_role("dialog", "Delete address book?");
    deletion
        .get_by_text("Remove Menu target book and all its 0 contacts? Export it first if you need a copy.")
        .wait_for()
        .await?;
    deletion.get_by_role("button", "Cancel").click().await?;
    deletion.wait_for_state(State::Hidden).await?;

    actions.focus().await?;
    page.keyboard().press("ArrowDown").await?;
    page.get_by_role("menuitem", "Edit address book")
        .wait_for()
        .await?;
    page.keyboard().press("Escape").await?;
    assert_eq!(
        actions
            .evaluate("el => el === document.activeElement", Value::Null)
            .await?,
        json!(true)
    );

    page.get_by_role("button", "Contacts settings")
        .click()
        .await?;
    let settings = page.get_by_role("dialog", "Contacts settings");
    settings
        .get_by_role("tab", "Import and export")
        .click()
        .await?;
    ui::choose(
        &settings.get_by_role("combobox", "Address book for import or export"),
        Choice::Label(book_a_name),
    )
    .await?;
    let download = page
        .wait_for_download(settings.get_by_role("link", "Export vCard").click())
        .await?;
    let exported = String::from_utf8_lossy(&download.read()?).into_owned();
    assert!(exported.contains("Scroll person 00"), "exported: {exported}");

    settings
        .get_by_label("Or paste content")
        .fill(&format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{}\r\nFN:Settings vCard proof\r\nEND:VCARD\r\n",
            uuid()
        ))
        .await?;
    settings
        .get_by_role("button", "Import vCard")
        .click()
        .await?;
    settings
        .get_by_text("1 imported; 0 failed.")
        .wait_for()
        .await?;
    settings
        .get_by_role("button", "Close settings")
        .click()
        .await?;
    page.reload().await?;
    page.get_by_role("button", regex("Settings vCard proof"))
        .wait_for()
        .await?;

    for (width, height) in [(1000, 700), (390, 844)] {
        page.set_viewport_size(width, height).await?;
        let footer = page
            .get_by_role("button", "Contacts settings")
            .bounding_box()
            .await?
            .context("Contacts settings button not visible")?;
        assert!(footer.y + footer.height <= height as f64);
        assert_eq!(
            page.locator(".sidebar")
                .evaluate("el => el.scrollHeight > el.clientHeight + 1", Value::Null)
                .await?,
            json!(false)
        );
        assert_eq!(
            page.evaluate("() => document.documentElement.scrollHeight > innerHeight + 1")
                .await?,
            json!(false)
        );
        assert_eq!(
            page.evaluate("() => document.documentElement.scrollWidth > innerWidth")
                .await?,
            json!(false)
        );
        page.screenshot(
            t.screenshot(&format!("contacts-independent-panes-{width}.png")),
            false,
        )
        .await?;
    }

    let collected = errors.lock().unwrap().clone();
    context.close().await?;
    assert_eq!(collected, Vec::<String>::new());
    Ok(())
}
