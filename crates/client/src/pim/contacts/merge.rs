//! Merge & fix duplicate contact suggestions view.

use leptos::prelude::*;

use super::model::{DuplicatePair, display_name, s};
use crate::icons::Icon;
use crate::pim::{Collection, CollectionDot};

#[component]
pub fn MergeView(
    pairs: Memo<Vec<DuplicatePair>>,
    books: RwSignal<Vec<Collection>>,
    busy: RwSignal<bool>,
    on_merge: Callback<DuplicatePair>,
    on_dismiss: Callback<String>,
    on_merge_all: Callback<()>,
) -> impl IntoView {
    let book_info = move |id: &str| {
        books.with(|list| {
            list.iter()
                .find(|b| b.id == id)
                .map(|b| (b.name.clone(), b.color.clone(), b.role != "read"))
                .unwrap_or_else(|| (String::new(), "#73396f".into(), false))
        })
    };

    view! {
        <div class="contacts-heading">
            <h1>"Merge & fix"</h1>
            <span class="contacts-count-pill" aria-hidden="true">
                {move || {
                    let n = pairs.with(Vec::len);
                    format!("{n} {}", if n == 1 { "suggestion" } else { "suggestions" })
                }}
            </span>
            <span class="contacts-heading-spacer"></span>
            {move || (!pairs.with(Vec::is_empty)).then(|| view! {
                <button
                    type="button"
                    class="primary-button contacts-merge-all-btn"
                    disabled=move || busy.get()
                    on:click=move |_| on_merge_all.run(())
                >
                    <Icon name="merge"/>"Merge all"
                </button>
            })}
        </div>
        <div class="contact-list contacts-merge-list">
            {move || {
                let list = pairs.get();
                if list.is_empty() {
                    return view! {
                        <div class="pim-empty">
                            <h2>"All clean"</h2>
                            <p>"No duplicate contacts were found across your address books."</p>
                        </div>
                    }
                    .into_any();
                }
                list.into_iter().map(|pair| {
                    let key = pair.key.clone();
                    let pair_merge = pair.clone();
                    let (a_book, a_color, a_writable) = book_info(&pair.primary.collection_id);
                    let (b_book, b_color, b_writable) = book_info(&pair.secondary.collection_id);
                    let can_merge = a_writable && b_writable;
                    let a_name = display_name(&pair.primary.data, "firstLast");
                    let b_name = display_name(&pair.secondary.data, "firstLast");
                    let a_email = pair.primary.data["emails"][0]["value"].as_str().unwrap_or("—").to_owned();
                    let b_email = pair.secondary.data["emails"][0]["value"].as_str().unwrap_or("—").to_owned();
                    let a_phone = pair.primary.data["phones"][0]["value"].as_str().unwrap_or("—").to_owned();
                    let b_phone = pair.secondary.data["phones"][0]["value"].as_str().unwrap_or("—").to_owned();
                    let a_org = s(&pair.primary.data, "organization");
                    let b_org = s(&pair.secondary.data, "organization");
                    let email_diff = b_email != "—" && !b_email.eq_ignore_ascii_case(&a_email);
                    let phone_diff = b_phone != "—" && b_phone != a_phone;
                    view! {
                        <article class="contacts-merge-card">
                            <header class="contacts-merge-top">
                                <strong>"Potential duplicate"</strong>
                                <span class="contacts-merge-reason">{pair.reason.clone()}</span>
                            </header>
                            <div class="contacts-merge-pair">
                                <div class="contacts-merge-person">
                                    <div class="contacts-merge-who">
                                        <CollectionDot color=a_color/>
                                        <strong>{a_name}</strong>
                                        <small>{a_book}</small>
                                    </div>
                                    <div class="contacts-merge-line">{a_email}</div>
                                    <div class="contacts-merge-line num">{a_phone}</div>
                                    {(!a_org.is_empty()).then(|| view! { <div class="contacts-merge-line">{a_org}</div> })}
                                </div>
                                <span class="contacts-merge-plus" aria-hidden="true">"+"</span>
                                <div class="contacts-merge-person">
                                    <div class="contacts-merge-who">
                                        <CollectionDot color=b_color/>
                                        <strong>{b_name}</strong>
                                        <small>{b_book}</small>
                                    </div>
                                    <div class=if email_diff { "contacts-merge-line is-diff" } else { "contacts-merge-line" }>{b_email}</div>
                                    <div class=if phone_diff { "contacts-merge-line num is-diff" } else { "contacts-merge-line num" }>{b_phone}</div>
                                    {(!b_org.is_empty()).then(|| view! { <div class="contacts-merge-line">{b_org}</div> })}
                                </div>
                            </div>
                            <footer class="contacts-merge-foot">
                                <button
                                    type="button"
                                    class="text-button"
                                    disabled=move || busy.get()
                                    on:click=move |_| on_dismiss.run(key.clone())
                                >"Dismiss"</button>
                                <button
                                    type="button"
                                    class="primary-button"
                                    disabled=move || busy.get() || !can_merge
                                    title=if can_merge { "Merge into one contact" } else { "Requires write access to both address books" }
                                    on:click=move |_| on_merge.run(pair_merge.clone())
                                >"Merge into one"</button>
                            </footer>
                        </article>
                    }
                }).collect_view().into_any()
            }}
        </div>
    }
}
