//! Development server for the Rust client. Serves `rust/client/dist` for the pages the
//! port renders and proxies everything else (the `/api/v1` API, the React client's pages
//! for apps not ported yet, downloads) to a Dar instance, so both clients share one origin
//! and one session. Requests to the instance carry its own origin, which it checks.
//!
//! Usage: dar-client-serve <dist> <upstream origin> [listen address]
//! e.g.   dar-client-serve rust/client/dist http://127.0.0.1:4311 127.0.0.1:4312

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;

struct Proxy {
    dist: PathBuf,
    upstream: String,
    client: reqwest::Client,
}

/// Pages the Rust client renders; keep in step with `navigation::handles`.
fn rendered(path: &str) -> bool {
    matches!(path, "/" | "/contacts" | "/calendar")
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        _ => "application/octet-stream",
    }
}

async fn file(dist: &Path, name: &str) -> Response {
    // Only plain names inside dist are served.
    if name.is_empty() || name.contains("..") || name.contains('/') {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = dist.join(name);
    match tokio::fs::read(&path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type(&path)), (header::CACHE_CONTROL, "no-store")], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn handle(State(proxy): State<Arc<Proxy>>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    if let Some(name) = path.strip_prefix("/rust/") {
        return file(&proxy.dist, name).await;
    }
    let wants_page = request.method() == Method::GET && request.headers().get(header::ACCEPT).and_then(|a| a.to_str().ok()).is_some_and(|a| a.contains("text/html"));
    if wants_page && rendered(&path) {
        return file(&proxy.dist, "index.html").await;
    }
    forward(&proxy, request).await.unwrap_or_else(|error| (StatusCode::BAD_GATEWAY, format!("The Dar instance could not be reached: {error}")).into_response())
}

async fn forward(proxy: &Proxy, request: Request) -> Result<Response, reqwest::Error> {
    let (parts, body) = request.into_parts();
    let query = parts.uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let url = format!("{}{}{}", proxy.upstream, parts.uri.path(), query);
    let mut headers = HeaderMap::new();
    for (name, value) in &parts.headers {
        if name == header::HOST || name == header::CONNECTION || name == header::TRANSFER_ENCODING {
            continue;
        }
        headers.append(name, value.clone());
    }
    // The instance accepts writes only from its own origin.
    let origin = HeaderValue::from_str(&proxy.upstream).expect("upstream origin");
    if headers.contains_key(header::ORIGIN) {
        headers.insert(header::ORIGIN, origin);
    }
    headers.remove(header::REFERER);
    let body = reqwest::Body::wrap_stream(body.into_data_stream());
    let response = proxy.client.request(parts.method, url).headers(headers).body(body).send().await?;
    let mut builder = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        if name == header::CONNECTION || name == header::TRANSFER_ENCODING {
            continue;
        }
        builder = builder.header(name, value);
    }
    Ok(builder.body(Body::from_stream(response.bytes_stream())).expect("response"))
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(dist), Some(upstream)) = (args.first(), args.get(1)) else {
        eprintln!("Usage: dar-client-serve <dist> <upstream origin> [listen address]");
        std::process::exit(2);
    };
    let listen = args.get(2).cloned().unwrap_or_else(|| "127.0.0.1:4312".into());
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().expect("HTTP client");
    let proxy = Arc::new(Proxy { dist: PathBuf::from(dist), upstream: upstream.trim_end_matches('/').to_owned(), client });
    let app = Router::new().fallback(handle).with_state(proxy);
    let listener = tokio::net::TcpListener::bind(&listen).await.expect("listen address");
    println!("Rust client at http://{listen} (proxying {upstream})");
    axum::serve(listener, app).await.expect("server");
}
