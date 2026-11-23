//! Dar's browser driver for the acceptance suite: Chromium over the DevTools Protocol, with
//! Playwright's model (browser → contexts → pages → locators) and its waiting rules, so suites
//! read like the Playwright tests they replace. One headless Chromium serves a whole run; each
//! context is a separate profile (cookies, storage, permissions, downloads).

mod cdp;
mod keys;
mod locator;
mod page;

pub use cdp::Event;
pub use locator::{Locator, Pattern, State, contains, regex};
pub use page::{ApiRequest, ApiResponse, ClickOptions, ConsoleMessage, Download, FilePayload, Fulfill, Keyboard, Mouse, Page, Rect, Request, Response, Route, RouteMatch, glob_matches};

use crate::{Error, Result};
use cdp::Connection;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;

/// The default time a locator waits for its element and for actionability.
pub const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub struct Browser {
    connection: Arc<Connection>,
    child: Mutex<Child>,
    profile: PathBuf,
}

static SHARED: OnceCell<Arc<Browser>> = OnceCell::const_new();

impl Browser {
    /// The run's Chromium (started on first use). `DAR_CHROMIUM` names the executable,
    /// otherwise `chromium` on PATH.
    pub async fn shared() -> Result<Arc<Browser>> {
        SHARED.get_or_try_init(|| async { Browser::launch().map(Arc::new) }).await.cloned()
    }

    pub fn launch() -> Result<Browser> {
        let executable = std::env::var_os("DAR_CHROMIUM").map(PathBuf::from).or_else(|| crate::which("chromium"))
            .ok_or_else(|| Error::new("Chromium is required: use Dar's dev shell or set DAR_CHROMIUM"))?;
        let profile = crate::temp_dir("dar-chromium-")?;
        let log = std::fs::File::create(profile.join("chromium.log"))?;
        let (chrome_reads, we_write) = std::io::pipe()?;
        let (we_read, chrome_writes) = std::io::pipe()?;
        // The pipe ends go to fds 3 and 4 through the shell, which needs no unsafe pre_exec.
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(r#"exec "$0" "$@" 3<&0 4>&1 0</dev/null 1>&2"#)
            .arg(&executable)
            .args([
                "--headless=new", "--remote-debugging-pipe", "--no-sandbox", "--no-first-run", "--no-default-browser-check",
                "--disable-background-networking", "--disable-background-timer-throttling", "--disable-backgrounding-occluded-windows",
                "--disable-renderer-backgrounding", "--disable-breakpad", "--disable-component-update", "--disable-default-apps",
                "--disable-dev-shm-usage", "--disable-extensions", "--disable-hang-monitor", "--disable-ipc-flooding-protection",
                "--disable-popup-blocking", "--disable-prompt-on-repost", "--disable-sync", "--disable-search-engine-choice-screen",
                "--disable-features=Translate,MediaRouter,OptimizationHints,DialMediaRouteProvider,AcceptCHFrame,AutoExpandDetailsElement,CertificateTransparencyComponentUpdater,DestroyProfileOnBrowserClose,GlobalMediaControls,HttpsUpgrades,LensOverlay,PaintHolding,ThirdPartyStoragePartitioning",
                "--enable-blink-features=ComputedAccessibilityInfo", "--force-color-profile=srgb", "--metrics-recording-only",
                "--password-store=basic", "--use-mock-keychain", "--hide-scrollbars", "--mute-audio", "--use-fake-ui-for-media-stream",
                "--use-fake-device-for-media-stream", "--autoplay-policy=no-user-gesture-required",
            ])
            .arg(format!("--user-data-dir={}", profile.join("profile").display()))
            .arg("about:blank")
            .stdin(Stdio::from(chrome_reads))
            .stdout(Stdio::from(chrome_writes))
            .stderr(log);
        let child = command.spawn().map_err(|error| Error::new(format!("Chromium did not start: {error}")))?;
        drop(command);
        Ok(Browser { connection: Connection::new(we_write, we_read), child: Mutex::new(child), profile })
    }

    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.connection.call(None, method, params).await
    }

    pub async fn new_context(self: &Arc<Self>, options: ContextOptions) -> Result<Context> {
        let id = self.call("Target.createBrowserContext", json!({"disposeOnDetach": true})).await?["browserContextId"]
            .as_str().ok_or_else(|| Error::new("no browser context id"))?.to_owned();
        let downloads = crate::temp_dir("dar-downloads-")?;
        self.call("Browser.setDownloadBehavior", json!({"behavior": "allowAndName", "browserContextId": id, "downloadPath": downloads, "eventsEnabled": true})).await?;
        if !options.permissions.is_empty() {
            self.call("Browser.grantPermissions", json!({"browserContextId": id, "permissions": options.permissions.iter().map(|p| permission(p)).collect::<Vec<_>>()})).await?;
        }
        let context = Context { inner: Arc::new(ContextInner { browser: self.clone(), id, options, downloads, pages: Mutex::default() }) };
        if let Some(state) = context.inner.options.storage_state.clone() {
            context.add_cookies(state.cookies).await?;
            if !state.origins.is_empty() {
                // Local storage is restored by visiting each origin once, as Playwright does.
                let page = context.new_page().await?;
                page.route("**/*", None, |route| async move { route.fulfill(Fulfill::text(200, "text/html", "<!doctype html>")).await }).await?;
                for origin in &state.origins {
                    page.goto(&format!("{}/", origin.origin)).await?;
                    page.evaluate_with("(entries) => { for (const [k, v] of entries) localStorage.setItem(k, v); }", json!(origin.local_storage)).await?;
                }
                page.close().await?;
            }
        }
        Ok(context)
    }

    /// A context with the given viewport and Playwright's other defaults.
    pub async fn new_context_sized(self: &Arc<Self>, width: u32, height: u32) -> Result<Context> {
        self.new_context(ContextOptions { viewport: (width, height), ..ContextOptions::default() }).await
    }
}

/// Playwright permission names, as the protocol spells them.
fn permission(name: &str) -> String {
    match name {
        "clipboard-read" => "clipboardReadWrite".into(),
        "clipboard-write" => "clipboardSanitizedWrite".into(),
        "geolocation" => "geolocation".into(),
        "notifications" => "notifications".into(),
        "microphone" => "audioCapture".into(),
        "camera" => "videoCapture".into(),
        other => other.into(),
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

/// Closes the run's shared browser (the harness calls this before exiting).
pub async fn shutdown() {
    if let Some(browser) = SHARED.get() {
        let _ = browser.call("Browser.close", json!({})).await;
        if let Ok(mut child) = browser.child.lock() {
            let _ = child.wait();
        }
    }
}

#[derive(Clone, Debug)]
pub struct ContextOptions {
    pub viewport: (u32, u32),
    pub is_mobile: bool,
    pub has_touch: bool,
    pub device_scale_factor: f64,
    /// `light` unless set, as in Playwright.
    pub color_scheme: Option<String>,
    pub reduced_motion: Option<String>,
    pub ignore_https_errors: bool,
    pub permissions: Vec<String>,
    pub storage_state: Option<StorageState>,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self { viewport: (1280, 720), is_mobile: false, has_touch: false, device_scale_factor: 1.0, color_scheme: None, reduced_motion: None,
            ignore_https_errors: false, permissions: Vec::new(), storage_state: None }
    }
}

impl ContextOptions {
    pub fn sized(width: u32, height: u32) -> Self {
        Self { viewport: (width, height), ..Self::default() }
    }
    /// A phone: viewport, mobile layout and touch (Playwright's `isMobile` and `hasTouch`).
    pub fn phone(width: u32, height: u32) -> Self {
        Self { viewport: (width, height), is_mobile: true, has_touch: true, ..Self::default() }
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct StorageState {
    pub cookies: Vec<Cookie>,
    pub origins: Vec<OriginStorage>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct OriginStorage {
    pub origin: String,
    pub local_storage: Vec<(String, String)>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cookie {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default = "root_path")]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub expires: f64,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default)]
    pub secure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_site: Option<String>,
}

fn root_path() -> String {
    "/".into()
}

impl Cookie {
    /// A cookie for `url`'s host, like Playwright's `{name, value, url}`.
    pub fn for_url(name: &str, value: &str, url: &str) -> Self {
        Self { name: name.into(), value: value.into(), domain: String::new(), path: "/".into(), url: Some(url.into()), expires: -1.0, http_only: false, secure: false, same_site: None }
    }
}

pub(crate) struct ContextInner {
    pub(crate) browser: Arc<Browser>,
    pub(crate) id: String,
    pub(crate) options: ContextOptions,
    pub(crate) downloads: PathBuf,
    pub(crate) pages: Mutex<Vec<Page>>,
}

#[derive(Clone)]
pub struct Context {
    pub(crate) inner: Arc<ContextInner>,
}

impl Context {
    pub async fn new_page(&self) -> Result<Page> {
        let page = Page::create(self.clone()).await?;
        self.inner.pages.lock().unwrap().push(page.clone());
        Ok(page)
    }

    pub fn pages(&self) -> Vec<Page> {
        self.inner.pages.lock().unwrap().iter().filter(|page| !page.is_closed()).cloned().collect()
    }

    pub fn options(&self) -> &ContextOptions {
        &self.inner.options
    }

    pub fn downloads_dir(&self) -> &Path {
        &self.inner.downloads
    }

    pub async fn cookies(&self) -> Result<Vec<Cookie>> {
        let result = self.inner.browser.call("Storage.getCookies", json!({"browserContextId": self.inner.id})).await?;
        Ok(serde_json::from_value(result["cookies"].clone())?)
    }

    pub async fn add_cookies(&self, cookies: Vec<Cookie>) -> Result<()> {
        let cookies: Vec<Value> = cookies.into_iter().map(|cookie| {
            let mut value = serde_json::to_value(&cookie).unwrap();
            if cookie.url.is_some() {
                value.as_object_mut().unwrap().remove("domain");
            }
            if cookie.expires <= 0.0 {
                value.as_object_mut().unwrap().remove("expires");
            }
            value
        }).collect();
        self.inner.browser.call("Storage.setCookies", json!({"browserContextId": self.inner.id, "cookies": cookies})).await?;
        Ok(())
    }

    pub async fn clear_cookies(&self) -> Result<()> {
        self.inner.browser.call("Storage.clearCookies", json!({"browserContextId": self.inner.id})).await?;
        Ok(())
    }

    /// Cookies and each open page's local storage, for a new context (Playwright's storageState).
    pub async fn storage_state(&self) -> Result<StorageState> {
        let mut origins: Vec<OriginStorage> = Vec::new();
        for page in self.pages() {
            let found = page.evaluate("() => ({origin: location.origin, entries: Object.entries(localStorage)})").await?;
            let origin = found["origin"].as_str().unwrap_or_default().to_owned();
            if origin.starts_with("http") && !origins.iter().any(|o| o.origin == origin) {
                origins.push(OriginStorage { origin, local_storage: serde_json::from_value(found["entries"].clone()).unwrap_or_default() });
            }
        }
        Ok(StorageState { cookies: self.cookies().await?, origins })
    }

    pub async fn grant_permissions(&self, permissions: &[&str], origin: Option<&str>) -> Result<()> {
        let mut params = json!({"browserContextId": self.inner.id, "permissions": permissions.iter().map(|p| permission(p)).collect::<Vec<_>>()});
        if let Some(origin) = origin {
            params["origin"] = json!(origin);
        }
        self.inner.browser.call("Browser.grantPermissions", params).await?;
        Ok(())
    }

    /// An HTTP client that sends this context's cookies (Playwright's `context.request`).
    pub fn request(&self) -> page::ApiRequest {
        page::ApiRequest::new(self.clone())
    }

    pub async fn close(&self) -> Result<()> {
        for page in self.pages() {
            page.close().await.ok();
        }
        self.inner.browser.call("Target.disposeBrowserContext", json!({"browserContextId": self.inner.id})).await.ok();
        let _ = std::fs::remove_dir_all(&self.inner.downloads);
        Ok(())
    }
}
