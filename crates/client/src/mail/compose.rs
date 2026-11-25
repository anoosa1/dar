//! Writing mail: floating & minimizable compose sheet, Contacts + recent autocomplete on
//! recipient chips, drag-and-drop & Dar Files attachment picker, debounced draft autosave,
//! Notes Markdown toolbar with Write/Preview, and the pinned inline reply dock.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use super::model::{
    Account, Addr, BASE, Body, Detail, MailState, avatar_tone, base64, day_label, encode, initials,
    render_html, tone,
};
use crate::api;
use crate::dates::local_time;
use crate::files::types::FileTile;
use crate::icons::Icon;
use crate::markdown::{Markdown, MarkdownToolbar, format_markdown};
use crate::ui::{Dialog, unique_id};

#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) struct Upload {
    pub name: String,
    pub kind: String,
    pub data: String,
    pub size: f64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) struct ComposeSpec {
    pub account_id: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: String,
    pub in_reply_to: Option<String>,
    pub replaces: Option<String>,
    pub attachments: Vec<Upload>,
    pub error: String,
    /// Written in Markdown and sent with an HTML part made from it.
    pub markdown: bool,
}

fn signature_block(account: Option<&Account>) -> String {
    account
        .map(|a| a.signature.trim())
        .filter(|s| !s.is_empty())
        .map(|s| format!("\n\n-- \n{s}"))
        .unwrap_or_default()
}

fn plain(body: &Body) -> String {
    if !body.text.is_empty() {
        return body.text.clone();
    }
    let frag = render_html(&body.html);
    frag.text_content().unwrap_or_default()
}

impl ComposeSpec {
    pub fn new(account: &Account) -> Self {
        ComposeSpec {
            account_id: account.id.clone(),
            text: signature_block(Some(account)),
            ..Default::default()
        }
    }

    pub fn reply(d: &Detail, mode: &str, account: Option<&Account>) -> Self {
        let own = account
            .map(|a| a.address.to_lowercase())
            .unwrap_or_default();
        let s = &d.summary;
        let prefix = if mode == "forward" { "Fwd: " } else { "Re: " };
        let lower = s.subject.to_lowercase();
        let subject = if lower.starts_with(prefix.to_lowercase().as_str())
            || (mode != "forward" && lower.starts_with("re:"))
        {
            s.subject.clone()
        } else {
            format!("{prefix}{}", s.subject)
        };
        let mut to: Vec<String> = Vec::new();
        let mut cc: Vec<String> = Vec::new();
        if mode != "forward" {
            let reply_to: Vec<String> = d.body.reply_to.iter().map(|a| a.address.clone()).collect();
            if s.from_address.to_lowercase() == own && !s.recipients.to.is_empty() {
                to = s.recipients.to.iter().map(|a| a.address.clone()).collect();
            } else if !reply_to.is_empty() {
                to = reply_to;
            } else if !s.from_address.is_empty() {
                to.push(s.from_address.clone());
            }
            if mode == "all" {
                for a in s.recipients.to.iter().chain(&s.recipients.cc) {
                    let lower = a.address.to_lowercase();
                    if lower != own
                        && !to.iter().any(|t| t.to_lowercase() == lower)
                        && !cc.iter().any(|t| t.to_lowercase() == lower)
                    {
                        cc.push(a.address.clone());
                    }
                }
            }
        }
        let when = format!(
            "{}, {}",
            day_label(s.date),
            local_time(s.date, &[("hour", "numeric"), ("minute", "2-digit")])
        );
        let original = plain(&d.body);
        let text = if mode == "forward" {
            let to_line: Vec<String> = s.recipients.to.iter().map(Addr::label).collect();
            format!(
                "{}\n\n---------- Forwarded message ----------\nFrom: {} <{}>\nDate: {when}\nSubject: {}\nTo: {}\n\n{}",
                signature_block(account),
                s.sender(),
                s.from_address,
                s.subject,
                to_line.join(", "),
                original.trim()
            )
        } else {
            let quoted: Vec<String> = original
                .trim()
                .lines()
                .map(|l| {
                    if l.is_empty() {
                        ">".into()
                    } else {
                        format!("> {l}")
                    }
                })
                .collect();
            format!(
                "{}\n\nOn {when}, {} wrote:\n{}",
                signature_block(account),
                s.sender(),
                quoted.join("\n")
            )
        };
        ComposeSpec {
            account_id: s.account_id.clone(),
            to,
            cc,
            subject,
            text,
            in_reply_to: (mode != "forward").then(|| s.id.clone()),
            ..Default::default()
        }
    }

    /// A saved draft, opened again for editing.
    pub fn draft(d: &Detail) -> Self {
        let s = &d.summary;
        ComposeSpec {
            account_id: s.account_id.clone(),
            to: d.body.to.iter().map(|a| a.address.clone()).collect(),
            cc: d.body.cc.iter().map(|a| a.address.clone()).collect(),
            subject: s.subject.clone(),
            // A Markdown draft reopens as its source, not its rendered text.
            text: if d.body.format == "markdown" {
                d.body.text_part.clone()
            } else {
                plain(&d.body)
            },
            markdown: d.body.format == "markdown",
            replaces: Some(s.id.clone()),
            ..Default::default()
        }
    }

    pub fn body(&self) -> Value {
        json!({
            "accountId": self.account_id, "to": self.to, "cc": self.cc, "bcc": self.bcc,
            "subject": self.subject, "text": self.text, "inReplyTo": self.in_reply_to,
            "replaces": self.replaces, "format": if self.markdown { "markdown" } else { "plain" },
            "attachments": self.attachments.iter().map(|a| json!({ "name": a.name, "type": a.kind, "data": a.data })).collect::<Vec<_>>(),
        })
    }

    fn has_content(&self) -> bool {
        !self.to.is_empty()
            || !self.cc.is_empty()
            || !self.subject.trim().is_empty()
            || !self.attachments.is_empty()
            || !self.text.trim().is_empty() && self.text.trim() != "--"
    }
}

fn valid_address(a: &str) -> bool {
    let a = a.trim();
    a.len() >= 3
        && a.contains('@')
        && !a.starts_with('@')
        && !a.ends_with('@')
        && !a.contains(char::is_whitespace)
        && !a.contains(['<', '>', ',', ';'])
}

#[derive(Clone, Debug, PartialEq)]
struct Suggestion {
    name: String,
    address: String,
    source: &'static str,
}

/// Addresses as chips with Contacts & recent-correspondents autocomplete.
#[component]
fn ChipInput(
    state: MailState,
    #[prop(into)] label: String,
    value: RwSignal<Vec<String>>,
    #[prop(optional)] autofocus: bool,
    #[prop(optional)] extra: Option<Children>,
) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let hits = RwSignal::new(Vec::<Suggestion>::new());
    let active_hit = RwSignal::new(0usize);
    let id = unique_id();
    let input = NodeRef::<html::Input>::new();

    let add_addr = move |addr: String| {
        let clean = addr.trim().to_owned();
        if clean.is_empty() {
            return;
        }
        value.update(|v| {
            if !v.iter().any(|x| x.eq_ignore_ascii_case(&clean)) {
                v.push(clean);
            }
        });
        text.set(String::new());
        hits.set(Vec::new());
        active_hit.set(0);
    };

    let commit = move || {
        let raw = text.get_untracked();
        let parts: Vec<String> = raw
            .split([',', ';'])
            .map(|p| {
                let p = p.trim();
                match (p.rfind('<'), p.rfind('>')) {
                    (Some(a), Some(b)) if a < b => p[a + 1..b].trim().to_owned(),
                    _ => p.to_owned(),
                }
            })
            .filter(|p| !p.is_empty())
            .collect();
        if !parts.is_empty() {
            value.update(|v| {
                for p in parts {
                    if !v.iter().any(|x| x.eq_ignore_ascii_case(&p)) {
                        v.push(p);
                    }
                }
            });
        }
        text.set(String::new());
        hits.set(Vec::new());
        active_hit.set(0);
    };

    // Contacts + recent correspondents autocomplete
    let req_seq = StoredValue::new(0u32);
    Effect::new(move |_| {
        let q = text.get().trim().to_lowercase();
        if q.len() < 2 || q.contains(',') || q.contains(';') {
            hits.set(Vec::new());
            return;
        }
        let seq = req_seq.get_value() + 1;
        req_seq.set_value(seq);
        let existing: Vec<String> = value
            .get_untracked()
            .into_iter()
            .map(|x| x.to_lowercase())
            .collect();
        let recents: Vec<Suggestion> = state
            .recent_correspondents()
            .into_iter()
            .filter(|(n, a)| {
                !existing.contains(&a.to_lowercase())
                    && (n.to_lowercase().contains(&q) || a.to_lowercase().contains(&q))
            })
            .take(5)
            .map(|(name, address)| Suggestion {
                name,
                address,
                source: "Recent",
            })
            .collect();
        hits.set(recents.clone());
        active_hit.set(0);

        spawn_local(async move {
            let enc = encode(&q);
            if let Ok(res) = api::get::<Value>(&format!("/api/v1/contacts/people?q={enc}")).await {
                if req_seq.try_get_value() != Some(seq) {
                    return;
                }
                let mut combined = Vec::new();
                if let Some(items) = res["items"].as_array() {
                    for p in items.iter().take(6) {
                        let name = p["data"]["displayName"]
                            .as_str()
                            .or_else(|| p["displayName"].as_str())
                            .unwrap_or_default()
                            .to_owned();
                        let emails = p["data"]["emails"]
                            .as_array()
                            .or_else(|| p["emails"].as_array());
                        if let Some(arr) = emails {
                            for em in arr {
                                let addr = em["value"]
                                    .as_str()
                                    .or_else(|| em.as_str())
                                    .unwrap_or_default();
                                if !addr.is_empty() && !existing.contains(&addr.to_lowercase()) {
                                    combined.push(Suggestion {
                                        name: name.clone(),
                                        address: addr.to_owned(),
                                        source: "Contacts",
                                    });
                                    break;
                                }
                            }
                        }
                    }
                }
                for r in recents {
                    if !combined
                        .iter()
                        .any(|c| c.address.eq_ignore_ascii_case(&r.address))
                    {
                        combined.push(r);
                    }
                }
                combined.truncate(6);
                let _ = hits.try_set(combined);
                let _ = active_hit.try_set(0);
            }
        });
    });

    Effect::new(move |_| {
        if autofocus && let Some(i) = input.get() {
            let _ = i.focus();
        }
    });

    view! {
        <div class="mail-field">
            <label for=id.clone()>{label.clone()}</label>
            <div class="mail-chips" on:click=move |_| { if let Some(i) = input.get_untracked() { let _ = i.focus(); } }>
                {move || value.get().into_iter().enumerate().map(|(i, a)| {
                    let bad = !valid_address(&a);
                    view! {
                        <span class=if bad { "mail-chip is-invalid" } else { "mail-chip" }>
                            <span class=avatar_tone(&a) aria-hidden="true">{initials(&a)}</span>{a.clone()}
                            {bad.then(|| view! { <span class="sr-only">" (not a valid address)"</span> })}
                            <button type="button" aria-label=format!("Remove {a}") on:click=move |e| { e.stop_propagation(); value.update(|v| { if i < v.len() { v.remove(i); } }); }><Icon name="close"/></button>
                        </span>
                    }
                }).collect_view()}
                <input
                    node_ref=input
                    id=id
                    type="email"
                    multiple
                    autocomplete="off"
                    prop:value=move || text.get()
                    on:input=move |e| {
                        let v = event_target_value(&e);
                        text.set(v.clone());
                        if v.ends_with(',') || v.ends_with(';') {
                            commit();
                        }
                    }
                    on:blur=move |_| commit()
                    on:keydown=move |e| {
                        let list = hits.get_untracked();
                        match e.key().as_str() {
                            "ArrowDown" if !list.is_empty() => {
                                e.prevent_default();
                                active_hit.update(|i| *i = (*i + 1) % list.len());
                            }
                            "ArrowUp" if !list.is_empty() => {
                                e.prevent_default();
                                active_hit.update(|i| *i = if *i == 0 { list.len() - 1 } else { *i - 1 });
                            }
                            "Enter" if !text.get_untracked().trim().is_empty() => {
                                e.prevent_default();
                                if let Some(s) = list.get(active_hit.get_untracked()) {
                                    add_addr(s.address.clone());
                                } else {
                                    commit();
                                }
                            }
                            "Escape" if !list.is_empty() => {
                                e.prevent_default();
                                e.stop_propagation();
                                hits.set(Vec::new());
                            }
                            "Backspace" if text.get_untracked().is_empty() => {
                                value.update(|v| { v.pop(); });
                            }
                            _ => {}
                        }
                    }
                />
            </div>
            {extra.map(|c| c())}
            {move || {
                let list = hits.get();
                (!list.is_empty()).then(|| view! {
                    <ul class="mail-autocomplete" role="listbox" aria-label="Suggested recipients">
                        {list.into_iter().enumerate().map(|(idx, s)| {
                            let addr = s.address.clone();
                            let display = if s.name.is_empty() { s.address.clone() } else { s.name.clone() };
                            view! {
                                <li
                                    role="option"
                                    class=move || if active_hit.get() == idx { "is-active" } else { "" }
                                    aria-selected=move || (active_hit.get() == idx).to_string()
                                    on:mousedown=move |e| {
                                        e.prevent_default();
                                        add_addr(addr.clone());
                                    }
                                >
                                    <span class=avatar_tone(&s.address) aria-hidden="true">{initials(&display)}</span>
                                    <span class="mail-ac-text"><b>{display}</b><small>{s.address}</small></span>
                                    <span class="mail-ac-src">{s.source}</span>
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                })
            }}
        </div>
    }
}

async fn read_file(file: web_sys::File) -> Option<Upload> {
    let buffer = JsFuture::from(file.array_buffer()).await.ok()?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    Some(Upload {
        name: file.name(),
        kind: if file.type_().is_empty() {
            "application/octet-stream".into()
        } else {
            file.type_()
        },
        size: bytes.len() as f64,
        data: base64(&bytes),
    })
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct PickerItem {
    id: String,
    name: String,
    kind: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct PickerCrumb {
    id: String,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PickerPage {
    folder: Option<PickerItem>,
    ancestors: Vec<PickerCrumb>,
    items: Vec<PickerItem>,
}

/// Modal dialog to attach files directly from Dar Files.
#[component]
fn FilesAttachDialog(on_pick: Callback<Upload>, on_close: Callback<()>) -> impl IntoView {
    let parent = RwSignal::new(String::new());
    let page = RwSignal::new(None::<PickerPage>);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);

    Effect::new(move |_| {
        let p = parent.get();
        page.set(None);
        error.set(String::new());
        spawn_local(async move {
            let url = format!("/api/v1/files/picker?parentId={}&kind=file", encode(&p));
            match api::get::<PickerPage>(&url).await {
                Ok(v) => {
                    let _ = page.try_set(Some(v));
                }
                Err(e) => {
                    let _ = error.try_set(e.message);
                }
            }
        });
    });

    let pick_file = move |item: PickerItem| {
        busy.set(true);
        error.set(String::new());
        spawn_local(async move {
            let url = format!("/api/v1/files/{}/content", encode(&item.id));
            let Some(win) = web_sys::window() else {
                let _ = busy.try_set(false);
                return;
            };
            match JsFuture::from(win.fetch_with_str(&url)).await {
                Ok(resp) => {
                    let resp: web_sys::Response = resp.unchecked_into();
                    let kind = resp
                        .headers()
                        .get("content-type")
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "application/octet-stream".into());
                    if let Ok(p) = resp.array_buffer()
                        && let Ok(buf) = JsFuture::from(p).await
                    {
                        let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                        if bytes.len() > 25 * 1024 * 1024 {
                            let _ = error.try_set(
                                "That file is larger than the 25 MB attachment limit.".into(),
                            );
                            let _ = busy.try_set(false);
                            return;
                        }
                        on_pick.try_run(Upload {
                            name: item.name,
                            kind,
                            size: bytes.len() as f64,
                            data: base64(&bytes),
                        });
                        on_close.try_run(());
                        return;
                    }
                    let _ = error.try_set("Could not read that file.".into());
                }
                Err(_) => {
                    let _ = error.try_set("Could not fetch that file.".into());
                }
            }
            let _ = busy.try_set(false);
        });
    };

    view! {
        <Dialog title="Attach from Files" on_close=on_close>
            <div class="mail-files-picker">
                {move || page.get().map(|p| {
                    let up = p.folder.as_ref().map(|_| p.ancestors.last().map(|a| a.id.clone()).unwrap_or_default());
                    view! {
                        <div class="dar-picker-head">
                            <button type="button" class="icon-button" disabled=up.is_none() aria-label="Up one folder" on:click=move |_| if let Some(u) = &up { parent.set(u.clone()); }><Icon name="back"/></button>
                            <nav class="dar-picker-path" aria-label="Folder path">
                                <button type="button" on:click=move |_| parent.set(String::new())>"All files"</button>
                                {p.ancestors.into_iter().map(|a| {
                                    let id = a.id.clone();
                                    view! { <span><Icon name="chevron"/><button type="button" on:click=move |_| parent.set(id.clone())>{a.name}</button></span> }
                                }).collect_view()}
                                {p.folder.map(|f| view! { <span><Icon name="chevron"/><strong>{f.name}</strong></span> })}
                            </nav>
                        </div>
                        <ul class="dar-picker-list">
                            {p.items.into_iter().map(|item| {
                                let is_folder = item.kind == "folder";
                                let id = item.id.clone();
                                let item_pick = item.clone();
                                view! {
                                    <li>
                                        <button
                                            type="button"
                                            class="dar-picker-row"
                                            disabled=move || busy.get()
                                            on:click=move |_| {
                                                if is_folder {
                                                    parent.set(id.clone());
                                                } else {
                                                    pick_file(item_pick.clone());
                                                }
                                            }
                                        >
                                            <FileTile kind=item.kind.clone() name=item.name.clone() media_type=""/>
                                            <span>{item.name}</span>
                                            {is_folder.then(|| view! { <Icon name="chevron"/> })}
                                        </button>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }
                })}
                {move || (page.with(Option::is_none) && error.with(String::is_empty)).then(|| view! { <p class="dar-picker-state muted">"Loading files\u{2026}"</p> })}
                {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            </div>
            <div class="dialog-actions">
                <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
            </div>
        </Dialog>
    }
}

#[component]
pub(crate) fn ComposeSheet(state: MailState, spec: ComposeSpec) -> impl IntoView {
    let accounts = state
        .overview
        .with_untracked(|o| o.as_ref().map(|o| o.accounts.clone()).unwrap_or_default());
    let accounts_sv = StoredValue::new(accounts.clone());
    let account = RwSignal::new(spec.account_id.clone());
    let to = RwSignal::new(spec.to.clone());
    let cc = RwSignal::new(spec.cc.clone());
    let bcc = RwSignal::new(spec.bcc.clone());
    let show_cc = RwSignal::new(!spec.cc.is_empty() || !spec.bcc.is_empty());
    let subject = RwSignal::new(spec.subject.clone());
    let text = RwSignal::new(spec.text.clone());
    let attachments = RwSignal::new(spec.attachments.clone());
    let error = RwSignal::new(spec.error.clone());
    let status = RwSignal::new(String::new());
    let expanded = RwSignal::new(false);
    let markdown = RwSignal::new(spec.markdown);
    let preview = RwSignal::new(false);
    let files_picker = RwSignal::new(false);
    let dragging = RwSignal::new(false);
    let body_field = NodeRef::<html::Textarea>::new();
    let busy = RwSignal::new(false);
    let replaces = StoredValue::new(spec.replaces.clone());
    let in_reply_to = spec.in_reply_to.clone();
    let start = StoredValue::new(spec.clone());

    let current = move || ComposeSpec {
        account_id: account.get_untracked(),
        to: to.get_untracked(),
        cc: cc.get_untracked(),
        bcc: bcc.get_untracked(),
        subject: subject.get_untracked(),
        text: text.get_untracked(),
        in_reply_to: in_reply_to.clone(),
        replaces: replaces.get_value(),
        attachments: attachments.get_untracked(),
        error: String::new(),
        markdown: markdown.get_untracked(),
    };
    let current = StoredValue::new_local(current);

    let send = move || {
        let spec = current.with_value(|c| c());
        let all: Vec<&String> = spec.to.iter().chain(&spec.cc).chain(&spec.bcc).collect();
        if all.is_empty() {
            error.set("Add at least one recipient.".into());
            return;
        }
        if let Some(bad) = all.iter().find(|a| !valid_address(a)) {
            error.set(format!("{bad} is not an email address."));
            return;
        }
        state.send(spec);
    };

    let save_draft = move |then_close: bool| {
        let spec = current.with_value(|c| c());
        if !spec.has_content() || spec == start.get_value() {
            if then_close {
                state.compose.set(None);
                state.compose_minimized.set(false);
            }
            return;
        }
        busy.set(true);
        status.set("Saving draft\u{2026}".into());
        let csrf = state.csrf();
        spawn_local(async move {
            match api::send::<Value>(&format!("{BASE}/drafts"), "POST", Some(spec.body()), &csrf)
                .await
            {
                Ok(v) => {
                    replaces.set_value(v["id"].as_str().map(str::to_owned));
                    start.set_value(current.with_value(|c| c()));
                    let _ = status.try_set("Draft saved".into());
                    let _ = busy.try_set(false);
                    if then_close {
                        let _ = state.compose.try_set(None);
                        let _ = state.compose_minimized.try_set(false);
                        state.notify("Draft saved");
                    }
                    state.reload();
                }
                Err(e) => {
                    let _ = busy.try_set(false);
                    let _ = status.try_set(String::new());
                    let _ = error.try_set(format!("The draft was not saved: {}", e.message));
                }
            }
        });
    };

    let switch_account = move |next_id: String| {
        let prev_id = account.get_untracked();
        if prev_id == next_id {
            return;
        }
        accounts_sv.with_value(|list| {
            let old_sig = signature_block(list.iter().find(|a| a.id == prev_id));
            let new_sig = signature_block(list.iter().find(|a| a.id == next_id));
            if old_sig != new_sig {
                text.update(|t| {
                    if !old_sig.is_empty() && t.ends_with(&old_sig) {
                        let cut = t.len() - old_sig.len();
                        t.truncate(cut);
                    }
                    if !new_sig.is_empty() {
                        t.push_str(&new_sig);
                    }
                });
            }
        });
        account.set(next_id);
    };

    // The Notes formatting toolbar, applied to the selection.
    let format = Callback::new(move |(before, after): (&'static str, &'static str)| {
        let Some(field) = body_field.get_untracked() else {
            return;
        };
        let (start, end) = (
            field.selection_start().ok().flatten().unwrap_or(0),
            field.selection_end().ok().flatten().unwrap_or(0),
        );
        let (next, s, e) = format_markdown(&text.get_untracked(), start, end, before, after);
        text.set(next);
        markdown.set(true);
        let field = send_wrapper::SendWrapper::new(field);
        request_animation_frame(move || {
            let _ = field.focus();
            let _ = field.set_selection_range(s, e);
        });
    });

    let add_uploads = move |list: Vec<web_sys::File>| {
        spawn_local(async move {
            for f in list {
                let total: f64 = attachments.get_untracked().iter().map(|a| a.size).sum();
                if total + f.size() > 25.0 * 1024.0 * 1024.0 {
                    let _ = error.try_set(format!(
                        "{} would take the attachments over 25 MB.",
                        f.name()
                    ));
                    continue;
                }
                if let Some(u) = read_file(f).await {
                    let _ = attachments.try_update(|a| a.push(u));
                }
            }
        });
    };

    let file_input = NodeRef::<html::Input>::new();
    let on_files = move |e: leptos::ev::Event| {
        let Some(input) = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            return;
        };
        let Some(files) = input.files() else { return };
        let list: Vec<web_sys::File> = (0..files.length()).filter_map(|i| files.get(i)).collect();
        input.set_value("");
        add_uploads(list);
    };

    let sheet = NodeRef::<html::Section>::new();
    let to_first = spec.to.is_empty();
    Effect::new(move |_| {
        if !state.compose_minimized.get()
            && let Some(s) = sheet.get()
            && let Some(el) = s
                .query_selector(if to_first {
                    ".mail-chips input"
                } else {
                    "textarea"
                })
                .ok()
                .flatten()
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = el.focus();
            if let Ok(t) = el.clone().dyn_into::<web_sys::HtmlTextAreaElement>() {
                t.set_selection_range(0, 0).ok();
            }
        }
    });

    let heading_live = move || {
        let s = subject.get();
        if !s.trim().is_empty() {
            s
        } else if replaces.get_value().is_some() {
            "Draft".to_owned()
        } else {
            "New message".to_owned()
        }
    };

    let current_acct_tone = move || {
        let cur = account.get();
        accounts_sv.with_value(|list| {
            list.iter()
                .find(|a| a.id == cur)
                .map(|a| tone(a.color))
                .unwrap_or_default()
        })
    };

    let from_options = accounts.clone();
    view! {
        {move || if state.compose_minimized.get() {
            view! {
                <div class="mail-compose-tray" role="region" aria-label="Minimized draft">
                    <button
                        type="button"
                        class="mail-tray-open"
                        on:click=move |_| state.compose_minimized.set(false)
                    >
                        <Icon name="pencil"/>
                        <span class="mail-tray-title">{heading_live}</span>
                        <small>"Draft"</small>
                    </button>
                    <button type="button" class="icon-button" aria-label="Restore compose" title="Restore" on:click=move |_| state.compose_minimized.set(false)><Icon name="expand"/></button>
                    <button type="button" class="icon-button" aria-label="Close (saves a draft)" title="Close (saves a draft)" on:click=move |_| save_draft(true)><Icon name="close"/></button>
                </div>
            }.into_any()
        } else {
            let from_opts = from_options.clone();
            view! {
                <section
                    node_ref=sheet
                    class=move || format!(
                        "mail-compose{}{}",
                        if expanded.get() { " is-expanded" } else { "" },
                        if dragging.get() { " is-dragging" } else { "" }
                    )
                    role="dialog"
                    aria-modal="false"
                    aria-label="Compose message"
                    on:keydown=move |e| {
                        if e.key() == "Escape" && !e.default_prevented() {
                            e.prevent_default();
                            save_draft(true);
                        }
                        if e.key() == "Enter" && (e.ctrl_key() || e.meta_key()) {
                            e.prevent_default();
                            send();
                        }
                    }
                    on:dragover=move |e| { e.prevent_default(); dragging.set(true); }
                    on:dragleave=move |_| dragging.set(false)
                    on:drop=move |e| {
                        e.prevent_default();
                        dragging.set(false);
                        if let Some(dt) = e.data_transfer()
                            && let Some(files) = dt.files()
                        {
                            let list: Vec<web_sys::File> = (0..files.length()).filter_map(|i| files.get(i)).collect();
                            if !list.is_empty() {
                                add_uploads(list);
                            }
                        }
                    }
                >
                    <div class="mail-compose-head">
                        <h2>{heading_live}</h2>
                        <button type="button" class="icon-button" aria-label="Minimize" title="Minimize" on:click=move |_| state.compose_minimized.set(true)><Icon name="minimize"/></button>
                        <button type="button" class="icon-button" aria-label=move || if expanded.get() { "Shrink" } else { "Expand" } title=move || if expanded.get() { "Shrink" } else { "Expand" } on:click=move |_| expanded.update(|v| *v = !*v)><Icon name="expand"/></button>
                        <button type="button" class="icon-button" aria-label="Close (saves a draft)" title="Close (saves a draft)" on:click=move |_| save_draft(true)><Icon name="close"/></button>
                    </div>
                    <div class="mail-compose-fields">
                        <div class="mail-field">
                            <label for="mail-from">"From"</label>
                            <span class=move || format!("mail-acct is-dot {}", current_acct_tone()) aria-hidden="true"></span>
                            <select id="mail-from" prop:value=move || account.get() on:change=move |e| switch_account(event_target_value(&e))>
                                {from_opts.into_iter().map(|a| {
                                    let label = if a.display_name.is_empty() { a.address.clone() } else { format!("{} <{}>", a.display_name, a.address) };
                                    view! { <option value=a.id.clone() selected=a.id == spec.account_id>{label}</option> }
                                }).collect_view()}
                            </select>
                        </div>
                        <ChipInput
                            state=state
                            label="To"
                            value=to
                            autofocus=to_first
                            extra=Box::new(move || {
                                view! {
                                    {move || (!show_cc.get()).then(|| view! {
                                        <button type="button" class="text-button mail-cc-inline" on:click=move |_| show_cc.set(true)>"Cc / Bcc"</button>
                                    })}
                                }.into_any()
                            })
                        />
                        <Show when=move || show_cc.get()>
                            <ChipInput state=state label="Cc" value=cc/>
                            <ChipInput state=state label="Bcc" value=bcc/>
                        </Show>
                        <div class="mail-field">
                            <label for="mail-subject">"Subject"</label>
                            <input id="mail-subject" maxlength="900" placeholder="Subject" prop:value=move || subject.get() on:input=move |e| subject.set(event_target_value(&e))/>
                        </div>
                    </div>
                    <div class="mail-compose-tools">
                        {move || markdown.get().then(|| view! {
                            <div role="radiogroup" aria-label="Markdown view" class="ui-segmented mail-compose-mode">
                                <label class=move || if preview.get() { "" } else { "is-checked" }><input type="radio" name="mail-compose-mode" prop:checked=move || !preview.get() on:change=move |_| preview.set(false)/><span>"Write"</span></label>
                                <label class=move || if preview.get() { "is-checked" } else { "" }><input type="radio" name="mail-compose-mode" prop:checked=move || preview.get() on:change=move |_| preview.set(true)/><span>"Preview"</span></label>
                            </div>
                        })}
                        {move || (!preview.get()).then(|| view! { <MarkdownToolbar on_format=format disabled=Signal::derive(|| false)>{()}</MarkdownToolbar> })}
                    </div>
                    {move || if markdown.get() && preview.get() {
                        view! { <div class="mail-compose-body mail-compose-preview" tabindex="0" aria-label="Message preview"><Markdown source=text.get()/></div> }.into_any()
                    } else {
                        view! { <textarea node_ref=body_field class="mail-compose-body" aria-label="Message" placeholder="Write your message\u{2026}" spellcheck="true" prop:value=move || text.get() on:input=move |e| text.set(event_target_value(&e))></textarea> }.into_any()
                    }}
                    {move || (!attachments.with(Vec::is_empty)).then(|| view! {
                        <ul class="mail-attachments is-compose" aria-label="Attachments">
                            {attachments.get().into_iter().enumerate().map(|(i, a)| view! {
                                <li class="mail-attachment">
                                    <span class="mail-attachment-icon" aria-hidden="true"><Icon name="file"/></span>
                                    <span class="mail-attachment-name"><b>{a.name.clone()}</b><small>{api::format_bytes(a.size)}</small></span>
                                    <button type="button" class="icon-button" aria-label=format!("Remove {}", a.name) on:click=move |_| attachments.update(|v| { if i < v.len() { v.remove(i); } })><Icon name="close"/></button>
                                </li>
                            }).collect_view()}
                        </ul>
                    })}
                    {move || (!error.with(String::is_empty)).then(|| view! { <p class="mail-compose-error" role="alert"><Icon name="alert"/>{error.get()}</p> })}
                    <div class="mail-compose-foot">
                        <label class="icon-button mail-attach" title="Attach files"><Icon name="paperclip"/><span class="sr-only">"Attach files"</span><input node_ref=file_input type="file" multiple on:change=on_files/></label>
                        <button type="button" class="icon-button" aria-label="Attach from Files" title="Attach from Dar Files" on:click=move |_| files_picker.set(true)><Icon name="folder"/></button>
                        <span class="mail-compose-status" role="status">{move || status.get()}</span>
                        <button type="button" class="text-button" disabled=move || busy.get() on:click=move |_| save_draft(false)>"Save draft"</button>
                        <button type="button" class="icon-button" aria-label="Discard" title="Discard" on:click=move |_| { state.compose.set(None); state.compose_minimized.set(false); }><Icon name="trash"/></button>
                        <button type="button" class="primary-button mail-send" title="Send (Ctrl Enter)" on:click=move |_| send()><Icon name="paperPlane"/>"Send"</button>
                    </div>
                    <Show when=move || files_picker.get()>
                        <FilesAttachDialog
                            on_pick=Callback::new(move |u: Upload| {
                                let total: f64 = attachments.get_untracked().iter().map(|a| a.size).sum();
                                if total + u.size > 25.0 * 1024.0 * 1024.0 {
                                    error.set(format!("{} would take the attachments over 25 MB.", u.name));
                                } else {
                                    attachments.update(|a| a.push(u));
                                }
                            })
                            on_close=Callback::new(move |_| files_picker.set(false))
                        />
                    </Show>
                </section>
            }.into_any()
        }}
    }
}

/// Pinned inline reply dock at the bottom of the reader sheet: Reply, Reply all or Forward,
/// written in place with constrained recipient bar and Send; Expand moves it to the compose sheet.
#[component]
pub(crate) fn InlineReply(state: MailState, detail: Detail) -> impl IntoView {
    if detail.summary.draft {
        let d = detail.clone();
        return view! {
            <div class="mail-inline-reply is-draft">
                <Icon name="pencil"/><span>"This is a draft."</span>
                <button type="button" class="primary-button" on:click=move |_| state.open_compose(ComposeSpec::draft(&d))>"Continue editing"</button>
            </div>
        }.into_any();
    }
    let account = state.account(&detail.summary.account_id);
    let all_count = detail.summary.recipients.to.len() + detail.summary.recipients.cc.len();
    let mode = RwSignal::new(if all_count > 1 { "all" } else { "reply" });
    let text = RwSignal::new(String::new());
    let open = RwSignal::new(false);
    let detail = StoredValue::new(detail);
    let account = StoredValue::new(account);
    let spec = move || {
        let mut s = detail.with_value(|d| {
            account.with_value(|a| ComposeSpec::reply(d, mode.get_untracked(), a.as_ref()))
        });
        let written = text.get_untracked();
        if !written.trim().is_empty() {
            s.text = format!("{}{}", written.trim_end(), s.text);
        }
        s
    };
    let recipients = Memo::new(move |_| {
        let m = mode.get();
        detail.with_value(|d| {
            account.with_value(|a| {
                let s = ComposeSpec::reply(d, m, a.as_ref());
                s.to.iter()
                    .chain(&s.cc)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            })
        })
    });
    let area = NodeRef::<html::Textarea>::new();
    view! {
        <div class=move || if open.get() { "mail-inline-reply is-open" } else { "mail-inline-reply" }>
            {move || if !open.get() {
                view! {
                    <button type="button" class="mail-reply-start" on:click=move |_| { mode.set("reply"); open.set(true); }>
                        <Icon name="reply"/>
                        <span>{move || format!("Reply to {}", detail.with_value(|d| d.summary.sender()))}</span>
                    </button>
                    {(all_count > 1).then(|| view! {
                        <button type="button" class="secondary-button" on:click=move |_| { mode.set("all"); open.set(true); }>
                            <Icon name="replyAll"/>"Reply all"
                        </button>
                    })}
                    <button type="button" class="secondary-button" on:click=move |_| state.open_compose(detail.with_value(|d| account.with_value(|a| ComposeSpec::reply(d, "forward", a.as_ref()))))>
                        <Icon name="forward"/>"Forward"
                    </button>
                }.into_any()
            } else {
                view! {
                    <div class="mail-reply-to">
                        <select class="mail-reply-kind" aria-label="Reply kind" prop:value=move || mode.get() on:change=move |e| mode.set(if event_target_value(&e) == "all" { "all" } else { "reply" })>
                            <option value="reply">"Reply"</option>
                            <option value="all">"Reply all"</option>
                        </select>
                        <span class="mail-reply-recipients">{move || format!("to {}", recipients.get())}</span>
                        <button type="button" class="icon-button" aria-label="Open in the compose sheet" title="Open in the compose sheet" on:click=move |_| { state.open_compose(spec()); text.set(String::new()); open.set(false); }><Icon name="expand"/></button>
                    </div>
                    <textarea
                        node_ref=area
                        class="mail-reply-text"
                        aria-label="Reply"
                        placeholder="Write a reply\u{2026}"
                        autofocus
                        prop:value=move || text.get()
                        on:input=move |e| text.set(event_target_value(&e))
                        on:keydown=move |e| if e.key() == "Enter" && (e.ctrl_key() || e.meta_key()) && !text.get_untracked().trim().is_empty() {
                            e.prevent_default();
                            state.send(spec());
                            text.set(String::new());
                            open.set(false);
                        }
                    ></textarea>
                    <div class="mail-reply-foot">
                        <button type="button" class="text-button" on:click=move |_| { text.set(String::new()); open.set(false); }>"Cancel"</button>
                        <button type="button" class="primary-button mail-send" disabled=move || text.with(|t| t.trim().is_empty()) on:click=move |_| { state.send(spec()); text.set(String::new()); open.set(false); }>
                            <Icon name="paperPlane"/>"Send"
                        </button>
                    </div>
                }.into_any()
            }}
        </div>
    }.into_any()
}
