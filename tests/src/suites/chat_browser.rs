//! Port of `test/chat-browser.mjs`: WhatsApp QR linking, account tab and views, imported chats,
//! conversation with quotes, send with ticks, live incoming message, reply menu, reactions,
//! chat info, Add to Contacts, mute, Save to Files, search, settings sheet, phone layout and
//! both themes; Discord QR sign-in with terms notice, server tree, flat channel rows, reactions
//! as counts, channel info and DMs; voice members, call view, joining with fake microphone as
//! Opus, others' audio with speaking rings, call code, mute, dock while reading, phone dock and
//! leaving.

use crate::suites::comms_support::{read_bridge_log, write_control};
use dar_acceptance::prelude::*;
use std::time::{SystemTime, UNIX_EPOCH};

pub const SUITE: Suite = Suite {
    name: "chat-browser",
    cases: &[case!(chat_browser_journey, 240)],
};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub async fn chat_browser_journey(t: &Ctx) -> Result<()> {
    let fixture = t.fixture().await?;
    let base = fixture.base.clone();
    let control = fixture
        .chat_control
        .clone()
        .context("fixture chat_control")?;

    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions {
            permissions: vec!["microphone".into()],
            ..ContextOptions::sized(1440, 950)
        })
        .await?;
    let page = context.new_page().await?;
    let errors = page.collect_errors(|text| text.contains("Failed to load resource"));

    let overview = || page.evaluate("async () => (await fetch('/api/v1/chat/overview')).json()");

    page.goto(&format!("{base}/chat")).await?;
    page.get_by_label("Username").fill("admin").await?;
    page.get_by_label("Password").fill(PASSWORD).await?;
    page.get_by_role("button", "Sign in").click().await?;
    page.get_by_role("link", "Chat").first().wait_for().await?;
    page.get_by_role("heading", contains("No accounts linked"))
        .wait_for()
        .await?;

    // Link with a QR code drawn in the editing pane; the fake bridge "scans" it on request.
    page.get_by_role("button", contains("Link an account"))
        .first()
        .click()
        .await?;
    let pane = page.locator("[data-editor-pane]");
    assert!(
        pane.get_by_role("radio", contains("QR code"))
            .is_checked()
            .await?
    );
    pane.get_by_label(contains("Account name"))
        .fill("Personal")
        .await?;
    pane.get_by_role("button", contains("Show QR code"))
        .click()
        .await?;
    let qr = pane.get_by_role("img", contains("QR code to scan with WhatsApp"));
    qr.wait_for().await?;
    let d_attr = qr
        .locator("path")
        .get_attribute("d")
        .await?
        .unwrap_or_default();
    assert!(d_attr.len() > 500, "the QR code has modules");
    pane.get_by_text(regex("Waiting for scan"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("chat-link.png"), false)
        .await?;

    let account_id = overview().await?["accounts"][0]["id"]
        .as_str()
        .context("whatsapp account id")?
        .to_owned();
    write_control(&control, &account_id, "pair", None)?;
    page.get_by_text(contains("Linked. Your chats are on their way."))
        .wait_for()
        .await?;
    pane.wait_for_state(State::Detached).await?;

    // The account tab, its views and the imported chats.
    let tab = page.get_by_role("tab", regex("Personal"));
    tab.wait_for().await?;
    assert_eq!(
        tab.get_attribute("aria-selected").await?.as_deref(),
        Some("true")
    );
    let list = page.get_by_role("list", contains("Chats"));
    list.get_by_role("button", regex("Sara Haddad"))
        .wait_for()
        .await?;
    list.get_by_role("button", regex("Family"))
        .wait_for()
        .await?;
    assert_eq!(
        list.get_by_role("button", regex("Old friend"))
            .count()
            .await?,
        0,
        "archived chats are not in Chats"
    );
    page.get_by_role("button", regex("^Archived"))
        .click()
        .await?;
    page.get_by_role("list", contains("Archived"))
        .get_by_role("button", regex("Old friend"))
        .wait_for()
        .await?;
    page.get_by_role("button", regex("^Groups")).click().await?;
    page.get_by_role("list", contains("Groups"))
        .get_by_role("button", regex("Family"))
        .wait_for()
        .await?;
    page.get_by_role("button", regex("^Chats")).click().await?;

    // A conversation: history with a quote, then a sent message with its tick and a live incoming one.
    list.get_by_role("button", regex("Sara Haddad"))
        .click()
        .await?;
    let chat = page.get_by_role("complementary", contains("Sara Haddad"));
    let log = chat.get_by_role("log", contains("Messages in Sara Haddad"));
    log.get_by_text(contains("Are we still on for dinner?"))
        .wait_for()
        .await?;
    log.locator(".chat-quote")
        .get_by_text(contains("Yes!"))
        .wait_for()
        .await?;

    // The imported photo downloads through the bridge when shown.
    page.wait_for_function("() => document.querySelector('.chat-photo img')?.naturalWidth === 4")
        .await?;
    let box_input = chat.get_by_label(contains("Message Sara Haddad"));
    box_input.fill("See you at 7").await?;
    box_input.press("Enter").await?;
    let sent = log
        .locator(".chat-message.is-out")
        .filter_has_text("See you at 7");
    sent.get_by_text(contains("Sent")).wait_for().await?;
    assert_eq!(box_input.input_value().await?, "");

    write_control(
        &control,
        &account_id,
        "incoming",
        Some(&json!({
            "messages": [{ "id": "LIVE1", "chat": "15550002222@s.whatsapp.net", "sender": "15550002222@s.whatsapp.net", "fromMe": false, "at": now_ms(), "kind": "text", "text": "Perfect, see you at 7 then" }]
        })),
    )?;
    log.get_by_text(contains("Perfect, see you at 7 then"))
        .wait_for()
        .await?;

    // Reply from the message menu.
    let incoming = log
        .locator(".chat-message.is-in")
        .filter_has_text("Perfect, see you at 7 then");
    incoming.hover().await?;
    incoming
        .get_by_role("button", contains("Message actions"))
        .click()
        .await?;
    page.get_by_role("menuitem", contains("Reply"))
        .click()
        .await?;
    chat.get_by_text(contains("Replying to Sara Haddad"))
        .wait_for()
        .await?;
    box_input.press("Escape").await?;
    assert_eq!(
        chat.get_by_text(contains("Replying to Sara Haddad"))
            .count()
            .await?,
        0
    );

    // React from the message menu; the chip shows, and clicking it removes the reaction.
    incoming.hover().await?;
    incoming
        .get_by_role("button", contains("Message actions"))
        .click()
        .await?;
    page.get_by_role("menuitem", contains("React"))
        .click()
        .await?;
    page.get_by_role("menuitemradio", contains("❤️"))
        .click()
        .await?;
    let chip = incoming.get_by_role("button", regex("Remove your reaction"));
    chip.wait_for().await?;

    // Chat info opens beside the conversation; mute goes through its menu.
    chat.get_by_role("button", contains("Chat info"))
        .click()
        .await?;
    let info_sheet = page.get_by_role("region", contains("Chat info"));
    info_sheet
        .get_by_role("heading", contains("Contact info"))
        .wait_for()
        .await?;

    // Add to Contacts saves the number in a Dar address book (created here through the API) and names the chat.
    let api_session = login(&base).await?;
    api_session
        .post(
            "/api/v1/contacts/books",
            &json!({ "name": "Chat test book", "color": "#2563eb" }),
        )
        .await?
        .expect(201)?;
    info_sheet
        .get_by_text(contains("Not in your contacts"))
        .wait_for()
        .await?;
    info_sheet.get_by_role("button", "Add").click().await?;
    let add = page.locator("[data-editor-pane]");
    add.get_by_role("heading", contains("Add to Contacts"))
        .wait_for()
        .await?;
    assert_eq!(
        add.get_by_label(contains("Name")).input_value().await?,
        "Sara Haddad"
    );
    add.get_by_role("button", contains("Add contact"))
        .click()
        .await?;
    page.get_by_text(contains("Sara Haddad added to Contacts"))
        .wait_for()
        .await?;
    info_sheet
        .get_by_text(contains("Dar contact"))
        .wait_for()
        .await?;
    info_sheet
        .get_by_role("button", contains("Mute"))
        .click()
        .await?;
    page.get_by_role("menuitem", contains("8 hours"))
        .click()
        .await?;
    page.get_by_text(contains("Chat muted")).wait_for().await?;
    info_sheet
        .get_by_role("button", contains("Unmute"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("chat-desktop.png"), false)
        .await?;
    chip.click().await?;
    chip.wait_for_state(State::Detached).await?;
    info_sheet
        .get_by_role("button", contains("Close chat info"))
        .click()
        .await?;
    info_sheet.wait_for_state(State::Detached).await?;

    // A document in the group: Save to Files from the message menu, into the shared folder picker.
    page.get_by_role("list", contains("Chats"))
        .get_by_role("button", regex("Family"))
        .click()
        .await?;
    let group = page.get_by_role("complementary", contains("Family"));
    let document = group
        .locator(".chat-message")
        .filter_has_text("Eid lunch menu.pdf");
    document.wait_for().await?;
    document.hover().await?;
    document
        .get_by_role("button", contains("Message actions"))
        .click()
        .await?;
    page.get_by_role("menuitem", contains("Save to Files…"))
        .click()
        .await?;
    let save = page.locator("[data-editor-pane]");
    save.get_by_role("heading", contains("Save to Files"))
        .wait_for()
        .await?;
    save.get_by_role("button", regex("^Save to"))
        .click()
        .await?;
    page.get_by_text(contains("Saved to Files as Eid lunch menu.pdf"))
        .wait_for()
        .await?;
    save.wait_for_state(State::Detached).await?;
    page.get_by_role("list", contains("Chats"))
        .get_by_role("button", regex("Sara Haddad"))
        .click()
        .await?;
    chat.get_by_label(contains("Message Sara Haddad"))
        .wait_for()
        .await?;

    // The shared search finds the message; its result opens the chat with the message outlined.
    page.get_by_role("button", "Close chat").click().await?;
    page.keyboard().press("Control+k").await?;
    let search = page.get_by_role("combobox", "Search all apps");
    search.fill("see you at 7 then").await?;
    let results = page.get_by_role("listbox", "Search results");
    results
        .get_by_role("option", regex("Sara Haddad"))
        .click()
        .await?;
    page.locator(".chat-message.is-found")
        .filter_has_text("Perfect, see you at 7 then")
        .wait_for()
        .await?;

    // Settings sheet lists the account.
    page.get_by_role("button", "Chat settings")
        .first()
        .click()
        .await?;
    let sheet = page.get_by_role("dialog", contains("Chat settings"));
    sheet
        .get_by_text(contains("Linked accounts"))
        .wait_for()
        .await?;
    sheet.get_by_text("Personal").wait_for().await?;
    sheet
        .get_by_role("button", contains("Close settings"))
        .click()
        .await?;
    sheet.wait_for_state(State::Detached).await?;

    // Both themes, desktop and phone: no sideways scrolling; on phones the chat opens full screen with Back.
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
                    "() => document.documentElement.scrollWidth > innerWidth ? [...document.querySelectorAll('body *')].filter(e => e.getBoundingClientRect().right > innerWidth + 1 && !e.closest('.sidebar')).slice(0, 8).map(e => e.tagName + '.' + [...e.classList].join('.')) : []",
                )
                .await?;
            assert_eq!(overflow, json!([]), "{theme} {width} overflow");
            if width == 390 {
                let back = page.get_by_role("button", contains("Back to chats"));
                if !back.is_visible().await? {
                    page.get_by_role("list", contains("Chats"))
                        .get_by_role("button", regex("Sara Haddad"))
                        .click()
                        .await?;
                }
                back.wait_for().await?;
                assert!(!page.locator(".chat-main").is_visible().await?);
                chat.get_by_label(contains("Message Sara Haddad"))
                    .wait_for()
                    .await?;
                page.screenshot(t.screenshot(&format!("chat-phone-{theme}.png")), false)
                    .await?;
                back.click().await?;
                page.locator(".chat-main").wait_for().await?;
                page.get_by_role("tab", regex("Personal"))
                    .wait_for()
                    .await?;
                page.screenshot(t.screenshot(&format!("chat-phone-list-{theme}.png")), false)
                    .await?;
            }
        }
    }

    // Discord: a second account tab, signed in by QR code with the terms notice.
    page.evaluate("() => { document.documentElement.dataset.theme = 'mocha'; }")
        .await?;
    page.set_viewport_size(1440, 950).await?;
    page.wait_for_timeout(200).await;
    page.get_by_role("tablist", contains("Accounts"))
        .get_by_role("button", contains("Link an account"))
        .click()
        .await?;
    let link = page.locator("[data-editor-pane]");
    link.get_by_role("radio", contains("Discord"))
        .click()
        .await?;
    link.get_by_text(contains(
        "Discord does not allow other apps on your account",
    ))
    .wait_for()
    .await?;
    assert_eq!(
        link.get_by_role("radio", contains("QR code"))
            .count()
            .await?,
        0,
        "Discord has no pairing code"
    );
    link.get_by_role("button", contains("Show QR code"))
        .click()
        .await?;
    link.get_by_role("img", contains("QR code to scan with Discord"))
        .wait_for()
        .await?;
    link.get_by_text(regex("Waiting for the Discord app"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("chat-discord-link.png"), false)
        .await?;

    let dc_id = overview().await?["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["platform"] == json!("discord"))
        .and_then(|a| a["id"].as_str())
        .context("discord account id")?
        .to_owned();
    write_control(&control, &dc_id, "scan", None)?;
    link.get_by_text(regex("Scanned by anas"))
        .wait_for()
        .await?;
    write_control(&control, &dc_id, "pair", None)?;
    link.wait_for_state(State::Detached).await?;

    let dc_tab = page.get_by_role("tab", regex("Discord"));
    dc_tab.wait_for().await?;
    assert_eq!(
        dc_tab.get_attribute("aria-selected").await?.as_deref(),
        Some("true")
    );

    // Servers with their channels in the sidebar; direct messages in the list.
    let servers = page.get_by_role("navigation", contains("Servers"));
    servers
        .get_by_role("button", regex("Homelab Club"))
        .wait_for()
        .await?;
    servers
        .get_by_role("button", regex("^Voice channel Lounge"))
        .wait_for()
        .await?;
    servers
        .get_by_role("button", regex("Channel off-topic.*muted"))
        .wait_for()
        .await?;
    page.get_by_role("list", contains("Direct messages"))
        .get_by_role("button", regex("Lina K"))
        .wait_for()
        .await?;

    // A channel replaces the list sheet; messages are flat rows.
    servers
        .get_by_role("button", regex("Channel general, 1 mentions"))
        .click()
        .await?;
    let channel = page.get_by_role("complementary", contains("general"));
    let rows = channel.get_by_role("log", contains("Messages in #general"));
    rows.get_by_text(contains("Anyone moved to the new caddy module?"))
        .wait_for()
        .await?;
    assert!(!page.locator(".chat-main").is_visible().await?);
    channel
        .get_by_role("heading", contains("#general"))
        .wait_for()
        .await?;
    channel
        .get_by_text(contains("Say hello"))
        .wait_for()
        .await?;
    page.wait_for_function("() => document.querySelector('.chat-conversation.is-flat .chat-photo img')?.naturalWidth === 4").await?;

    // Reactions are counts; the account's own toggles off and others stay.
    let painless = rows
        .locator(".chat-message")
        .filter_has_text("Yes, painless");
    painless
        .get_by_role(
            "button",
            regex(r"^👍 3, You and 2 others\. Remove your reaction"),
        )
        .click()
        .await?;
    painless
        .get_by_role("button", regex(r"^👍 2\. React with 👍"))
        .wait_for()
        .await?;
    painless.get_by_text(contains(":party:")).wait_for().await?;
    let dc_box = channel.get_by_label(contains("Message #general"));
    dc_box.fill("hello from Dar").await?;
    dc_box.press("Enter").await?;
    rows.locator(".chat-message")
        .filter_has_text("hello from Dar")
        .wait_for()
        .await?;
    assert_eq!(dc_box.input_value().await?, "");
    page.screenshot(t.screenshot("chat-discord-channel.png"), false)
        .await?;

    // Channel info: topic and server; Discord chats mute but do not pin or archive.
    channel
        .get_by_role("button", contains("Channel info"))
        .click()
        .await?;
    let dc_info = page.get_by_role("region", contains("Chat info"));
    dc_info
        .get_by_role("heading", contains("Channel info"))
        .wait_for()
        .await?;
    dc_info
        .get_by_text(regex("Homelab Club · Text channels · Discord"))
        .wait_for()
        .await?;
    assert_eq!(
        dc_info
            .get_by_role("button", contains("Pin"))
            .count()
            .await?,
        0
    );
    dc_info
        .get_by_role("button", contains("Close chat info"))
        .click()
        .await?;

    // Servers collapse; Direct messages brings back the list.
    servers
        .get_by_role("button", regex("Homelab Club"))
        .click()
        .await?;
    assert_eq!(
        servers
            .get_by_role("button", regex("Channel general"))
            .count()
            .await?,
        0
    );
    page.get_by_role("navigation", contains("Direct messages"))
        .get_by_role("button", regex("Direct messages"))
        .click()
        .await?;
    page.locator(".chat-main").wait_for().await?;
    page.get_by_role("list", contains("Direct messages"))
        .get_by_role("button", regex("Lina K"))
        .click()
        .await?;
    page.get_by_role("log", contains("Messages in Lina K"))
        .get_by_text(contains("Hopping into Lounge later?"))
        .wait_for()
        .await?;

    // Voice: who is in Lounge shows under it; opening it shows the call view, and joining sends the
    // microphone as Opus and plays the others (the fake bridge echoes it back as Lina).
    servers
        .get_by_role("button", regex("Homelab Club"))
        .click()
        .await?;
    servers
        .get_by_role("button", contains("Voice channel Lounge, 2 in voice"))
        .click()
        .await?;
    servers
        .get_by_role("list", contains("In voice"))
        .get_by_text(contains("Lina K"))
        .wait_for()
        .await?;
    let stage = page.get_by_role("region", contains("Voice channel Lounge"));
    stage
        .get_by_role("heading", contains("2 people are in Lounge"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("chat-voice-join.png"), false)
        .await?;
    stage
        .get_by_role("button", contains("Join voice"))
        .click()
        .await?;
    let dock = page
        .locator(".chat-sidebar")
        .get_by_role("region", contains("Voice call"));
    dock.get_by_text(contains("Voice connected"))
        .wait_for()
        .await?;
    stage
        .get_by_role("list", contains("In the call"))
        .get_by_text(contains("Anas (you)"))
        .wait_for()
        .await?;
    servers
        .get_by_role(
            "button",
            contains("Voice channel Lounge, you are in this call"),
        )
        .wait_for()
        .await?;

    for _ in 0..100 {
        if read_bridge_log(&control)
            .iter()
            .any(|e| e["method"] == json!("voiceAudio"))
        {
            break;
        }
        page.wait_for_timeout(100).await;
    }
    assert!(
        read_bridge_log(&control)
            .iter()
            .any(|e| e["method"] == json!("voiceAudio") && e["bytes"].as_u64().unwrap_or(0) > 0),
        "the microphone reached the bridge as Opus"
    );
    stage
        .locator(".chat-voice-tile.is-speaking")
        .filter_has_text("Lina K")
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("chat-voice-call.png"), false)
        .await?;

    // The call code of an encrypted call; mute and deafen; the dock stays while reading a text channel.
    stage
        .get_by_role("button", contains("End-to-end encrypted"))
        .click()
        .await?;
    let code = page.get_by_role("dialog", contains("Verify the call is private"));
    code.get_by_text(contains("48312")).wait_for().await?;
    code.get_by_role("button", "Close").click().await?;
    dock.get_by_role("button", contains("Mute")).click().await?;
    page.wait_for_function("() => document.querySelector('.chat-sidebar .chat-voice-dock [aria-label=Mute]')?.getAttribute('aria-pressed') === 'true'").await?;
    assert!(
        read_bridge_log(&control)
            .iter()
            .any(|e| e["method"] == json!("voiceSet") && e["muted"] == json!(true))
    );
    stage
        .locator(".chat-voice-tile")
        .filter_has_text("Anas (you)")
        .get_by_title(contains("Muted"))
        .wait_for()
        .await?;
    servers
        .get_by_role("button", regex("Channel general"))
        .click()
        .await?;
    page.get_by_role("log", contains("Messages in #general"))
        .wait_for()
        .await?;
    dock.get_by_text(contains("Lounge · Homelab Club"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("chat-voice-dock.png"), false)
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
                    "() => document.documentElement.scrollWidth > innerWidth ? [...document.querySelectorAll('body *')].filter(e => e.getBoundingClientRect().right > innerWidth + 1 && !e.closest('.sidebar')).slice(0, 8).map(e => e.tagName + '.' + [...e.classList].join('.')) : []",
                )
                .await?;
            assert_eq!(overflow, json!([]), "Discord {theme} {width} overflow");
            page.screenshot(
                t.screenshot(&format!("chat-discord-{width}-{theme}.png")),
                false,
            )
            .await?;
        }
    }

    // On a phone the call view fills the screen and the dock sits under the list.
    page.set_viewport_size(390, 844).await?;
    page.get_by_role("button", contains("Back to chats"))
        .click()
        .await?;
    let phone_dock = page
        .locator(".chat-main")
        .get_by_role("region", contains("Voice call"));
    phone_dock.wait_for().await?;
    page.screenshot(t.screenshot("chat-voice-phone-dock.png"), false)
        .await?;
    phone_dock
        .get_by_role("button", regex("Voice connected"))
        .click()
        .await?;
    stage
        .get_by_role("button", contains("Leave voice"))
        .wait_for()
        .await?;
    page.screenshot(t.screenshot("chat-voice-phone-call.png"), false)
        .await?;
    stage
        .get_by_role("button", contains("Leave voice"))
        .click()
        .await?;
    stage
        .get_by_role("button", contains("Join voice"))
        .wait_for()
        .await?;
    assert_eq!(
        page.get_by_role("region", contains("Voice call"))
            .count()
            .await?,
        0
    );
    assert!(
        read_bridge_log(&control)
            .iter()
            .any(|e| e["method"] == json!("voiceLeave"))
    );

    assert_eq!(*errors.lock().unwrap(), Vec::<String>::new());
    context.close().await?;
    Ok(())
}
