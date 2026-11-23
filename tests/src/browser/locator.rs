//! Locators: a page and a chain of steps, resolved afresh (by `engine.js`) each time they are
//! used. Actions wait, as Playwright's do, until exactly one element matches and is visible,
//! stable, enabled and not covered; more than one match is an error (strict mode).
//!
//! Names and texts take a [`Pattern`]: a `&str` matches the whole normalized text exactly
//! (Playwright's `exact: true`), [`contains`] a case-insensitive substring (Playwright's default
//! without `exact`), and [`regex`] a JavaScript regular expression.

use super::page::{ClickOptions, FilePayload, Page, Rect, transient, truthy};
use super::DEFAULT_TIMEOUT;
use crate::{Error, Result};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    Any,
    Exact(String),
    Contains(String),
    Regex { source: String, flags: String },
}

/// A case-insensitive substring (Playwright's string match without `exact`).
pub fn contains(text: impl Into<String>) -> Pattern {
    Pattern::Contains(text.into())
}

/// A JavaScript regular expression: `regex("^Site")`, or with flags `regex("/^site/i")`.
pub fn regex(source: &str) -> Pattern {
    if let Some(rest) = source.strip_prefix('/')
        && let Some(end) = rest.rfind('/')
        && rest[end + 1..].chars().all(|c| "dgimsuvy".contains(c))
    {
        return Pattern::Regex { source: rest[..end].to_owned(), flags: rest[end + 1..].to_owned() };
    }
    Pattern::Regex { source: source.to_owned(), flags: String::new() }
}

impl From<&str> for Pattern {
    fn from(text: &str) -> Self {
        Pattern::Exact(text.to_owned())
    }
}
impl From<String> for Pattern {
    fn from(text: String) -> Self {
        Pattern::Exact(text)
    }
}
impl From<&String> for Pattern {
    fn from(text: &String) -> Self {
        Pattern::Exact(text.clone())
    }
}
impl From<()> for Pattern {
    fn from(_: ()) -> Self {
        Pattern::Any
    }
}

impl Pattern {
    fn json(&self) -> Value {
        match self {
            Pattern::Any => Value::Null,
            Pattern::Exact(value) => json!({"kind": "exact", "value": value}),
            Pattern::Contains(value) => json!({"kind": "contains", "value": value}),
            Pattern::Regex { source, flags } => json!({"kind": "regex", "source": source, "flags": flags}),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RoleOptions {
    level: Option<u32>,
    checked: Option<bool>,
    pressed: Option<bool>,
    selected: Option<bool>,
    expanded: Option<bool>,
    disabled: Option<bool>,
    include_hidden: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum Step {
    Css(String),
    Role(String, Pattern, RoleOptions),
    Text(Pattern),
    Label(Pattern),
    Placeholder(Pattern),
    Title(Pattern),
    Frame,
    HasText(Pattern),
    HasNotText(Pattern),
    Has(Vec<Step>),
    HasNot(Vec<Step>),
    Visible(bool),
    Nth(i64),
    Or(Vec<Step>),
    And(Vec<Step>),
}

impl Step {
    pub(crate) fn role(role: &str, name: Pattern) -> Step {
        Step::Role(role.to_owned(), name, RoleOptions::default())
    }

    fn json(&self) -> Value {
        let steps = |steps: &[Step]| Value::Array(steps.iter().map(Step::json).collect());
        match self {
            Step::Css(selector) => json!({"kind": "css", "selector": selector}),
            Step::Role(role, name, o) => {
                let mut step = json!({"kind": "role", "role": role, "name": name.json(), "includeHidden": o.include_hidden});
                for (key, value) in [("checked", o.checked), ("pressed", o.pressed), ("selected", o.selected), ("expanded", o.expanded), ("disabled", o.disabled)] {
                    if let Some(value) = value {
                        step[key] = json!(value);
                    }
                }
                if let Some(level) = o.level {
                    step["level"] = json!(level);
                }
                step
            }
            Step::Text(p) => json!({"kind": "text", "pattern": p.json()}),
            Step::Label(p) => json!({"kind": "label", "pattern": p.json()}),
            Step::Placeholder(p) => json!({"kind": "placeholder", "pattern": p.json()}),
            Step::Title(p) => json!({"kind": "title", "pattern": p.json()}),
            Step::Frame => json!({"kind": "frame"}),
            Step::HasText(p) => json!({"kind": "hasText", "pattern": p.json()}),
            Step::HasNotText(p) => json!({"kind": "hasNotText", "pattern": p.json()}),
            Step::Has(inner) => json!({"kind": "has", "steps": steps(inner)}),
            Step::HasNot(inner) => json!({"kind": "hasNot", "steps": steps(inner)}),
            Step::Visible(value) => json!({"kind": "visible", "value": value}),
            Step::Nth(index) => json!({"kind": "nth", "index": index}),
            Step::Or(inner) => json!({"kind": "or", "steps": steps(inner)}),
            Step::And(inner) => json!({"kind": "and", "steps": steps(inner)}),
        }
    }

    fn describe(&self) -> String {
        let pattern = |p: &Pattern| match p {
            Pattern::Any => String::new(),
            Pattern::Exact(v) => format!("{v:?}"),
            Pattern::Contains(v) => format!("contains({v:?})"),
            Pattern::Regex { source, flags } => format!("/{source}/{flags}"),
        };
        match self {
            Step::Css(selector) => format!("locator({selector:?})"),
            Step::Role(role, name, _) => format!("get_by_role({role:?}, {})", pattern(name)),
            Step::Text(p) => format!("get_by_text({})", pattern(p)),
            Step::Label(p) => format!("get_by_label({})", pattern(p)),
            Step::Placeholder(p) => format!("get_by_placeholder({})", pattern(p)),
            Step::Title(p) => format!("get_by_title({})", pattern(p)),
            Step::Frame => "frame".into(),
            Step::HasText(p) => format!("filter_has_text({})", pattern(p)),
            Step::HasNotText(p) => format!("filter_has_not_text({})", pattern(p)),
            Step::Has(_) => "filter_has(…)".into(),
            Step::HasNot(_) => "filter_has_not(…)".into(),
            Step::Visible(v) => format!("filter_visible({v})"),
            Step::Nth(i) => format!("nth({i})"),
            Step::Or(_) => "or(…)".into(),
            Step::And(_) => "and(…)".into(),
        }
    }
}

/// The states `wait_for_state` waits for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Attached,
    Detached,
    Visible,
    Hidden,
}

#[derive(Clone)]
pub struct Locator {
    page: Page,
    steps: Vec<Step>,
    timeout: Duration,
}

impl std::fmt::Debug for Locator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe())
    }
}

/// What a probe of the page found.
enum Probe {
    Ready(Value),
    Retry(String),
}

impl Locator {
    pub(crate) fn new(page: Page, steps: Vec<Step>) -> Self {
        Self { page, steps, timeout: DEFAULT_TIMEOUT }
    }

    fn with(&self, step: Step) -> Self {
        let mut steps = self.steps.clone();
        steps.push(step);
        Self { page: self.page.clone(), steps, timeout: self.timeout }
    }

    pub fn page(&self) -> &Page {
        &self.page
    }

    pub fn describe(&self) -> String {
        self.steps.iter().map(Step::describe).collect::<Vec<_>>().join(".")
    }

    /// How long actions and waits on this locator may take (default 30 s).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn steps_json(&self) -> Value {
        Value::Array(self.steps.iter().map(Step::json).collect())
    }

    // ---- chaining -----------------------------------------------------------------------

    pub fn locator(&self, css: &str) -> Locator {
        self.with(Step::Css(css.to_owned()))
    }
    pub fn get_by_role(&self, role: &str, name: impl Into<Pattern>) -> Locator {
        self.with(Step::role(role, name.into()))
    }
    pub fn get_by_text(&self, text: impl Into<Pattern>) -> Locator {
        self.with(Step::Text(text.into()))
    }
    pub fn get_by_label(&self, text: impl Into<Pattern>) -> Locator {
        self.with(Step::Label(text.into()))
    }
    pub fn get_by_placeholder(&self, text: impl Into<Pattern>) -> Locator {
        self.with(Step::Placeholder(text.into()))
    }
    pub fn get_by_title(&self, text: impl Into<Pattern>) -> Locator {
        self.with(Step::Title(text.into()))
    }
    pub fn frame_locator(&self, css: &str) -> Locator {
        self.with(Step::Css(css.to_owned())).with(Step::Frame)
    }
    /// The content of this iframe (Playwright's `contentFrame()`).
    pub fn content_frame(&self) -> Locator {
        self.with(Step::Frame)
    }
    pub fn filter_has_text(&self, text: impl Into<Pattern>) -> Locator {
        let text = match text.into() {
            // `hasText` with a string is a substring match in Playwright.
            Pattern::Exact(value) => Pattern::Contains(value),
            other => other,
        };
        self.with(Step::HasText(text))
    }
    pub fn filter_has_not_text(&self, text: impl Into<Pattern>) -> Locator {
        let text = match text.into() {
            Pattern::Exact(value) => Pattern::Contains(value),
            other => other,
        };
        self.with(Step::HasNotText(text))
    }
    pub fn filter_has(&self, inner: &Locator) -> Locator {
        self.with(Step::Has(inner.steps.clone()))
    }
    pub fn filter_has_not(&self, inner: &Locator) -> Locator {
        self.with(Step::HasNot(inner.steps.clone()))
    }
    pub fn filter_visible(&self, visible: bool) -> Locator {
        self.with(Step::Visible(visible))
    }
    pub fn first(&self) -> Locator {
        self.with(Step::Nth(0))
    }
    pub fn last(&self) -> Locator {
        self.with(Step::Nth(-1))
    }
    pub fn nth(&self, index: i64) -> Locator {
        self.with(Step::Nth(index))
    }
    pub fn or(&self, other: &Locator) -> Locator {
        self.with(Step::Or(other.steps.clone()))
    }
    pub fn and(&self, other: &Locator) -> Locator {
        self.with(Step::And(other.steps.clone()))
    }

    fn role_option(mut self, apply: impl FnOnce(&mut RoleOptions)) -> Locator {
        match self.steps.last_mut() {
            Some(Step::Role(_, _, options)) => apply(options),
            _ => panic!("role options follow get_by_role: {}", self.describe()),
        }
        self
    }
    /// Role options (`{level: 2}`, `{pressed: true}`…) for the preceding `get_by_role`.
    pub fn level(self, level: u32) -> Locator {
        self.role_option(|o| o.level = Some(level))
    }
    pub fn checked(self, checked: bool) -> Locator {
        self.role_option(|o| o.checked = Some(checked))
    }
    pub fn pressed(self, pressed: bool) -> Locator {
        self.role_option(|o| o.pressed = Some(pressed))
    }
    pub fn selected(self, selected: bool) -> Locator {
        self.role_option(|o| o.selected = Some(selected))
    }
    pub fn expanded(self, expanded: bool) -> Locator {
        self.role_option(|o| o.expanded = Some(expanded))
    }
    pub fn disabled(self, disabled: bool) -> Locator {
        self.role_option(|o| o.disabled = Some(disabled))
    }
    pub fn include_hidden(self) -> Locator {
        self.role_option(|o| o.include_hidden = true)
    }

    // ---- probing ------------------------------------------------------------------------

    /// Runs `body` (JavaScript with `E` the engine, `els` the matches and `arg`) in the page.
    async fn run(&self, body: &str, arg: &Value) -> Result<Value> {
        let script = format!("(async (steps, arg) => {{ const E = globalThis.__darEngine; const els = E.resolve(steps); {body} }})");
        let expression = format!("{script}({}, {})", self.steps_json(), arg);
        self.page.evaluate_expression(&expression).await
    }

    /// Probes until the page answers `{ok: value}`; `{retry: reason}` waits and tries again.
    async fn poll(&self, body: &str, arg: Value, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        let delays = [0u64, 20, 50, 100, 100, 250];
        let mut attempt = 0;
        let mut last: String;
        loop {
            match self.run(body, &arg).await {
                Ok(answer) => match probe(answer) {
                    Ok(Probe::Ready(value)) => return Ok(value),
                    Ok(Probe::Retry(reason)) => last = reason,
                    Err(error) => return Err(error.context(self.describe())),
                },
                Err(error) if transient(&error) => last = error.to_string(),
                Err(error) => return Err(error.context(self.describe())),
            }
            if Instant::now() >= deadline {
                return Err(Error::new(format!("{} timed out after {timeout:?}: {last}", self.describe())));
            }
            tokio::time::sleep(Duration::from_millis(delays[attempt.min(delays.len() - 1)])).await;
            attempt += 1;
        }
    }

    const STRICT: &'static str = r#"if (els.length > 1) return {error: `strict mode: ${els.length} elements match, ` + els.slice(0, 3).map(E.describe).join(" ; ")};"#;

    /// Waits until one element is ready for a pointer action and returns where to click.
    async fn point(&self, options: &ClickOptions, enabled: bool) -> Result<(f64, f64)> {
        let body = format!(r#"
            if (els.length === 0) return {{retry: "no element matches"}};
            {strict}
            const el = els[0];
            if (!arg.force) {{
              if (!E.visible(el)) return {{retry: "the element is not visible"}};
              if (arg.enabled && E.disabled(el)) return {{retry: "the element is disabled"}};
            }}
            if (el.scrollIntoViewIfNeeded) el.scrollIntoViewIfNeeded(true); else el.scrollIntoView({{block: "center", inline: "center"}});
            if (!arg.force && !(await E.stable(el))) return {{retry: "the element is moving"}};
            const box = E.box(el);
            const x = box.x + (arg.position ? arg.position[0] : box.width / 2), y = box.y + (arg.position ? arg.position[1] : box.height / 2);
            if (!arg.force) {{ const covered = E.hitTarget(el, x, y); if (covered) return {{retry: `${{covered}} would receive the click`}}; }}
            return {{ok: [x, y]}};"#, strict = Self::STRICT);
        let value = self.poll(&body, json!({"force": options.force, "enabled": enabled, "position": options.position.map(|(x, y)| [x, y])}), options.timeout.unwrap_or(self.timeout)).await?;
        Ok((value[0].as_f64().unwrap_or_default(), value[1].as_f64().unwrap_or_default()))
    }

    /// The single matching element once attached (and visible, when asked), as a remote object.
    async fn handle(&self, visible: bool) -> Result<String> {
        self.poll(&format!(r#"if (els.length === 0) return {{retry: "no element matches"}}; {} if (arg && !E.visible(els[0])) return {{retry: "the element is not visible"}}; return {{ok: true}};"#, Self::STRICT), json!(visible), self.timeout).await?;
        let expression = format!("globalThis.__darEngine.resolve({})[0]", self.steps_json());
        let result = self.page.call("Runtime.evaluate", json!({"expression": expression})).await?;
        result["result"]["objectId"].as_str().map(str::to_owned).ok_or_else(|| Error::new(format!("{}: the element went away", self.describe())))
    }

    // ---- actions ------------------------------------------------------------------------

    pub async fn click(&self) -> Result<()> {
        self.click_with(ClickOptions::default()).await
    }

    pub async fn dblclick(&self) -> Result<()> {
        self.click_with(ClickOptions { click_count: Some(2), ..ClickOptions::default() }).await
    }

    pub async fn click_with(&self, options: ClickOptions) -> Result<()> {
        let (x, y) = self.point(&options, true).await?;
        if options.trial {
            return Ok(());
        }
        let button = options.button.unwrap_or("left");
        let keyboard = self.page.keyboard();
        for modifier in &options.modifiers {
            keyboard.down(modifier).await?;
        }
        self.page.mouse().move_to(x, y, 1).await?;
        let clicks = options.click_count.unwrap_or(1);
        for count in 1..=clicks {
            self.page.mouse_event("mousePressed", x, y, button, count).await?;
            if let Some(delay) = options.delay {
                tokio::time::sleep(delay).await;
            }
            self.page.mouse_event("mouseReleased", x, y, button, count).await?;
        }
        for modifier in options.modifiers.iter().rev() {
            keyboard.up(modifier).await?;
        }
        Ok(())
    }

    pub async fn hover(&self) -> Result<()> {
        let (x, y) = self.point(&ClickOptions::default(), false).await?;
        self.page.mouse().move_to(x, y, 1).await
    }

    /// Taps with touch (contexts with `has_touch`).
    pub async fn tap(&self) -> Result<()> {
        let (x, y) = self.point(&ClickOptions::default(), true).await?;
        for kind in ["touchStart", "touchEnd"] {
            let points = if kind == "touchStart" { json!([{"x": x, "y": y}]) } else { json!([]) };
            self.page.call("Input.dispatchTouchEvent", json!({"type": kind, "touchPoints": points, "modifiers": self.page.modifiers()})).await?;
        }
        Ok(())
    }

    /// Replaces the value of an input, textarea or editable element (Playwright's `fill`).
    pub async fn fill(&self, value: &str) -> Result<()> {
        let body = format!(r#"
            if (els.length === 0) return {{retry: "no element matches"}};
            {strict}
            const el = els[0];
            if (!E.visible(el)) return {{retry: "the element is not visible"}};
            if (!el.isContentEditable && !["input", "textarea"].includes(el.localName)) return {{error: "the element is not an editable input, textarea or contenteditable"}};
            if (!E.editable(el)) return {{retry: "the element is not editable yet"}};
            if (el.localName === "input" && ["checkbox", "radio", "file", "button", "submit", "reset", "image"].includes(el.type)) return {{error: `cannot fill an input of type ${{el.type}}`}};
            const direct = el.localName === "input" && ["date", "time", "datetime-local", "month", "week", "color", "range"].includes(el.type);
            if (direct) {{
              el.focus(); el.value = arg;
              if (el.value !== arg) return {{error: `the ${{el.type}} input rejected ${{arg}}`}};
              el.dispatchEvent(new Event("input", {{bubbles: true, composed: true}})); el.dispatchEvent(new Event("change", {{bubbles: true}}));
              return {{ok: "done"}};
            }}
            el.focus();
            if (el.isContentEditable) {{ const range = el.ownerDocument.createRange(); range.selectNodeContents(el); const selection = el.ownerDocument.defaultView.getSelection(); selection.removeAllRanges(); selection.addRange(range); }}
            else {{ el.select(); }}
            return {{ok: "type"}};"#, strict = Self::STRICT);
        let mode = self.poll(&body, json!(value), self.timeout).await?;
        if mode == "type" {
            if value.is_empty() {
                self.page.keyboard().press("Delete").await?;
            } else {
                self.page.keyboard().insert_text(value).await?;
            }
        }
        Ok(())
    }

    pub async fn clear(&self) -> Result<()> {
        self.fill("").await
    }

    /// Focuses the element and presses a key or chord.
    pub async fn press(&self, key: &str) -> Result<()> {
        self.focus().await?;
        self.page.keyboard().press(key).await
    }

    /// Focuses the element and types text key by key (`pressSequentially`).
    pub async fn press_sequentially(&self, text: &str) -> Result<()> {
        self.focus().await?;
        self.page.keyboard().type_text(text).await
    }

    pub async fn focus(&self) -> Result<()> {
        self.poll(&format!(r#"if (els.length === 0) return {{retry: "no element matches"}}; {} els[0].focus(); return {{ok: true}};"#, Self::STRICT), Value::Null, self.timeout).await?;
        Ok(())
    }

    pub async fn blur(&self) -> Result<()> {
        self.poll(&format!(r#"if (els.length === 0) return {{retry: "no element matches"}}; {} els[0].blur(); return {{ok: true}};"#, Self::STRICT), Value::Null, self.timeout).await?;
        Ok(())
    }

    pub async fn check(&self) -> Result<()> {
        self.set_checked(true).await
    }

    pub async fn uncheck(&self) -> Result<()> {
        self.set_checked(false).await
    }

    /// Clicks a checkbox, radio or switch until it has the state, then confirms it.
    pub async fn set_checked(&self, checked: bool) -> Result<()> {
        if self.is_checked().await? == checked {
            return Ok(());
        }
        self.click().await?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.is_checked().await? != checked {
            if Instant::now() > deadline {
                return Err(Error::new(format!("{}: clicking did not make it {}", self.describe(), if checked { "checked" } else { "unchecked" })));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }

    /// Chooses an option of a native `<select>` by value or label.
    pub async fn select_option(&self, value: &str) -> Result<()> {
        let body = format!(r#"
            if (els.length === 0) return {{retry: "no element matches"}};
            {strict}
            const el = els[0];
            if (el.localName !== "select") return {{error: "the element is not a <select>"}};
            if (E.disabled(el)) return {{retry: "the select is disabled"}};
            const option = [...el.options].find(o => o.value === arg) ?? [...el.options].find(o => E.norm(o.label) === E.norm(arg));
            if (!option) return {{retry: `no option ${{arg}}`}};
            el.value = option.value;
            el.dispatchEvent(new Event("input", {{bubbles: true, composed: true}})); el.dispatchEvent(new Event("change", {{bubbles: true}}));
            return {{ok: option.value}};"#, strict = Self::STRICT);
        self.poll(&body, json!(value), self.timeout).await?;
        Ok(())
    }

    pub async fn set_input_files(&self, files: Vec<FilePayload>) -> Result<()> {
        let object = self.handle(false).await?;
        self.page.set_files(&object, files).await
    }

    pub async fn scroll_into_view_if_needed(&self) -> Result<()> {
        self.poll(&format!(r#"if (els.length === 0) return {{retry: "no element matches"}}; {} if (!E.visible(els[0])) return {{retry: "the element is not visible"}}; els[0].scrollIntoViewIfNeeded(true); return {{ok: true}};"#, Self::STRICT), Value::Null, self.timeout).await?;
        Ok(())
    }

    /// Dispatches a DOM event of `kind` (`click`, `dragstart`…) with `init` properties.
    pub async fn dispatch_event(&self, kind: &str, init: Value) -> Result<()> {
        let body = format!(r#"
            if (els.length === 0) return {{retry: "no element matches"}};
            {strict}
            const [type, init] = arg, props = {{bubbles: true, cancelable: true, composed: true, ...init}};
            const Kind = /^(click|dblclick|mouse|contextmenu)/.test(type) ? MouseEvent : /^pointer/.test(type) ? PointerEvent : /^key/.test(type) ? KeyboardEvent
              : /^(drag|drop)/.test(type) ? DragEvent : /^focus|^blur/.test(type) ? FocusEvent : /^(input|beforeinput)$/.test(type) ? InputEvent : Event;
            if (Kind === DragEvent && !props.dataTransfer) props.dataTransfer = new DataTransfer();
            els[0].dispatchEvent(new Kind(type, props));
            return {{ok: true}};"#, strict = Self::STRICT);
        self.poll(&body, json!([kind, init]), self.timeout).await?;
        Ok(())
    }

    // ---- waiting ------------------------------------------------------------------------

    /// Waits until the element is visible (Playwright's `waitFor()`).
    pub async fn wait_for(&self) -> Result<()> {
        self.wait_for_state(State::Visible).await
    }

    pub async fn wait_for_state(&self, state: State) -> Result<()> {
        let body = format!(r#"
            const strict = () => {strict_fn};
            switch (arg) {{
              case "attached": if (els.length === 0) return {{retry: "no element matches"}}; return strict() ?? {{ok: true}};
              case "detached": return els.length === 0 ? {{ok: true}} : {{retry: `${{els.length}} element(s) still attached`}};
              case "visible": if (els.length === 0) return {{retry: "no element matches"}}; return strict() ?? (E.visible(els[0]) ? {{ok: true}} : {{retry: "the element is not visible"}});
              case "hidden": if (els.length === 0) return {{ok: true}}; return strict() ?? (E.visible(els[0]) ? {{retry: "the element is still visible"}} : {{ok: true}});
            }}"#, strict_fn = "(els.length > 1 ? {error: `strict mode: ${els.length} elements match, ` + els.slice(0, 3).map(E.describe).join(\" ; \")} : null)");
        let name = match state { State::Attached => "attached", State::Detached => "detached", State::Visible => "visible", State::Hidden => "hidden" };
        self.poll(&body, json!(name), self.timeout).await?;
        Ok(())
    }

    /// Polls a JavaScript function of the element until it returns a truthy value.
    pub async fn wait_until(&self, function: &str) -> Result<Value> {
        let body = format!(r#"if (els.length === 0) return {{retry: "no element matches"}}; {} const value = await ({function})(els[0], arg); return value ? {{ok: value}} : {{retry: "the condition is false"}};"#, Self::STRICT);
        self.poll(&body, Value::Null, self.timeout).await
    }

    // ---- reading ------------------------------------------------------------------------

    /// How many elements match now (no waiting).
    pub async fn count(&self) -> Result<usize> {
        let value = self.read("return {ok: els.length};").await?;
        Ok(value.as_u64().unwrap_or_default() as usize)
    }

    async fn read(&self, body: &str) -> Result<Value> {
        // A read retries only across navigations; it never waits for the element.
        self.poll(body, Value::Null, self.timeout).await
    }

    pub async fn is_visible(&self) -> Result<bool> {
        Ok(self.read(&format!("if (els.length === 0) return {{ok: false}}; {} return {{ok: E.visible(els[0])}};", Self::STRICT)).await? == json!(true))
    }

    pub async fn is_hidden(&self) -> Result<bool> {
        Ok(!self.is_visible().await?)
    }

    async fn attached(&self, body: &str) -> Result<Value> {
        self.poll(&format!(r#"if (els.length === 0) return {{retry: "no element matches"}}; {} const el = els[0]; {body}"#, Self::STRICT), Value::Null, self.timeout).await
    }

    pub async fn is_checked(&self) -> Result<bool> {
        Ok(self.attached("return {ok: E.checkedState(el) === true};").await? == json!(true))
    }

    pub async fn is_enabled(&self) -> Result<bool> {
        Ok(self.attached("return {ok: !E.disabled(el)};").await? == json!(true))
    }

    pub async fn is_disabled(&self) -> Result<bool> {
        Ok(!self.is_enabled().await?)
    }

    pub async fn is_editable(&self) -> Result<bool> {
        Ok(self.attached("return {ok: E.editable(el)};").await? == json!(true))
    }

    pub async fn input_value(&self) -> Result<String> {
        let value = self.attached(r#"if (!["input", "textarea", "select"].includes(el.localName)) return {error: "the element is not an input, textarea or select"}; return {ok: el.value};"#).await?;
        Ok(value.as_str().unwrap_or_default().to_owned())
    }

    pub async fn get_attribute(&self, name: &str) -> Result<Option<String>> {
        let value = self.poll(&format!(r#"if (els.length === 0) return {{retry: "no element matches"}}; {} return {{ok: [els[0].getAttribute(arg)]}};"#, Self::STRICT), json!(name), self.timeout).await?;
        Ok(value[0].as_str().map(str::to_owned))
    }

    pub async fn text_content(&self) -> Result<String> {
        Ok(self.attached("return {ok: el.textContent ?? \"\"};").await?.as_str().unwrap_or_default().to_owned())
    }

    pub async fn inner_text(&self) -> Result<String> {
        Ok(self.attached("return {ok: el.innerText ?? el.textContent ?? \"\"};").await?.as_str().unwrap_or_default().to_owned())
    }

    pub async fn inner_html(&self) -> Result<String> {
        Ok(self.attached("return {ok: el.innerHTML};").await?.as_str().unwrap_or_default().to_owned())
    }

    pub async fn all_text_contents(&self) -> Result<Vec<String>> {
        let value = self.read("return {ok: els.map(el => el.textContent ?? \"\")};").await?;
        Ok(serde_json::from_value(value).unwrap_or_default())
    }

    pub async fn all_inner_texts(&self) -> Result<Vec<String>> {
        let value = self.read("return {ok: els.map(el => el.innerText ?? \"\")};").await?;
        Ok(serde_json::from_value(value).unwrap_or_default())
    }

    /// A locator for each element that matches now.
    pub async fn all(&self) -> Result<Vec<Locator>> {
        Ok((0..self.count().await?).map(|index| self.nth(index as i64)).collect())
    }

    /// The element's box in viewport coordinates, or `None` when it is not visible.
    pub async fn bounding_box(&self) -> Result<Option<Rect>> {
        let value = self.attached("return {ok: E.visible(el) ? [E.box(el)] : [null]};").await?;
        Ok(serde_json::from_value(value[0].clone()).ok())
    }

    /// Runs `function(element, arg)` in the page and returns its JSON value.
    pub async fn evaluate(&self, function: &str, arg: Value) -> Result<Value> {
        let value = self.poll(&format!(r#"if (els.length === 0) return {{retry: "no element matches"}}; {} return {{ok: [await ({function})(els[0], arg)]}};"#, Self::STRICT), arg, self.timeout).await?;
        Ok(value[0].clone())
    }

    /// Runs `function(elements, arg)` over every match, without waiting.
    pub async fn evaluate_all(&self, function: &str, arg: Value) -> Result<Value> {
        let value = self.poll(&format!("return {{ok: [await ({function})(els, arg)]}};"), arg, self.timeout).await?;
        Ok(value[0].clone())
    }

    /// A PNG of the element.
    pub async fn screenshot(&self, path: impl AsRef<Path>) -> Result<Vec<u8>> {
        self.scroll_into_view_if_needed().await?;
        let rect = self.bounding_box().await?.ok_or_else(|| Error::new(format!("{}: not visible for a screenshot", self.describe())))?;
        self.page.capture(path.as_ref(), json!({"format": "png", "clip": {"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height, "scale": 1}})).await
    }

    /// Whether `function(element)` is truthy for the single match now.
    pub async fn matches(&self, function: &str) -> Result<bool> {
        Ok(truthy(&self.evaluate(function, Value::Null).await?))
    }
}

fn probe(answer: Value) -> Result<Probe> {
    if let Some(error) = answer.get("error") {
        return Err(Error::new(error.as_str().unwrap_or("locator error").to_owned()));
    }
    if let Some(value) = answer.get("ok") {
        return Ok(Probe::Ready(value.clone()));
    }
    Ok(Probe::Retry(answer.get("retry").and_then(Value::as_str).unwrap_or("not ready").to_owned()))
}

