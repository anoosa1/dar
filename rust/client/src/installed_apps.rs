//! Host page for separately installed apps (`next/client/installed-apps.tsx`):
//! loads the app's stylesheets and ES module, builds the `AppHost` bridge
//! (`next/sdk/client.tsx`), mounts the app, and cleans up on unmount.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use futures::channel::oneshot;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, future_to_promise};

use crate::api::{self, BrowserSession, Req};
use crate::shell::WorkspaceHeader;
use crate::ui::{document, window};

const OPEN_ERROR: &str = "This app could not be opened. Check its installation or try again.";

// Dynamic `import()` lives in a static `'self'` module snippet rather than
// `new Function(...)` so it complies with CSP `script-src 'self' 'wasm-unsafe-eval'`.
#[wasm_bindgen::prelude::wasm_bindgen(inline_js = r#"
export function dynamic_import(url) {
    return import(url);
}
export function report_js_error(error) {
    if (typeof reportError === "function") {
        reportError(error);
    }
}
"#)]
extern "C" {
    fn dynamic_import(url: &str) -> js_sys::Promise;
    fn report_js_error(error: &JsValue);
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstalledAppDescriptor {
    id: String,
    client_url: String,
    #[serde(default)]
    styles: Vec<String>,
    #[serde(default)]
    permissions: Vec<String>,
}

#[derive(Deserialize)]
struct InstalledAppsResponse {
    items: Vec<InstalledAppDescriptor>,
}

struct MountState {
    controller: web_sys::AbortController,
    styles: Vec<web_sys::HtmlLinkElement>,
    style_waiters: Vec<Rc<RefCell<Option<oneshot::Sender<Result<(), ()>>>>>>,
    dispose: Option<js_sys::Function>,
    closures: Vec<Box<dyn Any>>,
}

/// Hosts an installed app at `/apps/<id>`.
#[component]
pub fn InstalledAppPage(
    id: String,
    session: BrowserSession,
    on_signed_out: Callback<()>,
) -> impl IntoView {
    let error = RwSignal::new(String::new());
    let loading = RwSignal::new(true);
    let element = NodeRef::<html::Div>::new();
    let shell_app = StoredValue::new(format!("app:{id}"));
    let shell_session = StoredValue::new(session);
    let app_id = StoredValue::new(id);

    let controller = web_sys::AbortController::new().expect("AbortController");
    let state = Rc::new(RefCell::new(MountState {
        controller,
        styles: Vec::new(),
        style_waiters: Vec::new(),
        dispose: None,
        closures: Vec::new(),
    }));

    let cleanup_state = send_wrapper::SendWrapper::new(state.clone());
    on_cleanup(move || {
        let (controller, waiters, styles, finish, closures) = {
            let mut s = cleanup_state.borrow_mut();
            (
                s.controller.clone(),
                std::mem::take(&mut s.style_waiters),
                std::mem::take(&mut s.styles),
                s.dispose.take(),
                std::mem::take(&mut s.closures),
            )
        };
        controller.abort();
        for waiter in waiters {
            if let Some(tx) = waiter.borrow_mut().take() {
                let _ = tx.send(Err(()));
            }
        }
        for link in styles {
            link.remove();
        }
        // Opening another app in place unmounts this page during commit; the app's
        // own React root is unmounted just after, which React requires.
        if let Some(finish) = finish {
            schedule_dispose(finish, closures);
        }
    });

    let started = RwSignal::new(false);
    let effect_state = send_wrapper::SendWrapper::new(state);
    Effect::new(move |_| {
        let Some(mount_el) = element.get() else { return };
        if started.get_untracked() {
            return;
        }
        started.set(true);
        let state = (*effect_state).clone();
        let id = app_id.get_value();
        let session = shell_session.get_value();
        let signal = state.borrow().controller.signal();
        let fail = move || {
            if !signal.aborted() {
                let _ = error.try_set(OPEN_ERROR.into());
                let _ = loading.try_set(false);
            }
        };
        spawn_local(async move {
            let signal = state.borrow().controller.signal();
            match mount_app(&id, &session, mount_el, &signal, &state, fail.clone()).await {
                Ok(()) => {
                    if !signal.aborted() {
                        let _ = loading.try_set(false);
                    }
                }
                Err(()) => fail(),
            }
        });
    });

    view! {
        <Show when=move || loading.get() || !error.with(String::is_empty)>
            <div class="app-shell">
                <WorkspaceHeader
                    app=shell_app.get_value()
                    session=shell_session.get_value()
                    on_signed_out=on_signed_out
                />
                <main class="startup-state">
                    {move || {
                        let message = error.get();
                        if message.is_empty() {
                            view! { <p role="status">"Opening app\u{2026}"</p> }.into_any()
                        } else {
                            view! {
                                <p role="alert">{message}</p>
                                <button
                                    class="primary-button"
                                    on:click=|_| {
                                        let _ = window().location().reload();
                                    }
                                >
                                    "Try again"
                                </button>
                                <a href="/">"Return to Files"</a>
                            }
                            .into_any()
                        }
                    }}
                </main>
            </div>
        </Show>
        <div node_ref=element hidden=move || !error.with(String::is_empty)/>
    }
}

async fn mount_app(
    id: &str,
    session: &BrowserSession,
    mount_el: web_sys::HtmlDivElement,
    signal: &web_sys::AbortSignal,
    state: &Rc<RefCell<MountState>>,
    fail: impl Fn() + 'static,
) -> Result<(), ()> {
    let apps: InstalledAppsResponse = api::request(
        "/api/v1/apps",
        Req { signal: Some(signal.clone()), ..Default::default() },
    )
    .await
    .map_err(|_| ())?;
    if signal.aborted() {
        return Ok(());
    }
    let app = apps.items.into_iter().find(|a| a.id == id).ok_or(())?;

    if !app.styles.is_empty() {
        futures::future::try_join_all(
            app.styles.iter().map(|href| load_stylesheet(href, state)),
        )
        .await?;
    }
    if signal.aborted() {
        return Ok(());
    }

    let module = JsFuture::from(dynamic_import(&app.client_url))
        .await
        .map_err(|_| ())?;
    if signal.aborted() {
        return Ok(());
    }

    let mount = js_sys::Reflect::get(&module, &JsValue::from_str("mount"))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
        .ok_or(())?;

    let (host, closures) = build_host(id, session, signal, &app.permissions, fail);
    state.borrow_mut().closures = closures;

    let result = mount.call2(&module, &mount_el, &host).map_err(|_| ())?;
    let dispose = result.dyn_into::<js_sys::Function>().ok();
    if signal.aborted() {
        if let Some(finish) = dispose {
            let closures = std::mem::take(&mut state.borrow_mut().closures);
            schedule_dispose(finish, closures);
        }
        return Ok(());
    }
    state.borrow_mut().dispose = dispose;
    Ok(())
}

async fn load_stylesheet(href: &str, state: &Rc<RefCell<MountState>>) -> Result<(), ()> {
    if state.borrow().controller.signal().aborted() {
        return Err(());
    }
    let link: web_sys::HtmlLinkElement = document()
        .create_element("link")
        .map_err(|_| ())?
        .unchecked_into();
    link.set_rel("stylesheet");
    link.set_href(href);

    let (tx, rx) = oneshot::channel::<Result<(), ()>>();
    let tx = Rc::new(RefCell::new(Some(tx)));

    let on_load = {
        let tx = tx.clone();
        Closure::<dyn FnMut()>::new(move || {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(Ok(()));
            }
        })
    };
    let on_error = {
        let tx = tx.clone();
        Closure::<dyn FnMut()>::new(move || {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(Err(()));
            }
        })
    };

    link.set_onload(Some(on_load.as_ref().unchecked_ref()));
    link.set_onerror(Some(on_error.as_ref().unchecked_ref()));

    let Some(head) = document().query_selector("head").ok().flatten() else {
        link.set_onload(None);
        link.set_onerror(None);
        return Err(());
    };

    {
        let mut s = state.borrow_mut();
        s.styles.push(link.clone());
        s.style_waiters.push(tx);
    }
    if head.append_child(&link).is_err() {
        link.set_onload(None);
        link.set_onerror(None);
        return Err(());
    }

    let result = rx.await.unwrap_or(Err(()));
    link.set_onload(None);
    link.set_onerror(None);
    drop(on_load);
    drop(on_error);
    result
}

fn schedule_dispose(finish: js_sys::Function, closures: Vec<Box<dyn Any>>) {
    let callback = Closure::<dyn FnMut()>::once_into_js(move || {
        if let Err(err) = finish.call0(&JsValue::UNDEFINED) {
            report_js_error(&err);
        }
        drop(closures);
    });
    let _ = window().set_timeout_with_callback(callback.unchecked_ref());
}

fn build_host(
    id: &str,
    session: &BrowserSession,
    signal: &web_sys::AbortSignal,
    permissions: &[String],
    fail: impl Fn() + 'static,
) -> (JsValue, Vec<Box<dyn Any>>) {
    let base = format!("/api/v1/apps/{}/userData", encode_uri(id));

    let on_error = Closure::<dyn FnMut(JsValue)>::new(move |_err: JsValue| {
        fail();
    });

    let get = {
        let base = base.clone();
        let signal = signal.clone();
        Closure::<dyn FnMut(JsValue) -> js_sys::Promise>::new(move |key: JsValue| {
            let url = format!("{base}/{}", encode_uri(&js_string(&key)));
            let signal = signal.clone();
            future_to_promise(async move { js_request(&url, "GET", None, None, None, &signal).await })
        })
    };

    let put = {
        let base = base.clone();
        let signal = signal.clone();
        let csrf = session.csrf_token.clone();
        Closure::<dyn FnMut(JsValue, JsValue, JsValue, JsValue) -> js_sys::Promise>::new(
            move |key: JsValue, value: JsValue, revision: JsValue, schema_version: JsValue| {
                let url = format!("{base}/{}", encode_uri(&js_string(&key)));
                let if_match = format!("\"{}\"", js_string(&revision));
                let schema_version = if schema_version.is_undefined() {
                    JsValue::from_f64(1.0)
                } else {
                    schema_version
                };
                let payload = js_sys::Object::new();
                set_prop(&payload, "schemaVersion", &schema_version);
                set_prop(&payload, "value", &value);
                let body = match js_sys::JSON::stringify(&payload) {
                    Ok(s) => s.as_string().unwrap_or_default(),
                    Err(err) => return js_sys::Promise::reject(&err),
                };
                let signal = signal.clone();
                let csrf = csrf.clone();
                future_to_promise(async move {
                    js_request(&url, "PUT", Some(&body), Some(&csrf), Some(&if_match), &signal).await
                })
            },
        )
    };

    let delete = {
        let base = base.clone();
        let signal = signal.clone();
        let csrf = session.csrf_token.clone();
        Closure::<dyn FnMut(JsValue, JsValue) -> js_sys::Promise>::new(
            move |key: JsValue, revision: JsValue| {
                let url = format!("{base}/{}", encode_uri(&js_string(&key)));
                let if_match = format!("\"{}\"", js_string(&revision));
                let signal = signal.clone();
                let csrf = csrf.clone();
                future_to_promise(async move {
                    js_request(&url, "DELETE", None, Some(&csrf), Some(&if_match), &signal).await?;
                    Ok(JsValue::UNDEFINED)
                })
            },
        )
    };

    let list = {
        let base = base.clone();
        let signal = signal.clone();
        Closure::<dyn FnMut(JsValue) -> js_sys::Promise>::new(move |cursor: JsValue| {
            let cursor = if cursor.is_undefined() { String::new() } else { js_string(&cursor) };
            let url = format!("{base}?cursor={}", encode_uri(&cursor));
            let signal = signal.clone();
            future_to_promise(async move { js_request(&url, "GET", None, None, None, &signal).await })
        })
    };

    let regional = {
        let allowed = permissions.iter().any(|p| p == "regional:read");
        let signal = signal.clone();
        Closure::<dyn FnMut() -> js_sys::Promise>::new(move || {
            if !allowed {
                let err: JsValue = js_sys::Error::new("Permission not declared").into();
                return js_sys::Promise::reject(&err);
            }
            let signal = signal.clone();
            future_to_promise(async move {
                js_request("/api/v1/pim/preferences", "GET", None, None, None, &signal).await
            })
        })
    };

    let data = js_sys::Object::new();
    set_prop(&data, "get", get.as_ref());
    set_prop(&data, "put", put.as_ref());
    set_prop(&data, "delete", delete.as_ref());
    set_prop(&data, "list", list.as_ref());

    let host = js_sys::Object::new();
    set_prop(&host, "id", &JsValue::from_str(id));
    set_prop(&host, "session", &session_to_js(session));
    set_prop(&host, "signal", signal.as_ref());
    set_prop(&host, "onError", on_error.as_ref());
    set_prop(&host, "data", &data);
    set_prop(&host, "regional", regional.as_ref());

    let closures: Vec<Box<dyn Any>> = vec![
        Box::new(on_error),
        Box::new(get),
        Box::new(put),
        Box::new(delete),
        Box::new(list),
        Box::new(regional),
    ];
    (host.into(), closures)
}

async fn js_request(
    url: &str,
    method: &str,
    body: Option<&str>,
    csrf: Option<&str>,
    if_match: Option<&str>,
    signal: &web_sys::AbortSignal,
) -> Result<JsValue, JsValue> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    let headers = web_sys::Headers::new()?;
    let _ = headers.set("content-type", "application/json");
    if let Some(csrf) = csrf {
        let _ = headers.set("x-csrf-token", csrf);
    }
    if let Some(if_match) = if_match {
        let _ = headers.set("if-match", if_match);
    }
    init.set_headers(&headers);
    if let Some(body) = body {
        init.set_body(&JsValue::from_str(body));
    }
    init.set_signal(Some(signal));

    let response: web_sys::Response = JsFuture::from(window().fetch_with_str_and_init(url, &init))
        .await?
        .unchecked_into();
    let status = response.status();
    let json_promise = response.json().map_err(|_| {
        js_request_error("The instance returned an unexpected response. Try again shortly.", status)
    })?;
    let value = JsFuture::from(json_promise).await.map_err(|_| {
        js_request_error("The instance returned an unexpected response. Try again shortly.", status)
    })?;
    if !response.ok() {
        let message = js_sys::Reflect::get(&value, &JsValue::from_str("error"))
            .ok()
            .filter(|e| !e.is_null() && !e.is_undefined())
            .and_then(|e| js_sys::Reflect::get(&e, &JsValue::from_str("message")).ok())
            .and_then(|m| m.as_string())
            .unwrap_or_else(|| "The request could not be completed.".into());
        return Err(js_request_error(&message, status));
    }
    Ok(value)
}

fn js_request_error(message: &str, status: u16) -> JsValue {
    let err = js_sys::Error::new(message);
    let _ = js_sys::Reflect::set(&err, &JsValue::from_str("name"), &JsValue::from_str("RequestError"));
    let _ = js_sys::Reflect::set(&err, &JsValue::from_str("status"), &JsValue::from_f64(f64::from(status)));
    err.into()
}

fn session_to_js(session: &BrowserSession) -> JsValue {
    let user = js_sys::Object::new();
    set_prop(&user, "id", &JsValue::from_str(&session.user.id));
    set_prop(&user, "username", &JsValue::from_str(&session.user.username));
    set_prop(&user, "displayName", &JsValue::from_str(&session.user.display_name));
    set_prop(&user, "administrator", &JsValue::from_bool(session.user.administrator));

    let obj = js_sys::Object::new();
    set_prop(&obj, "user", &user);
    set_prop(&obj, "csrfToken", &JsValue::from_str(&session.csrf_token));
    obj.into()
}

fn set_prop(target: &JsValue, key: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(target, &JsValue::from_str(key), value);
}

fn encode_uri(value: &str) -> String {
    String::from(js_sys::encode_uri_component(value))
}

fn js_string(value: &JsValue) -> String {
    if let Some(s) = value.as_string() {
        s
    } else if let Some(n) = value.as_f64() {
        n.to_string()
    } else if let Some(b) = value.as_bool() {
        b.to_string()
    } else if value.is_null() {
        "null".into()
    } else if value.is_undefined() {
        "undefined".into()
    } else {
        js_sys::Object::from(value.clone()).to_string().into()
    }
}
