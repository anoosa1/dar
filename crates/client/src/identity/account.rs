//! Account security (`account.tsx`) and app-password scope formatting (`shared/app-scopes.ts`).

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen::JsCast;

use crate::api::{self, BrowserSession};
use crate::dates::{local_date, local_date_time};

const PASSWORD_APPS: [&str; 5] = ["files", "contacts", "calendar", "news", "photos"];

fn app_label(app: &str) -> &'static str {
    match app {
        "files" => "Files",
        "contacts" => "Contacts",
        "calendar" => "Calendar",
        "news" => "News",
        "photos" => "Photos",
        _ => "",
    }
}

/// Legacy write scopes included read access; v2 space-separated grants are independent.
pub fn password_grants(scope: &str) -> Vec<String> {
    if scope == "read" {
        return vec!["files:read".to_owned()];
    }
    if scope == "write" {
        return vec!["files:read".to_owned(), "files:write".to_owned()];
    }
    if let Some((app, permission)) = scope.split_once(':')
        && matches!(app, "contacts" | "calendar" | "pim")
        && matches!(permission, "read" | "write")
    {
        let apps: &[&str] = if app == "pim" {
            &["contacts", "calendar"]
        } else {
            &[app]
        };
        let mut out = Vec::new();
        for a in apps {
            out.push(format!("{a}:read"));
            if permission == "write" {
                out.push(format!("{a}:write"));
            }
        }
        return out;
    }
    if let Some(rest) = scope.strip_prefix("v2 ") {
        return rest.split(' ').map(str::to_owned).collect();
    }
    Vec::new()
}

pub fn password_allows(scope: &str, app: &str, permission: &str) -> bool {
    let target = format!("{app}:{permission}");
    password_grants(scope).iter().any(|g| g == &target)
}

pub fn password_scope_label(scope: &str) -> String {
    let parts: Vec<String> = PASSWORD_APPS
        .iter()
        .filter_map(|&app| {
            let read = password_allows(scope, app, "read");
            let write = password_allows(scope, app, "write");
            if read || write {
                let perm = if read && write {
                    "Read and write"
                } else if read {
                    "Read"
                } else {
                    "Write"
                };
                Some(format!("{}: {perm}", app_label(app)))
            } else {
                None
            }
        })
        .collect();
    parts.join(" \u{00b7} ")
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthSession {
    id: String,
    created_at: f64,
    current: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppPassword {
    id: String,
    label: String,
    scope: String,
    expires_at: f64,
}

#[derive(Deserialize)]
struct SessionList {
    items: Vec<AuthSession>,
}

#[derive(Deserialize)]
struct PasswordList {
    items: Vec<AppPassword>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewSecret {
    secret: String,
    dav_url: String,
    #[serde(default)]
    files_dav_url: Option<String>,
    #[serde(default)]
    news_url: Option<String>,
    #[serde(default)]
    immich_url: Option<String>,
}

fn select_input(event: web_sys::FocusEvent) {
    if let Some(input) = event
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        input.select();
    }
}

/// Account security section (`AccountSecurity`): password, browser sessions and app passwords.
#[component]
pub fn AccountSecurity(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let sessions: RwSignal<Vec<AuthSession>> = RwSignal::new(Vec::new());
    let passwords: RwSignal<Vec<AppPassword>> = RwSignal::new(Vec::new());
    let error = RwSignal::new(String::new());
    let secret: RwSignal<Option<NewSecret>> = RwSignal::new(None);
    let busy = RwSignal::new(false);
    let csrf = StoredValue::new(session.csrf_token.clone());
    let username = StoredValue::new(session.user.username.clone());

    let refresh = move || async move {
        let (a, b) = futures::future::try_join(
            api::get::<SessionList>("/api/v1/auth/sessions"),
            api::get::<PasswordList>("/api/v1/auth/app-passwords"),
        )
        .await?;
        let _ = sessions.try_set(a.items);
        let _ = passwords.try_set(b.items);
        Ok::<(), api::RequestError>(())
    };

    spawn_local(async move {
        if let Err(e) = refresh().await {
            let _ = error.try_set(e.message);
        }
    });

    let change_password = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let Some(form) = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok())
        else {
            return;
        };
        let Ok(data) = web_sys::FormData::new_with_form(&form) else {
            return;
        };
        let current_password = data.get("currentPassword").as_string().unwrap_or_default();
        let password = data.get("password").as_string().unwrap_or_default();
        error.set(String::new());
        busy.set(true);
        spawn_local(async move {
            match api::send::<serde_json::Value>(
                "/api/v1/auth/password",
                "POST",
                Some(json!({ "currentPassword": current_password, "password": password })),
                &csrf.get_value(),
            )
            .await
            {
                Ok(_) => {
                    on_signed_out.try_run(());
                }
                Err(e) => {
                    let _ = error.try_set(e.message);
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let create_password = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let Some(form) = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok())
        else {
            return;
        };
        let Ok(data) = web_sys::FormData::new_with_form(&form) else {
            return;
        };
        let grants: Vec<String> = data
            .get_all("grant")
            .iter()
            .filter_map(|v| v.as_string())
            .collect();
        if grants.is_empty() {
            error.set("Choose at least one app permission.".into());
            return;
        }
        let label = data.get("label").as_string().unwrap_or_default();
        let scope = format!("v2 {}", grants.join(" "));
        error.set(String::new());
        busy.set(true);
        spawn_local(async move {
            let result = async {
                let created = api::send::<NewSecret>(
                    "/api/v1/auth/app-passwords",
                    "POST",
                    Some(json!({ "label": label, "scope": scope })),
                    &csrf.get_value(),
                )
                .await?;
                let _ = secret.try_set(Some(created));
                refresh().await
            }
            .await;
            if let Err(e) = result {
                let _ = error.try_set(e.message);
            }
            let _ = busy.try_set(false);
        });
    };

    view! {
        <div class="account-security">
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            <section class="settings-security-section">
                <h2>"Password"</h2>
                <form on:submit=change_password>
                    <label>
                        "Current password"
                        <input name="currentPassword" type="password" autocomplete="current-password" required/>
                    </label>
                    <label>
                        "New password"
                        <input name="password" type="password" autocomplete="new-password" minlength="15" required/>
                    </label>
                    <p class="muted small">"Use at least 15 characters. Changing it revokes browser sessions and app passwords."</p>
                    <button class="secondary-button" disabled=move || busy.get()>"Change password"</button>
                </form>
            </section>
            <section class="settings-security-section">
                <h2>"Signed-in sessions"</h2>
                {move || {
                    sessions.get().into_iter().map(|item| {
                        let id = item.id.clone();
                        view! {
                            <div class="grant">
                                <span>
                                    {if item.current { "This browser session" } else { "Another browser session" }}
                                    <small>"Signed in "{local_date_time(item.created_at, &[])}</small>
                                </span>
                                {(!item.current).then(|| {
                                    let id = id.clone();
                                    view! {
                                        <button
                                            class="text-button danger"
                                            disabled=move || busy.get()
                                            on:click=move |_| {
                                                let id = id.clone();
                                                error.set(String::new());
                                                busy.set(true);
                                                spawn_local(async move {
                                                    let result = async {
                                                        api::send::<serde_json::Value>(
                                                            &format!("/api/v1/auth/sessions/{id}"),
                                                            "DELETE",
                                                            None,
                                                            &csrf.get_value(),
                                                        )
                                                        .await?;
                                                        refresh().await
                                                    }
                                                    .await;
                                                    if let Err(e) = result {
                                                        let _ = error.try_set(e.message);
                                                    }
                                                    let _ = busy.try_set(false);
                                                });
                                            }
                                        >"Sign out"</button>
                                    }
                                })}
                            </div>
                        }
                    }).collect_view()
                }}
            </section>
            <section class="settings-security-section">
                <h2>"App passwords"</h2>
                <p class="muted small">"Connect a WebDAV or Nextcloud News client using a separate, revocable password. Your account password cannot be used for WebDAV."</p>
                <form on:submit=create_password>
                    <label>
                        "Device or application name"
                        <input name="label" required maxlength="100" placeholder="For example, my laptop"/>
                    </label>
                    <fieldset class="password-permissions">
                        <legend>"App access"</legend>
                        <table>
                            <thead>
                                <tr>
                                    <th scope="col">"App"</th>
                                    <th scope="col">"Read"</th>
                                    <th scope="col">"Write"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {PASSWORD_APPS.into_iter().map(|app| {
                                    let label = app_label(app);
                                    view! {
                                        <tr>
                                            <th scope="row">{label}</th>
                                            {["read", "write"].into_iter().map(|permission| {
                                                view! {
                                                    <td>
                                                        <input
                                                            type="checkbox"
                                                            name="grant"
                                                            value=format!("{app}:{permission}")
                                                            aria-label=format!("{label} {permission}")
                                                            disabled=move || busy.get()
                                                        />
                                                    </td>
                                                }
                                            }).collect_view()}
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                        <p class="field-hint">"Choose at least one permission across any number of apps. Read allows viewing and syncing; Write allows changes. Sync clients usually need both."</p>
                    </fieldset>
                    <button class="secondary-button" disabled=move || busy.get()>"Create app password"</button>
                </form>
                {move || secret.get().map(|s| {
                    let files_dav = s.files_dav_url.filter(|u| !u.is_empty() && u != &s.dav_url);
                    let news_url = s.news_url.filter(|u| !u.is_empty());
                    let immich_url = s.immich_url.filter(|u| !u.is_empty());
                    view! {
                        <section class="new-credential">
                            <label>"App password"<input readonly=true value=s.secret on:focus=select_input/></label>
                            <label>"WebDAV address"<input readonly=true value=s.dav_url on:focus=select_input/></label>
                            {files_dav.map(|u| view! { <label>"Files WebDAV address"<input readonly=true value=u on:focus=select_input/></label> })}
                            {news_url.map(|u| view! { <label>"News server address (Nextcloud News apps)"<input readonly=true value=u on:focus=select_input/></label> })}
                            {immich_url.map(|u| view! { <label>"Immich app server URL (use Sign in with Dar, or your email and this password)"<input readonly=true value=u on:focus=select_input/></label> })}
                            <p class="muted small">"Username: "{username.get_value()}". Save the password now; it won\u{2019}t be shown again."</p>
                            <button class="text-button" on:click=move |_| secret.set(None)>"I saved it"</button>
                        </section>
                    }
                })}
                {move || {
                    passwords.get().into_iter().map(|item| {
                        let id = item.id.clone();
                        view! {
                            <div class="grant">
                                <span>
                                    <strong>{item.label}</strong>
                                    <small>{password_scope_label(&item.scope)}" \u{00b7} Expires "{local_date(item.expires_at, &[])}</small>
                                </span>
                                <button
                                    class="text-button danger"
                                    disabled=move || busy.get()
                                    on:click=move |_| {
                                        let id = id.clone();
                                        error.set(String::new());
                                        busy.set(true);
                                        spawn_local(async move {
                                            let result = async {
                                                api::send::<serde_json::Value>(
                                                    &format!("/api/v1/auth/app-passwords/{id}"),
                                                    "DELETE",
                                                    None,
                                                    &csrf.get_value(),
                                                )
                                                .await?;
                                                let _ = secret.try_set(None);
                                                refresh().await
                                            }
                                            .await;
                                            if let Err(e) = result {
                                                let _ = error.try_set(e.message);
                                            }
                                            let _ = busy.try_set(false);
                                        });
                                    }
                                >"Revoke"</button>
                            </div>
                        }
                    }).collect_view()
                }}
            </section>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_labels_match_shared_app_scopes() {
        assert_eq!(
            password_scope_label("v2 files:read contacts:read contacts:write calendar:write"),
            "Files: Read \u{00b7} Contacts: Read and write \u{00b7} Calendar: Write"
        );
        assert_eq!(password_scope_label("read"), "Files: Read");
        assert_eq!(password_scope_label("write"), "Files: Read and write");
        assert_eq!(
            password_scope_label("pim:write"),
            "Contacts: Read and write \u{00b7} Calendar: Read and write"
        );
    }
}
