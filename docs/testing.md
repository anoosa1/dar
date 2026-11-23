# Testing

Everything runs from Dar's dev shell (`nix develop`), after `npm ci` (for the SDK bundle and
pdf.js):

```sh
cargo xtask verify                 # build, unit tests, the bridge's Go tests, every acceptance suite
cargo xtask verify --only mail     # suites whose name contains "mail"; `=mail` for that one (no unit or Go tests)
cargo xtask verify --release       # the same against the shipped build (LTO, wasm-opt)
cargo test                         # unit tests of the native crates only
```

`verify` builds with the `fast` and `wasm-fast` profiles: optimized, incremental, without
LTO. Unchanged steps are skipped. Suites run in parallel (`--jobs`, default a third of the
cores, at most 5), each with its own instances and Chromium. Each run's logs, roots and
screenshots stay in the folder it prints; a failing case also saves `failure-<case>-<n>.png`
of the pages its suite left open.

## The acceptance suite (`tests/`)

The suites test the whole stack: SpacetimeDB with the module, the runtime, the client in
headless Chromium, on throwaway roots, databases and ports. Never point a suite at live data.

- `src/suites/<name>.rs` holds one suite: a `SUITE` with its cases, which run in order and
  share one fixture instance. `*_support.rs` hold an area's shared helpers and the fixture
  programs the runtime starts (fake chat bridge, Codex and Claude stand-ins), which run as
  `dar-acceptance fixture <name>`.
- A case is `async fn(t: &Ctx) -> Result<()>`. `t.fixture()` is the suite's standard
  instance: the example app installed, the `configured-rp` OIDC client, the fake chat
  bridge and the administrator `admin` with `PASSWORD`. `Instance::isolated` makes others.
- Suites do not share state: each has its own instances, so they can run at once. Use unique
  names anyway (`uuid()`), as a suite's cases share an instance.
- `assert!`, `assert_eq!` and `?` all fail a case; the runner catches panics and timeouts
  and runs the remaining cases and suites.

`use dar_acceptance::prelude::*;` brings in what suites use:

| Need | Use |
| --- | --- |
| Sign in, HTTP | `login`, `login_as`, `session.get/post/put/patch/delete`, `session.request(method, path)`, `Reply::expect(status)`, `upload`, `upload_with` |
| SpacetimeDB as a person | `session.connect()`, `db.db.my_files().iter()`, `reducer(\|done\| db.reducers.x_then(args…, done))`, `procedure(\|done\| db.procedures.y_then(args…, done))` |
| The service credential | `instance.database()?.call(name, json!([...]))`, `support::operator`, `backup`, `restore`… |
| Identity API | `IdentityAdmin::new(base)`, `.api`, `.write` (with the password proof), `CookieJar` |
| Browser | `Browser::shared()`, `browser.new_context(ContextOptions::sized(w, h))`, `ContextOptions::phone(w, h)`, `context.new_page()` |

Reducers and procedures take positional arguments in the order the module declares them
(see `crates/bindings/src/generated/*_reducer.rs`); an omitted optional argument is `None`.

## Browser tests

`browser/` drives Chromium over the DevTools Protocol with Playwright's model and waiting
rules. Locators resolve when used; actions wait until exactly one element matches and is
visible, stable, enabled and not covered. More than one match is an error.

- `page.get_by_role("button", "Save")` matches the accessible name exactly (Playwright's
  `exact: true`); `contains("Save")` is a case-insensitive substring and `regex("^Save")` a
  JavaScript regular expression. `()` matches any name. Role options follow:
  `.level(2)`, `.pressed(true)`, `.selected(true)`, `.checked(true)`, `.expanded(true)`.
- Also `get_by_label`, `get_by_text`, `get_by_placeholder`, `get_by_title`, `locator(css)`,
  `frame_locator(css)`, `filter_has_text`, `filter_has`, `filter_visible`, `first`, `last`,
  `nth`, `or`, `and`.
- Actions and reads: `click`, `click_with(ClickOptions)`, `fill`, `press`, `press_sequentially`,
  `check`, `set_checked`, `hover`, `focus`, `select_option`, `set_input_files`, `wait_for`,
  `wait_for_state(State::Hidden)`, `count`, `is_visible`, `input_value`, `get_attribute`,
  `text_content`, `bounding_box`, `evaluate(function, arg)`.
- Pages: `goto`, `reload`, `url`, `wait_for_url`, `evaluate`, `wait_for_function`, `keyboard()`,
  `mouse()`, `set_viewport_size`, `emulate_media`, `screenshot(t.screenshot("name.png"), true)`,
  `route(glob, times, handler)`, `wait_for_download(action)`, `request()` (with the page's
  cookies), `collect_errors(ignore)`, `on_console`, `on_request…`, and `track_sockets` with
  `drop_sockets` to make the client reconnect.
- Contexts default to Playwright's: light colour scheme, no reduced motion, 1280×720.
