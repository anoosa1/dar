//! Editorial News article reader: calm split empty-state, sticky header with Aa typography
//! popover (Paper/Sepia/Night/Dar, Editorial Serif/Clean Sans, Compact/Default/Large),
//! on-demand full-text fetch, Clip to Notes, Focus reading mode, scroll reading-progress bar,
//! custom Twilight podcast transport (-15s/+30s/speed/position), and Prev/Next/Next-unread footer.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use super::model::{
    BASE, NewsFeed, NewsItemDetail, NewsState, copy_text, format_clock, has_web_share, initial,
    is_playable_enclosure, is_video_enclosure, mark_class, web_share,
};
use super::render::render_article;
use crate::api;
use crate::dates::local_date_time;
use crate::icons::Icon;
use crate::ui::{ActionMenu, MenuAction, window};

#[component]
pub fn NewsReader(
    state: NewsState,
    item: Signal<Option<NewsItemDetail>>,
    feed: Signal<Option<NewsFeed>>,
) -> impl IntoView {
    let body_ref = NodeRef::<html::Div>::new();
    let scroll_ref = NodeRef::<html::Article>::new();
    let video_ref = NodeRef::<html::Video>::new();
    let audio_ref = NodeRef::<html::Audio>::new();
    let saved = RwSignal::new(0.0f64);
    let words = RwSignal::new(0usize);
    let read_progress = RwSignal::new(0.0f64);
    let plain_text = RwSignal::new(String::new());

    // Podcast audio player reactive state
    let playing = RwSignal::new(false);
    let audio_time = RwSignal::new(0.0f64);
    let audio_duration = RwSignal::new(0.0f64);
    let playback_rate = RwSignal::new(1.0f64);

    let has_error = Memo::new(move |_| !state.article_error.with(String::is_empty));
    let item_id = Memo::new(move |_| item.with(|i| i.as_ref().map(|i| i.id)));
    let has_selection = Memo::new(move |_| state.selected.get().is_some());
    let render_spec = Memo::new(move |_| {
        item.with(|i| {
            i.as_ref()
                .map(|i| (i.id, i.body.clone(), i.excerpt.clone(), state.news_images.get()))
        })
    });
    let enclosure = Memo::new(move |_| {
        item.with(|i| {
            i.as_ref().map(|i| {
                (
                    i.enclosure_link.clone(),
                    i.enclosure_mime.clone(),
                    i.media_description.clone(),
                    i.excerpt.clone(),
                )
            })
        })
    });

    // Rebuild sanitized DOM fragment into `.news-body` whenever article body or image setting changes.
    Effect::new(move |_| {
        let Some(el) = body_ref.get() else { return };
        let spec = render_spec.get();
        while let Some(first) = el.first_child() {
            let _ = el.remove_child(&first);
        }
        if let Some((_, html, excerpt, imgs)) = spec {
            let frag = render_article(&html, &imgs);
            let _ = el.append_child(&frag);
            let text = el.text_content().unwrap_or(excerpt);
            words.set(text.split_whitespace().count());
            plain_text.set(text);
        } else {
            words.set(0);
            plain_text.set(String::new());
        }
        read_progress.set(0.0);
        if let Some(sc) = scroll_ref.get_untracked() {
            sc.set_scroll_top(0);
        }
    });

    // On phones, move focus to the article heading when an article opens.
    Effect::new(move |_| {
        if item_id.get().is_none() {
            return;
        }
        let Some(body_el) = body_ref.get() else {
            return;
        };
        if window()
            .match_media("(max-width:800px)")
            .ok()
            .flatten()
            .is_some_and(|m| m.matches())
            && let Some(h1) = body_el
                .parent_element()
                .and_then(|p| p.query_selector("h1").ok().flatten())
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = h1.focus();
        }
    });

    // Restore saved media playback position when switching articles.
    Effect::new(move |_| {
        let _ = item_id.get();
        let pos = item.with_untracked(|i| i.as_ref().map(|i| i.position).unwrap_or(0.0));
        saved.set(pos);
        audio_time.set(pos);
        audio_duration.set(0.0);
        playing.set(false);
        playback_rate.set(1.0);
        if pos > 0.0 {
            if let Some(v) = video_ref.get() {
                v.set_current_time(pos);
            }
            if let Some(a) = audio_ref.get() {
                a.set_current_time(pos);
            }
        }
    });

    let remember = move || {
        let current_time = video_ref
            .get_untracked()
            .map(|v| v.current_time())
            .or_else(|| audio_ref.get_untracked().map(|a| a.current_time()));
        let Some(ct) = current_time else { return };
        audio_time.set(ct);
        let Some(id) = item_id.get_untracked() else {
            return;
        };
        if (ct - saved.get_untracked()).abs() < 15.0 {
            return;
        }
        saved.set(ct);
        let csrf = state.csrf();
        spawn_local(async move {
            let _ = api::send::<Value>(
                &format!("{BASE}/items/{id}/position"),
                "POST",
                Some(json!({ "position": ct.floor() as i64 })),
                &csrf,
            )
            .await;
        });
    };

    let share_actions = move |_| {
        let Some((title, url)) = item.with_untracked(|i| {
            i.as_ref()
                .filter(|i| !i.url.is_empty())
                .map(|i| (i.title.clone(), i.url.clone()))
        }) else {
            return Vec::new();
        };
        let copy_url = url.clone();
        let mut actions = vec![MenuAction::new("Copy link", "copy").run(move || {
            copy_text(&copy_url);
            state.notify("Article link copied");
        })];
        if has_web_share() {
            actions.push(
                MenuAction::new("Share\u{2026}", "share").run(move || web_share(&title, &url)),
            );
        }
        actions
    };

    let papers: [(&'static str, &'static str, &'static str); 4] = [
        ("dar", "Dar", "is-dar"),
        ("paper", "Paper", "is-paper"),
        ("sepia", "Sepia", "is-sepia"),
        ("night", "Night", "is-night"),
    ];
    let fonts: [(&'static str, &'static str); 2] =
        [("serif", "Editorial Serif"), ("sans", "Clean Sans")];
    let sizes: [(&'static str, &'static str); 3] =
        [("sm", "Compact"), ("md", "Default"), ("lg", "Large")];

    view! {
        <section
            class=move || format!(
                "news-reader news-paper-{} news-font-{} news-size-{}",
                state.news_paper.get(),
                state.news_font.get(),
                state.news_size.get()
            )
        >
            <Show
                when=move || has_selection.get()
                fallback=move || view! {
                    <div class="news-reader-empty">
                        <Icon name="rss"/>
                        <h2>"Select an article to read"</h2>
                        <p>"Choose any item from the list, or press "<kbd>"j"</kbd>" / "<kbd>"n"</kbd>" to start reading."</p>
                    </div>
                }
            >
                <div class="news-reader-head">
                    <button
                        type="button"
                        class="icon-button news-back"
                        aria-label="Back to articles"
                        on:click=move |_| {
                            state.selected.set(None);
                            state.article.set(None);
                            state.focus_mode.set(false);
                        }
                    >
                        <Icon name="back"/>
                    </button>
                    {move || state.focus_mode.get().then(|| view! {
                        <button
                            type="button"
                            class="icon-button news-exit-focus"
                            aria-label="Exit focus reading"
                            title="Exit focus reading (f)"
                            on:click=move |_| state.focus_mode.set(false)
                        >
                            <Icon name="back"/>
                        </button>
                    })}
                    {move || feed.get().map(|f| view! {
                        <span class="news-reader-feed">
                            <span class=mark_class(f.id) aria-hidden="true">{initial(&f.title)}</span>
                            <span class="news-reader-feed-name">{f.title}</span>
                        </span>
                    })}
                    <Show when=move || item_id.get().is_some()>
                        <div class="news-reader-tools">
                            <button
                                type="button"
                                class=move || if state.aa_open.get() { "icon-button is-active" } else { "icon-button" }
                                aria-expanded=move || state.aa_open.get().to_string()
                                aria-label="Reading typography"
                                title="Reading typography (a)"
                                on:click=move |e| {
                                    e.stop_propagation();
                                    state.aa_open.update(|v| *v = !*v);
                                }
                            >
                                <Icon name="typography"/>
                            </button>
                            {move || item.with(|i| i.as_ref().is_some_and(|i| !i.url.is_empty())).then(|| view! {
                                <button
                                    type="button"
                                    class=move || if state.fulltext_fetched.get() == item_id.get() { "icon-button is-active" } else { "icon-button" }
                                    disabled=move || state.fulltext_loading.get()
                                    aria-label="Fetch full text"
                                    title="Fetch full article text"
                                    on:click=move |_| state.fetch_fulltext()
                                >
                                    <Icon name="file"/>
                                </button>
                            })}
                            <button
                                type="button"
                                class="icon-button"
                                aria-label="Clip to Notes"
                                title="Save clipping to Dar Notes"
                                on:click=move |_| {
                                    let f_title = feed.with_untracked(|f| f.as_ref().map(|f| f.title.clone()).unwrap_or_default());
                                    let text = plain_text.get_untracked();
                                    state.clip_to_notes(f_title, text);
                                }
                            >
                                <Icon name="bookmark"/>
                            </button>
                            <button
                                type="button"
                                class=move || if item.with(|i| i.as_ref().is_some_and(|i| i.starred)) { "icon-button is-starred" } else { "icon-button" }
                                aria-pressed=move || item.with(|i| i.as_ref().is_some_and(|i| i.starred)).to_string()
                                aria-label="Star"
                                title="Star (s)"
                                on:click=move |_| {
                                    if let Some(art) = item.get_untracked() {
                                        state.set_state(vec![(art.id, art.feed_id)], None, Some(!art.starred));
                                    }
                                }
                            >
                                <Icon name="star"/>
                            </button>
                            <button
                                type="button"
                                class="icon-button"
                                aria-pressed=move || item.with(|i| i.as_ref().is_some_and(|i| i.unread)).to_string()
                                aria-label="Keep unread"
                                title="Keep unread (m)"
                                on:click=move |_| {
                                    if let Some(art) = item.get_untracked() {
                                        state.set_state(vec![(art.id, art.feed_id)], Some(!art.unread), None);
                                    }
                                }
                            >
                                <Icon name="unread"/>
                            </button>
                            {move || item.with(|i| i.as_ref().filter(|i| !i.url.is_empty()).map(|i| i.url.clone())).map(|url| view! {
                                <a class="icon-button" href=url target="_blank" rel="noopener noreferrer" aria-label="Open original" title="Open original (o)">
                                    <Icon name="external"/>
                                </a>
                            })}
                            {move || item.with(|i| i.as_ref().is_some_and(|i| !i.url.is_empty())).then(|| view! {
                                <ActionMenu label="Share" actions=share_actions><Icon name="share"/></ActionMenu>
                            })}
                            <button
                                type="button"
                                class=move || if state.focus_mode.get() { "icon-button news-focus-btn is-active" } else { "icon-button news-focus-btn" }
                                aria-pressed=move || state.focus_mode.get().to_string()
                                aria-label="Focus reading"
                                title="Focus reading (f)"
                                on:click=move |_| state.focus_mode.update(|v| *v = !*v)
                            >
                                <Icon name="expand"/>
                            </button>
                            <button
                                type="button"
                                class="icon-button news-close"
                                aria-label="Close article"
                                on:click=move |_| {
                                    state.selected.set(None);
                                    state.article.set(None);
                                    state.focus_mode.set(false);
                                }
                            >
                                <Icon name="close"/>
                            </button>
                        </div>
                    </Show>

                    <progress
                        class="news-read-progress"
                        max="100"
                        prop:value=move || read_progress.get()
                        aria-hidden="true"
                    ></progress>

                    {move || state.aa_open.get().then(|| view! {
                        <div class="news-aa-popover" role="dialog" aria-label="Reading typography" on:click=|e| e.stop_propagation()>
                            <div class="news-aa-group">
                                <span class="news-aa-label">"Paper"</span>
                                <div class="news-paper-swatches">
                                    {papers.into_iter().map(|(val, label, cls)| {
                                        let active = move || state.news_paper.get() == val;
                                        view! {
                                            <button
                                                type="button"
                                                class=move || format!("news-paper-swatch {cls}{}", if active() { " is-active" } else { "" })
                                                aria-pressed=move || active().to_string()
                                                on:click=move |_| state.save_pref("newsPaper", json!(val))
                                            >
                                                <i aria-hidden="true">"Aa"</i>
                                                <span>{label}</span>
                                            </button>
                                        }
                                    }).collect_view()}
                                </div>
                            </div>
                            <div class="news-aa-group">
                                <span class="news-aa-label">"Typeface"</span>
                                <div class="news-aa-seg">
                                    {fonts.into_iter().map(|(val, label)| {
                                        let active = move || state.news_font.get() == val;
                                        view! {
                                            <button
                                                type="button"
                                                class=move || if active() { "is-active" } else { "" }
                                                aria-pressed=move || active().to_string()
                                                on:click=move |_| state.save_pref("newsFont", json!(val))
                                            >
                                                {label}
                                            </button>
                                        }
                                    }).collect_view()}
                                </div>
                            </div>
                            <div class="news-aa-group">
                                <span class="news-aa-label">"Text size"</span>
                                <div class="news-aa-seg">
                                    {sizes.into_iter().map(|(val, label)| {
                                        let active = move || state.news_size.get() == val;
                                        view! {
                                            <button
                                                type="button"
                                                class=move || if active() { "is-active" } else { "" }
                                                aria-pressed=move || active().to_string()
                                                on:click=move |_| state.save_pref("newsSize", json!(val))
                                            >
                                                {label}
                                            </button>
                                        }
                                    }).collect_view()}
                                </div>
                            </div>
                        </div>
                    })}
                </div>

                {move || if has_error.get() {
                    view! {
                        <div class="news-empty" role="alert">
                            <h2>"This article could not be opened"</h2>
                            <p>{move || state.article_error.get()}</p>
                            <button
                                type="button"
                                class="secondary-button"
                                on:click=move |_| {
                                    if let Some(id) = state.selected.get_untracked() {
                                        state.open(id);
                                    }
                                }
                            >
                                "Retry"
                            </button>
                        </div>
                    }.into_any()
                } else if item_id.get().is_none() {
                    view! { <p class="news-empty" role="status">"Opening article\u{2026}"</p> }.into_any()
                } else {
                    view! {
                        <article
                            node_ref=scroll_ref
                            class="news-article-body"
                            dir=move || item.with(|i| i.as_ref().filter(|i| i.rtl).map(|_| "rtl"))
                            on:scroll=move |_| {
                                if let Some(el) = scroll_ref.get_untracked() {
                                    let max = (el.scroll_height() - el.client_height()).max(1) as f64;
                                    let pct = ((el.scroll_top() as f64) / max * 100.0).clamp(0.0, 100.0);
                                    read_progress.set(pct);
                                }
                            }
                        >
                            <div class="news-article-column">
                                {move || {
                                    let from_feed = feed.with(|f| f.as_ref().is_some_and(|f| f.full_text));
                                    let on_demand = state.fulltext_fetched.get() == item_id.get();
                                    (from_feed || on_demand).then(|| view! {
                                        <span class="news-fulltext"><Icon name="file"/>"Full text"</span>
                                    })
                                }}
                                <h1 tabindex="-1">
                                    {move || item.with(|i| i.as_ref().map(|i| {
                                        let title = if i.title.is_empty() { "Untitled".to_owned() } else { i.title.clone() };
                                        if i.url.is_empty() {
                                            view! { {title} }.into_any()
                                        } else {
                                            view! { <a href=i.url.clone() target="_blank" rel="noopener noreferrer">{title}</a> }.into_any()
                                        }
                                    }))}
                                </h1>
                                <p class="news-byline">
                                    {move || item.with(|i| i.as_ref().map(|i| {
                                        let mut parts = Vec::new();
                                        if !i.author.is_empty() {
                                            parts.push(format!("By {}", i.author));
                                        }
                                        parts.push(local_date_time(i.pub_date, &[("dateStyle", "medium"), ("timeStyle", "short")]));
                                        let w = words.get();
                                        if w > 80 {
                                            let mins = ((w as f64) / 230.0).round().max(1.0) as usize;
                                            parts.push(format!("{mins} min read"));
                                        }
                                        parts.join(" \u{00b7} ")
                                    }).unwrap_or_default())}
                                </p>

                                {move || {
                                    let Some((link, mime, desc, excerpt)) = enclosure.get() else {
                                        return None;
                                    };
                                    if link.is_empty() {
                                        return None;
                                    }
                                    if is_playable_enclosure(&link, &mime) {
                                        let is_video = is_video_enclosure(&link, &mime);
                                        // Suppress media_description when it duplicates the body excerpt.
                                        let show_desc = !desc.is_empty()
                                            && desc.trim() != excerpt.trim()
                                            && !excerpt.trim().starts_with(desc.trim());
                                        let feed_name = feed.with(|f| f.as_ref().map(|f| f.title.clone()).unwrap_or_else(|| "Podcast".into()));
                                        Some(view! {
                                            <div class="news-player">
                                                {if is_video {
                                                    view! {
                                                        <video
                                                            node_ref=video_ref
                                                            controls
                                                            preload="metadata"
                                                            src=link
                                                            on:pause=move |_| remember()
                                                            on:timeupdate=move |_| remember()
                                                        />
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <audio
                                                            node_ref=audio_ref
                                                            class="news-audio-native"
                                                            preload="metadata"
                                                            src=link
                                                            on:play=move |_| playing.set(true)
                                                            on:pause=move |_| {
                                                                playing.set(false);
                                                                remember();
                                                            }
                                                            on:loadedmetadata=move |_| {
                                                                if let Some(a) = audio_ref.get_untracked() {
                                                                    let d = a.duration();
                                                                    if d.is_finite() && d > 0.0 {
                                                                        audio_duration.set(d);
                                                                    }
                                                                }
                                                            }
                                                            on:timeupdate=move |_| remember()
                                                        />
                                                        <div class="news-pod-top">
                                                            <button
                                                                type="button"
                                                                class="news-pod-play"
                                                                aria-label=move || if playing.get() { "Pause episode" } else { "Play episode" }
                                                                on:click=move |_| {
                                                                    if let Some(a) = audio_ref.get_untracked() {
                                                                        if a.paused() {
                                                                            let _ = a.play();
                                                                        } else {
                                                                            let _ = a.pause();
                                                                        }
                                                                    }
                                                                }
                                                            >
                                                                <Icon name=Signal::derive(move || if playing.get() { "pause" } else { "play" })/>
                                                            </button>
                                                            <div class="news-pod-info">
                                                                <strong>{feed_name}</strong>
                                                                <small>
                                                                    {move || {
                                                                        let pos = audio_time.get();
                                                                        if pos > 1.0 {
                                                                            format!("At {}", format_clock(pos))
                                                                        } else {
                                                                            "Audio episode".into()
                                                                        }
                                                                    }}
                                                                </small>
                                                            </div>
                                                            <div class="news-pod-ctrls">
                                                                <button
                                                                    type="button"
                                                                    class="news-pod-chip"
                                                                    aria-label="Back 15 seconds"
                                                                    on:click=move |_| {
                                                                        if let Some(a) = audio_ref.get_untracked() {
                                                                            let next = (a.current_time() - 15.0).max(0.0);
                                                                            a.set_current_time(next);
                                                                            audio_time.set(next);
                                                                        }
                                                                    }
                                                                >
                                                                    "-15s"
                                                                </button>
                                                                <button
                                                                    type="button"
                                                                    class="news-pod-chip"
                                                                    aria-label="Forward 30 seconds"
                                                                    on:click=move |_| {
                                                                        if let Some(a) = audio_ref.get_untracked() {
                                                                            let dur = a.duration();
                                                                            let next = if dur.is_finite() && dur > 0.0 {
                                                                                (a.current_time() + 30.0).min(dur)
                                                                            } else {
                                                                                a.current_time() + 30.0
                                                                            };
                                                                            a.set_current_time(next);
                                                                            audio_time.set(next);
                                                                        }
                                                                    }
                                                                >
                                                                    "+30s"
                                                                </button>
                                                                <button
                                                                    type="button"
                                                                    class="news-pod-chip"
                                                                    aria-label="Playback speed"
                                                                    on:click=move |_| {
                                                                        let next = match playback_rate.get_untracked() {
                                                                            r if r < 1.1 => 1.25,
                                                                            r if r < 1.4 => 1.5,
                                                                            r if r < 1.8 => 2.0,
                                                                            _ => 1.0,
                                                                        };
                                                                        playback_rate.set(next);
                                                                        if let Some(a) = audio_ref.get_untracked() {
                                                                            a.set_playback_rate(next);
                                                                        }
                                                                    }
                                                                >
                                                                    {move || format!("{}\u{00d7}", playback_rate.get())}
                                                                </button>
                                                            </div>
                                                        </div>
                                                        <div class="news-pod-scrub">
                                                            <span>{move || format_clock(audio_time.get())}</span>
                                                            <input
                                                                type="range"
                                                                class="news-pod-range"
                                                                aria-label="Episode progress"
                                                                min="0"
                                                                max=move || {
                                                                    let d = audio_duration.get();
                                                                    if d > 0.0 { d.floor().to_string() } else { "100".into() }
                                                                }
                                                                prop:value=move || audio_time.get().floor().to_string()
                                                                on:input=move |e| {
                                                                    if let Ok(v) = event_target_value(&e).parse::<f64>()
                                                                        && let Some(a) = audio_ref.get_untracked()
                                                                    {
                                                                        a.set_current_time(v);
                                                                        audio_time.set(v);
                                                                    }
                                                                }
                                                            />
                                                            <span>{move || {
                                                                let d = audio_duration.get();
                                                                if d > 0.0 { format_clock(d) } else { "--:--".into() }
                                                            }}</span>
                                                        </div>
                                                    }.into_any()
                                                }}
                                                {show_desc.then(|| view! { <p class="news-pod-desc">{desc}</p> })}
                                            </div>
                                        }.into_any())
                                    } else if !link.starts_with("https:") {
                                        Some(view! {
                                            <p><a href=link target="_blank" rel="noopener noreferrer">"Open attachment"</a></p>
                                        }.into_any())
                                    } else {
                                        None
                                    }
                                }}
                                <div node_ref=body_ref class="news-body"></div>
                            </div>
                        </article>
                    }.into_any()
                }}

                <Show when=move || item_id.get().is_some()>
                    <div class="news-reader-foot">
                        <span class="news-foot-keys">
                            <kbd>"j"</kbd>" "<kbd>"k"</kbd>" Navigate"
                            <kbd>"f"</kbd>" Focus"
                        </span>
                        <div class="news-foot-nav">
                            <button
                                type="button"
                                class="secondary-button"
                                title="Previous article (k)"
                                on:click=move |_| state.step(-1, false)
                            >
                                "Previous"
                            </button>
                            <button
                                type="button"
                                class="secondary-button"
                                title="Next article (j)"
                                on:click=move |_| state.step(1, false)
                            >
                                "Next"
                            </button>
                            <button
                                type="button"
                                class="primary-button news-next-unread"
                                title="Next unread (n)"
                                on:click=move |_| state.step(1, true)
                            >
                                "Next unread"<Icon name="chevron"/>
                            </button>
                        </div>
                    </div>
                </Show>
            </Show>
        </section>
    }
}
