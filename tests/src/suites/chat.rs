//! Port of `test/chat.integration.test.ts`: link, import, send, receive, receipts, encrypted keys,
//! attachments, reactions, edits/revokes, settings, contacts, search, history, restart, isolation,
//! backups, unlink, disable, delete and pairing code.

use crate::suites::comms_support::{read_bridge_log, sql_text, until, write_control};
use dar_acceptance::prelude::*;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SUITE: Suite = Suite { name: "chat", cases: &[case!(chat_whatsapp_lifecycle, 240)] };

fn encode_uri_component(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

async fn chat_raw(session: &Session, method: &str, route: &str, body: Option<&Value>) -> Result<Reply> {
    let path = format!("/api/v1/chat{route}");
    match body {
        Some(b) => session.send_json(method, &path, b).await,
        None => Reply::from(session.request(method, &path).send().await?).await,
    }
}

async fn chat_call(session: &Session, method: &str, route: &str, body: Option<&Value>) -> Result<Value> {
    let reply = chat_raw(session, method, route, body).await?;
    ensure!((200..300).contains(&reply.status), "{method} {route}: {} {}", reply.status, reply.text());
    reply.json()
}

pub async fn chat_whatsapp_lifecycle(t: &Ctx) -> Result<()> {
    let root = t.temp("chat")?;
    let control = t.temp("chat-control")?;
    let bridge = instance::fixture_command(&control, "chat-bridge", &[("DAR_FAKE_CHAT_CONTROL", &control.to_string_lossy())])?;
    let config = InstanceConfig { chat_bridge: Some(bridge), ..InstanceConfig::default() };
    let runtime = Instance::isolated(&root, "dar-chat", config, true).await?;
    let base = runtime.base.clone();
    let db = runtime.database()?;

    session::setup_admin(&base).await?;
    let mut session = login(&base).await?;

    // The bridge says hello before Chat offers linking.
    let empty = until(
        Duration::from_secs(15),
        "the bridge",
        || chat_call(&session, "GET", "/overview", None),
        |o| o["bridge"]["available"] == json!(true),
    )
    .await?;
    assert_eq!(empty["accounts"], json!([]));

    // Link with a QR code; the fake "scans" when the test says so.
    let link_resp = chat_call(&session, "POST", "/accounts/link", Some(&json!({ "name": "Personal" }))).await?;
    let account_id = link_resp["accountId"].as_str().context("accountId")?.to_owned();
    let link_path = format!("/accounts/{account_id}/link");
    let qr = until(
        Duration::from_secs(15),
        "a QR code",
        || chat_call(&session, "GET", &link_path, None),
        |s| s["state"] == json!("qr"),
    )
    .await?;
    assert!(qr["qr"].as_str().unwrap_or_default().starts_with("2@fakeQrCode,"));
    assert!(qr["expiresAt"].as_i64().unwrap_or(0) > now_ms());
    let overview0 = chat_call(&session, "GET", "/overview", None).await?;
    assert_eq!(overview0["accounts"][0]["state"], json!("linking"));
    assert_eq!(overview0["accounts"][0]["name"], json!("Personal"));

    write_control(&control, &account_id, "pair", None)?;
    until(
        Duration::from_secs(15),
        "pairing",
        || chat_call(&session, "GET", &link_path, None),
        |s| s["state"] == json!("paired"),
    )
    .await?;
    let overview = until(
        Duration::from_secs(15),
        "the import",
        || chat_call(&session, "GET", "/overview", None),
        |o| {
            o["accounts"][0]["state"] == json!("connected")
                && o["accounts"][0]["importing"] == json!(false)
                && o["accounts"][0]["historyProgress"] == json!(100)
        },
    )
    .await?;
    let account = &overview["accounts"][0];
    assert_eq!(account["jid"], json!("15550001111@s.whatsapp.net"));
    assert_eq!(account["counts"], json!({ "chats": 2, "unread": 2, "groups": 1, "archived": 1 }));
    assert_eq!(account["unread"], json!(1), "a chat muted forever does not add to the account's unread count");

    // The fake checks lazy keys right after the import, so wait for its note.
    let log = until(
        Duration::from_secs(15),
        "the lazy key check",
        || async { Ok(read_bridge_log(&control)) },
        |entries| entries.iter().any(|e| e["method"] == json!("lazyCheck")),
    )
    .await?;
    assert!(log.iter().any(|e| e["method"] == json!("lazyCheck") && e["ok"] == json!(true)), "lazy keys round-trip through the runtime");

    // Keys are stored encrypted, bound to their account and key.
    let stored = sql_text(&db.sql("SELECT value FROM chat_keys").await?);
    assert!(!stored.contains(&dar_acceptance::base64(b"fake-device-secret")));
    assert!(!stored.contains("fake-device-secret"));
    assert!(!stored.contains("session-secret"));
    let scan_keys = sql_text(&db.sql("SELECT scan_key FROM chat_keys").await?);
    assert!(scan_keys.contains(&format!("{account_id}/l/evbuf/aa")));

    // Chat list: pinned first, then newest first; views filter.
    let chats = chat_call(&session, "GET", &format!("/threads?accountId={account_id}&view=chats"), None).await?;
    let pinned_names: Vec<&str> = chats["pinned"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    let thread_names: Vec<&str> = chats["threads"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(pinned_names, vec!["Sara Haddad"]);
    assert_eq!(thread_names, vec!["Family"]);
    let sara = chats["pinned"][0].clone();
    let family = chats["threads"][0].clone();
    let sara_id = sara["id"].as_str().unwrap().to_owned();
    let family_id = family["id"].as_str().unwrap().to_owned();
    assert_eq!(sara["preview"], json!("The place"));
    assert_eq!(sara["previewKind"], json!("image"));
    assert_eq!(sara["unread"], json!(1));
    assert_eq!(family["previewKind"], json!("voice"));
    assert_eq!(family["previewSender"], json!("Baba"));
    assert_eq!(family["mutedUntil"], json!(-1));

    let archived_names: Vec<String> = chat_call(&session, "GET", &format!("/threads?accountId={account_id}&view=archived"), None).await?["threads"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(archived_names, vec!["Old friend"]);
    let group_names: Vec<String> = chat_call(&session, "GET", &format!("/threads?accountId={account_id}&view=groups"), None).await?["threads"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(group_names, vec!["Family"]);
    let mut unread_names: Vec<String> = chat_call(&session, "GET", &format!("/threads?accountId={account_id}&view=unread"), None).await?["threads"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect();
    unread_names.sort();
    assert_eq!(unread_names, vec!["Family", "Sara Haddad"]);

    // A conversation reads oldest to newest, with quotes and statuses; opening it marks it read on WhatsApp.
    let messages_of = |session: &Session, thread_id: &str| {
        let session = session.clone();
        let url = format!("/messages?threadId={}", encode_uri_component(thread_id));
        async move { chat_call(&session, "GET", &url, None).await }
    };
    let mut page = messages_of(&session, &sara_id).await?;
    let ids: Vec<&str> = page["messages"].as_array().unwrap().iter().filter_map(|m| m["messageId"].as_str()).collect();
    assert_eq!(ids, vec!["H1", "H2", "H3"]);
    assert_eq!(page["messages"][1]["status"], json!("read"));
    assert_eq!(page["messages"][2]["quotedText"], json!("Yes!"));
    chat_call(&session, "POST", "/read", Some(&json!({ "threadId": sara_id }))).await?;
    assert_eq!(chat_call(&session, "GET", &format!("/threads?accountId={account_id}&view=chats"), None).await?["pinned"][0]["unread"], json!(0));
    let read_log = until(
        Duration::from_secs(15),
        "read receipts",
        || async { Ok(read_bridge_log(&control)) },
        |entries| entries.iter().any(|e| e["method"] == json!("markRead")),
    )
    .await?;
    assert_eq!(
        read_log.iter().find(|e| e["method"] == json!("markRead")),
        Some(&json!({
            "method": "markRead",
            "account": account_id,
            "chat": "15550002222@s.whatsapp.net",
            "items": [{ "id": "H3", "sender": "15550002222@s.whatsapp.net" }]
        }))
    );

    // Sending: the message is stored before it goes out, under the ID WhatsApp receives.
    let sent = chat_call(&session, "POST", "/send", Some(&json!({ "threadId": sara_id, "text": "See you at 7", "quotedId": "H1" }))).await?;
    let sent_id = sent["messageId"].as_str().context("sent messageId")?.to_owned();
    assert_eq!(sent["status"], json!("sent"));
    assert!(regex::Regex::new(r"^3EB0[0-9A-F]{18}$").unwrap().is_match(&sent_id));
    assert_eq!(sent["quotedText"], json!("Are we still on for dinner?"));
    let log = read_bridge_log(&control);
    let out = log.iter().find(|e| e["method"] == json!("send")).context("send in bridge log")?;
    assert_eq!(out["id"], json!(sent_id));
    assert_eq!(out["chat"], json!("15550002222@s.whatsapp.net"));
    assert_eq!(out["quotedId"], json!("H1"));
    assert_eq!(out["quotedSender"], json!("15550002222@s.whatsapp.net"));

    let failed = chat_call(&session, "POST", "/send", Some(&json!({ "threadId": sara_id, "text": "this will fail" }))).await?;
    let failed_id = failed["messageId"].as_str().unwrap().to_owned();
    assert_eq!(failed["status"], json!("failed"));
    assert_eq!(chat_call(&session, "POST", "/retry", Some(&json!({ "threadId": sara_id, "messageId": failed_id }))).await?["status"], json!("failed"));
    chat_call(&session, "POST", "/discard", Some(&json!({ "threadId": sara_id, "messageId": failed_id }))).await?;
    page = messages_of(&session, &sara_id).await?;
    let ids: Vec<&str> = page["messages"].as_array().unwrap().iter().filter_map(|m| m["messageId"].as_str()).collect();
    assert_eq!(ids, vec!["H1", "H2", "H3", sent_id.as_str()]);
    assert_eq!(chat_raw(&session, "POST", "/send", Some(&json!({ "threadId": sara_id, "text": "  " }))).await?.status, 400);

    // Receipts upgrade our messages; incoming messages count as unread; edits and deletions apply.
    write_control(&control, &account_id, "receipt", Some(&json!({ "chat": "15550002222@s.whatsapp.net", "ids": [sent_id], "status": "delivered", "fromMe": false })))?;
    write_control(
        &control,
        &account_id,
        "incoming",
        Some(&json!({
            "chats": [{ "id": "15550002222@s.whatsapp.net", "kind": "direct", "name": "Sara Haddad" }],
            "messages": [{ "id": "L1", "chat": "15550002222@s.whatsapp.net", "sender": "15550002222@s.whatsapp.net", "fromMe": false, "at": now_ms(), "kind": "text", "text": "Perfect, see you at 7 then" }]
        })),
    )?;
    let after = until(
        Duration::from_secs(15),
        "the receipt and incoming message",
        || messages_of(&session, &sara_id),
        |p| p["messages"].as_array().is_some_and(|m| m.len() == 5 && m[3]["status"] == json!("delivered")),
    )
    .await?;
    assert_eq!(after["thread"]["unread"], json!(1));
    assert_eq!(after["thread"]["preview"], json!("Perfect, see you at 7 then"));

    tokio::time::sleep(Duration::from_millis(300)).await;
    write_control(&control, &account_id, "receipt", Some(&json!({ "chat": "15550002222@s.whatsapp.net", "ids": [sent_id], "status": "read", "fromMe": false })))?;
    write_control(
        &control,
        &account_id,
        "incoming",
        Some(&json!({
            "messages": [
                { "id": "E1", "chat": "15550002222@s.whatsapp.net", "sender": "15550002222@s.whatsapp.net", "fromMe": false, "at": now_ms(), "kind": "edit", "target": "L1", "text": "Perfect, see you at 7:30 then" },
                { "id": "R1", "chat": "15550002222@s.whatsapp.net", "sender": "15550002222@s.whatsapp.net", "fromMe": false, "at": now_ms(), "kind": "revoke", "target": "H1" }
            ]
        })),
    )?;
    let changed = until(
        Duration::from_secs(15),
        "the edit and deletion",
        || messages_of(&session, &sara_id),
        |p| {
            p["messages"].as_array().is_some_and(|m| {
                m.len() >= 5 && m[3]["status"] == json!("read") && m[4]["edited"] == json!(true) && m[0]["deleted"] == json!(true)
            })
        },
    )
    .await?;
    assert_eq!(changed["messages"][4]["text"], json!("Perfect, see you at 7:30 then"));
    assert_eq!(changed["messages"][0]["text"], json!(""));
    assert_eq!(changed["messages"].as_array().unwrap().len(), 5, "edits and deletions are not messages");
    assert_eq!(changed["thread"]["preview"], json!("Perfect, see you at 7:30 then"));

    // Attachments download through the bridge on first use, once, into the blob store.
    let media_get = |session: &Session, thread_id: &str, message_id: &str, extra: &str| {
        let session = session.clone();
        let route = format!("/media?threadId={}&messageId={}{extra}", encode_uri_component(thread_id), encode_uri_component(message_id));
        async move { chat_raw(&session, "GET", &route, None).await }
    };
    let sara_msgs = messages_of(&session, &sara_id).await?;
    let photo = sara_msgs["messages"].as_array().unwrap().iter().find(|m| m["messageId"] == json!("H3")).context("H3")?;
    let photo_size = photo["media"]["size"].clone();
    assert_eq!(
        photo["media"],
        json!({ "kind": "image", "mime": "image/png", "size": photo_size, "seconds": 0, "width": 1200, "height": 900, "state": "remote" })
    );
    let resp = media_get(&session, &sara_id, "H3", "").await?;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("content-type"), Some("image/png"));
    assert_eq!(&resp.body[1..4], b"PNG");
    let h3_after = messages_of(&session, &sara_id).await?["messages"].as_array().unwrap().iter().find(|m| m["messageId"] == json!("H3")).cloned().unwrap();
    assert_eq!(h3_after["media"]["state"], json!("stored"));
    assert!(regex::Regex::new(r"[a-f0-9]{64}").unwrap().is_match(&sql_text(&db.sql("SELECT blob_id FROM chat_media").await?)));
    assert_eq!(media_get(&session, &sara_id, "H3", "").await?.status, 200);
    assert_eq!(
        read_bridge_log(&control).iter().filter(|e| e["method"] == json!("download") && e["id"] == json!("H3")).count(),
        1,
        "a stored attachment is not downloaded again"
    );

    // Documents download as attachments; HTML never renders inline.
    let resp = media_get(&session, &family_id, "D1", "&download=1").await?;
    assert_eq!(resp.status, 200);
    assert!(resp.header("content-disposition").unwrap_or_default().starts_with("attachment; filename*=UTF-8''Eid%20lunch%20menu.pdf"));
    let resp = media_get(&session, &family_id, "X1", "").await?;
    assert_eq!(resp.header("content-type"), Some("application/octet-stream"));
    assert!(resp.header("content-disposition").unwrap_or_default().starts_with("attachment"));

    // Expired media: the phone is asked to send it again, then it downloads.
    let old_id = chat_call(&session, "GET", &format!("/threads?accountId={account_id}&view=archived"), None).await?["threads"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let resp = media_get(&session, &old_id, "EXP1", "").await?;
    assert_eq!(resp.status, 409);
    assert!(resp.json()?["error"]["message"].as_str().unwrap_or_default().contains("asked to send it again"));
    let exp_msg = messages_of(&session, &old_id).await?["messages"].as_array().unwrap().iter().find(|m| m["messageId"] == json!("EXP1")).cloned().unwrap();
    assert_eq!(exp_msg["media"]["state"], json!("retrying"));
    write_control(&control, &account_id, "resend", Some(&json!({ "chat": "15550003333@s.whatsapp.net", "id": "EXP1" })))?;
    until(
        Duration::from_secs(15),
        "the resent attachment",
        || messages_of(&session, &old_id),
        |p| p["messages"].as_array().unwrap().iter().any(|m| m["messageId"] == json!("EXP1") && m["media"]["state"] == json!("stored")),
    )
    .await?;

    // Save to Files; a second copy gets a numbered name.
    let saved = chat_call(&session, "POST", "/save", Some(&json!({ "threadId": family_id, "messageId": "D1", "folderId": "" }))).await?;
    assert_eq!(saved["name"], json!("Eid lunch menu.pdf"));
    let saved2 = chat_call(&session, "POST", "/save", Some(&json!({ "threadId": family_id, "messageId": "D1", "folderId": "" }))).await?;
    assert_eq!(saved2["name"], json!("Eid lunch menu (2).pdf"));
    let saved_file_id = saved["fileId"].as_str().unwrap();
    assert_eq!(session.get(&format!("/api/v1/files/{saved_file_id}/content")).await?.text(), "%PDF-1.4 fake menu");

    // New voice notes download as they arrive; deleting a message for everyone removes its attachment.
    write_control(
        &control,
        &account_id,
        "incoming",
        Some(&json!({
            "messages": [{ "id": "V1", "chat": "120363000000000001@g.us", "sender": "15550005555@s.whatsapp.net", "senderName": "Baba", "fromMe": false, "at": now_ms(), "kind": "voice", "media": { "mime": "audio/ogg", "size": 20, "seconds": 3 } }]
        })),
    )?;
    until(
        Duration::from_secs(15),
        "the automatic voice note download",
        || messages_of(&session, &family_id),
        |p| p["messages"].as_array().unwrap().iter().any(|m| m["messageId"] == json!("V1") && m["media"]["state"] == json!("stored")),
    )
    .await?;
    write_control(
        &control,
        &account_id,
        "incoming",
        Some(&json!({
            "messages": [{ "id": "R2", "chat": "120363000000000001@g.us", "sender": "15550004444@s.whatsapp.net", "fromMe": false, "at": now_ms(), "kind": "revoke", "target": "X1" }]
        })),
    )?;
    until(
        Duration::from_secs(15),
        "the deletion",
        || messages_of(&session, &family_id),
        |p| p["messages"].as_array().unwrap().iter().any(|m| m["messageId"] == json!("X1") && m["deleted"] == json!(true)),
    )
    .await?;
    let x1 = messages_of(&session, &family_id).await?["messages"].as_array().unwrap().iter().find(|m| m["messageId"] == json!("X1")).cloned().unwrap();
    assert_eq!(x1["media"], Value::Null);
    assert!(!sql_text(&db.sql("SELECT id FROM chat_media").await?).contains("/X1"));

    // Reactions: from others as they arrive, and the account's own through the bridge; an empty one removes it.
    write_control(
        &control,
        &account_id,
        "incoming",
        Some(&json!({
            "messages": [{ "id": "RX1", "chat": "15550002222@s.whatsapp.net", "sender": "15550002222@s.whatsapp.net", "senderName": "Sara Haddad", "fromMe": false, "at": now_ms(), "kind": "reaction", "target": sent_id, "text": "❤️" }]
        })),
    )?;
    let reacted = until(
        Duration::from_secs(15),
        "the incoming reaction",
        || messages_of(&session, &sara_id),
        |p| p["messages"].as_array().unwrap().iter().any(|m| m["messageId"] == json!(sent_id) && m["reactions"].as_array().is_some_and(|r| r.len() == 1)),
    )
    .await?;
    let sent_after = reacted["messages"].as_array().unwrap().iter().find(|m| m["messageId"] == json!(sent_id)).unwrap();
    assert_eq!(sent_after["reactions"], json!([{ "emoji": "❤️", "count": 1, "mine": false, "names": ["Sara Haddad"] }]));
    chat_call(&session, "POST", "/react", Some(&json!({ "threadId": sara_id, "messageId": "L1", "emoji": "👍" }))).await?;
    let l1_after = messages_of(&session, &sara_id).await?["messages"].as_array().unwrap().iter().find(|m| m["messageId"] == json!("L1")).cloned().unwrap();
    assert_eq!(l1_after["reactions"], json!([{ "emoji": "👍", "count": 1, "mine": true, "names": [] }]));
    let log = read_bridge_log(&control);
    let reaction = log.iter().find(|e| e["method"] == json!("react")).context("react in log")?;
    assert_eq!(reaction["sender"], json!("15550002222@s.whatsapp.net"));
    assert_eq!(reaction["id"], json!("L1"));
    assert_eq!(reaction["emoji"], json!("👍"));
    chat_call(&session, "POST", "/react", Some(&json!({ "threadId": sara_id, "messageId": "L1", "emoji": "" }))).await?;
    let l1_after = messages_of(&session, &sara_id).await?["messages"].as_array().unwrap().iter().find(|m| m["messageId"] == json!("L1")).cloned().unwrap();
    assert_eq!(l1_after["reactions"], json!([]));

    // Edits and deletion for everyone apply to the account's own messages only; "delete from Dar" is local.
    chat_call(&session, "POST", "/edit", Some(&json!({ "threadId": sara_id, "messageId": sent_id, "text": "See you at 7:30" }))).await?;
    let own = messages_of(&session, &sara_id).await?["messages"].as_array().unwrap().iter().find(|m| m["messageId"] == json!(sent_id)).cloned().unwrap();
    assert_eq!(own["text"], json!("See you at 7:30"));
    assert_eq!(own["edited"], json!(true));
    assert_eq!(chat_raw(&session, "POST", "/edit", Some(&json!({ "threadId": sara_id, "messageId": "L1", "text": "Not mine" }))).await?.status, 403);
    chat_call(&session, "POST", "/revoke", Some(&json!({ "threadId": sara_id, "messageId": sent_id }))).await?;
    let own = messages_of(&session, &sara_id).await?["messages"].as_array().unwrap().iter().find(|m| m["messageId"] == json!(sent_id)).cloned().unwrap();
    assert_eq!(own["deleted"], json!(true));
    assert_eq!(own["reactions"], json!([]));
    assert_eq!(read_bridge_log(&control).iter().find(|e| e["method"] == json!("revoke")).unwrap()["sender"], json!("15550001111@s.whatsapp.net"));
    chat_call(&session, "POST", "/deleteForMe", Some(&json!({ "threadId": sara_id, "messageId": "H2" }))).await?;
    assert!(!messages_of(&session, &sara_id).await?["messages"].as_array().unwrap().iter().any(|m| m["messageId"] == json!("H2")));

    // Mute, pin and archive go to WhatsApp, and the list follows.
    chat_call(&session, "POST", "/chatSetting", Some(&json!({ "threadId": sara_id, "mutedUntil": -1 }))).await?;
    let sara_pinned = chat_call(&session, "GET", &format!("/threads?accountId={account_id}&view=chats"), None).await?["pinned"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(sara_id))
        .cloned()
        .unwrap();
    assert_eq!(sara_pinned["mutedUntil"], json!(-1));
    chat_call(&session, "POST", "/chatSetting", Some(&json!({ "threadId": sara_id, "archived": true }))).await?;
    let archived = chat_call(&session, "GET", &format!("/threads?accountId={account_id}&view=archived"), None).await?["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(sara_id))
        .cloned()
        .unwrap();
    assert_eq!(archived["pinned"], json!(false), "archiving unpins");
    let log = read_bridge_log(&control);
    let last_setting = log.iter().filter(|e| e["method"] == json!("chatSetting")).last().context("chatSetting in log")?;
    assert_eq!(last_setting["archived"], json!(true));
    assert_eq!(last_setting["chat"], json!("15550002222@s.whatsapp.net"));
    assert!(last_setting["lastId"].as_str().is_some_and(|s| !s.is_empty()));
    chat_call(&session, "POST", "/chatSetting", Some(&json!({ "threadId": sara_id, "archived": false }))).await?;
    chat_call(&session, "POST", "/chatSetting", Some(&json!({ "threadId": sara_id, "pinned": true }))).await?;
    chat_call(&session, "POST", "/chatSetting", Some(&json!({ "threadId": sara_id, "mutedUntil": 0 }))).await?;
    assert_eq!(chat_raw(&session, "POST", "/chatSetting", Some(&json!({ "threadId": sara_id, "mutedUntil": -5 }))).await?.status, 400);

    // Chat info: groups list their members from WhatsApp.
    let group_info = chat_call(&session, "GET", &format!("/info?threadId={}", encode_uri_component(&family_id)), None).await?;
    let member_names: Vec<&str> = group_info["group"]["members"].as_array().unwrap().iter().filter_map(|m| m["name"].as_str()).collect();
    assert_eq!(member_names, vec!["Me", "Mama", "Baba"]);
    assert_eq!(group_info["group"]["topic"], json!("Eid plans"));
    assert!(group_info["media"].as_array().unwrap().iter().any(|m| m["messageId"] == json!("D1")));
    assert_eq!(chat_call(&session, "GET", &format!("/info?threadId={}", encode_uri_component(&sara_id)), None).await?["group"], Value::Null);

    // Dar contacts name chats and group senders; nothing about them is sent to WhatsApp.
    let books_resp = session.get_json("/api/v1/contacts/books").await?;
    let book_id = if let Some(first) = books_resp["items"].as_array().and_then(|a| a.first()) {
        first["id"].as_str().unwrap().to_owned()
    } else {
        session
            .post("/api/v1/contacts/books", &json!({ "name": "Contacts", "color": "#2563eb" }))
            .await?
            .json()?["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let add_contact = |display_name: &'static str, phone: &'static str| {
        let session = session.clone();
        let book_id = book_id.clone();
        async move {
            let reply = Reply::from(
                session
                    .request("POST", &format!("/api/v1/contacts/books/{book_id}/items"))
                    .header("if-none-match", "*")
                    .json(&json!({ "data": { "displayName": display_name, "phones": [{ "value": phone, "label": "cell" }] } }))
                    .send()
                    .await?,
            )
            .await?
            .expect(201)?;
            reply.json()
        }
    };
    let sara_card = add_contact("Sara H. (Dar)", "+1 555 000 2222").await?;
    add_contact("Mom", "555-000-4444").await?;
    let named = chat_call(&session, "GET", &format!("/threads?accountId={account_id}&view=chats"), None).await?["pinned"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(sara_id))
        .cloned()
        .unwrap();
    assert_eq!(named["name"], json!("Sara H. (Dar)"));
    assert_eq!(named["whatsappName"], json!("Sara Haddad"));
    assert_eq!(named["contact"], json!({ "name": "Sara H. (Dar)", "book": book_id, "href": sara_card["href"] }));
    let d1 = messages_of(&session, &family_id).await?["messages"].as_array().unwrap().iter().find(|m| m["messageId"] == json!("D1")).cloned().unwrap();
    assert_eq!(d1["senderName"], json!("Mom"), "a national number takes the account's country");
    let fam_members: Vec<String> = chat_call(&session, "GET", &format!("/info?threadId={}", encode_uri_component(&family_id)), None).await?["group"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["name"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(fam_members, vec!["Me", "Mom", "Baba"]);
    let found = chat_call(&session, "GET", &format!("/contacts?accountId={account_id}&q=mom"), None).await?;
    let pairs: Vec<(&str, &str)> = found["items"].as_array().unwrap().iter().map(|c| (c["name"].as_str().unwrap(), c["phone"].as_str().unwrap())).collect();
    assert_eq!(pairs, vec![("Mom", "+15550004444")]);
    assert_eq!(chat_call(&session, "GET", &format!("/contacts?accountId={account_id}&q=4444"), None).await?["items"].as_array().unwrap().len(), 1, "search matches numbers too");
    assert!(
        !read_bridge_log(&control)
            .iter()
            .any(|e| e["method"].as_str().unwrap_or_default().to_ascii_lowercase().contains("contact")),
        "contacts never reach the bridge"
    );

    // The shared search finds chats by name and messages by text, opening the chat at the message.
    let search_hit = session.get_json(&format!("/api/v1/search?q={}", encode_uri_component("7:30 then"))).await?;
    let hits: Vec<&Value> = search_hit["items"].as_array().unwrap().iter().filter(|i| i["group"] == json!("Chat")).collect();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["title"], json!("Sara H. (Dar)"));
    assert!(hits[0]["detail"].as_str().unwrap_or_default().contains("Perfect, see you at 7:30 then"));
    assert_eq!(
        hits[0]["href"],
        json!(format!("/chat?account={}&chat={}&message=L1", encode_uri_component(&account_id), encode_uri_component(&sara_id)))
    );
    let fam_search = session.get_json("/api/v1/search?q=Family").await?;
    let fam_suffix = format!("chat={}", encode_uri_component(&family_id));
    assert!(fam_search["items"].as_array().unwrap().iter().any(|i| i["group"] == json!("Chat") && i["href"].as_str().unwrap_or_default().ends_with(&fam_suffix)));
    let arch_search = session.get_json(&format!("/api/v1/search?q={}", encode_uri_component("Long time"))).await?;
    assert_eq!(arch_search["items"].as_array().unwrap().iter().filter(|i| i["group"] == json!("Chat")).count(), 1, "archived chats are searched too");

    // Earlier messages from the phone: asked from the oldest one here, they arrive as history.
    chat_call(&session, "POST", "/history", Some(&json!({ "threadId": sara_id }))).await?;
    let asked = read_bridge_log(&control).into_iter().find(|e| e["method"] == json!("requestHistory")).context("requestHistory")?;
    assert_eq!(asked["chat"], json!("15550002222@s.whatsapp.net"));
    until(
        Duration::from_secs(15),
        "the earlier message from the phone",
        || messages_of(&session, &sara_id),
        |p| p["messages"][0]["messageId"] == json!("H0"),
    )
    .await?;

    // A new chat by phone number.
    let direct = chat_call(&session, "POST", "/chats", Some(&json!({ "accountId": account_id, "phone": "+1 555 000 9999" }))).await?;
    assert_eq!(direct["jid"], json!("15550009999@s.whatsapp.net"));
    assert_eq!(direct["name"], json!("+15550009999"));
    assert_eq!(chat_raw(&session, "POST", "/chats", Some(&json!({ "accountId": account_id, "phone": "0555" }))).await?.status, 400);

    // After a restart the bridge reconnects from the stored, encrypted keys.
    runtime.restart().await?;
    session = login(&base).await?;
    until(
        Duration::from_secs(20),
        "reconnecting after a restart",
        || chat_call(&session, "GET", "/overview", None),
        |o| o["bridge"]["available"] == json!(true) && o["accounts"][0]["state"] == json!("connected"),
    )
    .await?;
    let log = read_bridge_log(&control);
    let reconnect = log.iter().find(|e| e["method"] == json!("connect")).context("connect after restart")?;
    assert_eq!(reconnect["device"], json!("fake-device-secret"));
    assert_eq!(reconnect["keys"], json!(["device", "session/1555:0"]), "lazy keys are not loaded at start");
    assert!(log.iter().any(|e| e["method"] == json!("lazyAfterRestart") && e["ok"] == json!(true)));

    // Another person sees none of it.
    session
        .post("/api/v1/auth/users", &json!({ "username": "other", "displayName": "Other", "password": PASSWORD }))
        .await?
        .expect(201)?;
    {
        let other = login_as(&base, "other", PASSWORD).await?;
        assert_eq!(chat_call(&other, "GET", "/overview", None).await?["accounts"], json!([]));
        assert_eq!(chat_raw(&other, "GET", &format!("/threads?accountId={account_id}&view=chats"), None).await?.status, 404);
        assert_eq!(chat_raw(&other, "GET", &format!("/messages?threadId={}", encode_uri_component(&sara_id)), None).await?.status, 404);
        assert_eq!(chat_raw(&other, "POST", "/send", Some(&json!({ "threadId": sara_id, "text": "hi" }))).await?.status, 404);
        assert_eq!(chat_raw(&other, "DELETE", &format!("/accounts/{account_id}"), None).await?.status, 404);
    }

    // Backups carry chats, messages and attachments but no keys; a restored account comes back unlinked.
    let pin = uuid();
    let snapshot = db.call("service_backup", json!([pin])).await?;
    db.call("service_release_backup", json!([pin])).await?;
    assert_eq!(snapshot["format"], json!("dar-backup-v12"));
    assert_eq!(snapshot["chat"]["accounts"].as_array().unwrap().len(), 1);
    assert!(snapshot["chat"]["messages"].as_array().unwrap().len() >= 9);
    let kept: Vec<String> = snapshot["chat"]["media"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["blobId"].as_str().filter(|s| !s.is_empty()).map(str::to_owned))
        .collect();
    assert!(kept.len() >= 3);
    let blobs = snapshot["blobs"].as_array().unwrap();
    for id in &kept {
        assert!(blobs.iter().any(|b| b["id"].as_str() == Some(id.as_str())), "attachment blobs are in the backup");
    }
    let snapshot_text = snapshot.to_string();
    assert!(!snapshot_text.contains("fake-device-secret"));
    assert!(!snapshot_text.contains("ZmFrZS1kZXZpY2Ut"));
    let second_name = format!("{}-restore", runtime.config.database_name);
    instance::publish_module(&second_name, &base)?;
    let restored_db = Database::new(&db.url, &second_name, &db.token_file)?;
    restored_db.call("service_restore", json!([snapshot_text])).await?;
    assert!(regex::Regex::new(r"loggedOut.*restored").unwrap().is_match(&sql_text(&restored_db.sql("SELECT state, detail FROM chat_accounts").await?)));
    assert!(!sql_text(&restored_db.sql("SELECT id FROM chat_keys").await?).contains(&account_id));
    assert!(sql_text(&restored_db.sql("SELECT blob_id FROM chat_media").await?).contains(&kept[0]));

    // Unlinking keeps history, read-only, and removes the keys.
    let kept_messages = messages_of(&session, &sara_id).await?["messages"].as_array().unwrap().len();
    let unlinked = chat_call(&session, "POST", &format!("/accounts/{account_id}/unlink"), Some(&json!({}))).await?;
    assert_eq!(unlinked["confirmed"], json!(true));
    assert_eq!(chat_call(&session, "GET", "/overview", None).await?["accounts"][0]["state"], json!("loggedOut"));
    assert_eq!(messages_of(&session, &sara_id).await?["messages"].as_array().unwrap().len(), kept_messages);
    assert_eq!(chat_raw(&session, "POST", "/send", Some(&json!({ "threadId": sara_id, "text": "hello?" }))).await?.status, 409);
    assert!(!sql_text(&db.sql("SELECT id FROM chat_keys").await?).contains(&account_id));

    // Disabling Chat hides it; deleting the account removes everything.
    let settings = session.get_json("/api/v1/admin/installedApps").await?;
    let rev = settings["revision"].as_i64().context("installedApps revision")?;
    let disabled = Reply::from(
        session
            .request("PUT", "/api/v1/admin/installedApps")
            .header("if-match", format!("\"apps:{rev}\""))
            .json(&json!({ "id": "chat", "enabled": false, "revision": rev }))
            .send()
            .await?,
    )
    .await?
    .expect(200)?;
    assert_eq!(chat_raw(&session, "GET", "/overview", None).await?.status, 404);
    let rev2 = disabled.json()?["revision"].as_i64().context("disabled revision")?;
    Reply::from(
        session
            .request("PUT", "/api/v1/admin/installedApps")
            .header("if-match", format!("\"apps:{rev2}\""))
            .json(&json!({ "id": "chat", "enabled": true, "revision": rev2 }))
            .send()
            .await?,
    )
    .await?
    .expect(200)?;

    chat_call(&session, "DELETE", &format!("/accounts/{account_id}"), None).await?;
    assert_eq!(chat_call(&session, "GET", "/overview", None).await?["accounts"], json!([]));
    assert!(!sql_text(&db.sql("SELECT id FROM chat_threads").await?).contains(&account_id));
    assert!(!sql_text(&db.sql("SELECT id FROM chat_messages").await?).contains(&account_id));

    let linked_again = chat_call(&session, "POST", "/accounts/link", Some(&json!({ "name": "Work", "phone": "+966551230142" }))).await?;
    let again_id = linked_again["accountId"].as_str().unwrap().to_owned();
    let again_link_path = format!("/accounts/{again_id}/link");
    let code = until(
        Duration::from_secs(15),
        "a pairing code",
        || chat_call(&session, "GET", &again_link_path, None),
        |s| s["state"] == json!("code"),
    )
    .await?;
    assert_eq!(code["code"], json!("K7QM2X9D"));
    chat_call(&session, "DELETE", &format!("/accounts/{again_id}/link"), None).await?;
    assert_eq!(chat_call(&session, "GET", "/overview", None).await?["accounts"], json!([]), "a cancelled link leaves no account behind");

    runtime.close().await;
    Ok(())
}
