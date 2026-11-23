//! The page's live database connection (`client/live.ts`), on SpacetimeDB's Rust SDK
//! (`browser` feature). The runtime issues a short-lived browser token per tab; a lost
//! connection reconnects with a fresh token.

use std::sync::Arc;

use futures::channel::oneshot;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use spacetimedb_sdk::__codegen::InternalError;
use spacetimedb_sdk::DbContext;

use crate::api::{self, BrowserSession, Req};
use crate::module_bindings::{DbConnection, ProcedureEventContext, ReducerEventContext};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LiveStatus {
    Connecting,
    Open,
    Reconnecting,
    Error,
}

pub type Connection = Arc<DbConnection>;

/// Shared by every app on the page; provided once by the app root.
#[derive(Clone, Copy)]
pub struct Live {
    pub status: RwSignal<LiveStatus>,
    pub connection: RwSignal<Option<Connection>>,
    started: RwSignal<Option<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Token {
    token: String,
    database_url: String,
    database_name: String,
}

impl Live {
    pub fn new() -> Self {
        Self { status: RwSignal::new(LiveStatus::Connecting), connection: RwSignal::new(None), started: RwSignal::new(None) }
    }

    /// Opens the connection for `session` once; later calls reuse it.
    pub fn open(self, session: &BrowserSession) {
        if self.started.get_untracked().as_deref() == Some(session.csrf_token.as_str()) {
            return;
        }
        if let Some(old) = self.connection.get_untracked() {
            let _ = old.disconnect();
        }
        self.started.set(Some(session.csrf_token.clone()));
        self.connection.set(None);
        self.status.set(LiveStatus::Connecting);
        let session = session.clone();
        spawn_local(async move { self.run(session).await });
    }

    async fn run(self, session: BrowserSession) {
        let tab = crate::random_uuid();
        let mut failures = 0u32;
        loop {
            if self.started.get_untracked().as_deref() != Some(session.csrf_token.as_str()) {
                return;
            }
            match self.connect(&session, &tab).await {
                Ok(connection) => {
                    failures = 0;
                    self.connection.set(Some(connection.clone()));
                    self.status.set(LiveStatus::Open);
                    // Messages are applied here; callbacks run as each one is processed.
                    while connection.advance_one_message_async().await.is_ok() {}
                    if self.started.get_untracked().as_deref() != Some(session.csrf_token.as_str()) {
                        return;
                    }
                    self.status.set(LiveStatus::Reconnecting);
                }
                Err(error) => {
                    failures += 1;
                    leptos::logging::warn!("live connection failed: {error}");
                    if self.connection.get_untracked().is_none() || failures >= 5 {
                        self.status.set(LiveStatus::Error);
                        return;
                    }
                    if self.status.get_untracked() == LiveStatus::Open {
                        self.status.set(LiveStatus::Reconnecting);
                    }
                }
            }
            crate::sleep(1000 * (1 << failures.min(4))).await;
        }
    }

    async fn connect(&self, session: &BrowserSession, tab: &str) -> Result<Connection, String> {
        let token: Token = api::request(&format!("/api/v1/auth/token?tab={tab}"), Req { method: "POST", csrf: Some(&session.csrf_token), ..Default::default() })
            .await
            .map_err(|e| e.message)?;
        let connection = DbConnection::builder()
            .with_uri(token.database_url.as_str())
            .with_database_name(token.database_name)
            .with_token(Some(token.token));
        #[cfg(target_arch = "wasm32")]
        let connection = connection.build().await;
        #[cfg(not(target_arch = "wasm32"))]
        let connection = connection.build();
        let connection = connection.map_err(|e| e.to_string())?;
        Ok(Arc::new(connection))
    }
}

impl Default for Live {
    fn default() -> Self {
        Self::new()
    }
}

/// The reducer callback that reports its outcome on `sender`.
pub fn replied(sender: oneshot::Sender<Result<(), String>>) -> impl FnOnce(&ReducerEventContext, Result<Result<(), String>, InternalError>) + Send + 'static {
    move |_, result| {
        let _ = sender.send(match result {
            Ok(inner) => inner,
            Err(error) => Err(error.to_string()),
        });
    }
}

/// Calls a reducer and waits for its result: `reducer(|tx| c.reducers.x_then(args, replied(tx)))`.
pub async fn reducer(invoke: impl FnOnce(oneshot::Sender<Result<(), String>>) -> spacetimedb_sdk::Result<()>) -> Result<(), String> {
    let (sender, receiver) = oneshot::channel();
    invoke(sender).map_err(|e| e.to_string())?;
    receiver.await.unwrap_or_else(|_| Err("The connection was interrupted. Try again.".into()))
}

/// The procedure callback that reports its JSON result on `sender`.
pub fn answered(sender: oneshot::Sender<Result<String, String>>) -> impl FnOnce(&ProcedureEventContext, Result<String, InternalError>) + Send + 'static {
    move |_, result| {
        let _ = sender.send(result.map_err(|e| e.to_string()));
    }
}

/// Calls a procedure and parses its JSON result, which may carry `{error:{code}}`.
pub async fn procedure<T: serde::de::DeserializeOwned>(invoke: impl FnOnce(oneshot::Sender<Result<String, String>>)) -> Result<T, String> {
    let (sender, receiver) = oneshot::channel();
    invoke(sender);
    let value = receiver.await.unwrap_or_else(|_| Err("The connection was interrupted. Try again.".into()))?;
    api::read_procedure(&value)
}

thread_local! {
    static LIVE: std::cell::Cell<Option<Live>> = const { std::cell::Cell::new(None) };
}

/// Registers the page's connection for code running outside a component (async tasks).
pub fn register(live: Live) {
    LIVE.with(|l| l.set(Some(live)));
}

/// The open connection, if any.
pub fn connection() -> Option<Connection> {
    LIVE.with(|l| l.get()).and_then(|live| live.connection.try_get_untracked().flatten())
}
