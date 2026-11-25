//! Photos discovery views: Albums overview hub, Explore (Places, Cameras, Quick collections,
//! Files folders), Folders browser, and the "+ Add photos" album picker dialog.

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

use super::{
    BASE, PhotoAlbum, PhotoAsset, PhotoExplore, PhotoOverview, PhotoPage, error_message,
    format_number, thumb_url,
};
use crate::api::{self, BrowserSession, format_bytes};
use crate::dates::local_date;
use crate::icons::Icon;
use crate::ui::{ActionMenu, Dialog, MenuAction};

pub(crate) fn album_date_span(album: &PhotoAlbum) -> String {
    if album.min_taken_at <= 0.0 || album.max_taken_at <= 0.0 {
        return String::new();
    }
    let min_str = local_date(album.min_taken_at, &[("month", "short"), ("year", "numeric")]);
    let max_str = local_date(album.max_taken_at, &[("month", "short"), ("year", "numeric")]);
    if min_str == max_str {
        min_str
    } else {
        format!("{min_str} \u{2013} {max_str}")
    }
}

/// Albums overview hub: filter by All / Owned / Shared with me, sort, create new, open album.
#[component]
pub(crate) fn AlbumsHub(
    overview: Signal<Option<PhotoOverview>>,
    on_open_album: Callback<String>,
    on_new_album: Callback<()>,
    album_actions: Callback<PhotoAlbum, Vec<MenuAction>>,
) -> impl IntoView {
    let filter = RwSignal::new("all");
    let sort = RwSignal::new("updated");

    let filtered = Memo::new(move |_| {
        let f = filter.get();
        let s = sort.get();
        let mut list: Vec<PhotoAlbum> = overview.with(|o| {
            let Some(o) = o else {
                return Vec::new();
            };
            match f {
                "owned" => o.albums.clone(),
                "shared" => o.shared_albums.clone(),
                _ => o
                    .albums
                    .iter()
                    .chain(o.shared_albums.iter())
                    .cloned()
                    .collect(),
            }
        });
        match s {
            "name" => list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            "count" => list.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name))),
            _ => list.sort_by(|a, b| {
                b.updated_at
                    .total_cmp(&a.updated_at)
                    .then_with(|| a.name.cmp(&b.name))
            }),
        }
        list
    });

    let total_count = Memo::new(move |_| {
        overview.with(|o| {
            o.as_ref()
                .map(|o| o.albums.len() + o.shared_albums.len())
                .unwrap_or(0)
        })
    });

    view! {
        <div class="photos-list-heading">
            <h1>"Albums"</h1>
            <span class="photos-heading-count">{move || format_number(total_count.get() as u64)}</span>
            <div class="photos-tools">
                <button
                    type="button"
                    class="primary-button photos-head-cta"
                    on:click=move |_| { on_new_album.try_run(()); }
                >
                    <Icon name="plus"/>"New album"
                </button>
            </div>
        </div>
        <div class="photos-subbar" role="region" aria-label="Album filters">
            <div class="photos-filter-chips" role="group" aria-label="Filter albums">
                { [("all", "All"), ("owned", "My albums"), ("shared", "Shared with me")].into_iter().map(|(val, label)| {
                    view! {
                        <button
                            type="button"
                            class="photos-filter-chip"
                            class:is-active=move || filter.get() == val
                            aria-pressed=move || (filter.get() == val).to_string()
                            on:click=move |_| filter.set(val)
                        >
                            {label}
                        </button>
                    }
                }).collect_view() }
            </div>
            <div class="photos-sort-wrap">
                <Icon name="sort"/>
                <label class="sr-only" for="photos-albums-sort">"Sort albums"</label>
                <select
                    id="photos-albums-sort"
                    class="photos-sort-select"
                    prop:value=move || sort.get()
                    on:change=move |e| {
                        let v = event_target_value(&e);
                        sort.set(match v.as_str() {
                            "name" => "name",
                            "count" => "count",
                            _ => "updated",
                        });
                    }
                >
                    <option value="updated">"Recently updated"</option>
                    <option value="name">"Name (A\u{2013}Z)"</option>
                    <option value="count">"Most photos"</option>
                </select>
            </div>
        </div>
        <div class="photos-grid-scroll">
            {move || {
                let list = filtered.get();
                if list.is_empty() {
                    let msg = match filter.get() {
                        "shared" => "Albums other people on this Dar share with you appear here.",
                        _ => "Group trips, events and favorite moments into albums. Photos stay right where they are in Files.",
                    };
                    view! {
                        <div class="photos-empty">
                            <Icon name="albums"/>
                            <h2>"No albums yet"</h2>
                            <p>{msg}</p>
                            {(filter.get() != "shared").then(|| view! {
                                <button
                                    type="button"
                                    class="primary-button"
                                    on:click=move |_| { on_new_album.try_run(()); }
                                >
                                    <Icon name="plus"/>"Create album"
                                </button>
                            })}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <ul class="photos-album-cards is-hub" aria-label="Albums list">
                            {list.into_iter().map(|a| {
                                let id = a.id.clone();
                                let span = album_date_span(&a);
                                let count_txt = format!("{} item{}", format_number(a.count), if a.count == 1 { "" } else { "s" });
                                let meta = if let Some(owner) = &a.owner {
                                    format!("{} \u{00b7} {count_txt}", owner.name)
                                } else if !span.is_empty() {
                                    format!("{count_txt} \u{00b7} {span}")
                                } else {
                                    count_txt
                                };
                                let is_shared = a.shared || a.owner.is_some();
                                let menu_label = format!("Actions for {}", a.name);
                                let a_menu = a.clone();
                                view! {
                                    <li class="photos-album-card-wrap">
                                        <button
                                            type="button"
                                            class="photos-album-card"
                                            on:click=move |_| { on_open_album.try_run(id.clone()); }
                                        >
                                            <span class="photos-album-card-cover is-wide">
                                                {if !a.cover_id.is_empty() {
                                                    view! { <img src=thumb_url(&a.cover_id, "s") alt="" loading="lazy"/> }.into_any()
                                                } else {
                                                    view! { <Icon name="image"/> }.into_any()
                                                }}
                                                {is_shared.then(|| view! {
                                                    <span class="photos-album-card-badge" title="Shared">
                                                        <Icon name="share"/>
                                                        {if a.member_count > 0 {
                                                            format!("{}", a.member_count + 1)
                                                        } else {
                                                            "Shared".to_owned()
                                                        }}
                                                    </span>
                                                })}
                                            </span>
                                            <span class="photos-album-card-title">{a.name.clone()}</span>
                                            <small>{meta}</small>
                                        </button>
                                        <ActionMenu
                                            label=menu_label
                                            class="photos-album-card-menu"
                                            actions=move |_| album_actions.run(a_menu.clone())
                                        >
                                            <Icon name="more"/>
                                        </ActionMenu>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_any()
                }
            }}
        </div>
    }
}

/// Explore view: zero-ML discovery cards powered by EXIF GPS, camera model, Files folders & sizes.
#[component]
pub(crate) fn ExploreView(
    on_open_recent: Callback<()>,
    on_open_large: Callback<()>,
    on_open_unassigned: Callback<()>,
    on_open_folders: Callback<()>,
    on_open_camera: Callback<String>,
    on_open_place: Callback<(String, String)>,
    on_open_folder: Callback<(String, String)>,
) -> impl IntoView {
    let data: RwSignal<Option<PhotoExplore>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());

    let load = move || {
        error.set(String::new());
        spawn_local(async move {
            match api::get::<PhotoExplore>(&format!("{BASE}/explore")).await {
                Ok(val) => {
                    data.try_set(Some(val));
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
        });
    };
    load();

    view! {
        <div class="photos-list-heading">
            <h1>"Explore"</h1>
            {move || data.with(|d| d.as_ref().map(|d| d.total_bytes)).filter(|&b| b > 0.0).map(|b| view! {
                <span class="photos-heading-count">{format_bytes(b)}</span>
            })}
        </div>
        <div class="photos-grid-scroll photos-explore">
            {move || (!error.get().is_empty()).then(|| view! {
                <div class="photos-empty" role="alert">
                    <h2>"Explore could not be loaded"</h2>
                    <p>{error.get()}</p>
                    <button type="button" class="secondary-button" on:click=move |_| load()>"Retry"</button>
                </div>
            })}
            {move || (data.with(Option::is_none) && error.get().is_empty()).then(|| view! {
                <p class="photos-empty" role="status">"Scanning your library\u{2026}"</p>
            })}
            {move || data.get().map(|ex| {
                let has_places = !ex.places.is_empty();
                let has_cameras = !ex.cameras.is_empty();
                let has_folders = !ex.folders.is_empty();
                let folder_preview: Vec<_> = ex.folders.iter().take(6).cloned().collect();
                let folders_total = ex.folders.len();
                view! {
                    <section class="photos-explore-section" aria-label="Quick collections">
                        <div class="photos-explore-head">
                            <h2>"Quick collections"</h2>
                        </div>
                        <div class="photos-quick-grid">
                            <button type="button" class="photos-quick-card" on:click=move |_| { on_open_recent.try_run(()); }>
                                <span class="photos-quick-icon"><Icon name="clock"/></span>
                                <span class="photos-quick-text">
                                    <strong>"Recently added"</strong>
                                    <small>"Latest uploads & imports"</small>
                                </span>
                            </button>
                            <button type="button" class="photos-quick-card" on:click=move |_| { on_open_unassigned.try_run(()); }>
                                <span class="photos-quick-icon"><Icon name="albums"/></span>
                                <span class="photos-quick-text">
                                    <strong>"Not in any album"</strong>
                                    <small>{format!("{} item{}", format_number(ex.unassigned_count), if ex.unassigned_count == 1 { "" } else { "s" })}</small>
                                </span>
                            </button>
                            <button type="button" class="photos-quick-card" on:click=move |_| { on_open_large.try_run(()); }>
                                <span class="photos-quick-icon"><Icon name="archive2"/></span>
                                <span class="photos-quick-text">
                                    <strong>"Large files"</strong>
                                    <small>{format!("{} over 5 MB", format_number(ex.large_count))}</small>
                                </span>
                            </button>
                            <button type="button" class="photos-quick-card" on:click=move |_| { on_open_folders.try_run(()); }>
                                <span class="photos-quick-icon"><Icon name="folder"/></span>
                                <span class="photos-quick-text">
                                    <strong>"Files folders"</strong>
                                    <small>{format!("{} folder{}", format_number(folders_total as u64), if folders_total == 1 { "" } else { "s" })}</small>
                                </span>
                            </button>
                        </div>
                    </section>

                    <section class="photos-explore-section" aria-label="Places">
                        <div class="photos-explore-head">
                            <h2>"Places"</h2>
                            <span class="photos-heading-count">
                                {if has_places { format!("{} locations", ex.places.len()) } else { "From EXIF GPS".to_owned() }}
                            </span>
                        </div>
                        {if has_places {
                            view! {
                                <div class="photos-explore-cards">
                                    {ex.places.into_iter().map(|p| {
                                        let label = format!("{:.2}\u{00b0}, {:.2}\u{00b0}", p.latitude, p.longitude);
                                        let key = p.key.clone();
                                        let label_click = label.clone();
                                        let count_str = format!("{} item{}", format_number(p.count), if p.count == 1 { "" } else { "s" });
                                        view! {
                                            <button
                                                type="button"
                                                class="photos-explore-card"
                                                on:click=move |_| { on_open_place.try_run((key.clone(), label_click.clone())); }
                                            >
                                                <span class="photos-explore-thumb">
                                                    <img src=thumb_url(&p.cover_id, "s") alt="" loading="lazy"/>
                                                    <span class="photos-explore-pill"><Icon name="location"/>{count_str}</span>
                                                </span>
                                                <strong>{label}</strong>
                                            </button>
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <p class="photos-explore-empty">"Photos with GPS coordinates in their EXIF metadata are automatically grouped here by location."</p>
                            }.into_any()
                        }}
                    </section>

                    {has_cameras.then(|| view! {
                        <section class="photos-explore-section" aria-label="Cameras and lenses">
                            <div class="photos-explore-head">
                                <h2>"Cameras"</h2>
                                <span class="photos-heading-count">{ex.cameras.len()}</span>
                            </div>
                            <div class="photos-explore-cards">
                                {ex.cameras.into_iter().map(|c| {
                                    let name_click = c.name.clone();
                                    let count_str = format!("{} item{}", format_number(c.count), if c.count == 1 { "" } else { "s" });
                                    view! {
                                        <button
                                            type="button"
                                            class="photos-explore-card"
                                            on:click=move |_| { on_open_camera.try_run(name_click.clone()); }
                                        >
                                            <span class="photos-explore-thumb">
                                                <img src=thumb_url(&c.cover_id, "s") alt="" loading="lazy"/>
                                                <span class="photos-explore-pill"><Icon name="camera"/>{count_str}</span>
                                            </span>
                                            <strong>{c.name}</strong>
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        </section>
                    })}

                    {has_folders.then(|| view! {
                        <section class="photos-explore-section" aria-label="Folders in Files">
                            <div class="photos-explore-head">
                                <h2>"Folders in Files"</h2>
                                {(folders_total > 6).then(|| view! {
                                    <button type="button" class="text-button" on:click=move |_| { on_open_folders.try_run(()); }>
                                        "View all "{folders_total}
                                    </button>
                                })}
                            </div>
                            <div class="photos-explore-cards">
                                {folder_preview.into_iter().map(|f| {
                                    let id = f.id.clone();
                                    let path = f.path.clone();
                                    let count_str = format!("{} item{}", format_number(f.count), if f.count == 1 { "" } else { "s" });
                                    view! {
                                        <button
                                            type="button"
                                            class="photos-explore-card"
                                            on:click=move |_| { on_open_folder.try_run((id.clone(), path.clone())); }
                                        >
                                            <span class="photos-explore-thumb">
                                                <img src=thumb_url(&f.cover_id, "s") alt="" loading="lazy"/>
                                                <span class="photos-explore-pill"><Icon name="folder"/>{count_str}</span>
                                            </span>
                                            <strong>{f.name}</strong>
                                            <small>{f.path}</small>
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        </section>
                    })}
                }
            })}
        </div>
    }
}

/// Folders view: browse all Files folders containing photos or videos.
#[component]
pub(crate) fn FoldersView(on_open_folder: Callback<(String, String)>) -> impl IntoView {
    let data: RwSignal<Option<PhotoExplore>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());

    let load = move || {
        error.set(String::new());
        spawn_local(async move {
            match api::get::<PhotoExplore>(&format!("{BASE}/explore")).await {
                Ok(val) => {
                    data.try_set(Some(val));
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
        });
    };
    load();

    view! {
        <div class="photos-list-heading">
            <h1>"Folders"</h1>
            {move || data.with(|d| d.as_ref().map(|d| d.folders.len())).map(|n| view! {
                <span class="photos-heading-count">{format_number(n as u64)}</span>
            })}
        </div>
        <div class="photos-grid-scroll">
            {move || (!error.get().is_empty()).then(|| view! {
                <div class="photos-empty" role="alert">
                    <h2>"Folders could not be loaded"</h2>
                    <p>{error.get()}</p>
                    <button type="button" class="secondary-button" on:click=move |_| load()>"Retry"</button>
                </div>
            })}
            {move || (data.with(Option::is_none) && error.get().is_empty()).then(|| view! {
                <p class="photos-empty" role="status">"Loading folders\u{2026}"</p>
            })}
            {move || data.get().map(|ex| {
                if ex.folders.is_empty() {
                    view! {
                        <div class="photos-empty">
                            <Icon name="folder"/>
                            <h2>"No photo folders yet"</h2>
                            <p>"Folders in Files that hold photos or videos appear here automatically."</p>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <ul class="photos-album-cards is-hub" aria-label="Folders containing photos">
                            {ex.folders.into_iter().map(|f| {
                                let id = f.id.clone();
                                let path_click = f.path.clone();
                                let meta = format!("{} \u{00b7} {} item{}", f.path, format_number(f.count), if f.count == 1 { "" } else { "s" });
                                view! {
                                    <li class="photos-album-card-wrap">
                                        <button
                                            type="button"
                                            class="photos-album-card"
                                            on:click=move |_| { on_open_folder.try_run((id.clone(), path_click.clone())); }
                                        >
                                            <span class="photos-album-card-cover is-wide">
                                                <img src=thumb_url(&f.cover_id, "s") alt="" loading="lazy"/>
                                            </span>
                                            <span class="photos-album-card-title">{f.name}</span>
                                            <small>{meta}</small>
                                        </button>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_any()
                }
            })}
        </div>
    }
}

/// Dialog for picking library photos directly from inside an album ("+ Add photos").
#[component]
pub(crate) fn AlbumPickerDialog(
    album: PhotoAlbum,
    existing_ids: HashSet<String>,
    session: BrowserSession,
    on_close: Callback<()>,
    on_added: Callback<u64>,
) -> impl IntoView {
    let items: RwSignal<Option<Vec<PhotoAsset>>> = RwSignal::new(None);
    let chosen: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let existing = StoredValue::new(existing_ids);
    let album_sv = StoredValue::new(album.clone());
    let session_sv = StoredValue::new(session);

    spawn_local(async move {
        match api::get::<PhotoPage>(&format!("{BASE}/assets?view=library&limit=200")).await {
            Ok(page) => {
                let filtered: Vec<PhotoAsset> = existing.with_value(|ex| {
                    page.items
                        .into_iter()
                        .filter(|a| !ex.contains(&a.id))
                        .collect()
                });
                items.try_set(Some(filtered));
            }
            Err(cause) => {
                error.try_set(error_message(&cause.message));
            }
        }
    });

    let toggle = move |id: String| {
        chosen.update(|s| {
            if !s.remove(&id) {
                s.insert(id);
            }
        });
    };

    let submit = move |_| {
        let ids: Vec<String> = chosen.with_untracked(|s| s.iter().cloned().collect());
        if ids.is_empty() {
            return;
        }
        busy.set(true);
        error.set(String::new());
        let album_id = album_sv.with_value(|a| a.id.clone());
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            #[derive(serde::Deserialize)]
            struct AddedRes {
                #[serde(default)]
                added: u64,
            }
            match api::send::<AddedRes>(
                &format!("{BASE}/albums/{album_id}/add"),
                "POST",
                Some(json!({ "ids": ids })),
                &csrf,
            )
            .await
            {
                Ok(res) => {
                    on_added.try_run(res.added);
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                    busy.try_set(false);
                }
            }
        });
    };

    let title = format!("Add photos to {}", album.name);
    view! {
        <Dialog title=title on_close=on_close>
            <div class="photos-picker">
                {move || (!error.get().is_empty()).then(|| view! {
                    <p role="alert" class="error">{error.get()}</p>
                })}
                {move || match items.get() {
                    None => view! { <p class="photos-hint">"Loading library\u{2026}"</p> }.into_any(),
                    Some(list) if list.is_empty() => view! {
                        <p class="photos-hint">"All your recent library photos are already in this album."</p>
                    }.into_any(),
                    Some(list) => view! {
                        <div class="photos-picker-grid" role="group" aria-label="Choose photos to add">
                            {list.into_iter().map(|item| {
                                let id_cls = item.id.clone();
                                let id_chk = item.id.clone();
                                let id_tog = item.id.clone();
                                let label = format!("Add {}", item.name);
                                view! {
                                    <button
                                        type="button"
                                        class="photos-picker-tile"
                                        class:is-selected=move || chosen.with(|s| s.contains(&id_cls))
                                        aria-pressed=move || chosen.with(|s| s.contains(&id_chk)).to_string()
                                        aria-label=label
                                        on:click=move |_| toggle(id_tog.clone())
                                    >
                                        <img src=thumb_url(&item.id, "s") alt="" loading="lazy"/>
                                        <span class="photos-picker-check" aria-hidden="true"><Icon name="check"/></span>
                                    </button>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any(),
                }}
                <div class="dialog-actions">
                    <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                    <button
                        type="button"
                        class="primary-button"
                        disabled=move || busy.get() || chosen.with(HashSet::is_empty)
                        on:click=submit
                    >
                        {move || {
                            let n = chosen.with(HashSet::len);
                            if busy.get() {
                                "Adding\u{2026}".to_owned()
                            } else if n > 0 {
                                format!("Add {n} photo{}", if n == 1 { "" } else { "s" })
                            } else {
                                "Add photos".to_owned()
                            }
                        }}
                    </button>
                </div>
            </div>
        </Dialog>
    }
}
