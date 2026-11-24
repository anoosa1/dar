//! A page (tab) of a browser context: navigation, script evaluation, input, emulation, request
//! routing, console and error events, downloads and screenshots.

use super::cdp::{Connection, Event};
use super::keys;
use super::locator::{Locator, Pattern, Step};
use super::{Context, DEFAULT_TIMEOUT};
use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};

pub(crate) const ENGINE: &str = include_str!("engine.js");

type Listener<T> = Arc<dyn Fn(&T) + Send + Sync>;
type RouteHandler =
    Arc<dyn Fn(Route) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

/// How a route or a URL wait names URLs: a Playwright glob (`**/api/v1/files/uploads`,
/// exact when it has no wildcard) or a predicate.
#[derive(Clone)]
pub enum RouteMatch {
    Glob(String),
    Predicate(Arc<dyn Fn(&str) -> bool + Send + Sync>),
}

impl From<&str> for RouteMatch {
    fn from(glob: &str) -> Self {
        RouteMatch::Glob(glob.to_owned())
    }
}
impl From<String> for RouteMatch {
    fn from(glob: String) -> Self {
        RouteMatch::Glob(glob)
    }
}
impl From<&String> for RouteMatch {
    fn from(glob: &String) -> Self {
        RouteMatch::Glob(glob.clone())
    }
}

impl RouteMatch {
    pub fn predicate(test: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        RouteMatch::Predicate(Arc::new(test))
    }
    pub fn matches(&self, url: &str) -> bool {
        match self {
            RouteMatch::Glob(glob) => glob_matches(glob, url),
            RouteMatch::Predicate(test) => test(url),
        }
    }
}

/// Playwright's URL glob: `**` matches anything, `*` anything but `/`, `?` one character,
/// `{a,b}` alternatives. The whole URL must match.
pub fn glob_matches(glob: &str, url: &str) -> bool {
    fn expand(glob: &str) -> Vec<String> {
        if let (Some(open), Some(close)) = (glob.find('{'), glob.find('}'))
            && open < close
        {
            let (head, rest) = glob.split_at(open);
            let (alternatives, tail) = rest[1..].split_at(close - open - 1);
            return alternatives
                .split(',')
                .flat_map(|alternative| expand(&format!("{head}{alternative}{}", &tail[1..])))
                .collect();
        }
        vec![glob.to_owned()]
    }
    fn matches(pattern: &[char], text: &[char]) -> bool {
        match pattern.first() {
            None => text.is_empty(),
            Some('*') if pattern.get(1) == Some(&'*') => {
                let rest = &pattern[2..];
                // `**/` also matches nothing at all.
                if rest.first() == Some(&'/') && matches(&rest[1..], text) {
                    return true;
                }
                (0..=text.len()).any(|skip| matches(rest, &text[skip..]))
            }
            Some('*') => {
                let rest = &pattern[1..];
                let limit = text.iter().position(|c| *c == '/').unwrap_or(text.len());
                (0..=limit).any(|skip| matches(rest, &text[skip..]))
            }
            Some('?') => !text.is_empty() && matches(&pattern[1..], &text[1..]),
            Some(c) => text.first() == Some(c) && matches(&pattern[1..], &text[1..]),
        }
    }
    let text: Vec<char> = url.chars().collect();
    expand(glob)
        .iter()
        .any(|glob| matches(&glob.chars().collect::<Vec<_>>(), &text))
}

#[derive(Debug, Clone)]
pub struct ConsoleMessage {
    /// `log`, `error`, `warning`, `info`, `debug`…
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub post_data: Option<Vec<u8>>,
    /// The protocol error text, for requests that failed (`requestfailed`).
    pub failure: Option<String>,
    pub(crate) network_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub url: String,
    pub status: u16,
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A file for `set_input_files`. The browser derives its media type from the extension.
#[derive(Debug, Clone)]
pub struct FilePayload {
    pub name: String,
    pub mime_type: String,
    pub buffer: Vec<u8>,
}

impl FilePayload {
    pub fn new(name: &str, mime_type: &str, buffer: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            mime_type: mime_type.into(),
            buffer: buffer.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Download {
    pub url: String,
    pub suggested_filename: String,
    pub path: PathBuf,
}

impl Download {
    pub fn read(&self) -> Result<Vec<u8>> {
        Ok(std::fs::read(&self.path)?)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClickOptions {
    /// `left` (default), `right` or `middle`.
    pub button: Option<&'static str>,
    pub click_count: Option<u32>,
    /// `Shift`, `Control`, `Alt`, `Meta`.
    pub modifiers: Vec<&'static str>,
    /// A point relative to the element's top-left corner instead of its center.
    pub position: Option<(f64, f64)>,
    /// Skip the actionability checks, as Playwright's `force`.
    pub force: bool,
    /// Only run actionability and hit-target checks without dispatching mouse events (Playwright's `trial`).
    pub trial: bool,
    pub timeout: Option<Duration>,
    /// Do not wait for a navigation the click starts (always true here: clicks never wait).
    pub no_wait_after: bool,
    pub delay: Option<Duration>,
}

/// A response a route answers with (Playwright's `route.fulfill`).
#[derive(Debug, Clone, Default)]
pub struct Fulfill {
    pub status: u16,
    pub content_type: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Fulfill {
    pub fn json(status: u16, body: &Value) -> Self {
        Self {
            status,
            content_type: Some("application/json".into()),
            headers: Vec::new(),
            body: serde_json::to_vec(body).unwrap(),
        }
    }
    pub fn text(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type: Some(content_type.into()),
            headers: Vec::new(),
            body: body.into(),
        }
    }
}

struct RouteEntry {
    id: u64,
    pattern: RouteMatch,
    handler: RouteHandler,
    remaining: Option<u32>,
}

#[derive(Default, Clone)]
struct Navigation {
    url: String,
    loader: String,
    loaded: String,
    main_frame: String,
}

pub(crate) struct PageInner {
    pub(crate) context: Context,
    pub(crate) connection: Arc<Connection>,
    pub(crate) session: String,
    target: String,
    closed: AtomicBool,
    navigation: watch::Sender<Navigation>,
    console: Mutex<Vec<(u64, Listener<ConsoleMessage>)>>,
    errors: Mutex<Vec<(u64, Listener<String>)>>,
    requests: Mutex<Vec<(u64, Listener<Request>)>>,
    finished: Mutex<Vec<(u64, Listener<Request>)>>,
    failed: Mutex<Vec<(u64, Listener<Request>)>>,
    responses: Mutex<Vec<(u64, Listener<Response>)>>,
    dialogs: Mutex<Vec<(u64, Listener<(String, String)>)>>,
    accept_next_dialog: AtomicBool,
    in_flight: Mutex<HashMap<String, Request>>,
    failures: Mutex<HashSet<String>>,
    routes: Mutex<Vec<RouteEntry>>,
    active_routes: AtomicU64,
    fetch_enabled: AtomicBool,
    next_id: AtomicU64,
    media: Mutex<Media>,
    viewport: Mutex<(u32, u32)>,
    mouse: Mutex<(f64, f64, u32)>,
    modifiers: Mutex<u32>,
    events: broadcast::Sender<Event>,
}

#[derive(Clone)]
struct Media {
    color_scheme: String,
    reduced_motion: String,
    forced_colors: String,
    media: String,
}

/// A browser tab. Cheap to clone; all clones drive the same tab.
#[derive(Clone)]
pub struct Page {
    pub(crate) inner: Arc<PageInner>,
}

impl Page {
    pub(crate) async fn create(context: Context) -> Result<Page> {
        let browser = context.inner.browser.clone();
        let target = browser
            .call(
                "Target.createTarget",
                json!({"url": "about:blank", "browserContextId": context.inner.id}),
            )
            .await?["targetId"]
            .as_str()
            .ok_or_else(|| Error::new("no target id"))?
            .to_owned();
        let session = browser
            .call(
                "Target.attachToTarget",
                json!({"targetId": target, "flatten": true}),
            )
            .await?["sessionId"]
            .as_str()
            .ok_or_else(|| Error::new("no session id"))?
            .to_owned();
        let receiver = browser.connection.subscribe_session(&session);
        let options = context.inner.options.clone();
        let (events, _) = broadcast::channel(1024);
        let inner = Arc::new(PageInner {
            connection: browser.connection.clone(),
            context,
            session,
            target: target.clone(),
            closed: AtomicBool::new(false),
            navigation: watch::channel(Navigation {
                url: "about:blank".into(),
                main_frame: target,
                ..Navigation::default()
            })
            .0,
            console: Mutex::default(),
            errors: Mutex::default(),
            requests: Mutex::default(),
            finished: Mutex::default(),
            failed: Mutex::default(),
            responses: Mutex::default(),
            dialogs: Mutex::default(),
            accept_next_dialog: AtomicBool::new(false),
            in_flight: Mutex::default(),
            failures: Mutex::default(),
            routes: Mutex::default(),
            active_routes: AtomicU64::new(0),
            fetch_enabled: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            media: Mutex::new(Media {
                color_scheme: options
                    .color_scheme
                    .clone()
                    .unwrap_or_else(|| "light".into()),
                reduced_motion: options
                    .reduced_motion
                    .clone()
                    .unwrap_or_else(|| "no-preference".into()),
                forced_colors: "none".into(),
                media: String::new(),
            }),
            viewport: Mutex::new(options.viewport),
            mouse: Mutex::new((0.0, 0.0, 0)),
            modifiers: Mutex::new(0),
            events,
        });
        let page = Page { inner };
        tokio::spawn(page.clone().pump(receiver));
        for (method, params) in [
            ("Page.enable", json!({})),
            ("Page.setLifecycleEventsEnabled", json!({"enabled": true})),
            ("Runtime.enable", json!({})),
            ("Network.enable", json!({})),
            ("Log.enable", json!({})),
            ("Accessibility.enable", json!({})),
            (
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled": true}),
            ),
            (
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source": ENGINE}),
            ),
        ] {
            page.call(method, params).await?;
        }
        if options.ignore_https_errors {
            page.call(
                "Security.setIgnoreCertificateErrors",
                json!({"ignore": true}),
            )
            .await?;
        }
        if options.has_touch {
            page.call(
                "Emulation.setTouchEmulationEnabled",
                json!({"enabled": true, "maxTouchPoints": 1}),
            )
            .await?;
        }
        page.apply_viewport().await?;
        page.apply_media().await?;
        page.call("Runtime.evaluate", json!({"expression": ENGINE}))
            .await?;
        OPEN.lock().unwrap().push((
            std::thread::current().name().unwrap_or_default().to_owned(),
            page.clone(),
        ));
        Ok(page)
    }

    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value> {
        if self.is_closed() {
            return Err(Error::new(format!("{method}: the page is closed")));
        }
        self.inner
            .connection
            .call(Some(&self.inner.session), method, params)
            .await
    }

    pub fn context(&self) -> Context {
        self.inner.context.clone()
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Relaxed)
    }

    pub async fn close(&self) -> Result<()> {
        if self.inner.closed.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        self.inner
            .context
            .inner
            .browser
            .call("Target.closeTarget", json!({"targetId": self.inner.target}))
            .await
            .ok();
        self.inner
            .connection
            .unsubscribe_session(&self.inner.session);
        Ok(())
    }

    // ---- events -------------------------------------------------------------------------

    async fn pump(self, mut receiver: tokio::sync::mpsc::UnboundedReceiver<Event>) {
        while let Some(event) = receiver.recv().await {
            let _ = self.inner.events.send(event.clone());
            let p = &event.params;
            match event.method.as_str() {
                "Page.frameNavigated" if p["frame"]["parentId"].is_null() => {
                    let (url, loader, frame) = (
                        p["frame"]["url"].as_str().unwrap_or_default().to_owned(),
                        p["frame"]["loaderId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                        p["frame"]["id"].as_str().unwrap_or_default().to_owned(),
                    );
                    self.inner.in_flight.lock().unwrap().clear();
                    self.inner.navigation.send_modify(|n| {
                        n.url = url;
                        n.loader = loader;
                        n.main_frame = frame;
                    });
                }
                "Page.navigatedWithinDocument" => {
                    let frame = p["frameId"].as_str().unwrap_or_default();
                    if frame == self.inner.navigation.borrow().main_frame {
                        let url = p["url"].as_str().unwrap_or_default().to_owned();
                        self.inner.navigation.send_modify(|n| n.url = url);
                    }
                }
                "Page.lifecycleEvent" if p["name"] == "load" => {
                    let (frame, loader) = (
                        p["frameId"].as_str().unwrap_or_default(),
                        p["loaderId"].as_str().unwrap_or_default().to_owned(),
                    );
                    if frame == self.inner.navigation.borrow().main_frame {
                        self.inner.navigation.send_modify(|n| n.loaded = loader);
                    }
                }
                "Runtime.consoleAPICalled" => {
                    let text = p["args"]
                        .as_array()
                        .map(|args| args.iter().map(remote_text).collect::<Vec<_>>().join(" "))
                        .unwrap_or_default();
                    let kind = match p["type"].as_str().unwrap_or("log") {
                        "warning" => "warning",
                        other => other,
                    }
                    .to_owned();
                    self.emit(&self.inner.console, &ConsoleMessage { kind, text });
                }
                "Log.entryAdded" => {
                    let entry = &p["entry"];
                    let kind = match entry["level"].as_str().unwrap_or("info") {
                        "verbose" => "debug",
                        other => other,
                    }
                    .to_owned();
                    self.emit(
                        &self.inner.console,
                        &ConsoleMessage {
                            kind,
                            text: entry["text"].as_str().unwrap_or_default().to_owned(),
                        },
                    );
                }
                "Runtime.exceptionThrown" => {
                    let details = &p["exceptionDetails"];
                    let description = details["exception"]["description"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| details["exception"]["value"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| {
                            details["text"]
                                .as_str()
                                .unwrap_or("Uncaught exception")
                                .to_owned()
                        });
                    let first = description.lines().next().unwrap_or_default();
                    let message = first
                        .split_once(": ")
                        .filter(|(name, _)| name.ends_with("Error") || name == &"Error")
                        .map(|(_, m)| m)
                        .unwrap_or(first)
                        .to_owned();
                    self.emit(&self.inner.errors, &message);
                }
                "Page.javascriptDialogOpening" => {
                    let dialog = (
                        p["type"].as_str().unwrap_or_default().to_owned(),
                        p["message"].as_str().unwrap_or_default().to_owned(),
                    );
                    self.emit(&self.inner.dialogs, &dialog);
                    let accept = self.inner.accept_next_dialog.swap(false, Ordering::SeqCst);
                    let page = self.clone();
                    tokio::spawn(async move {
                        page.call("Page.handleJavaScriptDialog", json!({"accept": accept}))
                            .await
                            .ok();
                    });
                }
                "Network.requestWillBeSent" => {
                    let request = &p["request"];
                    let entry = Request {
                        url: request["url"].as_str().unwrap_or_default().to_owned(),
                        method: request["method"].as_str().unwrap_or("GET").to_owned(),
                        headers: headers(&request["headers"]),
                        post_data: request["postData"].as_str().map(|d| d.as_bytes().to_vec()),
                        failure: None,
                        network_id: p["requestId"].as_str().map(str::to_owned),
                    };
                    self.inner.in_flight.lock().unwrap().insert(
                        p["requestId"].as_str().unwrap_or_default().to_owned(),
                        entry.clone(),
                    );
                    self.emit(&self.inner.requests, &entry);
                }
                "Network.responseReceived" => {
                    let response = &p["response"];
                    self.emit(
                        &self.inner.responses,
                        &Response {
                            url: response["url"].as_str().unwrap_or_default().to_owned(),
                            status: response["status"].as_u64().unwrap_or(0) as u16,
                            headers: headers(&response["headers"]),
                        },
                    );
                }
                "Network.loadingFinished" => {
                    if let Some(request) = self
                        .inner
                        .in_flight
                        .lock()
                        .unwrap()
                        .remove(p["requestId"].as_str().unwrap_or_default())
                    {
                        self.emit(&self.inner.finished, &request);
                    }
                }
                "Network.loadingFailed" => {
                    let id = p["requestId"].as_str().unwrap_or_default().to_owned();
                    self.inner.failures.lock().unwrap().insert(id.clone());
                    if let Some(mut request) = self.inner.in_flight.lock().unwrap().remove(&id) {
                        request.failure =
                            Some(p["errorText"].as_str().unwrap_or("failed").to_owned());
                        self.emit(&self.inner.failed, &request);
                    }
                }
                "Fetch.requestPaused" => self.paused(p.clone()),
                // Closing the page (or its context) detaches it too, and that event can arrive before
                // the session is unsubscribed: only a page that was not being closed went away.
                "Inspector.detached" if self.inner.closed.load(Ordering::Relaxed) => {}
                "Inspector.detached" | "Inspector.targetCrashed" => {
                    self.emit(
                        &self.inner.errors,
                        &format!("the page went away ({})", event.method),
                    );
                }
                _ => {}
            }
        }
    }

    fn emit<T>(&self, listeners: &Mutex<Vec<(u64, Listener<T>)>>, value: &T) {
        let listeners: Vec<_> = listeners
            .lock()
            .unwrap()
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        for listener in listeners {
            listener(value);
        }
    }

    fn listen<T>(&self, list: &Mutex<Vec<(u64, Listener<T>)>>, listener: Listener<T>) -> u64 {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        list.lock().unwrap().push((id, listener));
        id
    }

    /// Console messages, including the browser's own log (failed loads), like Playwright's
    /// `page.on("console")`. Returns an id for `off`.
    pub fn on_console(&self, listener: impl Fn(&ConsoleMessage) + Send + Sync + 'static) -> u64 {
        self.listen(&self.inner.console, Arc::new(listener))
    }
    /// Uncaught exceptions (`page.on("pageerror")`), as their message.
    pub fn on_page_error(&self, listener: impl Fn(&String) + Send + Sync + 'static) -> u64 {
        self.listen(&self.inner.errors, Arc::new(listener))
    }
    pub fn on_request(&self, listener: impl Fn(&Request) + Send + Sync + 'static) -> u64 {
        self.listen(&self.inner.requests, Arc::new(listener))
    }
    pub fn on_request_finished(&self, listener: impl Fn(&Request) + Send + Sync + 'static) -> u64 {
        self.listen(&self.inner.finished, Arc::new(listener))
    }
    pub fn on_request_failed(&self, listener: impl Fn(&Request) + Send + Sync + 'static) -> u64 {
        self.listen(&self.inner.failed, Arc::new(listener))
    }
    pub fn on_response(&self, listener: impl Fn(&Response) + Send + Sync + 'static) -> u64 {
        self.listen(&self.inner.responses, Arc::new(listener))
    }
    /// Browser dialogs as (type, message); dismissed by default unless `accept_next_dialog` is set.
    pub fn on_dialog(&self, listener: impl Fn(&(String, String)) + Send + Sync + 'static) -> u64 {
        self.listen(&self.inner.dialogs, Arc::new(listener))
    }
    /// Accepts the next browser dialog (such as a `beforeunload` confirmation) instead of dismissing it.
    pub fn accept_next_dialog(&self) {
        self.inner.accept_next_dialog.store(true, Ordering::SeqCst);
    }
    /// Removes a listener added with any `on_*`.
    pub fn off(&self, id: u64) {
        self.inner.console.lock().unwrap().retain(|(i, _)| *i != id);
        self.inner.errors.lock().unwrap().retain(|(i, _)| *i != id);
        self.inner
            .requests
            .lock()
            .unwrap()
            .retain(|(i, _)| *i != id);
        self.inner
            .finished
            .lock()
            .unwrap()
            .retain(|(i, _)| *i != id);
        self.inner.failed.lock().unwrap().retain(|(i, _)| *i != id);
        self.inner
            .responses
            .lock()
            .unwrap()
            .retain(|(i, _)| *i != id);
        self.inner.dialogs.lock().unwrap().retain(|(i, _)| *i != id);
    }

    /// Collects console errors and page errors into a list, skipping messages `ignore` accepts:
    /// the usual "no errors happened" check of the browser suites.
    pub fn collect_errors(
        &self,
        ignore: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Arc<Mutex<Vec<String>>> {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let ignore = Arc::new(ignore);
        let (sink, filter) = (errors.clone(), ignore.clone());
        self.on_page_error(move |message| {
            if !filter(message) {
                sink.lock().unwrap().push(message.clone())
            }
        });
        let sink = errors.clone();
        self.on_console(move |message| {
            if message.kind == "error" && !ignore(&message.text) {
                sink.lock().unwrap().push(message.text.clone())
            }
        });
        errors
    }

    // ---- navigation ---------------------------------------------------------------------

    pub fn url(&self) -> String {
        self.inner.navigation.borrow().url.clone()
    }

    /// Navigates and waits for the load event, like Playwright's `goto`.
    pub async fn goto(&self, url: &str) -> Result<()> {
        let result = self.call("Page.navigate", json!({"url": url})).await?;
        if let Some(error) = result["errorText"].as_str().filter(|e| !e.is_empty()) {
            return Err(Error::new(format!("goto {url}: {error}")));
        }
        let Some(loader) = result["loaderId"].as_str() else {
            return Ok(());
        };
        self.wait_loaded(loader).await
    }

    async fn wait_loaded(&self, loader: &str) -> Result<()> {
        let mut navigation = self.inner.navigation.subscribe();
        let deadline = tokio::time::sleep(DEFAULT_TIMEOUT);
        tokio::pin!(deadline);
        // A later navigation (a redirect from script) replaces this one; its load counts.
        let mut seen = false;
        loop {
            {
                let n = navigation.borrow_and_update();
                seen |= n.loader == loader;
                if n.loaded == loader || (seen && n.loader != loader && n.loaded == n.loader) {
                    return Ok(());
                }
            }
            tokio::select! {
                changed = navigation.changed() => { changed.map_err(|_| Error::new("the page closed during navigation"))?; }
                _ = &mut deadline => return Err(Error::new(format!("navigation did not load within {DEFAULT_TIMEOUT:?}"))),
            }
        }
    }

    pub async fn reload(&self) -> Result<()> {
        let before = self.inner.navigation.borrow().loader.clone();
        let mut navigation = self.inner.navigation.subscribe();
        self.call("Page.reload", json!({})).await?;
        let deadline = Instant::now() + DEFAULT_TIMEOUT;
        loop {
            {
                let n = navigation.borrow_and_update();
                if n.loader != before && n.loaded == n.loader {
                    return Ok(());
                }
            }
            if tokio::time::timeout_at(deadline.into(), navigation.changed())
                .await
                .is_err()
            {
                return Err(Error::new("reload did not load in time"));
            }
        }
    }

    pub async fn go_back(&self) -> Result<()> {
        self.evaluate("() => history.back()").await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(())
    }

    /// Waits until the page's URL satisfies `test` (pushState navigations included).
    pub async fn wait_for_url(&self, test: impl Fn(&url::Url) -> bool) -> Result<()> {
        let mut navigation = self.inner.navigation.subscribe();
        let deadline = Instant::now() + DEFAULT_TIMEOUT;
        loop {
            let current = navigation.borrow_and_update().url.clone();
            if url::Url::parse(&current).is_ok_and(|u| test(&u)) {
                return Ok(());
            }
            if tokio::time::timeout_at(deadline.into(), navigation.changed())
                .await
                .is_err()
            {
                return Err(Error::new(format!(
                    "the URL did not match in time (it is {current})"
                )));
            }
        }
    }

    /// Waits for the main frame's load event (Playwright's `waitForLoadState("load")`).
    pub async fn wait_for_load(&self) -> Result<()> {
        let loader = self.inner.navigation.borrow().loader.clone();
        if loader.is_empty() {
            return Ok(());
        }
        self.wait_loaded(&loader).await
    }

    /// Runs `action` and waits for the subsequent main-frame load event (`waitForEvent("load")`).
    pub async fn wait_for_next_load<F: Future<Output = Result<()>>>(
        &self,
        action: F,
    ) -> Result<()> {
        let before = self.inner.navigation.borrow().loader.clone();
        let mut navigation = self.inner.navigation.subscribe();
        action.await?;
        let deadline = Instant::now() + DEFAULT_TIMEOUT;
        loop {
            {
                let n = navigation.borrow_and_update();
                if n.loader != before && n.loaded == n.loader {
                    return Ok(());
                }
            }
            if tokio::time::timeout_at(deadline.into(), navigation.changed())
                .await
                .is_err()
            {
                return Err(Error::new("navigation did not load in time"));
            }
        }
    }

    /// Waits until there are no in-flight network requests for 150 ms (Playwright's `networkidle`),
    /// bounded to 2 s so background streams never stall multi-view audits.
    pub async fn wait_for_network_idle(&self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut idle_since: Option<Instant> = None;
        loop {
            let empty = self.inner.in_flight.lock().unwrap().is_empty();
            if empty {
                let since = *idle_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(150) {
                    return Ok(());
                }
            } else {
                idle_since = None;
            }
            if Instant::now() > deadline {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Replaces the main frame's HTML content (Playwright's `page.setContent`).
    pub async fn set_content(&self, html: &str) -> Result<()> {
        let frame_id = self.inner.navigation.borrow().main_frame.clone();
        self.call(
            "Page.setDocumentContent",
            json!({"frameId": frame_id, "html": html}),
        )
        .await?;
        self.call("Runtime.evaluate", json!({"expression": ENGINE}))
            .await?;
        Ok(())
    }

    pub async fn title(&self) -> Result<String> {
        Ok(self
            .evaluate("() => document.title")
            .await?
            .as_str()
            .unwrap_or_default()
            .to_owned())
    }

    pub async fn content(&self) -> Result<String> {
        Ok(self
            .evaluate("() => document.documentElement.outerHTML")
            .await?
            .as_str()
            .unwrap_or_default()
            .to_owned())
    }

    // ---- scripts ------------------------------------------------------------------------

    /// Runs a function (`() => …`, `async () => …`) or an expression in the page and returns
    /// its JSON value. Promises are awaited.
    pub async fn evaluate(&self, script: &str) -> Result<Value> {
        self.evaluate_expression(&invocation(script, None)).await
    }

    /// Runs a function with one JSON argument: `evaluate_with("(n) => n + 1", json!(1))`.
    pub async fn evaluate_with(&self, function: &str, arg: Value) -> Result<Value> {
        self.evaluate_expression(&invocation(function, Some(&arg)))
            .await
    }

    pub(crate) async fn evaluate_expression(&self, expression: &str) -> Result<Value> {
        let result = self.call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true, "awaitPromise": true, "userGesture": true})).await?;
        if let Some(details) = result.get("exceptionDetails") {
            let description = details["exception"]["description"]
                .as_str()
                .or(details["text"].as_str())
                .unwrap_or("evaluation failed");
            return Err(Error::new(format!("page script failed: {description}")));
        }
        Ok(result["result"]["value"].clone())
    }

    /// Polls a function until it returns a truthy value (Playwright's `waitForFunction`).
    pub async fn wait_for_function(&self, script: &str) -> Result<Value> {
        self.wait_for_function_with(script, Value::Null).await
    }

    pub async fn wait_for_function_with(&self, function: &str, arg: Value) -> Result<Value> {
        let expression = invocation(function, Some(&arg));
        let deadline = Instant::now() + DEFAULT_TIMEOUT;
        let mut last_error: Option<Error>;
        loop {
            match self.evaluate_expression(&expression).await {
                Ok(value) if truthy(&value) => return Ok(value),
                Ok(_) => last_error = None,
                Err(error) if transient(&error) => last_error = Some(error),
                Err(error) => return Err(error),
            }
            if Instant::now() > deadline {
                return Err(Error::new(format!(
                    "waitForFunction timed out after {DEFAULT_TIMEOUT:?}: {function}{}",
                    last_error.map(|e| format!(" ({e})")).unwrap_or_default()
                )));
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    /// Tracks the page's WebSockets whose URL contains `needle` (call before `goto`), so a test
    /// can drop them with [`Page::drop_sockets`] and count reconnections with
    /// [`Page::sockets_opened`] (what Playwright's `routeWebSocket` was used for).
    pub async fn track_sockets(&self, needle: &str) -> Result<()> {
        self.add_init_script(&format!(r#"(() => {{
            const Native = globalThis.WebSocket, needle = {needle}, sockets = [];
            let opened = 0;
            class Tracked extends Native {{
                constructor(url, protocols) {{ super(url, protocols); if (String(url).includes(needle)) {{ sockets.push(this); opened++; }} }}
            }}
            globalThis.WebSocket = Tracked;
            globalThis.__darSockets = {{ opened: () => opened, drop: (code, reason) => {{ for (const socket of sockets.splice(0)) socket.close(code, reason); }} }};
        }})();"#, needle = serde_json::to_string(needle)?)).await
    }

    /// How many tracked WebSockets this document has opened.
    pub async fn sockets_opened(&self) -> Result<u64> {
        Ok(self
            .evaluate("() => globalThis.__darSockets?.opened() ?? 0")
            .await?
            .as_u64()
            .unwrap_or_default())
    }

    /// Closes the tracked WebSockets from the page's side (`code` 4000–4999), as a dropped
    /// connection; the app should reconnect.
    pub async fn drop_sockets(&self, code: u16, reason: &str) -> Result<()> {
        self.evaluate_with(
            "([code, reason]) => globalThis.__darSockets.drop(code, reason)",
            json!([code, reason]),
        )
        .await?;
        Ok(())
    }

    /// A script for every document this page loads from now on, before the page's own.
    pub async fn add_init_script(&self, source: &str) -> Result<()> {
        self.call(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source": source}),
        )
        .await?;
        Ok(())
    }

    // ---- locators -----------------------------------------------------------------------

    pub fn locator(&self, css: &str) -> Locator {
        Locator::new(self.clone(), vec![Step::Css(css.to_owned())])
    }
    pub fn get_by_role(&self, role: &str, name: impl Into<Pattern>) -> Locator {
        Locator::new(self.clone(), vec![Step::role(role, name.into())])
    }
    pub fn get_by_text(&self, text: impl Into<Pattern>) -> Locator {
        Locator::new(self.clone(), vec![Step::Text(text.into())])
    }
    pub fn get_by_label(&self, text: impl Into<Pattern>) -> Locator {
        Locator::new(self.clone(), vec![Step::Label(text.into())])
    }
    pub fn get_by_placeholder(&self, text: impl Into<Pattern>) -> Locator {
        Locator::new(self.clone(), vec![Step::Placeholder(text.into())])
    }
    pub fn get_by_title(&self, text: impl Into<Pattern>) -> Locator {
        Locator::new(self.clone(), vec![Step::Title(text.into())])
    }
    /// Locators inside a same-origin iframe (Playwright's `frameLocator`).
    pub fn frame_locator(&self, css: &str) -> Locator {
        Locator::new(self.clone(), vec![Step::Css(css.to_owned()), Step::Frame])
    }

    // ---- input --------------------------------------------------------------------------

    pub fn keyboard(&self) -> Keyboard {
        Keyboard { page: self.clone() }
    }

    pub fn mouse(&self) -> Mouse {
        Mouse { page: self.clone() }
    }

    pub async fn set_viewport_size(&self, width: u32, height: u32) -> Result<()> {
        *self.inner.viewport.lock().unwrap() = (width, height);
        self.apply_viewport().await
    }

    pub fn viewport_size(&self) -> (u32, u32) {
        *self.inner.viewport.lock().unwrap()
    }

    async fn apply_viewport(&self) -> Result<()> {
        let (width, height) = *self.inner.viewport.lock().unwrap();
        let options = &self.inner.context.inner.options;
        self.call("Emulation.setDeviceMetricsOverride", json!({"width": width, "height": height, "deviceScaleFactor": options.device_scale_factor, "mobile": options.is_mobile,
            "screenWidth": width, "screenHeight": height})).await?;
        Ok(())
    }

    /// Playwright's `emulateMedia`: each `Some` changes that feature; the rest stay.
    pub async fn emulate_media(
        &self,
        color_scheme: Option<&str>,
        reduced_motion: Option<&str>,
        forced_colors: Option<&str>,
    ) -> Result<()> {
        {
            let mut media = self.inner.media.lock().unwrap();
            if let Some(value) = color_scheme {
                media.color_scheme = value.into();
            }
            if let Some(value) = reduced_motion {
                media.reduced_motion = value.into();
            }
            if let Some(value) = forced_colors {
                media.forced_colors = value.into();
            }
        }
        self.apply_media().await
    }

    pub async fn emulate_print(&self, print: bool) -> Result<()> {
        self.inner.media.lock().unwrap().media = if print { "print".into() } else { String::new() };
        self.apply_media().await
    }

    async fn apply_media(&self) -> Result<()> {
        let media = self.inner.media.lock().unwrap().clone();
        self.call(
            "Emulation.setEmulatedMedia",
            json!({"media": media.media, "features": [
                {"name": "prefers-color-scheme", "value": media.color_scheme},
                {"name": "prefers-reduced-motion", "value": media.reduced_motion},
                {"name": "forced-colors", "value": media.forced_colors},
            ]}),
        )
        .await?;
        Ok(())
    }

    pub async fn bring_to_front(&self) -> Result<()> {
        self.call("Page.bringToFront", json!({})).await?;
        Ok(())
    }

    /// Saves a PNG of the viewport, or of the whole page.
    pub async fn screenshot(&self, path: impl AsRef<Path>, full_page: bool) -> Result<Vec<u8>> {
        let mut params = json!({"format": "png", "captureBeyondViewport": full_page});
        if full_page {
            let metrics = self.call("Page.getLayoutMetrics", json!({})).await?;
            let size = &metrics["cssContentSize"];
            params["clip"] = json!({"x": 0, "y": 0, "width": size["width"], "height": size["height"], "scale": 1});
        }
        self.capture(path.as_ref(), params).await
    }

    pub(crate) async fn capture(&self, path: &Path, params: Value) -> Result<Vec<u8>> {
        use base64::Engine as _;
        let data = self.call("Page.captureScreenshot", params).await?["data"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| Error::new(e.to_string()))?;
        if !path.as_os_str().is_empty() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, &bytes)?;
        }
        Ok(bytes)
    }

    /// Waits for a short time, like Playwright's `waitForTimeout` (prefer a real condition).
    pub async fn wait_for_timeout(&self, milliseconds: u64) {
        tokio::time::sleep(Duration::from_millis(milliseconds)).await;
    }

    // ---- downloads ----------------------------------------------------------------------

    /// Runs `action` and returns the download it starts, once saved.
    pub async fn wait_for_download<F: Future<Output = Result<()>>>(
        &self,
        action: F,
    ) -> Result<Download> {
        let browser = self.inner.context.inner.browser.clone();
        let mut events = browser.connection.subscribe_browser();
        action.await?;
        let deadline = Instant::now() + DEFAULT_TIMEOUT;
        let mut started: Option<(String, String, String)> = None;
        loop {
            let event = tokio::time::timeout_at(deadline.into(), events.recv())
                .await
                .map_err(|_| Error::new("no download finished in time"))?
                .ok_or_else(|| Error::new("the browser closed"))?;
            match event.method.as_str() {
                "Browser.downloadWillBegin" if started.is_none() => {
                    started = Some((
                        event.params["guid"].as_str().unwrap_or_default().into(),
                        event.params["url"].as_str().unwrap_or_default().into(),
                        event.params["suggestedFilename"]
                            .as_str()
                            .unwrap_or_default()
                            .into(),
                    ));
                }
                "Browser.downloadProgress" => {
                    let Some((guid, url, name)) = &started else {
                        continue;
                    };
                    if event.params["guid"] != json!(guid) {
                        continue;
                    }
                    match event.params["state"].as_str() {
                        Some("completed") => {
                            let path = event.params["filePath"]
                                .as_str()
                                .map(PathBuf::from)
                                .unwrap_or_else(|| self.inner.context.inner.downloads.join(guid));
                            return Ok(Download {
                                url: url.clone(),
                                suggested_filename: name.clone(),
                                path,
                            });
                        }
                        Some("canceled") => return Err(Error::new("the download was cancelled")),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    // ---- routing ------------------------------------------------------------------------

    /// Intercepts requests whose URL matches (Playwright's `page.route`). The handler must
    /// fulfill, abort, continue or fetch the route. `times` limits how often it applies.
    pub async fn route<F, Fut>(
        &self,
        pattern: impl Into<RouteMatch>,
        times: Option<u32>,
        handler: F,
    ) -> Result<u64>
    where
        F: Fn(Route) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let handler: RouteHandler = Arc::new(move |route| Box::pin(handler(route)));
        self.inner.routes.lock().unwrap().push(RouteEntry {
            id,
            pattern: pattern.into(),
            handler,
            remaining: times,
        });
        if !self.inner.fetch_enabled.swap(true, Ordering::Relaxed) {
            self.call(
                "Fetch.enable",
                json!({"patterns": [{"urlPattern": "*", "requestStage": "Request"}]}),
            )
            .await?;
        }
        Ok(id)
    }

    pub fn unroute(&self, id: u64) {
        self.inner
            .routes
            .lock()
            .unwrap()
            .retain(|entry| entry.id != id);
    }

    /// Removes every route and waits for handlers still running (`unrouteAll({behavior: "wait"})`).
    pub async fn unroute_all(&self) -> Result<()> {
        self.inner.routes.lock().unwrap().clear();
        let deadline = Instant::now() + DEFAULT_TIMEOUT;
        while self.inner.active_routes.load(Ordering::Relaxed) > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if self.inner.fetch_enabled.swap(false, Ordering::Relaxed) {
            self.call("Fetch.disable", json!({})).await?;
        }
        Ok(())
    }

    fn paused(&self, params: Value) {
        let url = params["request"]["url"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let handler = {
            let mut routes = self.inner.routes.lock().unwrap();
            // The newest route wins, as in Playwright.
            let position = routes.iter().rposition(|entry| entry.pattern.matches(&url));
            position.map(|index| {
                let handler = routes[index].handler.clone();
                if let Some(remaining) = routes[index].remaining.as_mut() {
                    *remaining -= 1;
                    if *remaining == 0 {
                        routes.remove(index);
                    }
                }
                handler
            })
        };
        let page = self.clone();
        let request_id = params["requestId"].as_str().unwrap_or_default().to_owned();
        let Some(handler) = handler else {
            tokio::spawn(async move {
                page.call("Fetch.continueRequest", json!({"requestId": request_id}))
                    .await
                    .ok();
            });
            return;
        };
        let request = Request {
            url,
            method: params["request"]["method"]
                .as_str()
                .unwrap_or("GET")
                .to_owned(),
            headers: headers(&params["request"]["headers"]),
            post_data: params["request"]["postData"]
                .as_str()
                .map(|d| d.as_bytes().to_vec()),
            failure: None,
            network_id: params["networkId"].as_str().map(str::to_owned),
        };
        let route = Route {
            page: page.clone(),
            request_id,
            request,
            handled: Arc::new(AtomicBool::new(false)),
        };
        self.inner.active_routes.fetch_add(1, Ordering::Relaxed);
        tokio::spawn(async move {
            let fallback = route.clone();
            if let Err(error) = handler(route).await {
                page.emit(
                    &page.inner.errors,
                    &format!("route handler failed: {error}"),
                );
            }
            if !fallback.handled.load(Ordering::Relaxed) {
                fallback.continue_().await.ok();
            }
            page.inner.active_routes.fetch_sub(1, Ordering::Relaxed);
        });
    }

    /// Requests sent with this page's cookies (Playwright's `page.request`).
    pub fn request(&self) -> ApiRequest {
        ApiRequest::new(self.inner.context.clone())
    }

    pub(crate) fn network_failed(&self, network_id: &str) -> bool {
        self.inner.failures.lock().unwrap().contains(network_id)
    }

    pub(crate) async fn set_files(&self, object_id: &str, files: Vec<FilePayload>) -> Result<()> {
        let directory = crate::temp_dir("dar-upload-")?;
        let mut paths = Vec::new();
        for (index, file) in files.iter().enumerate() {
            let folder = directory.join(index.to_string());
            std::fs::create_dir_all(&folder)?;
            let path = folder.join(&file.name);
            std::fs::write(&path, &file.buffer)?;
            paths.push(path);
        }
        self.call(
            "DOM.setFileInputFiles",
            json!({"files": paths, "objectId": object_id}),
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn mouse_event(
        &self,
        kind: &str,
        x: f64,
        y: f64,
        button: &str,
        click_count: u32,
    ) -> Result<()> {
        let modifiers = *self.inner.modifiers.lock().unwrap();
        let buttons = match (kind, button) {
            ("mousePressed", "left") => 1,
            ("mousePressed", "right") => 2,
            ("mousePressed", "middle") => 4,
            _ => 0,
        };
        self.call("Input.dispatchMouseEvent", json!({"type": kind, "x": x, "y": y, "button": button, "buttons": buttons, "clickCount": click_count, "modifiers": modifiers})).await?;
        *self.inner.mouse.lock().unwrap() = (x, y, buttons);
        Ok(())
    }

    pub(crate) fn modifiers(&self) -> u32 {
        *self.inner.modifiers.lock().unwrap()
    }
}

fn headers(value: &Value) -> HashMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(k, v)| {
                    (
                        k.to_ascii_lowercase(),
                        v.as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn remote_text(value: &Value) -> String {
    if let Some(text) = value["value"].as_str() {
        return text.to_owned();
    }
    if !value["value"].is_null() {
        return value["value"].to_string();
    }
    if value["type"] == "undefined" {
        return "undefined".into();
    }
    value["description"]
        .as_str()
        .or(value["unserializableValue"].as_str())
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// Errors that a navigation or re-render causes, which a poll retries.
pub(crate) fn transient(error: &Error) -> bool {
    let text = error.to_string();
    [
        "Execution context was destroyed",
        "Cannot find context",
        "Inspected target navigated",
        "uniqueContextId not found",
        "__darEngine is not defined",
        "Cannot read properties of undefined (reading 'resolve')",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

/// `script` called with `arg`, or evaluated as an expression when it is not a function.
pub(crate) fn invocation(script: &str, arg: Option<&Value>) -> String {
    let trimmed = script.trim_start();
    let function = trimmed.starts_with("function")
        || trimmed.starts_with("async")
        || trimmed.starts_with('(')
        || {
            let head: String = trimmed
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            !head.is_empty() && trimmed[head.len()..].trim_start().starts_with("=>")
        };
    let arg = arg
        .map(|a| a.to_string())
        .unwrap_or_else(|| "undefined".into());
    if function {
        format!("({script})({arg})")
    } else {
        format!("({script})")
    }
}

/// An intercepted request (Playwright's `Route`).
#[derive(Clone)]
pub struct Route {
    page: Page,
    request_id: String,
    request: Request,
    handled: Arc<AtomicBool>,
}

impl Route {
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// The network error, if the page already gave up on this request.
    pub fn failure(&self) -> Option<String> {
        self.request
            .network_id
            .as_deref()
            .filter(|id| self.page.network_failed(id))
            .map(|_| "net::ERR_ABORTED".into())
    }

    pub async fn fulfill(&self, response: Fulfill) -> Result<()> {
        use base64::Engine as _;
        self.handled.store(true, Ordering::Relaxed);
        let mut headers: Vec<Value> = response
            .headers
            .iter()
            .map(|(name, value)| json!({"name": name, "value": value}))
            .collect();
        if let Some(content_type) = &response.content_type {
            headers.push(json!({"name": "content-type", "value": content_type}));
        }
        headers.push(json!({"name": "content-length", "value": response.body.len().to_string()}));
        self.page.call("Fetch.fulfillRequest", json!({"requestId": self.request_id, "responseCode": if response.status == 0 { 200 } else { response.status },
            "responseHeaders": headers, "body": base64::engine::general_purpose::STANDARD.encode(&response.body)})).await?;
        Ok(())
    }

    pub async fn abort(&self) -> Result<()> {
        self.handled.store(true, Ordering::Relaxed);
        self.page
            .call(
                "Fetch.failRequest",
                json!({"requestId": self.request_id, "errorReason": "Failed"}),
            )
            .await?;
        Ok(())
    }

    pub async fn continue_(&self) -> Result<()> {
        self.handled.store(true, Ordering::Relaxed);
        self.page
            .call(
                "Fetch.continueRequest",
                json!({"requestId": self.request_id}),
            )
            .await?;
        Ok(())
    }

    /// Sends the request to the server from the test (with the page's cookies) and returns
    /// its response without answering the page (Playwright's `route.fetch`).
    pub async fn fetch(&self) -> Result<ApiResponse> {
        let mut builder = self
            .page
            .request()
            .builder(&self.request.method, &self.request.url)
            .await?;
        for (name, value) in &self.request.headers {
            if !["cookie", "content-length", "host"].contains(&name.as_str()) {
                builder = builder.header(name, value);
            }
        }
        if let Some(body) = &self.request.post_data {
            builder = builder.body(body.clone());
        }
        ApiRequest::send(builder).await
    }
}

/// Keyboard input to the focused element.
pub struct Keyboard {
    page: Page,
}

impl Keyboard {
    /// Presses a key or chord: `Enter`, `Escape`, `Control+k`, `Shift+Enter`, `a`.
    pub async fn press(&self, chord: &str) -> Result<()> {
        let parts: Vec<&str> = if chord == "+" {
            vec!["+"]
        } else {
            chord.split('+').collect()
        };
        let (key, held) = parts.split_last().unwrap();
        for modifier in held {
            self.down(modifier).await?;
        }
        self.down(key).await?;
        self.up(key).await?;
        for modifier in held.iter().rev() {
            self.up(modifier).await?;
        }
        Ok(())
    }

    pub async fn down(&self, name: &str) -> Result<()> {
        let modifiers = self.page.modifiers();
        let key = keys::key(name, modifiers & keys::SHIFT != 0)
            .ok_or_else(|| Error::new(format!("unknown key {name}")))?;
        if let Some(bit) = keys::modifier(name) {
            *self.page.inner.modifiers.lock().unwrap() |= bit;
        }
        let modifiers = self.page.modifiers();
        // Control, Alt and Meta chords type nothing.
        let text = key
            .text
            .filter(|_| modifiers & (keys::CONTROL | keys::ALT | keys::META) == 0);
        let mut params = json!({"type": if text.is_some() { "keyDown" } else { "rawKeyDown" }, "key": key.key, "code": key.code,
            "windowsVirtualKeyCode": key.key_code, "nativeVirtualKeyCode": key.key_code, "modifiers": modifiers, "location": key.location});
        if let Some(text) = text {
            params["text"] = json!(text);
            params["unmodifiedText"] = json!(text);
        }
        self.page.call("Input.dispatchKeyEvent", params).await?;
        Ok(())
    }

    pub async fn up(&self, name: &str) -> Result<()> {
        let key =
            keys::key(name, false).ok_or_else(|| Error::new(format!("unknown key {name}")))?;
        if let Some(bit) = keys::modifier(name) {
            *self.page.inner.modifiers.lock().unwrap() &= !bit;
        }
        self.page.call("Input.dispatchKeyEvent", json!({"type": "keyUp", "key": key.key, "code": key.code, "windowsVirtualKeyCode": key.key_code,
            "nativeVirtualKeyCode": key.key_code, "modifiers": self.page.modifiers(), "location": key.location})).await?;
        Ok(())
    }

    /// Types text key by key (Playwright's `keyboard.type` / `pressSequentially`).
    pub async fn type_text(&self, text: &str) -> Result<()> {
        for c in text.chars() {
            if c.is_ascii() && !c.is_ascii_control() {
                self.press(&c.to_string()).await?;
            } else if c == '\n' {
                self.press("Enter").await?;
            } else {
                self.insert_text(&c.to_string()).await?;
            }
        }
        Ok(())
    }

    /// Inserts text as an IME would, with an input event and no key events.
    pub async fn insert_text(&self, text: &str) -> Result<()> {
        self.page
            .call("Input.insertText", json!({"text": text}))
            .await?;
        Ok(())
    }
}

/// Mouse input in viewport coordinates.
pub struct Mouse {
    page: Page,
}

impl Mouse {
    pub async fn move_to(&self, x: f64, y: f64, steps: u32) -> Result<()> {
        let (from_x, from_y, buttons) = *self.page.inner.mouse.lock().unwrap();
        let button = if buttons & 1 != 0 { "left" } else { "none" };
        let steps = steps.max(1);
        for step in 1..=steps {
            let t = step as f64 / steps as f64;
            let (px, py) = (from_x + (x - from_x) * t, from_y + (y - from_y) * t);
            let modifiers = self.page.modifiers();
            self.page.call("Input.dispatchMouseEvent", json!({"type": "mouseMoved", "x": px, "y": py, "button": button, "buttons": buttons, "modifiers": modifiers})).await?;
        }
        *self.page.inner.mouse.lock().unwrap() = (x, y, buttons);
        Ok(())
    }

    pub async fn down(&self) -> Result<()> {
        let (x, y, _) = *self.page.inner.mouse.lock().unwrap();
        self.page.mouse_event("mousePressed", x, y, "left", 1).await
    }

    pub async fn up(&self) -> Result<()> {
        let (x, y, _) = *self.page.inner.mouse.lock().unwrap();
        self.page
            .mouse_event("mouseReleased", x, y, "left", 1)
            .await
    }

    pub async fn click(&self, x: f64, y: f64) -> Result<()> {
        self.move_to(x, y, 1).await?;
        self.page
            .mouse_event("mousePressed", x, y, "left", 1)
            .await?;
        self.page
            .mouse_event("mouseReleased", x, y, "left", 1)
            .await
    }

    pub async fn wheel(&self, delta_x: f64, delta_y: f64) -> Result<()> {
        let (x, y, _) = *self.page.inner.mouse.lock().unwrap();
        self.page.call("Input.dispatchMouseEvent", json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": delta_x, "deltaY": delta_y, "modifiers": self.page.modifiers()})).await?;
        Ok(())
    }
}

/// Requests that carry a browser context's cookies, and store the cookies they receive.
pub struct ApiRequest {
    context: Context,
}

/// A response to an `ApiRequest`.
#[derive(Debug, Clone)]
pub struct ApiResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub url: String,
}

impl ApiResponse {
    pub fn status(&self) -> u16 {
        self.status
    }
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn json(&self) -> Result<Value> {
        serde_json::from_slice(&self.body).map_err(|error| {
            Error::new(format!(
                "{} is not JSON ({error}): {}",
                self.url,
                self.text()
            ))
        })
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

impl ApiRequest {
    pub(crate) fn new(context: Context) -> Self {
        Self { context }
    }

    pub(crate) async fn builder(&self, method: &str, url: &str) -> Result<reqwest::RequestBuilder> {
        let parsed = url::Url::parse(url)?;
        let cookies = self.context.cookies().await?;
        let header: Vec<String> = cookies
            .iter()
            .filter(|cookie| {
                let domain = cookie.domain.trim_start_matches('.');
                let host = parsed.host_str().unwrap_or_default();
                (host == domain || host.ends_with(&format!(".{domain}")))
                    && parsed.path().starts_with(&cookie.path)
                    && (!cookie.secure || parsed.scheme() == "https")
            })
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect();
        let client = crate::session::following();
        let mut builder = client.request(
            reqwest::Method::from_bytes(method.as_bytes())
                .map_err(|e| Error::new(e.to_string()))?,
            url,
        );
        if !header.is_empty() {
            builder = builder.header("cookie", header.join("; "));
        }
        Ok(builder)
    }

    pub(crate) async fn send(builder: reqwest::RequestBuilder) -> Result<ApiResponse> {
        let response = builder.send().await?;
        let status = response.status().as_u16();
        let url = response.url().to_string();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_owned(),
                    v.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect();
        let body = response.bytes().await?.to_vec();
        Ok(ApiResponse {
            status,
            headers,
            body,
            url,
        })
    }

    async fn store_cookies(&self, url: &str, response: &reqwest::Response) -> Result<()> {
        let cookies: Vec<super::Cookie> = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|value| {
                let text = value.to_str().ok()?;
                let (pair, attributes) = text.split_once(';').unwrap_or((text, ""));
                let (name, value) = pair.split_once('=')?;
                let mut cookie = super::Cookie::for_url(name.trim(), value.trim(), url);
                for attribute in attributes.split(';') {
                    let (key, value) = attribute.split_once('=').unwrap_or((attribute, ""));
                    match key.trim().to_ascii_lowercase().as_str() {
                        "path" => cookie.path = value.trim().to_owned(),
                        "httponly" => cookie.http_only = true,
                        "secure" => cookie.secure = true,
                        "samesite" => cookie.same_site = Some(value.trim().to_owned()),
                        "max-age" if value.trim().starts_with('-') || value.trim() == "0" => {
                            cookie.expires = 1.0
                        }
                        _ => {}
                    }
                }
                Some(cookie)
            })
            .collect();
        if !cookies.is_empty() {
            self.context.add_cookies(cookies).await?;
        }
        Ok(())
    }

    /// Sends a request: `data` is JSON (`Value`), and `body` raw bytes; `headers` are added.
    pub async fn fetch(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        data: Option<Value>,
        body: Option<Vec<u8>>,
    ) -> Result<ApiResponse> {
        let mut builder = self.builder(method, url).await?;
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        if let Some(data) = data {
            builder = builder.json(&data);
        }
        if let Some(body) = body {
            builder = builder.body(body);
        }
        let response = builder.send().await?;
        self.store_cookies(url, &response).await?;
        let status = response.status().as_u16();
        let final_url = response.url().to_string();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_owned(),
                    v.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect();
        let body = response.bytes().await?.to_vec();
        Ok(ApiResponse {
            status,
            headers,
            body,
            url: final_url,
        })
    }

    pub async fn get(&self, url: &str) -> Result<ApiResponse> {
        self.fetch("GET", url, &[], None, None).await
    }
    pub async fn get_with(&self, url: &str, headers: &[(&str, &str)]) -> Result<ApiResponse> {
        self.fetch("GET", url, headers, None, None).await
    }
    pub async fn post(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        data: Value,
    ) -> Result<ApiResponse> {
        self.fetch("POST", url, headers, Some(data), None).await
    }
    pub async fn post_empty(&self, url: &str, headers: &[(&str, &str)]) -> Result<ApiResponse> {
        self.fetch("POST", url, headers, None, None).await
    }
    pub async fn put(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        data: Value,
    ) -> Result<ApiResponse> {
        self.fetch("PUT", url, headers, Some(data), None).await
    }
    pub async fn put_bytes(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<ApiResponse> {
        self.fetch("PUT", url, headers, None, Some(body)).await
    }
    pub async fn patch(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        data: Value,
    ) -> Result<ApiResponse> {
        self.fetch("PATCH", url, headers, Some(data), None).await
    }
    pub async fn delete(&self, url: &str, headers: &[(&str, &str)]) -> Result<ApiResponse> {
        self.fetch("DELETE", url, headers, None, None).await
    }
}

/// Pages by the suite that opened them, for screenshots when a case fails.
static OPEN: Mutex<Vec<(String, Page)>> = Mutex::new(Vec::new());

/// Saves a full-page screenshot of every page `suite` still has open to `dir`
/// (`failure-<case>-<n>.png`), and forgets its closed pages.
pub async fn screenshot_open_pages(suite: &str, case: &str, dir: &Path) {
    let pages: Vec<Page> = {
        let mut open = OPEN.lock().unwrap();
        open.retain(|(_, page)| !page.is_closed());
        open.iter()
            .filter(|(name, _)| name == suite)
            .map(|(_, page)| page.clone())
            .collect()
    };
    for (index, page) in pages.iter().enumerate() {
        let path = dir.join(format!("failure-{case}-{index}.png"));
        let _ = tokio::time::timeout(Duration::from_secs(10), page.screenshot(&path, true)).await;
    }
}
