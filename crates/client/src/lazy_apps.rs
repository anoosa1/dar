//! Apps whose code loads on first use. Each `#[lazy]` function below becomes its own WebAssembly
//! chunk (split out by `wasm-split-cli` in xtask/src/client.rs), so the first page downloads only the
//! frame, Files and the app it opens. Once an app is up, the others are fetched in the
//! background, so later switches do not wait. Navigation stays in place either way.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::BrowserSession;

/// Builds a loaded app.
type Build = fn(BrowserSession, Callback<()>) -> AnyView;

macro_rules! lazy_apps {
    ($($app:ident => $kind:literal, $chunk:ident, $component:path;)*) => {
        $(
            // The chunk hands back the app's constructor, not the app: the app is then built in
            // one pass with its DOM, as an app in the main module is. (Building it as the chunk
            // arrives would subscribe its effects before its DOM, so they would run first.)
            #[leptos::lazy]
            fn $chunk() -> Build {
                |session, on_signed_out| view! { <$component session=session on_signed_out=on_signed_out/> }.into_any()
            }
        )*

        /// An app that loads lazily, named by its path segment.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum LazyApp { $($app,)* }

        impl LazyApp {
            /// The app's path segment.
            pub fn kind(self) -> &'static str {
                match self {
                    $(Self::$app => $kind,)*
                }
            }

            pub fn from_kind(kind: &str) -> Option<Self> {
                match kind {
                    $($kind => Some(Self::$app),)*
                    _ => None,
                }
            }

            async fn load(self) -> Build {
                match self {
                    $(Self::$app => $chunk().await,)*
                }
            }
        }

        /// Starts fetching every lazy app's code without rendering it.
        fn preload_all() {
            $(let _ = leptos::lazy_preload!($chunk);)*
        }
    };
}

lazy_apps! {
    News => "news", news_app, crate::news::NewsWorkspace;
    Mail => "mail", mail_app, crate::mail::MailWorkspace;
    Chat => "chat", chat_app, crate::chat::ChatWorkspace;
    Photos => "photos", photos_app, crate::photos::PhotosWorkspace;
    Books => "books", books_app, crate::books::BooksWorkspace;
    Git => "git", git_app, crate::git::GitWorkspace;
    Agent => "agent", agent_app, crate::agent::AgentWorkspace;
    Notes => "notes", notes_app, crate::workspace::Notes;
    Tasks => "tasks", tasks_app, crate::workspace::TasksWorkspace;
    Contacts => "contacts", contacts_app, crate::pim::contacts::Contacts;
    Calendar => "calendar", calendar_app, crate::pim::calendar::Calendar;
    Settings => "identity", settings_app, crate::identity::IdentitySettings;
}

thread_local! {
    static PRELOADING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Renders a lazy app: "Opening app…" until its code has loaded, then the app. A moment after
/// the first one opens, the other apps' code is fetched in the background.
#[component]
pub fn LoadApp(app: LazyApp, session: BrowserSession, on_signed_out: Callback<()>) -> AnyView {
    use futures::FutureExt;
    let mut load = Box::pin(app.load());
    // Code that has loaded already renders at once, without a frame of "Opening app…".
    if let Some(build) = load.as_mut().now_or_never() {
        return build(session, on_signed_out);
    }
    let loaded = RwSignal::new(None::<Build>);
    spawn_local(async move {
        let build = load.await;
        loaded.try_set(Some(build));
        if !PRELOADING.replace(true) {
            crate::sleep(1500).await;
            preload_all();
        }
    });
    // The app is built under this closure, so it is disposed with LoadApp.
    (move || match loaded.get() {
        Some(build) => build(session.clone(), on_signed_out),
        // Inside the app shell: the rail and top bar stay while the app's code arrives.
        None => {
            let kind = if app.kind() == "identity" { "settings" } else { app.kind() };
            view! {
                <div class="app-shell">
                    <crate::shell::WorkspaceHeader app=kind session=session.clone() on_signed_out=on_signed_out/>
                    <main class="startup-state" aria-busy="true"><div class="brand-mark"></div><p role="status">"Opening app\u{2026}"</p></main>
                </div>
            }
            .into_any()
        }
    })
    .into_any()
}

/// Fetches the other apps' code in the background once the landing app (Files, which is not
/// lazy) is up.
pub fn preload_soon() {
    if !PRELOADING.replace(true) {
        spawn_local(async {
            crate::sleep(1500).await;
            preload_all();
        });
    }
}
