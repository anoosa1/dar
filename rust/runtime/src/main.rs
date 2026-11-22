//! Dar's runtime in Rust (`next/runtime/main.ts`), configured from the same environment as
//! `scripts/dev.mjs` and the NixOS module set, against SpacetimeDB running the Rust module.
//!
//! `dar-runtime` serves; `dar-runtime apps-list|apps-install|apps-enable|apps-disable|
//! apps-uninstall <root> [argument]` changes installed apps offline (`apps/install.ts`).

#![recursion_limit = "256"]

mod agent;
mod apps;
mod auth;
mod blobs;
mod books;
mod chat;
mod dav;
mod db;
mod error;
mod http;
mod installed;
mod git;
mod git_api;
mod history;
mod mail;
mod news;
mod notes_library;
mod maintenance;
mod net;
mod digests;
mod oidc;
mod oidc_store;
mod operator;
mod identity_http;
mod photos;
mod scim_model;
mod pim;
mod preferences;
mod root;
mod thumbnails;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

fn required(name: &str) -> String {
    std::env::var(name).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| {
        eprintln!("{name} is required.");
        std::process::exit(2);
    })
}

fn optional(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn secret_file(name: &str) -> String {
    let path = required(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("{name} could not be read: {e}");
        std::process::exit(2);
    });
    text.trim().to_owned()
}

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

/// The operator's offline commands.
fn command(args: &[String]) -> Option<i32> {
    let name = args.first()?;
    if !name.starts_with("apps-") {
        return None;
    }
    let Some(root) = args.get(1) else {
        eprintln!("Usage: dar-runtime {name} <root> [argument]");
        return Some(2);
    };
    match apps::manage(&PathBuf::from(root), name, args.get(2).map(String::as_str)) {
        Ok(message) => {
            println!("{message}");
            Some(0)
        }
        Err(error) => {
            eprintln!("{error}");
            Some(1)
        }
    }
}

async fn shutdown() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = command(&args) {
        std::process::exit(code);
    }
    if let Some(code) = operator::run(&args).await {
        std::process::exit(code);
    }
    let host = optional("DAR_HOST").unwrap_or_else(|| "127.0.0.1".into());
    let port: u16 = optional("DAR_PORT").unwrap_or_else(|| "4311".into()).parse().unwrap_or_else(|_| fail("DAR_PORT must be a port number."));
    let origin = required("DAR_ORIGIN");
    let development = std::env::var("DAR_DEVELOPMENT").as_deref() == Ok("1");
    let parsed = reqwest::Url::parse(&origin).unwrap_or_else(|_| fail("DAR_ORIGIN must be an origin without a path or credentials."));
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() || !parsed.username().is_empty() || parsed.password().is_some() {
        fail("DAR_ORIGIN must be an origin without a path or credentials.");
    }
    let origin = parsed.origin().ascii_serialization();
    if !development && parsed.scheme() != "https" {
        fail("Production requires an HTTPS origin.");
    }
    if development && !["127.0.0.1", "localhost", "[::1]"].contains(&parsed.host_str().unwrap_or_default()) {
        fail("Development HTTP is loopback-only.");
    }
    if development && !["127.0.0.1", "::1", "localhost"].contains(&host.as_str()) {
        fail("Development HTTP must bind to loopback.");
    }
    let root = root::open_root(&PathBuf::from(required("DAR_ROOT"))).unwrap_or_else(|e| fail(format!("The storage root could not be opened: {e}")));
    let lock = root::RootLock::acquire(&root).unwrap_or_else(|e| fail(e));
    let database_url = required("DAR_DATABASE_URL");
    let database_name = required("DAR_DATABASE_NAME");
    let issuer_url = format!("{origin}/auth/issuer");
    let db = db::Database::new(&database_url, &database_name, &secret_file("DAR_DATABASE_TOKEN_FILE"));
    if let Err(error) = db.call("service_startup", serde_json::json!([issuer_url])).await {
        fail(format!("The database rejected startup: {error}"));
    }
    let registry = apps::load_or_report(&optional("DAR_APPS_FILE").map(PathBuf::from).unwrap_or_else(|| root.join("apps.json")));
    let issuer = auth::Issuer::open(&auth::secrets_dir(&root), &issuer_url).unwrap_or_else(|e| fail(format!("The signing key could not be opened: {e}")));
    let blobs = blobs::Blobs::open(&root).unwrap_or_else(|e| fail(format!("The blob directory could not be opened: {e}")));
    let bootstrap = secret_file("DAR_BOOTSTRAP_TOKEN_FILE");
    if bootstrap.len() < 32 {
        fail("The operator bootstrap token must have at least 32 characters.");
    }
    let database_public_url = optional("DAR_DATABASE_PUBLIC_URL").unwrap_or_else(|| if development { database_url.clone() } else { format!("{origin}/spacetime") });
    let config = http::Config {
        origin: origin.clone(),
        development,
        database_public_url: database_public_url.trim_end_matches('/').to_owned() + "/",
        database_name,
        client_dist: PathBuf::from(optional("DAR_CLIENT_DIST").unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").to_owned() + "/../client/dist")),
    };
    let identity = oidc::Oidc::open(&origin, development, &auth::secrets_dir(&root), db.clone()).unwrap_or_else(|e| fail(format!("The identity provider could not start: {e}")));
    let oidc_file = optional("DAR_OIDC_FILE");
    let oidc_path = oidc_file.clone().map(PathBuf::from).unwrap_or_else(|| root.join("oidc.json"));
    identity.configure(&oidc_path, oidc_file.is_none()).await.unwrap_or_else(|e| fail(format!("The OIDC configuration was not applied: {e}")));
    let provider = identity.provider.clone();
    let chat_enabled = !installed::app_disabled(&db, "chat").await.unwrap_or(false);
    let news_allow_private = optional("DAR_NEWS_ALLOW_PRIVATE_NETWORKS").as_deref() == Some("1");
    let immich_allow_local = development || optional("DAR_IMMICH_ALLOW_LOCAL_HOST").as_deref() == Some("1");
    let codex_file = optional("DAR_CODEX_FILE").map(PathBuf::from).unwrap_or_else(|| root.join("codex.json"));
    let claude_file = optional("DAR_CLAUDE_FILE").map(PathBuf::from).unwrap_or_else(|| root.join("claude.json"));
    let chat_bridge = optional("DAR_CHAT_BRIDGE");
    let news = news::NewsRuntime::new(db.clone(), news_allow_private);
    let mail = mail::MailRuntime::open(db.clone(), blobs.clone(), &auth::secrets_dir(&root), &origin).unwrap_or_else(|e| fail(format!("The mail runtime could not start: {e}")));
    match mail.publish_providers(optional("DAR_MAIL_PROVIDERS_FILE").map(PathBuf::from).as_deref()).await {
        Ok(0) => {}
        Ok(n) => println!("Mail: {n} provider(s) from the host configuration."),
        Err(e) => fail(format!("The mail providers could not be recorded: {e}")),
    }
    let books = books::BooksRuntime::open(db.clone(), &root).await.unwrap_or_else(|e| fail(format!("The books runtime could not start: {e}")));
    let photos = Arc::new(photos::PhotosRuntime::open(db.clone(), &root).await.unwrap_or_else(|e| fail(format!("The photos runtime could not start: {e}"))));
    let immich = photos::ImmichConnector::new(db.clone(), &root, blobs.clone(), photos.clone(), immich_allow_local);
    let chat = chat::ChatRuntime::open(db.clone(), &root, chat_bridge, chat_enabled, blobs.clone()).await.unwrap_or_else(|e| fail(format!("The chat runtime could not start: {e}")));
    let agent = agent::AgentRuntime::open(db.clone(), &root, codex_file, claude_file).await.unwrap_or_else(|e| fail(format!("The agent runtime could not start: {e}")));
    let runtime = Arc::new(http::Runtime::new(
        config,
        db.clone(),
        issuer,
        blobs.clone(),
        bootstrap,
        identity.clone(),
        &root,
        registry,
        news,
        mail,
        books,
        photos,
        immich,
        chat,
        agent,
    ));
    runtime.publish_installed_apps().await.unwrap_or_else(|e| fail(format!("The installed apps could not be recorded: {e:?}")));
    // Before anything reads stored content by its database name.
    match maintenance::migrate_blobs(&db, &root).await {
        Ok((migrated, removed)) if migrated + removed > 0 => println!("Stored content is now named by digest: {migrated} blobs linked, {removed} upload-ID names removed."),
        Ok(_) => {}
        Err(e) => fail(e),
    }
    blobs.recover().await.unwrap_or_else(|e| fail(format!("Temporary uploads could not be cleared: {e}")));
    maintenance::start_loop(runtime.clone());
    runtime.checksums.nudge();
    runtime.texts.nudge();
    let checksums = runtime.checksums.clone();
    let texts = runtime.texts.clone();
    let app = axum::Router::new()
        .route("/api/v1/chat/live", axum::routing::get(http::chat_live))
        .route("/dav/git/{*rest}", axum::routing::any(http::handle))
        .route(news::NEXTCLOUD_BASE, axum::routing::any(http::handle))
        .route(&format!("{}/{{*rest}}", news::NEXTCLOUD_BASE), axum::routing::any(http::handle))
        .route(photos::IMMICH_BASE, axum::routing::any(http::handle))
        .route(&format!("{}/{{*rest}}", photos::IMMICH_BASE), axum::routing::any(http::handle))
        .fallback(http::handle)
        .with_state(runtime.clone())
        .merge(dav::router(&origin, db, blobs, checksums, texts))
        .nest("/oidc", provider.router().layer(axum::middleware::from_fn_with_state(identity, oidc::load_named_client)));
    let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await.unwrap_or_else(|e| fail(format!("Dar could not listen on {host}:{port}: {e}")));
    println!("Dar (Rust runtime) listening at {origin}.");
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder.timer(hyper_util::rt::TokioTimer::new()).header_read_timeout(std::time::Duration::from_secs(15));
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut signal = std::pin::pin!(shutdown());
    loop {
        tokio::select! {
            _ = &mut signal => break,
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let _ = stream.set_nodelay(true);
                let app = app.clone();
                let mut stop = stop_rx.clone();
                let service = hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
                    let app = app.clone();
                    req.extensions_mut().insert(axum::extract::ConnectInfo(peer));
                    async move {
                        let req = req.map(axum::body::Body::new);
                        tower::ServiceExt::oneshot(app, req).await
                    }
                });
                let conn = builder.serve_connection(hyper_util::rt::TokioIo::new(stream), service).with_upgrades();
                tokio::spawn(async move {
                    tokio::pin!(conn);
                    tokio::select! {
                        _ = &mut conn => {}
                        _ = stop.changed() => {
                            conn.as_mut().graceful_shutdown();
                            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), &mut conn).await;
                        }
                    }
                });
            }
        }
    }
    let _ = stop_tx.send(true);
    runtime.news.close().await;
    runtime.mail.close().await;
    runtime.chat.close().await;
    runtime.immich.close().await;
    runtime.photos.close().await;
    runtime.books.close().await;
    runtime.agent.close().await;
    drop(lock);
}
