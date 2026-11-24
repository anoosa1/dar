//! Adding accounts and Mail settings: accounts, reading, and (for administrators) the mail
//! providers people can add accounts with.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use super::workspace::{MailState, Pane, View};
use super::*;
use crate::api;
use crate::icons::Icon;
use crate::settings::{Saver, Segmented, SettingRow, SettingsSheet};
use crate::ui::{Dialog, EditorPane, window};

fn provider_for(providers: &[Provider], address: &str) -> Option<String> {
    let domain = address
        .rsplit_once('@')
        .map(|(_, d)| d.trim().to_lowercase())?;
    providers
        .iter()
        .find(|p| p.domains.iter().any(|d| d.to_lowercase() == domain))
        .map(|p| p.id.clone())
}

#[component]
pub(crate) fn AddAccountPane(state: MailState, account: Option<String>) -> impl IntoView {
    let providers = state
        .overview
        .with_untracked(|o| o.as_ref().map(|o| o.providers.clone()).unwrap_or_default());
    let existing = account.as_deref().and_then(|id| state.account(id));
    let renew = existing.is_some();
    let address = RwSignal::new(
        existing
            .as_ref()
            .map(|a| a.address.clone())
            .unwrap_or_default(),
    );
    let provider = RwSignal::new(
        existing
            .as_ref()
            .map(|a| a.provider_id.clone())
            .or_else(|| (providers.len() == 1).then(|| providers[0].id.clone()))
            .unwrap_or_default(),
    );
    let chosen = RwSignal::new(renew || providers.len() == 1);
    let name = RwSignal::new(String::new());
    let display = RwSignal::new(state.session.with_value(|s| s.user.display_name.clone()));
    let username = RwSignal::new(String::new());
    let secret = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let providers = StoredValue::new(providers);
    let current = Memo::new(move |_| {
        providers.with_value(|list| list.iter().find(|p| p.id == provider.get()).cloned())
    });
    let account_id = StoredValue::new(account.clone());
    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let Some(p) = current.get_untracked() else {
            error.set("Choose a provider.".into());
            return;
        };
        let addr = address.get_untracked().trim().to_owned();
        if !addr.contains('@') {
            error.set("Enter the account's email address.".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        let csrf = state.csrf();
        spawn_local(async move {
            let label = if name.get_untracked().trim().is_empty() {
                addr.split('@')
                    .nth(1)
                    .map(|d| d.split('.').next().unwrap_or(d))
                    .map(|d| {
                        let mut c = d.chars();
                        c.next()
                            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                            .unwrap_or_default()
                    })
                    .unwrap_or_else(|| addr.clone())
            } else {
                name.get_untracked().trim().to_owned()
            };
            if p.auth == "oauth" {
                let body = json!({ "providerId": p.id, "name": label, "address": addr, "displayName": display.get_untracked(), "accountId": account_id.get_value() });
                match api::send::<Value>(&format!("{BASE}/oauth/start"), "POST", Some(body), &csrf)
                    .await
                {
                    Ok(v) => {
                        let _ = window()
                            .location()
                            .set_href(v["url"].as_str().unwrap_or("/mail"));
                    }
                    Err(e) => {
                        let _ = error.try_set(e.message);
                        let _ = busy.try_set(false);
                    }
                }
                return;
            }
            let key = if p.auth == "token" {
                "token"
            } else {
                "password"
            };
            let result = match account_id.get_value() {
                Some(id) => api::send::<Value>(&format!("{BASE}/accounts/{}", encode(&id)), "PATCH", Some(json!({ key: secret.get_untracked() })), &csrf).await,
                None => api::send::<Value>(&format!("{BASE}/accounts"), "POST", Some(json!({
                    "providerId": p.id, "name": label, "address": addr, "displayName": display.get_untracked(), "username": username.get_untracked(), key: secret.get_untracked(),
                })), &csrf).await,
            };
            match result {
                Ok(_) => {
                    let _ = state.pane.try_set(None);
                    state.notify(if renew {
                        "Signed in again"
                    } else {
                        "Account added; its mail is being mirrored"
                    });
                    state.reload();
                    if !renew {
                        state.choose(View::Unified("inbox"));
                    }
                }
                Err(e) => {
                    let _ = error.try_set(e.message);
                    let _ = busy.try_set(false);
                }
            }
        });
    };
    let folder = StoredValue::new(state.overview.with_untracked(|o| {
        o.as_ref()
            .map(|o| o.mail_folder.clone())
            .unwrap_or_else(|| "docs/mail".into())
    }));
    view! {
        <EditorPane title=if renew { "Sign in again" } else { "Add account" } on_close=Callback::new(move |_| state.pane.set(None))>
            <form class="mail-form" on:submit=submit>
                <label>"Email address"
                    <input type="email" required autocomplete="email" autofocus=!renew readonly=renew prop:value=move || address.get()
                        on:input=move |e| { let v = event_target_value(&e); if !chosen.get_untracked() && let Some(id) = providers.with_value(|p| provider_for(p, &v)) { provider.set(id); } address.set(v); }/>
                </label>
                {(!renew).then(|| view! {
                    <fieldset class="mail-providers">
                        <legend>"Provider"</legend>
                        {providers.get_value().into_iter().map(|p| {
                            let id = p.id.clone();
                            let id2 = p.id.clone();
                            let checked = Memo::new(move |_| provider.get() == id);
                            let hint = match (p.protocol.as_str(), p.auth.as_str()) {
                                (_, "oauth") => format!("Sign in with {}", p.name),
                                ("jmap", "token") => "JMAP with an API token".to_owned(),
                                ("jmap", _) => "JMAP".to_owned(),
                                _ => "IMAP and SMTP".to_owned(),
                            };
                            view! {
                                <label class=move || if checked.get() { "mail-provider is-checked" } else { "mail-provider" }>
                                    <input type="radio" name="provider" prop:checked=move || checked.get() on:change=move |_| { provider.set(id2.clone()); chosen.set(true); }/>
                                    <span class="mail-provider-logo" aria-hidden="true">{initials(&p.name).chars().next().unwrap_or('?').to_string()}</span>
                                    <span><b>{p.name.clone()}</b><small>{hint}</small></span>
                                </label>
                            }
                        }).collect_view()}
                    </fieldset>
                })}
                {move || current.get().map(|p| {
                    let oauth = p.auth == "oauth";
                    view! {
                        {(!renew).then(|| view! {
                            <div class="mail-form-row">
                                <label>"Account name"<input placeholder="Work, Personal…" maxlength="100" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/></label>
                                <label>"Your name"<input maxlength="200" autocomplete="name" prop:value=move || display.get() on:input=move |e| display.set(event_target_value(&e))/></label>
                            </div>
                        })}
                        {(!oauth && !renew && p.auth == "password").then(|| view! {
                            <label>"User name"<small>"Leave empty to sign in with the email address."</small><input autocomplete="username" prop:value=move || username.get() on:input=move |e| username.set(event_target_value(&e))/></label>
                        })}
                        {(!oauth).then(|| view! {
                            <label>{if p.auth == "token" { "API token" } else { "Password or app password" }}
                                <input type="password" required autocomplete="new-password" prop:value=move || secret.get() on:input=move |e| secret.set(event_target_value(&e))/>
                            </label>
                        })}
                        <p class="mail-note"><Icon name="lock"/><span>{if oauth {
                            format!("You will sign in on {}'s own page. Dar keeps only the access it grants, encrypted on this server.", p.name)
                        } else {
                            "The password is encrypted on this Dar server and used only to reach this provider's servers.".to_owned()
                        }}{if renew { String::new() } else { format!(" Messages are mirrored as .eml files into {} in Files.", folder.get_value()) }}</span></p>
                    }
                })}
                {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions">
                    <button type="button" class="text-button">"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get() || current.with(Option::is_none)>
                        {move || match (busy.get(), current.get().map(|p| p.auth == "oauth").unwrap_or(false)) {
                            (true, _) => "Signing in\u{2026}".to_owned(),
                            (false, true) => format!("Continue with {}", current.get().map(|p| p.name).unwrap_or_default()),
                            (false, false) => if renew { "Sign in".into() } else { "Sign in and add".into() },
                        }}
                    </button>
                </div>
            </form>
        </EditorPane>
    }
}

const COLORS: [&str; 6] = ["Plum", "Rose", "Violet", "Green", "Amber", "Slate"];

#[component]
fn AccountRow(state: MailState, account: Account, on_remove: Callback<Account>) -> impl IntoView {
    let editing = RwSignal::new(false);
    let name = RwSignal::new(account.name.clone());
    let display = RwSignal::new(account.display_name.clone());
    let signature = RwSignal::new(account.signature.clone());
    let color = RwSignal::new(account.color);
    let error = RwSignal::new(String::new());
    let id = StoredValue::new(account.id.clone());
    let a = StoredValue::new(account.clone());
    let status = match account.status.as_str() {
        _ if !account.provider_available => view! { <span class="mail-state is-bad"><Icon name="alert"/>"Provider turned off"</span> }.into_any(),
        "signIn" => view! { <span class="mail-state is-bad"><Icon name="alert"/>"Sign-in rejected"</span> }.into_any(),
        "error" => view! { <span class="mail-state is-bad" title=account.last_error.clone()><Icon name="alert"/>"Not syncing"</span> }.into_any(),
        "syncing" if account.synced_at == 0.0 => view! { <span class="mail-state">"Mirroring\u{2026}"</span> }.into_any(),
        _ => view! { <span class="mail-state is-ok"><Icon name="check"/>"Synced"</span> }.into_any(),
    };
    let save = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let csrf = state.csrf();
        let body = json!({ "name": name.get_untracked(), "displayName": display.get_untracked(), "signature": signature.get_untracked(), "color": color.get_untracked() });
        spawn_local(async move {
            match api::send::<Value>(
                &format!("{BASE}/accounts/{}", encode(&id.get_value())),
                "PATCH",
                Some(body),
                &csrf,
            )
            .await
            {
                Ok(_) => {
                    let _ = editing.try_set(false);
                    state.notify("Account saved");
                    state.reload();
                }
                Err(e) => {
                    let _ = error.try_set(e.message);
                }
            }
        });
    };
    let kind = if account.protocol == "jmap" {
        "JMAP"
    } else {
        "IMAP and SMTP"
    };
    view! {
        <div class="mail-account-setting">
            <div class="mail-account-line">
                <span class=format!("mail-acct {}", tone(account.color)) aria-hidden="true">{initials(&account.name).chars().next().unwrap_or('?').to_string()}</span>
                <span class="mail-account-text"><b>{account.name.clone()}</b><small>{format!("{} \u{00b7} {} \u{00b7} {}", account.address, account.provider_name, kind)}</small>
                    {(!account.last_error.is_empty()).then(|| view! { <small class="mail-account-error">{account.last_error.clone()}</small> })}</span>
                {status}
            </div>
            <div class="mail-account-actions">
                <button type="button" class="secondary-button" on:click=move |_| state.sync(Some(id.get_value()))><Icon name="refresh"/>"Sync now"</button>
                <button type="button" class="secondary-button" aria-expanded=move || editing.get().to_string() on:click=move |_| editing.update(|v| *v = !*v)><Icon name="pencil"/>"Edit"</button>
                <button type="button" class="secondary-button" on:click=move |_| state.pane.set(Some(Pane::AddAccount(Some(id.get_value()))))><Icon name="key"/>"Sign in again"</button>
                <button type="button" class="text-button danger" on:click=move |_| { on_remove.try_run(a.get_value()); }>"Remove"</button>
            </div>
            <Show when=move || editing.get()>
                <form class="mail-form mail-account-form" on:submit=save>
                    <div class="mail-form-row">
                        <label>"Account name"<input required maxlength="100" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/></label>
                        <label>"Your name"<input maxlength="200" prop:value=move || display.get() on:input=move |e| display.set(event_target_value(&e))/></label>
                    </div>
                    <label>"Color"
                        <select prop:value=move || color.get().to_string() on:change=move |e| color.set(event_target_value(&e).parse().unwrap_or(0))>
                            {COLORS.iter().enumerate().map(|(i, c)| view! { <option value=i.to_string()>{*c}</option> }).collect_view()}
                        </select>
                    </label>
                    <label>"Signature"<textarea rows="3" maxlength="4000" prop:value=move || signature.get() on:input=move |e| signature.set(event_target_value(&e))></textarea></label>
                    {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                    <div class="dialog-actions"><button type="button" class="text-button" on:click=move |_| editing.set(false)>"Cancel"</button><button class="primary-button">"Save"</button></div>
                </form>
            </Show>
        </div>
    }
}

#[component]
pub(crate) fn MailSettings(state: MailState, section: &'static str) -> impl IntoView {
    let administrator = state
        .overview
        .with_untracked(|o| o.as_ref().is_some_and(|o| o.administrator));
    let mut sections = vec!["Accounts", "Reading"];
    if administrator {
        sections.push("Providers");
    }
    let section = RwSignal::new(if sections.contains(&section) {
        section
    } else {
        "Accounts"
    });
    let saver = Saver::new(state.session.get_value());
    let removing = RwSignal::new(None::<Account>);
    let keep = RwSignal::new(true);
    let remove = move |_| {
        let Some(a) = removing.get_untracked() else {
            return;
        };
        removing.set(None);
        let csrf = state.csrf();
        let keep = keep.get_untracked();
        spawn_local(async move {
            match api::send::<Value>(
                &format!(
                    "{BASE}/accounts/{}?keepFiles={}",
                    encode(&a.id),
                    if keep { 1 } else { 0 }
                ),
                "DELETE",
                None,
                &csrf,
            )
            .await
            {
                Ok(_) => {
                    state.notify(if keep {
                        format!("{} removed; its .eml files stay in Files", a.name)
                    } else {
                        format!("{} removed; its files moved to deleted files", a.name)
                    });
                    state.reload();
                }
                Err(e) => state.fail(e.message),
            }
        });
    };
    let value = move |key: &'static str, fallback: &'static str| {
        Signal::derive(move || {
            saver.preferences.with(|p| {
                p.as_ref()
                    .and_then(|p| p.rest.get(key))
                    .and_then(Value::as_str)
                    .unwrap_or(fallback)
                    .to_owned()
            })
        })
    };
    view! {
        <SettingsSheet title="Mail settings" icon="mail" sections=sections section=section on_close=Callback::new(move |_| state.pane.set(None)) notice=saver.notice>
            {move || match section.get() {
                "Accounts" => {
                    let o = state.overview.get().unwrap_or_default();
                    view! {
                        <section class="settings-section">
                            <h3>"Accounts"</h3>
                            {o.accounts.is_empty().then(|| view! { <p class="settings-hint">"No accounts yet."</p> })}
                            {o.accounts.into_iter().map(|a| view! { <AccountRow state=state account=a on_remove=Callback::new(move |a: Account| { keep.set(true); removing.set(Some(a)); })/> }).collect_view()}
                            <button type="button" class="secondary-button" on:click=move |_| state.pane.set(Some(Pane::AddAccount(None)))><Icon name="plus"/>"Add account"</button>
                        </section>
                        <section class="settings-section">
                            <h3>"Files mirror"</h3>
                            <SettingRow label="Mirror folder" hint="Every message is kept as its original .eml file, one folder per account and mailbox. Moving, deleting or adding .eml files there changes the mailbox on the server at the next sync." state=Signal::derive(|| None)>
                                <code class="mail-folder-path">{o.mail_folder.clone()}</code>
                            </SettingRow>
                            <p class="settings-hint">"Deleting a whole mailbox folder in Files does not delete its messages: the folder is made again on the next sync. A message deleted on the server moves to deleted files."</p>
                        </section>
                    }.into_any()
                }
                "Reading" => view! {
                    <section class="settings-section">
                        <h3>"Reading"</h3>
                        <SettingRow label="Remote images" hint="Images from the web can tell senders you opened a message. When loaded, they come through Dar so your address stays private." state=saver.state("mailImages")>
                            <Segmented label="Remote images" value=value("mailImages", "block") options=vec![("block", "Block"), ("load", "Load")]
                                on_change=Callback::new(move |v: String| saver.change("mailImages", json!(v), if v == "load" { "Remote images load".into() } else { "Remote images are blocked".into() }, true))/>
                        </SettingRow>
                        <SettingRow label="Mark as read" hint="When a message counts as read." state=saver.state("mailMarkRead")>
                            <Segmented label="Mark as read" value=value("mailMarkRead", "open") options=vec![("open", "When opened"), ("manual", "Manually")]
                                on_change=Callback::new(move |v: String| saver.change("mailMarkRead", json!(v), "Saved".into(), true))/>
                        </SettingRow>
                        <h3>"Keyboard"</h3>
                        <dl class="mail-keys">
                            <dt><kbd>"c"</kbd></dt><dd>"Compose"</dd><dt><kbd>"j"</kbd>" / "<kbd>"k"</kbd></dt><dd>"Next / previous message"</dd>
                            <dt><kbd>"r"</kbd>" "<kbd>"a"</kbd>" "<kbd>"f"</kbd></dt><dd>"Reply, reply all, forward"</dd><dt><kbd>"e"</kbd></dt><dd>"Archive"</dd>
                            <dt><kbd>"#"</kbd></dt><dd>"Delete"</dd><dt><kbd>"s"</kbd>" "<kbd>"u"</kbd></dt><dd>"Flag, mark unread"</dd><dt><kbd>"Ctrl Enter"</kbd></dt><dd>"Send"</dd>
                        </dl>
                    </section>
                }.into_any(),
                _ => view! { <Providers state=state/> }.into_any(),
            }}
            {move || removing.get().map(|a| view! {
                <Dialog title=format!("Remove {}?", a.name) on_close=Callback::new(move |_| removing.set(None))>
                    <p>"Dar stops syncing this account and forgets its sign-in. Nothing changes on the mail server."</p>
                    <label class="mail-check-row"><input type="checkbox" prop:checked=move || keep.get() on:change=move |e| keep.set(event_target_checked(&e))/>"Keep its .eml files in Files"</label>
                    <div class="dialog-actions">
                        <button type="button" class="text-button" on:click=move |_| removing.set(None)>"Cancel"</button>
                        <button type="button" class="primary-button danger" on:click=remove>"Remove account"</button>
                    </div>
                </Dialog>
            })}
        </SettingsSheet>
    }
}

fn template(kind: &str) -> (String, Value) {
    match kind {
        "google" => (
            "Google".into(),
            json!({
                "protocol": "imap", "auth": "oauth", "domains": ["gmail.com", "googlemail.com"], "saveSent": false,
                "imap": { "host": "imap.gmail.com", "port": 993, "security": "tls" }, "smtp": { "host": "smtp.gmail.com", "port": 465, "security": "tls" },
                "oauth": { "authorizeUrl": "https://accounts.google.com/o/oauth2/v2/auth", "tokenUrl": "https://oauth2.googleapis.com/token", "clientId": "",
                    "scopes": ["https://mail.google.com/"], "params": { "access_type": "offline", "prompt": "consent" } },
            }),
        ),
        "microsoft" => (
            "Microsoft".into(),
            json!({
                "protocol": "imap", "auth": "oauth", "domains": ["outlook.com", "hotmail.com", "live.com"], "saveSent": false,
                "imap": { "host": "outlook.office365.com", "port": 993, "security": "tls" }, "smtp": { "host": "smtp.office365.com", "port": 587, "security": "starttls" },
                "oauth": { "authorizeUrl": "https://login.microsoftonline.com/common/oauth2/v2.0/authorize", "tokenUrl": "https://login.microsoftonline.com/common/oauth2/v2.0/token", "clientId": "",
                    "scopes": ["https://outlook.office.com/IMAP.AccessAsUser.All", "https://outlook.office.com/SMTP.Send", "offline_access"], "params": {} },
            }),
        ),
        "fastmail" => (
            "Fastmail".into(),
            json!({ "protocol": "jmap", "auth": "token", "domains": ["fastmail.com"], "jmap": { "url": "https://api.fastmail.com/jmap/session" } }),
        ),
        "jmap" => (
            String::new(),
            json!({ "protocol": "jmap", "auth": "password", "domains": [], "jmap": { "url": "https://" } }),
        ),
        _ => (
            String::new(),
            json!({ "protocol": "imap", "auth": "password", "domains": [], "imap": { "host": "", "port": 993, "security": "tls" }, "smtp": { "host": "", "port": 465, "security": "tls" } }),
        ),
    }
}

#[component]
fn Providers(state: MailState) -> impl IntoView {
    let list = RwSignal::new(None::<Vec<Provider>>);
    let error = RwSignal::new(String::new());
    let editing = RwSignal::new(None::<Option<Provider>>);
    let deleting = RwSignal::new(None::<Provider>);
    let load = move || {
        spawn_local(async move {
            match api::get::<Value>(&format!("{BASE}/admin/providers")).await {
                Ok(v) => {
                    let _ = list.try_set(serde_json::from_value(v["items"].clone()).ok());
                }
                Err(e) => {
                    let _ = error.try_set(e.message);
                }
            }
        });
    };
    load();
    let toggle = move |p: Provider| {
        let csrf = state.csrf();
        spawn_local(async move {
            match api::send::<Value>(
                &format!("{BASE}/admin/providers/{}", encode(&p.id)),
                "PUT",
                Some(json!({ "name": p.name, "document": p.document, "enabled": !p.enabled })),
                &csrf,
            )
            .await
            {
                Ok(_) => {
                    state.notify(if p.enabled {
                        format!("{} turned off", p.name)
                    } else {
                        format!("{} turned on", p.name)
                    });
                    load();
                    state.reload();
                }
                Err(e) => state.fail(e.message),
            }
        });
    };
    let delete = move |_| {
        let Some(p) = deleting.get_untracked() else {
            return;
        };
        deleting.set(None);
        let csrf = state.csrf();
        spawn_local(async move {
            match api::send::<Value>(
                &format!("{BASE}/admin/providers/{}", encode(&p.id)),
                "DELETE",
                None,
                &csrf,
            )
            .await
            {
                Ok(_) => {
                    state.notify(format!("{} removed", p.name));
                    load();
                    state.reload();
                }
                Err(e) => state.fail(e.message),
            }
        });
    };
    view! {
        {move || match editing.get() {
            Some(p) => view! { <ProviderForm state=state provider=p on_done=Callback::new(move |_| { editing.set(None); load(); state.reload(); }) on_cancel=Callback::new(move |_| editing.set(None))/> }.into_any(),
            None => view! {
                <section class="settings-section">
                    <h3>"Mail providers"</h3>
                    <p class="settings-hint">"People can add mail accounts only with these providers. Providers from the host configuration (services.dar.mail.providers) are changed in Nix."</p>
                    {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                    {move || list.get().map(|items| {
                        if items.is_empty() {
                            return view! { <p class="settings-hint">"No providers yet."</p> }.into_any();
                        }
                        items.into_iter().map(|p| {
                            let nix = p.source == "nix";
                            let (p1, p2, p3) = (p.clone(), p.clone(), p.clone());
                            let kind = format!("{} \u{00b7} {}", if p.protocol == "jmap" { "JMAP" } else { "IMAP and SMTP" }, match p.auth.as_str() { "oauth" => "OAuth", "token" => "API token", _ => "password" });
                            view! {
                                <div class="mail-provider-setting">
                                    <span class="mail-provider-logo" aria-hidden="true">{initials(&p.name).chars().next().unwrap_or('?').to_string()}</span>
                                    <span class="mail-account-text"><b>{p.name.clone()}{(!p.enabled).then(|| view! { <span class="mail-label">"Off"</span> })}</b>
                                        <small>{format!("{} \u{00b7} {} \u{00b7} {} account{}", p.id, kind, p.accounts, if p.accounts == 1 { "" } else { "s" })}</small>
                                        {nix.then(|| view! { <small>"From the host configuration"</small> })}</span>
                                    {(!nix).then(|| view! {
                                        <div class="mail-account-actions">
                                            <button type="button" class="secondary-button" on:click=move |_| editing.set(Some(Some(p1.clone())))><Icon name="pencil"/>"Edit"</button>
                                            <button type="button" class="secondary-button" on:click=move |_| toggle(p2.clone())>{if p.enabled { "Turn off" } else { "Turn on" }}</button>
                                            <button type="button" class="text-button danger" on:click=move |_| deleting.set(Some(p3.clone()))>"Remove"</button>
                                        </div>
                                    })}
                                </div>
                            }
                        }).collect_view().into_any()
                    })}
                    <button type="button" class="secondary-button" on:click=move |_| editing.set(Some(None))><Icon name="plus"/>"Add provider"</button>
                </section>
            }.into_any(),
        }}
        {move || deleting.get().map(|p| view! {
            <Dialog title=format!("Remove {}?", p.name) on_close=Callback::new(move |_| deleting.set(None))>
                <p>"People can no longer add accounts with this provider. Providers still in use cannot be removed; turn them off instead."</p>
                <div class="dialog-actions"><button type="button" class="text-button" on:click=move |_| deleting.set(None)>"Cancel"</button><button type="button" class="primary-button danger" on:click=delete>"Remove provider"</button></div>
            </Dialog>
        })}
    }
}

#[component]
fn ProviderForm(
    state: MailState,
    provider: Option<Provider>,
    on_done: Callback<()>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    let is_new = provider.is_none();
    let doc = provider
        .as_ref()
        .map(|p| p.document.clone())
        .unwrap_or_else(|| template("imap").1);
    let id = RwSignal::new(provider.as_ref().map(|p| p.id.clone()).unwrap_or_default());
    let name = RwSignal::new(
        provider
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_default(),
    );
    let has_secret = provider.as_ref().is_some_and(|p| p.has_secret);
    let s = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    let n = |v: &Value, d: u64| v.as_u64().unwrap_or(d).to_string();
    let protocol = RwSignal::new(s(&doc["protocol"]));
    let auth = RwSignal::new(s(&doc["auth"]));
    let domains = RwSignal::new(
        doc["domains"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|d| d.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
    );
    let imap_host = RwSignal::new(s(&doc["imap"]["host"]));
    let imap_port = RwSignal::new(n(&doc["imap"]["port"], 993));
    let imap_security = RwSignal::new(doc["imap"]["security"].as_str().unwrap_or("tls").to_owned());
    let smtp_host = RwSignal::new(s(&doc["smtp"]["host"]));
    let smtp_port = RwSignal::new(n(&doc["smtp"]["port"], 465));
    let smtp_security = RwSignal::new(doc["smtp"]["security"].as_str().unwrap_or("tls").to_owned());
    let jmap_url = RwSignal::new(s(&doc["jmap"]["url"]));
    let authorize = RwSignal::new(s(&doc["oauth"]["authorizeUrl"]));
    let token = RwSignal::new(s(&doc["oauth"]["tokenUrl"]));
    let client = RwSignal::new(s(&doc["oauth"]["clientId"]));
    let secret = RwSignal::new(String::new());
    let scopes = RwSignal::new(
        doc["oauth"]["scopes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|d| d.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default(),
    );
    let params = RwSignal::new(
        doc["oauth"]["params"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
    );
    let save_sent = RwSignal::new(doc["saveSent"] != false);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let apply = move |kind: String| {
        let (label, d) = template(&kind);
        if !label.is_empty() {
            name.set(label.clone());
            if id.get_untracked().is_empty() {
                id.set(label.to_lowercase());
            }
        }
        protocol.set(s(&d["protocol"]));
        auth.set(s(&d["auth"]));
        domains.set(
            d["domains"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
        );
        imap_host.set(s(&d["imap"]["host"]));
        imap_port.set(n(&d["imap"]["port"], 993));
        imap_security.set(d["imap"]["security"].as_str().unwrap_or("tls").into());
        smtp_host.set(s(&d["smtp"]["host"]));
        smtp_port.set(n(&d["smtp"]["port"], 465));
        smtp_security.set(d["smtp"]["security"].as_str().unwrap_or("tls").into());
        jmap_url.set(s(&d["jmap"]["url"]));
        authorize.set(s(&d["oauth"]["authorizeUrl"]));
        token.set(s(&d["oauth"]["tokenUrl"]));
        scopes.set(
            d["oauth"]["scopes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default(),
        );
        params.set(
            d["oauth"]["params"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
        );
        save_sent.set(d["saveSent"] != false);
    };
    let redirect = format!(
        "{}/api/v1/mail/oauth/callback",
        window().location().origin().unwrap_or_default()
    );
    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let port = |v: RwSignal<String>| v.get_untracked().trim().parse::<u16>().unwrap_or(0);
        let mut document = json!({
            "protocol": protocol.get_untracked(), "auth": auth.get_untracked(), "saveSent": save_sent.get_untracked(),
            "domains": domains.get_untracked().split([',', ' ']).map(str::trim).filter(|d| !d.is_empty()).map(str::to_lowercase).collect::<Vec<_>>(),
        });
        if protocol.get_untracked() == "jmap" {
            document["jmap"] = json!({ "url": jmap_url.get_untracked().trim() });
        } else {
            document["imap"] = json!({ "host": imap_host.get_untracked().trim(), "port": port(imap_port), "security": imap_security.get_untracked() });
            document["smtp"] = json!({ "host": smtp_host.get_untracked().trim(), "port": port(smtp_port), "security": smtp_security.get_untracked() });
        }
        if auth.get_untracked() == "oauth" {
            let extra: serde_json::Map<String, Value> = params
                .get_untracked()
                .lines()
                .filter_map(|l| l.split_once('='))
                .map(|(k, v)| (k.trim().to_owned(), json!(v.trim())))
                .filter(|(k, _)| !k.is_empty())
                .collect();
            document["oauth"] = json!({
                "authorizeUrl": authorize.get_untracked().trim(), "tokenUrl": token.get_untracked().trim(), "clientId": client.get_untracked().trim(),
                "scopes": scopes.get_untracked().split_whitespace().collect::<Vec<_>>(), "params": extra,
            });
        }
        let mut body =
            json!({ "name": name.get_untracked().trim(), "document": document, "create": is_new });
        if !secret.get_untracked().is_empty() {
            body["secret"] = json!(secret.get_untracked());
        }
        busy.set(true);
        error.set(String::new());
        let csrf = state.csrf();
        let target = id.get_untracked().trim().to_lowercase();
        spawn_local(async move {
            match api::send::<Value>(
                &format!("{BASE}/admin/providers/{}", encode(&target)),
                "PUT",
                Some(body),
                &csrf,
            )
            .await
            {
                Ok(_) => {
                    state.notify("Provider saved");
                    on_done.try_run(());
                }
                Err(e) => {
                    let _ = error.try_set(e.message);
                    let _ = busy.try_set(false);
                }
            }
        });
    };
    let field = |label: &'static str,
                 value: RwSignal<String>,
                 kind: &'static str,
                 placeholder: &'static str| {
        view! {
            <label>{label}<input type=kind placeholder=placeholder prop:value=move || value.get() on:input=move |e| value.set(event_target_value(&e))/></label>
        }
    };
    let security = |label: &'static str, value: RwSignal<String>| {
        view! {
            <label>{label}<select prop:value=move || value.get() on:change=move |e| value.set(event_target_value(&e))><option value="tls">"TLS"</option><option value="starttls">"STARTTLS"</option></select></label>
        }
    };
    view! {
        <form class="mail-form settings-section" on:submit=submit>
            <h3>{if is_new { "Add provider" } else { "Edit provider" }}</h3>
            {is_new.then(|| view! {
                <label>"Start from"
                    <select on:change=move |e| apply(event_target_value(&e))>
                        <option value="imap">"Other IMAP and SMTP"</option><option value="jmap">"Other JMAP"</option>
                        <option value="google">"Google (OAuth)"</option><option value="microsoft">"Microsoft 365 and Outlook (OAuth)"</option><option value="fastmail">"Fastmail (JMAP)"</option>
                    </select>
                </label>
            })}
            <div class="mail-form-row">
                <label>"ID"<small>"Lowercase letters, digits and dashes; cannot change."</small><input required pattern="[a-z0-9][a-z0-9-]*" maxlength="64" readonly=!is_new prop:value=move || id.get() on:input=move |e| id.set(event_target_value(&e))/></label>
                <label>"Name"<small>"Shown when people add an account."</small><input required maxlength="100" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/></label>
            </div>
            {field("Email domains (suggest this provider for these addresses)", domains, "text", "example.com, example.org")}
            <div class="mail-form-row">
                <label>"Protocol"<select prop:value=move || protocol.get() on:change=move |e| protocol.set(event_target_value(&e))><option value="imap">"IMAP and SMTP"</option><option value="jmap">"JMAP"</option></select></label>
                <label>"Sign-in"<select prop:value=move || auth.get() on:change=move |e| auth.set(event_target_value(&e))>
                    <option value="password">"Password or app password"</option><option value="token">"API token (JMAP)"</option><option value="oauth">"OAuth2"</option></select></label>
            </div>
            {move || if protocol.get() == "jmap" {
                view! { {field("JMAP session URL", jmap_url, "url", "https://api.example.com/jmap/session")} }.into_any()
            } else {
                view! {
                    <div class="mail-form-row is-server">{field("IMAP server", imap_host, "text", "imap.example.com")}{field("Port", imap_port, "number", "993")}{security("Security", imap_security)}</div>
                    <div class="mail-form-row is-server">{field("SMTP server", smtp_host, "text", "smtp.example.com")}{field("Port", smtp_port, "number", "465")}{security("Security", smtp_security)}</div>
                    <label class="mail-check-row"><input type="checkbox" prop:checked=move || save_sent.get() on:change=move |e| save_sent.set(event_target_checked(&e))/>"Save sent messages to the Sent mailbox (turn off when the server does it, like Gmail)"</label>
                }.into_any()
            }}
            {move || (auth.get() == "oauth").then(|| view! {
                <p class="mail-note"><Icon name="info"/><span>"Register this redirect address with the provider: "<code>{redirect.clone()}</code></span></p>
                {field("Authorization URL", authorize, "url", "https://…/authorize")}
                {field("Token URL", token, "url", "https://…/token")}
                {field("Client ID", client, "text", "")}
                <label>"Client secret"<small>{if has_secret { "Saved. Leave empty to keep it." } else { "Leave empty for public clients." }}</small>
                    <input type="password" autocomplete="off" prop:value=move || secret.get() on:input=move |e| secret.set(event_target_value(&e))/></label>
                {field("Scopes (space separated)", scopes, "text", "")}
                <label>"Extra authorization parameters"<small>"One key=value per line, like access_type=offline."</small>
                    <textarea rows="2" prop:value=move || params.get() on:input=move |e| params.set(event_target_value(&e))></textarea></label>
            })}
            {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            <div class="dialog-actions">
                <button type="button" class="text-button" on:click=move |_| { on_cancel.try_run(()); }>"Cancel"</button>
                <button class="primary-button" disabled=move || busy.get()>{move || if busy.get() { "Saving\u{2026}" } else { "Save provider" }}</button>
            </div>
        </form>
    }
}
