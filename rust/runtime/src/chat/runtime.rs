//! Owns Chat's effects (`next/runtime/chat/runtime.ts`): the bridge process, its encrypted
//! key store, account states, linking, sending, Discord servers/friends/voice, and live sockets.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::WebSocket;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64_STD;
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

use crate::auth::{KeyBox, random_bytes, secrets_dir};
use crate::blobs::Blobs;
use crate::db::Database;
use crate::error::{AppError, Result};

use super::bridge::{Bridge, BridgeHandler};
use super::live::{LiveHandler, LiveSocket, spawn_live_socket};
use super::media::ChatMedia;
use super::{
    CHAT_LIMIT_DISCORD_TEXT, CHAT_READ_OPERATIONS, is_account_id, json_i64, now_ms,
    value_to_string,
};

fn reason_message(code: &str) -> &'static str {
    match code {
        "timeout" => "The code expired before it was used. Start again to get a new one.",
        "invalidPhone" => "Enter the full number with its country code, without a leading 0.",
        "pairingFailed" => "WhatsApp could not link this device. Start again.",
        "passkey" => {
            "This WhatsApp account asks for a passkey to link a device, which Dar does not support yet."
        }
        "outdated" => "WhatsApp no longer accepts this version of the bridge. Update Dar.",
        "multidevice" => "Turn on linked devices in WhatsApp, then scan again.",
        "cancelled" => "Sign-in was cancelled on the phone. Start again to get a new code.",
        "captcha" => {
            "Discord asked for a check that Dar cannot show. Sign in to Discord in its app or website first, then try again."
        }
        "connectFailed" => {
            "Dar could not reach the service. Check the server's internet connection."
        }
        _ => "WhatsApp could not link this device. Start again.",
    }
}

fn voice_end_message(reason: &str) -> &'static str {
    match reason {
        "left" | "moved" => "",
        "stopped" => "Discord disconnected.",
        "disconnected" => "You were disconnected from the call.",
        "timeout" => "Discord did not connect the call in time.",
        "connectionLost" => "The call's connection was lost.",
        "failed" => "Discord ended the call.",
        "connectFailed" => "Discord is not connected.",
        _ => "Discord ended the call.",
    }
}

/// A Discord nonce: a snowflake for now, which Discord uses to recognise a resent message.
fn discord_nonce() -> String {
    let ts = (now_ms() - 1_420_070_400_000).max(0) as u64;
    let rand = random_bytes::<3>();
    let seq = (((rand[0] as u64) << 16) | ((rand[1] as u64) << 8) | (rand[2] as u64)) & 0x3f_ffff;
    ((ts << 22) | seq).to_string()
}

/// WhatsApp message ID: `"3EB0"` followed by 18 uppercase hex characters.
fn whatsapp_message_id() -> String {
    let bytes = random_bytes::<9>();
    let mut out = String::with_capacity(22);
    out.push_str("3EB0");
    for b in bytes {
        out.push_str(&format!("{b:02X}"));
    }
    out
}

fn account_of(thread_id: &str) -> String {
    thread_id.chars().take(36).collect()
}

fn is_discord_id(value: &str) -> bool {
    (1..=25).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_digit())
}

enum StateCmd {
    Set {
        state: String,
        jid: Option<String>,
        detail: String,
        done: oneshot::Sender<()>,
    },
    Flush(oneshot::Sender<()>),
}

struct Voice {
    servers: Map<String, Value>,
    call: Option<Value>,
    sockets: HashMap<u64, LiveSocket>,
    owner: Option<u64>,
    orphan: Option<AbortHandle>,
}

impl Voice {
    fn new() -> Self {
        Self {
            servers: Map::new(),
            call: None,
            sockets: HashMap::new(),
            owner: None,
            orphan: None,
        }
    }

    fn push(&self, value: &Value) {
        for socket in self.sockets.values() {
            socket.send_text(value);
        }
    }

    fn broadcast_ownership(&self) {
        for (id, socket) in &self.sockets {
            let here = self.owner == Some(*id);
            let elsewhere = self.owner.is_some() && self.owner != Some(*id);
            socket.send_text(&json!({
                "type": "owner",
                "here": here,
                "elsewhere": elsewhere,
            }));
        }
    }
}

struct ChatInner {
    db: Database,
    key_box: KeyBox,
    bridge: Bridge,
    media: ChatMedia,
    closing: AtomicBool,
    active: AtomicBool,
    links: Mutex<HashMap<String, Value>>,
    platforms: Mutex<HashMap<String, String>>,
    queues: Mutex<HashMap<String, mpsc::UnboundedSender<StateCmd>>>,
    retries: Mutex<HashMap<String, AbortHandle>>,
    voices: Mutex<HashMap<String, Voice>>,
    purge_task: Mutex<Option<AbortHandle>>,
}

#[derive(Clone)]
pub struct ChatRuntime {
    inner: Arc<ChatInner>,
}

impl ChatRuntime {
    pub async fn open(
        db: Database,
        root: &Path,
        executable: Option<String>,
        enabled: bool,
        blobs: Blobs,
    ) -> std::io::Result<Self> {
        let key_box = KeyBox::open(&secrets_dir(root), "chat-keys")?;
        let downloads_env = root.join("chat-downloads").to_string_lossy().into_owned();
        let mut env = HashMap::new();
        env.insert("DAR_CHAT_DOWNLOADS".to_owned(), downloads_env);
        let bridge = Bridge::new(executable, env);
        let media = ChatMedia::open(db.clone(), root, blobs, bridge.clone()).await?;

        let inner = Arc::new(ChatInner {
            db,
            key_box,
            bridge: bridge.clone(),
            media,
            closing: AtomicBool::new(false),
            active: AtomicBool::new(enabled),
            links: Mutex::new(HashMap::new()),
            platforms: Mutex::new(HashMap::new()),
            queues: Mutex::new(HashMap::new()),
            retries: Mutex::new(HashMap::new()),
            voices: Mutex::new(HashMap::new()),
            purge_task: Mutex::new(None),
        });

        let runtime = Self {
            inner: inner.clone(),
        };

        // Hourly purge of expired event buffers and retry copies.
        {
            let rt = runtime.clone();
            let handle = tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(3600));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                interval.tick().await;
                loop {
                    interval.tick().await;
                    if rt.inner.closing.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = rt.worker("purge", json!({})).await;
                }
            });
            *inner.purge_task.lock().expect("purge task") = Some(handle.abort_handle());
        }

        // Start the bridge after `media` and `ChatInner` are fully initialized.
        bridge.start(Arc::new(BridgeAdapter(inner)));

        Ok(runtime)
    }

    pub(crate) async fn worker(&self, operation: &str, value: Value) -> Result<Value> {
        self.inner.worker(operation, value).await
    }

    pub(crate) async fn call(
        &self,
        session_id: &str,
        operation: &str,
        value: Value,
    ) -> Result<Value> {
        self.inner.call(session_id, operation, value).await
    }

    pub(crate) async fn overview(&self, session_id: &str) -> Result<Value> {
        let mut result = self.call(session_id, "overview", json!({})).await?;
        let status = self.inner.bridge.status();
        let detail = if self.inner.active.load(Ordering::Relaxed) {
            status.detail
        } else {
            String::new()
        };
        if let Some(obj) = result.as_object_mut() {
            obj.insert(
                "bridge".to_owned(),
                json!({ "available": status.available, "detail": detail }),
            );
        }
        Ok(result)
    }

    pub(crate) async fn link(
        &self,
        session_id: &str,
        name: Option<&str>,
        phone: Option<&str>,
        account_id: Option<&str>,
        platform: Option<&str>,
    ) -> Result<Value> {
        let status = self.inner.bridge.status();
        if !status.available {
            let detail = if status.detail.is_empty() {
                "The chat bridge is not running."
            } else {
                &status.detail
            };
            return Err(AppError::with("bridgeUnavailable", 503, detail));
        }
        let chosen = if let Some(aid) = account_id {
            self.platform_of(session_id, aid).await?
        } else {
            platform.unwrap_or("whatsapp").to_owned()
        };
        let mut args = Map::new();
        if let Some(n) = name {
            args.insert("name".to_owned(), json!(n));
        }
        if let Some(aid) = account_id {
            args.insert("accountId".to_owned(), json!(aid));
        }
        args.insert("platform".to_owned(), json!(chosen));

        let begun = self
            .call(session_id, "beginLink", Value::Object(args))
            .await?;
        let begun_id = begun["accountId"].as_str().unwrap_or_default().to_owned();
        self.inner
            .platforms
            .lock()
            .expect("platforms")
            .insert(begun_id.clone(), chosen.clone());
        self.inner.links.lock().expect("links").insert(
            begun_id.clone(),
            json!({ "accountId": begun_id, "state": "starting" }),
        );
        let bridge_phone = if chosen == "whatsapp" {
            phone.unwrap_or("")
        } else {
            ""
        };
        if let Err(err) = self
            .inner
            .bridge
            .call(
                "link",
                json!({
                    "account": begun_id,
                    "phone": bridge_phone,
                    "platform": chosen,
                }),
                Duration::from_secs(30),
            )
            .await
        {
            self.inner.links.lock().expect("links").insert(
                begun_id.clone(),
                json!({
                    "accountId": begun_id,
                    "state": "failed",
                    "reason": err.message,
                }),
            );
            return Err(err);
        }
        Ok(begun)
    }

    pub(crate) async fn link_state(&self, session_id: &str, account_id: &str) -> Result<Value> {
        let row = self
            .call(session_id, "account", json!({ "accountId": account_id }))
            .await?;
        if let Some(link) = self
            .inner
            .links
            .lock()
            .expect("links")
            .get(account_id)
            .cloned()
        {
            return Ok(link);
        }
        if row["state"].as_str() == Some("linking") {
            Ok(json!({
                "accountId": account_id,
                "state": "failed",
                "reason": reason_message("timeout"),
            }))
        } else {
            let paired = !row["jid"].as_str().unwrap_or_default().is_empty();
            Ok(json!({
                "accountId": account_id,
                "state": if paired { "paired" } else { "failed" },
            }))
        }
    }

    pub(crate) async fn cancel_link(&self, session_id: &str, account_id: &str) -> Result<Value> {
        let row = self
            .call(session_id, "account", json!({ "accountId": account_id }))
            .await?;
        self.inner.links.lock().expect("links").remove(account_id);
        let state = row["state"].as_str().unwrap_or_default();
        if state != "linking" && state != "failed" {
            return Ok(json!({ "ok": true }));
        }
        let _ = self
            .inner
            .bridge
            .call(
                "disconnect",
                json!({ "account": account_id }),
                Duration::from_secs(60),
            )
            .await;
        if row["jid"].as_str().unwrap_or_default().is_empty() {
            self.call(
                session_id,
                "deleteAccount",
                json!({ "accountId": account_id }),
            )
            .await?;
        }
        Ok(json!({ "ok": true }))
    }

    /// Unlinks Dar on WhatsApp/Discord; the history stays, read-only, until the account is deleted.
    pub(crate) async fn unlink(&self, session_id: &str, account_id: &str) -> Result<Value> {
        let row = self
            .call(session_id, "account", json!({ "accountId": account_id }))
            .await?;
        self.inner.cancel_retry(account_id);
        let mut confirmed = true;
        if row["state"].as_str() != Some("loggedOut")
            && self
                .inner
                .bridge
                .call(
                    "logout",
                    json!({ "account": account_id }),
                    Duration::from_secs(30),
                )
                .await
                .is_err()
        {
            confirmed = false;
            let _ = self
                .inner
                .bridge
                .call(
                    "disconnect",
                    json!({ "account": account_id }),
                    Duration::from_secs(60),
                )
                .await;
        }
        let detail = if confirmed {
            "unlinked"
        } else {
            "unlinkedHere"
        };
        let _ = self
            .inner
            .enqueue_state(account_id, "loggedOut", None, detail)
            .await;
        self.worker("kvClear", json!({ "accountId": account_id }))
            .await?;
        Ok(json!({ "ok": true, "confirmed": confirmed }))
    }

    pub(crate) async fn remove(&self, session_id: &str, account_id: &str) -> Result<Value> {
        let row = self
            .call(session_id, "account", json!({ "accountId": account_id }))
            .await?;
        self.inner.cancel_retry(account_id);
        self.inner.links.lock().expect("links").remove(account_id);
        let has_jid = !row["jid"].as_str().unwrap_or_default().is_empty();
        if has_jid && row["state"].as_str() != Some("loggedOut") {
            if self
                .inner
                .bridge
                .call(
                    "logout",
                    json!({ "account": account_id }),
                    Duration::from_secs(30),
                )
                .await
                .is_err()
            {
                let _ = self
                    .inner
                    .bridge
                    .call(
                        "disconnect",
                        json!({ "account": account_id }),
                        Duration::from_secs(60),
                    )
                    .await;
            }
        } else {
            let _ = self
                .inner
                .bridge
                .call(
                    "disconnect",
                    json!({ "account": account_id }),
                    Duration::from_secs(60),
                )
                .await;
        }
        self.call(
            session_id,
            "deleteAccount",
            json!({ "accountId": account_id }),
        )
        .await
    }

    async fn deliver(&self, session_id: &str, thread_id: &str, target: Value) -> Result<Value> {
        let account_id = target["accountId"].as_str().unwrap_or_default();
        let platform = target["platform"].as_str().unwrap_or("whatsapp");
        let chat = target["chat"].as_str().unwrap_or_default();
        let message_id = target["message"]["messageId"].as_str().unwrap_or_default();
        let text = target["message"]["text"].as_str().unwrap_or_default();

        let mut send_params = Map::new();
        send_params.insert("account".to_owned(), json!(account_id));
        send_params.insert("chat".to_owned(), json!(chat));
        send_params.insert("text".to_owned(), json!(text));
        send_params.insert("id".to_owned(), json!(message_id));
        if let Some(quoted) = target["quoted"].as_object() {
            send_params.insert(
                "quotedId".to_owned(),
                quoted.get("id").cloned().unwrap_or(json!("")),
            );
            send_params.insert(
                "quotedSender".to_owned(),
                quoted.get("sender").cloned().unwrap_or(json!("")),
            );
            send_params.insert(
                "quotedText".to_owned(),
                quoted.get("text").cloned().unwrap_or(json!("")),
            );
        }

        let mut ok = false;
        let mut real_id = String::new();
        if let Ok(sent) = self
            .inner
            .bridge
            .call("send", Value::Object(send_params), Duration::from_secs(45))
            .await
        {
            ok = true;
            if platform == "discord"
                && let Some(sid) = sent["id"].as_str()
            {
                real_id = sid.to_owned();
            }
        }

        let mut result_params = Map::new();
        result_params.insert("threadId".to_owned(), json!(thread_id));
        result_params.insert("messageId".to_owned(), json!(message_id));
        result_params.insert("ok".to_owned(), json!(ok));
        if !real_id.is_empty() {
            result_params.insert("id".to_owned(), json!(real_id));
        }
        self.call(session_id, "sendResult", Value::Object(result_params))
            .await
    }

    async fn platform_of(&self, session_id: &str, account_id: &str) -> Result<String> {
        if let Some(known) = self
            .inner
            .platforms
            .lock()
            .expect("platforms")
            .get(account_id)
            .cloned()
        {
            return Ok(known);
        }
        let row = self
            .call(session_id, "account", json!({ "accountId": account_id }))
            .await?;
        let platform = row["platform"].as_str().unwrap_or("whatsapp").to_owned();
        self.inner
            .platforms
            .lock()
            .expect("platforms")
            .insert(account_id.to_owned(), platform.clone());
        Ok(platform)
    }

    pub(crate) async fn send(
        &self,
        session_id: &str,
        thread_id: &str,
        text: &str,
        quoted_id: Option<&str>,
    ) -> Result<Value> {
        let platform = self.platform_of(session_id, &account_of(thread_id)).await?;
        if platform == "discord" && text.chars().count() > CHAT_LIMIT_DISCORD_TEXT {
            return Err(AppError::with(
                "invalidInput",
                400,
                "Discord messages can be up to 4,000 characters.",
            ));
        }
        let message_id = if platform == "discord" {
            discord_nonce()
        } else {
            whatsapp_message_id()
        };
        let mut params = Map::new();
        params.insert("threadId".to_owned(), json!(thread_id));
        params.insert("messageId".to_owned(), json!(message_id));
        params.insert("text".to_owned(), json!(text));
        if let Some(q) = quoted_id {
            params.insert("quotedId".to_owned(), json!(q));
        }
        let target = self
            .call(session_id, "prepareSend", Value::Object(params))
            .await?;
        self.deliver(session_id, thread_id, target).await
    }

    pub(crate) async fn retry(
        &self,
        session_id: &str,
        thread_id: &str,
        message_id: &str,
    ) -> Result<Value> {
        let target = self
            .call(
                session_id,
                "retry",
                json!({ "threadId": thread_id, "messageId": message_id }),
            )
            .await?;
        self.deliver(session_id, thread_id, target).await
    }

    pub(crate) async fn mark_read(&self, session_id: &str, thread_id: &str) -> Result<Value> {
        let result = self
            .call(session_id, "markRead", json!({ "threadId": thread_id }))
            .await?;
        let platform = result["platform"].as_str().unwrap_or("whatsapp");
        let account_id = result["accountId"].as_str().unwrap_or_default().to_owned();
        let chat = result["chat"].as_str().unwrap_or_default().to_owned();
        if platform == "discord" {
            if let Some(last) = result["last"].as_str().filter(|s| !s.is_empty()) {
                let last = last.to_owned();
                let bridge = self.inner.bridge.clone();
                tokio::spawn(async move {
                    let _ = bridge
                        .call(
                            "markRead",
                            json!({ "account": account_id, "chat": chat, "last": last }),
                            Duration::from_secs(60),
                        )
                        .await;
                });
            }
        } else if let Some(items) = result["items"].as_array().filter(|a| !a.is_empty()) {
            let items = items.clone();
            let bridge = self.inner.bridge.clone();
            tokio::spawn(async move {
                let _ = bridge
                    .call(
                        "markRead",
                        json!({ "account": account_id, "chat": chat, "items": items }),
                        Duration::from_secs(60),
                    )
                    .await;
            });
        }
        Ok(json!({ "ok": true }))
    }

    /// Follows Administration -> Apps: a disabled Chat disconnects every account and keeps the data.
    pub async fn set_enabled(&self, enabled: bool) {
        if self.inner.active.swap(enabled, Ordering::AcqRel) == enabled {
            return;
        }
        if enabled {
            let inner = self.inner.clone();
            tokio::spawn(async move {
                inner.connect_all().await;
            });
            return;
        }
        {
            let mut retries = self.inner.retries.lock().expect("retries");
            for (_, handle) in retries.drain() {
                handle.abort();
            }
        }
        let accounts = self
            .worker("accounts", json!({}))
            .await
            .ok()
            .and_then(|v| v["accounts"].as_array().cloned())
            .unwrap_or_default();
        let mut tasks = Vec::with_capacity(accounts.len());
        for item in accounts {
            if let Some(id) = item["id"].as_str() {
                let bridge = self.inner.bridge.clone();
                let id = id.to_owned();
                tasks.push(tokio::spawn(async move {
                    let _ = bridge
                        .call("disconnect", json!({ "account": id }), Duration::from_secs(60))
                        .await;
                }));
            }
        }
        for task in tasks {
            let _ = task.await;
        }
    }

    /// Reacts to, edits or deletes a message on WhatsApp/Discord, then records the change here.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn act(
        &self,
        session_id: &str,
        thread_id: &str,
        message_id: &str,
        action: &str,
        emoji: Option<String>,
        text: Option<String>,
        remove: bool,
    ) -> Result<Value> {
        let target = self
            .call(
                session_id,
                "target",
                json!({ "threadId": thread_id, "messageId": message_id }),
            )
            .await?;
        if target["deleted"] == Value::Bool(true) {
            return Err(AppError::with("conflict", 409, "This message was deleted."));
        }
        if action != "react" && target["fromMe"] != Value::Bool(true) {
            return Err(AppError::with(
                "forbidden",
                403,
                "Only your own messages can be changed.",
            ));
        }
        let platform = target["platform"].as_str().unwrap_or("whatsapp");
        if platform == "discord" && action == "react" && emoji.as_deref().unwrap_or("").is_empty() {
            return Err(AppError::with("invalidInput", 400, "Choose a reaction."));
        }
        if platform == "discord"
            && action == "edit"
            && text.as_deref().unwrap_or("").chars().count() > CHAT_LIMIT_DISCORD_TEXT
        {
            return Err(AppError::with(
                "invalidInput",
                400,
                "Discord messages can be up to 4,000 characters.",
            ));
        }

        let mut bridge_params = Map::new();
        bridge_params.insert("account".to_owned(), target["accountId"].clone());
        bridge_params.insert("chat".to_owned(), target["chat"].clone());
        bridge_params.insert("id".to_owned(), target["id"].clone());
        bridge_params.insert("sender".to_owned(), target["sender"].clone());
        if let Some(ref e) = emoji {
            bridge_params.insert("emoji".to_owned(), json!(e));
        }
        if let Some(ref t) = text {
            bridge_params.insert("text".to_owned(), json!(t));
        }
        if remove {
            bridge_params.insert("remove".to_owned(), json!(true));
        }

        self.inner
            .bridge
            .call(action, Value::Object(bridge_params), Duration::from_secs(45))
            .await?;

        if action == "react" && platform == "discord" {
            return self
                .call(
                    session_id,
                    "applyDiscordReaction",
                    json!({
                        "threadId": thread_id,
                        "messageId": message_id,
                        "emoji": emoji.unwrap_or_default(),
                        "remove": remove,
                    }),
                )
                .await;
        }
        if action == "react" {
            return self
                .call(
                    session_id,
                    "applyReaction",
                    json!({
                        "threadId": thread_id,
                        "messageId": message_id,
                        "emoji": emoji.unwrap_or_default(),
                    }),
                )
                .await;
        }
        if action == "edit" {
            return self
                .call(
                    session_id,
                    "applyEdit",
                    json!({
                        "threadId": thread_id,
                        "messageId": message_id,
                        "text": text.unwrap_or_default(),
                    }),
                )
                .await;
        }
        self.call(
            session_id,
            "applyRevoke",
            json!({ "threadId": thread_id, "messageId": message_id }),
        )
        .await
    }

    /// Mutes, pins or archives a chat on WhatsApp/Discord too, so the phone follows.
    pub(crate) async fn chat_setting(
        &self,
        session_id: &str,
        thread_id: &str,
        pinned: Option<bool>,
        archived: Option<bool>,
        muted_until: Option<i64>,
    ) -> Result<Value> {
        let target = self
            .call(session_id, "chatTarget", json!({ "threadId": thread_id }))
            .await?;
        if target["platform"].as_str() == Some("discord") {
            let Some(muted_until) = muted_until else {
                return Err(AppError::with(
                    "invalidInput",
                    400,
                    "Discord chats can be muted, not pinned or archived.",
                ));
            };
            self.inner
                .bridge
                .call(
                    "chatSetting",
                    json!({
                        "account": target["accountId"],
                        "chat": target["chat"],
                        "serverId": target["serverId"],
                        "mutedUntil": muted_until,
                    }),
                    Duration::from_secs(45),
                )
                .await?;
            return self
                .call(
                    session_id,
                    "applyChatSetting",
                    json!({ "threadId": thread_id, "mutedUntil": muted_until }),
                )
                .await;
        }

        let mut bridge_params = Map::new();
        bridge_params.insert("account".to_owned(), target["accountId"].clone());
        bridge_params.insert("chat".to_owned(), target["chat"].clone());
        bridge_params.insert("lastAt".to_owned(), json!(json_i64(target.get("lastAt"))));
        bridge_params.insert("lastId".to_owned(), target["lastId"].clone());
        bridge_params.insert("lastSender".to_owned(), target["lastSender"].clone());
        bridge_params.insert("lastFromMe".to_owned(), target["lastFromMe"].clone());

        let mut apply_params = Map::new();
        apply_params.insert("threadId".to_owned(), json!(thread_id));
        if let Some(p) = pinned {
            bridge_params.insert("pinned".to_owned(), json!(p));
            apply_params.insert("pinned".to_owned(), json!(p));
        }
        if let Some(a) = archived {
            bridge_params.insert("archived".to_owned(), json!(a));
            apply_params.insert("archived".to_owned(), json!(a));
        }
        if let Some(m) = muted_until {
            bridge_params.insert("mutedUntil".to_owned(), json!(m));
            apply_params.insert("mutedUntil".to_owned(), json!(m));
        }

        self.inner
            .bridge
            .call(
                "chatSetting",
                Value::Object(bridge_params),
                Duration::from_secs(45),
            )
            .await?;
        self.call(
            session_id,
            "applyChatSetting",
            Value::Object(apply_params),
        )
        .await
    }

    /// Chat info; groups add their members from WhatsApp when it is reachable.
    pub(crate) async fn info(&self, session_id: &str, thread_id: &str) -> Result<Value> {
        let mut info = self
            .call(session_id, "chatInfo", json!({ "threadId": thread_id }))
            .await?;
        let mut group = Value::Null;
        let mut group_error = String::new();

        if info["thread"]["kind"].as_str() == Some("group")
            && info["account"]["platform"].as_str() == Some("whatsapp")
        {
            let account_id = info["account"]["id"].as_str().unwrap_or_default().to_owned();
            let chat_jid = info["thread"]["jid"].as_str().unwrap_or_default().to_owned();
            let fetched: Result<Value> = async {
                let mut value = self
                    .inner
                    .bridge
                    .call(
                        "groupInfo",
                        json!({ "account": account_id, "chat": chat_jid }),
                        Duration::from_secs(20),
                    )
                    .await?;
                let jids: Vec<Value> = value["members"]
                    .as_array()
                    .map(|list| list.iter().map(|m| m["jid"].clone()).collect())
                    .unwrap_or_default();
                let names = self
                    .call(
                        session_id,
                        "names",
                        json!({ "accountId": account_id, "jids": jids }),
                    )
                    .await?;
                if let Some(members) = value["members"].as_array_mut() {
                    for member in members.iter_mut() {
                        if let Some(jid) = member["jid"].as_str()
                            && let Some(contact_name) =
                                names.get(jid).and_then(|c| c.get("name")).and_then(Value::as_str)
                        {
                            member["name"] = json!(contact_name);
                        }
                    }
                }
                Ok(value)
            }
            .await;

            match fetched {
                Ok(g) => group = g,
                Err(error) => {
                    group_error = if error.code != "bridgeError" {
                        error.message
                    } else {
                        "Members are shown when WhatsApp is connected.".to_owned()
                    };
                }
            }
        }

        if let Some(obj) = info.as_object_mut() {
            obj.insert("group".to_owned(), group);
            obj.insert("groupError".to_owned(), json!(group_error));
        }
        Ok(info)
    }

    /// Asks the phone or Discord for messages before the oldest one here.
    pub(crate) async fn request_history(
        &self,
        session_id: &str,
        thread_id: &str,
    ) -> Result<Value> {
        let oldest = self
            .call(session_id, "oldest", json!({ "threadId": thread_id }))
            .await?;
        if oldest["platform"].as_str() == Some("discord") {
            let mut params = Map::new();
            params.insert("account".to_owned(), oldest["accountId"].clone());
            params.insert("chat".to_owned(), oldest["chat"].clone());
            if let Some(msg) = oldest["message"].as_object()
                && let Some(id) = msg.get("id")
            {
                params.insert("before".to_owned(), id.clone());
            }
            let result = self
                .inner
                .bridge
                .call(
                    "requestHistory",
                    Value::Object(params),
                    Duration::from_secs(60),
                )
                .await?;
            return Ok(json!({
                "ok": true,
                "count": result["count"],
                "more": result["more"] == Value::Bool(true),
            }));
        }

        let Some(msg) = oldest["message"].as_object() else {
            return Err(AppError::with(
                "conflict",
                409,
                "There is no message to start from yet.",
            ));
        };
        let params = json!({
            "account": oldest["accountId"],
            "chat": oldest["chat"],
            "id": msg.get("id").cloned().unwrap_or(json!("")),
            "fromMe": msg.get("fromMe").cloned().unwrap_or(json!(false)),
            "at": json_i64(msg.get("at")),
        });
        self.inner
            .bridge
            .call("requestHistory", params, Duration::from_secs(30))
            .await?;
        Ok(json!({ "ok": true }))
    }

    /// Discord: fetches what was said in a chat since the newest message Dar has (or the latest page).
    pub(crate) async fn catch_up(&self, session_id: &str, thread_id: &str) -> Result<Value> {
        let newest = self
            .call(session_id, "newest", json!({ "threadId": thread_id }))
            .await?;
        if newest["platform"].as_str() != Some("discord")
            || newest["state"].as_str() != Some("connected")
        {
            return Ok(json!({ "count": 0 }));
        }
        let mut params = Map::new();
        params.insert("account".to_owned(), newest["accountId"].clone());
        params.insert("chat".to_owned(), newest["chat"].clone());
        if let Some(msg) = newest["message"].as_object()
            && let Some(id) = msg.get("id")
        {
            params.insert("after".to_owned(), id.clone());
        }
        self.inner
            .bridge
            .call(
                "requestHistory",
                Value::Object(params),
                Duration::from_secs(90),
            )
            .await
    }

    /// Discord: servers with their channels.
    pub(crate) async fn servers(&self, session_id: &str, account_id: &str) -> Result<Value> {
        self.call(session_id, "servers", json!({ "accountId": account_id }))
            .await
    }

    /// Discord: friends to start a direct message with.
    pub(crate) async fn friends(&self, session_id: &str, account_id: &str) -> Result<Value> {
        if self.platform_of(session_id, account_id).await? != "discord" {
            return Err(AppError::invalid());
        }
        self.call(session_id, "account", json!({ "accountId": account_id }))
            .await?;
        self.inner
            .bridge
            .call(
                "friends",
                json!({ "account": account_id }),
                Duration::from_secs(20),
            )
            .await
    }

    /// Opens a chat: by phone number on WhatsApp, with a friend on Discord.
    pub(crate) async fn open_chat(
        &self,
        session_id: &str,
        account_id: &str,
        phone: Option<&str>,
        user: Option<&str>,
    ) -> Result<Value> {
        if self.platform_of(session_id, account_id).await? != "discord" {
            return self
                .call(
                    session_id,
                    "openDirect",
                    json!({ "accountId": account_id, "phone": phone.unwrap_or("") }),
                )
                .await;
        }
        self.call(session_id, "account", json!({ "accountId": account_id }))
            .await?;
        let user_id = user.unwrap_or("");
        if phone.is_some() || !is_discord_id(user_id) {
            return Err(AppError::with(
                "invalidInput",
                400,
                "Choose a friend to message.",
            ));
        }
        let opened = self
            .inner
            .bridge
            .call(
                "openDirect",
                json!({ "account": account_id, "user": user_id }),
                Duration::from_secs(30),
            )
            .await?;
        self.call(
            session_id,
            "find",
            json!({ "accountId": account_id, "chat": opened["chat"] }),
        )
        .await
    }

    async fn discord_account(&self, session_id: &str, account_id: &str) -> Result<()> {
        if !is_account_id(account_id) {
            return Err(AppError::invalid());
        }
        self.call(session_id, "account", json!({ "accountId": account_id }))
            .await?;
        if self.platform_of(session_id, account_id).await? != "discord" {
            return Err(AppError::with(
                "invalidInput",
                400,
                "Voice is available for Discord accounts.",
            ));
        }
        Ok(())
    }

    /// Checks a live socket may follow this Discord account, before it is accepted.
    pub async fn authorize_live(&self, session_id: &str, account_id: &str) -> Result<()> {
        self.discord_account(session_id, account_id).await
    }

    /// Serves one browser's live socket for an account it was authorized for.
    pub fn attach_live(&self, account_id: String, socket: WebSocket) {
        let handler = Arc::new(AccountLiveHandler {
            inner: self.inner.clone(),
            account_id: account_id.clone(),
        });
        let (live, start_tx) = spawn_live_socket(socket, handler);
        {
            let mut voices = self.inner.voices.lock().expect("voices");
            let v = voices
                .entry(account_id)
                .or_insert_with(Voice::new);
            v.sockets.insert(live.id(), live.clone());
            live.send_text(&json!({
                "type": "hello",
                "servers": Value::Object(v.servers.clone()),
                "call": v.call.clone().unwrap_or(Value::Null),
            }));
            v.broadcast_ownership();
        }
        // Release the reader task only after hello & ownership frames have been queued.
        let _ = start_tx.send(());
    }

    /// Joins a Discord voice channel; the call's progress arrives on the live socket.
    pub(crate) async fn voice_join(
        &self,
        session_id: &str,
        thread_id: &str,
        muted: bool,
        deafened: bool,
    ) -> Result<Value> {
        let target = self
            .call(session_id, "chatTarget", json!({ "threadId": thread_id }))
            .await?;
        if target["platform"].as_str() != Some("discord")
            || target["serverId"].as_str().unwrap_or_default().is_empty()
            || target["kind"].as_str() != Some("voice")
        {
            return Err(AppError::with(
                "invalidInput",
                400,
                "Only Discord voice channels can be joined.",
            ));
        }
        self.inner
            .bridge
            .call(
                "voiceJoin",
                json!({
                    "account": target["accountId"],
                    "chat": target["chat"],
                    "muted": muted,
                    "deafened": deafened,
                }),
                Duration::from_secs(30),
            )
            .await?;
        Ok(json!({ "ok": true }))
    }

    pub(crate) async fn voice_leave(&self, session_id: &str, account_id: &str) -> Result<Value> {
        self.discord_account(session_id, account_id).await?;
        self.inner
            .bridge
            .call(
                "voiceLeave",
                json!({ "account": account_id }),
                Duration::from_secs(15),
            )
            .await?;
        Ok(json!({ "ok": true }))
    }

    pub(crate) async fn voice_set(
        &self,
        session_id: &str,
        account_id: &str,
        muted: bool,
        deafened: bool,
    ) -> Result<Value> {
        self.discord_account(session_id, account_id).await?;
        self.inner
            .bridge
            .call(
                "voiceSet",
                json!({
                    "account": account_id,
                    "muted": muted,
                    "deafened": deafened,
                }),
                Duration::from_secs(15),
            )
            .await?;
        Ok(json!({ "ok": true }))
    }

    pub(crate) async fn voice_code(&self, session_id: &str, account_id: &str) -> Result<Value> {
        self.discord_account(session_id, account_id).await?;
        self.inner
            .bridge
            .call(
                "voiceCode",
                json!({ "account": account_id }),
                Duration::from_secs(15),
            )
            .await
    }

    pub(crate) async fn serve_media(
        &self,
        method: &Method,
        headers: &HeaderMap,
        session_id: &str,
        thread_id: &str,
        message_id: &str,
        attachment: bool,
    ) -> Result<Response> {
        self.inner
            .media
            .serve(method, headers, session_id, thread_id, message_id, attachment)
            .await
    }

    pub(crate) async fn save_media(
        &self,
        session_id: &str,
        thread_id: &str,
        message_id: &str,
        folder_id: &str,
    ) -> Result<Value> {
        self.inner
            .media
            .save(session_id, thread_id, message_id, folder_id)
            .await
    }

    pub async fn close(&self) {
        self.inner.closing.store(true, Ordering::Release);
        self.inner.media.close();
        {
            let mut voices = self.inner.voices.lock().expect("voices");
            for v in voices.values_mut() {
                if let Some(handle) = v.orphan.take() {
                    handle.abort();
                }
                for socket in v.sockets.values() {
                    socket.close(1001, "stopping");
                }
            }
        }
        if let Some(handle) = self.inner.purge_task.lock().expect("purge task").take() {
            handle.abort();
        }
        {
            let mut retries = self.inner.retries.lock().expect("retries");
            for (_, handle) in retries.drain() {
                handle.abort();
            }
        }
        self.inner.bridge.close().await;
    }
}

struct AccountLiveHandler {
    inner: Arc<ChatInner>,
    account_id: String,
}

impl LiveHandler for AccountLiveHandler {
    fn on_text(&self, socket_id: u64, value: Map<String, Value>) {
        let mut voices = self.inner.voices.lock().expect("voices");
        let Some(v) = voices.get_mut(&self.account_id) else {
            return;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("claim") if v.call.is_some() => {
                v.owner = Some(socket_id);
                if let Some(handle) = v.orphan.take() {
                    handle.abort();
                }
                v.broadcast_ownership();
            }
            Some("release") if v.owner == Some(socket_id) => {
                v.owner = None;
                v.broadcast_ownership();
                self.inner.watch_owner_locked(&self.account_id, v);
            }
            _ => {}
        }
    }

    fn on_binary(&self, socket_id: u64, data: &[u8]) {
        // Microphone frames: 2, then one Opus packet; only from the socket that owns the call.
        if data.len() < 2 || data.len() > 1300 || data[0] != 2 {
            return;
        }
        let allow = {
            let voices = self.inner.voices.lock().expect("voices");
            voices.get(&self.account_id).is_some_and(|v| {
                v.owner == Some(socket_id)
                    && v.call
                        .as_ref()
                        .is_some_and(|c| c["selfMute"] != Value::Bool(true))
            })
        };
        if !allow {
            return;
        }
        let frame = B64_STD.encode(&data[1..]);
        self.inner.bridge.notify(
            "voiceAudio",
            json!({ "account": self.account_id, "frames": [frame] }),
        );
    }

    fn on_close(&self, socket_id: u64) {
        let mut voices = self.inner.voices.lock().expect("voices");
        let Some(v) = voices.get_mut(&self.account_id) else {
            return;
        };
        v.sockets.remove(&socket_id);
        if v.owner == Some(socket_id) {
            v.owner = None;
            v.broadcast_ownership();
            self.inner.watch_owner_locked(&self.account_id, v);
        }
    }
}

impl ChatInner {
    async fn worker(&self, operation: &str, value: Value) -> Result<Value> {
        self.db
            .operation(
                "service_chat_worker",
                operation,
                CHAT_READ_OPERATIONS,
                json!([operation, value.to_string()]),
            )
            .await
    }

    async fn call(&self, session_id: &str, operation: &str, value: Value) -> Result<Value> {
        self.db
            .operation(
                "service_chat",
                operation,
                CHAT_READ_OPERATIONS,
                json!([session_id, operation, value.to_string()]),
            )
            .await
    }

    fn seal(&self, account_id: &str, key: &str, value: &str) -> String {
        self.key_box
            .encrypt(&json!({ "a": account_id, "k": key, "v": value }))
    }

    fn open(&self, account_id: &str, key: &str, sealed: &str) -> Result<String> {
        let value = self.key_box.decrypt(sealed).ok_or_else(|| {
            AppError::with("runtimeError", 500, "The identity payload cannot be decrypted.")
        })?;
        if value["a"].as_str() != Some(account_id) || value["k"].as_str() != Some(key) {
            return Err(AppError::with(
                "runtimeError",
                500,
                "A chat key does not belong where it is stored.",
            ));
        }
        Ok(value["v"].as_str().unwrap_or_default().to_owned())
    }

    fn cancel_retry(&self, account_id: &str) {
        if let Some(handle) = self.retries.lock().expect("retries").remove(account_id) {
            handle.abort();
        }
    }

    /// Returns the FIFO state-update channel for `account_id`, spawning its serial runner if needed.
    fn state_channel(self: &Arc<Self>, account_id: &str) -> mpsc::UnboundedSender<StateCmd> {
        let mut queues = self.queues.lock().expect("state queues");
        if let Some(tx) = queues.get(account_id)
            && !tx.is_closed()
        {
            return tx.clone();
        }
        let (tx, mut rx) = mpsc::unbounded_channel::<StateCmd>();
        let inner = self.clone();
        let aid = account_id.to_owned();
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    StateCmd::Set {
                        state,
                        jid,
                        detail,
                        done,
                    } => {
                        let mut payload = Map::new();
                        payload.insert("accountId".to_owned(), json!(aid));
                        payload.insert("state".to_owned(), json!(state));
                        payload.insert("detail".to_owned(), json!(detail));
                        if let Some(j) = jid {
                            payload.insert("jid".to_owned(), json!(j));
                        }
                        if let Err(err) = inner.worker("state", Value::Object(payload)).await
                            && err.code == "notFound"
                        {
                            let bridge = inner.bridge.clone();
                            let account = aid.clone();
                            tokio::spawn(async move {
                                let _ = bridge
                                    .call(
                                        "disconnect",
                                        json!({ "account": account }),
                                        Duration::from_secs(60),
                                    )
                                    .await;
                            });
                        }
                        let _ = done.send(());
                    }
                    StateCmd::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        queues.insert(account_id.to_owned(), tx.clone());
        tx
    }

    /// Enqueues a state transition synchronously (preserving bridge event order) and returns
    /// a receiver that resolves once the write has finished.
    fn enqueue_state(
        self: &Arc<Self>,
        account_id: &str,
        state: &str,
        jid: Option<String>,
        detail: &str,
    ) -> oneshot::Receiver<()> {
        let (done, rx) = oneshot::channel();
        let tx = self.state_channel(account_id);
        let _ = tx.send(StateCmd::Set {
            state: state.to_owned(),
            jid,
            detail: detail.to_owned(),
            done,
        });
        rx
    }

    /// Waits for all state transitions previously enqueued for `account_id`.
    async fn flush_state(self: &Arc<Self>, account_id: &str) {
        let tx = {
            let queues = self.queues.lock().expect("state queues");
            queues.get(account_id).cloned()
        };
        if let Some(tx) = tx {
            let (done, rx) = oneshot::channel();
            if tx.send(StateCmd::Flush(done)).is_ok() {
                let _ = rx.await;
            }
        }
    }

    /// Leaves a call nobody hears any more, as Discord does when its app closes.
    fn watch_owner_locked(self: &Arc<Self>, account_id: &str, v: &mut Voice) {
        if let Some(handle) = v.orphan.take() {
            handle.abort();
        }
        if v.owner.is_some() || v.call.is_none() {
            return;
        }
        let inner = self.clone();
        let aid = account_id.to_owned();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let should_leave = {
                let voices = inner.voices.lock().expect("voices");
                voices
                    .get(&aid)
                    .is_some_and(|v| v.owner.is_none() && v.call.is_some())
            };
            if should_leave {
                let _ = inner
                    .bridge
                    .call("voiceLeave", json!({ "account": aid }), Duration::from_secs(60))
                    .await;
            }
        });
        v.orphan = Some(handle.abort_handle());
    }

    fn voice_event(self: &Arc<Self>, account_id: &str, event: &str, data: Map<String, Value>) {
        if event == "voiceAudio" {
            let owner_socket = {
                let voices = self.voices.lock().expect("voices");
                voices
                    .get(account_id)
                    .and_then(|v| v.owner.and_then(|id| v.sockets.get(&id).cloned()))
            };
            let Some(owner) = owner_socket else {
                return;
            };
            let Some(user_str) = data.get("user").and_then(Value::as_str) else {
                return;
            };
            if !(1..=20).contains(&user_str.len())
                || !user_str.bytes().all(|b| b.is_ascii_digit())
            {
                return;
            }
            let Ok(user_id) = user_str.parse::<u64>() else {
                return;
            };
            let Some(b64) = data.get("data").and_then(Value::as_str) else {
                return;
            };
            let Ok(opus) = B64_STD.decode(b64) else {
                return;
            };
            let mut frame = Vec::with_capacity(9 + opus.len());
            frame.push(1);
            frame.extend_from_slice(&user_id.to_be_bytes());
            frame.extend_from_slice(&opus);
            owner.send_audio(frame);
            return;
        }

        let mut voices = self.voices.lock().expect("voices");
        let v = voices
            .entry(account_id.to_owned())
            .or_insert_with(Voice::new);

        if event == "voiceStates" {
            let servers = data
                .get("servers")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let complete = data.get("complete") == Some(&Value::Bool(true));
            if complete {
                v.servers.clear();
            }
            for (id, channels) in &servers {
                if channels
                    .as_object()
                    .is_some_and(|ch| !ch.is_empty())
                {
                    v.servers.insert(id.clone(), channels.clone());
                } else {
                    v.servers.remove(id);
                }
            }
            v.push(&json!({
                "type": "voice",
                "servers": Value::Object(servers),
                "complete": complete,
            }));
            return;
        }

        if event == "voiceCall" {
            let state = value_to_string(data.get("state"));
            if state == "ended" {
                let reason = value_to_string(data.get("reason"));
                v.call = None;
                if let Some(handle) = v.orphan.take() {
                    handle.abort();
                }
                v.owner = None;
                v.push(&json!({
                    "type": "call",
                    "call": Value::Null,
                    "ended": {
                        "channelId": value_to_string(data.get("channelId")),
                        "reason": reason,
                        "message": voice_end_message(&reason),
                    },
                }));
                v.broadcast_ownership();
                return;
            }
            let call = json!({
                "state": state,
                "serverId": value_to_string(data.get("serverId")),
                "channelId": value_to_string(data.get("channelId")),
                "selfMute": data.get("selfMute") == Some(&Value::Bool(true)),
                "selfDeaf": data.get("selfDeaf") == Some(&Value::Bool(true)),
                "canSpeak": data.get("canSpeak") != Some(&Value::Bool(false)),
                "e2ee": data.get("e2ee") == Some(&Value::Bool(true)),
                "protocol": json_i64(data.get("protocol")),
            });
            v.call = Some(call.clone());
            v.push(&json!({ "type": "call", "call": call }));
        }
    }

    fn connect(self: Arc<Self>, account_id: String, attempt: u32) {
        self.cancel_retry(&account_id);
        if self.closing.load(Ordering::Relaxed) || !self.active.load(Ordering::Relaxed) {
            return;
        }
        tokio::spawn(async move {
            let _ = self
                .enqueue_state(&account_id, "connecting", None, "")
                .await;
            let platform = self
                .platforms
                .lock()
                .expect("platforms")
                .get(&account_id)
                .cloned()
                .unwrap_or_else(|| "whatsapp".to_owned());
            if let Err(err) = self
                .bridge
                .call(
                    "connect",
                    json!({ "account": account_id, "platform": platform }),
                    Duration::from_secs(120),
                )
                .await
            {
                if err.code == "notLinked" {
                    let _ = self
                        .enqueue_state(&account_id, "loggedOut", None, "notLinked")
                        .await;
                    return;
                }
                if self.closing.load(Ordering::Relaxed)
                    || !self.active.load(Ordering::Relaxed)
                    || !self.bridge.status().available
                {
                    return;
                }
                let _ = self
                    .enqueue_state(&account_id, "disconnected", None, "unreachable")
                    .await;
                let delay_ms = (15_000u64 << attempt.min(5)).min(10 * 60_000);
                let inner_retry = self.clone();
                let aid = account_id.clone();
                let handle = tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    inner_retry.connect(aid, attempt.saturating_add(1));
                });
                let mut retries = self.retries.lock().expect("retries");
                if self.closing.load(Ordering::Relaxed) || !self.active.load(Ordering::Relaxed) {
                    handle.abort();
                } else if let Some(prev) = retries.insert(account_id, handle.abort_handle()) {
                    prev.abort();
                }
            }
        });
    }

    async fn connect_all(self: &Arc<Self>) {
        if !self.active.load(Ordering::Relaxed) {
            return;
        }
        let accounts = self
            .worker("accounts", json!({}))
            .await
            .ok()
            .and_then(|v| v["accounts"].as_array().cloned())
            .unwrap_or_default();
        for item in accounts {
            let Some(id) = item["id"].as_str() else {
                continue;
            };
            let platform = item["platform"].as_str().unwrap_or("whatsapp").to_owned();
            let state = item["state"].as_str().unwrap_or_default();
            self.platforms
                .lock()
                .expect("platforms")
                .insert(id.to_owned(), platform);
            if state != "failed" {
                self.clone().connect(id.to_owned(), 0);
            }
        }
    }

    async fn handle_bridge_request(
        self: Arc<Self>,
        method: String,
        mut params: Map<String, Value>,
    ) -> Result<Value> {
        let account_id = value_to_string(params.get("account"));
        if !is_account_id(&account_id) {
            return Err(AppError::invalid());
        }

        let outcome: Result<Value> = async {
            match method.as_str() {
                "kv.load" => {
                    let cursor = value_to_string(params.get("cursor"));
                    let page = self
                        .worker(
                            "kvLoad",
                            json!({ "accountId": account_id, "cursor": cursor }),
                        )
                        .await?;
                    let mut entries = Vec::new();
                    if let Some(raw_entries) = page["entries"].as_array() {
                        for e in raw_entries {
                            let key = e["key"].as_str().unwrap_or_default();
                            let sealed = e["value"].as_str().unwrap_or_default();
                            let plain = self.open(&account_id, key, sealed)?;
                            entries.push(json!({ "key": key, "value": plain }));
                        }
                    }
                    Ok(json!({ "entries": entries, "next": page["next"] }))
                }
                "kv.get" => {
                    let keys: Vec<String> = params
                        .get("keys")
                        .and_then(Value::as_array)
                        .map(|arr| arr.iter().map(|v| value_to_string(Some(v))).collect())
                        .unwrap_or_default();
                    let mut values = Map::new();
                    for chunk in keys.chunks(500) {
                        let found = self
                            .worker(
                                "kvGet",
                                json!({ "accountId": account_id, "keys": chunk }),
                            )
                            .await?;
                        if let Some(map) = found["values"].as_object() {
                            for (key, sealed_val) in map {
                                let sealed = sealed_val.as_str().unwrap_or_default();
                                let plain = self.open(&account_id, key, sealed)?;
                                values.insert(key.clone(), json!(plain));
                            }
                        }
                    }
                    Ok(json!({ "values": Value::Object(values) }))
                }
                "kv.commit" => {
                    let deletes: Vec<String> = params
                        .get("deletes")
                        .and_then(Value::as_array)
                        .map(|arr| arr.iter().map(|v| value_to_string(Some(v))).collect())
                        .unwrap_or_default();
                    let mut sealed_puts = Vec::new();
                    if let Some(puts) = params.get("puts").and_then(Value::as_array) {
                        for e in puts {
                            let key = value_to_string(e.get("key"));
                            let val = value_to_string(e.get("value"));
                            let mut entry = Map::new();
                            entry.insert("key".to_owned(), json!(key));
                            entry.insert(
                                "value".to_owned(),
                                json!(self.seal(&account_id, &key, &val)),
                            );
                            if e.get("lazy") == Some(&Value::Bool(true)) {
                                entry.insert("lazy".to_owned(), json!(true));
                            }
                            if let Some(exp) = e
                                .get("expiresAt")
                                .and_then(|v| v.as_f64())
                                .filter(|&n| n != 0.0 && n.is_finite())
                            {
                                entry.insert("expiresAt".to_owned(), json!(exp as i64));
                            }
                            sealed_puts.push(Value::Object(entry));
                        }
                    }
                    self.worker(
                        "kvCommit",
                        json!({
                            "accountId": account_id,
                            "deletes": deletes,
                            "puts": sealed_puts,
                        }),
                    )
                    .await?;
                    Ok(json!({}))
                }
                "ingest" => {
                    // History can follow pairing within milliseconds; store the link first.
                    self.flush_state(&account_id).await;
                    params.remove("account");
                    params.insert("accountId".to_owned(), json!(account_id));
                    let result = self.worker("ingest", Value::Object(params)).await?;
                    if let Some(download) = result["download"].as_array() {
                        let ids: Vec<String> = download
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect();
                        if !ids.is_empty() {
                            self.media.queue(ids);
                        }
                    }
                    Ok(json!({ "ok": true }))
                }
                _ => Err(AppError::invalid()),
            }
        }
        .await;

        if let Err(ref err) = outcome
            && err.code == "notFound"
        {
            // The account was deleted; let the bridge stop instead of retrying forever.
            let bridge = self.bridge.clone();
            tokio::spawn(async move {
                let _ = bridge
                    .call(
                        "disconnect",
                        json!({ "account": account_id }),
                        Duration::from_secs(60),
                    )
                    .await;
            });
        }
        outcome
    }

    fn handle_bridge_event(
        self: &Arc<Self>,
        account_id: String,
        event: String,
        data: Map<String, Value>,
    ) {
        if !is_account_id(&account_id) {
            return;
        }
        match event.as_str() {
            "linkCode" => {
                let expires_in = data
                    .get("expiresIn")
                    .and_then(Value::as_f64)
                    .unwrap_or(20.0);
                let expires_at = now_ms() + (expires_in * 1000.0) as i64;
                self.links.lock().expect("links").insert(
                    account_id.clone(),
                    json!({
                        "accountId": account_id,
                        "state": "qr",
                        "qr": value_to_string(data.get("code")),
                        "expiresAt": expires_at,
                    }),
                );
            }
            "pairingCode" => {
                self.links.lock().expect("links").insert(
                    account_id.clone(),
                    json!({
                        "accountId": account_id,
                        "state": "code",
                        "code": value_to_string(data.get("code")),
                    }),
                );
            }
            "linkScanned" => {
                let name: String = value_to_string(data.get("name"))
                    .chars()
                    .take(64)
                    .collect();
                self.links.lock().expect("links").insert(
                    account_id.clone(),
                    json!({
                        "accountId": account_id,
                        "state": "scanned",
                        "name": name,
                    }),
                );
            }
            "linkFailed" => {
                let reason = data
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("pairingFailed");
                self.links.lock().expect("links").insert(
                    account_id.clone(),
                    json!({
                        "accountId": account_id,
                        "state": "failed",
                        "reason": reason_message(reason),
                    }),
                );
                let _ = self.enqueue_state(&account_id, "failed", None, reason);
            }
            "paired" => {
                {
                    let mut links = self.links.lock().expect("links");
                    if links.contains_key(&account_id) {
                        links.insert(
                            account_id.clone(),
                            json!({ "accountId": account_id, "state": "paired" }),
                        );
                    }
                }
                let jid = value_to_string(data.get("jid"));
                let _ = self.enqueue_state(&account_id, "connecting", Some(jid), "");
            }
            "connected" => {
                let _ = self.enqueue_state(&account_id, "connected", None, "");
            }
            "disconnected" | "connectFailure" => {
                let detail = value_to_string(data.get("reason"));
                let _ = self.enqueue_state(&account_id, "disconnected", None, &detail);
            }
            "loggedOut" => {
                {
                    let mut voices = self.voices.lock().expect("voices");
                    if let Some(v) = voices.get_mut(&account_id) {
                        v.servers.clear();
                        v.push(&json!({ "type": "voice", "servers": {}, "complete": true }));
                    }
                }
                let detail = value_to_string(data.get("reason"));
                let done = self.enqueue_state(&account_id, "loggedOut", None, &detail);
                let inner = self.clone();
                tokio::spawn(async move {
                    let _ = done.await;
                    let _ = inner
                        .worker("kvClear", json!({ "accountId": account_id }))
                        .await;
                });
            }
            "replaced" => {
                let _ = self.enqueue_state(&account_id, "failed", None, "replaced");
            }
            "banned" => {
                let _ = self.enqueue_state(&account_id, "failed", None, "banned");
            }
            "outdated" => {
                let _ = self.enqueue_state(&account_id, "failed", None, "outdated");
            }
            "voiceStates" | "voiceCall" | "voiceAudio" => {
                self.voice_event(&account_id, &event, data);
            }
            "mediaRetried" => {
                let chat = value_to_string(data.get("chat"));
                let msg_id = value_to_string(data.get("id"));
                let ok = data.get("ok") == Some(&Value::Bool(true));
                let media = self.media.clone();
                tokio::spawn(async move {
                    let _ = media.retried(&account_id, &chat, &msg_id, ok).await;
                });
            }
            _ => {}
        }
    }
}

struct BridgeAdapter(Arc<ChatInner>);

impl BridgeHandler for BridgeAdapter {
    fn request(
        &self,
        method: String,
        params: Map<String, Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>> {
        let inner = self.0.clone();
        Box::pin(async move { inner.handle_bridge_request(method, params).await })
    }

    fn event(&self, account: String, event: String, data: Map<String, Value>) {
        self.0.handle_bridge_event(account, event, data);
    }

    fn on_ready(&self) {
        let inner = self.0.clone();
        tokio::spawn(async move {
            inner.connect_all().await;
        });
    }
}
