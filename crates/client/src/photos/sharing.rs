//! Album sharing (`photos/sharing.tsx`): sharing an album with people on this Dar,
//! managing shared links, the quick link dialog for selected photos, and the settings links list.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use wasm_bindgen::JsCast;

use super::{
    BASE, PhotoAlbum, PhotoAlbumMember, PhotoAlbumSharing, PhotoLink, copy_to_clipboard,
    error_message, valid_link_password,
};
use crate::api::{self, BrowserSession, Req};
use crate::dates::local_date;
use crate::icons::Icon;
use crate::settings::SettingSwitch;
use crate::ui::{Dialog, EditorPane, Select};

/// Expiry choices for shared links; 0 never expires.
const EXPIRIES: [(&str, i64); 6] = [
    ("Never", 0),
    ("1 day", 1),
    ("7 days", 7),
    ("30 days", 30),
    ("90 days", 90),
    ("1 year", 365),
];

pub fn expiry_label(at: f64) -> String {
    if at == 0.0 {
        "Never expires".to_owned()
    } else if at < js_sys::Date::now() {
        "Expired".to_owned()
    } else {
        format!("Expires {}", local_date(at, &[("dateStyle", "medium")]))
    }
}

#[derive(Deserialize)]
struct LinksResponse {
    #[serde(default)]
    links: Vec<PhotoLink>,
}

/// Album sharing in the right editing pane: people on this Dar, then shared links.
#[component]
pub fn SharePane(
    album: PhotoAlbum,
    session: BrowserSession,
    on_close: Callback<()>,
    on_changed: Callback<()>,
) -> impl IntoView {
    let sharing: RwSignal<Option<PhotoAlbumSharing>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let username = RwSignal::new(String::new());
    let role = RwSignal::new("viewer".to_owned());
    let busy = RwSignal::new(false);
    let album_id = StoredValue::new(album.id.clone());
    let session = StoredValue::new(session);

    let load = move || {
        let id = album_id.get_value();
        spawn_local(async move {
            match api::get::<PhotoAlbumSharing>(&format!("{BASE}/albums/{id}/sharing")).await {
                Ok(value) => {
                    sharing.try_set(Some(value));
                    error.try_set(String::new());
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
        });
    };
    load();

    let share = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let trimmed = username.get_untracked().trim().to_owned();
        if trimmed.is_empty() {
            error.set("Enter a username.".to_owned());
            return;
        }
        busy.set(true);
        error.set(String::new());
        let chosen_role = role.get_untracked();
        let id = album_id.get_value();
        let csrf = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            let body = json!({ "username": trimmed, "role": chosen_role });
            match api::send::<Value>(
                &format!("{BASE}/albums/{id}/members"),
                "POST",
                Some(body),
                &csrf,
            )
            .await
            {
                Ok(_) => {
                    username.try_set(String::new());
                    load();
                    {
                        on_changed.try_run(());
                    };
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
            busy.try_set(false);
        });
    };

    let change_member = move |member: PhotoAlbumMember, next: String| {
        let id = album_id.get_value();
        let csrf = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            let body = json!({ "username": member.username, "role": next });
            match api::send::<Value>(
                &format!("{BASE}/albums/{id}/members"),
                "POST",
                Some(body),
                &csrf,
            )
            .await
            {
                Ok(_) => load(),
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
        });
    };

    let remove_member = move |member: PhotoAlbumMember| {
        let id = album_id.get_value();
        let csrf = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            match api::request::<Value>(
                &format!("{BASE}/albums/{id}/members/{}", member.user_id),
                Req {
                    method: "DELETE",
                    csrf: Some(&csrf),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) => {
                    load();
                    {
                        on_changed.try_run(());
                    };
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
        });
    };

    let create_link = move |_| {
        busy.set(true);
        error.set(String::new());
        let id = album_id.get_value();
        let csrf = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            let body = json!({
                "albumId": id,
                "expiresAt": 0,
                "allowDownload": true,
                "allowUpload": false,
                "showMetadata": true,
            });
            match api::send::<Value>(&format!("{BASE}/links"), "POST", Some(body), &csrf).await {
                Ok(_) => {
                    load();
                    {
                        on_changed.try_run(());
                    };
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
            busy.try_set(false);
        });
    };

    let role_options = vec![
        ("viewer".to_owned(), "Can view".to_owned()),
        ("editor".to_owned(), "Can add".to_owned()),
    ];
    let role_options = StoredValue::new(role_options);

    let title = format!("Share {}", album.name);
    view! {
        <EditorPane title=title on_close=on_close>
            <div class="photos-share">
                {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <section aria-label="People on this Dar">
                    <h3>"People on this Dar"</h3>
                    <form class="photos-share-add" on:submit=share>
                        <label>"Username"
                            <input
                                prop:value=move || username.get()
                                maxlength="64"
                                autocomplete="off"
                                placeholder="Their Dar username"
                                on:input=move |e| username.set(event_target_value(&e))
                            />
                        </label>
                        <label>"Access"
                            <Select options=role_options.get_value() value=role/>
                        </label>
                        <button class="primary-button" disabled=move || busy.get()>"Share"</button>
                    </form>
                    {move || match sharing.get() {
                        None => view! { <p role="status" class="photos-hint">"Loading\u{2026}"</p> }.into_any(),
                        Some(data) if data.members.is_empty() => view! {
                            <p class="photos-hint">"Not shared with anyone yet. People who can add put their own photos in this album."</p>
                        }.into_any(),
                        Some(data) => view! {
                            <ul class="photos-share-people">
                                {data.members.into_iter().map(|member| {
                                    let avatar: String = member.display_name.chars().take(2).collect::<String>().to_uppercase();
                                    let member_role = RwSignal::new(member.role.clone());
                                    let m_change = member.clone();
                                    let m_remove = member.clone();
                                    let display_for_select = format!("Access for {}", member.display_name);
                                    let display_for_remove = format!("Stop sharing with {}", member.display_name);
                                    view! {
                                        <li>
                                            <span class="photos-share-avatar" aria-hidden="true">{avatar}</span>
                                            <span class="photos-share-name">
                                                <strong>{member.display_name.clone()}</strong>
                                                <small>"@"{member.username.clone()}</small>
                                            </span>
                                            <Select
                                                aria_label=display_for_select
                                                options=role_options.get_value()
                                                value=member_role
                                                on_change=Callback::new(move |next: String| change_member(m_change.clone(), next))
                                            />
                                            <button
                                                type="button"
                                                class="icon-button"
                                                aria-label=display_for_remove
                                                title="Stop sharing"
                                                on:click=move |_| remove_member(m_remove.clone())
                                            >
                                                <Icon name="close"/>
                                            </button>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any(),
                    }}
                </section>
                <section aria-label="Shared links">
                    <h3>"Shared links"</h3>
                    <p class="photos-hint">"Anyone with a link can open the album without an account."</p>
                    <For
                        each=move || sharing.with(|s| s.as_ref().map(|s| s.links.clone()).unwrap_or_default())
                        key=|link| link.id.clone()
                        children=move |link| {
                            let link_id = link.id.clone();
                            let sess = session.get_value();
                            view! {
                                <LinkEditor
                                    link=link
                                    session=sess
                                    on_changed=Callback::new(move |value: Option<PhotoLink>| {
                                        let id = link_id.clone();
                                        sharing.update(|current| {
                                            if let Some(s) = current {
                                                match value {
                                                    Some(next) => {
                                                        for l in &mut s.links {
                                                            if l.id == next.id {
                                                                *l = next.clone();
                                                            }
                                                        }
                                                    }
                                                    None => {
                                                        s.links.retain(|l| l.id != id);
                                                    }
                                                }
                                            }
                                        });
                                        { on_changed.try_run(()); };
                                    })
                                />
                            }
                        }
                    />
                    <button
                        type="button"
                        class="secondary-button"
                        disabled=move || busy.get() || sharing.with(Option::is_none)
                        on:click=create_link
                    >
                        <Icon name="link"/>"Create link"
                    </button>
                </section>
                <div class="dialog-actions">
                    <button type="button" class="primary-button" on:click=move |_| { on_close.try_run(()); }>"Done"</button>
                </div>
            </div>
        </EditorPane>
    }
}

/// One shared link: copy, expiry, password and the download, upload and metadata switches. Changes save at once.
#[component]
pub fn LinkEditor(
    link: PhotoLink,
    session: BrowserSession,
    on_changed: Callback<Option<PhotoLink>>,
) -> impl IntoView {
    let current = RwSignal::new(link);
    let error = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let editing_password = RwSignal::new(false);
    let copied = RwSignal::new(false);
    let session = StoredValue::new(session);

    let save = move |change: Map<String, Value>, after_ok: Option<Callback<()>>| {
        error.set(String::new());
        let snapshot = current.get_untracked();
        let csrf = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            let patch = |rev: u64, change: &Map<String, Value>, id: &str, csrf: &str| {
                let mut body = change.clone();
                body.insert("revision".to_owned(), json!(rev));
                let url = format!("{BASE}/links/{id}");
                let body_val = Value::Object(body);
                let csrf = csrf.to_owned();
                async move { api::send::<PhotoLink>(&url, "PATCH", Some(body_val), &csrf).await }
            };
            let result = match patch(snapshot.revision, &change, &snapshot.id, &csrf).await {
                Ok(next) => Ok(next),
                Err(cause) if cause.status == 409 => {
                    match api::get::<LinksResponse>(&format!("{BASE}/links")).await {
                        Ok(resp) => match resp.links.into_iter().find(|l| l.id == snapshot.id) {
                            Some(fresh) => {
                                patch(fresh.revision, &change, &snapshot.id, &csrf).await
                            }
                            None => Err(cause),
                        },
                        Err(_) => Err(cause),
                    }
                }
                Err(cause) => Err(cause),
            };
            match result {
                Ok(next) => {
                    current.try_set(next.clone());
                    {
                        on_changed.try_run(Some(next));
                    };
                    if let Some(cb) = after_ok {
                        cb.run(());
                    }
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
        });
    };

    let remove = move |_| {
        let id = current.with_untracked(|c| c.id.clone());
        let csrf = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            match api::request::<Value>(
                &format!("{BASE}/links/{id}"),
                Req {
                    method: "DELETE",
                    csrf: Some(&csrf),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) => {
                    on_changed.try_run(None);
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
        });
    };

    let copy = move |_| {
        let url = current.with_untracked(|c| c.url.clone());
        spawn_local(async move {
            match copy_to_clipboard(&url).await {
                Ok(()) => {
                    copied.try_set(true);
                    crate::sleep(2000).await;
                    copied.try_set(false);
                }
                Err(()) => {
                    error.try_set("Copy the link from the field.".to_owned());
                }
            }
        });
    };

    let none_state = Signal::derive(|| None);
    let allow_download = Signal::derive(move || current.with(|c| c.allow_download));
    let allow_upload = Signal::derive(move || current.with(|c| c.allow_upload));
    let show_metadata = Signal::derive(move || current.with(|c| c.show_metadata));
    let has_album = Memo::new(move |_| current.with(|c| !c.album_id.is_empty()));

    view! {
        <article
            class="photos-link"
            aria-label=move || current.with(|c| if c.album_name.is_empty() { "Shared link".to_owned() } else { format!("Shared link to {}", c.album_name) })
        >
            {move || {
                let url = current.with(|c| c.url.clone());
                if !url.is_empty() {
                    let href = url.clone();
                    view! {
                        <div class="photos-link-url">
                            <input
                                readonly
                                value=url.clone()
                                prop:value=url
                                aria-label="Link address"
                                on:focus=move |e| {
                                    if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                        input.select();
                                    }
                                }
                            />
                            <button type="button" class="secondary-button" on:click=copy>
                                <Icon name=Signal::derive(move || if copied.get() { "check" } else { "copy" })/>
                                {move || if copied.get() { "Copied" } else { "Copy" }}
                            </button>
                            <a class="icon-button" href=href target="_blank" rel="noreferrer" aria-label="Open link" title="Open link">
                                <Icon name="external"/>
                            </a>
                        </div>
                    }
                    .into_any()
                } else {
                    view! {
                        <p class="photos-hint">"This link was made on another installation and can\u{2019}t be shown. Remove it and create a new one."</p>
                    }
                    .into_any()
                }
            }}
            <p class="photos-link-meta">
                {move || current.with(|c| {
                    let mut out = expiry_label(c.expires_at);
                    if c.has_password {
                        out.push_str(" \u{00b7} Password");
                    }
                    if c.album_id.is_empty() {
                        out.push_str(&format!(" \u{00b7} {} photo{}", c.count, if c.count == 1 { "" } else { "s" }));
                    }
                    out
                })}
            </p>
            <label class="photos-link-row">"Expires"
                {move || {
                    let (expires_at, created_at) = current.with(|c| (c.expires_at, c.created_at));
                    let days = if expires_at > 0.0 {
                        ((expires_at - created_at) / 86_400_000.0).round().max(1.0) as i64
                    } else {
                        0
                    };
                    let choice = EXPIRIES.iter().find(|(_, d)| *d == days).map(|(_, d)| *d).unwrap_or(-1);
                    let mut opts = Vec::new();
                    if choice < 0 {
                        opts.push(("-1".to_owned(), expiry_label(expires_at)));
                    }
                    for (label, d) in EXPIRIES {
                        opts.push((d.to_string(), label.to_owned()));
                    }
                    let selected = RwSignal::new(choice.to_string());
                    view! {
                        <Select
                            options=opts
                            value=selected
                            on_change=Callback::new(move |val: String| {
                                if let Ok(d) = val.parse::<i64>() && d >= 0 {
                                    let next_at = if d > 0 { js_sys::Date::now() + (d as f64) * 86_400_000.0 } else { 0.0 };
                                    let mut map = Map::new();
                                    map.insert("expiresAt".to_owned(), json!(next_at));
                                    save(map, None);
                                }
                            })
                        />
                    }
                }}
            </label>
            {move || {
                if editing_password.get() {
                    view! {
                        <form class="photos-link-password" on:submit=move |event: web_sys::SubmitEvent| {
                            event.prevent_default();
                            let pw = password.get_untracked();
                            if !valid_link_password(&pw) {
                                error.set("Use a link password of 4 to 200 characters.".to_owned());
                                return;
                            }
                            let mut map = Map::new();
                            map.insert("password".to_owned(), json!(pw));
                            save(map, Some(Callback::new(move |_| {
                                password.try_set(String::new());
                                editing_password.try_set(false);
                            })));
                        }>
                            <label>"Link password"
                                <input
                                    type="password"
                                    autocomplete="new-password"
                                    prop:value=move || password.get()
                                    maxlength="200"
                                    on:input=move |e| password.set(event_target_value(&e))
                                />
                            </label>
                            <button class="secondary-button">"Set password"</button>
                            <button type="button" class="text-button" on:click=move |_| { editing_password.set(false); password.set(String::new()); }>"Cancel"</button>
                        </form>
                    }
                    .into_any()
                } else {
                    let has_pw = current.with(|c| c.has_password);
                    view! {
                        <div class="photos-link-row">
                            <span>"Password"</span>
                            <span class="photos-hint">{if has_pw { "On" } else { "Off" }}</span>
                            <button type="button" class="text-button" on:click=move |_| editing_password.set(true)>
                                {if has_pw { "Change" } else { "Add password" }}
                            </button>
                            {has_pw.then(|| view! {
                                <button type="button" class="text-button" on:click=move |_| {
                                    let mut map = Map::new();
                                    map.insert("password".to_owned(), Value::Null);
                                    save(map, None);
                                }>"Remove"</button>
                            })}
                        </div>
                    }
                    .into_any()
                }
            }}
            <SettingSwitch
                label="Allow download"
                hint="Visitors can download originals"
                state=none_state
                checked=allow_download
                on_change=Callback::new(move |val: bool| {
                    let mut map = Map::new();
                    map.insert("allowDownload".to_owned(), json!(val));
                    save(map, None);
                })
            />
            <Show when=move || has_album.get()>
                <SettingSwitch
                    label="Allow upload"
                    hint="Visitors can add photos; they go to your Photos folder and this album"
                    state=none_state
                    checked=allow_upload
                    on_change=Callback::new(move |val: bool| {
                        let mut map = Map::new();
                        map.insert("allowUpload".to_owned(), json!(val));
                        save(map, None);
                    })
                />
            </Show>
            <SettingSwitch
                label="Show metadata"
                hint="Dates, camera details and location"
                state=none_state
                checked=show_metadata
                on_change=Callback::new(move |val: bool| {
                    let mut map = Map::new();
                    map.insert("showMetadata".to_owned(), json!(val));
                    save(map, None);
                })
            />
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            <button type="button" class="text-button danger" on:click=remove><Icon name="trash"/>"Remove link"</button>
        </article>
    }
}

/// A new link to chosen photos, then its settings.
#[component]
pub fn LinkDialog(
    ids: Vec<String>,
    session: BrowserSession,
    on_close: Callback<()>,
    on_created: Callback<()>,
) -> impl IntoView {
    let link: RwSignal<Option<PhotoLink>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let count = ids.len();
    let session = StoredValue::new(session);

    {
        let ids = ids.clone();
        let csrf = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            let body = json!({
                "ids": ids,
                "expiresAt": 0,
                "allowDownload": true,
                "showMetadata": true,
            });
            match api::send::<PhotoLink>(&format!("{BASE}/links"), "POST", Some(body), &csrf).await
            {
                Ok(value) => {
                    link.try_set(Some(value));
                    {
                        on_created.try_run(());
                    };
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
        });
    }

    let created = Memo::new(move |_| link.with(|l| l.as_ref().map(|l| l.id.clone())));
    view! {
        <Dialog title="Shared link" on_close=on_close>
            <p>{count}" photo"{if count == 1 { "" } else { "s" }}" \u{00b7} anyone with the link can view "{if count == 1 { "it" } else { "them" }}" without an account."</p>
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            {move || (created.get().is_none() && error.get().is_empty()).then(|| view! { <p role="status">"Creating link\u{2026}"</p> })}
            {move || created.get().and_then(|_| link.get_untracked()).map(|initial| {
                let sess = session.get_value();
                view! {
                    <LinkEditor
                        link=initial
                        session=sess
                        on_changed=Callback::new(move |value: Option<PhotoLink>| {
                            if value.is_none() {
                                { on_close.try_run(()); };
                            }
                        })
                    />
                }
            })}
            <div class="dialog-actions">
                <button type="button" class="primary-button" on:click=move |_| { on_close.try_run(()); }>"Done"</button>
            </div>
        </Dialog>
    }
}

/// Every link you made, for Photos settings.
#[component]
pub fn LinksList(session: BrowserSession) -> impl IntoView {
    let links: RwSignal<Option<Vec<PhotoLink>>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let session = StoredValue::new(session);

    spawn_local(async move {
        match api::get::<LinksResponse>(&format!("{BASE}/links")).await {
            Ok(value) => {
                links.try_set(Some(value.links));
            }
            Err(cause) => {
                error.try_set(error_message(&cause.message));
            }
        }
    });

    let state_kind = Memo::new(move |_| {
        if !error.get().is_empty() {
            "error"
        } else {
            match links.with(|l| l.as_ref().map(Vec::len)) {
                None => "loading",
                Some(0) => "empty",
                Some(_) => "ready",
            }
        }
    });

    move || {
        match state_kind.get() {
        "error" => view! { <p role="alert" class="error">{move || error.get()}</p> }.into_any(),
        "loading" => view! { <p role="status" class="photos-hint">"Loading\u{2026}"</p> }.into_any(),
        "empty" => view! { <p class="photos-hint">"No shared links. Create one from an album\u{2019}s Share pane or from selected photos."</p> }.into_any(),
        _ => view! {
            <div class="photos-links">
                <For
                    each=move || links.with(|l| l.clone().unwrap_or_default())
                    key=|link| link.id.clone()
                    children=move |link| {
                        let id_summary = link.id.clone();
                        let id_expiry = link.id.clone();
                        let id_change = link.id.clone();
                        let sess = session.get_value();
                        view! {
                            <details class="photos-link-summary">
                                <summary>
                                    <Icon name="link"/>
                                    <span>{move || links.with(|list| {
                                        list.as_ref()
                                            .and_then(|l| l.iter().find(|x| x.id == id_summary))
                                            .map(|l| if !l.album_name.is_empty() { l.album_name.clone() } else { format!("{} photo{}", l.count, if l.count == 1 { "" } else { "s" }) })
                                            .unwrap_or_default()
                                    })}</span>
                                    <small>{move || links.with(|list| {
                                        list.as_ref()
                                            .and_then(|l| l.iter().find(|x| x.id == id_expiry))
                                            .map(|l| expiry_label(l.expires_at))
                                            .unwrap_or_default()
                                    })}</small>
                                </summary>
                                <LinkEditor
                                    link=link
                                    session=sess
                                    on_changed=Callback::new(move |value: Option<PhotoLink>| {
                                        let id = id_change.clone();
                                        links.update(|current| {
                                            if let Some(list) = current {
                                                match value {
                                                    Some(next) => {
                                                        for l in list.iter_mut() {
                                                            if l.id == next.id {
                                                                *l = next.clone();
                                                            }
                                                        }
                                                    }
                                                    None => {
                                                        list.retain(|l| l.id != id);
                                                    }
                                                }
                                            }
                                        });
                                    })
                                />
                            </details>
                        }
                    }
                />
            </div>
        }
        .into_any(),
    }
    }
}
