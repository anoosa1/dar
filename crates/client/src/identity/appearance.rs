//! Account theme hook and Personalization (Appearance + App menu) settings (`appearance.tsx`).

use leptos::prelude::*;

use crate::api::BrowserSession;
use crate::icons::Icon;
use crate::settings::{SaveProblems, SaveState, Saver};
use crate::shared::{AppLauncherCategory, AppLauncherPrefs, Context};
use crate::shell::{app_list, app_tint_class};
use crate::ui::{listen, unique_id, window};

const APPEARANCES: [(&str, &str); 3] = [
    ("system", "Follow system"),
    ("latte", "Light \u{2014} Twilight Afterglow"),
    ("mocha", "Dark \u{2014} Twilight Nightfall"),
];

fn apply_theme(value: &str) {
    let dark = window()
        .match_media("(prefers-color-scheme: dark)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches());
    let theme = if value == "system" {
        if dark { "mocha" } else { "latte" }
    } else {
        value
    };
    if let Some(root) = window().document().and_then(|d| d.document_element()) {
        let _ = root.set_attribute("data-theme", theme);
    }
}

/// Applies the account's appearance preference (or system theme when signed out or `"system"`)
/// to `document.documentElement.dataset.theme` and tracks OS theme changes.
pub fn use_appearance(session: Signal<Option<BrowserSession>>) {
    let current = RwSignal::new("system".to_owned());
    let shared = use_context::<Context>();
    Effect::new(move |_| {
        let user_id = session.with(|s| s.as_ref().map(|s| s.user.id.clone()));
        let mut value = user_id
            .as_deref()
            .and_then(|id| {
                let key = format!("dar.appearance.{id}");
                window()
                    .local_storage()
                    .ok()
                    .flatten()?
                    .get_item(&key)
                    .ok()
                    .flatten()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "system".to_owned());
        if let (Some(id), Some(ctx)) = (user_id.as_deref(), shared)
            && let Some(p) = ctx.preferences.value.get()
        {
            value = p.appearance.unwrap_or_else(|| "system".to_owned());
            if let Some(s) = window().local_storage().ok().flatten() {
                let _ = s.set_item(&format!("dar.appearance.{id}"), &value);
            }
        }
        current.set(value.clone());
        apply_theme(&value);
    });
    if let Ok(Some(media)) = window().match_media("(prefers-color-scheme: dark)") {
        listen(&media, "change", false, move |_| {
            apply_theme(&current.get_untracked());
        });
    }
}

/// Personal Appearance & App menu organizer settings section (`AppearanceSettings`).
#[component]
pub fn AppearanceSettings(session: BrowserSession) -> impl IntoView {
    let saver = Saver::new(session);
    let loaded = Memo::new(move |_| saver.preferences.with(Option::is_some));
    let value = Signal::derive(move || {
        saver
            .value("appearance")
            .as_str()
            .unwrap_or("system")
            .to_owned()
    });
    let state = saver.state("appearance");
    let launcher_state = saver.state("appLauncher");
    let new_cat_name = RwSignal::new(String::new());

    let launcher_prefs = Memo::new(move |_| {
        let raw = saver.value("appLauncher");
        serde_json::from_value::<AppLauncherPrefs>(raw).unwrap_or_default()
    });

    let ordered_apps = Memo::new(move |_| {
        let list = app_list();
        let prefs = launcher_prefs.get();
        let mut out = Vec::with_capacity(list.len());
        for id in &prefs.order {
            if let Some(a) = list.iter().find(|x| &x.id == id)
                && !out.iter().any(|x: &crate::shell::AppEntry| x.id == a.id)
            {
                out.push(a.clone());
            }
        }
        for a in list {
            if !out.iter().any(|x| x.id == a.id) {
                out.push(a);
            }
        }
        out
    });

    let save_launcher = move |next: AppLauncherPrefs, msg: &str| {
        if let Ok(val) = serde_json::to_value(&next) {
            saver.change("appLauncher", val, msg.to_owned(), true);
        }
    };

    let move_app = move |app_id: String, delta: isize| {
        let mut prefs = launcher_prefs.get_untracked();
        let mut order: Vec<String> = ordered_apps
            .get_untracked()
            .into_iter()
            .map(|a| a.id)
            .collect();
        let Some(idx) = order.iter().position(|id| id == &app_id) else {
            return;
        };
        let next_idx = (idx as isize + delta).clamp(0, order.len().saturating_sub(1) as isize) as usize;
        if next_idx == idx {
            return;
        }
        order.swap(idx, next_idx);
        prefs.order = order;
        save_launcher(prefs, "App order saved");
    };

    let assign_category = move |app_id: String, cat_id: String| {
        let mut prefs = launcher_prefs.get_untracked();
        for cat in &mut prefs.categories {
            cat.apps.retain(|id| id != &app_id);
            if cat.id == cat_id {
                cat.apps.push(app_id.clone());
            }
        }
        save_launcher(prefs, "App category updated");
    };

    let add_category = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        let name = new_cat_name.get_untracked().trim().to_owned();
        if name.is_empty() {
            return;
        }
        let mut prefs = launcher_prefs.get_untracked();
        if prefs.categories.len() >= 24 {
            return;
        }
        prefs.categories.push(AppLauncherCategory {
            id: format!("cat-{}", unique_id()),
            name,
            apps: Vec::new(),
        });
        new_cat_name.set(String::new());
        save_launcher(prefs, "Category added");
    };

    let delete_category = move |cat_id: String| {
        let mut prefs = launcher_prefs.get_untracked();
        prefs.categories.retain(|c| c.id != cat_id);
        save_launcher(prefs, "Category removed");
    };

    let reset_launcher = move |_| {
        save_launcher(AppLauncherPrefs::default(), "App menu reset");
    };

    view! {
        <h1>"Appearance"</h1>
        <SaveProblems saver=saver/>
        {move || {
            if !loaded.get() {
                view! { <p role="status">"Loading appearance\u{2026}"</p> }.into_any()
            } else {
                view! {
                    <fieldset class="appearance-options">
                        <legend>"Color theme"</legend>
                        {APPEARANCES.into_iter().map(|(id, label)| {
                            view! {
                                <label class=move || if value.get() == id { "appearance-option is-checked" } else { "appearance-option" }>
                                    <input
                                        type="radio"
                                        name="appearance"
                                        value=id
                                        prop:checked=move || value.get() == id
                                        on:change=move |_| saver.change("appearance", serde_json::json!(id), "Appearance saved".into(), false)
                                    />
                                    <span>{label}</span>
                                    <span class=format!("theme-swatch swatch-{id}") aria-hidden="true"></span>
                                </label>
                            }
                        }).collect_view()}
                    </fieldset>
                    <p class="field-hint">"Applies to every app and saves for your account as you choose."</p>
                    <p role="status" class="settings-inline-status">
                        {move || match state.get() {
                            Some(SaveState::Saving) => "Saving\u{2026}",
                            Some(SaveState::Saved) => "Appearance saved",
                            _ => "",
                        }}
                    </p>

                    <section class="launcher-organizer" aria-labelledby="app-menu-heading">
                        <div class="launcher-organizer-head">
                            <div>
                                <h2 id="app-menu-heading">"App menu"</h2>
                                <p class="field-hint">"Reorder apps and optionally group them into categories. Without categories, every app appears under Apps."</p>
                            </div>
                            <button type="button" class="secondary-button" on:click=reset_launcher>"Reset default"</button>
                        </div>

                        <div class="launcher-categories-box">
                            <h3>"Categories"</h3>
                            <form class="launcher-cat-form" on:submit=add_category>
                                <input
                                    type="text"
                                    placeholder="New category name (e.g. Library, Work, Code)"
                                    maxlength="48"
                                    prop:value=move || new_cat_name.get()
                                    on:input=move |e| new_cat_name.set(event_target_value(&e))
                                />
                                <button type="submit" class="secondary-button">"Add category"</button>
                            </form>
                            {move || {
                                let cats = launcher_prefs.get().categories;
                                if cats.is_empty() {
                                    view! { <p class="field-hint">"No categories yet — all apps show under Apps."</p> }.into_any()
                                } else {
                                    view! {
                                        <ul class="launcher-cat-chips">
                                            {cats.into_iter().map(|cat| {
                                                let cid = cat.id.clone();
                                                let count = cat.apps.len();
                                                view! {
                                                    <li>
                                                        <span>{cat.name}</span>
                                                        <small>{count}</small>
                                                        <button
                                                            type="button"
                                                            class="icon-button"
                                                            aria-label="Remove category"
                                                            on:click=move |_| delete_category(cid.clone())
                                                        >
                                                            <Icon name="close"/>
                                                        </button>
                                                    </li>
                                                }
                                            }).collect_view()}
                                        </ul>
                                    }.into_any()
                                }
                            }}
                        </div>

                        <ul class="launcher-app-list" aria-label="App order and categories">
                            {move || {
                                let apps = ordered_apps.get();
                                let len = apps.len();
                                let cats = launcher_prefs.get().categories;
                                apps.into_iter().enumerate().map(|(idx, app)| {
                                    let current_cat = cats
                                        .iter()
                                        .find(|c| c.apps.iter().any(|id| id == &app.id))
                                        .map(|c| c.id.clone())
                                        .unwrap_or_default();
                                    let id_up = app.id.clone();
                                    let id_dn = app.id.clone();
                                    let id_cat = app.id.clone();
                                    let tint = app_tint_class(&app.id);
                                    let cats_for_row = cats.clone();
                                    let is_last = idx + 1 >= len;
                                    view! {
                                        <li class="launcher-app-row">
                                            <span class=format!("apps-menu-badge {tint}") aria-hidden="true">
                                                <Icon name=app.icon/>
                                            </span>
                                            <strong class="launcher-app-name">{app.label.clone()}</strong>
                                            {(!cats_for_row.is_empty()).then(|| {
                                                let cur = current_cat.clone();
                                                view! {
                                                    <select
                                                        class="launcher-cat-select"
                                                        aria-label=format!("Category for {}", app.label)
                                                        on:change=move |e| assign_category(id_cat.clone(), event_target_value(&e))
                                                    >
                                                        <option value="" selected=cur.is_empty()>"Other (uncategorized)"</option>
                                                        {cats_for_row.into_iter().map(|c| {
                                                            let sel = c.id == cur;
                                                            view! { <option value=c.id selected=sel>{c.name}</option> }
                                                        }).collect_view()}
                                                    </select>
                                                }
                                            })}
                                            <div class="launcher-order-btns">
                                                <button
                                                    type="button"
                                                    class="icon-button"
                                                    aria-label=format!("Move {} up", app.label)
                                                    disabled=idx == 0
                                                    on:click=move |_| move_app(id_up.clone(), -1)
                                                >
                                                    <Icon name="arrowUp"/>
                                                </button>
                                                <button
                                                    type="button"
                                                    class="icon-button"
                                                    aria-label=format!("Move {} down", app.label)
                                                    disabled=is_last
                                                    on:click=move |_| move_app(id_dn.clone(), 1)
                                                >
                                                    <Icon name="arrowDown"/>
                                                </button>
                                            </div>
                                        </li>
                                    }
                                }).collect_view()
                            }}
                        </ul>

                        <p role="status" class="settings-inline-status">
                            {move || match launcher_state.get() {
                                Some(SaveState::Saving) => "Saving app menu\u{2026}",
                                Some(SaveState::Saved) => "App menu saved",
                                _ => "",
                            }}
                        </p>
                    </section>
                }
                .into_any()
            }
        }}
    }
}
