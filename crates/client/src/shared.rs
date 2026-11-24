//! Data several components read, fetched once per page (`shared-request.ts`), and the
//! account preferences (`app-settings.tsx`).

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::api::{self, BrowserSession, Req, RequestError};
use crate::ui::{listen, window};

/// Window focus refreshes data older than this; the named events always refresh.
const FRESH_MS: f64 = 15_000.0;

pub struct Shared<T: Send + Sync + 'static> {
    pub value: RwSignal<Option<T>>,
    pub error: RwSignal<String>,
    url: &'static str,
    sequence: RwSignal<u32>,
    fetched_at: RwSignal<f64>,
    received_at: RwSignal<f64>,
}

impl<T: Send + Sync + 'static> Clone for Shared<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static> Copy for Shared<T> {}

impl<T: DeserializeOwned + Clone + PartialEq + Send + Sync + 'static> Shared<T> {
    /// Fetches now, again on each of `events`, and on focus once stale.
    pub fn new(url: &'static str, events: &'static [&'static str]) -> Self {
        let shared = Self {
            value: RwSignal::new(None),
            error: RwSignal::new(String::new()),
            url,
            sequence: RwSignal::new(0),
            fetched_at: RwSignal::new(0.0),
            received_at: RwSignal::new(0.0),
        };
        for name in events {
            listen(&window(), name, false, move |_| shared.refresh());
        }
        listen(&window(), "focus", false, move |_| {
            if js_sys::Date::now() - shared.fetched_at.get_untracked() > FRESH_MS {
                shared.refresh();
            }
        });
        shared.refresh();
        shared
    }

    pub fn refresh(self) {
        let id = self.sequence.get_untracked() + 1;
        self.sequence.set(id);
        self.fetched_at.set(js_sys::Date::now());
        spawn_local(async move {
            let result: Result<T, RequestError> = api::get(self.url).await;
            if self.sequence.try_get_untracked() != Some(id) {
                return;
            }
            match result {
                Ok(value) => {
                    self.received_at.set(js_sys::Date::now());
                    if self
                        .value
                        .with_untracked(|cur| cur.as_ref() != Some(&value))
                    {
                        self.value.set(Some(value));
                    }
                    if !self.error.with_untracked(String::is_empty) {
                        self.error.set(String::new());
                    }
                }
                Err(e) => self.error.set(e.message),
            }
        });
    }

    /// Milliseconds since the latest value arrived.
    pub fn age(self) -> f64 {
        if self.value.with_untracked(Option::is_none) {
            f64::INFINITY
        } else {
            js_sys::Date::now() - self.received_at.get_untracked()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    pub revision: u64,
    pub disabled: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct InstalledApps {
    pub items: Vec<InstalledApp>,
}

/// The account's preferences. Only the fields this port uses are typed; the rest are kept
/// so a save never drops them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub appearance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_view: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_show_hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_confirm_trash: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_details_tab: Option<String>,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

pub const PREFERENCES: &str = "/api/v1/pim/preferences";

/// Everything the shell shares across apps; provided by the app root once signed in.
#[derive(Clone, Copy)]
pub struct Context {
    pub preferences: Shared<Preferences>,
    pub app_settings: Shared<AppSettings>,
    pub installed: Shared<InstalledApps>,
}

impl Context {
    pub fn new() -> Self {
        Self {
            preferences: Shared::new(PREFERENCES, &["dar:preferences"]),
            app_settings: Shared::new("/api/v1/appSettings", &["dar:apps"]),
            installed: Shared::new("/api/v1/apps", &["dar:apps"]),
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

pub fn context() -> Context {
    expect_context::<Context>()
}

/// Saves a partial preference update at `revision`; the server merges it.
pub async fn save_preferences(
    value: serde_json::Value,
    revision: u64,
    session: &BrowserSession,
) -> Result<Preferences, RequestError> {
    let tag = format!("\"preferences:{revision}\"");
    let result = api::request::<Preferences>(
        PREFERENCES,
        Req {
            method: "PUT",
            body: Some(value.to_string()),
            csrf: Some(&session.csrf_token),
            headers: &[("if-match", &tag)],
            ..Default::default()
        },
    )
    .await?;
    let _ = window().dispatch_event(&web_sys::Event::new("dar:preferences").expect("event"));
    Ok(result)
}
