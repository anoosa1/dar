//! News settings sheet: General (reading, typography defaults, updates, Nextcloud News apps),
//! Import and export (OPML + Starred JSON), and Keyboard shortcuts.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use super::model::{
    BASE, ImportResult, NEWS_KEEP_CHOICES, NEWS_REFRESH_CHOICES, copy_text, minutes,
};
use crate::api::{self, BrowserSession};
use crate::icons::Icon;
use crate::settings::{
    Notice, SaveProblems, Saver, Segmented, SettingRow, SettingSwitch, SettingsSheet,
};
use crate::ui::{Select, window};

#[component]
pub fn NewsSettings(
    session: BrowserSession,
    on_close: Callback<()>,
    on_imported: Callback<()>,
) -> impl IntoView {
    let username = StoredValue::new(session.user.username.clone());
    let csrf = StoredValue::new(session.csrf_token.clone());
    let saver = Saver::new(session);
    let section = RwSignal::new("General");
    let importing = RwSignal::new(false);
    let import_result = RwSignal::new(String::new());
    let import_error = RwSignal::new(String::new());

    let text = move |key: &'static str, default: &'static str| {
        Signal::derive(move || saver.value(key).as_str().unwrap_or(default).to_owned())
    };
    let flag = move |key: &'static str, default: bool| {
        Signal::derive(move || saver.value(key).as_bool().unwrap_or(default))
    };

    let refresh_val = RwSignal::new("30".to_owned());
    let keep_val = RwSignal::new("30".to_owned());
    Effect::new(move |_| {
        let r = saver
            .value("newsRefresh")
            .as_i64()
            .unwrap_or(30)
            .to_string();
        if refresh_val.get_untracked() != r {
            refresh_val.set(r);
        }
        let k = saver
            .value("newsKeepDays")
            .as_i64()
            .unwrap_or(30)
            .to_string();
        if keep_val.get_untracked() != k {
            keep_val.set(k);
        }
    });

    let refresh_options: StoredValue<Vec<(String, String)>> = StoredValue::new(
        NEWS_REFRESH_CHOICES
            .iter()
            .map(|&v| (v.to_string(), format!("Every {}", minutes(v))))
            .collect(),
    );
    let keep_options: StoredValue<Vec<(String, String)>> = StoredValue::new(
        NEWS_KEEP_CHOICES
            .iter()
            .map(|&v| (v.to_string(), format!("{v} days")))
            .collect(),
    );

    let address = StoredValue::new(format!(
        "{}/dav/news",
        window().location().origin().unwrap_or_default()
    ));

    let import_file = move |file: Option<web_sys::File>| {
        let Some(file) = file else { return };
        importing.set(true);
        import_error.set(String::new());
        import_result.set(String::new());
        let file = send_wrapper::SendWrapper::new(file);
        let csrf = csrf.get_value();
        spawn_local(async move {
            let file = file.take();
            if file.size() > 2.0 * 1024.0 * 1024.0 {
                import_error.set("Choose an OPML file up to 2 MiB.".into());
                importing.set(false);
                return;
            }
            let text = JsFuture::from(file.text())
                .await
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            match api::send::<ImportResult>(
                &format!("{BASE}/opml"),
                "POST",
                Some(json!({ "text": text })),
                &csrf,
            )
            .await
            {
                Ok(result) => {
                    let folders_part = if result.folders > 0 {
                        format!(
                            " into {} new folder{}",
                            result.folders,
                            if result.folders == 1 { "" } else { "s" }
                        )
                    } else {
                        String::new()
                    };
                    let skipped_part = if result.skipped > 0 {
                        format!("; {} already subscribed", result.skipped)
                    } else {
                        String::new()
                    };
                    import_result.set(format!(
                        "Imported {} feed{}{}{}. New feeds fill in as they update.",
                        result.added,
                        if result.added == 1 { "" } else { "s" },
                        folders_part,
                        skipped_part
                    ));
                    {
                        on_imported.try_run(());
                    };
                }
                Err(e) => import_error.set(e.message),
            }
            importing.set(false);
        });
    };

    view! {
        <SettingsSheet
            title="News settings"
            icon="rss"
            sections=vec!["General", "Import and export", "Keyboard shortcuts"]
            section=section
            on_close=on_close
            notice=saver.notice
        >
            <SaveProblems saver=saver/>
            {move || match section.get() {
                "General" if saver.preferences.with(Option::is_some) => {
                    let addr = address.get_value();
                    let addr_copy = addr.clone();
                    let user = username.get_value();
                    view! {
                        <section class="settings-group" aria-label="Reading">
                            <h3>"Reading"</h3>
                            <SettingRow
                                label="Mark as read"
                                hint="When an article opens, or as it scrolls past in the list"
                                state=saver.state("newsMarkRead")
                            >
                                <Segmented
                                    label="Mark as read"
                                    value=text("newsMarkRead", "open")
                                    options=vec![("open", "On open"), ("scroll", "On scroll")]
                                    on_change=Callback::new(move |value: String| {
                                        let msg = if value == "open" {
                                            "Articles are marked read when opened"
                                        } else {
                                            "Articles are marked read as they scroll past"
                                        };
                                        saver.change("newsMarkRead", json!(value), msg.into(), true);
                                    })
                                />
                            </SettingRow>
                            <SettingRow label="Order" hint="" state=saver.state("newsOrder")>
                                <Segmented
                                    label="Order"
                                    value=text("newsOrder", "newest")
                                    options=vec![("newest", "Newest first"), ("oldest", "Oldest first")]
                                    on_change=Callback::new(move |value: String| {
                                        let msg = format!("Showing {value} articles first");
                                        saver.change("newsOrder", json!(value), msg, true);
                                    })
                                />
                            </SettingRow>
                            <SettingRow label="Layout" hint="" state=saver.state("newsLayout")>
                                <Segmented
                                    label="Layout"
                                    value=text("newsLayout", "split")
                                    options=vec![("split", "Split"), ("cards", "Cards")]
                                    on_change=Callback::new(move |value: String| {
                                        let msg = if value == "cards" {
                                            "Magazine cards layout"
                                        } else {
                                            "Split reading layout"
                                        };
                                        saver.change("newsLayout", json!(value), msg.into(), true);
                                    })
                                />
                            </SettingRow>
                            <SettingSwitch
                                label="Show read articles in feeds and folders"
                                hint="Unread always shows only unread articles"
                                state=saver.state("newsShowRead")
                                checked=flag("newsShowRead", false)
                                on_change=Callback::new(move |value: bool| {
                                    saver.change(
                                        "newsShowRead",
                                        json!(value),
                                        if value { "Read articles shown" } else { "Read articles hidden" }.into(),
                                        true,
                                    );
                                })
                            />
                            <SettingRow
                                label="Remote images"
                                hint="Through Dar keeps sites from seeing when you read"
                                state=saver.state("newsImages")
                            >
                                <Segmented
                                    label="Remote images"
                                    value=text("newsImages", "proxy")
                                    options=vec![("proxy", "Through Dar"), ("block", "Block")]
                                    on_change=Callback::new(move |value: String| {
                                        let msg = if value == "proxy" {
                                            "Images load through Dar"
                                        } else {
                                            "Images blocked"
                                        };
                                        saver.change("newsImages", json!(value), msg.into(), true);
                                    })
                                />
                            </SettingRow>
                        </section>
                        <section class="settings-group" aria-label="Updates">
                            <h3>"Updates"</h3>
                            <SettingRow
                                label="Check feeds"
                                hint="Failing feeds back off automatically"
                                state=saver.state("newsRefresh")
                            >
                                <Select
                                    aria_label="Check feeds"
                                    value=refresh_val
                                    options=refresh_options.get_value()
                                    on_change=Callback::new(move |v: String| {
                                        if let Ok(n) = v.parse::<i64>() {
                                            saver.change("newsRefresh", json!(n), format!("Feeds are checked every {}", minutes(n)), true);
                                        }
                                    })
                                />
                            </SettingRow>
                            <SettingRow
                                label="Keep read articles"
                                hint="Starred articles and articles still in a feed are always kept"
                                state=saver.state("newsKeepDays")
                            >
                                <Select
                                    aria_label="Keep read articles"
                                    value=keep_val
                                    options=keep_options.get_value()
                                    on_change=Callback::new(move |v: String| {
                                        if let Ok(n) = v.parse::<i64>() {
                                            saver.change("newsKeepDays", json!(n), format!("Read articles are kept for {n} days"), true);
                                        }
                                    })
                                />
                            </SettingRow>
                        </section>
                        <section class="settings-group" aria-label="Mobile apps">
                            <h3>"Mobile apps"</h3>
                            <div class="settings-row is-stacked">
                                <div class="settings-row-text">
                                    <span class="settings-row-label">"Nextcloud News apps"</span>
                                    <small>
                                        "Use server address "<code>{addr}</code>
                                        ", your username "<code>{user}</code>
                                        " and an app password with News access."
                                    </small>
                                </div>
                                <div class="settings-row-control">
                                    <span class="news-sync">
                                        <button
                                            type="button"
                                            class="secondary-button"
                                            on:click=move |_| {
                                                copy_text(&addr_copy);
                                                saver.notice.set(Some(Notice { message: "Server address copied".into(), undo: None }));
                                            }
                                        >
                                            <Icon name="copy"/>"Copy address"
                                        </button>
                                        <a class="secondary-button" href="/settings?section=security">"App passwords"</a>
                                    </span>
                                </div>
                            </div>
                        </section>
                    }.into_any()
                }
                "Import and export" => view! {
                    <section class="settings-group" aria-label="Import and export">
                        <h3>"Subscriptions"</h3>
                        <SettingRow
                            label="Import OPML"
                            hint="Folders are kept; feeds you already follow are skipped"
                            state=Signal::derive(|| None)
                            stacked=true
                        >
                            <label class="secondary-button news-file">
                                <Icon name="upload"/>
                                {move || if importing.get() { "Importing\u{2026}" } else { "Choose OPML file" }}
                                <input
                                    type="file"
                                    accept=".opml,.xml,text/xml,text/x-opml,application/xml"
                                    disabled=move || importing.get()
                                    on:change=move |e| {
                                        if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                            let file = input.files().and_then(|list| list.item(0));
                                            input.set_value("");
                                            import_file(file);
                                        }
                                    }
                                />
                            </label>
                        </SettingRow>
                        {move || (!import_result.with(String::is_empty)).then(|| view! {
                            <p class="news-import-result" role="status">{import_result.get()}</p>
                        })}
                        {move || (!import_error.with(String::is_empty)).then(|| view! {
                            <p class="error" role="alert">{import_error.get()}</p>
                        })}
                        <SettingRow
                            label="Export OPML"
                            hint="Every feed and folder, for any other reader"
                            state=Signal::derive(|| None)
                        >
                            <a class="secondary-button" href=format!("{BASE}/opml") download=""><Icon name="download"/>"Export"</a>
                        </SettingRow>
                        <SettingRow
                            label="Export starred articles"
                            hint="Titles, links and text as JSON"
                            state=Signal::derive(|| None)
                        >
                            <a class="secondary-button" href=format!("{BASE}/starred") download=""><Icon name="download"/>"Export"</a>
                        </SettingRow>
                    </section>
                }.into_any(),
                "Keyboard shortcuts" => view! {
                    <dl class="ui-shortcuts">
                        <dt>"j / k"</dt><dd>"Next or previous article"</dd>
                        <dt>"n"</dt><dd>"Next unread article"</dd>
                        <dt>"s"</dt><dd>"Star or unstar"</dd>
                        <dt>"m"</dt><dd>"Mark read or unread"</dd>
                        <dt>"o"</dt><dd>"Open the original page"</dd>
                        <dt>"f"</dt><dd>"Toggle focus reading mode"</dd>
                        <dt>"a"</dt><dd>"Reading typography (Aa)"</dd>
                        <dt>"v"</dt><dd>"Switch Split / Magazine cards"</dd>
                        <dt>"r"</dt><dd>"Update feeds"</dd>
                        <dt>"Shift A"</dt><dd>"Mark everything in this view read"</dd>
                        <dt>"Escape"</dt><dd>"Close popover or article"</dd>
                        <dt>"Ctrl / Cmd K"</dt><dd>"Search all apps"</dd>
                    </dl>
                }.into_any(),
                _ => ().into_any(),
            }}
        </SettingsSheet>
    }
}
