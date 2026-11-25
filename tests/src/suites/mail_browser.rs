//! Port of `test/mail-browser.mjs`: Mail in a real browser on an isolated instance with a fake
//! IMAP/SMTP provider — three-pane layout, reader with blocked remote images, inline reply with
//! Undo, compose sheet and drafts, keyboard archive with Undo, Markdown toolbar & preview,
//! settings with providers, adding an account, phones and both themes.

use crate::suites::comms_support::{
    MailAttachment, MailMessageSpec, MailServer, build_mail_message,
};
use chrono::Utc;
use dar_acceptance::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "mail-browser",
    cases: &[case!(mail_browser_journey, 240)],
};

async fn mail_browser_journey(t: &Ctx) -> Result<()> {
    let root = t.temp("mail-browser")?;
    let mut server = MailServer::start(&root).await?;
    let now = Utc::now();
    {
        let mut s = server.state.lock().unwrap();
        s.mail.put(
            "INBOX",
            build_mail_message(MailMessageSpec {
                subject: "Q4 venue shortlist",
                id: Some("v1@example.test"),
                body: "Two venues so far.",
                date: now - chrono::Duration::days(3),
                ..Default::default()
            }),
            &["\\Seen"],
        );
        s.mail.put(
            "INBOX",
            build_mail_message(MailMessageSpec {
                subject: "Re: Q4 venue shortlist",
                id: Some("v2@example.test"),
                references: &["v1@example.test"],
                cc: Some("Rami <rami@example.test>"),
                date: now - chrono::Duration::hours(1),
                html: Some(r#"<p>I've attached the revised quote from the second venue.</p><p><img src="https://tracker.example/p.gif" alt="pixel"></p>"#),
                attachment: Some(MailAttachment {
                    name: "quote.pdf",
                    content_type: "application/pdf",
                    data: b"%PDF-1.4",
                }),
                ..Default::default()
            }),
            &[],
        );
        s.mail.put(
            "INBOX",
            build_mail_message(MailMessageSpec {
                subject: "Dinner on Friday?",
                from: "Omar Saleh <omar@example.test>",
                body: "Around 8?",
                date: now - chrono::Duration::minutes(10),
                ..Default::default()
            }),
            &[],
        );
    }

    let providers_file = root.join("providers.json");
    let providers = json!([
        {
            "id": "work-mail",
            "name": "Work Mail",
            "document": {
                "protocol": "imap",
                "auth": "password",
                "domains": ["example.test"],
                "imap": { "host": "127.0.0.1", "port": server.imap_port, "security": "tls" },
                "smtp": { "host": "127.0.0.1", "port": server.smtp_port, "security": "tls" },
            }
        },
        {
            "id": "google",
            "name": "Google",
            "document": {
                "protocol": "imap",
                "auth": "oauth",
                "domains": ["gmail.com"],
                "imap": { "host": "127.0.0.1", "port": server.imap_port, "security": "tls" },
                "smtp": { "host": "127.0.0.1", "port": server.smtp_port, "security": "tls" },
                "oauth": {
                    "authorizeUrl": format!("{}/oauth/authorize", server.https),
                    "tokenUrl": format!("{}/oauth/token", server.https),
                    "clientId": "dar-test-client",
                    "scopes": ["mail"],
                }
            }
        },
    ]);
    std::fs::write(&providers_file, serde_json::to_string(&providers)?)?;

    let mut cfg = InstanceConfig::default();
    cfg.env.push((
        "DAR_MAIL_PROVIDERS_FILE".into(),
        providers_file.to_string_lossy().into_owned(),
    ));
    cfg.env.push((
        "SSL_CERT_FILE".into(),
        server.ca_file.to_string_lossy().into_owned(),
    ));
    let runtime = Instance::isolated(&root, "dar-mail-ui", cfg, true).await?;
    let base = runtime.base.clone();
    session::setup_admin(&base).await?;

    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1440, 950))
        .await?;
    let page = context.new_page().await?;
    let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let e = errors.clone();
        page.on_page_error(move |msg| e.lock().unwrap().push(msg.clone()));
    }
    {
        let e = errors.clone();
        page.on_console(move |m| {
            if m.kind == "error" && !m.text.contains("Failed to load resource") {
                e.lock().unwrap().push(m.text.clone());
            }
        });
    }

    page.goto(&format!("{base}/mail")).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.get_by_role("link", "Mail").first().wait_for().await?;

    // No accounts yet: the empty state offers Add account; Cancel closes cleanly; the pane picks the provider by domain.
    page.get_by_role("heading", contains("Add your first mail account"))
        .wait_for()
        .await?;
    page.get_by_role("button", "Add account")
        .last()
        .click()
        .await?;
    let pane = page.locator("[data-editor-pane]");
    pane.get_by_role("button", "Cancel").click().await?;
    pane.wait_for_state(State::Detached).await?;
    page.get_by_role("button", "Add account")
        .last()
        .click()
        .await?;
    pane.get_by_label("Email address")
        .fill("someone@gmail.com")
        .await?;
    assert!(
        pane.get_by_role("radio", regex("Google"))
            .is_checked()
            .await?
    );
    pane.get_by_role("button", contains("Continue with Google"))
        .wait_for()
        .await?;
    pane.get_by_role("radio", regex("Work Mail"))
        .check()
        .await?;
    pane.get_by_label("Email address")
        .fill("anas@example.test")
        .await?;
    pane.get_by_label(contains("Account name"))
        .fill("Work")
        .await?;
    pane.get_by_label(contains("Password or app password"))
        .fill("wrong password")
        .await?;
    pane.get_by_role("button", contains("Sign in and add"))
        .click()
        .await?;
    pane.get_by_role("alert", ())
        .filter_has_text(regex("rejected the sign-in"))
        .wait_for()
        .await?;
    pane.get_by_label(contains("Password or app password"))
        .fill("imap secret password")
        .await?;
    pane.get_by_role("button", contains("Sign in and add"))
        .click()
        .await?;
    pane.wait_for_state(State::Detached).await?;

    // The first pass mirrors the mailbox; the list groups the 2-message Q4 thread into 1 row.
    let list = page.get_by_role("list", contains("All inboxes messages"));
    list.get_by_role("button", regex(r"Dinner on Friday\?"))
        .timeout(Duration::from_secs(40))
        .wait_for()
        .await?;
    page.get_by_role("button", regex("^Inbox"))
        .first()
        .wait_for()
        .await?;
    assert_eq!(
        list.locator(".mail-message").count().await?,
        2,
        "conversation grouping should collapse Q4 venue shortlist replies into one row"
    );

    // Quick filter chips filter the conversation list client-side.
    page.get_by_role("button", "Has files").click().await?;
    assert_eq!(list.locator(".mail-message").count().await?, 1);
    page.get_by_role("button", "All").first().click().await?;
    assert_eq!(list.locator(".mail-message").count().await?, 2);

    // `?` opens the keyboard shortcuts dialog and Escape closes it.
    page.locator("body").press("?").await?;
    let shortcuts = page.get_by_role("dialog", contains("Mail keyboard shortcuts"));
    shortcuts.wait_for().await?;
    page.locator("body").press("Escape").await?;
    shortcuts.wait_for_state(State::Detached).await?;

    // Reader: the thread, blocked remote images, Show images, attachments.
    list.get_by_role("button", regex("Re: Q4 venue shortlist"))
        .click()
        .await?;
    let reader = page.get_by_role("complementary", contains("Message"));
    reader
        .get_by_role("heading", contains("Re: Q4 venue shortlist"))
        .wait_for()
        .await?;
    reader
        .get_by_text(contains("I've attached the revised quote"))
        .wait_for()
        .await?;
    reader
        .get_by_text(contains("1 remote image blocked"))
        .wait_for()
        .await?;
    assert_eq!(reader.locator(".mail-body img").count().await?, 0);
    page.screenshot(t.screenshot("mail-desktop.png"), false)
        .await?;
    reader
        .get_by_role("button", contains("Show images"))
        .click()
        .await?;
    reader
        .locator(".mail-body img[src*='/api/v1/mail/image?url=']")
        .wait_for_state(State::Attached)
        .await?;
    reader
        .get_by_role("link", contains("Download quote.pdf"))
        .wait_for()
        .await?;
    reader
        .get_by_text(contains("2 messages"))
        .wait_for()
        .await?;

    // Inline reply (Reply all: it went to two people), Undo puts it in the compose sheet.
    reader
        .get_by_role("button", contains("Reply all"))
        .click()
        .await?;
    let reply_box = reader.get_by_role("textbox", contains("Reply"));
    reply_box.fill("Great, please confirm.").await?;
    reader
        .get_by_text(regex(r"to lina@example\.test, rami@example\.test"))
        .wait_for()
        .await?;
    reader.get_by_role("button", "Send").click().await?;
    let toast = page.locator(".mail-toast");
    toast.get_by_text(regex("Sending")).wait_for().await?;
    toast
        .get_by_role("button", contains("Undo"))
        .click()
        .await?;
    let sheet = page.get_by_role("dialog", contains("Compose message"));
    sheet.wait_for().await?;
    let sheet_msg = sheet
        .get_by_role("textbox", contains("Message"))
        .input_value()
        .await?;
    assert!(
        sheet_msg.starts_with("Great, please confirm.") && sheet_msg.contains("> I've attached"),
        "unexpected reply quote: {sheet_msg}"
    );
    sheet
        .get_by_role("textbox", contains("Message"))
        .press("Control+Enter")
        .await?;
    page.get_by_text(contains("Message sent"))
        .timeout(Duration::from_secs(20))
        .wait_for()
        .await?;
    {
        let s = server.state.lock().unwrap();
        assert_eq!(s.sent.len(), 1, "Undo stopped the first send");
        let mut to = s.sent[0].to.clone();
        to.sort();
        assert_eq!(to, vec!["lina@example.test", "rami@example.test"]);
    }

    // Archive by keyboard, then Undo.
    list.get_by_role("button", regex(r"Dinner on Friday\?"))
        .click()
        .await?;
    reader
        .get_by_role("heading", contains("Dinner on Friday?"))
        .wait_for()
        .await?;
    page.locator("body").press("e").await?;
    toast
        .get_by_text(contains("Message archived"))
        .wait_for()
        .await?;
    assert_eq!(
        list.get_by_role("button", regex(r"Dinner on Friday\?"))
            .count()
            .await?,
        0
    );
    toast
        .get_by_role("button", contains("Undo"))
        .click()
        .await?;
    list.get_by_role("button", regex(r"Dinner on Friday\?"))
        .wait_for()
        .await?;

    // Compose a new message; closing saves a draft on the server.
    page.get_by_role("button", "Compose").click().await?;
    sheet.wait_for().await?;
    sheet
        .get_by_label(contains("To"))
        .fill("maya@example.test, not-an-address")
        .await?;
    sheet.get_by_label(contains("To")).press("Enter").await?;
    sheet
        .get_by_text(contains("not-an-address"))
        .wait_for()
        .await?;
    sheet
        .get_by_role("button", contains("Remove not-an-address"))
        .click()
        .await?;
    sheet
        .get_by_label(contains("Subject"))
        .fill("Photos from the weekend")
        .await?;
    sheet
        .get_by_role("textbox", contains("Message"))
        .fill("Here they are.")
        .await?;
    sheet
        .get_by_role("button", contains("Close (saves a draft)"))
        .click()
        .await?;
    page.get_by_text(contains("Draft saved"))
        .first()
        .wait_for()
        .await?;
    page.get_by_role("button", regex("^Drafts"))
        .first()
        .click()
        .await?;
    let drafts_list = page.get_by_role("list", contains("Drafts messages"));
    drafts_list
        .get_by_text(contains("To maya@example.test"))
        .wait_for()
        .await?;
    drafts_list
        .get_by_role("button", regex("Photos from the weekend"))
        .click()
        .await?;
    reader
        .get_by_role("button", contains("Continue editing"))
        .click()
        .await?;
    assert_eq!(
        sheet
            .get_by_label(contains("Subject"))
            .input_value()
            .await?,
        "Photos from the weekend"
    );
    sheet
        .get_by_role("button", contains("Discard"))
        .click()
        .await?;
    sheet.wait_for_state(State::Detached).await?;
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .mail
            .boxes
            .get("Drafts")
            .unwrap()
            .messages
            .len(),
        1
    );

    // Markdown: the toolbar formats the selection, Preview shows it, and it is sent as HTML.
    page.get_by_role("button", "Compose").click().await?;
    sheet.wait_for().await?;
    sheet
        .get_by_label(contains("To"))
        .fill("lina@example.test")
        .await?;
    sheet.get_by_label(contains("To")).press("Enter").await?;
    sheet
        .get_by_label(contains("Subject"))
        .fill("Agenda")
        .await?;
    // No format choice: plain until a formatting button is used.
    assert_eq!(
        sheet
            .get_by_label(contains("Message format"))
            .count()
            .await?,
        0
    );
    let body = sheet.get_by_role("textbox", contains("Message"));
    assert_eq!(
        sheet
            .get_by_role("radio", contains("Preview"))
            .count()
            .await?,
        0
    );
    body.fill("Venue first").await?;
    body.evaluate("el => el.setSelectionRange(0, 5)", Value::Null)
        .await?;
    sheet
        .get_by_role("button", contains("Bold"))
        .click()
        .await?;
    assert_eq!(body.input_value().await?, "**Venue** first");
    sheet
        .get_by_role("radio", contains("Preview"))
        .check()
        .await?;
    sheet
        .locator(".mail-compose-preview strong")
        .filter_has_text("Venue")
        .wait_for()
        .await?;
    sheet.get_by_role("button", "Send").click().await?;
    page.get_by_text(contains("Message sent"))
        .timeout(Duration::from_secs(20))
        .wait_for()
        .await?;
    {
        let s = server.state.lock().unwrap();
        let last_data = &s.sent.last().unwrap().data;
        assert!(
            last_data.contains("<strong>Venue</strong> first"),
            "{last_data}"
        );
        assert!(last_data.contains("**Venue** first"), "{last_data}");
    }

    // Settings: accounts, reading, and providers for the administrator.
    page.get_by_role("button", contains("Mail settings"))
        .first()
        .click()
        .await?;
    let settings = page.get_by_role("dialog", contains("Mail settings"));
    settings
        .get_by_text(contains("anas@example.test · Work Mail · IMAP and SMTP"))
        .wait_for()
        .await?;
    settings
        .get_by_role("tab", contains("Reading"))
        .click()
        .await?;
    settings
        .get_by_role("radio", contains("Load"))
        .check()
        .await?;
    settings
        .get_by_text(contains("Saved"))
        .first()
        .wait_for()
        .await?;
    settings
        .get_by_role("radio", contains("Block"))
        .check()
        .await?;
    settings
        .get_by_role("tab", contains("Providers"))
        .click()
        .await?;
    settings
        .get_by_text(contains("From the host configuration"))
        .first()
        .wait_for()
        .await?;
    settings
        .get_by_role("button", contains("Add provider"))
        .click()
        .await?;
    settings
        .get_by_label(contains("Start from"))
        .select_option("google")
        .await?;
    let name_field = settings.get_by_role("textbox", regex("^Name"));
    assert_eq!(name_field.input_value().await?, "Google");
    settings
        .get_by_text(contains("/api/v1/mail/oauth/callback"))
        .wait_for()
        .await?;
    settings
        .get_by_role("button", contains("Cancel"))
        .click()
        .await?;
    settings
        .get_by_role("button", contains("Close settings"))
        .click()
        .await?;
    settings.wait_for_state(State::Detached).await?;

    // Phones and both themes: no sideways scrolling; the message opens full screen with Back.
    page.get_by_role("button", contains("All inboxes"))
        .first()
        .click()
        .await?;
    for theme in ["latte", "mocha"] {
        page.evaluate_with(
            "(value) => { document.documentElement.dataset.theme = value; }",
            json!(theme),
        )
        .await?;
        for width in [1440u32, 390u32] {
            page.set_viewport_size(width, 844).await?;
            page.wait_for_timeout(300).await;
            let overflow = page
                .evaluate(
                    "() => document.documentElement.scrollWidth > innerWidth ? [...document.querySelectorAll('body *')].filter(e => e.getBoundingClientRect().right > innerWidth + 1 && !e.closest('.sidebar')).slice(0, 8).map(e => e.className) : []",
                )
                .await?;
            assert_eq!(overflow, json!([]), "{theme} {width} overflow");
            if width == 1440 {
                list.get_by_role("button", regex("Re: Q4 venue shortlist"))
                    .click()
                    .await?;
                reader
                    .get_by_role("heading", contains("Re: Q4 venue shortlist"))
                    .wait_for()
                    .await?;
                page.screenshot(t.screenshot(&format!("mail-desktop-{theme}.png")), false)
                    .await?;
                page.get_by_role("button", contains("Close message"))
                    .click()
                    .await?;
            }
            if width == 390 {
                list.get_by_role("button", regex(r"Dinner on Friday\?"))
                    .click()
                    .await?;
                let back = page.get_by_role("button", contains("Back to messages"));
                back.wait_for().await?;
                assert!(!page.locator(".mail-main").is_visible().await?);
                page.screenshot(t.screenshot(&format!("mail-phone-{theme}.png")), false)
                    .await?;
                back.click().await?;
                page.locator(".mail-main").wait_for().await?;
            }
        }
    }

    assert_eq!(*errors.lock().unwrap(), Vec::<String>::new());
    context.close().await?;
    runtime.close().await;
    server.close();
    Ok(())
}
