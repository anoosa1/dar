//! Writing mail: the floating compose sheet (bottom right, the list stays usable behind it),
//! recipient chips, attachments, drafts on the server, and the inline reply at the end of a
//! conversation (direction B's reply in direction A's reader).

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use super::workspace::{MailState, day_label};
use super::*;
use crate::api;
use crate::dates::local_time;
use crate::icons::Icon;
use crate::markdown::{Markdown, MarkdownToolbar, format_markdown};
use crate::ui::unique_id;

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
    account.map(|a| a.signature.trim()).filter(|s| !s.is_empty()).map(|s| format!("\n\n-- \n{s}")).unwrap_or_default()
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
        ComposeSpec { account_id: account.id.clone(), text: signature_block(Some(account)), ..Default::default() }
    }

    pub fn reply(d: &Detail, mode: &str, account: Option<&Account>) -> Self {
        let own = account.map(|a| a.address.to_lowercase()).unwrap_or_default();
        let s = &d.summary;
        let prefix = if mode == "forward" { "Fwd: " } else { "Re: " };
        let lower = s.subject.to_lowercase();
        let subject = if lower.starts_with(prefix.to_lowercase().as_str()) || (mode != "forward" && lower.starts_with("re:")) { s.subject.clone() } else { format!("{prefix}{}", s.subject) };
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
                    if lower != own && !to.iter().any(|t| t.to_lowercase() == lower) && !cc.iter().any(|t| t.to_lowercase() == lower) {
                        cc.push(a.address.clone());
                    }
                }
            }
        }
        let when = format!("{}, {}", day_label(s.date), local_time(s.date, &[("hour", "numeric"), ("minute", "2-digit")]));
        let original = plain(&d.body);
        let text = if mode == "forward" {
            let to_line: Vec<String> = s.recipients.to.iter().map(Addr::label).collect();
            format!("{}\n\n---------- Forwarded message ----------\nFrom: {} <{}>\nDate: {when}\nSubject: {}\nTo: {}\n\n{}", signature_block(account), s.sender(), s.from_address, s.subject, to_line.join(", "), original.trim())
        } else {
            let quoted: Vec<String> = original.trim().lines().map(|l| if l.is_empty() { ">".into() } else { format!("> {l}") }).collect();
            format!("{}\n\nOn {when}, {} wrote:\n{}", signature_block(account), s.sender(), quoted.join("\n"))
        };
        ComposeSpec {
            account_id: s.account_id.clone(), to, cc, subject, text,
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
            text: if d.body.format == "markdown" { d.body.text_part.clone() } else { plain(&d.body) },
            markdown: d.body.format == "markdown",
            replaces: Some(s.id.clone()),
            ..Default::default()
        }
    }

    pub fn body(&self) -> Value {
        json!({
            "accountId": self.account_id, "to": self.to, "cc": self.cc, "bcc": self.bcc, "subject": self.subject, "text": self.text,
            "inReplyTo": self.in_reply_to, "replaces": self.replaces, "format": if self.markdown { "markdown" } else { "plain" },
            "attachments": self.attachments.iter().map(|a| json!({ "name": a.name, "type": a.kind, "data": a.data })).collect::<Vec<_>>(),
        })
    }

    fn has_content(&self) -> bool {
        !self.to.is_empty() || !self.cc.is_empty() || !self.subject.trim().is_empty() || !self.attachments.is_empty() || !self.text.trim().is_empty() && self.text.trim() != "--"
    }
}

fn valid_address(a: &str) -> bool {
    let a = a.trim();
    a.len() >= 3 && a.contains('@') && !a.starts_with('@') && !a.ends_with('@') && !a.contains(char::is_whitespace) && !a.contains(['<', '>', ',', ';'])
}

/// Addresses as chips: Enter, comma or leaving the field makes a chip; Backspace on an empty
/// field removes the last one.
#[component]
fn ChipInput(#[prop(into)] label: String, value: RwSignal<Vec<String>>, #[prop(optional)] autofocus: bool) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let id = unique_id();
    let input = NodeRef::<html::Input>::new();
    let commit = move || {
        let raw = text.get_untracked();
        let parts: Vec<String> = raw.split([',', ';']).map(|p| {
            let p = p.trim();
            // "Name <address>" keeps the address.
            match (p.rfind('<'), p.rfind('>')) {
                (Some(a), Some(b)) if a < b => p[a + 1..b].trim().to_owned(),
                _ => p.to_owned(),
            }
        }).filter(|p| !p.is_empty()).collect();
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
    };
    Effect::new(move |_| {
        if autofocus
            && let Some(i) = input.get()
        {
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
                <input node_ref=input id=id type="email" multiple autocomplete="off" prop:value=move || text.get()
                    on:input=move |e| { let v = event_target_value(&e); text.set(v.clone()); if v.ends_with(',') || v.ends_with(';') { commit(); } }
                    on:blur=move |_| commit()
                    on:keydown=move |e| {
                        match e.key().as_str() {
                            "Enter" if !text.get_untracked().trim().is_empty() => { e.prevent_default(); commit(); }
                            "Backspace" if text.get_untracked().is_empty() => { value.update(|v| { v.pop(); }); }
                            _ => {}
                        }
                    }/>
            </div>
        </div>
    }
}

async fn read_file(file: web_sys::File) -> Option<Upload> {
    let buffer = JsFuture::from(file.array_buffer()).await.ok()?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    Some(Upload { name: file.name(), kind: if file.type_().is_empty() { "application/octet-stream".into() } else { file.type_() }, size: bytes.len() as f64, data: base64(&bytes) })
}

#[component]
pub(crate) fn ComposeSheet(state: MailState, spec: ComposeSpec) -> impl IntoView {
    let accounts = state.overview.with_untracked(|o| o.as_ref().map(|o| o.accounts.clone()).unwrap_or_default());
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
    let body_field = NodeRef::<html::Textarea>::new();
    let busy = RwSignal::new(false);
    let replaces = StoredValue::new(spec.replaces.clone());
    let in_reply_to = spec.in_reply_to.clone();
    let start = StoredValue::new(spec.clone());
    let heading = if spec.replaces.is_some() { "Draft".to_owned() } else if spec.subject.is_empty() { "New message".to_owned() } else { spec.subject.clone() };
    let current = move || ComposeSpec {
        account_id: account.get_untracked(), to: to.get_untracked(), cc: cc.get_untracked(), bcc: bcc.get_untracked(), subject: subject.get_untracked(),
        text: text.get_untracked(), in_reply_to: in_reply_to.clone(), replaces: replaces.get_value(), attachments: attachments.get_untracked(), error: String::new(),
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
            }
            return;
        }
        busy.set(true);
        status.set("Saving draft\u{2026}".into());
        let csrf = state.csrf();
        spawn_local(async move {
            match api::send::<Value>(&format!("{BASE}/drafts"), "POST", Some(spec.body()), &csrf).await {
                Ok(v) => {
                    replaces.set_value(v["id"].as_str().map(str::to_owned));
                    start.set_value(current.with_value(|c| c()));
                    let _ = status.try_set("Draft saved".into());
                    let _ = busy.try_set(false);
                    if then_close {
                        let _ = state.compose.try_set(None);
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
    // The Notes formatting toolbar, applied to the selection.
    let format = Callback::new(move |(before, after): (&'static str, &'static str)| {
        let Some(field) = body_field.get_untracked() else { return };
        let (start, end) = (field.selection_start().ok().flatten().unwrap_or(0), field.selection_end().ok().flatten().unwrap_or(0));
        let (next, s, e) = format_markdown(&text.get_untracked(), start, end, before, after);
        text.set(next);
        markdown.set(true);
        let field = send_wrapper::SendWrapper::new(field);
        request_animation_frame(move || {
            let _ = field.focus();
            let _ = field.set_selection_range(s, e);
        });
    });
    let file_input = NodeRef::<html::Input>::new();
    let on_files = move |e: leptos::ev::Event| {
        let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) else { return };
        let Some(files) = input.files() else { return };
        let list: Vec<web_sys::File> = (0..files.length()).filter_map(|i| files.get(i)).collect();
        input.set_value("");
        spawn_local(async move {
            for f in list {
                let total: f64 = attachments.get_untracked().iter().map(|a| a.size).sum();
                if total + f.size() > 25.0 * 1024.0 * 1024.0 {
                    let _ = error.try_set(format!("{} would take the attachments over 25 MB.", f.name()));
                    continue;
                }
                if let Some(u) = read_file(f).await {
                    let _ = attachments.try_update(|a| a.push(u));
                }
            }
        });
    };
    let sheet = NodeRef::<html::Section>::new();
    let to_first = spec.to.is_empty();
    Effect::new(move |_| {
        if let Some(s) = sheet.get()
            && let Some(el) = s.query_selector(if to_first { ".mail-chips input" } else { "textarea" }).ok().flatten().and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = el.focus();
            if let Ok(t) = el.clone().dyn_into::<web_sys::HtmlTextAreaElement>() {
                t.set_selection_range(0, 0).ok();
            }
        }
    });
    let from_options = accounts.clone();
    view! {
        <section node_ref=sheet class=move || if expanded.get() { "mail-compose is-expanded" } else { "mail-compose" } role="dialog" aria-modal="false" aria-label="Compose message"
            on:keydown=move |e| {
                if e.key() == "Escape" && !e.default_prevented() { e.prevent_default(); save_draft(true); }
                if e.key() == "Enter" && (e.ctrl_key() || e.meta_key()) { e.prevent_default(); send(); }
            }>
            <div class="mail-compose-head">
                <h2>{heading}</h2>
                <button type="button" class="icon-button" aria-label=move || if expanded.get() { "Shrink" } else { "Expand" } on:click=move |_| expanded.update(|v| *v = !*v)><Icon name="expand"/></button>
                <button type="button" class="icon-button" aria-label="Close (saves a draft)" title="Close (saves a draft)" on:click=move |_| save_draft(true)><Icon name="close"/></button>
            </div>
            <div class="mail-compose-fields">
                <div class="mail-field">
                    <label for="mail-from">"From"</label>
                    <select id="mail-from" prop:value=move || account.get() on:change=move |e| account.set(event_target_value(&e))>
                        {from_options.into_iter().map(|a| {
                            let label = if a.display_name.is_empty() { a.address.clone() } else { format!("{} <{}>", a.display_name, a.address) };
                            view! { <option value=a.id.clone() selected=a.id == spec.account_id>{label}</option> }
                        }).collect_view()}
                    </select>
                </div>
                <ChipInput label="To" value=to autofocus=to_first/>
                {move || (!show_cc.get()).then(|| view! { <button type="button" class="text-button mail-cc-toggle" on:click=move |_| show_cc.set(true)>"Cc / Bcc"</button> })}
                <Show when=move || show_cc.get()>
                    <ChipInput label="Cc" value=cc/>
                    <ChipInput label="Bcc" value=bcc/>
                </Show>
                <div class="mail-field">
                    <label for="mail-subject">"Subject"</label>
                    <input id="mail-subject" maxlength="900" prop:value=move || subject.get() on:input=move |e| subject.set(event_target_value(&e))/>
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
                view! { <textarea node_ref=body_field class="mail-compose-body" aria-label="Message" spellcheck="true" prop:value=move || text.get() on:input=move |e| text.set(event_target_value(&e))></textarea> }.into_any()
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
                <label class="icon-button mail-attach" title="Attach files"><Icon name="link"/><span class="sr-only">"Attach files"</span><input node_ref=file_input type="file" multiple on:change=on_files/></label>
                <span class="mail-compose-status" role="status">{move || status.get()}</span>
                <button type="button" class="text-button" disabled=move || busy.get() on:click=move |_| save_draft(false)>"Save draft"</button>
                <button type="button" class="icon-button" aria-label="Discard" title="Discard" on:click=move |_| state.compose.set(None)><Icon name="trash"/></button>
                <button type="button" class="primary-button mail-send" title="Send (Ctrl Enter)" on:click=move |_| send()><Icon name="send"/>"Send"</button>
            </div>
        </section>
    }
}

/// Reply at the end of the conversation: Reply, Reply all or Forward, written in place, sent
/// with the same undo as the sheet; Expand moves it to the compose sheet.
#[component]
pub(crate) fn InlineReply(state: MailState, detail: Detail) -> impl IntoView {
    if detail.summary.draft {
        let d = detail.clone();
        return view! {
            <div class="mail-inline-reply is-draft">
                <Icon name="pencil"/><span>"This is a draft."</span>
                <button type="button" class="primary-button" on:click=move |_| state.compose.set(Some(ComposeSpec::draft(&d)))>"Continue editing"</button>
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
        let mut s = detail.with_value(|d| account.with_value(|a| ComposeSpec::reply(d, mode.get_untracked(), a.as_ref())));
        let written = text.get_untracked();
        if !written.trim().is_empty() {
            s.text = format!("{}{}", written.trim_end(), s.text);
        }
        s
    };
    let recipients = Memo::new(move |_| {
        let m = mode.get();
        detail.with_value(|d| account.with_value(|a| {
            let s = ComposeSpec::reply(d, m, a.as_ref());
            s.to.iter().chain(&s.cc).cloned().collect::<Vec<_>>().join(", ")
        }))
    });
    let area = NodeRef::<html::Textarea>::new();
    view! {
        <div class=move || if open.get() { "mail-inline-reply is-open" } else { "mail-inline-reply" }>
            {move || if !open.get() {
                view! {
                    <button type="button" class="mail-reply-start" on:click=move |_| { mode.set("reply"); open.set(true); }><Icon name="reply"/>{move || format!("Reply to {}", detail.with_value(|d| d.summary.sender()))}</button>
                    {(all_count > 1).then(|| view! { <button type="button" class="secondary-button" on:click=move |_| { mode.set("all"); open.set(true); }><Icon name="reply"/>"Reply all"</button> })}
                    <button type="button" class="secondary-button" on:click=move |_| state.compose.set(Some(detail.with_value(|d| account.with_value(|a| ComposeSpec::reply(d, "forward", a.as_ref())))))><Icon name="share"/>"Forward"</button>
                }.into_any()
            } else {
                view! {
                    <div class="mail-reply-to">
                        <select aria-label="Reply kind" prop:value=move || mode.get() on:change=move |e| mode.set(if event_target_value(&e) == "all" { "all" } else { "reply" })>
                            <option value="reply">"Reply"</option>
                            <option value="all">"Reply all"</option>
                        </select>
                        <span class="mail-reply-recipients">{move || format!("to {}", recipients.get())}</span>
                    </div>
                    <textarea node_ref=area class="mail-reply-text" aria-label="Reply" autofocus prop:value=move || text.get() on:input=move |e| text.set(event_target_value(&e))
                        on:keydown=move |e| if e.key() == "Enter" && (e.ctrl_key() || e.meta_key()) && !text.get_untracked().trim().is_empty() { e.prevent_default(); state.send(spec()); }></textarea>
                    <div class="mail-reply-foot">
                        <button type="button" class="icon-button" aria-label="Open in the compose sheet" title="Open in the compose sheet" on:click=move |_| { state.compose.set(Some(spec())); text.set(String::new()); open.set(false); }><Icon name="expand"/></button>
                        <button type="button" class="text-button" on:click=move |_| { text.set(String::new()); open.set(false); }>"Cancel"</button>
                        <button type="button" class="primary-button mail-send" disabled=move || text.with(|t| t.trim().is_empty()) on:click=move |_| { state.send(spec()); text.set(String::new()); open.set(false); }><Icon name="send"/>"Send"</button>
                    </div>
                }.into_any()
            }}
        </div>
    }.into_any()
}
