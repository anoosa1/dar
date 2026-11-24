//! Contact details inspector: photo portrait, quick actions, one-click copy on every field,
//! interactive group chips, vCard QR modal trigger, clone, Dar cross-app links, export & delete.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

use super::model::{
    BASE, capitalized, contact_groups, display_name, format_date, photo_url, s, work_line,
};
use crate::icons::Icon;
use crate::pim::presentation::contact_href;
use crate::pim::{Collection, CollectionDot, Item, avatar_hue, initials};
use crate::ui::window;

fn copy_to_clipboard(value: String, copied: RwSignal<Option<String>>, key: String) {
    let clipboard = window().navigator().clipboard();
    if !clipboard.is_undefined() && !clipboard.is_null() {
        let promise = clipboard.write_text(&value);
        spawn_local(async move {
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
            copied.try_set(Some(key.clone()));
            crate::sleep(1600).await;
            if copied.try_get_untracked().flatten().as_deref() == Some(key.as_str()) {
                copied.try_set(None);
            }
        });
    }
}

#[component]
pub fn ContactDetails(
    item: Item,
    book: Option<Collection>,
    all_groups: Signal<Vec<String>>,
    name_order: Signal<String>,
    writable: bool,
    busy: RwSignal<bool>,
    on_close: Callback<()>,
    on_edit: Callback<()>,
    on_toggle_favorite: Callback<Item>,
    on_clone: Callback<Item>,
    on_show_qr: Callback<Item>,
    on_delete: Callback<()>,
    on_filter_group: Callback<String>,
    on_update_groups: Callback<(Item, Vec<String>)>,
) -> impl IntoView {
    let copied = RwSignal::new(None::<String>);
    let adding_group = RwSignal::new(false);
    let new_group_text = RwSignal::new(String::new());

    let data = item.data.clone();
    let name = display_name(&data, &name_order.get_untracked());
    let nickname = s(&data, "nickname");
    let pronouns = s(&data, "pronouns");
    let nick_line = [
        (!nickname.is_empty()).then(|| format!("\"{nickname}\"")),
        (!pronouns.is_empty()).then_some(pronouns),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let work = work_line(&data);
    let first = |key: &str| data[key][0]["value"].as_str().map(str::to_owned);
    let email = first("emails");
    let phone = first("phones").filter(|p| contact_href("Phone", p).is_some());
    let favorite = data["favorite"] == json!(true);
    let has_photo = data["hasPhoto"] == json!(true);
    let p_url = photo_url(&item.collection_id, &item.href, item.revision);

    let list = |key: &str| data[key].as_array().cloned().unwrap_or_default();
    let socials = list("socials");
    let related = list("related");
    let born = s(&data, "birthday");
    let anniversary = s(&data, "anniversary");
    let has_card = !list("emails").is_empty()
        || !list("phones").is_empty()
        || !list("urls").is_empty()
        || !list("addresses").is_empty()
        || !socials.is_empty()
        || !related.is_empty()
        || !born.is_empty()
        || !anniversary.is_empty();
    let groups = contact_groups(&data);
    let notes = s(&data, "notes");

    let item_fav = item.clone();
    let item_clone = item.clone();
    let item_qr = item.clone();
    let item_qr_qa = item.clone();
    let item_grp_rm = StoredValue::new(item.clone());
    let item_grp_add = StoredValue::new(item.clone());
    let current_groups = StoredValue::new(groups.clone());

    let values = [
        ("Email", "mail", list("emails")),
        ("Phone", "phone", list("phones")),
        ("Website", "globe", list("urls")),
    ]
    .into_iter()
    .flat_map(|(label, icon, vals)| {
        vals.into_iter().enumerate().map(move |(idx, v)| {
            let value = v["value"].as_str().unwrap_or_default().to_owned();
            let tag = v["label"]
                .as_str()
                .filter(|l| !l.is_empty())
                .map(capitalized)
                .unwrap_or_else(|| label.to_owned());
            let link = contact_href(label, &value);
            let site = label == "Website";
            let copy_val = value.clone();
            let copy_key = format!("{label}:{idx}:{value}");
            let copy_key_check = copy_key.clone();
            let copy_label = format!("Copy {value}");
            view! {
                <div class="property contacts-prop">
                    <Icon name=icon/>
                    <div class="contacts-prop-body">
                        {match link {
                            Some(href) => view! {
                                <a
                                    href=href
                                    target=site.then_some("_blank")
                                    rel=site.then_some("noopener noreferrer")
                                    class=(label == "Phone").then_some("num")
                                >{value.clone()}</a>
                            }.into_any(),
                            None => view! { <span>{value.clone()}</span> }.into_any(),
                        }}
                        <small>{tag}</small>
                    </div>
                    <button
                        type="button"
                        class=move || if copied.get().as_deref() == Some(copy_key_check.as_str()) { "icon-button contacts-copy-btn is-copied" } else { "icon-button contacts-copy-btn" }
                        aria-label=copy_label
                        title="Copy"
                        on:click=move |_| copy_to_clipboard(copy_val.clone(), copied, copy_key.clone())
                    >
                        <Icon name=Signal::derive({
                            let k = copy_key.clone();
                            move || if copied.get().as_deref() == Some(k.as_str()) { "check" } else { "copy" }
                        })/>
                    </button>
                </div>
            }
        })
    })
    .collect_view();

    let social_rows = socials
        .into_iter()
        .enumerate()
        .map(|(idx, v)| {
            let service = capitalized(v["service"].as_str().unwrap_or("social"));
            let value = v["value"].as_str().unwrap_or_default().to_owned();
            let link = contact_href("Website", &value).filter(|_| value.contains('.'));
            let copy_val = value.clone();
            let copy_key = format!("social:{idx}:{value}");
            let copy_key_check = copy_key.clone();
            view! {
                <div class="property contacts-prop">
                    <Icon name="link"/>
                    <div class="contacts-prop-body">
                        {match link {
                            Some(href) => view! { <a href=href target="_blank" rel="noopener noreferrer">{value.clone()}</a> }.into_any(),
                            None => view! { <span>{value.clone()}</span> }.into_any(),
                        }}
                        <small>{service}</small>
                    </div>
                    <button
                        type="button"
                        class=move || if copied.get().as_deref() == Some(copy_key_check.as_str()) { "icon-button contacts-copy-btn is-copied" } else { "icon-button contacts-copy-btn" }
                        aria-label=format!("Copy {value}")
                        title="Copy"
                        on:click=move |_| copy_to_clipboard(copy_val.clone(), copied, copy_key.clone())
                    >
                        <Icon name="copy"/>
                    </button>
                </div>
            }
        })
        .collect_view();

    let addresses = list("addresses")
        .into_iter()
        .enumerate()
        .map(|(idx, a)| {
            let get = |k: &str| a[k].as_str().unwrap_or_default().to_owned();
            let first_line = [
                (!get("poBox").is_empty()).then(|| format!("PO Box {}", get("poBox"))),
                Some(get("extended")),
                Some(get("street")),
            ]
            .into_iter()
            .flatten()
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
            let second_line = ["city", "region", "postalCode", "country"]
                .iter()
                .map(|k| get(k))
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            let label = get("label");
            let full = [first_line.clone(), second_line.clone()]
                .into_iter()
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            let copy_key = format!("addr:{idx}");
            let copy_key_check = copy_key.clone();
            view! {
                <div class="property contacts-prop">
                    <Icon name="location"/>
                    <div class="contacts-prop-body">
                        {(!first_line.is_empty()).then(|| view! { <span>{first_line}</span> })}
                        {(!second_line.is_empty()).then(|| view! { <span>{second_line}</span> })}
                        <small>{if label.is_empty() { "Address".to_owned() } else { format!("{} address", capitalized(&label)) }}</small>
                    </div>
                    {(!full.is_empty()).then(|| view! {
                        <button
                            type="button"
                            class=move || if copied.get().as_deref() == Some(copy_key_check.as_str()) { "icon-button contacts-copy-btn is-copied" } else { "icon-button contacts-copy-btn" }
                            aria-label="Copy address"
                            title="Copy address"
                            on:click=move |_| copy_to_clipboard(full.clone(), copied, copy_key.clone())
                        >
                            <Icon name="copy"/>
                        </button>
                    })}
                </div>
            }
        })
        .collect_view();

    let related_rows = related
        .into_iter()
        .map(|r| {
            let rel = capitalized(r["relation"].as_str().unwrap_or("related"));
            let val = r["value"].as_str().unwrap_or_default().to_owned();
            view! {
                <div class="property contacts-prop">
                    <Icon name="user"/>
                    <div class="contacts-prop-body">
                        <span>{val}</span>
                        <small>{rel}</small>
                    </div>
                </div>
            }
        })
        .collect_view();

    let add_group_submit = move |name: String| {
        let trimmed = name.trim().to_owned();
        if trimmed.is_empty() {
            return;
        }
        let mut next = current_groups.get_value();
        if !next.contains(&trimmed) {
            next.push(trimmed);
            on_update_groups.run((item_grp_add.get_value(), next));
        }
        new_group_text.set(String::new());
        adding_group.set(false);
    };

    let has_dar_links = email.is_some() || phone.is_some();

    view! {
        <div class="pim-detail-top">
            <button
                type="button"
                class="text-button contacts-detail-back"
                aria-label="Back to contacts"
                on:click=move |_| on_close.run(())
            >
                <Icon name="back"/>"Back"
            </button>
            <span class="contacts-detail-label">"Contact"</span>
            <button
                type="button"
                class="icon-button"
                aria-label="Show vCard QR code"
                title="Show vCard QR code"
                on:click=move |_| on_show_qr.run(item_qr.clone())
            >
                <Icon name="qr"/>
            </button>
            {writable.then(|| view! {
                <button
                    type="button"
                    class="icon-button"
                    aria-label="Clone contact"
                    title="Clone contact"
                    disabled=move || busy.get()
                    on:click=move |_| on_clone.run(item_clone.clone())
                >
                    <Icon name="copy"/>
                </button>
            })}
            <button
                type="button"
                class="icon-button"
                aria-label="Close contact details"
                title="Close contact details"
                on:click=move |_| on_close.run(())
            >
                <Icon name="close"/>
            </button>
        </div>

        <div class="pim-hero">
            <span class=format!("contact-portrait {}", avatar_hue(&name)) aria-hidden="true">
                {if has_photo {
                    view! { <img src=p_url alt="" class="contact-avatar-img"/> }.into_any()
                } else {
                    view! { {initials(&name)} }.into_any()
                }}
            </span>
            <h2>{name.clone()}</h2>
            {(!nick_line.is_empty()).then(|| view! { <p class="muted contacts-nick-line">{nick_line}</p> })}
            {(!work.is_empty()).then(|| view! { <p class="muted">{work}</p> })}
        </div>

        <div class="quick-actions">
            {email.clone().map(|e| view! {
                <a class="quick-action" href=contact_href("Email", &e).unwrap_or_default() aria-label=format!("Email {e}")>
                    <span><Icon name="mail"/></span>"Email"
                </a>
            })}
            {phone.clone().map(|p| view! {
                <a class="quick-action" href=contact_href("Phone", &p).unwrap_or_default() aria-label=format!("Call {p}")>
                    <span><Icon name="phone"/></span>"Call"
                </a>
            })}
            {writable.then(|| view! {
                <button
                    type="button"
                    class="quick-action"
                    aria-pressed=favorite.to_string()
                    aria-label=if favorite { "Remove favorite" } else { "Add favorite" }
                    disabled=move || busy.get()
                    on:click=move |_| on_toggle_favorite.run(item_fav.clone())
                >
                    <span><Icon name="star"/></span>{if favorite { "Starred" } else { "Favorite" }}
                </button>
                <button
                    type="button"
                    class="quick-action"
                    aria-label="Edit contact"
                    on:click=move |_| on_edit.run(())
                >
                    <span><Icon name="pencil"/></span>"Edit"
                </button>
            })}
            <button
                type="button"
                class="quick-action"
                aria-label="Share QR code"
                on:click=move |_| on_show_qr.run(item_qr_qa.clone())
            >
                <span><Icon name="qr"/></span>"QR"
            </button>
        </div>

        {has_card.then(|| view! {
            <div class="property-card">
                {values}
                {social_rows}
                {addresses}
                {(!born.is_empty()).then(|| view! {
                    <div class="property contacts-prop">
                        <Icon name="cake"/>
                        <div class="contacts-prop-body">
                            <span>{format_date(&born)}</span>
                            <small>"Birthday"</small>
                        </div>
                    </div>
                })}
                {(!anniversary.is_empty()).then(|| view! {
                    <div class="property contacts-prop">
                        <Icon name="calendar"/>
                        <div class="contacts-prop-body">
                            <span>{format_date(&anniversary)}</span>
                            <small>"Anniversary"</small>
                        </div>
                    </div>
                })}
                {related_rows}
            </div>
        })}

        {(!groups.is_empty() || !notes.is_empty() || writable).then(|| view! {
            <div class="property-card">
                <div class="property contacts-prop">
                    <Icon name="tag"/>
                    <div class="contacts-detail-groups">
                        <div class="contacts-detail-chips">
                            {groups.iter().map(|g| {
                                let g_click = g.clone();
                                let g_rm = g.clone();
                                view! {
                                    <span class="contacts-chip">
                                        <button
                                            type="button"
                                            class="contacts-chip-label"
                                            on:click=move |_| on_filter_group.run(g_click.clone())
                                        >{g.clone()}</button>
                                        {writable.then(|| view! {
                                            <button
                                                type="button"
                                                class="contacts-chip-remove"
                                                aria-label=format!("Remove from {g_rm}")
                                                disabled=move || busy.get()
                                                on:click=move |_| {
                                                    let next: Vec<String> = current_groups
                                                        .get_value()
                                                        .into_iter()
                                                        .filter(|x| x != &g_rm)
                                                        .collect();
                                                    on_update_groups.run((item_grp_rm.get_value(), next));
                                                }
                                            >
                                                <Icon name="close"/>
                                            </button>
                                        })}
                                    </span>
                                }
                            }).collect_view()}
                            {writable.then(|| view! {
                                <button
                                    type="button"
                                    class="contacts-chip-add"
                                    on:click=move |_| adding_group.update(|v| *v = !*v)
                                >
                                    <Icon name="plus"/>"Group"
                                </button>
                            })}
                        </div>
                        {move || adding_group.get().then(|| {
                            let existing = all_groups.get();
                            let mine = current_groups.get_value();
                            let suggestions: Vec<String> = existing
                                .into_iter()
                                .filter(|g| !mine.contains(g))
                                .collect();
                            view! {
                                <div class="contacts-inline-group-picker">
                                    <div class="contacts-inline-group-row">
                                        <input
                                            type="text"
                                            placeholder="Group name"
                                            aria-label="Add to group"
                                            prop:value=move || new_group_text.get()
                                            on:input=move |e| new_group_text.set(event_target_value(&e))
                                            on:keydown=move |e| {
                                                if e.key() == "Enter" {
                                                    e.prevent_default();
                                                    add_group_submit(new_group_text.get_untracked());
                                                }
                                            }
                                        />
                                        <button
                                            type="button"
                                            class="secondary-button"
                                            on:click=move |_| add_group_submit(new_group_text.get_untracked())
                                        >"Add"</button>
                                    </div>
                                    {(!suggestions.is_empty()).then(|| view! {
                                        <div class="contacts-group-suggestions">
                                            {suggestions.into_iter().map(|sg| {
                                                let chosen = sg.clone();
                                                view! {
                                                    <button
                                                        type="button"
                                                        class="contacts-suggest-pill"
                                                        on:click=move |_| add_group_submit(chosen.clone())
                                                    >{sg}</button>
                                                }
                                            }).collect_view()}
                                        </div>
                                    })}
                                </div>
                            }
                        })}
                    </div>
                </div>
                {(!notes.is_empty()).then(|| view! {
                    <div class="property contacts-prop">
                        <Icon name="note"/>
                        <div class="contacts-prop-body">
                            <p class="pim-notes">{notes}</p>
                        </div>
                    </div>
                })}
            </div>
        })}

        {has_dar_links.then(|| view! {
            <div class="property-card contacts-dar-links">
                <h3>"In Dar"</h3>
                <div class="contacts-dar-link-row">
                    {email.is_some().then(|| view! {
                        <a class="text-button contacts-app-link" href="/mail">
                            <Icon name="mail"/>"Open Mail"
                        </a>
                    })}
                    {phone.is_some().then(|| view! {
                        <a class="text-button contacts-app-link" href="/chat">
                            <Icon name="chat"/>"Open Chat"
                        </a>
                    })}
                    <a class="text-button contacts-app-link" href="/calendar">
                        <Icon name="calendar"/>"Schedule"
                    </a>
                </div>
            </div>
        })}

        <div class="pim-detail-footer">
            <span class="muted">
                <CollectionDot color=book.as_ref().map(|b| b.color.clone()).unwrap_or_default()/>
                {book.as_ref().map(|b| b.name.clone())}
            </span>
            <a
                class="text-button"
                aria-label="Export contact"
                title="Export contact"
                href=format!("{BASE}/{}/items/{}?export=1", item.collection_id, item.href)
            >
                <Icon name="download"/>"Export"
            </a>
            {writable.then(|| view! {
                <button
                    type="button"
                    class="text-button danger"
                    aria-label="Delete contact"
                    title="Delete contact"
                    on:click=move |_| on_delete.run(())
                >
                    <Icon name="trash"/>"Delete"
                </button>
            })}
        </div>
    }
}
