//! Port of `test/chat-discord.integration.test.ts`: QR sign-in, servers and channels, DMs,
//! send with nonce, reactions, live changes, history, mute, voice, restart, backups and sign-out.

use crate::suites::comms_support::{WsClient, read_bridge_log, sql_text, until, write_control};
use dar_acceptance::prelude::*;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SUITE: Suite = Suite { name: "chat-discord", cases: &[case!(chat_discord_lifecycle, 240)] };

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

pub async fn chat_discord_lifecycle(t: &Ctx) -> Result<()> {
    let root = t.temp("chat-discord")?;
    let control = t.temp("chat-discord-control")?;
    let bridge = instance::fixture_command(&control, "chat-bridge", &[("DAR_FAKE_CHAT_CONTROL", &control.to_string_lossy())])?;
    let config = InstanceConfig { chat_bridge: Some(bridge), ..InstanceConfig::default() };
    let runtime = Instance::isolated(&root, "dar-discord", config, true).await?;
    let base = runtime.base.clone();
    let port = runtime.port;
    let db = runtime.database()?;

    session::setup_admin(&base).await?;
    let mut session = login(&base).await?;

    until(
        Duration::from_secs(15),
        "the bridge",
        || chat_call(&session, "GET", "/overview", None),
        |o| o["bridge"]["available"] == json!(true),
    )
    .await?;

    // QR sign-in: Discord's code, the scan, then the phone's confirmation.
    let account_id = chat_call(&session, "POST", "/accounts/link", Some(&json!({ "platform": "discord" }))).await?["accountId"]
        .as_str()
        .context("accountId")?
        .to_owned();
    let link_path = format!("/accounts/{account_id}/link");
    let qr = until(
        Duration::from_secs(15),
        "a Discord QR code",
        || chat_call(&session, "GET", &link_path, None),
        |s| s["state"] == json!("qr"),
    )
    .await?;
    assert!(qr["qr"].as_str().unwrap_or_default().starts_with("https://discord.com/ra/"));
    assert!(qr["expiresAt"].as_i64().unwrap_or(0) - now_ms() > 60_000);
    let account0 = &chat_call(&session, "GET", "/overview", None).await?["accounts"][0];
    assert_eq!(account0["platform"], json!("discord"));
    assert_eq!(account0["name"], json!("Discord"));

    write_control(&control, &account_id, "scan", None)?;
    let scanned = until(
        Duration::from_secs(15),
        "the scan",
        || chat_call(&session, "GET", &link_path, None),
        |s| s["state"] == json!("scanned"),
    )
    .await?;
    assert_eq!(scanned["name"], json!("anas"));
    write_control(&control, &account_id, "pair", None)?;
    until(
        Duration::from_secs(15),
        "the sign-in",
        || chat_call(&session, "GET", &link_path, None),
        |s| s["state"] == json!("paired"),
    )
    .await?;
    let overview = until(
        Duration::from_secs(15),
        "connected",
        || chat_call(&session, "GET", "/overview", None),
        |o| o["accounts"][0]["state"] == json!("connected"),
    )
    .await?;
    assert_eq!(overview["accounts"][0]["jid"], json!("100"));

    // The token is stored encrypted.
    let key_vals = sql_text(&db.sql("SELECT value FROM chat_keys").await?);
    assert!(!key_vals.contains("fake-discord-token"));
    assert!(!key_vals.contains(&dar_acceptance::base64(b"fake-discord-token")));
    assert!(sql_text(&db.sql("SELECT scan_key FROM chat_keys").await?).contains(&format!("{account_id}/c/discord/token")));

    // Servers with their channels in Discord's order; server channels stay out of the DM list.
    let fetch_servers = |session: &Session| {
        let session = session.clone();
        let url = format!("/servers?accountId={account_id}");
        async move { Ok(chat_call(&session, "GET", &url, None).await?["servers"].as_array().cloned().unwrap_or_default()) }
    };
    let mut tree = until(Duration::from_secs(15), "servers", || fetch_servers(&session), |list| list.len() == 2).await?;
    let server_names: Vec<&str> = tree.iter().filter_map(|s| s["name"].as_str()).collect();
    assert_eq!(server_names, vec!["Homelab Club", "Kernel newbies"]);
    let ch_names: Vec<&str> = tree[0]["channels"].as_array().unwrap().iter().filter_map(|c| c["name"].as_str()).collect();
    assert_eq!(ch_names, vec!["general", "nixos", "off-topic", "Lounge"]);
    let general = tree[0]["channels"][0].clone();
    let lounge = tree[0]["channels"][3].clone();
    let general_id = general["id"].as_str().unwrap().to_owned();
    let lounge_id = lounge["id"].as_str().unwrap().to_owned();
    assert_eq!(general["section"], json!("Text channels"));
    assert_eq!(general["topic"], json!("Say hello"));
    assert_eq!(general["mentions"], json!(1));
    assert_eq!(lounge["kind"], json!("voice"));
    assert_eq!(tree[0]["mentions"], json!(1));
    assert_eq!(tree[1]["mutedUntil"], json!(-1));

    let dms = chat_call(&session, "GET", &format!("/threads?accountId={account_id}&view=chats"), None).await?;
    let dm_names: Vec<&str> = dms["threads"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(dm_names, vec!["Lina K"]);
    let account = &chat_call(&session, "GET", "/overview", None).await?["accounts"][0];
    assert_eq!(account["counts"]["chats"], json!(1));
    assert_eq!(account["unread"], json!(2), "an unread DM and a channel that mentions the account");

    // Messages: Discord reactions are counts, the account's own marked.
    let messages_of = |session: &Session, thread_id: &str| {
        let session = session.clone();
        let url = format!("/messages?threadId={}", encode_uri_component(thread_id));
        async move { chat_call(&session, "GET", &url, None).await }
    };
    let mut page = messages_of(&session, &general_id).await?;
    let labels: Vec<&str> = page["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let t = m["text"].as_str().unwrap_or_default();
            if !t.is_empty() { t } else { m["fileName"].as_str().unwrap_or_default() }
        })
        .collect();
    assert_eq!(labels, vec!["Anyone moved to the new caddy module?", "Yes, painless", "rack.png"]);
    let painless = page["messages"][1].clone();
    let painless_id = painless["messageId"].as_str().unwrap().to_owned();
    let rx: Vec<(&str, i64, bool)> = painless["reactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["emoji"].as_str().unwrap(), r["count"].as_i64().unwrap(), r["mine"].as_bool().unwrap()))
        .collect();
    assert_eq!(rx, vec![("👍", 3, true), ("party:55", 1, false)]);

    // Several reactions per person; removing one keeps the others.
    chat_call(&session, "POST", "/react", Some(&json!({ "threadId": general_id, "messageId": painless_id, "emoji": "👍", "remove": true }))).await?;
    chat_call(&session, "POST", "/react", Some(&json!({ "threadId": general_id, "messageId": painless_id, "emoji": "🎉" }))).await?;
    page = messages_of(&session, &general_id).await?;
    let mut rx_after: Vec<(String, i64, bool)> = page["messages"][1]["reactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["emoji"].as_str().unwrap().to_owned(), r["count"].as_i64().unwrap(), r["mine"].as_bool().unwrap()))
        .collect();
    rx_after.sort();
    let mut expected_rx = vec![("party:55".to_owned(), 1, false), ("👍".to_owned(), 2, false), ("🎉".to_owned(), 1, true)];
    expected_rx.sort();
    assert_eq!(rx_after, expected_rx);
    let reacted: Vec<(String, bool, String, String)> = read_bridge_log(&control)
        .into_iter()
        .filter(|e| e["method"] == json!("react"))
        .map(|e| {
            (
                e["emoji"].as_str().unwrap_or_default().to_owned(),
                e["remove"].as_bool().unwrap_or(false),
                e["chat"].as_str().unwrap_or_default().to_owned(),
                e["id"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        reacted,
        vec![
            ("👍".to_owned(), true, "911".to_owned(), painless_id.clone()),
            ("🎉".to_owned(), false, "911".to_owned(), painless_id.clone()),
        ]
    );
    assert_eq!(chat_raw(&session, "POST", "/react", Some(&json!({ "threadId": general_id, "messageId": painless_id, "emoji": "" }))).await?.status, 400);

    // The attachment downloads under its file name.
    let media = chat_raw(&session, "GET", &format!("/media?threadId={}&messageId=1300000000000000013&download=1", encode_uri_component(&general_id)), None).await?;
    assert_eq!(media.status, 200);
    assert!(media.header("content-disposition").unwrap_or_default().contains("rack.png"));
    assert!(!media.body.is_empty());

    // Sending: the pending copy is stored under a nonce and moves to Discord's ID, whichever arrives first.
    let sent = chat_call(&session, "POST", "/send", Some(&json!({ "threadId": general_id, "text": "hello there", "quotedId": painless_id }))).await?;
    let log = read_bridge_log(&control);
    let sent_log = log.iter().filter(|e| e["method"] == json!("send")).last().context("send log")?;
    let nonce_str = sent_log["id"].as_str().unwrap();
    assert!(regex::Regex::new(r"^\d{17,20}$").unwrap().is_match(nonce_str));
    assert_eq!(sent_log["chat"], json!("911"));
    assert_eq!(sent_log["quotedId"], json!(painless_id));
    let expected_id = (nonce_str.parse::<u128>().unwrap() + 7).to_string();
    let sent_id = sent["messageId"].as_str().unwrap().to_owned();
    assert_eq!(sent_id, expected_id);
    assert_eq!(sent["status"], json!("sent"));

    chat_call(&session, "POST", "/send", Some(&json!({ "threadId": general_id, "text": "echo first" }))).await?;
    page = messages_of(&session, &general_id).await?;
    let msgs = page["messages"].as_array().unwrap();
    assert_eq!(msgs.iter().filter(|m| m["text"] == json!("echo first")).count(), 1, "the gateway's copy and the answer make one message");
    assert_eq!(msgs.iter().filter(|m| m["text"] == json!("hello there")).count(), 1);
    assert!(msgs.iter().all(|m| m["status"] != json!("pending")));
    assert_eq!(chat_raw(&session, "POST", "/send", Some(&json!({ "threadId": general_id, "text": "x".repeat(4001) }))).await?.status, 400);
    let failed = chat_call(&session, "POST", "/send", Some(&json!({ "threadId": general_id, "text": "this will fail" }))).await?;
    assert_eq!(failed["status"], json!("failed"));
    chat_call(&session, "POST", "/discard", Some(&json!({ "threadId": general_id, "messageId": failed["messageId"] }))).await?;

    // Editing has no time limit on Discord; deleting removes it for everyone.
    chat_call(&session, "POST", "/edit", Some(&json!({ "threadId": general_id, "messageId": sent_id, "text": "hello again" }))).await?;
    chat_call(&session, "POST", "/revoke", Some(&json!({ "threadId": general_id, "messageId": sent_id }))).await?;
    page = messages_of(&session, &general_id).await?;
    assert_eq!(page["messages"].as_array().unwrap().iter().find(|m| m["messageId"] == json!(sent_id)).unwrap()["deleted"], json!(true));
    assert!(read_bridge_log(&control).iter().any(|e| e["method"] == json!("edit") && e["text"] == json!("hello again") && e["chat"] == json!("911")));

    // Live: a mention adds to the channel's mentions; edits, deletions and reactions from others follow.
    let at = now_ms();
    write_control(
        &control,
        &account_id,
        "ingest",
        Some(&json!({
            "kind": "messages",
            "messages": [
                { "id": "1300000000000000101", "chat": "912", "sender": "200", "senderName": "lina.k", "fromMe": false, "at": at, "kind": "text", "text": "@Anas can you look?", "mention": true },
                { "id": "1300000000000000102", "chat": "912", "sender": "300", "senderName": "tarek", "fromMe": false, "at": at + 1, "kind": "text", "text": "typo hree" }
            ]
        })),
    )?;
    tree = until(Duration::from_secs(15), "a mention", || fetch_servers(&session), |list| list[0]["channels"][1]["mentions"] == json!(1)).await?;
    let nixos = tree[0]["channels"][1].clone();
    let nixos_id = nixos["id"].as_str().unwrap().to_owned();
    assert_eq!(nixos["unread"], json!(2));
    assert_eq!(nixos["preview"], json!("typo hree"));
    write_control(
        &control,
        &account_id,
        "ingest",
        Some(&json!({
            "kind": "messages",
            "messages": [
                { "id": "1300000000000000102-edit", "chat": "912", "sender": "300", "fromMe": false, "at": at + 2, "kind": "edit", "text": "typo here", "target": "1300000000000000102" },
                { "id": "1300000000000000101-delete", "chat": "912", "fromMe": false, "at": at + 3, "kind": "revoke", "target": "1300000000000000101" }
            ]
        })),
    )?;
    until(
        Duration::from_secs(15),
        "the edit and the deletion",
        || messages_of(&session, &nixos_id),
        |p| {
            let ms = p["messages"].as_array().unwrap();
            ms.iter().any(|m| m["text"] == json!("typo here")) && ms.iter().any(|m| m["deleted"] == json!(true))
        },
    )
    .await?;
    write_control(
        &control,
        &account_id,
        "ingest",
        Some(&json!({
            "kind": "reactions",
            "items": [
                { "chat": "912", "id": "1300000000000000102", "emoji": "😂", "me": false, "delta": 1 },
                { "chat": "912", "id": "1300000000000000102", "emoji": "😂", "me": false, "delta": 1 }
            ]
        })),
    )?;
    until(
        Duration::from_secs(15),
        "reactions from others",
        || messages_of(&session, &nixos_id),
        |p| {
            p["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["messageId"] == json!("1300000000000000102"))
                .is_some_and(|m| m["reactions"][0]["count"] == json!(2))
        },
    )
    .await?;
    // Read on another device clears it here too.
    write_control(&control, &account_id, "ingest", Some(&json!({ "kind": "read", "chat": "912" })))?;
    until(
        Duration::from_secs(15),
        "read elsewhere",
        || fetch_servers(&session),
        |list| list[0]["channels"][1]["unread"] == json!(0) && list[0]["channels"][1]["mentions"] == json!(0),
    )
    .await?;

    // Opening a channel marks it read at the newest message.
    write_control(
        &control,
        &account_id,
        "ingest",
        Some(&json!({
            "kind": "messages",
            "messages": [{ "id": "1300000000000000103", "chat": "911", "sender": "200", "senderName": "lina.k", "fromMe": false, "at": now_ms(), "kind": "text", "text": "ping <@100>", "mention": true }]
        })),
    )?;
    until(Duration::from_secs(15), "a new mention in general", || fetch_servers(&session), |list| list[0]["channels"][0]["mentions"] == json!(1)).await?;
    chat_call(&session, "POST", "/read", Some(&json!({ "threadId": general_id }))).await?;
    let read_log = until(
        Duration::from_secs(15),
        "the read mark",
        || async { Ok(read_bridge_log(&control)) },
        |entries| entries.iter().any(|e| e["method"] == json!("markRead")),
    )
    .await?;
    let mark = read_log.iter().find(|e| e["method"] == json!("markRead")).unwrap();
    assert_eq!(mark["chat"], json!("911"));
    assert!(regex::Regex::new(r"^\d+$").unwrap().is_match(mark["last"].as_str().unwrap_or_default()));
    assert!(mark.get("items").is_none());
    assert_eq!(fetch_servers(&session).await?[0]["channels"][0]["mentions"], json!(0));

    // Catching up asks from the newest message; earlier messages come straight from Discord.
    chat_call(&session, "POST", "/catchUp", Some(&json!({ "threadId": general_id }))).await?;
    let log = read_bridge_log(&control);
    let caught = log.iter().filter(|e| e["method"] == json!("requestHistory")).last().unwrap();
    assert!(regex::Regex::new(r"^\d+$").unwrap().is_match(caught["after"].as_str().unwrap_or_default()));
    assert!(caught.get("before").is_none());
    let earlier = chat_call(&session, "POST", "/history", Some(&json!({ "threadId": general_id }))).await?;
    assert_eq!(earlier, json!({ "ok": true, "count": 1, "more": false }));
    assert_eq!(messages_of(&session, &general_id).await?["messages"][0]["text"], json!("Welcome to the channel"));

    // Mute follows Discord's channel settings; pinning is WhatsApp's.
    chat_call(&session, "POST", "/chatSetting", Some(&json!({ "threadId": general_id, "mutedUntil": -1 }))).await?;
    let log = read_bridge_log(&control);
    let muted = log.iter().filter(|e| e["method"] == json!("chatSetting")).last().unwrap();
    assert_eq!((&muted["chat"], &muted["serverId"], &muted["mutedUntil"]), (&json!("911"), &json!("900"), &json!(-1)));
    assert_eq!(fetch_servers(&session).await?[0]["channels"][0]["mutedUntil"], json!(-1));
    assert_eq!(chat_raw(&session, "POST", "/chatSetting", Some(&json!({ "threadId": general_id, "pinned": true }))).await?.status, 400);
    let info = chat_call(&session, "GET", &format!("/info?threadId={}", encode_uri_component(&general_id)), None).await?;
    assert_eq!(info["server"]["name"], json!("Homelab Club"));
    assert_eq!(info["account"]["platform"], json!("discord"));
    assert_eq!(info["group"], Value::Null);

    // A new direct message with a friend.
    let friends = chat_call(&session, "GET", &format!("/friends?accountId={account_id}"), None).await?;
    let friend_names: Vec<&str> = friends["friends"].as_array().unwrap().iter().filter_map(|f| f["name"].as_str()).collect();
    assert_eq!(friend_names, vec!["Lina K", "tarek"]);
    let opened = chat_call(&session, "POST", "/chats", Some(&json!({ "accountId": account_id, "user": "300" }))).await?;
    assert_eq!(opened["name"], json!("tarek"));
    assert_eq!(opened["kind"], json!("direct"));
    assert_eq!(chat_raw(&session, "POST", "/chats", Some(&json!({ "accountId": account_id, "phone": "+15550009999" }))).await?.status, 400);

    // Voice: a live socket per account brings who is in each voice channel and the call.
    let live_url = format!("ws://127.0.0.1:{port}/api/v1/chat/live?accountId={account_id}");
    assert!(WsClient::connect(&live_url, &[("Origin", &base)]).await.is_err(), "a socket without the session");
    assert!(WsClient::connect(&live_url, &[("Cookie", &session.cookie), ("Origin", "http://127.0.0.1:1")]).await.is_err(), "a socket from another origin");
    let other_url = live_url.replace(&account_id, &uuid());
    assert!(WsClient::connect(&other_url, &[("Cookie", &session.cookie), ("Origin", &base)]).await.is_err(), "a socket for someone else's account");

    let tab = WsClient::connect(&live_url, &[("Cookie", &session.cookie), ("Origin", &base)]).await?;
    let other = WsClient::connect(&live_url, &[("Cookie", &session.cookie), ("Origin", &base)]).await?;
    let hello = until(Duration::from_secs(15), "hello", || async { Ok(tab.last("hello")) }, |v| v.is_some()).await?.unwrap();
    let lounge_ids: Vec<&str> = hello["servers"]["900"]["921"].as_array().unwrap().iter().filter_map(|p| p["id"].as_str()).collect();
    assert_eq!(lounge_ids, vec!["200", "300"]);
    assert_eq!(hello["call"], Value::Null);
    assert_eq!(chat_raw(&session, "POST", "/voice/join", Some(&json!({ "threadId": general_id }))).await?.status, 400, "a text channel cannot be joined");
    chat_call(&session, "POST", "/voice/join", Some(&json!({ "threadId": lounge_id, "muted": false }))).await?;
    assert_eq!(
        read_bridge_log(&control).into_iter().find(|e| e["method"] == json!("voiceJoin")),
        Some(json!({ "method": "voiceJoin", "account": account_id, "chat": "921", "muted": false, "deafened": false }))
    );
    tab.send_json(&json!({ "type": "claim" })).await?;
    let connected_call = until(
        Duration::from_secs(15),
        "the call to connect",
        || async { Ok(tab.last("call")) },
        |c| c.as_ref().is_some_and(|v| v["call"]["state"] == json!("connected")),
    )
    .await?
    .unwrap();
    assert_eq!(connected_call["call"]["channelId"], json!("921"));
    assert_eq!(connected_call["call"]["e2ee"], json!(true));
    until(
        Duration::from_secs(15),
        "the other tab to see the call is elsewhere",
        || async { Ok(other.last("owner")) },
        |o| o.as_ref().is_some_and(|v| v["elsewhere"] == json!(true)),
    )
    .await?;
    assert_eq!(tab.last("owner").unwrap()["here"], json!(true));
    until(
        Duration::from_secs(15),
        "me in Lounge",
        || async { Ok(tab.last("voice")) },
        |v| v.as_ref().and_then(|x| x["servers"]["900"]["921"].as_array()).is_some_and(|a| a.len() == 3),
    )
    .await?;

    // Microphone frames from the owning tab reach the bridge; another tab's are ignored. Lina's audio comes back as binary.
    let opus: [u8; 7] = [2, 0xfc, 0xff, 0xfe, 1, 2, 3];
    other.send_binary(&opus).await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!read_bridge_log(&control).iter().any(|e| e["method"] == json!("voiceAudio")), "audio from a tab that does not own the call");
    for _ in 0..3 {
        tab.send_binary(&opus).await?;
    }
    until(
        Duration::from_secs(15),
        "microphone audio at the bridge",
        || async { Ok(read_bridge_log(&control)) },
        |entries| entries.iter().any(|e| e["method"] == json!("voiceAudio") && e["bytes"] == json!(6)),
    )
    .await?;
    until(Duration::from_secs(15), "Lina's audio", || async { Ok(tab.audio.lock().unwrap().len()) }, |&n| n >= 3).await?;
    let frame = tab.audio.lock().unwrap()[0].clone();
    assert_eq!(frame[0], 1);
    let user_id = u64::from_be_bytes(frame[1..9].try_into().unwrap());
    assert_eq!(user_id, 200);
    assert_eq!(&frame[9..], &opus[1..]);
    assert_eq!(other.audio.lock().unwrap().len(), 0, "only the owning tab hears the call");

    // Mute and deafen go to Discord; the call code shows for an encrypted call.
    chat_call(&session, "POST", "/voice/set", Some(&json!({ "accountId": account_id, "muted": true, "deafened": false }))).await?;
    until(
        Duration::from_secs(15),
        "muted",
        || async { Ok(tab.last("call")) },
        |c| c.as_ref().is_some_and(|v| v["call"]["selfMute"] == json!(true)),
    )
    .await?;
    assert_eq!(chat_call(&session, "GET", &format!("/voice/code?accountId={account_id}"), None).await?["code"].as_str().unwrap().len(), 30);
    assert_eq!(chat_raw(&session, "POST", "/voice/set", Some(&json!({ "accountId": account_id, "muted": "yes", "deafened": false }))).await?.status, 400);

    // Discord ending the call says why; leaving ends it too.
    write_control(&control, &account_id, "voiceend", None)?;
    let ended = until(
        Duration::from_secs(15),
        "the call to end",
        || async { Ok(tab.last("call")) },
        |c| c.as_ref().is_some_and(|v| v["call"].is_null()),
    )
    .await?
    .unwrap();
    assert_eq!(ended["ended"]["reason"], json!("disconnected"));
    assert!(ended["ended"]["message"].as_str().unwrap_or_default().contains("disconnected"));

    chat_call(&session, "POST", "/voice/join", Some(&json!({ "threadId": lounge_id, "muted": true }))).await?;
    tab.send_json(&json!({ "type": "claim" })).await?;
    until(
        Duration::from_secs(15),
        "joining again",
        || async { Ok(tab.last("call")) },
        |c| c.as_ref().is_some_and(|v| v["call"]["state"] == json!("connected")),
    )
    .await?;
    chat_call(&session, "POST", "/voice/leave", Some(&json!({ "accountId": account_id }))).await?;
    until(
        Duration::from_secs(15),
        "leaving",
        || async { Ok(tab.last("call")) },
        |c| c.as_ref().is_some_and(|v| v["call"].is_null()),
    )
    .await?;

    // A call nobody hears any more is left after a while, as when Discord's app closes.
    chat_call(&session, "POST", "/voice/join", Some(&json!({ "threadId": lounge_id, "muted": true }))).await?;
    tab.send_json(&json!({ "type": "claim" })).await?;
    until(
        Duration::from_secs(15),
        "owning the call",
        || async { Ok(tab.last("owner")) },
        |o| o.as_ref().is_some_and(|v| v["here"] == json!(true)),
    )
    .await?;
    tab.close().await;
    until(
        Duration::from_secs(25),
        "the orphaned call to be left",
        || async { Ok(read_bridge_log(&control)) },
        |entries| entries.iter().filter(|e| e["method"] == json!("voiceLeave")).count() == 2,
    )
    .await?;
    other.close().await;

    // Search finds channel messages under the channel's name.
    let found = session.get_json("/api/v1/search?q=caddy").await?;
    let hits: Vec<&Value> = found["items"].as_array().unwrap().iter().filter(|i| i["group"] == json!("Chat")).collect();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["title"], json!("#general"));
    assert!(hits[0]["detail"].as_str().unwrap_or_default().contains("lina.k: Anyone moved"));

    // Channels and servers the account no longer sees go, with their messages.
    let kept_channels: Vec<Value> = [&general, &nixos, &lounge]
        .iter()
        .map(|c| {
            json!({
                "id": c["jid"],
                "kind": c["kind"],
                "name": c["name"],
                "serverId": "900",
                "section": c["section"],
                "order": c["order"]
            })
        })
        .collect();
    write_control(&control, &account_id, "ingest", Some(&json!({ "kind": "channels", "serverId": "900", "complete": true, "chats": kept_channels })))?;
    until(Duration::from_secs(15), "a removed channel", || fetch_servers(&session), |list| list[0]["channels"].as_array().unwrap().len() == 3).await?;
    write_control(
        &control,
        &account_id,
        "ingest",
        Some(&json!({ "kind": "servers", "complete": true, "servers": [{ "id": "900", "name": "Homelab Club", "order": "00000" }] })),
    )?;
    until(Duration::from_secs(15), "a server left", || fetch_servers(&session), |list| list.len() == 1).await?;
    assert!(!sql_text(&db.sql("SELECT id FROM chat_threads").await?).contains("/951"));

    // Backups keep servers; one made before Discord (no new columns) still restores.
    let pin = uuid();
    let snapshot = db.call("service_backup", json!([pin])).await?;
    db.call("service_release_backup", json!([pin])).await?;
    assert_eq!(snapshot["chat"]["servers"].as_array().unwrap().len(), 1);
    assert!(!snapshot.to_string().contains("fake-discord-token"));
    let mut old = snapshot.clone();
    if let Some(chat_obj) = old["chat"].as_object_mut() {
        chat_obj.remove("servers");
        if let Some(threads) = chat_obj.get_mut("threads").and_then(Value::as_array_mut) {
            for th in threads {
                if let Some(obj) = th.as_object_mut() {
                    for key in ["serverId", "section", "order", "topic", "mentions"] {
                        obj.remove(key);
                    }
                }
            }
        }
        if let Some(reactions) = chat_obj.get_mut("reactions").and_then(Value::as_array_mut) {
            for r in reactions {
                if let Some(obj) = r.as_object_mut() {
                    obj.remove("count");
                }
            }
        }
    }
    let second_name = format!("{}-restore", runtime.config.database_name);
    instance::publish_module(&second_name, &base)?;
    let restored_db = Database::new(&db.url, &second_name, &db.token_file)?;
    restored_db.call("service_restore", json!([old.to_string()])).await?;
    assert!(sql_text(&restored_db.sql("SELECT state FROM chat_accounts").await?).contains("loggedOut"));

    // After a restart the bridge reconnects with the stored token.
    runtime.restart().await?;
    session = login(&base).await?;
    until(
        Duration::from_secs(20),
        "reconnecting after a restart",
        || chat_call(&session, "GET", "/overview", None),
        |o| o["bridge"]["available"] == json!(true) && o["accounts"][0]["state"] == json!("connected"),
    )
    .await?;
    let reconnect = read_bridge_log(&control).into_iter().find(|e| e["method"] == json!("connect")).context("connect in log")?;
    assert_eq!(reconnect["platform"], json!("discord"));
    assert_eq!(reconnect["token"], json!("fake-discord-token"));

    // Signing out keeps the messages, read-only, and deletes the token.
    let out = chat_call(&session, "POST", &format!("/accounts/{account_id}/unlink"), Some(&json!({}))).await?;
    assert_eq!(out["confirmed"], json!(true));
    assert_eq!(chat_call(&session, "GET", "/overview", None).await?["accounts"][0]["state"], json!("loggedOut"));
    assert!(!sql_text(&db.sql("SELECT id FROM chat_keys").await?).contains(&account_id));
    assert!(!messages_of(&session, &general_id).await?["messages"].as_array().unwrap().is_empty());
    chat_call(&session, "DELETE", &format!("/accounts/{account_id}"), None).await?;
    assert!(!sql_text(&db.sql("SELECT id FROM chat_servers").await?).contains(&account_id));

    runtime.close().await;
    Ok(())
}
