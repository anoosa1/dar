//! The acceptance suites: one module per suite (ported from `test/`), each with a `SUITE`, and
//! one `*_support` module per area for its shared helpers and fixture programs. `SUITES` lists
//! the suites in the order they start, the longest first, so a parallel run ends sooner.

use dar_acceptance::harness::Suite;
use std::process::ExitCode;

mod agent_git_support;
mod comms_support;
mod core_support;
mod photos_books_apps_support;
mod pim_workspace_support;

mod agent;
mod agent_browser;
mod app_launcher_browser;
mod appearance_scopes;
mod appearance_scopes_browser;
mod apps;
mod auth;
mod auth_security;
mod backup;
mod blob_migration;
mod books;
mod books_browser;
mod browser;
mod calendar_subscriptions;
mod chat;
mod chat_browser;
mod chat_discord;
mod claude;
mod codex;
mod contacts_layout_browser;
mod dav;
mod davx5;
mod files;
mod gc;
mod git;
mod git_app;
mod git_browser;
mod git_interop;
mod git_tracker;
mod git_tracker_browser;
mod global_search;
mod history;
mod history_browser;
mod http_security;
mod identity;
mod identity_browser;
mod identity_lifecycle;
mod identity_proxy;
mod identity_upgrade;
mod mail;
mod mail_browser;
mod news;
mod news_browser;
mod oidc_config;
mod pagination;
mod photos;
mod photos_browser;
mod photos_immich;
mod photos_library;
mod photos_sharing;
mod pim;
mod pim_browser;
mod pim_dav_client;
mod pim_upgrade;
mod prayer_times_browser;
mod production_proxy;
mod refinement_browser;
mod restart;
mod shared_ui_browser;
mod smoke;
mod themed_controls_browser;
mod twilight_browser;
mod ui_settings;
mod upload_timeout;
mod notes_cards;
mod notes_extras;
mod notes_graph;
mod notes_links;
mod workspace;
mod workspace_browser;
mod workspace_upgrade;

pub const SUITES: &[Suite] = &[
    // Long-running first (times from the sequential Node runs).
    twilight_browser::SUITE, upload_timeout::SUITE, codex::SUITE, mail_browser::SUITE, photos_immich::SUITE, restart::SUITE,
    chat::SUITE, chat_discord::SUITE, shared_ui_browser::SUITE, chat_browser::SUITE, production_proxy::SUITE, pim_browser::SUITE,
    agent::SUITE, claude::SUITE, mail::SUITE, photos::SUITE, photos_library::SUITE, photos_sharing::SUITE, photos_browser::SUITE,
    identity::SUITE, identity_lifecycle::SUITE, identity_browser::SUITE, oidc_config::SUITE, workspace::SUITE, workspace_browser::SUITE, notes_links::SUITE, notes_graph::SUITE, notes_cards::SUITE, notes_extras::SUITE,
    agent_browser::SUITE, news::SUITE, news_browser::SUITE, git_interop::SUITE, git_tracker::SUITE, git::SUITE, git_app::SUITE,
    git_browser::SUITE, git_tracker_browser::SUITE, history::SUITE, history_browser::SUITE, books::SUITE, books_browser::SUITE,
    pim::SUITE, pim_upgrade::SUITE, pim_dav_client::SUITE, calendar_subscriptions::SUITE, davx5::SUITE, dav::SUITE,
    contacts_layout_browser::SUITE, workspace_upgrade::SUITE, ui_settings::SUITE, refinement_browser::SUITE, backup::SUITE,
    browser::SUITE, files::SUITE, auth::SUITE, auth_security::SUITE, http_security::SUITE, gc::SUITE, blob_migration::SUITE,
    pagination::SUITE, identity_upgrade::SUITE, identity_proxy::SUITE, apps::SUITE, prayer_times_browser::SUITE,
    appearance_scopes::SUITE, appearance_scopes_browser::SUITE, app_launcher_browser::SUITE, themed_controls_browser::SUITE,
    global_search::SUITE, smoke::SUITE,
];

/// Fixture programs suites start as separate processes (`dar-acceptance fixture <name> …`;
/// see `dar_acceptance::instance::fixture_command`).
pub fn fixture(args: &[String]) -> ExitCode {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("chat-bridge") => comms_support::chat_bridge_fixture(rest),
        Some("codex") => agent_git_support::codex_fixture(rest),
        Some("claude") => agent_git_support::claude_fixture(rest),
        _ => {
            eprintln!("unknown fixture {args:?}");
            ExitCode::FAILURE
        }
    }
}
