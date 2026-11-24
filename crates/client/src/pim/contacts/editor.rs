//! Contact editor: photo upload/crop/remove, address book move, name & work, typed
//! emails/phones/URLs/addresses, interactive group chip picker, birthday picker with
//! omit-year toggle, and progressive "+ Add field" disclosure for rich vCard properties.

use leptos::html;
use leptos::prelude::*;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use super::model::{
    ADDRESS_LABELS, ADDRESS_TYPE_LABELS, EMAIL_LABELS, PHONE_LABELS, RELATED_TYPES,
    SOCIAL_SERVICES, URL_LABELS, contact_groups, empty, photo_url, s,
};
use crate::icons::Icon;
use crate::pim::{Collection, Done, avatar_hue, initials};
use crate::ui::{EditorPane, Select, document};

#[component]
pub fn ContactEditor(
    books: Vec<Collection>,
    all_groups: Vec<String>,
    initial: Option<Value>,
    #[prop(optional, into)] initial_href: String,
    #[prop(optional)] initial_revision: u64,
    #[prop(into)] initial_book: String,
    on_save: Callback<(Value, String, Done)>,
    on_close: Callback<()>,
) -> impl IntoView {
    let editing = initial.is_some();
    let start = initial.clone().unwrap_or_else(empty);
    let data = RwSignal::new(start.clone());
    let first_book = books.first().map(|b| b.id.clone()).unwrap_or_default();
    let start_book = if initial_book.is_empty() {
        first_book
    } else {
        initial_book.clone()
    };
    let book = RwSignal::new(start_book.clone());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let photo_preview: RwSignal<Option<String>> = RwSignal::new(None);
    let photo_removed = RwSignal::new(false);
    let photo_input = NodeRef::<html::Input>::new();
    let group_input = RwSignal::new(String::new());

    // Revoke any blob URL created for photo preview on cleanup.
    on_cleanup(move || {
        if let Some(url) = photo_preview.try_get_untracked().flatten() {
            let _ = web_sys::Url::revoke_object_url(&url);
        }
    });

    let had_initial_photo = start["hasPhoto"] == json!(true);
    let initial_photo_url = StoredValue::new(
        (editing && had_initial_photo && !initial_href.is_empty())
            .then(|| photo_url(&start_book, &initial_href, initial_revision)),
    );

    let original = json!({ "data": start, "book": start_book });
    let dirty = Signal::derive(move || {
        json!({ "data": data.get(), "book": book.get() }) != original
            || !group_input.with(|g| g.trim().is_empty())
    });

    let extra_names = initial.as_ref().is_some_and(|i| {
        let joined = [s(i, "firstName"), s(i, "lastName")]
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        !s(i, "prefix").is_empty()
            || !s(i, "middleName").is_empty()
            || !s(i, "suffix").is_empty()
            || !s(i, "displayName").is_empty() && s(i, "displayName") != joined
    });

    // Progressive extra-field visibility toggles (auto-open when values exist).
    let show_nickname = RwSignal::new(!s(&data.get_untracked(), "nickname").is_empty());
    let show_pronouns = RwSignal::new(!s(&data.get_untracked(), "pronouns").is_empty());
    let show_department = RwSignal::new(!s(&data.get_untracked(), "department").is_empty());
    let show_anniversary = RwSignal::new(!s(&data.get_untracked(), "anniversary").is_empty());
    let show_socials = RwSignal::new(
        data.get_untracked()["socials"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
    );
    let show_related = RwSignal::new(
        data.get_untracked()["related"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
    );

    let omit_bday_year = RwSignal::new(s(&data.get_untracked(), "birthday").starts_with("--"));

    let set = move |key: &'static str, value: Value| data.update(|d| d[key] = value);
    let input = move |key: &'static str, label: &'static str, autofocus: bool| {
        view! {
            <label>
                {label}
                <input
                    prop:value=move || data.with(|d| s(d, key))
                    on:input=move |e| set(key, json!(event_target_value(&e)))
                    maxlength="1024"
                    autofocus=autofocus
                />
            </label>
        }
    };
    let display_placeholder = move || {
        data.with(|d| {
            [s(d, "firstName"), s(d, "lastName")]
                .into_iter()
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
    };

    let book_options: Vec<(String, String)> = books
        .iter()
        .map(|b| (b.id.clone(), b.name.clone()))
        .collect();
    let show_book_select = !editing || book_options.len() > 1;
    let known_groups = StoredValue::new(all_groups);

    // Resize uploaded image onto a 256x256 square canvas and store canonical base64 JPEG.
    let on_pick_photo = move |_| {
        let Some(input_el) = photo_input.get() else {
            return;
        };
        let Some(file) = input_el.files().and_then(|l| l.item(0)) else {
            return;
        };
        if file.size() > 8.0 * 1024.0 * 1024.0 {
            error.set("Photo file must be 8 MiB or smaller.".into());
            return;
        }
        let Ok(blob_url) = web_sys::Url::create_object_url_with_blob(&file) else {
            return;
        };
        if let Some(old) = photo_preview.get_untracked() {
            let _ = web_sys::Url::revoke_object_url(&old);
        }
        photo_preview.set(Some(blob_url.clone()));
        photo_removed.set(false);
        error.set(String::new());

        let Ok(img) = web_sys::HtmlImageElement::new() else {
            return;
        };
        let img_clone = img.clone();
        let onload = Closure::<dyn FnMut()>::once(move || {
            let w = img_clone.natural_width() as f64;
            let h = img_clone.natural_height() as f64;
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let Ok(el) = document().create_element("canvas") else {
                return;
            };
            let Ok(canvas) = el.dyn_into::<web_sys::HtmlCanvasElement>() else {
                return;
            };
            let size = 256u32;
            canvas.set_width(size);
            canvas.set_height(size);
            let Ok(Some(ctx_val)) = canvas.get_context("2d") else {
                return;
            };
            let Ok(ctx) = ctx_val.dyn_into::<web_sys::CanvasRenderingContext2d>() else {
                return;
            };
            let side = w.min(h);
            let sx = (w - side) / 2.0;
            let sy = (h - side) / 2.0;
            let _ = ctx
                .draw_image_with_html_image_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                    &img_clone,
                    sx,
                    sy,
                    side,
                    side,
                    0.0,
                    0.0,
                    size as f64,
                    size as f64,
                );
            if let Ok(data_url) = canvas.to_data_url_with_type_and_encoder_options(
                "image/jpeg",
                &wasm_bindgen::JsValue::from_f64(0.84),
            ) && let Some(b64) = data_url.strip_prefix("data:image/jpeg;base64,")
            {
                data.try_update(|d| {
                    d["photoData"] = json!(b64);
                    d["photoMime"] = json!("image/jpeg");
                    d["removePhoto"] = json!(false);
                    d["hasPhoto"] = json!(true);
                });
            }
        });
        img.set_onload(Some(onload.as_ref().unchecked_ref()));
        onload.forget();
        img.set_src(&blob_url);
    };

    let remove_photo = move |_| {
        if let Some(old) = photo_preview.get_untracked() {
            let _ = web_sys::Url::revoke_object_url(&old);
        }
        photo_preview.set(None);
        photo_removed.set(true);
        data.update(|d| {
            if let Some(obj) = d.as_object_mut() {
                obj.remove("photoData");
                obj.remove("photoMime");
            }
            d["removePhoto"] = json!(true);
            d["hasPhoto"] = json!(false);
        });
    };

    let add_group_chip = move |raw: String| {
        let trimmed = raw.trim().to_owned();
        if trimmed.is_empty() {
            return;
        }
        data.update(|d| {
            let mut list = contact_groups(d);
            if !list.contains(&trimmed) {
                list.push(trimmed);
                d["groups"] = json!(list);
            }
        });
        group_input.set(String::new());
    };

    let multi = move |key: &'static str,
                      label: &'static str,
                      icon: &'static str,
                      options: &'static [(&'static str, &'static str)]| {
        let kind = match key {
            "emails" => "email",
            "phones" => "tel",
            _ => "url",
        };
        let default_label = options.first().map(|(v, _)| *v).unwrap_or("other");
        view! {
            <fieldset class="form-section">
                <legend><Icon name=icon/>{label}</legend>
                {move || data.with(|d| d[key].as_array().cloned().unwrap_or_default()).into_iter().enumerate().map(|(i, v)| {
                    let n = i + 1;
                    let cur_label = v["label"].as_str().unwrap_or(default_label).to_ascii_lowercase();
                    let is_custom = !options.iter().any(|(val, _)| *val == cur_label);
                    let custom_val = cur_label.clone();
                    view! {
                        <div class="pim-multi-row">
                            <label>
                                <span class="sr-only">{format!("{label} {n}")}</span>
                                <input
                                    aria-label=format!("{label} {n}")
                                    type=kind
                                    prop:value=v["value"].as_str().unwrap_or_default().to_owned()
                                    on:input=move |e| data.update(|d| d[key][i]["value"] = json!(event_target_value(&e)))
                                />
                            </label>
                            <label>
                                <span class="sr-only">{format!("Label for {label} {n}")}</span>
                                <select
                                    aria-label=format!("Label for {label} {n}")
                                    prop:value=cur_label
                                    on:change=move |e| data.update(|d| d[key][i]["label"] = json!(event_target_value(&e)))
                                >
                                    {options.iter().map(|&(val, text)| view! { <option value=val>{text}</option> }).collect_view()}
                                    {is_custom.then(|| view! { <option value=custom_val.clone()>{custom_val.clone()}</option> })}
                                </select>
                            </label>
                            <button
                                type="button"
                                class="icon-button"
                                aria-label=format!("Remove {label} {n}")
                                on:click=move |_| data.update(|d| {
                                    if let Some(list) = d[key].as_array_mut() {
                                        list.remove(i);
                                    }
                                })
                            >
                                <Icon name="close"/>
                            </button>
                        </div>
                    }
                }).collect_view()}
                <button
                    type="button"
                    class="text-button add-button"
                    on:click=move |_| data.update(|d| {
                        if let Some(list) = d[key].as_array_mut() {
                            list.push(json!({ "value": "", "label": default_label }));
                        }
                    })
                >
                    <Icon name="plus"/>"Add "{label.to_lowercase()}
                </button>
            </fieldset>
        }
    };

    let bday_date_value = move || {
        let raw = data.with(|d| s(d, "birthday"));
        if let Some(rest) = raw.strip_prefix("--") {
            format!("2000-{rest}")
        } else {
            raw
        }
    };

    view! {
        <EditorPane title=if editing { "Edit contact" } else { "New contact" } on_close=on_close dirty=dirty>
            <form class="pim-editor contact-editor" on:submit=move |e: web_sys::SubmitEvent| {
                e.prevent_default();
                let pending_group = group_input.get_untracked();
                if !pending_group.trim().is_empty() {
                    add_group_chip(pending_group);
                }
                busy.set(true);
                error.set(String::new());
                on_save.run((
                    data.get_untracked(),
                    book.get_untracked(),
                    Callback::new(move |result: Result<(), String>| {
                        busy.set(false);
                        if let Err(e) = result {
                            error.set(e);
                        }
                    }),
                ));
            }>
                {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}

                <div class="contacts-editor-photo">
                    {move || {
                        let name_now = data.with(|d| {
                            let dn = s(d, "displayName");
                            if !dn.is_empty() { dn } else { format!("{} {}", s(d, "firstName"), s(d, "lastName")) }
                        });
                        let preview = photo_preview.get();
                        let existing = (!photo_removed.get()).then(|| initial_photo_url.get_value()).flatten();
                        let shown = preview.or(existing);
                        view! {
                            <span class=format!("contact-initials contacts-editor-avatar {}", avatar_hue(&name_now))>
                                {match shown {
                                    Some(url) => view! { <img src=url alt="" class="contact-avatar-img"/> }.into_any(),
                                    None => view! { {initials(&name_now)} }.into_any(),
                                }}
                            </span>
                        }
                    }}
                    <div class="contacts-editor-photo-text">
                        <strong>"Contact photo"</strong>
                        <small>"Saved in vCard and synced over CardDAV"</small>
                    </div>
                    <div class="contacts-editor-photo-actions">
                        <label class="secondary-button contacts-photo-upload">
                            "Upload"
                            <input
                                type="file"
                                node_ref=photo_input
                                accept="image/jpeg,image/png,image/webp"
                                class="sr-only"
                                on:change=on_pick_photo
                            />
                        </label>
                        {move || (photo_preview.with(Option::is_some) || (!photo_removed.get() && initial_photo_url.with_value(Option::is_some))).then(|| view! {
                            <button type="button" class="text-button danger" on:click=remove_photo>"Remove"</button>
                        })}
                    </div>
                </div>

                {show_book_select.then(|| view! {
                    <label>
                        "Address book"
                        <Select options=book_options.clone() value=book/>
                    </label>
                })}

                <section class="form-section">
                    <h3><Icon name="user"/>"Name"</h3>
                    <div class="pim-form-grid">
                        {input("firstName", "First name", !editing)}
                        {input("lastName", "Last name", false)}
                    </div>
                    {move || (show_nickname.get() || show_pronouns.get()).then(|| view! {
                        <div class="pim-form-grid">
                            {move || show_nickname.get().then(|| input("nickname", "Nickname", false))}
                            {move || show_pronouns.get().then(|| input("pronouns", "Pronouns", false))}
                        </div>
                    })}
                    <details class="form-more" open=extra_names>
                        <summary>"Prefix, middle name, suffix and display name"</summary>
                        <div class="pim-form-grid">
                            {input("prefix", "Prefix", false)}
                            {input("middleName", "Middle name", false)}
                            {input("suffix", "Suffix", false)}
                            <label>
                                "Display name"
                                <input
                                    prop:value=move || data.with(|d| s(d, "displayName"))
                                    on:input=move |e| set("displayName", json!(event_target_value(&e)))
                                    maxlength="1024"
                                    placeholder=move || {
                                        let p = display_placeholder();
                                        (!p.is_empty()).then_some(p)
                                    }
                                />
                            </label>
                        </div>
                    </details>
                </section>

                <section class="form-section">
                    <h3><Icon name="building"/>"Work"</h3>
                    <div class="pim-form-grid">
                        {input("organization", "Organization", false)}
                        {input("title", "Job title", false)}
                    </div>
                    {move || show_department.get().then(|| view! {
                        <div class="pim-form-grid">
                            {input("department", "Department", false)}
                        </div>
                    })}
                </section>

                {multi("emails", "Email", "mail", &EMAIL_LABELS)}
                {multi("phones", "Phone", "phone", &PHONE_LABELS)}
                {multi("urls", "Website", "globe", &URL_LABELS)}

                {move || show_socials.get().then(|| view! {
                    <fieldset class="form-section">
                        <legend><Icon name="link"/>"Social profiles"</legend>
                        {move || data.with(|d| d["socials"].as_array().cloned().unwrap_or_default()).into_iter().enumerate().map(|(i, v)| {
                            let n = i + 1;
                            let cur = v["service"].as_str().unwrap_or("github").to_ascii_lowercase();
                            view! {
                                <div class="pim-multi-row">
                                    <label>
                                        <span class="sr-only">{format!("Social profile {n}")}</span>
                                        <input
                                            aria-label=format!("Social profile {n}")
                                            placeholder="Handle or URL"
                                            prop:value=v["value"].as_str().unwrap_or_default().to_owned()
                                            on:input=move |e| data.update(|d| d["socials"][i]["value"] = json!(event_target_value(&e)))
                                        />
                                    </label>
                                    <label>
                                        <span class="sr-only">{format!("Service for social profile {n}")}</span>
                                        <select
                                            aria-label=format!("Service for social profile {n}")
                                            prop:value=cur
                                            on:change=move |e| data.update(|d| d["socials"][i]["service"] = json!(event_target_value(&e)))
                                        >
                                            {SOCIAL_SERVICES.iter().map(|&(val, text)| view! { <option value=val>{text}</option> }).collect_view()}
                                        </select>
                                    </label>
                                    <button
                                        type="button"
                                        class="icon-button"
                                        aria-label=format!("Remove social profile {n}")
                                        on:click=move |_| data.update(|d| {
                                            if let Some(list) = d["socials"].as_array_mut() {
                                                list.remove(i);
                                            }
                                        })
                                    >
                                        <Icon name="close"/>
                                    </button>
                                </div>
                            }
                        }).collect_view()}
                        <button
                            type="button"
                            class="text-button add-button"
                            on:click=move |_| data.update(|d| {
                                if let Some(list) = d["socials"].as_array_mut() {
                                    list.push(json!({ "service": "github", "value": "" }));
                                } else {
                                    d["socials"] = json!([{ "service": "github", "value": "" }]);
                                }
                            })
                        >
                            <Icon name="plus"/>"Add social profile"
                        </button>
                    </fieldset>
                })}

                <fieldset class="form-section">
                    <legend><Icon name="location"/>"Postal addresses"</legend>
                    {move || data.with(|d| d["addresses"].as_array().cloned().unwrap_or_default()).into_iter().enumerate().map(|(i, a)| {
                        let cur_label = a["label"].as_str().unwrap_or("home").to_ascii_lowercase();
                        view! {
                            <div class="pim-form-grid contact-address">
                                <label>
                                    "Label"
                                    <select
                                        prop:value=cur_label
                                        on:change=move |e| data.update(|d| d["addresses"][i]["label"] = json!(event_target_value(&e)))
                                    >
                                        {ADDRESS_TYPE_LABELS.iter().map(|&(val, text)| view! { <option value=val>{text}</option> }).collect_view()}
                                    </select>
                                </label>
                                {ADDRESS_LABELS.iter().map(|&(key, label)| {
                                    let value = a[key].as_str().unwrap_or_default().to_owned();
                                    view! {
                                        <label class=(key == "street").then_some("wide")>
                                            {label}
                                            <input
                                                prop:value=value
                                                on:input=move |e| data.update(|d| d["addresses"][i][key] = json!(event_target_value(&e)))
                                            />
                                        </label>
                                    }
                                }).collect_view()}
                                <button
                                    type="button"
                                    class="text-button danger"
                                    on:click=move |_| data.update(|d| {
                                        if let Some(list) = d["addresses"].as_array_mut() {
                                            list.remove(i);
                                        }
                                    })
                                >
                                    <Icon name="trash"/>"Remove address"
                                </button>
                            </div>
                        }
                    }).collect_view()}
                    <button
                        type="button"
                        class="text-button add-button"
                        on:click=move |_| data.update(|d| {
                            if let Some(list) = d["addresses"].as_array_mut() {
                                list.push(json!({ "label": "home", "street": "", "city": "", "region": "", "postalCode": "", "country": "" }));
                            }
                        })
                    >
                        <Icon name="plus"/>"Add address"
                    </button>
                </fieldset>

                {move || show_related.get().then(|| view! {
                    <fieldset class="form-section">
                        <legend><Icon name="contacts"/>"Related people"</legend>
                        {move || data.with(|d| d["related"].as_array().cloned().unwrap_or_default()).into_iter().enumerate().map(|(i, v)| {
                            let n = i + 1;
                            let cur = v["relation"].as_str().unwrap_or("spouse").to_ascii_lowercase();
                            view! {
                                <div class="pim-multi-row">
                                    <label>
                                        <span class="sr-only">{format!("Related person {n}")}</span>
                                        <input
                                            aria-label=format!("Related person {n}")
                                            placeholder="Name"
                                            prop:value=v["value"].as_str().unwrap_or_default().to_owned()
                                            on:input=move |e| data.update(|d| d["related"][i]["value"] = json!(event_target_value(&e)))
                                        />
                                    </label>
                                    <label>
                                        <span class="sr-only">{format!("Relation {n}")}</span>
                                        <select
                                            aria-label=format!("Relation {n}")
                                            prop:value=cur
                                            on:change=move |e| data.update(|d| d["related"][i]["relation"] = json!(event_target_value(&e)))
                                        >
                                            {RELATED_TYPES.iter().map(|&(val, text)| view! { <option value=val>{text}</option> }).collect_view()}
                                        </select>
                                    </label>
                                    <button
                                        type="button"
                                        class="icon-button"
                                        aria-label=format!("Remove related person {n}")
                                        on:click=move |_| data.update(|d| {
                                            if let Some(list) = d["related"].as_array_mut() {
                                                list.remove(i);
                                            }
                                        })
                                    >
                                        <Icon name="close"/>
                                    </button>
                                </div>
                            }
                        }).collect_view()}
                        <button
                            type="button"
                            class="text-button add-button"
                            on:click=move |_| data.update(|d| {
                                if let Some(list) = d["related"].as_array_mut() {
                                    list.push(json!({ "relation": "spouse", "value": "" }));
                                } else {
                                    d["related"] = json!([{ "relation": "spouse", "value": "" }]);
                                }
                            })
                        >
                            <Icon name="plus"/>"Add related person"
                        </button>
                    </fieldset>
                })}

                <section class="form-section">
                    <h3><Icon name="tag"/>"Groups"</h3>
                    <div class="contacts-editor-groups">
                        <div class="contacts-detail-chips">
                            {move || {
                                let mine = data.with(contact_groups);
                                mine.into_iter().map(|g| {
                                    let g_rm = g.clone();
                                    view! {
                                        <span class="contacts-chip">
                                            <span class="contacts-chip-text">{g}</span>
                                            <button
                                                type="button"
                                                class="contacts-chip-remove"
                                                aria-label=format!("Remove group {g_rm}")
                                                on:click=move |_| data.update(|d| {
                                                    let next: Vec<String> = contact_groups(d)
                                                        .into_iter()
                                                        .filter(|x| x != &g_rm)
                                                        .collect();
                                                    d["groups"] = json!(next);
                                                })
                                            >
                                                <Icon name="close"/>
                                            </button>
                                        </span>
                                    }
                                }).collect_view()
                            }}
                        </div>
                        <div class="contacts-editor-group-input">
                            <label class="contacts-editor-group-label">
                                <span class="sr-only">"Add group"</span>
                                <input
                                    type="text"
                                    aria-label="Add group"
                                    placeholder="Type a group name and press Enter"
                                    prop:value=move || group_input.get()
                                    on:input=move |e| group_input.set(event_target_value(&e))
                                    on:keydown=move |e| {
                                        if e.key() == "Enter" {
                                            e.prevent_default();
                                            add_group_chip(group_input.get_untracked());
                                        }
                                    }
                                />
                            </label>
                            <button
                                type="button"
                                class="secondary-button"
                                on:click=move |_| add_group_chip(group_input.get_untracked())
                            >"Add"</button>
                        </div>
                        {move || {
                            let mine = data.with(contact_groups);
                            let unused: Vec<String> = known_groups
                                .get_value()
                                .into_iter()
                                .filter(|g| !mine.contains(g))
                                .collect();
                            (!unused.is_empty()).then(|| view! {
                                <div class="contacts-group-suggestions">
                                    {unused.into_iter().map(|g| {
                                        let chosen = g.clone();
                                        view! {
                                            <button
                                                type="button"
                                                class="contacts-suggest-pill"
                                                on:click=move |_| add_group_chip(chosen.clone())
                                            >
                                                <Icon name="plus"/>{g}
                                            </button>
                                        }
                                    }).collect_view()}
                                </div>
                            })
                        }}
                    </div>
                </section>

                <section class="form-section">
                    <h3><Icon name="info"/>"More"</h3>
                    <div class="pim-form-grid">
                        <label>
                            "Birthday"
                            <input
                                type="date"
                                prop:value=bday_date_value
                                on:input=move |e| {
                                    let val = event_target_value(&e);
                                    if val.is_empty() {
                                        set("birthday", json!(""));
                                    } else if omit_bday_year.get_untracked() && val.len() == 10 {
                                        set("birthday", json!(format!("--{}", &val[5..])));
                                    } else {
                                        set("birthday", json!(val));
                                    }
                                }
                            />
                        </label>
                        <label class="pim-checkbox contacts-bday-omit">
                            <input
                                type="checkbox"
                                prop:checked=move || omit_bday_year.get()
                                on:change=move |e| {
                                    let checked = event_target_checked(&e);
                                    omit_bday_year.set(checked);
                                    let cur = data.with(|d| s(d, "birthday"));
                                    if checked && cur.len() == 10 && !cur.starts_with("--") {
                                        set("birthday", json!(format!("--{}", &cur[5..])));
                                    } else if !checked && cur.starts_with("--") && cur.len() == 7 {
                                        set("birthday", json!(format!("2000-{}", &cur[2..])));
                                    }
                                }
                            />
                            "Omit birth year"
                        </label>
                    </div>
                    {move || show_anniversary.get().then(|| view! {
                        <div class="pim-form-grid">
                            <label>
                                "Anniversary"
                                <input
                                    type="date"
                                    prop:value=move || data.with(|d| s(d, "anniversary"))
                                    on:input=move |e| set("anniversary", json!(event_target_value(&e)))
                                />
                            </label>
                        </div>
                    })}
                    <label>
                        "Notes"
                        <textarea
                            rows="4"
                            prop:value=move || data.with(|d| s(d, "notes"))
                            on:input=move |e| set("notes", json!(event_target_value(&e)))
                        ></textarea>
                    </label>
                    <label class="pim-checkbox">
                        <input
                            type="checkbox"
                            prop:checked=move || data.with(|d| d["favorite"] == json!(true))
                            on:change=move |e| set("favorite", json!(event_target_checked(&e)))
                        />
                        "Favorite"
                    </label>

                    <div class="contacts-add-fields">
                        {move || (!show_nickname.get()).then(|| view! {
                            <button type="button" class="contacts-add-field-pill" on:click=move |_| show_nickname.set(true)><Icon name="plus"/>"Nickname"</button>
                        })}
                        {move || (!show_pronouns.get()).then(|| view! {
                            <button type="button" class="contacts-add-field-pill" on:click=move |_| show_pronouns.set(true)><Icon name="plus"/>"Pronouns"</button>
                        })}
                        {move || (!show_department.get()).then(|| view! {
                            <button type="button" class="contacts-add-field-pill" on:click=move |_| show_department.set(true)><Icon name="plus"/>"Department"</button>
                        })}
                        {move || (!show_anniversary.get()).then(|| view! {
                            <button type="button" class="contacts-add-field-pill" on:click=move |_| show_anniversary.set(true)><Icon name="plus"/>"Anniversary"</button>
                        })}
                        {move || (!show_socials.get()).then(|| view! {
                            <button type="button" class="contacts-add-field-pill" on:click=move |_| {
                                show_socials.set(true);
                                data.update(|d| {
                                    if d["socials"].as_array().is_none_or(Vec::is_empty) {
                                        d["socials"] = json!([{ "service": "github", "value": "" }]);
                                    }
                                });
                            }><Icon name="plus"/>"Social profile"</button>
                        })}
                        {move || (!show_related.get()).then(|| view! {
                            <button type="button" class="contacts-add-field-pill" on:click=move |_| {
                                show_related.set(true);
                                data.update(|d| {
                                    if d["related"].as_array().is_none_or(Vec::is_empty) {
                                        d["related"] = json!([{ "relation": "spouse", "value": "" }]);
                                    }
                                });
                            }><Icon name="plus"/>"Related person"</button>
                        })}
                    </div>
                </section>

                <div class="dialog-actions pane-footer">
                    <button type="button" class="text-button">"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get() || book.with(String::is_empty)>"Save contact"</button>
                </div>
            </form>
        </EditorPane>
    }
}
