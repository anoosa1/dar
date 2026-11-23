//! Helpers and fixtures shared by the comms suites (ported from `test/shared/`, `test/support/` and
//! `test/fixtures/`). Only that area's owner edits this file.

#![allow(dead_code)]

use axum::extract::State as AxumState;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::Engine as _;
use dar_acceptance::prelude::*;
use p256::ecdsa::signature::Signer as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_rustls::{TlsAcceptor, TlsConnector};

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

fn utc_ms(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> i64 {
    chrono::NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|d| d.and_hms_opt(hour, min, sec))
        .unwrap()
        .and_utc()
        .timestamp_millis()
}

fn b64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64_decode(text: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|e| Error::new(format!("base64 decode: {e}")))
}

/// Real `width`×`height` RGB PNG (matching `makePng` in `fake-chat-bridge.mjs`).
pub fn make_png(width: u32, height: u32) -> Vec<u8> {
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut body = Vec::with_capacity(4 + data.len());
        body.extend_from_slice(kind);
        body.extend_from_slice(data);
        let mut crc = flate2::Crc::new();
        crc.update(&body);
        let mut out = Vec::with_capacity(12 + data.len());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc.sum().to_be_bytes());
        out
    }
    let mut header = [0u8; 13];
    header[0..4].copy_from_slice(&width.to_be_bytes());
    header[4..8].copy_from_slice(&height.to_be_bytes());
    header[8] = 8;
    header[9] = 2;
    let mut rows = Vec::with_capacity((height as usize) * (1 + (width as usize) * 3));
    for _ in 0..height {
        rows.push(0);
        rows.extend(std::iter::repeat_n(0x9au8, (width as usize) * 3));
    }
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&rows).unwrap();
    let compressed = enc.finish().unwrap();
    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    png.extend_from_slice(&chunk(b"IHDR", &header));
    png.extend_from_slice(&chunk(b"IDAT", &compressed));
    png.extend_from_slice(&chunk(b"IEND", &[]));
    png
}

// =================================================================================================
// Fake Chat Bridge (`test/fixtures/fake-chat-bridge.mjs`)
// =================================================================================================

struct BridgeError {
    code: String,
    message: String,
}

impl BridgeError {
    fn new(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }
}

#[derive(Clone)]
struct VoiceCall {
    chat: String,
    muted: bool,
    deafened: bool,
    state: String,
    frames: u64,
}

struct BridgeState {
    control: PathBuf,
    writer: mpsc::UnboundedSender<String>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<String, oneshot::Sender<std::result::Result<Value, String>>>>,
    linking: Mutex<HashSet<String>>,
    connected: Mutex<HashSet<String>>,
    resent: Mutex<HashSet<String>>,
    discord: Mutex<HashSet<String>>,
    calls: Mutex<HashMap<String, VoiceCall>>,
    png: Vec<u8>,
}

const WHATSAPP_JID: &str = "15550001111@s.whatsapp.net";
const DISCORD_ME: &str = "100";

impl BridgeState {
    fn write_frame(&self, frame: Value) {
        let _ = self.writer.send(frame.to_string());
    }

    fn emit(&self, account: &str, event: &str, data: Value) {
        self.write_frame(json!({ "event": event, "account": account, "data": data }));
    }

    fn log(&self, entry: Value) {
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(self.control.join("log.jsonl")) {
            let _ = writeln!(file, "{entry}");
        }
    }

    fn log_with_params(&self, method: &str, params: &Value) {
        let mut map = params.as_object().cloned().unwrap_or_default();
        map.insert("method".into(), json!(method));
        self.log(Value::Object(map));
    }

    async fn call(&self, method: &str, params: Value) -> std::result::Result<Value, BridgeError> {
        let id = format!("g{}", self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        self.write_frame(json!({ "id": id, "method": method, "params": params }));
        match rx.await {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(msg)) => Err(BridgeError::new(msg, "bridgeError")),
            Err(_) => Err(BridgeError::new("bridge stdin closed", "bridgeError")),
        }
    }

    fn voice_states(&self, account: &str) {
        let call = self.calls.lock().unwrap().get(account).cloned();
        let mut people = vec![
            json!({ "id": "200", "name": "Lina K" }),
            json!({ "id": "300", "name": "tarek", "muted": true }),
        ];
        if let Some(c) = call {
            let mut me_obj = serde_json::Map::new();
            me_obj.insert("id".into(), json!(DISCORD_ME));
            me_obj.insert("name".into(), json!("Anas"));
            if c.muted {
                me_obj.insert("muted".into(), json!(true));
            }
            if c.deafened {
                me_obj.insert("deafened".into(), json!(true));
            }
            people.push(Value::Object(me_obj));
        }
        self.emit(account, "voiceStates", json!({ "servers": { "900": { "921": people } }, "complete": true }));
    }

    fn call_state(&self, account: &str) {
        if let Some(c) = self.calls.lock().unwrap().get(account).cloned() {
            self.emit(
                account,
                "voiceCall",
                json!({
                    "state": c.state,
                    "serverId": "900",
                    "channelId": c.chat,
                    "selfMute": c.muted,
                    "selfDeaf": c.deafened,
                    "canSpeak": true,
                    "e2ee": c.state == "connected",
                    "protocol": 1,
                }),
            );
        }
    }

    async fn discord_ready(&self, account: &str) -> std::result::Result<(), BridgeError> {
        let dc_at = utc_ms(2026, 11, 17, 9, 0, 0);
        self.call(
            "ingest",
            json!({
                "account": account,
                "kind": "servers",
                "complete": true,
                "servers": [
                    { "id": "900", "name": "Homelab Club", "order": "00000", "mutedUntil": 0 },
                    { "id": "950", "name": "Kernel newbies", "order": "00001", "mutedUntil": -1 }
                ]
            }),
        )
        .await?;
        self.call(
            "ingest",
            json!({
                "account": account,
                "kind": "channels",
                "serverId": "900",
                "complete": true,
                "chats": [
                    { "id": "911", "kind": "channel", "name": "general", "serverId": "900", "section": "Text channels", "order": "000001/0/000001/911", "topic": "Say hello", "unread": 1, "mentions": 1, "mutedUntil": 0, "at": dc_at + 60_000 },
                    { "id": "912", "kind": "channel", "name": "nixos", "serverId": "900", "section": "Text channels", "order": "000001/0/000002/912", "topic": "", "unread": 0, "mentions": 0, "mutedUntil": 0, "at": dc_at },
                    { "id": "913", "kind": "channel", "name": "off-topic", "serverId": "900", "section": "Text channels", "order": "000001/0/000003/913", "topic": "", "unread": 1, "mentions": 0, "mutedUntil": -1, "at": dc_at },
                    { "id": "921", "kind": "voice", "name": "Lounge", "serverId": "900", "section": "Voice channels", "order": "000002/1/000000/921", "topic": "", "unread": 0, "mentions": 0, "mutedUntil": 0, "at": 0 }
                ]
            }),
        )
        .await?;
        self.call(
            "ingest",
            json!({
                "account": account,
                "kind": "channels",
                "serverId": "950",
                "complete": true,
                "chats": [
                    { "id": "951", "kind": "channel", "name": "welcome", "serverId": "950", "section": "", "order": "000000/0/000000/951", "unread": 0, "mentions": 0, "mutedUntil": 0, "at": dc_at }
                ]
            }),
        )
        .await?;
        self.call(
            "ingest",
            json!({
                "account": account,
                "kind": "chats",
                "chats": [
                    { "id": "700", "kind": "direct", "name": "Lina K", "unread": 1, "mentions": 1, "mutedUntil": 0, "at": dc_at + 120_000 }
                ]
            }),
        )
        .await?;
        self.voice_states(account);
        self.call(
            "ingest",
            json!({
                "account": account,
                "kind": "history",
                "messages": [
                    { "id": "1300000000000000011", "chat": "911", "sender": "200", "senderName": "lina.k", "fromMe": false, "at": dc_at, "kind": "text", "text": "Anyone moved to the new caddy module?" },
                    { "id": "1300000000000000012", "chat": "911", "sender": "300", "senderName": "tarek", "fromMe": false, "at": dc_at + 30_000, "kind": "text", "text": "Yes, painless", "reactions": [{ "emoji": "👍", "count": 3, "me": true }, { "emoji": "party:55", "count": 1, "me": false }] },
                    { "id": "1300000000000000013", "chat": "911", "sender": "300", "senderName": "tarek", "fromMe": false, "at": dc_at + 60_000, "kind": "image", "text": "", "fileName": "rack.png", "media": { "mime": "image/png", "size": self.png.len(), "width": 4, "height": 3 } },
                    { "id": "1300000000000000021", "chat": "700", "sender": "200", "senderName": "Lina K", "fromMe": false, "at": dc_at + 120_000, "kind": "text", "text": "Hopping into Lounge later?" }
                ]
            }),
        )
        .await?;
        Ok(())
    }

    async fn discord_pair(&self, account: &str) -> std::result::Result<(), BridgeError> {
        self.linking.lock().unwrap().remove(account);
        let user_json = json!({ "id": DISCORD_ME, "username": "anas" }).to_string();
        self.call(
            "kv.commit",
            json!({
                "account": account,
                "puts": [
                    { "key": "discord/token", "value": b64_encode(b"fake-discord-token") },
                    { "key": "discord/user", "value": b64_encode(user_json.as_bytes()) }
                ],
                "deletes": []
            }),
        )
        .await?;
        self.emit(account, "paired", json!({ "jid": DISCORD_ME, "name": "Anas" }));
        self.connected.lock().unwrap().insert(account.to_owned());
        self.discord_ready(account).await?;
        self.emit(account, "connected", json!({ "name": "Anas" }));
        Ok(())
    }

    async fn whatsapp_pair(&self, account: &str) -> std::result::Result<(), BridgeError> {
        self.linking.lock().unwrap().remove(account);
        self.call(
            "kv.commit",
            json!({
                "account": account,
                "puts": [
                    { "key": "device", "value": b64_encode(b"fake-device-secret") },
                    { "key": "session/1555:0", "value": b64_encode(b"session-secret") },
                    { "key": "evbuf/aa", "value": b64_encode(b"buffered"), "lazy": true, "expiresAt": now_ms() + 60_000 }
                ],
                "deletes": []
            }),
        )
        .await?;
        self.emit(account, "paired", json!({ "jid": WHATSAPP_JID, "platform": "android" }));
        self.connected.lock().unwrap().insert(account.to_owned());
        self.emit(account, "connected", json!({ "pushName": "Me" }));
        let at = utc_ms(2026, 11, 16, 9, 0, 0);
        let d1_len = b"%PDF-1.4 fake menu".len();
        let x1_len = b"<script>alert(1)</script>".len();
        self.call(
            "ingest",
            json!({
                "account": account,
                "kind": "history",
                "syncType": "INITIAL_BOOTSTRAP",
                "final": false,
                "chats": [
                    { "id": "15550002222@s.whatsapp.net", "kind": "direct", "name": "Sara Haddad", "unread": 1, "pinned": true, "archived": false, "mutedUntil": 0, "at": at + 60_000 },
                    { "id": "120363000000000001@g.us", "kind": "group", "name": "Family", "unread": 2, "pinned": false, "archived": false, "mutedUntil": -1, "at": at + 120_000 },
                    { "id": "15550003333@s.whatsapp.net", "kind": "direct", "name": "Old friend", "unread": 0, "pinned": false, "archived": true, "mutedUntil": 0, "at": at }
                ],
                "messages": [
                    { "id": "H1", "chat": "15550002222@s.whatsapp.net", "sender": "15550002222@s.whatsapp.net", "fromMe": false, "at": at, "kind": "text", "text": "Are we still on for dinner?" },
                    { "id": "H2", "chat": "15550002222@s.whatsapp.net", "sender": WHATSAPP_JID, "fromMe": true, "at": at + 30_000, "kind": "text", "text": "Yes!", "status": "read" },
                    { "id": "H3", "chat": "15550002222@s.whatsapp.net", "sender": "15550002222@s.whatsapp.net", "fromMe": false, "at": at + 60_000, "kind": "image", "text": "The place", "quotedId": "H2", "quotedSender": "You", "quotedText": "Yes!", "media": { "mime": "image/png", "size": self.png.len(), "width": 1200, "height": 900 } },
                    { "id": "D1", "chat": "120363000000000001@g.us", "sender": "15550004444@s.whatsapp.net", "senderName": "Mama", "fromMe": false, "at": at + 80_000, "kind": "document", "fileName": "Eid lunch menu.pdf", "media": { "mime": "application/pdf", "size": d1_len } },
                    { "id": "X1", "chat": "120363000000000001@g.us", "sender": "15550004444@s.whatsapp.net", "senderName": "Mama", "fromMe": false, "at": at + 85_000, "kind": "document", "fileName": "page.html", "media": { "mime": "text/html", "size": x1_len } },
                    { "id": "EXP1", "chat": "15550003333@s.whatsapp.net", "sender": "15550003333@s.whatsapp.net", "fromMe": false, "at": at + 1000, "kind": "image", "media": { "mime": "image/png", "size": self.png.len(), "width": 1, "height": 1 } },
                    { "id": "G1", "chat": "120363000000000001@g.us", "sender": "15550004444@s.whatsapp.net", "senderName": "Mama", "fromMe": false, "at": at + 90_000, "kind": "text", "text": "Eid mubarak" },
                    { "id": "G2", "chat": "120363000000000001@g.us", "sender": "15550005555@s.whatsapp.net", "senderName": "Baba", "fromMe": false, "at": at + 120_000, "kind": "voice" },
                    { "id": "O1", "chat": "15550003333@s.whatsapp.net", "sender": "15550003333@s.whatsapp.net", "fromMe": false, "at": at, "kind": "text", "text": "Long time" }
                ]
            }),
        )
        .await?;
        self.call(
            "ingest",
            json!({
                "account": account,
                "kind": "history",
                "syncType": "INITIAL_BOOTSTRAP",
                "final": true,
                "progress": 100,
                "chats": [],
                "messages": []
            }),
        )
        .await?;
        let lazy = self.call("kv.get", json!({ "account": account, "keys": ["evbuf/aa", "evbuf/missing"] })).await?;
        let values = lazy.get("values").and_then(Value::as_object);
        let ok = values.is_some_and(|v| {
            v.get("evbuf/aa").and_then(Value::as_str) == Some(b64_encode(b"buffered").as_str())
                && !v.contains_key("evbuf/missing")
        });
        self.log(json!({ "method": "lazyCheck", "account": account, "ok": ok }));
        Ok(())
    }

    async fn handle(self: &Arc<Self>, method: &str, params: Value) -> std::result::Result<Value, BridgeError> {
        let account = params.get("account").and_then(Value::as_str).unwrap_or_default().to_owned();
        match method {
            "hello" => Ok(json!({ "protocol": 1, "platforms": ["whatsapp", "discord"] })),
            "link" => {
                let platform = params.get("platform").and_then(Value::as_str).unwrap_or("whatsapp");
                let phone = params.get("phone").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
                self.linking.lock().unwrap().insert(account.clone());
                if platform == "discord" {
                    self.discord.lock().unwrap().insert(account.clone());
                    let state = self.clone();
                    let acc = account.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        let prefix: String = acc.chars().take(4).collect();
                        state.emit(&acc, "linkCode", json!({ "code": format!("https://discord.com/ra/fakeFingerprint{prefix}"), "expiresIn": 110 }));
                    });
                    return Ok(json!({}));
                }
                self.discord.lock().unwrap().remove(&account);
                let state = self.clone();
                let acc = account.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let prefix: String = acc.chars().take(8).collect();
                    state.emit(&acc, "linkCode", json!({ "code": format!("2@fakeQrCode,{prefix},abc="), "expiresIn": 20 }));
                    if phone.is_some() {
                        state.emit(&acc, "pairingCode", json!({ "code": "K7QM2X9D" }));
                    }
                });
                Ok(json!({}))
            }
            "connect" => {
                let platform = params.get("platform").and_then(Value::as_str).unwrap_or("whatsapp").to_owned();
                let page = self.call("kv.load", json!({ "account": account, "cursor": "" })).await?;
                let entries = page.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
                if platform == "discord" {
                    self.discord.lock().unwrap().insert(account.clone());
                    let token = entries.iter().find(|e| e.get("key").and_then(Value::as_str) == Some("discord/token"));
                    let Some(token) = token else {
                        return Err(BridgeError::new("This account is not linked.", "notLinked"));
                    };
                    let decoded = String::from_utf8_lossy(&b64_decode(token["value"].as_str().unwrap_or_default()).unwrap_or_default()).into_owned();
                    self.log(json!({ "method": "connect", "account": account, "platform": platform, "token": decoded }));
                    self.connected.lock().unwrap().insert(account.clone());
                    let state = self.clone();
                    let acc = account.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        if state.discord_ready(&acc).await.is_ok() {
                            state.emit(&acc, "connected", json!({}));
                        }
                    });
                    return Ok(json!({ "jid": DISCORD_ME }));
                }
                let device = entries.iter().find(|e| e.get("key").and_then(Value::as_str) == Some("device"));
                let Some(device) = device else {
                    return Err(BridgeError::new("This account is not linked.", "notLinked"));
                };
                let decoded = String::from_utf8_lossy(&b64_decode(device["value"].as_str().unwrap_or_default()).unwrap_or_default()).into_owned();
                let mut keys: Vec<String> = entries.iter().filter_map(|e| e.get("key").and_then(Value::as_str).map(str::to_owned)).collect();
                keys.sort();
                self.log(json!({ "method": "connect", "account": account, "device": decoded, "keys": keys }));
                let lazy = self.call("kv.get", json!({ "account": account, "keys": ["evbuf/aa"] })).await?;
                let ok = lazy.get("values").and_then(|v| v.get("evbuf/aa")).and_then(Value::as_str) == Some(b64_encode(b"buffered").as_str());
                self.log(json!({ "method": "lazyAfterRestart", "account": account, "ok": ok }));
                self.connected.lock().unwrap().insert(account.clone());
                let state = self.clone();
                let acc = account.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    state.emit(&acc, "connected", json!({}));
                });
                Ok(json!({ "jid": WHATSAPP_JID }))
            }
            "disconnect" => {
                self.linking.lock().unwrap().remove(&account);
                self.connected.lock().unwrap().remove(&account);
                Ok(json!({}))
            }
            "logout" => {
                if !self.connected.lock().unwrap().contains(&account) {
                    return Err(BridgeError::new("WhatsApp is not connected.", "notConnected"));
                }
                self.connected.lock().unwrap().remove(&account);
                self.log(json!({ "method": "logout", "account": account }));
                Ok(json!({}))
            }
            "send" => {
                self.log_with_params("send", &params);
                let text = params.get("text").and_then(Value::as_str).unwrap_or_default();
                let id_str = params.get("id").and_then(Value::as_str).unwrap_or_default();
                let chat = params.get("chat").and_then(Value::as_str).unwrap_or_default();
                let is_discord = self.discord.lock().unwrap().contains(&account);
                let is_connected = self.connected.lock().unwrap().contains(&account);
                if is_discord {
                    if !is_connected {
                        return Err(BridgeError::new("Discord is not connected.", "notConnected"));
                    }
                    if text.contains("fail") {
                        return Err(BridgeError::new("Discord did not accept this.", "invalidInput"));
                    }
                    let id_num: u128 = id_str.parse().unwrap_or(0) + 7;
                    let id = id_num.to_string();
                    if text.contains("echo") {
                        let mut msg = json!({
                            "id": id,
                            "chat": chat,
                            "sender": DISCORD_ME,
                            "senderName": "Anas",
                            "fromMe": true,
                            "at": now_ms(),
                            "kind": "text",
                            "text": text,
                            "nonce": id_str,
                        });
                        if let Some(quoted_id) = params.get("quotedId").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                            msg["quotedId"] = json!(quoted_id);
                        }
                        self.call("ingest", json!({ "account": account, "kind": "messages", "messages": [msg] })).await?;
                    }
                    return Ok(json!({ "id": id, "at": now_ms() }));
                }
                if !is_connected {
                    return Err(BridgeError::new("WhatsApp is not connected.", "notConnected"));
                }
                if text.contains("fail") {
                    return Err(BridgeError::new("WhatsApp did not accept the message.", "sendFailed"));
                }
                Ok(json!({ "id": id_str, "at": now_ms() }))
            }
            "markRead" => {
                self.log_with_params("markRead", &params);
                Ok(json!({}))
            }
            "react" => {
                self.log_with_params("react", &params);
                let now = now_ms();
                Ok(json!({ "id": format!("R{now}"), "at": now }))
            }
            "edit" => {
                self.log_with_params("edit", &params);
                let now = now_ms();
                Ok(json!({ "id": format!("E{now}"), "at": now }))
            }
            "revoke" => {
                self.log_with_params("revoke", &params);
                let now = now_ms();
                Ok(json!({ "id": format!("X{now}"), "at": now }))
            }
            "chatSetting" => {
                self.log_with_params("chatSetting", &params);
                Ok(json!({}))
            }
            "groupInfo" => Ok(json!({
                "name": "Family",
                "topic": "Eid plans",
                "created": utc_ms(2020, 1, 1, 0, 0, 0),
                "members": [
                    { "jid": WHATSAPP_JID, "name": "Me", "admin": true },
                    { "jid": "15550004444@s.whatsapp.net", "name": "Mama", "admin": true },
                    { "jid": "15550005555@s.whatsapp.net", "name": "Baba" }
                ]
            })),
            "requestHistory" => {
                self.log_with_params("requestHistory", &params);
                let chat = params.get("chat").and_then(Value::as_str).unwrap_or_default().to_owned();
                if self.discord.lock().unwrap().contains(&account) {
                    if params.get("before").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) {
                        let dc_at = utc_ms(2026, 11, 17, 9, 0, 0);
                        self.call(
                            "ingest",
                            json!({
                                "account": account,
                                "kind": "history",
                                "messages": [
                                    { "id": "1299999999999999990", "chat": chat, "sender": "200", "senderName": "lina.k", "fromMe": false, "at": dc_at - 86_400_000, "kind": "text", "text": "Welcome to the channel" }
                                ]
                            }),
                        )
                        .await?;
                        return Ok(json!({ "count": 1, "more": false }));
                    }
                    if params.get("after").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) {
                        return Ok(json!({ "count": 0 }));
                    }
                    return Ok(json!({ "count": 0 }));
                }
                let at = params.get("at").and_then(Value::as_i64).unwrap_or(0);
                let state = self.clone();
                let acc = account.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let _ = state
                        .call(
                            "ingest",
                            json!({
                                "account": acc,
                                "kind": "history",
                                "syncType": "ON_DEMAND",
                                "final": true,
                                "progress": 100,
                                "chats": [],
                                "messages": [
                                    { "id": "H0", "chat": chat, "sender": chat, "fromMe": false, "at": at - 86_400_000, "kind": "text", "text": "Hello from last week" }
                                ]
                            }),
                        )
                        .await;
                });
                Ok(json!({}))
            }
            "voiceJoin" => {
                let chat = params.get("chat").and_then(Value::as_str).unwrap_or_default().to_owned();
                let muted = params.get("muted").and_then(Value::as_bool).unwrap_or(false);
                let deafened = params.get("deafened").and_then(Value::as_bool).unwrap_or(false);
                self.log(json!({ "method": "voiceJoin", "account": account, "chat": chat, "muted": muted, "deafened": deafened }));
                if !self.connected.lock().unwrap().contains(&account) {
                    return Err(BridgeError::new("Discord is not connected.", "notConnected"));
                }
                if chat != "921" {
                    return Err(BridgeError::new("This voice channel no longer exists.", "notFound"));
                }
                self.calls.lock().unwrap().insert(account.clone(), VoiceCall { chat, muted, deafened, state: "connecting".into(), frames: 0 });
                self.call_state(&account);
                let state = self.clone();
                let acc = account.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(60)).await;
                    let present = {
                        let mut calls = state.calls.lock().unwrap();
                        if let Some(c) = calls.get_mut(&acc) {
                            c.state = "connected".into();
                            true
                        } else {
                            false
                        }
                    };
                    if present {
                        state.call_state(&acc);
                        state.voice_states(&acc);
                    }
                });
                Ok(json!({}))
            }
            "voiceSet" => {
                let muted = params.get("muted").and_then(Value::as_bool).unwrap_or(false);
                let deafened = params.get("deafened").and_then(Value::as_bool).unwrap_or(false);
                self.log(json!({ "method": "voiceSet", "account": account, "muted": muted, "deafened": deafened }));
                {
                    let mut calls = self.calls.lock().unwrap();
                    let Some(c) = calls.get_mut(&account) else {
                        return Err(BridgeError::new("You are not in a call.", "conflict"));
                    };
                    c.muted = muted;
                    c.deafened = deafened;
                }
                self.call_state(&account);
                self.voice_states(&account);
                Ok(json!({}))
            }
            "voiceLeave" => {
                self.log(json!({ "method": "voiceLeave", "account": account }));
                let removed = self.calls.lock().unwrap().remove(&account);
                if let Some(c) = removed {
                    self.emit(&account, "voiceCall", json!({ "state": "ended", "serverId": "900", "channelId": c.chat, "reason": "left" }));
                    self.voice_states(&account);
                }
                Ok(json!({}))
            }
            "voiceAudio" => {
                let frames = params.get("frames").and_then(Value::as_array).cloned().unwrap_or_default();
                let mut to_log = Vec::new();
                let mut to_emit = Vec::new();
                {
                    let mut calls = self.calls.lock().unwrap();
                    if let Some(c) = calls.get_mut(&account)
                        && !c.muted
                    {
                        for frame in frames {
                            let Some(frame_str) = frame.as_str() else { continue };
                            c.frames += 1;
                            if c.frames == 1 || c.frames.is_multiple_of(25) {
                                let bytes = b64_decode(frame_str).map(|b| b.len()).unwrap_or(0);
                                to_log.push((c.frames, bytes));
                            }
                            if !c.deafened {
                                to_emit.push((c.frames, frame_str.to_owned()));
                            }
                        }
                    }
                }
                for (count, bytes) in to_log {
                    self.log(json!({ "method": "voiceAudio", "account": account, "frames": count, "bytes": bytes }));
                }
                for (seq, data) in to_emit {
                    self.emit(&account, "voiceAudio", json!({ "user": "200", "seq": seq, "data": data }));
                }
                Ok(json!({}))
            }
            "voiceCode" => {
                if !self.calls.lock().unwrap().contains_key(&account) {
                    return Err(BridgeError::new("You are not in a call.", "conflict"));
                }
                Ok(json!({ "code": "483122207906411751234567890123" }))
            }
            "friends" => {
                if !self.discord.lock().unwrap().contains(&account) {
                    return Err(BridgeError::new("This is not available for this account.", "unsupported"));
                }
                Ok(json!({
                    "friends": [
                        { "id": "200", "name": "Lina K", "username": "lina" },
                        { "id": "300", "name": "tarek", "username": "tarek" }
                    ]
                }))
            }
            "openDirect" => {
                let user = params.get("user").and_then(Value::as_str).unwrap_or_default();
                self.log(json!({ "method": "openDirect", "account": account, "user": user }));
                self.call("ingest", json!({ "account": account, "kind": "chats", "chats": [{ "id": "7300", "kind": "direct", "name": "tarek" }] })).await?;
                Ok(json!({ "chat": "7300" }))
            }
            "download" => {
                let chat = params.get("chat").and_then(Value::as_str).unwrap_or_default();
                let id = params.get("id").and_then(Value::as_str).unwrap_or_default();
                let file = params.get("path").and_then(Value::as_str).unwrap_or_default();
                self.log(json!({ "method": "download", "account": account, "chat": chat, "id": id }));
                let write_wx = |path: &str, bytes: &[u8]| -> std::result::Result<(), BridgeError> {
                    use std::os::unix::fs::OpenOptionsExt;
                    let mut f = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(path)
                        .map_err(|e| BridgeError::new(e.to_string(), "bridgeError"))?;
                    f.write_all(bytes).map_err(|e| BridgeError::new(e.to_string(), "bridgeError"))
                };
                if self.discord.lock().unwrap().contains(&account) {
                    if id != "1300000000000000013" {
                        return Err(BridgeError::new("This attachment cannot be downloaded.", "noMedia"));
                    }
                    write_wx(file, &self.png)?;
                    return Ok(json!({ "size": self.png.len() }));
                }
                if !self.connected.lock().unwrap().contains(&account) {
                    return Err(BridgeError::new("WhatsApp is not connected.", "notConnected"));
                }
                let file_path = Path::new(file);
                let downloads_dir = std::env::var("DAR_CHAT_DOWNLOADS").unwrap_or_default();
                let base_re = regex::Regex::new(r"^[0-9a-f-]{36}\.part$").unwrap();
                let dir_ok = file_path.parent().and_then(Path::to_str) == Some(downloads_dir.as_str());
                let name_ok = file_path.file_name().and_then(|n| n.to_str()).is_some_and(|n| base_re.is_match(n));
                if !dir_ok || !name_ok {
                    return Err(BridgeError::new("bad path", "invalidInput"));
                }
                if id == "EXP1" && !self.resent.lock().unwrap().contains(id) {
                    return Err(BridgeError::new("WhatsApp no longer has this file; your phone was asked to send it again.", "retrying"));
                }
                let bytes: &[u8] = match id {
                    "H3" | "EXP1" => &self.png,
                    "V1" => b"OggS fake voice note",
                    "D1" => b"%PDF-1.4 fake menu",
                    "X1" => b"<script>alert(1)</script>",
                    _ => return Err(BridgeError::new("This attachment cannot be downloaded.", "noMedia")),
                };
                write_wx(file, bytes)?;
                Ok(json!({ "size": bytes.len() }))
            }
            _ => Err(BridgeError::new(format!("unknown method {method}"), "bridgeError")),
        }
    }

    async fn run_control(&self, account: &str, action: &str, data: Value) {
        let outcome: std::result::Result<(), BridgeError> = async {
            match action {
                "scan" => {
                    if self.linking.lock().unwrap().contains(account) {
                        self.emit(account, "linkScanned", json!({ "name": "anas" }));
                    }
                }
                "pair" => {
                    if self.linking.lock().unwrap().contains(account) {
                        if self.discord.lock().unwrap().contains(account) {
                            self.discord_pair(account).await?;
                        } else {
                            self.whatsapp_pair(account).await?;
                        }
                    }
                }
                "ingest" => {
                    let mut map = data.as_object().cloned().unwrap_or_default();
                    map.insert("account".into(), json!(account));
                    self.call("ingest", Value::Object(map)).await?;
                }
                "incoming" => {
                    let chats = data.get("chats").cloned().unwrap_or_else(|| json!([]));
                    let messages = data.get("messages").cloned().unwrap_or_else(|| json!([]));
                    self.call("ingest", json!({ "account": account, "kind": "messages", "chats": chats, "messages": messages })).await?;
                }
                "receipt" => {
                    let mut map = data.as_object().cloned().unwrap_or_default();
                    map.insert("account".into(), json!(account));
                    map.insert("kind".into(), json!("receipts"));
                    self.call("ingest", Value::Object(map)).await?;
                }
                "resend" => {
                    if let Some(id) = data.get("id").and_then(Value::as_str) {
                        self.resent.lock().unwrap().insert(id.to_owned());
                        let chat = data.get("chat").and_then(Value::as_str).unwrap_or_default();
                        self.emit(account, "mediaRetried", json!({ "chat": chat, "id": id, "ok": true }));
                    }
                }
                "voiceend" => {
                    let removed = self.calls.lock().unwrap().remove(account);
                    if let Some(c) = removed {
                        self.emit(account, "voiceCall", json!({ "state": "ended", "serverId": "900", "channelId": c.chat, "reason": "disconnected" }));
                        self.voice_states(account);
                    }
                }
                "loggedout" => {
                    self.connected.lock().unwrap().remove(account);
                    self.emit(account, "loggedOut", json!({ "reason": "logged out from another device" }));
                }
                _ => {}
            }
            Ok(())
        }
        .await;
        match outcome {
            Ok(()) => self.log(json!({ "method": "control", "action": action, "account": account, "ok": true })),
            Err(err) => self.log(json!({ "method": "control", "action": action, "account": account, "ok": false, "error": err.message })),
        }
    }
}

/// The fake chat bridge (`test/fixtures/fake-chat-bridge.mjs`), run by the runtime as
/// `DAR_CHAT_BRIDGE`; `DAR_FAKE_CHAT_CONTROL` names its control folder.
pub fn chat_bridge_fixture(_args: &[String]) -> ExitCode {
    let Ok(control) = std::env::var("DAR_FAKE_CHAT_CONTROL") else {
        eprintln!("DAR_FAKE_CHAT_CONTROL is required");
        return ExitCode::FAILURE;
    };
    let control = PathBuf::from(control);
    if !control.exists() {
        return ExitCode::FAILURE;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("bridge runtime");
    runtime.block_on(async move {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let writer_task = tokio::task::spawn_blocking(move || {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            while let Some(line) = rx.blocking_recv() {
                if writeln!(out, "{line}").is_err() || out.flush().is_err() {
                    break;
                }
            }
        });
        let state = Arc::new(BridgeState {
            control: control.clone(),
            writer: tx,
            next_id: AtomicU64::new(0),
            pending: Mutex::default(),
            linking: Mutex::default(),
            connected: Mutex::default(),
            resent: Mutex::default(),
            discord: Mutex::default(),
            calls: Mutex::default(),
            png: make_png(4, 3),
        });

        // Control folder watcher (every 100 ms).
        let control_state = state.clone();
        let control_re = regex::Regex::new(r"^([0-9a-f-]{36})\.(pair|incoming|receipt|loggedout|resend|scan|ingest|voiceend)(\.json)?$").unwrap();
        let watcher = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            loop {
                interval.tick().await;
                let Ok(entries) = std::fs::read_dir(&control_state.control) else { continue };
                let mut names: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect();
                names.sort();
                for name in names {
                    let Some(caps) = control_re.captures(&name) else { continue };
                    let file = control_state.control.join(&name);
                    let done = control_state.control.join(format!("{name}.done"));
                    if std::fs::rename(&file, &done).is_err() {
                        continue;
                    }
                    let account = caps[1].to_owned();
                    let action = caps[2].to_owned();
                    let data = if caps.get(3).is_some() {
                        std::fs::read_to_string(&done)
                            .ok()
                            .and_then(|s| serde_json::from_str(&s).ok())
                            .unwrap_or_else(|| json!({}))
                    } else {
                        json!({})
                    };
                    let st = control_state.clone();
                    tokio::spawn(async move {
                        st.run_control(&account, &action, data).await;
                    });
                }
            }
        });

        // Read NDJSON frames from stdin until EOF.
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(frame) = serde_json::from_str::<Value>(trimmed) else { continue };
            let method = frame.get("method").and_then(Value::as_str).map(str::to_owned);
            let id = frame.get("id").cloned().filter(|v| !v.is_null());
            let Some(method) = method else {
                if let Some(id_str) = id.as_ref().and_then(Value::as_str)
                    && let Some(tx) = state.pending.lock().unwrap().remove(id_str)
                {
                    if let Some(err) = frame.get("error") {
                        let msg = err.get("message").and_then(Value::as_str).unwrap_or("error").to_owned();
                        let _ = tx.send(Err(msg));
                    } else {
                        let _ = tx.send(Ok(frame.get("result").cloned().unwrap_or(Value::Null)));
                    }
                }
                continue;
            };
            let params = frame.get("params").cloned().unwrap_or_else(|| json!({}));
            let st = state.clone();
            tokio::spawn(async move {
                let outcome = st.handle(&method, params).await;
                let Some(id) = id else { return };
                match outcome {
                    Ok(result) => st.write_frame(json!({ "id": id, "result": result })),
                    Err(err) => st.write_frame(json!({ "id": id, "error": { "code": err.code, "message": err.message } })),
                }
            });
        }
        watcher.abort();
        drop(state);
        let _ = writer_task.await;
    });
    ExitCode::SUCCESS
}

// =================================================================================================
// Shared Chat Helpers (control files, bridge log, polling, RFC 6455 WebSocket client)
// =================================================================================================

pub fn read_bridge_log(control: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(control.join("log.jsonl")) else {
        return Vec::new();
    };
    text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect()
}

pub fn write_control(control: &Path, account_id: &str, action: &str, data: Option<&Value>) -> Result<()> {
    let name = match data {
        Some(_) => format!("{account_id}.{action}.json"),
        None => format!("{account_id}.{action}"),
    };
    let content = match data {
        Some(v) => v.to_string(),
        None => String::new(),
    };
    std::fs::write(control.join(name), content)?;
    Ok(())
}

pub async fn until<T, F, Fut>(timeout: Duration, what: &str, mut read: F, mut ok: impl FnMut(&T) -> bool) -> Result<T>
where
    T: std::fmt::Debug,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let deadline = Instant::now() + timeout;
    let mut last: Option<T> = None;
    while Instant::now() < deadline {
        let value = read().await?;
        if ok(&value) {
            return Ok(value);
        }
        last = Some(value);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("Timed out waiting for {what}: {last:?}");
}

pub fn sql_text(rows: &Value) -> String {
    rows.to_string()
}

/// Minimal RFC 6455 WebSocket client over plaintext TCP (for `chat-discord` live voice tests).
pub struct WsClient {
    writer: tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>,
    pub texts: Arc<Mutex<Vec<Value>>>,
    pub audio: Arc<Mutex<Vec<Vec<u8>>>>,
    reader_task: tokio::task::JoinHandle<()>,
}

impl WsClient {
    /// Attempts to connect; returns `Ok(WsClient)` on HTTP 101 or `Err` if refused.
    pub async fn connect(url: &str, headers: &[(&str, &str)]) -> Result<Self> {
        let parsed = url::Url::parse(url)?;
        let host = parsed.host_str().context("no ws host")?;
        let port = parsed.port_or_known_default().context("no ws port")?;
        let mut path = parsed.path().to_owned();
        if let Some(q) = parsed.query() {
            path.push('?');
            path.push_str(q);
        }
        let mut stream = TcpStream::connect((host, port)).await?;
        let key = b64_encode(&dar_acceptance::random_bytes(16));
        let mut req = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
        );
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).await?;
        stream.flush().await?;

        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        while !buf.ends_with(b"\r\n\r\n") {
            let n = stream.read(&mut byte).await?;
            if n == 0 {
                bail!("WebSocket handshake EOF");
            }
            buf.push(byte[0]);
            if buf.len() > 16384 {
                bail!("WebSocket handshake header too large");
            }
        }
        let header_text = String::from_utf8_lossy(&buf);
        let first_line = header_text.lines().next().unwrap_or_default();
        if !first_line.contains(" 101 ") {
            bail!("WebSocket refused: {first_line}");
        }

        let (mut read_half, write_half) = stream.into_split();
        let texts: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let audio: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let t_clone = texts.clone();
        let a_clone = audio.clone();
        let reader_task = tokio::spawn(async move {
            let mut frag_opcode = 0u8;
            let mut frag_buf: Vec<u8> = Vec::new();
            loop {
                let mut hdr = [0u8; 2];
                if read_half.read_exact(&mut hdr).await.is_err() {
                    break;
                }
                let fin = (hdr[0] & 0x80) != 0;
                let opcode = hdr[0] & 0x0f;
                let masked = (hdr[1] & 0x80) != 0;
                let mut len = (hdr[1] & 0x7f) as u64;
                if len == 126 {
                    let mut b = [0u8; 2];
                    if read_half.read_exact(&mut b).await.is_err() {
                        break;
                    }
                    len = u16::from_be_bytes(b) as u64;
                } else if len == 127 {
                    let mut b = [0u8; 8];
                    if read_half.read_exact(&mut b).await.is_err() {
                        break;
                    }
                    len = u64::from_be_bytes(b);
                }
                let mut mask = [0u8; 4];
                if masked && read_half.read_exact(&mut mask).await.is_err() {
                    break;
                }
                let mut payload = vec![0u8; len as usize];
                if read_half.read_exact(&mut payload).await.is_err() {
                    break;
                }
                if masked {
                    for (i, b) in payload.iter_mut().enumerate() {
                        *b ^= mask[i % 4];
                    }
                }
                let effective = if opcode == 0 {
                    frag_buf.extend_from_slice(&payload);
                    if !fin {
                        continue;
                    }
                    payload = std::mem::take(&mut frag_buf);
                    frag_opcode
                } else if !fin && (opcode == 1 || opcode == 2) {
                    frag_opcode = opcode;
                    frag_buf = payload;
                    continue;
                } else {
                    opcode
                };
                match effective {
                    1 => {
                        if let Ok(val) = serde_json::from_slice::<Value>(&payload) {
                            t_clone.lock().unwrap().push(val);
                        }
                    }
                    2 => {
                        a_clone.lock().unwrap().push(payload);
                    }
                    8 => break,
                    _ => {}
                }
            }
        });
        Ok(Self { writer: tokio::sync::Mutex::new(write_half), texts, audio, reader_task })
    }

    async fn send_frame(&self, opcode: u8, payload: &[u8]) -> Result<()> {
        let mut frame = Vec::with_capacity(14 + payload.len());
        frame.push(0x80 | opcode);
        let len = payload.len();
        if len < 126 {
            frame.push(0x80 | (len as u8));
        } else if len < 65536 {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
        let mask_vec = dar_acceptance::random_bytes(4);
        frame.extend_from_slice(&mask_vec);
        for (i, &b) in payload.iter().enumerate() {
            frame.push(b ^ mask_vec[i % 4]);
        }
        let mut w = self.writer.lock().await;
        w.write_all(&frame).await?;
        w.flush().await?;
        Ok(())
    }

    pub async fn send_json(&self, value: &Value) -> Result<()> {
        self.send_frame(1, value.to_string().as_bytes()).await
    }

    pub async fn send_binary(&self, bytes: &[u8]) -> Result<()> {
        self.send_frame(2, bytes).await
    }

    pub fn last(&self, msg_type: &str) -> Option<Value> {
        self.texts.lock().unwrap().iter().rev().find(|v| v.get("type").and_then(Value::as_str) == Some(msg_type)).cloned()
    }

    pub async fn close(self) {
        let _ = self.send_frame(8, &[]).await;
        self.reader_task.abort();
    }
}

// =================================================================================================
// News Local Feed Fixture (`feedServer` in `test/news.integration.test.ts`)
// =================================================================================================

#[derive(Default)]
pub struct FeedState {
    pub extra: bool,
    pub failing: bool,
    pub conditional: u64,
    pub requests: u64,
}

pub struct FeedServer {
    pub site: String,
    pub state: Arc<Mutex<FeedState>>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl FeedServer {
    pub async fn start() -> Result<Self> {
        let state = Arc::new(Mutex::new(FeedState::default()));
        let app = Router::new()
            .route("/", get(feed_index))
            .route("/feed.xml", get(feed_rss))
            .route("/atom.xml", get(feed_atom))
            .route("/article", get(feed_article))
            .route("/pixel.png", get(feed_pixel))
            .route("/vector.svg", get(feed_svg))
            .fallback(get(feed_not_found))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        let site = format!("http://127.0.0.1:{}", addr.port());
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        Ok(Self { site, state, shutdown: Some(tx) })
    }

    pub fn close(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for FeedServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

fn req_base(headers: &HeaderMap) -> String {
    let host = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("127.0.0.1");
    format!("http://{host}")
}

async fn feed_index(AxumState(state): AxumState<Arc<Mutex<FeedState>>>) -> Response {
    state.lock().unwrap().requests += 1;
    (
        StatusCode::OK,
        [("content-type", "text/html")],
        r#"<html><head><link rel="alternate" type="application/rss+xml" title="Fixture News" href="/feed.xml"></head><body>Home</body></html>"#,
    )
        .into_response()
}

async fn feed_rss(AxumState(state): AxumState<Arc<Mutex<FeedState>>>, headers: HeaderMap) -> Response {
    let base = req_base(&headers);
    let (failing, extra) = {
        let mut s = state.lock().unwrap();
        s.requests += 1;
        (s.failing, s.extra)
    };
    if failing {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let etag = if extra { "\"v2\"" } else { "\"v1\"" };
    if headers.get("if-none-match").and_then(|v| v.to_str().ok()) == Some(etag) {
        state.lock().unwrap().conditional += 1;
        let mut resp = StatusCode::NOT_MODIFIED.into_response();
        resp.headers_mut().insert("etag", HeaderValue::from_str(etag).unwrap());
        return resp;
    }
    let extra_item = if extra {
        format!(
            "<item><title>Fresh arrival</title><link>{base}/posts/3</link><guid>post-3</guid><pubDate>Wed, 11 Nov 2026 08:00:00 GMT</pubDate><description>New</description></item>"
        )
    } else {
        String::new()
    };
    let body = format!(
        r#"<?xml version="1.0"?><rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/"><channel><title>Fixture News</title><link>{base}/</link>
    <item><title>Welcome to the fixture</title><link>{base}/posts/1</link><guid>post-1</guid><pubDate>Mon, 09 Nov 2026 08:00:00 GMT</pubDate><description>Short summary</description><content:encoded><![CDATA[<p>Hello <b>reader</b><script>alert(1)</script><img src="/pixel.png" onerror="x()"></p>]]></content:encoded></item>
    <item><title>Episode one</title><link>{base}/posts/2</link><guid>post-2</guid><pubDate>Tue, 10 Nov 2026 08:00:00 GMT</pubDate><description>Listen</description><enclosure url="https://media.example/ep1.mp3" type="audio/mpeg" length="1"/></item>
    {extra_item}</channel></rss>"#
    );
    (StatusCode::OK, [("content-type", "application/rss+xml"), ("etag", etag)], body).into_response()
}

async fn feed_atom(AxumState(state): AxumState<Arc<Mutex<FeedState>>>, headers: HeaderMap) -> Response {
    state.lock().unwrap().requests += 1;
    let base = req_base(&headers);
    let body = format!(
        r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Atom Blog</title><link href="{base}/blog"/><entry><id>tag:blog,1</id><title>Long read</title><link href="{base}/article"/><updated>2026-11-08T09:00:00Z</updated><summary>Only a teaser</summary></entry></feed>"#
    );
    (StatusCode::OK, [("content-type", "application/atom+xml")], body).into_response()
}

async fn feed_article(AxumState(state): AxumState<Arc<Mutex<FeedState>>>) -> Response {
    state.lock().unwrap().requests += 1;
    let nav = "Menu item. ".repeat(40);
    let paras: String = (0..6)
        .map(|i| format!("<p>Full article paragraph {i} with enough words to be counted as real content by the extractor.</p>"))
        .collect();
    let body = format!("<html><body><nav>{nav}</nav><article><h1>Long read</h1>{paras}</article></body></html>");
    (StatusCode::OK, [("content-type", "text/html; charset=utf-8")], body).into_response()
}

async fn feed_pixel(AxumState(state): AxumState<Arc<Mutex<FeedState>>>) -> Response {
    state.lock().unwrap().requests += 1;
    let bytes: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52];
    (StatusCode::OK, [("content-type", "image/png")], bytes).into_response()
}

async fn feed_svg(AxumState(state): AxumState<Arc<Mutex<FeedState>>>) -> Response {
    state.lock().unwrap().requests += 1;
    (StatusCode::OK, [("content-type", "image/svg+xml")], "<svg onload='x()'/>").into_response()
}

async fn feed_not_found(AxumState(state): AxumState<Arc<Mutex<FeedState>>>) -> Response {
    state.lock().unwrap().requests += 1;
    StatusCode::NOT_FOUND.into_response()
}

// =================================================================================================
// Fake Mail Provider (`test/support/mail-server.mjs`): DER Certs, IMAP TLS, SMTP TLS, HTTPS OAuth/JMAP
// =================================================================================================

fn der_tlv(tag: u8, parts: &[&[u8]]) -> Vec<u8> {
    let len: usize = parts.iter().map(|p| p.len()).sum();
    let mut out = Vec::with_capacity(4 + len);
    out.push(tag);
    if len < 128 {
        out.push(len as u8);
    } else if len < 256 {
        out.push(0x81);
        out.push(len as u8);
    } else {
        out.push(0x82);
        out.push((len >> 8) as u8);
        out.push((len & 0xff) as u8);
    }
    for p in parts {
        out.extend_from_slice(p);
    }
    out
}

fn der_seq(parts: &[&[u8]]) -> Vec<u8> {
    der_tlv(0x30, parts)
}
fn der_set(parts: &[&[u8]]) -> Vec<u8> {
    der_tlv(0x31, parts)
}
fn der_oid(text: &str) -> Vec<u8> {
    let nums: Vec<u32> = text.split('.').map(|s| s.parse().unwrap()).collect();
    let mut bytes = vec![(40 * nums[0] + nums[1]) as u8];
    for &v in &nums[2..] {
        let mut out = vec![(v & 127) as u8];
        let mut x = v >> 7;
        while x > 0 {
            out.insert(0, ((x & 127) as u8) | 128);
            x >>= 7;
        }
        bytes.extend_from_slice(&out);
    }
    der_tlv(0x06, &[&bytes])
}
fn der_int(bytes: &[u8]) -> Vec<u8> {
    if bytes.first().is_some_and(|b| b & 0x80 != 0) {
        let mut prefixed = Vec::with_capacity(bytes.len() + 1);
        prefixed.push(0);
        prefixed.extend_from_slice(bytes);
        der_tlv(0x02, &[&prefixed])
    } else {
        der_tlv(0x02, &[bytes])
    }
}
fn der_utf8(s: &str) -> Vec<u8> {
    der_tlv(0x0c, &[s.as_bytes()])
}
fn der_utctime(dt: chrono::DateTime<chrono::Utc>) -> Vec<u8> {
    let s = dt.format("%y%m%d%H%M%SZ").to_string();
    der_tlv(0x17, &[s.as_bytes()])
}
fn der_name(cn: &str) -> Vec<u8> {
    let cn_oid = der_oid("2.5.4.3");
    let cn_val = der_utf8(cn);
    let inner = der_seq(&[&cn_oid, &cn_val]);
    let set = der_set(&[&inner]);
    der_seq(&[&set])
}
fn der_ext(id: &str, critical: bool, value: &[u8]) -> Vec<u8> {
    let oid = der_oid(id);
    let octet = der_tlv(0x04, &[value]);
    if critical {
        let crit = der_tlv(0x01, &[&[0xff]]);
        der_seq(&[&oid, &crit, &octet])
    } else {
        der_seq(&[&oid, &octet])
    }
}
fn der_bit_string(bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(1 + bytes.len());
    body.push(0);
    body.extend_from_slice(bytes);
    der_tlv(0x03, &[&body])
}

fn gen_p256_key() -> p256::ecdsa::SigningKey {
    loop {
        let bytes = dar_acceptance::random_bytes(32);
        if let Ok(key) = p256::ecdsa::SigningKey::from_slice(&bytes) {
            return key;
        }
    }
}

fn p256_spki_der(key: &p256::ecdsa::SigningKey) -> Vec<u8> {
    let ec_pub = der_oid("1.2.840.10045.2.1");
    let prime256v1 = der_oid("1.2.840.10045.3.1.7");
    let alg = der_seq(&[&ec_pub, &prime256v1]);
    let point = key.verifying_key().to_encoded_point(false);
    let bits = der_bit_string(point.as_bytes());
    der_seq(&[&alg, &bits])
}

fn p256_pkcs8_der(key: &p256::ecdsa::SigningKey) -> Vec<u8> {
    let version = der_int(&[0]);
    let ec_pub = der_oid("1.2.840.10045.2.1");
    let prime256v1 = der_oid("1.2.840.10045.3.1.7");
    let alg = der_seq(&[&ec_pub, &prime256v1]);
    let ec_ver = der_int(&[1]);
    let priv_bytes = key.to_bytes();
    let priv_oct = der_tlv(0x04, &[&priv_bytes]);
    let point = key.verifying_key().to_encoded_point(false);
    let pub_bits = der_bit_string(point.as_bytes());
    let pub_ctx = der_tlv(0xa1, &[&pub_bits]);
    let ec_priv = der_seq(&[&ec_ver, &priv_oct, &pub_ctx]);
    let ec_priv_oct = der_tlv(0x04, &[&ec_priv]);
    der_seq(&[&version, &alg, &ec_priv_oct])
}

fn make_cert_der(subject: &str, issuer: &str, pub_spki: &[u8], signer: &p256::ecdsa::SigningKey, is_ca: bool) -> Vec<u8> {
    let ecdsa_sha256_oid = der_oid("1.2.840.10045.4.3.2");
    let ecdsa_alg = der_seq(&[&ecdsa_sha256_oid]);
    let now = chrono::Utc::now();
    let not_before = der_utctime(now - chrono::Duration::hours(1));
    let not_after = der_utctime(now + chrono::Duration::hours(24));
    let validity = der_seq(&[&not_before, &not_after]);
    let extensions: Vec<Vec<u8>> = if is_ca {
        let bc_true = der_tlv(0x01, &[&[0xff]]);
        let bc = der_seq(&[&bc_true]);
        let ku = der_tlv(0x03, &[&[1, 0x06]]);
        vec![der_ext("2.5.29.19", true, &bc), der_ext("2.5.29.15", true, &ku)]
    } else {
        let san_dns = der_tlv(0x82, &[b"localhost"]);
        let san_ip = der_tlv(0x87, &[&[127, 0, 0, 1]]);
        let san = der_seq(&[&san_dns, &san_ip]);
        let ku = der_tlv(0x03, &[&[7, 0x80]]);
        let server_auth = der_oid("1.3.6.1.5.5.7.3.1");
        let eku = der_seq(&[&server_auth]);
        vec![der_ext("2.5.29.17", false, &san), der_ext("2.5.29.15", true, &ku), der_ext("2.5.29.37", false, &eku)]
    };
    let ext_refs: Vec<&[u8]> = extensions.iter().map(Vec::as_slice).collect();
    let exts_seq = der_seq(&ext_refs);
    let exts_ctx = der_tlv(0xa3, &[&exts_seq]);
    let v3_int = der_int(&[2]);
    let v3 = der_tlv(0xa0, &[&v3_int]);
    let mut serial = dar_acceptance::random_bytes(8);
    serial[0] &= 0x7f;
    if serial[0] == 0 {
        serial[0] = 1;
    }
    let serial_der = der_int(&serial);
    let issuer_der = der_name(issuer);
    let subject_der = der_name(subject);
    let tbs = der_seq(&[&v3, &serial_der, &ecdsa_alg, &issuer_der, &validity, &subject_der, pub_spki, &exts_ctx]);
    let sig: p256::ecdsa::DerSignature = signer.sign(&tbs);
    let sig_bits = der_bit_string(sig.as_bytes());
    der_seq(&[&tbs, &ecdsa_alg, &sig_bits])
}

fn to_pem(label: &str, der: &[u8]) -> String {
    let encoded = b64_encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

pub struct MailCerts {
    pub ca_file: PathBuf,
    pub ca_der: Vec<u8>,
    pub leaf_der: Vec<u8>,
    pub leaf_key_der: Vec<u8>,
}

pub fn make_certificates(dir: &Path) -> Result<MailCerts> {
    let ca_key = gen_p256_key();
    let leaf_key = gen_p256_key();
    let ca_der = make_cert_der("Dar test mail CA", "Dar test mail CA", &p256_spki_der(&ca_key), &ca_key, true);
    let leaf_der = make_cert_der("localhost", "Dar test mail CA", &p256_spki_der(&leaf_key), &ca_key, false);
    let leaf_key_der = p256_pkcs8_der(&leaf_key);
    let ca_file = dir.join("mail-ca.pem");
    std::fs::write(&ca_file, to_pem("CERTIFICATE", &ca_der))?;
    Ok(MailCerts { ca_file, ca_der, leaf_der, leaf_key_der })
}

fn tls_server_config(certs: &MailCerts) -> Arc<rustls::ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let chain = vec![CertificateDer::from(certs.leaf_der.clone()), CertificateDer::from(certs.ca_der.clone())];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certs.leaf_key_der.clone()));
    Arc::new(
        rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap(),
    )
}

/// Sends an HTTPS GET trusting `ca_der` and returns the `Location` header (without following redirects).
pub async fn https_get_location(url: &str, ca_der: &[u8]) -> Result<String> {
    let parsed = url::Url::parse(url)?;
    let host = parsed.host_str().context("no https host")?;
    let port = parsed.port_or_known_default().unwrap_or(443);
    let mut path = parsed.path().to_owned();
    if let Some(q) = parsed.query() {
        path.push('?');
        path.push_str(q);
    }
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(ca_der.to_vec())).map_err(|e| Error::new(format!("CA cert: {e}")))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = Arc::new(
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let connector = TlsConnector::from(config);
    let tcp = TcpStream::connect((host, port)).await?;
    let server_name = ServerName::try_from(host.to_owned()).map_err(|e| Error::new(format!("server name: {e}")))?;
    let mut tls = connector.connect(server_name, tcp).await?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    tls.write_all(req.as_bytes()).await?;
    tls.flush().await?;
    let mut resp = Vec::new();
    let _ = tls.read_to_end(&mut resp).await;
    let text = String::from_utf8_lossy(&resp);
    for line in text.split("\r\n") {
        if let Some((k, v)) = line.split_once(':')
            && k.eq_ignore_ascii_case("location")
        {
            return Ok(v.trim().to_owned());
        }
    }
    bail!("no Location header in response from {url}: {text}");
}

// ---------- Mailboxes & message builder ----------

pub struct MailAttachment<'a> {
    pub name: &'a str,
    pub content_type: &'a str,
    pub data: &'a [u8],
}

pub struct MailMessageSpec<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub cc: Option<&'a str>,
    pub subject: &'a str,
    pub body: &'a str,
    pub date: chrono::DateTime<chrono::Utc>,
    pub id: Option<&'a str>,
    pub html: Option<&'a str>,
    pub attachment: Option<MailAttachment<'a>>,
    pub references: &'a [&'a str],
}

impl Default for MailMessageSpec<'_> {
    fn default() -> Self {
        Self {
            from: "Lina Karam <lina@example.test>",
            to: "Anas <anas@example.test>",
            cc: None,
            subject: "",
            body: "Hello",
            date: chrono::DateTime::parse_from_rfc3339("2026-11-20T10:24:00Z").unwrap().with_timezone(&chrono::Utc),
            id: None,
            html: None,
            attachment: None,
            references: &[],
        }
    }
}

pub fn build_mail_message(spec: MailMessageSpec<'_>) -> Vec<u8> {
    let generated_mid = format!("{}@example.test", dar_acceptance::random_hex(6));
    let mid = spec.id.unwrap_or(&generated_mid);
    let date_rfc2822 = spec.date.format("%a, %d %b %Y %H:%M:%S +0000").to_string();
    let mut head = vec![
        format!("From: {}", spec.from),
        format!("To: {}", spec.to),
    ];
    if let Some(cc) = spec.cc {
        head.push(format!("Cc: {cc}"));
    }
    head.push(format!("Subject: {}", spec.subject));
    head.push(format!("Date: {date_rfc2822}"));
    head.push(format!("Message-ID: <{mid}>"));
    if !spec.references.is_empty() {
        let refs: Vec<String> = spec.references.iter().map(|r| format!("<{r}>")).collect();
        head.push(format!("References: {}", refs.join(" ")));
        head.push(format!("In-Reply-To: <{}>", spec.references.last().unwrap()));
    }
    head.push("MIME-Version: 1.0".into());
    if spec.html.is_none() && spec.attachment.is_none() {
        head.push("Content-Type: text/plain; charset=utf-8".into());
        head.push(String::new());
        head.push(spec.body.to_owned());
        head.push(String::new());
        return head.join("\r\n").into_bytes();
    }
    let mut parts = Vec::new();
    let first_ct = if spec.html.is_some() { "text/html" } else { "text/plain" };
    let first_body = spec.html.unwrap_or(spec.body);
    parts.push(format!("--b1\r\nContent-Type: {first_ct}; charset=utf-8\r\n\r\n{first_body}\r\n"));
    if let Some(att) = spec.attachment {
        parts.push(format!(
            "--b1\r\nContent-Type: {}; name=\"{}\"\r\nContent-Disposition: attachment; filename=\"{}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
            att.content_type,
            att.name,
            att.name,
            b64_encode(att.data)
        ));
    }
    head.push("Content-Type: multipart/mixed; boundary=b1".into());
    head.push(String::new());
    head.push(format!("{}--b1--", parts.concat()));
    head.push(String::new());
    head.join("\r\n").into_bytes()
}

#[derive(Clone, Debug)]
pub struct ImapMessage {
    pub uid: u32,
    pub flags: BTreeSet<String>,
    pub data: Vec<u8>,
    pub id: String,
}

#[derive(Clone, Debug)]
pub struct ImapMailbox {
    pub name: String,
    pub special: String,
    pub validity: u32,
    pub next: u32,
    pub messages: Vec<ImapMessage>,
}

pub struct MailStore {
    pub boxes: BTreeMap<String, ImapMailbox>,
    pub box_order: Vec<String>,
    validity: u32,
    jmap_ids: u32,
}

impl MailStore {
    fn new() -> Self {
        let mut s = Self { boxes: BTreeMap::new(), box_order: Vec::new(), validity: 1000, jmap_ids: 0 };
        s.add("INBOX", "");
        s.add("Sent", "\\Sent");
        s.add("Drafts", "\\Drafts");
        s.add("Trash", "\\Trash");
        s.add("Archive", "\\Archive");
        s.add("Junk", "\\Junk");
        s.add("Projects", "");
        s
    }

    pub fn add(&mut self, name: &str, special: &str) {
        self.validity += 1;
        if !self.boxes.contains_key(name) {
            self.box_order.push(name.to_owned());
        }
        self.boxes.insert(
            name.to_owned(),
            ImapMailbox {
                name: name.to_owned(),
                special: special.to_owned(),
                validity: self.validity,
                next: 1,
                messages: Vec::new(),
            },
        );
    }

    pub fn put(&mut self, box_name: &str, data: Vec<u8>, flags: &[&str]) -> ImapMessage {
        self.jmap_ids += 1;
        let id = format!("e{}", self.jmap_ids);
        let b = self.boxes.get_mut(box_name).expect("mailbox exists");
        let uid = b.next;
        b.next += 1;
        let msg = ImapMessage { uid, flags: flags.iter().map(|s| (*s).to_owned()).collect(), data, id };
        b.messages.push(msg.clone());
        msg
    }

    pub fn find_by_text(&self, needle: &str) -> Option<(String, ImapMessage)> {
        for name in &self.box_order {
            if let Some(b) = self.boxes.get(name) {
                for m in &b.messages {
                    if String::from_utf8_lossy(&m.data).contains(needle) {
                        return Some((b.name.clone(), m.clone()));
                    }
                }
            }
        }
        None
    }

    pub fn mutate_by_text(&mut self, needle: &str, f: impl FnOnce(&mut ImapMessage)) -> bool {
        for b in self.boxes.values_mut() {
            for m in &mut b.messages {
                if String::from_utf8_lossy(&m.data).contains(needle) {
                    f(m);
                    return true;
                }
            }
        }
        false
    }

    pub fn remove_by_text(&mut self, needle: &str) -> bool {
        for b in self.boxes.values_mut() {
            if let Some(pos) = b.messages.iter().position(|m| String::from_utf8_lossy(&m.data).contains(needle)) {
                b.messages.remove(pos);
                return true;
            }
        }
        false
    }
}

#[derive(Clone, Debug)]
pub struct SentMessage {
    pub user: Option<String>,
    pub from: String,
    pub to: Vec<String>,
    pub data: String,
}

#[derive(Clone, Debug)]
pub struct AppendedMessage {
    pub box_name: String,
    pub flags: Vec<String>,
    pub data: String,
}

#[derive(Clone, Debug)]
pub struct OAuthCode {
    pub challenge: String,
    pub redirect: String,
    pub user: String,
}

pub struct OAuthState {
    pub secret: String,
    pub codes: HashMap<String, OAuthCode>,
    pub access: HashMap<String, String>,
    pub refresh: HashMap<String, String>,
    pub authorizations: Vec<HashMap<String, String>>,
    pub token_requests: Vec<HashMap<String, String>>,
    pub expires_in: u64,
}

#[derive(Clone, Debug)]
pub struct JmapMailbox {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
    pub parent_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct JmapEmail {
    pub id: String,
    pub blob_id: String,
    pub mailbox_ids: BTreeMap<String, bool>,
    pub keywords: BTreeSet<String>,
}

#[derive(Clone, Debug)]
pub struct JmapSubmission {
    pub envelope: Value,
    pub data: String,
}

pub struct JmapState {
    pub token: String,
    pub address: String,
    pub mailboxes: BTreeMap<String, JmapMailbox>,
    pub emails: BTreeMap<String, JmapEmail>,
    pub email_order: Vec<String>,
    pub blobs: HashMap<String, Vec<u8>>,
    pub calls: Vec<String>,
    pub submissions: Vec<JmapSubmission>,
}

impl JmapState {
    pub fn add(&mut self, mailbox: &str, data: Vec<u8>, keywords: &[&str]) -> JmapEmail {
        let blob_id = format!("B{}", dar_acceptance::random_hex(6));
        self.blobs.insert(blob_id.clone(), data);
        let id = format!("e{}", dar_acceptance::random_hex(4));
        let mut mailbox_ids = BTreeMap::new();
        mailbox_ids.insert(mailbox.to_owned(), true);
        let email = JmapEmail {
            id: id.clone(),
            blob_id,
            mailbox_ids,
            keywords: keywords.iter().map(|s| (*s).to_owned()).collect(),
        };
        self.email_order.push(id.clone());
        self.emails.insert(id, email.clone());
        email
    }

    pub fn find_by_text(&self, needle: &str) -> Option<JmapEmail> {
        for id in &self.email_order {
            if let Some(e) = self.emails.get(id)
                && let Some(blob) = self.blobs.get(&e.blob_id)
                && String::from_utf8_lossy(blob).contains(needle)
            {
                return Some(e.clone());
            }
        }
        None
    }
}

pub struct MailState {
    pub users: HashMap<String, String>,
    pub mail: MailStore,
    pub sent: Vec<SentMessage>,
    pub appended: Vec<AppendedMessage>,
    pub moves: Vec<(String, String, usize)>,
    pub created: Vec<String>,
    pub expunged: usize,
    pub imap_commands: Vec<String>,
    pub imap_connections: usize,
    pub oauth: OAuthState,
    pub jmap: JmapState,
}

impl MailState {
    fn new() -> Self {
        let mut users = HashMap::new();
        users.insert("anas@example.test".into(), "imap secret password".into());
        users.insert("oauth@example.test".into(), "unused".into());
        let mut mailboxes = BTreeMap::new();
        for (id, name, role) in [("mi", "Inbox", "inbox"), ("ms", "Sent", "sent"), ("mt", "Trash", "trash"), ("ma", "Archive", "archive")] {
            mailboxes.insert(id.into(), JmapMailbox { id: id.into(), name: name.into(), role: Some(role.into()), parent_id: None });
        }
        Self {
            users,
            mail: MailStore::new(),
            sent: Vec::new(),
            appended: Vec::new(),
            moves: Vec::new(),
            created: Vec::new(),
            expunged: 0,
            imap_commands: Vec::new(),
            imap_connections: 0,
            oauth: OAuthState {
                secret: "dar-test-client-secret".into(),
                codes: HashMap::new(),
                access: HashMap::new(),
                refresh: HashMap::new(),
                authorizations: Vec::new(),
                token_requests: Vec::new(),
                expires_in: 3600,
            },
            jmap: JmapState {
                token: "jmap-test-token".into(),
                address: "jmap@example.test".into(),
                mailboxes,
                emails: BTreeMap::new(),
                email_order: Vec::new(),
                blobs: HashMap::new(),
                calls: Vec::new(),
                submissions: Vec::new(),
            },
        }
    }
}

pub struct MailServer {
    pub state: Arc<Mutex<MailState>>,
    pub ca_file: PathBuf,
    pub ca_der: Vec<u8>,
    pub imap_port: u16,
    pub smtp_port: u16,
    pub https_port: u16,
    pub https: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl MailServer {
    pub async fn start(dir: &Path) -> Result<Self> {
        let certs = make_certificates(dir)?;
        let tls_cfg = tls_server_config(&certs);
        let state = Arc::new(Mutex::new(MailState::new()));

        let imap_listener = TcpListener::bind("127.0.0.1:0").await?;
        let imap_port = imap_listener.local_addr()?.port();
        let smtp_listener = TcpListener::bind("127.0.0.1:0").await?;
        let smtp_port = smtp_listener.local_addr()?.port();
        let https_listener = TcpListener::bind("127.0.0.1:0").await?;
        let https_port = https_listener.local_addr()?.port();
        let https = format!("https://127.0.0.1:{https_port}");

        let mut tasks = Vec::new();
        {
            let acceptor = TlsAcceptor::from(tls_cfg.clone());
            let st = state.clone();
            tasks.push(tokio::spawn(async move {
                while let Ok((stream, _)) = imap_listener.accept().await {
                    let acc = acceptor.clone();
                    let st = st.clone();
                    tokio::spawn(async move {
                        if let Ok(tls) = acc.accept(stream).await {
                            let _ = handle_imap(tls, st).await;
                        }
                    });
                }
            }));
        }
        {
            let acceptor = TlsAcceptor::from(tls_cfg.clone());
            let st = state.clone();
            tasks.push(tokio::spawn(async move {
                while let Ok((stream, _)) = smtp_listener.accept().await {
                    let acc = acceptor.clone();
                    let st = st.clone();
                    tokio::spawn(async move {
                        if let Ok(tls) = acc.accept(stream).await {
                            let _ = handle_smtp(tls, st).await;
                        }
                    });
                }
            }));
        }
        {
            let acceptor = TlsAcceptor::from(tls_cfg);
            let st = state.clone();
            let base = https.clone();
            tasks.push(tokio::spawn(async move {
                while let Ok((stream, _)) = https_listener.accept().await {
                    let acc = acceptor.clone();
                    let st = st.clone();
                    let base = base.clone();
                    tokio::spawn(async move {
                        if let Ok(tls) = acc.accept(stream).await {
                            let _ = handle_https(tls, st, &base).await;
                        }
                    });
                }
            }));
        }

        Ok(Self {
            state,
            ca_file: certs.ca_file,
            ca_der: certs.ca_der,
            imap_port,
            smtp_port,
            https_port,
            https,
            tasks,
        })
    }

    pub fn close(&mut self) {
        for t in self.tasks.drain(..) {
            t.abort();
        }
    }
}

impl Drop for MailServer {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------- IMAP protocol handler ----------

fn imap_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn imap_uid_set(text: &str, all: &[u32]) -> Vec<u32> {
    let max = all.iter().copied().max().unwrap_or(0);
    let mut out = BTreeSet::new();
    for part in text.split(',') {
        let mut sp = part.splitn(2, ':');
        let a = sp.next().unwrap_or_default();
        let b = sp.next();
        let lo = if a == "*" { max } else { a.parse().unwrap_or(0) };
        let hi = match b {
            None => lo,
            Some("*") => max,
            Some(v) => v.parse().unwrap_or(0),
        };
        let (min_v, max_v) = (lo.min(hi), lo.max(hi));
        for &u in all {
            if u >= min_v && u <= max_v {
                out.insert(u);
            }
        }
    }
    out.into_iter().collect()
}

fn imap_tokens(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ' ' {
            i += 1;
            continue;
        }
        if chars[i] == '"' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                s.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.push(s);
            continue;
        }
        if chars[i] == '(' {
            let start = i;
            let mut depth = 0usize;
            while i < chars.len() {
                if chars[i] == '(' {
                    depth += 1;
                } else if chars[i] == ')' {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i] != ' ' {
            i += 1;
        }
        out.push(chars[start..i].iter().collect());
    }
    out
}

fn imap_authenticate(state: &MailState, mechanism: &str, data: &str) -> Option<String> {
    let raw = String::from_utf8(b64_decode(data).ok()?).ok()?;
    if mechanism == "PLAIN" {
        let parts: Vec<&str> = raw.split('\0').collect();
        if parts.len() >= 3 && state.users.get(parts[1]).is_some_and(|p| p == parts[2]) {
            return Some(parts[1].to_owned());
        }
        return None;
    }
    let re = regex::Regex::new("^user=([^\x01]*)\x01auth=Bearer ([^\x01]*)\x01\x01$").unwrap();
    let caps = re.captures(&raw)?;
    let (u, tok) = (&caps[1], &caps[2]);
    if state.oauth.access.get(tok).is_some_and(|owner| owner == u) {
        Some(u.to_owned())
    } else {
        None
    }
}

async fn handle_imap<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, state: Arc<Mutex<MailState>>) -> Result<()> {
    state.lock().unwrap().imap_connections += 1;
    stream.write_all(b"* OK Dar fake IMAP ready\r\n").await?;
    stream.flush().await?;
    let caps = "CAPABILITY IMAP4rev1 AUTH=PLAIN AUTH=XOAUTH2 SASL-IR MOVE UIDPLUS LITERAL+ SPECIAL-USE";
    let literal_re = regex::Regex::new(r"\{(\d+)(\+?)\}$").unwrap();
    let mut buf: Vec<u8> = Vec::new();
    let mut user: Option<String> = None;
    let mut selected: Option<String> = None;
    let mut tmp = [0u8; 4096];

    loop {
        // Check if we have a full line in `buf`.
        let Some(crlf) = buf.windows(2).position(|w| w == b"\r\n") else {
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&tmp[..n]);
            continue;
        };
        let line = String::from_utf8_lossy(&buf[..crlf]).into_owned();
        buf.drain(..crlf + 2);

        let (cmd_line, literal) = if let Some(cap) = literal_re.captures(&line) {
            let size: usize = cap[1].parse().unwrap_or(0);
            let plus = !cap[2].is_empty();
            let prefix = line[..cap.get(0).unwrap().start()].trim_end().to_owned();
            if !plus {
                stream.write_all(b"+ go ahead\r\n").await?;
                stream.flush().await?;
            }
            while buf.len() < size + 2 {
                let n = stream.read(&mut tmp).await?;
                if n == 0 {
                    return Ok(());
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let lit: Vec<u8> = buf.drain(..size).collect();
            // Consume trailing CRLF after literal.
            if let Some(end) = buf.windows(2).position(|w| w == b"\r\n") {
                buf.drain(..end + 2);
            }
            (prefix, Some(lit))
        } else {
            (line, None)
        };

        let toks = imap_tokens(&cmd_line);
        if toks.len() < 2 {
            continue;
        }
        let tag = toks[0].clone();
        let mut command = toks[1].to_ascii_uppercase();
        let mut args: Vec<String> = toks[2..].to_vec();
        if command == "UID" && !args.is_empty() {
            command = format!("UID {}", args[0].to_ascii_uppercase());
            args.remove(0);
        }

        let (response, close) = {
            let mut st = state.lock().unwrap();
            st.imap_commands.push(command.clone());
            let mut out: Vec<u8> = Vec::new();
            let ok = |out: &mut Vec<u8>, text: &str, code: &str| {
                if code.is_empty() {
                    out.extend_from_slice(format!("{tag} OK {text}\r\n").as_bytes());
                } else {
                    out.extend_from_slice(format!("{tag} OK [{code}] {text}\r\n").as_bytes());
                }
            };
            let no = |out: &mut Vec<u8>, text: &str| {
                out.extend_from_slice(format!("{tag} NO {text}\r\n").as_bytes());
            };
            let mut close = false;
            match command.as_str() {
                "CAPABILITY" => {
                    out.extend_from_slice(format!("* {caps}\r\n").as_bytes());
                    ok(&mut out, "done", "");
                }
                "NOOP" => ok(&mut out, "done", ""),
                "LOGOUT" => {
                    out.extend_from_slice(b"* BYE\r\n");
                    ok(&mut out, "done", "");
                    close = true;
                }
                "LOGIN" => {
                    if args.len() >= 2 && st.users.get(&args[0]).is_some_and(|p| p == &args[1]) {
                        user = Some(args[0].clone());
                        ok(&mut out, "done", "");
                    } else {
                        no(&mut out, "[AUTHENTICATIONFAILED] Invalid credentials");
                    }
                }
                "AUTHENTICATE" => {
                    let mech = args.first().map(|s| s.to_ascii_uppercase()).unwrap_or_default();
                    let data = args.get(1).map(String::as_str).unwrap_or_default();
                    if let Some(u) = imap_authenticate(&st, &mech, data) {
                        user = Some(u);
                        ok(&mut out, "done", "");
                    } else {
                        no(&mut out, "[AUTHENTICATIONFAILED] Invalid credentials");
                    }
                }
                _ if user.is_none() => no(&mut out, "Sign in first"),
                "LIST" => {
                    if args.get(1).map(String::as_str) == Some("") {
                        out.extend_from_slice(b"* LIST (\\Noselect) \"/\" \"\"\r\n");
                    } else {
                        let all_names: Vec<String> = st.mail.box_order.clone();
                        for name in &all_names {
                            let Some(b) = st.mail.boxes.get(name) else { continue };
                            let prefix = format!("{}/", b.name);
                            let has_children = b.name.contains('/') || all_names.iter().any(|k| k.starts_with(&prefix));
                            let mut attrs = Vec::new();
                            if !b.special.is_empty() {
                                attrs.push(b.special.as_str());
                            }
                            attrs.push(if has_children { "\\HasChildren" } else { "\\HasNoChildren" });
                            out.extend_from_slice(format!("* LIST ({}) \"/\" {}\r\n", attrs.join(" "), imap_string(&b.name)).as_bytes());
                        }
                    }
                    ok(&mut out, "done", "");
                }
                "SUBSCRIBE" => ok(&mut out, "done", ""),
                "SELECT" | "EXAMINE" => {
                    let box_name = args.first().cloned().unwrap_or_default();
                    if let Some(b) = st.mail.boxes.get(&box_name) {
                        selected = Some(box_name);
                        out.extend_from_slice(
                            format!("* {} EXISTS\r\n* OK [UIDVALIDITY {}] UIDs valid\r\n* OK [UIDNEXT {}] next\r\n", b.messages.len(), b.validity, b.next)
                                .as_bytes(),
                        );
                        ok(&mut out, "selected", "READ-WRITE");
                    } else {
                        selected = None;
                        no(&mut out, "No such mailbox");
                    }
                }
                "CREATE" => {
                    let name = args.first().cloned().unwrap_or_default();
                    if st.mail.boxes.contains_key(&name) {
                        no(&mut out, "[ALREADYEXISTS] exists");
                    } else {
                        st.mail.add(&name, "");
                        st.created.push(name);
                        ok(&mut out, "done", "");
                    }
                }
                "RENAME" => {
                    let from = args.first().cloned().unwrap_or_default();
                    let to = args.get(1).cloned().unwrap_or_default();
                    if let Some(mut b) = st.mail.boxes.remove(&from) {
                        b.name = to.clone();
                        st.mail.boxes.insert(to.clone(), b);
                        if let Some(pos) = st.mail.box_order.iter().position(|n| n == &from) {
                            st.mail.box_order[pos] = to;
                        }
                        ok(&mut out, "done", "");
                    } else {
                        no(&mut out, "missing");
                    }
                }
                "APPEND" => {
                    let box_name = args.first().cloned().unwrap_or_default();
                    if !st.mail.boxes.contains_key(&box_name) {
                        no(&mut out, "[TRYCREATE] No such mailbox");
                    } else {
                        let flags: Vec<String> = args
                            .get(1)
                            .filter(|s| s.starts_with('(') && s.ends_with(')'))
                            .map(|s| s[1..s.len() - 1].split_whitespace().map(str::to_owned).collect())
                            .unwrap_or_default();
                        let lit = literal.unwrap_or_default();
                        let flag_refs: Vec<&str> = flags.iter().map(String::as_str).collect();
                        let m = st.mail.put(&box_name, lit.clone(), &flag_refs);
                        let validity = st.mail.boxes[&box_name].validity;
                        st.appended.push(AppendedMessage {
                            box_name,
                            flags,
                            data: String::from_utf8_lossy(&lit).into_owned(),
                        });
                        ok(&mut out, "appended", &format!("APPENDUID {validity} {}", m.uid));
                    }
                }
                _ if selected.is_none() => no(&mut out, "Select a mailbox first"),
                "UID FETCH" => {
                    let sel = selected.as_ref().unwrap();
                    if let Some(b) = st.mail.boxes.get(sel) {
                        let all: Vec<u32> = b.messages.iter().map(|m| m.uid).collect();
                        let uids = imap_uid_set(args.first().map(String::as_str).unwrap_or_default(), &all);
                        let what = args.get(1..).unwrap_or_default().join(" ").to_ascii_uppercase();
                        for (n, m) in b.messages.iter().enumerate() {
                            if !uids.contains(&m.uid) {
                                continue;
                            }
                            if what.contains("BODY.PEEK[]") {
                                out.extend_from_slice(format!("* {} FETCH (UID {} BODY[] {{{}}}\r\n", n + 1, m.uid, m.data.len()).as_bytes());
                                out.extend_from_slice(&m.data);
                                out.extend_from_slice(b")\r\n");
                            } else {
                                let flags: Vec<&str> = m.flags.iter().map(String::as_str).collect();
                                out.extend_from_slice(
                                    format!("* {} FETCH (UID {} FLAGS ({}) RFC822.SIZE {})\r\n", n + 1, m.uid, flags.join(" "), m.data.len()).as_bytes(),
                                );
                            }
                        }
                    }
                    ok(&mut out, "done", "");
                }
                "UID STORE" => {
                    let sel = selected.as_ref().unwrap();
                    if let Some(b) = st.mail.boxes.get_mut(sel) {
                        let all: Vec<u32> = b.messages.iter().map(|m| m.uid).collect();
                        let uids = imap_uid_set(args.first().map(String::as_str).unwrap_or_default(), &all);
                        let op = args.get(1).map(|s| s.to_ascii_uppercase()).unwrap_or_default();
                        let flags_arg = args.get(2).map(String::as_str).unwrap_or("()");
                        let flags: Vec<String> = flags_arg.trim_matches(['(', ')']).split_whitespace().map(str::to_owned).collect();
                        for m in &mut b.messages {
                            if uids.contains(&m.uid) {
                                for f in &flags {
                                    if op.starts_with('+') {
                                        m.flags.insert(f.clone());
                                    } else {
                                        m.flags.remove(f);
                                    }
                                }
                            }
                        }
                    }
                    ok(&mut out, "done", "");
                }
                "UID MOVE" | "UID COPY" => {
                    let sel = selected.as_ref().unwrap().clone();
                    let target = args.get(1).cloned().unwrap_or_default();
                    if !st.mail.boxes.contains_key(&target) {
                        no(&mut out, "[TRYCREATE] No such mailbox");
                    } else {
                        let all: Vec<u32> = st.mail.boxes[&sel].messages.iter().map(|m| m.uid).collect();
                        let uids = imap_uid_set(args.first().map(String::as_str).unwrap_or_default(), &all);
                        let to_copy: Vec<ImapMessage> = st.mail.boxes[&sel].messages.iter().filter(|m| uids.contains(&m.uid)).cloned().collect();
                        let mut from_uids = Vec::new();
                        let mut to_uids = Vec::new();
                        for m in to_copy {
                            let flags: Vec<String> = m.flags.iter().cloned().collect();
                            let flag_refs: Vec<&str> = flags.iter().map(String::as_str).collect();
                            let copy = st.mail.put(&target, m.data, &flag_refs);
                            from_uids.push(m.uid.to_string());
                            to_uids.push(copy.uid.to_string());
                        }
                        if command == "UID MOVE"
                            && let Some(b) = st.mail.boxes.get_mut(&sel)
                        {
                            b.messages.retain(|m| !uids.contains(&m.uid));
                        }
                        let validity = st.mail.boxes[&target].validity;
                        st.moves.push((sel, target, from_uids.len()));
                        let copyuid = format!("COPYUID {validity} {} {}", from_uids.join(","), to_uids.join(","));
                        if command == "UID MOVE" {
                            out.extend_from_slice(format!("* OK [{copyuid}] moved\r\n").as_bytes());
                            ok(&mut out, "done", "");
                        } else {
                            ok(&mut out, "copied", &copyuid);
                        }
                    }
                }
                "UID EXPUNGE" | "EXPUNGE" => {
                    let sel = selected.as_ref().unwrap();
                    let removed = if let Some(b) = st.mail.boxes.get_mut(sel) {
                        let all: Vec<u32> = b.messages.iter().map(|m| m.uid).collect();
                        let uids = if command == "UID EXPUNGE" {
                            imap_uid_set(args.first().map(String::as_str).unwrap_or_default(), &all)
                        } else {
                            all
                        };
                        let before = b.messages.len();
                        b.messages.retain(|m| !(uids.contains(&m.uid) && m.flags.contains("\\Deleted")));
                        before - b.messages.len()
                    } else {
                        0
                    };
                    st.expunged += removed;
                    ok(&mut out, "done", "");
                }
                _ => {
                    out.extend_from_slice(format!("{tag} BAD Unknown command {command}\r\n").as_bytes());
                }
            }
            (out, close)
        };
        stream.write_all(&response).await?;
        stream.flush().await?;
        if close {
            return Ok(());
        }
    }
}

// ---------- SMTP protocol handler ----------

async fn handle_smtp<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, state: Arc<Mutex<MailState>>) -> Result<()> {
    stream.write_all(b"220 Dar fake SMTP\r\n").await?;
    stream.flush().await?;
    let mut buf: Vec<u8> = Vec::new();
    let mut user: Option<String> = None;
    let mut from = String::new();
    let mut rcpt: Vec<String> = Vec::new();
    let mut in_data = false;
    let mut tmp = [0u8; 4096];
    let angle_re = regex::Regex::new(r"<([^>]*)>").unwrap();
    let xoauth_re = regex::Regex::new("^user=([^\x01]*)\x01auth=Bearer ([^\x01]*)").unwrap();

    loop {
        if in_data {
            if let Some(end) = buf.windows(5).position(|w| w == b"\r\n.\r\n") {
                let raw = String::from_utf8_lossy(&buf[..end]).into_owned();
                buf.drain(..end + 5);
                let unstuffed = raw.replace("\r\n..", "\r\n.");
                state.lock().unwrap().sent.push(SentMessage {
                    user: user.clone(),
                    from: from.clone(),
                    to: std::mem::take(&mut rcpt),
                    data: unstuffed,
                });
                in_data = false;
                stream.write_all(b"250 queued\r\n").await?;
                stream.flush().await?;
                continue;
            }
        } else if let Some(crlf) = buf.windows(2).position(|w| w == b"\r\n") {
            let line = String::from_utf8_lossy(&buf[..crlf]).into_owned();
            buf.drain(..crlf + 2);
            let upper = line.to_ascii_uppercase();
            if upper.starts_with("EHLO") {
                stream.write_all(b"250-fake\r\n250-AUTH PLAIN LOGIN XOAUTH2\r\n250 8BITMIME\r\n").await?;
            } else if upper.starts_with("AUTH PLAIN ") {
                let decoded = b64_decode(&line[11..]).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
                let parts: Vec<&str> = decoded.split('\0').collect();
                let ok = parts.len() >= 3 && state.lock().unwrap().users.get(parts[1]).is_some_and(|p| p == parts[2]);
                if ok {
                    user = Some(parts[1].to_owned());
                    stream.write_all(b"235 ok\r\n").await?;
                } else {
                    stream.write_all(b"535 bad credentials\r\n").await?;
                }
            } else if upper.starts_with("AUTH XOAUTH2 ") {
                let decoded = b64_decode(&line[13..]).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
                let authed = xoauth_re.captures(&decoded).and_then(|c| {
                    let (u, tok) = (c[1].to_owned(), c[2].to_owned());
                    if state.lock().unwrap().oauth.access.get(&tok).is_some_and(|owner| owner == &u) {
                        Some(u)
                    } else {
                        None
                    }
                });
                if let Some(u) = authed {
                    user = Some(u);
                    stream.write_all(b"235 ok\r\n").await?;
                } else {
                    stream.write_all(b"535 bad token\r\n").await?;
                }
            } else if upper.starts_with("QUIT") {
                stream.write_all(b"221 bye\r\n").await?;
                stream.flush().await?;
                return Ok(());
            } else if user.is_none() {
                stream.write_all(b"530 sign in first\r\n").await?;
            } else if upper.starts_with("MAIL FROM:") {
                if let Some(c) = angle_re.captures(&line) {
                    from = c[1].to_owned();
                }
                stream.write_all(b"250 ok\r\n").await?;
            } else if upper.starts_with("RCPT TO:") {
                if let Some(c) = angle_re.captures(&line) {
                    rcpt.push(c[1].to_owned());
                }
                stream.write_all(b"250 ok\r\n").await?;
            } else if upper == "DATA" {
                in_data = true;
                stream.write_all(b"354 go\r\n").await?;
            } else {
                stream.write_all(b"500 what\r\n").await?;
            }
            stream.flush().await?;
            continue;
        }
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

// ---------- HTTPS OAuth2 + JMAP protocol handler ----------

async fn read_http_request<S: AsyncRead + Unpin>(stream: &mut S, buf: &mut Vec<u8>) -> Result<Option<(String, String, HashMap<String, String>, Vec<u8>)>> {
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    buf.drain(..header_end + 4);
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or("/").to_owned();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    let body = if headers.get("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
        let mut out = Vec::new();
        loop {
            let line_end = loop {
                if let Some(pos) = buf.windows(2).position(|w| w == b"\r\n") {
                    break pos;
                }
                let n = stream.read(&mut tmp).await?;
                if n == 0 {
                    return Ok(None);
                }
                buf.extend_from_slice(&tmp[..n]);
            };
            let size_str = String::from_utf8_lossy(&buf[..line_end]);
            let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
            buf.drain(..line_end + 2);
            if size == 0 {
                while buf.len() < 2 {
                    let n = stream.read(&mut tmp).await?;
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                if buf.len() >= 2 {
                    buf.drain(..2);
                }
                break;
            }
            while buf.len() < size + 2 {
                let n = stream.read(&mut tmp).await?;
                if n == 0 {
                    return Ok(None);
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            out.extend_from_slice(&buf[..size]);
            buf.drain(..size + 2);
        }
        out
    } else {
        let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        while buf.len() < len {
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                return Ok(None);
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        buf.drain(..len).collect()
    };
    Ok(Some((method, target, headers, body)))
}

async fn write_http_response<S: AsyncWrite + Unpin>(stream: &mut S, status: u16, extra_headers: &[(&str, &str)], body: &[u8]) -> Result<()> {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in extra_headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}

async fn handle_https<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, state: Arc<Mutex<MailState>>, base: &str) -> Result<()> {
    let mut buf = Vec::new();
    while let Some((_method, target, headers, body)) = read_http_request(&mut stream, &mut buf).await? {
        let full_url = format!("{base}{target}");
        let parsed = url::Url::parse(&full_url).unwrap_or_else(|_| url::Url::parse(base).unwrap());
        let pathname = parsed.path().to_owned();
        let close = headers.get("connection").is_some_and(|v| v.eq_ignore_ascii_case("close"));

        if pathname == "/oauth/authorize" {
            let params: HashMap<String, String> = parsed.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
            let redirect = params.get("redirect_uri").cloned().unwrap_or_default();
            let state_param = params.get("state").cloned().unwrap_or_default();
            let code = dar_acceptance::random_hex(8);
            {
                let mut st = state.lock().unwrap();
                st.oauth.authorizations.push(params.clone());
                st.oauth.codes.insert(
                    code.clone(),
                    OAuthCode {
                        challenge: params.get("code_challenge").cloned().unwrap_or_default(),
                        redirect: redirect.clone(),
                        user: params.get("login_hint").cloned().unwrap_or_default(),
                    },
                );
            }
            let encoded_state: String = url::form_urlencoded::byte_serialize(state_param.as_bytes()).collect();
            let location = format!("{redirect}?code={code}&state={encoded_state}");
            write_http_response(&mut stream, 302, &[("Location", &location)], &[]).await?;
        } else if pathname == "/oauth/token" {
            let form: HashMap<String, String> = url::form_urlencoded::parse(&body).map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
            let (status, payload) = {
                let mut st = state.lock().unwrap();
                st.oauth.token_requests.push(form.clone());
                if form.get("client_id").map(String::as_str) != Some("dar-test-client")
                    || form.get("client_secret").map(String::as_str) != Some(st.oauth.secret.as_str())
                {
                    (401, json!({ "error": "invalid_client" }))
                } else {
                    let grant = form.get("grant_type").map(String::as_str).unwrap_or_default();
                    let user_res: std::result::Result<String, (u16, Value)> = if grant == "authorization_code" {
                        let code = form.get("code").cloned().unwrap_or_default();
                        let pending = st.oauth.codes.remove(&code);
                        let verifier = form.get("code_verifier").map(String::as_str).unwrap_or_default();
                        use sha2::Digest as _;
                        let digest = sha2::Sha256::digest(verifier.as_bytes());
                        let challenge = dar_acceptance::base64url(&digest);
                        let redirect = form.get("redirect_uri").map(String::as_str).unwrap_or_default();
                        match pending {
                            Some(p) if p.challenge == challenge && p.redirect == redirect => Ok(p.user),
                            _ => Err((400, json!({ "error": "invalid_grant" }))),
                        }
                    } else if grant == "refresh_token" {
                        let rt = form.get("refresh_token").cloned().unwrap_or_default();
                        st.oauth.refresh.get(&rt).cloned().ok_or((400, json!({ "error": "invalid_grant" })))
                    } else {
                        Err((400, json!({ "error": "unsupported_grant_type" })))
                    };
                    match user_res {
                        Ok(u) => {
                            let access = format!("at-{}", dar_acceptance::random_hex(8));
                            let refresh = format!("rt-{}", dar_acceptance::random_hex(8));
                            st.oauth.access.insert(access.clone(), u.clone());
                            st.oauth.refresh.insert(refresh.clone(), u);
                            (
                                200,
                                json!({ "access_token": access, "refresh_token": refresh, "expires_in": st.oauth.expires_in, "token_type": "Bearer" }),
                            )
                        }
                        Err(e) => e,
                    }
                }
            };
            let bytes = payload.to_string().into_bytes();
            write_http_response(&mut stream, status, &[("Content-Type", "application/json")], &bytes).await?;
        } else if !pathname.starts_with("/jmap/") {
            write_http_response(&mut stream, 404, &[], &[]).await?;
        } else {
            let expected_auth = format!("Bearer {}", state.lock().unwrap().jmap.token);
            if headers.get("authorization").map(String::as_str) != Some(expected_auth.as_str()) {
                let bytes = json!({ "type": "unauthorized" }).to_string().into_bytes();
                write_http_response(&mut stream, 401, &[("Content-Type", "application/json")], &bytes).await?;
            } else if pathname == "/jmap/session" {
                let bytes = json!({
                    "apiUrl": "/jmap/api",
                    "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
                    "uploadUrl": "/jmap/upload/{accountId}/",
                    "primaryAccounts": { "urn:ietf:params:jmap:mail": "A1" },
                    "capabilities": {},
                    "accounts": { "A1": {} }
                })
                .to_string()
                .into_bytes();
                write_http_response(&mut stream, 200, &[("Content-Type", "application/json")], &bytes).await?;
            } else if pathname.starts_with("/jmap/download/") {
                let blob_id = pathname.split('/').nth(4).unwrap_or_default();
                let blob = state.lock().unwrap().jmap.blobs.get(blob_id).cloned();
                if let Some(bytes) = blob {
                    write_http_response(&mut stream, 200, &[("Content-Type", "message/rfc822")], &bytes).await?;
                } else {
                    write_http_response(&mut stream, 404, &[], &[]).await?;
                }
            } else if pathname.starts_with("/jmap/upload/") {
                let id = format!("B{}", dar_acceptance::random_hex(6));
                state.lock().unwrap().jmap.blobs.insert(id.clone(), body);
                let bytes = json!({ "blobId": id, "accountId": "A1" }).to_string().into_bytes();
                write_http_response(&mut stream, 201, &[("Content-Type", "application/json")], &bytes).await?;
            } else if pathname != "/jmap/api" {
                write_http_response(&mut stream, 404, &[], &[]).await?;
            } else {
                let req: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
                let responses: Vec<Value> = {
                    let mut st = state.lock().unwrap();
                    let jmap = &mut st.jmap;
                    let calls = req.get("methodCalls").and_then(Value::as_array).cloned().unwrap_or_default();
                    calls
                        .into_iter()
                        .map(|call| {
                            let name = call[0].as_str().unwrap_or_default().to_owned();
                            let args = call[1].clone();
                            let id = call[2].clone();
                            jmap.calls.push(name.clone());
                            match name.as_str() {
                                "Mailbox/get" => {
                                    let list: Vec<Value> = jmap
                                        .mailboxes
                                        .values()
                                        .map(|m| json!({ "id": m.id, "name": m.name, "role": m.role, "parentId": m.parent_id }))
                                        .collect();
                                    json!([name, { "list": list }, id])
                                }
                                "Email/query" => {
                                    let in_box = args["filter"]["inMailbox"].as_str().unwrap_or_default();
                                    let pos = args["position"].as_u64().unwrap_or(0) as usize;
                                    let limit = args["limit"].as_u64().unwrap_or(100) as usize;
                                    let ids: Vec<String> = jmap
                                        .email_order
                                        .iter()
                                        .filter_map(|eid| jmap.emails.get(eid))
                                        .filter(|e| e.mailbox_ids.get(in_box).copied().unwrap_or(false))
                                        .map(|e| e.id.clone())
                                        .collect();
                                    let total = ids.len();
                                    let page: Vec<String> = ids.into_iter().skip(pos).take(limit).collect();
                                    json!([name, { "ids": page, "total": total }, id])
                                }
                                "Email/get" => {
                                    let req_ids = args["ids"].as_array().cloned().unwrap_or_default();
                                    let list: Vec<Value> = req_ids
                                        .iter()
                                        .filter_map(Value::as_str)
                                        .filter_map(|eid| jmap.emails.get(eid))
                                        .map(|e| {
                                            let size = jmap.blobs.get(&e.blob_id).map(Vec::len).unwrap_or(0);
                                            let kw: serde_json::Map<String, Value> = e.keywords.iter().map(|k| (k.clone(), json!(true))).collect();
                                            json!({ "id": e.id, "blobId": e.blob_id, "size": size, "keywords": kw, "mailboxIds": e.mailbox_ids })
                                        })
                                        .collect();
                                    json!([name, { "list": list }, id])
                                }
                                "Email/set" => {
                                    let mut updated = serde_json::Map::new();
                                    if let Some(updates) = args.get("update").and_then(Value::as_object) {
                                        for (eid, patch) in updates {
                                            updated.insert(eid.clone(), Value::Null);
                                            if let Some(e) = jmap.emails.get_mut(eid)
                                                && let Some(patch_obj) = patch.as_object()
                                            {
                                                for (key, value) in patch_obj {
                                                    if let Some((field, item)) = key.split_once('/') {
                                                        let truthy = !value.is_null() && *value != json!(false);
                                                        if field == "keywords" {
                                                            if truthy {
                                                                e.keywords.insert(item.to_owned());
                                                            } else {
                                                                e.keywords.remove(item);
                                                            }
                                                        } else if field == "mailboxIds" {
                                                            if truthy {
                                                                e.mailbox_ids.insert(item.to_owned(), true);
                                                            } else {
                                                                e.mailbox_ids.remove(item);
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    let destroyed: Vec<String> = args
                                        .get("destroy")
                                        .and_then(Value::as_array)
                                        .map(|arr| arr.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                                        .unwrap_or_default();
                                    for eid in &destroyed {
                                        jmap.emails.remove(eid);
                                        jmap.email_order.retain(|x| x != eid);
                                    }
                                    json!([name, { "updated": updated, "destroyed": destroyed }, id])
                                }
                                "Email/import" => {
                                    let mut created = serde_json::Map::new();
                                    if let Some(emails) = args.get("emails").and_then(Value::as_object) {
                                        for (k, spec) in emails {
                                            let eid = format!("e{}", dar_acceptance::random_hex(4));
                                            let blob_id = spec["blobId"].as_str().unwrap_or_default().to_owned();
                                            let mailbox_ids: BTreeMap<String, bool> = spec["mailboxIds"]
                                                .as_object()
                                                .map(|m| m.iter().map(|(mk, mv)| (mk.clone(), mv.as_bool().unwrap_or(true))).collect())
                                                .unwrap_or_default();
                                            let keywords: BTreeSet<String> = spec["keywords"]
                                                .as_object()
                                                .map(|m| m.keys().cloned().collect())
                                                .unwrap_or_default();
                                            jmap.email_order.push(eid.clone());
                                            jmap.emails.insert(eid.clone(), JmapEmail { id: eid.clone(), blob_id, mailbox_ids, keywords });
                                            created.insert(k.clone(), json!({ "id": eid }));
                                        }
                                    }
                                    json!([name, { "created": created }, id])
                                }
                                "Mailbox/set" => {
                                    let mut created = serde_json::Map::new();
                                    if let Some(creates) = args.get("create").and_then(Value::as_object) {
                                        for (k, spec) in creates {
                                            let mid = format!("m{}", dar_acceptance::random_hex(3));
                                            jmap.mailboxes.insert(
                                                mid.clone(),
                                                JmapMailbox {
                                                    id: mid.clone(),
                                                    name: spec["name"].as_str().unwrap_or_default().to_owned(),
                                                    parent_id: spec["parentId"].as_str().map(str::to_owned),
                                                    role: None,
                                                },
                                            );
                                            created.insert(k.clone(), json!({ "id": mid }));
                                        }
                                    }
                                    if let Some(updates) = args.get("update").and_then(Value::as_object) {
                                        for (mid, patch) in updates {
                                            if let Some(m) = jmap.mailboxes.get_mut(mid) {
                                                if let Some(n) = patch.get("name").and_then(Value::as_str) {
                                                    m.name = n.to_owned();
                                                }
                                                if patch.get("parentId").is_some() {
                                                    m.parent_id = patch["parentId"].as_str().map(str::to_owned);
                                                }
                                            }
                                        }
                                    }
                                    json!([name, { "created": created, "updated": {} }, id])
                                }
                                "Identity/get" => {
                                    json!([name, { "list": [{ "id": "I1", "email": jmap.address, "name": "JMAP Tester" }] }, id])
                                }
                                "EmailSubmission/set" => {
                                    let s = &args["create"]["s"];
                                    let email_id = s["emailId"].as_str().unwrap_or_default();
                                    let data = jmap
                                        .emails
                                        .get(email_id)
                                        .and_then(|e| jmap.blobs.get(&e.blob_id))
                                        .map(|b| String::from_utf8_lossy(b).into_owned())
                                        .unwrap_or_default();
                                    jmap.submissions.push(JmapSubmission { envelope: s["envelope"].clone(), data });
                                    json!([name, { "created": { "s": { "id": "S1" } } }, id])
                                }
                                _ => json!(["error", { "type": "unknownMethod" }, id]),
                            }
                        })
                        .collect()
                };
                let bytes = json!({ "methodResponses": responses }).to_string().into_bytes();
                write_http_response(&mut stream, 200, &[("Content-Type", "application/json")], &bytes).await?;
            }
        }
        if close {
            break;
        }
    }
    Ok(())
}
