//! Interactive sandboxed shell for an Agent thread (`agent/terminal.tsx`).

use std::cell::RefCell;
use std::rc::Rc;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde::Deserialize;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::api::{self, BrowserSession, Req};
use crate::icons::Icon;
use crate::ui::window;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: String,
}

/// Minimal terminal screen: printable text, CR/LF/BS/TAB, cursor movement, erase line/screen
/// and SGR bold/colour. Other escape sequences are consumed without effect, so full-screen
/// programs degrade to readable text rather than garbage.
#[derive(Clone, Debug)]
pub struct Screen {
    pub lines: Vec<Vec<Cell>>,
    pub row: usize,
    pub col: usize,
    pub style: String,
    pub rows: usize,
    pending: String,
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen {
    pub fn new() -> Self {
        Self { lines: vec![Vec::new()], row: 0, col: 0, style: String::new(), rows: 24, pending: String::new() }
    }

    fn ensure_line(&mut self) -> &mut Vec<Cell> {
        while self.lines.len() <= self.row {
            self.lines.push(Vec::new());
        }
        &mut self.lines[self.row]
    }

    fn top(&self) -> usize {
        self.lines.len().saturating_sub(self.rows)
    }

    pub fn write(&mut self, text: &str) {
        let mut data = String::with_capacity(self.pending.len() + text.len());
        data.push_str(&self.pending);
        data.push_str(text);
        self.pending.clear();

        let bytes = data.as_bytes();
        let mut i = 0;
        while i < data.len() {
            let rest = &data[i..];
            if bytes[i] == 0x1b {
                if let Some((len, raw, final_ch)) = match_csi(rest) {
                    self.control(raw, final_ch);
                    i += len;
                    continue;
                }
                if let Some(len) = match_osc(rest) {
                    i += len;
                    continue;
                }
                if is_incomplete_escape(rest) {
                    self.pending.push_str(rest);
                    break;
                }
                // Consume ESC + one following char (if any) without effect.
                i += 1;
                if i < data.len() {
                    i += data[i..].chars().next().map(char::len_utf8).unwrap_or(1);
                }
                continue;
            }
            let ch = rest.chars().next().unwrap();
            i += ch.len_utf8();
            match ch {
                '\r' => self.col = 0,
                '\n' => {
                    self.row += 1;
                    self.ensure_line();
                }
                '\x08' => self.col = self.col.saturating_sub(1),
                '\t' => self.col = (self.col / 8 + 1) * 8,
                '\x07' => {}
                c if c < ' ' => {}
                c => {
                    let style = self.style.clone();
                    let col = self.col;
                    let line = self.ensure_line();
                    while line.len() < col {
                        line.push(Cell { ch: ' ', style: String::new() });
                    }
                    if col < line.len() {
                        line[col] = Cell { ch: c, style };
                    } else {
                        line.push(Cell { ch: c, style });
                    }
                    self.col += 1;
                }
            }
        }
        if self.lines.len() > 2000 {
            let drop_count = self.lines.len() - 2000;
            self.lines.drain(0..drop_count);
            self.row = self.row.saturating_sub(drop_count);
        }
    }

    fn control(&mut self, raw: &str, final_ch: char) {
        let cleaned = raw.replace('?', "");
        let values: Vec<usize> = cleaned.split(';').map(|v| v.parse::<usize>().unwrap_or(0)).collect();
        let n = values.first().copied().unwrap_or(0).max(1);
        match final_ch {
            'm' => {
                let codes: Vec<usize> = if raw.is_empty() { vec![0] } else { values };
                for code in codes {
                    if code == 0 {
                        self.style.clear();
                    } else if code == 1 {
                        self.style = with_token(&self.style, |t| t == "b", Some("b"));
                    } else if (30..=37).contains(&code) || (90..=97).contains(&code) {
                        let color = format!("c{}", if code >= 90 { code - 82 } else { code - 30 });
                        self.style = with_token(&self.style, is_color_token, Some(&color));
                    } else if code == 39 {
                        self.style = with_token(&self.style, is_color_token, None);
                    }
                }
            }
            'K' => {
                let col = self.col;
                let line = self.ensure_line();
                match values.first().copied().unwrap_or(0) {
                    2 => line.clear(),
                    1 => {
                        let count = (col + 1).min(line.len());
                        for cell in line.iter_mut().take(count) {
                            *cell = Cell { ch: ' ', style: String::new() };
                        }
                    }
                    _ => {
                        let keep = line.len().min(col);
                        line.truncate(keep);
                    }
                }
            }
            'J' => {
                let mode = values.first().copied().unwrap_or(0);
                if mode == 2 || mode == 3 {
                    self.lines = vec![Vec::new()];
                    self.row = 0;
                    self.col = 0;
                } else {
                    let col = self.col;
                    let row = self.row;
                    let line = self.ensure_line();
                    let keep = line.len().min(col);
                    line.truncate(keep);
                    self.lines.truncate(row + 1);
                }
            }
            'A' => self.row = self.top().max(self.row.saturating_sub(n)),
            'B' => {
                self.row += n;
                self.ensure_line();
            }
            'C' => self.col += n,
            'D' => self.col = self.col.saturating_sub(n),
            'G' => self.col = n - 1,
            'H' | 'f' => {
                let r = values.first().copied().unwrap_or(1).max(1);
                let c = values.get(1).copied().unwrap_or(1).max(1);
                self.row = self.top() + r - 1;
                self.col = c - 1;
                self.ensure_line();
            }
            _ => {}
        }
    }
}

fn is_color_token(token: &str) -> bool {
    token.strip_prefix('c').is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

fn with_token(style: &str, remove: impl Fn(&str) -> bool, add: Option<&str>) -> String {
    let mut tokens: Vec<&str> = style.split_whitespace().filter(|t| !remove(t)).collect();
    if let Some(tok) = add {
        tokens.push(tok);
    }
    tokens.join(" ")
}

/// Matches `^\x1b\[([0-9;?]*)([ -/]*)([@-~])`
fn match_csi(rest: &str) -> Option<(usize, &str, char)> {
    let bytes = rest.as_bytes();
    if bytes.len() < 3 || bytes[0] != 0x1b || bytes[1] != b'[' {
        return None;
    }
    let mut i = 2;
    let param_start = i;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b';' || bytes[i] == b'?') {
        i += 1;
    }
    let param_end = i;
    while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
        i += 1;
    }
    if i < bytes.len() && (0x40..=0x7e).contains(&bytes[i]) {
        Some((i + 1, &rest[param_start..param_end], bytes[i] as char))
    } else {
        None
    }
}

/// Matches `^\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)`
fn match_osc(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    if bytes.len() < 3 || bytes[0] != 0x1b || bytes[1] != b']' {
        return None;
    }
    let mut i = 2;
    while i < bytes.len() {
        if bytes[i] == 0x07 {
            return Some(i + 1);
        }
        if bytes[i] == 0x1b {
            return if i + 1 < bytes.len() && bytes[i + 1] == b'\\' { Some(i + 2) } else { None };
        }
        i += 1;
    }
    None
}

/// Matches `^\x1b(?:\[[0-9;?]*[ -/]*|\][^\x07]*)?$`
fn is_incomplete_escape(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    if bytes.is_empty() || bytes[0] != 0x1b {
        return false;
    }
    if bytes.len() == 1 {
        return true;
    }
    if bytes[1] == b'[' {
        let mut i = 2;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b';' || bytes[i] == b'?') {
            i += 1;
        }
        while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
            i += 1;
        }
        return i == bytes.len();
    }
    if bytes[1] == b']' {
        return !bytes[2..].contains(&0x07);
    }
    false
}

/// Streaming UTF-8 decoder holding trailing incomplete multi-byte sequences across chunks.
#[derive(Default)]
struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    fn decode(&mut self, bytes: &[u8]) -> String {
        let mut input = std::mem::take(&mut self.pending);
        input.extend_from_slice(bytes);
        let mut out = String::new();
        let mut start = 0;
        while start < input.len() {
            match std::str::from_utf8(&input[start..]) {
                Ok(valid) => {
                    out.push_str(valid);
                    break;
                }
                Err(err) => {
                    let valid_len = err.valid_up_to();
                    if valid_len > 0 {
                        out.push_str(std::str::from_utf8(&input[start..start + valid_len]).unwrap());
                    }
                    start += valid_len;
                    if let Some(error_len) = err.error_len() {
                        out.push('\u{FFFD}');
                        start += error_len;
                    } else {
                        self.pending = input[start..].to_vec();
                        break;
                    }
                }
            }
        }
        out
    }
}

fn base64_decode(input: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for b in input.bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => continue,
        };
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    out
}

fn map_key(key: &str) -> Option<&'static str> {
    match key {
        "Enter" => Some("\r"),
        "Backspace" => Some("\x7f"),
        "Tab" => Some("\t"),
        "Escape" => Some("\x1b"),
        "ArrowUp" => Some("\x1b[A"),
        "ArrowDown" => Some("\x1b[B"),
        "ArrowRight" => Some("\x1b[C"),
        "ArrowLeft" => Some("\x1b[D"),
        "Home" => Some("\x1b[H"),
        "End" => Some("\x1b[F"),
        "Delete" => Some("\x1b[3~"),
        "PageUp" => Some("\x1b[5~"),
        "PageDown" => Some("\x1b[6~"),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TerminalState {
    Idle,
    Opening,
    Open,
    Closed,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalPoll {
    open: bool,
    #[allow(dead_code)]
    start: u64,
    end: u64,
    #[serde(default)]
    data: String,
    #[serde(default)]
    exited: bool,
    #[allow(dead_code)]
    exit_code: Option<i64>,
}

#[derive(Deserialize)]
struct TerminalStart {
    start: u64,
    #[allow(dead_code)]
    end: u64,
}

struct LinePart {
    style: String,
    text: String,
    cursor: bool,
}

/// Interactive shell for a thread's working copy, sandboxed on the server like agent commands.
#[component]
pub fn ThreadTerminal(
    #[prop(into)] thread_id: Signal<String>,
    session: BrowserSession,
    #[prop(into)] visible: Signal<bool>,
) -> impl IntoView {
    let screen = StoredValue::new_local(RefCell::new(Screen::new()));
    let redraw = RwSignal::new(0u64);
    let state = RwSignal::new(TerminalState::Idle);
    let error = RwSignal::new(String::new());
    let view_ref = NodeRef::<html::Div>::new();
    let measure_ref = NodeRef::<html::Span>::new();
    let offset = StoredValue::new_local(RefCell::new(0u64));
    let loop_abort: StoredValue<RefCell<Option<web_sys::AbortController>>, LocalStorage> = StoredValue::new_local(RefCell::new(None));
    let queue = StoredValue::new_local(RefCell::new(String::new()));
    let flushing = StoredValue::new_local(RefCell::new(false));
    let decoder = StoredValue::new_local(RefCell::new(Utf8Stream::default()));
    let csrf = StoredValue::new(session.csrf_token);

    let url = move || format!("/api/v1/agent/threads/{}/terminal", thread_id.get_untracked());

    let size = move || -> (u32, u32) {
        let box_rect = view_ref.get_untracked().map(|el| el.get_bounding_client_rect());
        let cell_rect = measure_ref.get_untracked().map(|el| el.get_bounding_client_rect());
        let box_h = box_rect.as_ref().map(|r| r.height()).unwrap_or(300.0);
        let box_w = box_rect.as_ref().map(|r| r.width()).unwrap_or(600.0);
        let cell_h = cell_rect.as_ref().map(|r| r.height()).filter(|&h| h > 0.0).unwrap_or(17.0);
        let cell_w = cell_rect.as_ref().map(|r| r.width()).filter(|&w| w > 0.0).unwrap_or(7.2);
        let rows = ((box_h / cell_h).floor() as i64 - 1).max(4) as u32;
        let cols = (((box_w - 16.0) / cell_w).floor() as i64).max(20) as u32;
        (rows, cols)
    };

    let abort_loop = move || {
        loop_abort.with_value(|cell| {
            if let Some(ctrl) = cell.borrow_mut().take() {
                ctrl.abort();
            }
        });
    };

    on_cleanup(move || abort_loop());

    // Reset when thread changes.
    Effect::new(move |_| {
        let _ = thread_id.get();
        abort_loop();
        state.set(TerminalState::Idle);
        error.set(String::new());
        screen.with_value(|s| *s.borrow_mut() = Screen::new());
        redraw.update(|n| *n += 1);
    });

    // Keep scrolled to bottom after screen updates.
    Effect::new(move |_| {
        redraw.track();
        state.track();
        if let Some(el) = view_ref.get() {
            el.set_scroll_top(el.scroll_height());
        }
    });

    // Resize observer while open.
    Effect::new(move |_| {
        let is_open = state.get() == TerminalState::Open;
        let _vis = visible.get();
        let Some(el) = view_ref.get() else { return };
        if !is_open {
            return;
        }
        let timer_gen = Rc::new(RefCell::new(0u64));
        let cb = {
            let timer_gen = timer_gen.clone();
            Closure::<dyn FnMut()>::new(move || {
                let next_gen = {
                    let mut g = timer_gen.borrow_mut();
                    *g += 1;
                    *g
                };
                let timer_gen = timer_gen.clone();
                spawn_local(async move {
                    crate::sleep(200).await;
                    if *timer_gen.borrow() != next_gen {
                        return;
                    }
                    if state.try_get_untracked() != Some(TerminalState::Open) {
                        return;
                    }
                    let (rows, cols) = size();
                    screen.with_value(|s| s.borrow_mut().rows = rows as usize);
                    let target = format!("{}/resize", url());
                    let body = serde_json::json!({ "rows": rows, "cols": cols }).to_string();
                    let token = csrf.get_value();
                    let _ = api::request::<serde_json::Value>(&target, Req { method: "POST", body: Some(body), csrf: Some(&token), ..Default::default() }).await;
                });
            })
        };
        let observer = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()).expect("ResizeObserver");
        observer.observe(&el);
        let kept = SendWrapper::new((observer, cb, timer_gen));
        on_cleanup(move || {
            let (observer, _cb, timer_gen) = kept.take();
            *timer_gen.borrow_mut() += 1;
            observer.disconnect();
        });
    });

    let start_read = move |controller: web_sys::AbortController, endpoint: String| {
        spawn_local(async move {
            let signal = controller.signal();
            while !signal.aborted() {
                let after = offset.with_value(|o| *o.borrow());
                let poll_url = format!("{endpoint}?after={after}");
                match api::request::<TerminalPoll>(&poll_url, Req { signal: Some(signal.clone()), ..Default::default() }).await {
                    Ok(value) => {
                        if signal.aborted() {
                            return;
                        }
                        if !value.data.is_empty() {
                            let bytes = base64_decode(&value.data);
                            let decoded = decoder.with_value(|d| d.borrow_mut().decode(&bytes));
                            screen.with_value(|s| s.borrow_mut().write(&decoded));
                            redraw.try_update(|n| *n += 1);
                        }
                        offset.with_value(|o| *o.borrow_mut() = value.end);
                        if !value.open || value.exited {
                            state.try_set(TerminalState::Closed);
                            return;
                        }
                    }
                    Err(e) => {
                        if signal.aborted() {
                            return;
                        }
                        error.try_set(if e.message.is_empty() { "The terminal connection failed.".into() } else { e.message });
                        state.try_set(TerminalState::Closed);
                        return;
                    }
                }
            }
        });
    };

    let open_terminal = move || {
        let endpoint = url();
        let (rows, cols) = size();
        state.set(TerminalState::Opening);
        error.set(String::new());
        screen.with_value(|s| {
            let mut next = Screen::new();
            next.rows = rows as usize;
            *s.borrow_mut() = next;
        });
        offset.with_value(|o| *o.borrow_mut() = 0);
        decoder.with_value(|d| *d.borrow_mut() = Utf8Stream::default());
        redraw.update(|n| *n += 1);
        let token = csrf.get_value();
        spawn_local(async move {
            let body = serde_json::json!({ "rows": rows, "cols": cols }).to_string();
            match api::request::<TerminalStart>(&endpoint, Req { method: "POST", body: Some(body), csrf: Some(&token), ..Default::default() }).await {
                Ok(value) => {
                    offset.with_value(|o| *o.borrow_mut() = value.start);
                    state.try_set(TerminalState::Open);
                    abort_loop();
                    let controller = web_sys::AbortController::new().expect("AbortController");
                    loop_abort.with_value(|cell| *cell.borrow_mut() = Some(controller.clone()));
                    start_read(controller, endpoint);
                    // Focus screen on next animation frame.
                    let promise = js_sys::Promise::new(&mut |resolve, _| {
                        let _ = window().request_animation_frame(&resolve);
                    });
                    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                    if let Some(el) = view_ref.get_untracked() {
                        let _ = el.focus();
                    }
                }
                Err(e) => {
                    error.try_set(if e.message.is_empty() { "The terminal could not be opened.".into() } else { e.message });
                    state.try_set(TerminalState::Idle);
                }
            }
        });
    };

    let flush = move || {
        let already = flushing.with_value(|f| {
            let mut f = f.borrow_mut();
            if *f || queue.with_value(|q| q.borrow().is_empty()) {
                true
            } else {
                *f = true;
                false
            }
        });
        if already {
            return;
        }
        let endpoint = format!("{}/input", url());
        let token = csrf.get_value();
        spawn_local(async move {
            loop {
                let data = queue.with_value(|q| std::mem::take(&mut *q.borrow_mut()));
                if data.is_empty() {
                    break;
                }
                let body = serde_json::json!({ "data": data }).to_string();
                if let Err(e) = api::request::<serde_json::Value>(&endpoint, Req { method: "POST", body: Some(body), csrf: Some(&token), ..Default::default() }).await {
                    error.try_set(if e.message.is_empty() { "Input could not be sent.".into() } else { e.message });
                    break;
                }
            }
            flushing.with_value(|f| *f.borrow_mut() = false);
        });
    };

    let send = move |data: String| {
        queue.with_value(|q| q.borrow_mut().push_str(&data));
        flush();
    };

    let on_keydown = move |event: web_sys::KeyboardEvent| {
        if state.get_untracked() != TerminalState::Open || event.meta_key() {
            return;
        }
        let key = event.key();
        if event.alt_key() && key.chars().count() > 1 {
            return;
        }
        let data: Option<String> = if event.ctrl_key() && key.len() == 1 {
            let b = key.as_bytes()[0];
            if b.is_ascii_alphabetic() || matches!(b, b'@' | b'[' | b'\\' | b']' | b'^' | b'_') {
                if key.eq_ignore_ascii_case("c") {
                    let has_sel = window().get_selection().ok().flatten().and_then(|s| s.to_string().as_string()).is_some_and(|s| !s.is_empty());
                    if has_sel {
                        return;
                    }
                }
                let ctrl_byte = b.to_ascii_uppercase() & 31;
                Some((ctrl_byte as char).to_string())
            } else if event.shift_key() {
                return;
            } else {
                map_key(&key).map(str::to_owned).or_else(|| (key.chars().count() == 1).then_some(key))
            }
        } else if event.ctrl_key() && event.shift_key() {
            return;
        } else {
            map_key(&key).map(str::to_owned).or_else(|| (key.chars().count() == 1).then_some(key))
        };
        if let Some(data) = data {
            event.prevent_default();
            send(data);
        }
    };

    let on_paste = move |event: web_sys::ClipboardEvent| {
        if state.get_untracked() != TerminalState::Open {
            return;
        }
        if let Some(dt) = event.clipboard_data()
            && let Ok(text) = dt.get_data("text")
        {
            event.prevent_default();
            let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
            send(normalized);
        }
    };

    let end_session = move |_| {
        abort_loop();
        let endpoint = url();
        let token = csrf.get_value();
        spawn_local(async move {
            let _ = api::request::<serde_json::Value>(&endpoint, Req { method: "DELETE", csrf: Some(&token), ..Default::default() }).await;
            state.try_set(TerminalState::Closed);
        });
    };

    let show_screen = Memo::new(move |_| {
        redraw.track();
        let st = state.get();
        let has_lines = screen.with_value(|s| s.borrow().lines.iter().any(|l| !l.is_empty()));
        !(st == TerminalState::Idle || (st == TerminalState::Closed && !has_lines))
    });

    view! {
        <div class="agent-shell-terminal">
            <span node_ref=measure_ref class="agent-shell-measure" aria-hidden="true">"M"</span>
            {move || {
                if !show_screen.get() {
                    view! {
                        <div class="agent-shell-start">
                            <p>{move || if state.get() == TerminalState::Closed { "The terminal session ended." } else { "A shell in this thread\u{2019}s working copy. It runs in the same sandbox as Codex: no network, workspace writes only." }}</p>
                            {move || { let err = error.get(); (!err.is_empty()).then(|| view! { <p role="alert" class="error">{err}</p> }) }}
                            <button type="button" class="secondary-button" on:click=move |_| open_terminal()>
                                <Icon name="terminal"/>
                                {move || if state.get() == TerminalState::Closed { "Start a new terminal" } else { "Open terminal" }}
                            </button>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div node_ref=view_ref class="agent-shell-screen" role="log" aria-label="Terminal" aria-live="off" tabindex="0"
                            on:keydown=on_keydown
                            on:paste=on_paste>
                            {move || {
                                redraw.track();
                                let st = state.get();
                                let rendered_lines: Vec<Vec<LinePart>> = screen.with_value(|s| {
                                    let s = s.borrow();
                                    let cursor_row = s.row;
                                    let cursor_col = s.col;
                                    s.lines.iter().enumerate().map(|(row, line)| {
                                        let cursor_here = row == cursor_row && st == TerminalState::Open;
                                        let total_len = if cursor_here { line.len().max(cursor_col + 1) } else { line.len() };
                                        let mut parts: Vec<LinePart> = Vec::new();
                                        for col in 0..total_len {
                                            let (ch, style) = match line.get(col) {
                                                Some(cell) => (cell.ch, cell.style.as_str()),
                                                None => (' ', ""),
                                            };
                                            let is_cursor = cursor_here && col == cursor_col;
                                            if !is_cursor && let Some(last) = parts.last_mut() && !last.cursor && last.style == style {
                                                last.text.push(ch);
                                            } else {
                                                parts.push(LinePart { style: style.to_owned(), text: ch.to_string(), cursor: is_cursor });
                                            }
                                        }
                                        parts
                                    }).collect()
                                });
                                rendered_lines.into_iter().map(|parts| {
                                    view! {
                                        <div class="agent-shell-line">
                                            {parts.into_iter().map(|part| {
                                                let mut classes: Vec<String> = part.style.split_whitespace().map(|name| format!("t-{name}")).collect();
                                                if part.cursor {
                                                    classes.push("agent-shell-cursor".into());
                                                }
                                                let class_attr = (!classes.is_empty()).then(|| classes.join(" "));
                                                view! { <span class=class_attr>{part.text}</span> }
                                            }).collect_view()}
                                            "\u{200b}"
                                        </div>
                                    }
                                }).collect_view()
                            }}
                        </div>
                        <div class="agent-shell-status">
                            <span>{move || match state.get() {
                                TerminalState::Opening => "Starting\u{2026}",
                                TerminalState::Open => "Sandboxed shell \u{00b7} no network",
                                _ => "Session ended",
                            }}</span>
                            {move || { let err = error.get(); (!err.is_empty()).then(|| view! { <span role="alert" class="error">{err}</span> }) }}
                            {move || match state.get() {
                                TerminalState::Open => view! { <button type="button" class="text-button" on:click=end_session>"End session"</button> }.into_any(),
                                TerminalState::Closed => view! { <button type="button" class="text-button" on:click=move |_| open_terminal()>"Restart"</button> }.into_any(),
                                _ => ().into_any(),
                            }}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_handles_basic_text_and_csi() {
        let mut s = Screen::new();
        s.write("hello\r\n\x1b[1;32mworld\x1b[0m");
        assert_eq!(s.lines.len(), 2);
        let line0: String = s.lines[0].iter().map(|c| c.ch).collect();
        let line1: String = s.lines[1].iter().map(|c| c.ch).collect();
        assert_eq!(line0, "hello");
        assert_eq!(line1, "world");
        assert_eq!(s.lines[1][0].style, "b c2");
    }

    #[test]
    fn screen_buffers_split_escape() {
        let mut s = Screen::new();
        s.write("a\x1b[3");
        s.write("1mb");
        assert_eq!(s.lines[0].len(), 2);
        assert_eq!(s.lines[0][1].ch, 'b');
        assert_eq!(s.lines[0][1].style, "c1");
    }

    #[test]
    fn base64_and_utf8_stream() {
        let bytes = base64_decode("aGVsbG8=");
        assert_eq!(bytes, b"hello");
        let mut stream = Utf8Stream::default();
        let emoji = "hi \u{1F600}".as_bytes();
        let part1 = stream.decode(&emoji[..5]);
        let part2 = stream.decode(&emoji[5..]);
        assert_eq!(format!("{part1}{part2}"), "hi \u{1F600}");
    }
}
