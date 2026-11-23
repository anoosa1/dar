//! A SpacetimeDB connection as a signed-in person (`connect` in `integration-helpers.ts`), with
//! helpers that await reducers and procedures like the TypeScript SDK's promises.
//!
//! ```ignore
//! let db = session.connect().await?;
//! live::reducer(|done| db.reducers.rename_entry_then(id.clone(), name, 1, done)).await?;
//! assert!(live::reducer(|done| db.reducers.rename_entry_then(id, "stale".into(), 1, done)).await.is_err());
//! let rows: Vec<_> = db.db.my_files().iter().collect();
//! ```

use crate::session::Session;
use crate::{Error, Result};
use dar_bindings::{DbConnection, ProcedureEventContext, ReducerEventContext};
use spacetimedb_sdk::{DbContext as _, __codegen::InternalError};
use std::ops::Deref;
use std::time::Duration;
use tokio::sync::oneshot;

/// How long a reducer or procedure may take before the call fails.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// A live connection; derefs to the generated `DbConnection` (`.db`, `.reducers`, `.procedures`).
pub struct Live {
    connection: DbConnection,
}

impl Deref for Live {
    type Target = DbConnection;
    fn deref(&self) -> &DbConnection {
        &self.connection
    }
}

impl Live {
    /// Connects with the session's token and waits until `queries` are applied.
    pub async fn connect(session: &Session, queries: &[&str]) -> Result<Live> {
        let uri = session.database_url.replacen("http", "ws", 1);
        let (name, token) = (session.database_name.clone(), session.token.clone());
        let (sender, on_connect) = oneshot::channel::<Result<()>>();
        let connected = std::sync::Arc::new(std::sync::Mutex::new(Some(sender)));
        let failed = connected.clone();
        let connection = tokio::task::block_in_place(|| {
            DbConnection::builder().with_uri(uri).with_database_name(name).with_token(Some(token))
                .on_connect(move |_, _, _| { if let Some(s) = connected.lock().unwrap().take() { let _ = s.send(Ok(())); } })
                .on_connect_error(move |_, error| { if let Some(s) = failed.lock().unwrap().take() { let _ = s.send(Err(Error::new(error.to_string()))); } })
                .build()
        }).map_err(|error| Error::new(format!("connecting to SpacetimeDB: {error}")))?;
        connection.run_threaded();
        tokio::time::timeout(CALL_TIMEOUT, on_connect).await.map_err(|_| Error::new("SpacetimeDB did not accept the connection in time"))?
            .map_err(|_| Error::new("the connection closed before it was accepted"))??;
        let live = Live { connection };
        live.subscribe(queries).await?;
        Ok(live)
    }

    /// Subscribes to more queries and waits until they are applied.
    pub async fn subscribe(&self, queries: &[&str]) -> Result<()> {
        if queries.is_empty() {
            return Ok(());
        }
        let (sender, receiver) = oneshot::channel::<Result<()>>();
        let sender = std::sync::Arc::new(std::sync::Mutex::new(Some(sender)));
        let on_error = sender.clone();
        self.connection.subscription_builder()
            .on_applied(move |_| { if let Some(s) = sender.lock().unwrap().take() { let _ = s.send(Ok(())); } })
            .on_error(move |_, error| { if let Some(s) = on_error.lock().unwrap().take() { let _ = s.send(Err(Error::new(error.to_string()))); } })
            .subscribe(queries.iter().map(|q| q.to_string()).collect::<Vec<_>>());
        tokio::time::timeout(CALL_TIMEOUT, receiver).await.map_err(|_| Error::new(format!("subscribing to {queries:?} timed out")))?
            .map_err(|_| Error::new("the subscription was dropped"))?
    }

    pub fn disconnect(&self) {
        let _ = self.connection.disconnect();
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.connection.disconnect();
    }
}

/// The callback a `*_then` reducer call takes; [`reducer`] makes one for you.
pub type Reply = Box<dyn FnOnce(&ReducerEventContext, std::result::Result<std::result::Result<(), String>, InternalError>) + Send + 'static>;

/// Calls a reducer and waits for its outcome: `Err` with the reducer's message (and the
/// caller's line) when it failed.
#[track_caller]
pub fn reducer(invoke: impl FnOnce(Reply) -> spacetimedb_sdk::Result<()>) -> impl std::future::Future<Output = Result<()>> {
    let location = std::panic::Location::caller();
    let outcome = call_reducer(invoke);
    async move { outcome.await.map_err(|error| error.context(format!("reducer at {location}"))) }
}

async fn call_reducer(invoke: impl FnOnce(Reply) -> spacetimedb_sdk::Result<()>) -> Result<()> {
    let (sender, receiver) = oneshot::channel();
    invoke(Box::new(move |_, outcome| {
        let _ = sender.send(match outcome {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(Error::new(message)),
            Err(error) => Err(Error::new(error.to_string())),
        });
    })).map_err(|error| Error::new(format!("calling the reducer: {error}")))?;
    tokio::time::timeout(CALL_TIMEOUT, receiver).await.map_err(|_| Error::new("the reducer did not answer in time"))?.map_err(|_| Error::new("the connection closed"))?
}

/// The callback a `*_then` procedure call takes; [`procedure`] makes one for you.
pub type Answer = Box<dyn FnOnce(&ProcedureEventContext, std::result::Result<String, InternalError>) + Send + 'static>;

/// Calls a procedure returning JSON text and parses it. A Rust procedure reports errors in-band
/// as `{"error":{"code":…}}`, which becomes `Err` with the code (and the caller's line).
#[track_caller]
pub fn procedure(invoke: impl FnOnce(Answer)) -> impl std::future::Future<Output = Result<serde_json::Value>> {
    let location = std::panic::Location::caller();
    let outcome = call_procedure(invoke);
    async move { outcome.await.map_err(|error| error.context(format!("procedure at {location}"))) }
}

async fn call_procedure(invoke: impl FnOnce(Answer)) -> Result<serde_json::Value> {
    let (sender, receiver) = oneshot::channel();
    invoke(Box::new(move |_, outcome| {
        let _ = sender.send(outcome.map_err(|error| Error::new(error.to_string())));
    }));
    let text = tokio::time::timeout(CALL_TIMEOUT, receiver).await.map_err(|_| Error::new("the procedure did not answer in time"))?.map_err(|_| Error::new("the connection closed"))??;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| Error::new(format!("procedure result is not JSON ({error}): {text}")))?;
    if let Some(code) = value.get("error").and_then(|e| e.get("code")).and_then(|c| c.as_str()) {
        return Err(Error::new(code.to_owned()));
    }
    Ok(value)
}
