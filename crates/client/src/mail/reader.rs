//! Mail thread reader: calm split empty-state, action toolbar, List-Unsubscribe banner,
//! remote-image privacy bar (with "Always allow"), collapsible thread cards with per-message
//! actions, expandable header details, trimmed quote toggle (`···`), attachment preview & download,
//! raw `.eml` source viewer, and pinned inline reply dock.

use std::collections::{HashMap, HashSet};

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::compose::InlineReply;
use super::list::message_actions;
use super::model::{
    Addr, BASE, Body, MailState, Part, avatar_tone, encode, initials, parse_unsubscribe,
    render_html, split_quoted_text, time_label, tone,
};
use crate::api;
use crate::dates::local_date_time;
use crate::icons::Icon;
use crate::ui::{ActionMenu, Dialog, MenuAction, window};

#[derive(Clone, Debug, PartialEq)]
struct PreviewAttachment {
    message_id: String,
    part: Part,
}

#[component]
pub(crate) fn MailReader(state: MailState) -> impl IntoView {
    let expanded = RwSignal::new(HashSet::<String>::new());
    let bodies = RwSignal::new(HashMap::<String, Body>::new());
    let header_open = RwSignal::new(HashSet::<String>::new());
    let raw_source = RwSignal::new(None::<(String, String)>);
    let preview_att = RwSignal::new(None::<PreviewAttachment>);

    // When a new conversation opens, expand the opened message (and newest in thread if different).
    Effect::new(move |_| {
        let detail = state.detail.get();
        let mut init = HashSet::new();
        if let Some(d) = detail {
            init.insert(d.summary.id.clone());
            if let Some(last) = d.thread.last()
                && last.id != d.summary.id
            {
                init.insert(last.id.clone());
            }
        }
        expanded.set(init);
        bodies.set(HashMap::new());
        header_open.set(HashSet::new());
    });

    let load_body = move |id: String| {
        if bodies.with_untracked(|b| b.contains_key(&id))
            || state
                .detail
                .with_untracked(|d| d.as_ref().is_some_and(|d| d.summary.id == id))
        {
            return;
        }
        let sender_allowed = state.detail.with_untracked(|d| {
            d.as_ref()
                .and_then(|d| d.thread.iter().find(|m| m.id == id))
                .is_some_and(|m| state.is_sender_allowed(&m.from_address))
        });
        let images = state.images.get_untracked() == "load" || sender_allowed;
        spawn_local(async move {
            match api::get::<Body>(&format!(
                "{BASE}/messages/{}/body{}",
                encode(&id),
                if images { "?images=1" } else { "" }
            ))
            .await
            {
                Ok(b) => {
                    let _ = bodies.try_update(|m| {
                        m.insert(id, b);
                    });
                }
                Err(e) => state.fail(e.message),
            }
        });
    };

    // React to keyboard `;` expand/collapse-all trigger
    Effect::new(move |prev: Option<u32>| {
        let tick = state.expand_all_signal.get();
        if prev.is_some_and(|p| p != tick)
            && let Some(d) = state.detail.get_untracked()
        {
            let all_ids: Vec<String> = if d.thread.is_empty() {
                vec![d.summary.id.clone()]
            } else {
                d.thread.iter().map(|m| m.id.clone()).collect()
            };
            let any_folded = expanded.with_untracked(|e| all_ids.iter().any(|id| !e.contains(id)));
            if any_folded {
                for id in &all_ids {
                    load_body(id.clone());
                }
                expanded.set(all_ids.into_iter().collect());
            } else {
                let keep = d
                    .thread
                    .last()
                    .map(|m| m.id.clone())
                    .unwrap_or(d.summary.id);
                expanded.set(std::iter::once(keep).collect());
            }
        }
        tick
    });

    let show_images = move |id: String| {
        spawn_local(async move {
            match api::get::<Body>(&format!("{BASE}/messages/{}/body?images=1", encode(&id))).await
            {
                Ok(b) => {
                    let main = state
                        .detail
                        .with_untracked(|d| d.as_ref().is_some_and(|d| d.summary.id == id));
                    if main {
                        let _ = state.detail.try_update(|d| {
                            if let Some(d) = d {
                                d.body = b;
                            }
                        });
                    } else {
                        let _ = bodies.try_update(|m| {
                            m.insert(id, b);
                        });
                    }
                }
                Err(e) => state.fail(e.message),
            }
        });
    };

    let open_raw = move |id: String, name: String| {
        spawn_local(async move {
            let url = format!("{BASE}/messages/{}/raw", encode(&id));
            match web_sys::window()
                .map(|w| wasm_bindgen_futures::JsFuture::from(w.fetch_with_str(&url)))
            {
                Some(fut) => match fut.await {
                    Ok(resp) => {
                        let resp: web_sys::Response = wasm_bindgen::JsCast::unchecked_into(resp);
                        if let Ok(p) = resp.text()
                            && let Ok(txt) = wasm_bindgen_futures::JsFuture::from(p).await
                        {
                            let _ = raw_source
                                .try_set(Some((name, txt.as_string().unwrap_or_default())));
                        }
                    }
                    Err(_) => state.fail("Could not load raw source"),
                },
                None => state.fail("Could not load raw source"),
            }
        });
    };

    let copy_text = move |val: String, label: &'static str| {
        let cb = window().navigator().clipboard();
        if !cb.is_undefined() && !cb.is_null() {
            let p = cb.write_text(&val);
            spawn_local(async move {
                let _ = wasm_bindgen_futures::JsFuture::from(p).await;
                state.notify(label);
            });
        }
    };

    view! {
        <div class="mail-reader">
            {move || {
                // Split-view calm empty state when no conversation is selected
                if state.selected.get().is_none() {
                    let unread = state.overview.with(|o| o.as_ref().map(|o| o.counts.unread).unwrap_or(0));
                    let flagged = state.overview.with(|o| o.as_ref().map(|o| o.counts.flagged).unwrap_or(0));
                    return view! {
                        <div class="mail-reader-empty">
                            <div class="mail-reader-empty-icon"><Icon name="mail"/></div>
                            <h2>"Select a conversation"</h2>
                            <p>{format!("{unread} unread \u{00b7} {flagged} flagged across your accounts")}</p>
                            <div class="mail-reader-hints">
                                <span><kbd>"j"</kbd>" / "<kbd>"k"</kbd>" Navigate"</span>
                                <span><kbd>"c"</kbd>" Compose"</span>
                                <span><kbd>"e"</kbd>" Archive"</span>
                                <span><kbd>"?"</kbd>" All shortcuts"</span>
                            </div>
                        </div>
                    }.into_any();
                }

                if let Some(error) = Some(state.detail_error.get()).filter(|e| !e.is_empty()) {
                    return view! {
                        <div class="mail-reader-head">
                            <button type="button" class="icon-button mail-back" aria-label="Back to messages" on:click=move |_| state.close_reader()><Icon name="back"/></button>
                            <div class="mail-reader-tools">
                                <button type="button" class="icon-button" aria-label="Close message" on:click=move |_| state.close_reader()><Icon name="close"/></button>
                            </div>
                        </div>
                        <div class="mail-empty" role="alert">
                            <h2>"This message could not be opened"</h2>
                            <p>{error}</p>
                            <button class="secondary-button" on:click=move |_| { if let Some(id) = state.selected.get_untracked() { state.open(id); } }>"Retry"</button>
                        </div>
                    }.into_any();
                }

                let Some(d) = state.detail.get() else {
                    return view! { <p class="mail-empty" role="status">"Opening message\u{2026}"</p> }.into_any();
                };
                let account = state.account(&d.summary.account_id);
                let id = d.summary.id.clone();
                let flagged = d.summary.flagged;
                let seen = d.summary.seen;
                let in_trash = d.summary.mailbox_role == "trash"
                    || d.thread.iter().find(|t| t.id == id).is_some_and(|t| t.mailbox_role == "trash");
                let (id1, id2, id3, id4, id_raw, id_loc) = (
                    id.clone(),
                    id.clone(),
                    id.clone(),
                    id.clone(),
                    id.clone(),
                    id.clone(),
                );
                let file_name_raw = d.file_name.clone();
                let file_id_loc = d.summary.file_id.clone();
                let detail_menu = d.clone();
                let account_menu = account.clone();
                let crumb_tone = account.as_ref().map(|a| tone(a.color)).unwrap_or_default();
                let thread = if d.thread.is_empty() {
                    vec![d.summary.clone()]
                } else {
                    d.thread.clone()
                };
                let thread_len = thread.len();
                let unsubscribe = parse_unsubscribe(&d.body.list_unsubscribe);
                let reply_detail = d.clone();
                let file_name_display = if d.file_name.is_empty() {
                    "message.eml".to_owned()
                } else {
                    d.file_name.clone()
                };

                view! {
                    <div class="mail-reader-head">
                        <button type="button" class="icon-button mail-back" aria-label="Back to messages" on:click=move |_| state.close_reader()><Icon name="back"/></button>
                        <span class="mail-reader-crumb">
                            <span class=format!("mail-acct is-dot {crumb_tone}") aria-hidden="true"></span>
                            <span class="mail-crumb-text">{format!("{} \u{00b7} {}", d.account_name, d.mailbox_name)}</span>
                        </span>
                        <div class="mail-reader-tools">
                            <button type="button" class="icon-button" aria-label="Archive" title="Archive (e)" on:click=move |_| state.move_to(vec![id1.clone()], Some("archive"), None, "archived".into())><Icon name="archive"/></button>
                            <button type="button" class="icon-button" aria-label=if in_trash { "Delete for good" } else { "Delete" } title="Delete (#)" on:click=move |_| state.move_to(vec![id2.clone()], Some("trash"), None, "moved to Trash".into())><Icon name="trash"/></button>
                            <button type="button" class="icon-button" aria-label=if seen { "Mark as unread" } else { "Mark as read" } title=if seen { "Mark as unread (u)" } else { "Mark as read" } on:click=move |_| state.set_flags(vec![id3.clone()], Some(!seen), None, true)><Icon name="mailUnread"/></button>
                            <button type="button" class=if flagged { "icon-button is-flagged" } else { "icon-button" } aria-label=if flagged { "Remove flag" } else { "Flag" } aria-pressed=flagged.to_string() title="Flag (s)" on:click=move |_| state.set_flags(vec![id4.clone()], None, Some(!flagged), true)><Icon name="flag"/></button>
                            {(thread_len > 1).then(|| view! {
                                <button
                                    type="button"
                                    class="icon-button mail-desktop-only"
                                    aria-label="Expand or collapse all messages"
                                    title="Expand or collapse all (;)"
                                    on:click=move |_| state.expand_all_signal.update(|n| *n += 1)
                                >
                                    <Icon name="unfold"/>
                                </button>
                            })}
                            <button
                                type="button"
                                class="icon-button mail-desktop-only"
                                aria-label="Print conversation"
                                title="Print"
                                on:click=move |_| { let _ = window().print(); }
                            >
                                <Icon name="print"/>
                            </button>
                            <ActionMenu
                                label="More message actions"
                                actions=move |_| message_actions(
                                    state,
                                    vec![detail_menu.summary.id.clone()],
                                    vec![detail_menu.summary.mailbox_id.clone()],
                                    account_menu.clone(),
                                    Some(detail_menu.clone()),
                                    Some(Callback::new({
                                        let id_r = id_raw.clone();
                                        let fn_r = file_name_raw.clone();
                                        move |_| open_raw(id_r.clone(), fn_r.clone())
                                    })),
                                )
                            >
                                <Icon name="more"/>
                            </ActionMenu>
                            <button type="button" class="icon-button mail-close" aria-label="Close message" on:click=move |_| state.close_reader()><Icon name="close"/></button>
                        </div>
                    </div>

                    <div class="mail-thread">
                        <div class="mail-thread-header">
                            <h1 tabindex="-1">{d.summary.subject_or()}</h1>
                            <div class="mail-thread-meta">
                                <span>{format!("{thread_len} message{}", if thread_len == 1 { "" } else { "s" })}</span>
                                <span aria-hidden="true">"\u{00b7}"</span>
                                <button
                                    type="button"
                                    class="mail-meta-link"
                                    title=file_name_display.clone()
                                    on:click=move |_| {
                                        let id = id_loc.clone();
                                        let file = file_id_loc.clone();
                                        spawn_local(async move {
                                            match api::get::<serde_json::Value>(&format!("{BASE}/messages/{}/location", encode(&id))).await {
                                                Ok(v) => crate::navigation::navigate(&format!(
                                                    "/?folder={}&file={}",
                                                    encode(v["parentId"].as_str().unwrap_or_default()),
                                                    encode(v["fileId"].as_str().unwrap_or(&file))
                                                )),
                                                Err(e) => state.fail(e.message),
                                            }
                                        });
                                    }
                                >
                                    <Icon name="file"/>"Open .eml in Files"
                                </button>
                            </div>
                        </div>

                        {unsubscribe.map(|(url, is_http)| view! {
                            <div class="mail-unsub-strip" role="note">
                                <Icon name="bell"/>
                                <span>"This message is from a mailing list."</span>
                                {if is_http {
                                    view! { <a class="secondary-button mail-strip-btn" href=url target="_blank" rel="noopener noreferrer">"Unsubscribe"</a> }.into_any()
                                } else {
                                    view! { <a class="secondary-button mail-strip-btn" href=url>"Unsubscribe"</a> }.into_any()
                                }}
                            </div>
                        })}

                        {thread.into_iter().map(|m| {
                            let mid = m.id.clone();
                            let is_main = mid == d.summary.id;
                            let main_body = is_main.then(|| d.body.clone());
                            let (mid_toggle, mid_open, mid_img, mid_hdr_btn, mid_hdr_check) = (
                                mid.clone(),
                                mid.clone(),
                                mid.clone(),
                                mid.clone(),
                                mid.clone(),
                            );
                            let open = Memo::new(move |_| expanded.with(|e| e.contains(&mid_open)));
                            let hdr_expanded = Memo::new(move |_| header_open.with(|h| h.contains(&mid_hdr_check)));
                            let to_line = {
                                let to: Vec<String> = m.recipients.to.iter().chain(&m.recipients.cc).map(Addr::label).collect();
                                if to.is_empty() { String::new() } else { format!("to {}", to.join(", ")) }
                            };
                            let to_full = m.recipients.to.iter().map(|a| if a.name.is_empty() { a.address.clone() } else { format!("{} <{}>", a.name, a.address) }).collect::<Vec<_>>().join(", ");
                            let cc_full = m.recipients.cc.iter().map(|a| if a.name.is_empty() { a.address.clone() } else { format!("{} <{}>", a.name, a.address) }).collect::<Vec<_>>().join(", ");
                            let from_addr = m.from_address.clone();
                            let from_addr_copy = m.from_address.clone();
                            let from_addr_allow = m.from_address.clone();
                            let sender_display = m.sender();
                            let date_full = local_date_time(m.date, &[]);
                            let file_mirror = file_name_display.clone();
                            let m_for_reply = m.clone();
                            let m_for_all = m.clone();
                            let m_for_fwd = m.clone();
                            let m_for_snip = m.clone();
                            let mid_for_reply = mid.clone();
                            let mid_for_all = mid.clone();
                            let mid_for_fwd = mid.clone();
                            let main_body_r = main_body.clone();
                            let main_body_a = main_body.clone();
                            let main_body_f = main_body.clone();

                            // If initially expanded but not the main detail message, fetch its body automatically
                            let mid_autoload = mid.clone();
                            Effect::new(move |_| {
                                if open.get() && !is_main {
                                    load_body(mid_autoload.clone());
                                }
                            });

                            view! {
                                <article class=move || if open.get() { "mail-msg is-open" } else { "mail-msg is-folded" }>
                                    <div class="mail-msg-head-wrap">
                                        <button
                                            type="button"
                                            class="mail-msg-head"
                                            aria-expanded=move || open.get().to_string()
                                            on:click=move |_| {
                                                let id = mid_toggle.clone();
                                                let now = !expanded.with_untracked(|e| e.contains(&id));
                                                expanded.update(|e| {
                                                    if now { e.insert(id.clone()); } else { e.remove(&id); }
                                                });
                                                if now { load_body(id); }
                                            }
                                        >
                                            <span class=avatar_tone(&m.from_address) aria-hidden="true">{initials(&m.sender())}</span>
                                            <span class="mail-msg-from">
                                                <b>{m.sender()}</b>
                                                {(!m.from_address.is_empty() && !m.from_name.is_empty()).then(|| view! { <small>{m.from_address.clone()}</small> })}
                                            </span>
                                            <time class="mail-msg-date" title=date_full.clone()>{time_label(m.date)}</time>
                                            <span class="mail-msg-to">{move || if open.get() { to_line.clone() } else { m_for_snip.snippet.clone() }}</span>
                                        </button>

                                        {move || open.get().then(|| {
                                            let m_r = m_for_reply.clone();
                                            let m_a = m_for_all.clone();
                                            let m_f = m_for_fwd.clone();
                                            let mid_r = mid_for_reply.clone();
                                            let mid_a = mid_for_all.clone();
                                            let mid_f = mid_for_fwd.clone();
                                            let mb_r = main_body_r.clone();
                                            let mb_a = main_body_a.clone();
                                            let mb_f = main_body_f.clone();
                                            let mid_h = mid_hdr_btn.clone();
                                            view! {
                                                <div class="mail-msg-actions">
                                                    <button
                                                        type="button"
                                                        class="icon-button"
                                                        aria-label="Message details"
                                                        title="Message details"
                                                        aria-expanded=move || hdr_expanded.get().to_string()
                                                        on:click=move |e| {
                                                            e.stop_propagation();
                                                            header_open.update(|h| {
                                                                if !h.remove(&mid_h) { h.insert(mid_h.clone()); }
                                                            });
                                                        }
                                                    >
                                                        <Icon name="info"/>
                                                    </button>
                                                    <button
                                                        type="button"
                                                        class="icon-button"
                                                        aria-label="Reply to this message"
                                                        title="Reply"
                                                        on:click=move |e| {
                                                            e.stop_propagation();
                                                            let b = mb_r.clone().or_else(|| bodies.with_untracked(|map| map.get(&mid_r).cloned())).unwrap_or_default();
                                                            state.reply_to_message(&m_r, &b, "reply");
                                                        }
                                                    >
                                                        <Icon name="reply"/>
                                                    </button>
                                                    <ActionMenu
                                                        label="Actions for this message"
                                                        actions=move |_| {
                                                            let m_a = m_a.clone();
                                                            let m_f = m_f.clone();
                                                            let mid_a = mid_a.clone();
                                                            let mid_f = mid_f.clone();
                                                            let mb_a = mb_a.clone();
                                                            let mb_f = mb_f.clone();
                                                            vec![
                                                                MenuAction::new("Reply all", "replyAll").run(move || {
                                                                    let b = mb_a.clone().or_else(|| bodies.with_untracked(|map| map.get(&mid_a).cloned())).unwrap_or_default();
                                                                    state.reply_to_message(&m_a, &b, "all");
                                                                }),
                                                                MenuAction::new("Forward", "forward").run(move || {
                                                                    let b = mb_f.clone().or_else(|| bodies.with_untracked(|map| map.get(&mid_f).cloned())).unwrap_or_default();
                                                                    state.reply_to_message(&m_f, &b, "forward");
                                                                }),
                                                            ]
                                                        }
                                                    >
                                                        <Icon name="more"/>
                                                    </ActionMenu>
                                                </div>
                                            }
                                        })}
                                    </div>

                                    {move || (open.get() && hdr_expanded.get()).then(|| {
                                        let addr_copy = from_addr_copy.clone();
                                        let addr_val = from_addr.clone();
                                        let sender_val = sender_display.clone();
                                        view! {
                                            <dl class="mail-hdr-details">
                                                <dt>"From"</dt>
                                                <dd>
                                                    <span>{if addr_val.is_empty() { sender_val } else { format!("{sender_val} <{addr_val}>") }}</span>
                                                    {(!addr_copy.is_empty()).then(|| view! {
                                                        <button type="button" class="text-button mail-inline-action" on:click=move |_| copy_text(addr_copy.clone(), "Address copied")>"Copy"</button>
                                                    })}
                                                </dd>
                                                {(!to_full.is_empty()).then(|| view! { <dt>"To"</dt><dd>{to_full.clone()}</dd> })}
                                                {(!cc_full.is_empty()).then(|| view! { <dt>"Cc"</dt><dd>{cc_full.clone()}</dd> })}
                                                <dt>"Date"</dt><dd>{date_full.clone()}</dd>
                                                <dt>"Mirror"</dt><dd><code>{file_mirror.clone()}</code></dd>
                                            </dl>
                                        }
                                    })}

                                    {move || open.get().then(|| {
                                        let body = main_body.clone().or_else(|| bodies.with(|b| b.get(&mid).cloned()));
                                        let mid_img = mid_img.clone();
                                        let mid_allow = mid_img.clone();
                                        let allow_addr = from_addr_allow.clone();
                                        match body {
                                            None => view! { <p class="mail-msg-loading" role="status">"Loading\u{2026}"</p> }.into_any(),
                                            Some(b) => view! {
                                                <MessageBody
                                                    id=mid.clone()
                                                    body=b
                                                    sender_address=allow_addr
                                                    on_images=Callback::new(move |_| show_images(mid_img.clone()))
                                                    on_always_allow=Callback::new(move |addr: String| {
                                                        state.allow_sender(addr);
                                                        show_images(mid_allow.clone());
                                                    })
                                                    on_preview=Callback::new(move |p: PreviewAttachment| preview_att.set(Some(p)))
                                                />
                                            }.into_any(),
                                        }
                                    })}
                                </article>
                            }
                        }).collect_view()}
                    </div>

                    // Pinned sticky Inline Reply dock at bottom of reader sheet
                    <div class="mail-reply-dock">
                        <InlineReply state=state detail=reply_detail/>
                    </div>
                }.into_any()
            }}

            // Attachment preview modal (images)
            {move || preview_att.get().map(|att| {
                let src = format!("{BASE}/messages/{}/attachments/{}?inline=1", encode(&att.message_id), att.part.index);
                let dl = format!("{BASE}/messages/{}/attachments/{}", encode(&att.message_id), att.part.index);
                let name = att.part.name.clone();
                view! {
                    <Dialog title=name.clone() on_close=Callback::new(move |_| preview_att.set(None))>
                        <div class="mail-att-preview">
                            <img src=src.clone() alt=name.clone()/>
                        </div>
                        <div class="dialog-actions">
                            <a class="secondary-button" href=dl.clone() download=name.clone()><Icon name="download"/>"Download"</a>
                            <button type="button" class="primary-button" on:click=move |_| preview_att.set(None)>"Close"</button>
                        </div>
                    </Dialog>
                }
            })}

            // Raw .eml source viewer modal
            {move || raw_source.get().map(|(name, source)| view! {
                <Dialog title=format!("Source \u{00b7} {name}") on_close=Callback::new(move |_| raw_source.set(None))>
                    <pre class="mail-raw-source">{source.clone()}</pre>
                    <div class="dialog-actions">
                        <button type="button" class="primary-button" on:click=move |_| raw_source.set(None)>"Close"</button>
                    </div>
                </Dialog>
            })}
        </div>
    }
}

#[component]
fn MessageBody(
    id: String,
    body: Body,
    sender_address: String,
    on_images: Callback<()>,
    on_always_allow: Callback<String>,
    on_preview: Callback<PreviewAttachment>,
) -> impl IntoView {
    let html_ref = NodeRef::<html::Div>::new();
    let html = body.html.clone();
    let show_quoted = RwSignal::new(false);

    Effect::new(move |_| {
        let Some(el) = html_ref.get() else { return };
        while let Some(first) = el.first_child() {
            let _ = el.remove_child(&first);
        }
        let _ = el.append_child(&render_html(&html));
    });

    let attachments: Vec<Part> = body
        .attachments
        .iter()
        .filter(|a| !a.inline)
        .cloned()
        .collect();
    let (main_text, quoted_text) = if body.html.is_empty() {
        split_quoted_text(&body.text)
    } else {
        (String::new(), None)
    };

    view! {
        {(body.blocked_images > 0).then(|| {
            let allow_addr = sender_address.clone();
            let has_addr = !allow_addr.is_empty();
            view! {
                <div class="mail-images-blocked" role="note">
                    <Icon name="shield"/>
                    <span>{format!("{} remote image{} blocked", body.blocked_images, if body.blocked_images == 1 { "" } else { "s" })}</span>
                    <button type="button" class="secondary-button mail-strip-btn" on:click=move |_| { on_images.try_run(()); }>"Show images"</button>
                    {has_addr.then(|| view! {
                        <button type="button" class="text-button mail-strip-btn" on:click=move |_| { on_always_allow.try_run(allow_addr.clone()); }>"Always allow"</button>
                    })}
                </div>
            }
        })}
        {if body.html.is_empty() {
            view! {
                <div class="mail-body mail-text">
                    {main_text}
                    {quoted_text.map(|q| view! {
                        <div class="mail-quoted-wrap">
                            <button
                                type="button"
                                class="mail-quote-toggle"
                                aria-label="Toggle quoted text"
                                aria-expanded=move || show_quoted.get().to_string()
                                title="Show or hide quoted text"
                                on:click=move |_| show_quoted.update(|v| *v = !*v)
                            >
                                "\u{00b7}\u{00b7}\u{00b7}"
                            </button>
                            <Show when=move || show_quoted.get()>
                                <blockquote class="mail-quoted-block">{q.clone()}</blockquote>
                            </Show>
                        </div>
                    })}
                </div>
            }.into_any()
        } else {
            view! { <div node_ref=html_ref class="mail-body"></div> }.into_any()
        }}
        {(!attachments.is_empty()).then(|| view! {
            <ul class="mail-attachments" aria-label="Attachments">
                {attachments.into_iter().map(|a| {
                    let href = format!("{BASE}/messages/{}/attachments/{}", encode(&id), a.index);
                    let is_image = matches!(a.kind.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif");
                    let kind = if a.kind.starts_with("image/") { "image" } else if a.kind == "application/pdf" { "pdf" } else { "file" };
                    let prev_id = id.clone();
                    let prev_part = a.clone();
                    view! {
                        <li class="mail-attachment">
                            <span class=format!("mail-attachment-icon is-{kind}") aria-hidden="true"><Icon name=kind/></span>
                            <span class="mail-attachment-name"><b>{a.name.clone()}</b><small>{api::format_bytes(a.size)}</small></span>
                            {is_image.then(|| view! {
                                <button
                                    type="button"
                                    class="icon-button"
                                    aria-label=format!("Preview {}", a.name)
                                    title="Preview"
                                    on:click=move |_| { on_preview.try_run(PreviewAttachment { message_id: prev_id.clone(), part: prev_part.clone() }); }
                                >
                                    <Icon name="eye"/>
                                </button>
                            })}
                            <a class="icon-button" href=href download=a.name.clone() aria-label=format!("Download {}", a.name) title="Download"><Icon name="download"/></a>
                        </li>
                    }
                }).collect_view()}
            </ul>
        })}
    }
}
