//! Spaced-repetition flashcard review view for Notes: deck selection, one-card-at-a-time review
//! with Markdown rendering, Space / 1-4 keyboard shortcuts, and revision-checked schedule writes
//! back into the source Markdown notes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dar_notes::cards::{Card, Date, Grade, Schedule, format_interval, schedule};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use wasm_bindgen::JsCast;

use super::note_links;
use super::notes::{fetch_detail, read_file_content};
use crate::api::{self, BrowserSession};
use crate::files::editor::{TextFile, write_text_file};
use crate::icons::Icon;
use crate::live::Live;
use crate::markdown::Markdown;
use crate::ui::{listen, window};

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardSummary {
    #[serde(default)]
    pub due: usize,
    #[serde(default)]
    pub new: usize,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeckCount {
    pub name: String,
    #[serde(default)]
    pub due: usize,
    #[serde(default)]
    pub new: usize,
    #[serde(default)]
    pub total: usize,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewCard {
    pub note_id: String,
    pub title: String,
    pub path: String,
    #[serde(default)]
    pub revision: u32,
    #[serde(default)]
    pub card_index: usize,
    pub question: String,
    pub answer: String,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub interval: Option<u32>,
    #[serde(default)]
    pub ease: Option<u32>,
    pub deck: String,
}

impl ReviewCard {
    fn schedule_state(&self) -> Option<Schedule> {
        let due = Date::parse(self.due.as_deref()?)?;
        let interval = self.interval?;
        let ease = self.ease?;
        (interval > 0).then_some(Schedule {
            due,
            interval,
            ease,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardsResponse {
    #[serde(default)]
    pub today: String,
    #[serde(default)]
    pub due: usize,
    #[serde(default)]
    pub new: usize,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub decks: Vec<DeckCount>,
    #[serde(default)]
    pub cards: Vec<ReviewCard>,
}

fn today_date() -> Date {
    let (iso, _) = note_links::today();
    Date::parse(&iso).unwrap_or_else(|| Date::new(2026, 1, 1).expect("fallback date"))
}

fn is_typing_target(e: &web_sys::KeyboardEvent) -> bool {
    if e.ctrl_key() || e.meta_key() || e.alt_key() {
        return true;
    }
    let Some(target) = e
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
    else {
        return false;
    };
    target
        .closest(
            "input,textarea,select,[contenteditable=\"true\"],.cm-editor,[role=\"dialog\"],dialog",
        )
        .ok()
        .flatten()
        .is_some()
}

fn match_card<'a>(found: &'a [Card], target: &ReviewCard) -> Option<&'a Card> {
    if let Some(card) = found.get(target.card_index)
        && card.question == target.question
        && card.answer == target.answer
    {
        return Some(card);
    }
    found
        .iter()
        .find(|c| c.question == target.question && c.answer == target.answer)
}

/// Re-reads the note's revision and content, rewrites only the graded card's `<!--SR:...-->`
/// comment, and writes back with `expected_revision` (retrying once on conflict).
async fn persist_grade(
    live: Live,
    session: &BrowserSession,
    card: &ReviewCard,
    grade: Grade,
    today: Date,
) -> Result<(), String> {
    for attempt in 0..2 {
        let conn = live
            .connection
            .try_get_untracked()
            .flatten()
            .ok_or_else(|| "Notes are reconnecting. Try again in a moment.".to_owned())?;
        let detail = fetch_detail(&conn, &card.note_id).await?;
        if detail.role == "read" {
            return Err("This note is read-only; its review schedule cannot be saved.".to_owned());
        }
        let content = read_file_content(&card.note_id).await?;
        let found = dar_notes::cards::find(&content);
        let target = match_card(&found, card).ok_or_else(|| {
            "This card changed or was removed from the note. Refresh to continue.".to_owned()
        })?;
        let next_state = schedule(target.schedule, grade, today);
        let updated = dar_notes::cards::rewrite(&content, target, next_state);
        let entry = &detail.entry;
        let media_type = if entry.media_type.is_empty() {
            "text/markdown"
        } else {
            &entry.media_type
        };
        let file = TextFile {
            name: &entry.name,
            parent_id: &entry.parent_id,
            content: &updated,
            media_type,
            file_id: Some(&entry.id),
            expected_revision: Some(entry.revision),
        };
        match write_text_file(session, file).await {
            Ok(_) => return Ok(()),
            Err(err)
                if attempt == 0
                    && (err.status == 409
                        || err.status == 412
                        || err.message.to_lowercase().contains("conflict")) =>
            {
                continue;
            }
            Err(err) => {
                return Err(if err.message.is_empty() {
                    "Could not save review schedule.".into()
                } else {
                    err.message
                });
            }
        }
    }
    Err("The note changed while saving. Try again.".into())
}

#[component]
pub fn NoteCards(
    session: BrowserSession,
    on_open: Callback<String>,
    on_due_change: Callback<usize>,
    on_note_updated: Callback<()>,
) -> impl IntoView {
    let live = expect_context::<Live>();
    let session_sv = StoredValue::new(session);

    let loaded = RwSignal::new(false);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(String::new());
    let total_in_library = RwSignal::new(0usize);
    let today = RwSignal::new(today_date());
    let decks: RwSignal<Vec<DeckCount>> = RwSignal::new(Vec::new());
    let cards: RwSignal<Vec<ReviewCard>> = RwSignal::new(Vec::new());
    let selected_deck = RwSignal::new(String::new());
    let revealed = RwSignal::new(false);
    let saving = RwSignal::new(false);
    let reviewed = RwSignal::new(0usize);
    let again_count = RwSignal::new(0usize);
    let reload_tick = RwSignal::new(0u32);
    let seq = RwSignal::new(0u32);

    let reveal_btn = NodeRef::<html::Button>::new();
    let answer_ref = NodeRef::<html::Div>::new();
    let end_ref = NodeRef::<html::Div>::new();

    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || alive.store(false, Ordering::Relaxed));
    }

    // Load due and new cards on mount and when Refresh is requested.
    {
        let alive = alive.clone();
        Effect::new(move |_| {
            let _tick = reload_tick.get();
            let ticket = seq.get_untracked() + 1;
            seq.set(ticket);
            loading.set(true);
            let alive = alive.clone();
            let query_today = today_date();
            spawn_local(async move {
                let url = format!("/api/v1/notes/cards?today={query_today}");
                match api::get::<CardsResponse>(&url).await {
                    Ok(resp)
                        if alive.load(Ordering::Relaxed)
                            && seq.try_get_untracked() == Some(ticket) =>
                    {
                        let due_total = resp.cards.len();
                        let parsed_today = Date::parse(&resp.today).unwrap_or(query_today);
                        let _ = today.try_set(parsed_today);
                        let _ = total_in_library.try_set(resp.total);
                        let cur_deck = selected_deck.try_get_untracked().unwrap_or_default();
                        if !cur_deck.is_empty() && !resp.decks.iter().any(|d| d.name == cur_deck) {
                            let _ = selected_deck.try_set(String::new());
                        }
                        let _ = decks.try_set(resp.decks);
                        let _ = cards.try_set(resp.cards);
                        let _ = revealed.try_set(false);
                        let _ = error.try_set(String::new());
                        let _ = loaded.try_set(true);
                        let _ = loading.try_set(false);
                        on_due_change.try_run(due_total);
                    }
                    Err(e)
                        if alive.load(Ordering::Relaxed)
                            && seq.try_get_untracked() == Some(ticket) =>
                    {
                        let msg = if e.message.is_empty() {
                            "Flashcards could not be loaded.".into()
                        } else {
                            e.message
                        };
                        let _ = error.try_set(msg);
                        let _ = loading.try_set(false);
                    }
                    _ => {}
                }
            });
        });
    }

    let all_remaining = Memo::new(move |_| cards.with(Vec::len));
    let deck_remaining = Memo::new(move |_| {
        let deck = selected_deck.get();
        cards.with(|list| {
            if deck.is_empty() {
                list.len()
            } else {
                list.iter().filter(|c| c.deck == deck).count()
            }
        })
    });
    let current_card = Memo::new(move |_| {
        let deck = selected_deck.get();
        cards.with(|list| {
            if deck.is_empty() {
                list.first().cloned()
            } else {
                list.iter().find(|c| c.deck == deck).cloned()
            }
        })
    });

    // Move focus to the revealed answer region so keyboard and screen-reader users track the transition.
    Effect::new(move |_| {
        if revealed.get() {
            request_animation_frame(move || {
                if let Some(el) = answer_ref.try_get_untracked().flatten() {
                    let _ = el.focus();
                }
            });
        }
    });

    let alive_grade = StoredValue::new(alive.clone());
    let do_grade = Callback::new(move |grade: Grade| {
        if saving.try_get_untracked().unwrap_or(true) {
            return;
        }
        let Some(card) = current_card.try_get_untracked().flatten() else {
            return;
        };
        let sess = session_sv.get_value();
        let review_day = today.try_get_untracked().unwrap_or_else(today_date);
        let _ = saving.try_set(true);
        let _ = error.try_set(String::new());
        spawn_local(async move {
            let outcome = persist_grade(live, &sess, &card, grade, review_day).await;
            if !alive_grade
                .try_with_value(|a| a.load(Ordering::Relaxed))
                .unwrap_or(false)
            {
                return;
            }
            match outcome {
                Ok(()) => {
                    let was_due = card.due.is_some();
                    let note_id = card.note_id.clone();
                    let card_index = card.card_index;
                    let deck_name = card.deck.clone();
                    let _ = cards.try_update(|list| {
                        if let Some(pos) = list
                            .iter()
                            .position(|c| c.note_id == note_id && c.card_index == card_index)
                        {
                            list.remove(pos);
                        }
                    });
                    let _ = decks.try_update(|list| {
                        if let Some(d) = list.iter_mut().find(|d| d.name == deck_name) {
                            if was_due {
                                d.due = d.due.saturating_sub(1);
                            } else {
                                d.new = d.new.saturating_sub(1);
                            }
                        }
                    });
                    let _ = reviewed.try_update(|n| *n += 1);
                    if grade == Grade::Again {
                        let _ = again_count.try_update(|n| *n += 1);
                    }
                    let _ = revealed.try_set(false);
                    let remaining_total = cards.try_with_untracked(Vec::len).unwrap_or(0);
                    on_due_change.try_run(remaining_total);
                    on_note_updated.try_run(());
                    request_animation_frame(move || {
                        if let Some(btn) = reveal_btn.try_get_untracked().flatten() {
                            let _ = btn.focus();
                        } else if let Some(end) = end_ref.try_get_untracked().flatten() {
                            let _ = end.focus();
                        }
                    });
                }
                Err(msg) => {
                    let _ = error.try_set(msg);
                }
            }
            let _ = saving.try_set(false);
        });
    });

    // Keyboard shortcuts: Space reveals answer; 1-4 grade Again/Hard/Good/Easy when revealed.
    {
        let alive = alive.clone();
        listen(&window(), "keydown", false, move |event| {
            if !alive.load(Ordering::Relaxed) {
                return;
            }
            let Ok(e) = event.dyn_into::<web_sys::KeyboardEvent>() else {
                return;
            };
            if is_typing_target(&e) {
                return;
            }
            if saving.try_get_untracked().unwrap_or(true) {
                return;
            }
            if current_card
                .try_with_untracked(Option::is_none)
                .unwrap_or(true)
            {
                return;
            }
            let key = e.key();
            let is_revealed = revealed.try_get_untracked().unwrap_or(false);
            if !is_revealed && (key == " " || key == "Spacebar") {
                let on_other_control = e
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .and_then(|el| el.closest("button,a").ok().flatten())
                    .is_some_and(|el| !el.class_list().contains("note-cards-reveal"));
                if !on_other_control {
                    e.prevent_default();
                    let _ = revealed.try_set(true);
                }
                return;
            }
            if is_revealed {
                let grade = match key.as_str() {
                    "1" => Some(Grade::Again),
                    "2" => Some(Grade::Hard),
                    "3" => Some(Grade::Good),
                    "4" => Some(Grade::Easy),
                    _ => None,
                };
                if let Some(g) = grade {
                    e.prevent_default();
                    do_grade.try_run(g);
                }
            }
        });
    }

    let refresh = move || {
        error.set(String::new());
        reload_tick.update(|n| *n += 1);
    };

    view! {
        <div class="note-cards">
            {move || (!error.get().is_empty()).then(|| view! {
                <div role="alert" class="error">
                    {error.get()}
                    <button type="button" class="text-button" on:click=move |_| refresh()>"Reload"</button>
                </div>
            })}

            {move || (!loaded.get() && loading.get() && error.with(String::is_empty)).then(|| view! {
                <p role="status">"Loading flashcards\u{2026}"</p>
            })}

            <Show when=move || { loaded.get() && total_in_library.get() > 0 }>
                <div class="note-cards-toolbar">
                    <div class="note-cards-decks" role="group" aria-label="Decks">
                        <button
                            type="button"
                            class=move || format!("note-deck-chip {}", if selected_deck.with(String::is_empty) { "is-active" } else { "" })
                            aria-pressed=move || selected_deck.with(String::is_empty).to_string()
                            disabled=move || saving.get()
                            on:click=move |_| {
                                selected_deck.set(String::new());
                                revealed.set(false);
                                error.set(String::new());
                            }
                        >
                            <span>"All decks"</span>
                            <small class="note-deck-count">{move || all_remaining.get()}</small>
                        </button>
                        {move || decks.get().into_iter().map(|deck| {
                            let name_cls = deck.name.clone();
                            let name_pressed = deck.name.clone();
                            let name_click = deck.name.clone();
                            let count = deck.due + deck.new;
                            view! {
                                <button
                                    type="button"
                                    class=move || format!("note-deck-chip {}", if selected_deck.get() == name_cls { "is-active" } else { "" })
                                    aria-pressed=move || (selected_deck.get() == name_pressed).to_string()
                                    disabled=move || saving.get()
                                    on:click=move |_| {
                                        selected_deck.set(name_click.clone());
                                        revealed.set(false);
                                        error.set(String::new());
                                    }
                                >
                                    <span>{deck.name}</span>
                                    <small class="note-deck-count">{count}</small>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                    <button
                        type="button"
                        class="text-button note-cards-refresh"
                        disabled=move || loading.get() || saving.get()
                        on:click=move |_| refresh()
                    >
                        <Icon name="refresh"/>"Refresh"
                    </button>
                </div>
            </Show>

            {move || loaded.get().then(|| {
                if total_in_library.get() == 0 {
                    return view! {
                        <div class="empty-state note-cards-empty">
                            <span class="empty-icon"><Icon name="stack"/></span>
                            <h2>"No flashcards yet"</h2>
                            <p>"Add cards in any note using Question::Answer (or ::: for reversible), a ? line between question and answer, or a list item tagged #card with nested answer items."</p>
                        </div>
                    }.into_any();
                }
                if let Some(card) = current_card.get() {
                    let note_id = card.note_id.clone();
                    let deck_label = card.deck.clone();
                    let note_title = if card.title.is_empty() { card.path.clone() } else { card.title.clone() };
                    let status_label = match &card.due {
                        Some(due) => format!("Due {due}"),
                        None => "New".to_owned(),
                    };
                    let question = card.question.clone();
                    let answer = card.answer.clone();
                    let sched = card.schedule_state();
                    let review_day = today.get();
                    let grades: [(&'static str, &'static str, &'static str, Grade, String); 4] = [
                        ("Again", "1", "is-again", Grade::Again, format_interval(schedule(sched, Grade::Again, review_day).interval)),
                        ("Hard", "2", "is-hard", Grade::Hard, format_interval(schedule(sched, Grade::Hard, review_day).interval)),
                        ("Good", "3", "is-good", Grade::Good, format_interval(schedule(sched, Grade::Good, review_day).interval)),
                        ("Easy", "4", "is-easy", Grade::Easy, format_interval(schedule(sched, Grade::Easy, review_day).interval)),
                    ];
                    return view! {
                        <article class="note-card" aria-label="Flashcard">
                            <header class="note-card-header">
                                <div class="note-card-meta">
                                    <span class="note-card-deck">{deck_label}</span>
                                    <span class="note-card-source">{note_title}</span>
                                    <span class="note-card-status">{status_label}</span>
                                </div>
                                <div class="note-card-tools">
                                    <span class="note-card-progress" aria-live="polite">
                                        {move || {
                                            let left = deck_remaining.get();
                                            let done = reviewed.get();
                                            if done > 0 {
                                                format!("{done} reviewed \u{00b7} {left} left")
                                            } else {
                                                format!("{left} left")
                                            }
                                        }}
                                    </span>
                                    <button
                                        type="button"
                                        class="text-button note-card-open"
                                        on:click=move |_| { on_open.try_run(note_id.clone()); }
                                    >
                                        <Icon name="file"/>"Open note"
                                    </button>
                                </div>
                            </header>

                            <div class="note-card-body">
                                <div class="note-card-section" role="region" aria-label="Question">
                                    <span class="note-card-label">"Question"</span>
                                    <Markdown source=question/>
                                </div>

                                <Show
                                    when=move || revealed.get()
                                    fallback=move || view! {
                                        <div class="note-card-actions">
                                            <button
                                                type="button"
                                                node_ref=reveal_btn
                                                class="primary-button note-cards-reveal"
                                                disabled=move || saving.get()
                                                on:click=move |_| revealed.set(true)
                                            >
                                                "Show answer"
                                                <kbd class="note-cards-key" aria-hidden="true">"Space"</kbd>
                                            </button>
                                        </div>
                                    }
                                >
                                    <div node_ref=answer_ref tabindex="-1" class="note-card-section note-card-answer" role="region" aria-label="Answer">
                                        <span class="note-card-label">"Answer"</span>
                                        <Markdown source=answer.clone()/>
                                    </div>
                                    <div class="note-cards-grades" role="group" aria-label="Grade card">
                                        {grades.iter().map(|(label, key_hint, cls, grade, interval)| {
                                            let label = *label;
                                            let key_hint = *key_hint;
                                            let cls = *cls;
                                            let grade = *grade;
                                            let interval = interval.clone();
                                            view! {
                                                <button
                                                    type="button"
                                                    class=format!("note-grade {cls}")
                                                    disabled=move || saving.get()
                                                    on:click=move |_| { do_grade.try_run(grade); }
                                                >
                                                    <span class="note-grade-top">
                                                        <span class="note-grade-label">{label}</span>
                                                        <kbd class="note-cards-key" aria-hidden="true">{key_hint}</kbd>
                                                    </span>
                                                    <small class="note-grade-interval">{interval}</small>
                                                </button>
                                            }
                                        }).collect_view()}
                                    </div>
                                    {move || saving.get().then(|| view! {
                                        <p role="status" class="muted small note-cards-saving">"Saving\u{2026}"</p>
                                    })}
                                </Show>
                            </div>
                        </article>
                    }.into_any();
                }

                // End screen / all caught up for the selected deck.
                let done = reviewed.get();
                let again = again_count.get();
                let total = total_in_library.get();
                let other_due = all_remaining.get();
                let in_subdeck = !selected_deck.with(String::is_empty);
                let heading = if done > 0 { "Review complete" } else { "All caught up" };
                let summary = if done > 0 {
                    let noun = if done == 1 { "card" } else { "cards" };
                    if again > 0 {
                        format!("Reviewed {done} {noun} ({again} marked Again for tomorrow).")
                    } else {
                        format!("Reviewed {done} {noun} in this session.")
                    }
                } else {
                    let noun = if total == 1 { "card" } else { "cards" };
                    format!("No cards are due right now. {total} {noun} scheduled across your notes.")
                };
                view! {
                    <div node_ref=end_ref tabindex="-1" class="empty-state note-cards-end">
                        <span class="empty-icon"><Icon name="check"/></span>
                        <h2>{heading}</h2>
                        <p>{summary}</p>
                        <div class="note-cards-end-actions">
                            {(in_subdeck && other_due > 0).then(|| view! {
                                <button
                                    type="button"
                                    class="primary-button"
                                    on:click=move |_| {
                                        selected_deck.set(String::new());
                                        revealed.set(false);
                                    }
                                >
                                    {format!("Review other decks ({other_due})")}
                                </button>
                            })}
                            <button type="button" class="secondary-button" on:click=move |_| refresh()>
                                <Icon name="refresh"/>"Refresh"
                            </button>
                        </div>
                    </div>
                }.into_any()
            })}
        </div>
    }
}
