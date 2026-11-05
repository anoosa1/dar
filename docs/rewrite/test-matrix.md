# Rewrite test matrix

## October 23 continuation

The restored pinned environment passes 291 unit/service tests / 5,814
assertions after the approved dependency pin updates. The production dependency
audit reports zero findings. The latest integrated attempt passed all 88 native
tests, then stopped in Prayer's browser journey. That Settings/refresh race,
Contacts' phone menu clipping and login destination loss have focused passing
browser proofs; see [v1 readiness](v1-readiness.md) for the exact logs. Both
October 23 collection roots are retired after later source changes. No fresh
coverage result or release readiness is claimed. The new Calendar settings and
recurrence browser journeys extend the suite before the next collection.

The older records below describe historical snapshots; their temporary
artifacts and preview processes are no longer available on this host.

## Frozen development/release diagnostic — 2026-10-09

Candidate 7's unchanged release/development bundles, maps, stylesheet and unit
runtime produce 82.79% coverage (27,758 / 33,527 statements), with 14 subsystems
below 80%. The gates remain unchanged and fail. The isolated collection is
`/tmp/dar-v1-coverage-oct09c-5p_hdnq2`: 289 unit/service tests / 5,784 assertions,
70 native tests, all 30 release browser journeys, external DAV, 112,920
expanded property assertions, and three rounds of six lifecycle fixtures pass.
Four development journeys fail: three report fixed-Harness React warnings;
News's first checkbox edit is not rendered. Failed logs and execution are
retained as diagnostic evidence, not release acceptance.

After that frozen collection, candidate 8's fast stage passes the same 289
tests / 5,784 assertions. Calendar recurrence/native/DAV checks pass 10 tests,
including six previously failing rule/window cases, finite future-split counts,
portable bytes and restart identities. Chat's development browser journey is
console-clean. The selected Contacts B menu placement and News draft rendering
correction are undergoing focused browser verification. No earlier compiled
coverage is merged into the next snapshot. See [v1 readiness](v1-readiness.md).

## V1 workspace and release rehearsal — 2026-10-09

The approved B workspace candidate passes 286 unit/service tests / 5,767
assertions, 67 native tests plus one production TLS/OIDC rehearsal, all 29 PR
browser journeys, external vdirsyncer round trips, 112,903 expanded assertions
and three rounds of six lifecycle fixtures. The frozen source-mapped collection
`/tmp/dar-v1-coverage-oct09b-btsx7g6o` fails the binding coverage gate at
80.78% (23,458 / 29,040 statements), with 24 subsystems below 80%.
[The exact readiness record](v1-readiness.md) records the interrupted/resumed
driver, CSS refinements, visual proofs and remaining acceptance work.

After that collection, `todo-journal.test.mjs` reproduced and then verified a
fix for failed recipient delivery being superseded by another portable edit.
Its focused process proof and the existing Todo recovery/sync tests pass;
fresh integrated measurements are required for this later compiled snapshot.
The older records below remain historical.

## Reading comfort polish — 2026-10-08

The latest Home draft/compaction source passes fast with 270 tests / 5,619
assertions, native dashboard revision stability and the new Home recovery
browser journey. `chat-history.mjs` additionally proves submitted-query paging,
older page ownership, archive filtering and phone error/retry; Weather now checks
uncached provider outage and HTTP refresh failure retaining the saved forecast.
The maintainer selected Font Awesome toolbar treatment C, softly tinted.
Fresh collection n passes release builds, 270 unit/service tests / 5,619
ordinary assertions, all 64 native tests, all 27 PR browser journeys, real
vdirsyncer, 112,755 expanded assertions and three five-test lifecycle rounds.
The icon matrix covers 130 page states, 70 editor/composer states and eight
views at 200% magnification, plus the separate 60-state public/login matrix.
Native 320px inspection also verifies fit with classic scrollbars and portable
Maildir readback after saving a draft. Two resumed fixture checks adopt readable
navigation text and Home's automatically recovered editing mode; compiled
JavaScript/maps remain unchanged throughout the continuations.
The gate **fails at 81.28% (22,913 / 28,191)**, with **23 subsystems below 80%**.
Report: `/tmp/dar-icons-coverage-oct08n.LyKLdG/report.json`. Logs:
`/tmp/dar-icons-integrated-oct08n.log`, `/tmp/dar-icons-integrated-oct08n2.log`
and `/tmp/dar-icons-integrated-oct08n3.log`. The current seeded preview is
`http://127.0.0.1:4276/`; earlier collections and previews remain historical.
The binding 90% overall / 80% subsystem gates and broader parity gaps remain open.
See [the latest record](ui-polish.md#home-recovery-and-toolbar-icon-continuation).

The subsequent Todo/Calendar recovery snapshot passes fast (268 tests / 5,611
assertions), native list-cleanup rollback/retry and subscription malformed-feed
retention, and both new real browser journeys. PR/nightly now include
`todo-recovery.mjs` and `calendar-subscription.mjs`. Collection j at
`/tmp/dar-recovery-coverage-oct08j.hTzieD` passes builds/unit, 63 native tests and
13 browser journeys, then fails Todo's new editor-entry focus assertion. The
focus/close-address fixes pass the focused Todo journey. Fast also passes 268
tests / 5,611 assertions for that source. Collection k at
`/tmp/dar-recovery-coverage-oct08k.BKCJHy` completes all verification stages,
but its gate fails at 81.31% overall, with 22 subsystems below 80%.
The subsequent Chat history fixes require a fresh collection; k remains
historical for them. See [the recovery record](ui-polish.md#todo-and-calendar-recovery-continuation)
for reproductions, storage readback and the exact open acceptance gaps.

`preferences.test.mjs` adds real HTTP/SQLite proof for false-valued preferences,
two-user isolation, concurrent conditional writes, invalid scope/key/revision
rejection, missing/deleted records and restart. It passes against collection k's
unchanged compiled snapshot in `/tmp/dar-preferences-native-oct08k.log`; its raw
execution measurements join that collection. This is scoped preference proof,
not full contract/parity acceptance.

Collection k's native stage initially stops on OIDC fixture port allocation
(63/64 pass). Four-worker rerun passes OIDC but hits a lock in the Files share
fixture's separate second-user seed (63/64 pass). Native concurrency is now
bounded to four; that connection uses a five-second busy timeout and closes on
failure. Production JavaScript/source maps remain frozen throughout recovery.
The complete four-worker native rerun passes all 64 tests in
`/tmp/dar-recovery-native-oct08k2.log`. All 25 browser journeys, external DAV,
expanded parser tests and three lifecycle rounds also pass against that same
compiled snapshot, without rebuilding. Logs and the failed gate are recorded in
[UI polish](ui-polish.md#completed-recovery-verification).

The maintainer selected C. `ui-polish.mjs` is included in PR/nightly: 13 loaded
pages at 320, 390, 768, 1024 and 1440 CSS pixels in light/dark themes, short-screen
editing, real Email bulk partial/lost-reply/stale selection recovery, owned
delayed reader replies and missing Notes/Email object links. `notes-titles.test.mjs`
adds API/assistant/search/Activity/portable-file/restart proof for independent
titles, normalized collision-safe filenames, concurrent creation and retained
attachments/IDs/revisions. Focused Email selection units prove the 200-message
limit and deterministic revision-backed ETag.

Build/unit proof passes 268 tests / 5,611 assertions. The focused Notes, Todo,
Activity, Chat, workspace and shared browser journeys pass. The first two
instrumented nightly attempts stopped on old mobile Activity button locators
in Activity and Chat fixtures; those fixtures now use the named source selector.
Activity's retry fixture retains its injected failure until the actual click,
then removes the interception before later navigation. Neither attempt counts
as full nightly/coverage acceptance. A third attempt exposed a Prayer settings
editor remount during refresh; only initial deep-link opening now waits for the
load, and both Prayer journeys pass after keeping open editors mounted. The
expanded shared journey passes all app-settings links, Notes preferences load
failure/retry, loaded-page overflow, long reading text, short-screen Email draft
API persistence and keyboard dismissal/focus restoration. A new coherent
collection is in progress. The later full-reload sweep reached high profiler
memory and stalled during teardown. The collector now bounds its work/disposal
and avoids queuing periodic captures, while the sweep uses real SPA launcher
navigation. Native navigation/reload coverage retention and the complete
instrumented shared journey pass. Logs: `/tmp/dar-polish-coverage-navigation-oct08.log`
and `/tmp/dar-polish-profiler-proof-oct08.log`. A fresh full run uses these fixes.
See [UI polish](ui-polish.md) for logs, audit scope and remaining checks. Binding
90% overall / 80% subsystem gates remain unchanged.

The complete instrumented run `/tmp/dar-polish-nightly-oct08f.log` passes 268
tests / 5,611 ordinary assertions, 62 native tests, 21 browser journeys, external
DAV, 112,747 expanded assertions and three repeated five-test lifecycle rounds.
Its coherent coverage gate fails at 81.10% (22,676 / 27,960), with the unchanged
90% overall / 80% subsystem requirements. Artifacts:
`/tmp/dar-polish-coverage-oct08f.wMPl9s/report.json`.

The subsequent keyboard/public-access audit fixes Home/search focus, Chat's
nested main landmark, app-password draft/one-time-reply ownership, public drop
input overflow and public-share request disposal. It strengthens the SPA sweep
with destination-heading/launcher checks and visible keyboard entry. The new
`workspace-access.mjs` proves route/lost-reply recovery and real DAV scope and
revocation. `share-visitor.mjs` adds all five widths/themes for login and five
share states, long filenames and held-request cancellation through history.
Focused final proof passes in `/tmp/dar-polish-final-focused-oct08c.log`. Fresh
fast/nightly/coverage for this source snapshot is in progress; no app is marked
complete on the strength of the earlier 81.10% report.

Collection h passed fast but failed before browser acceptance on the native Chat
attachment fixture's separate SQLite connection. Test-owned connections now
have a bounded busy timeout and the focused native journey passes. A subsequent
Chat audit fixes delayed opens/mutations/paging, editor ownership on same-ID
reopening, preservation of text typed during send and historical-link recovery.
Both expanded Chat browser journeys pass, with real committed-reply holding and
API/session-storage readback. `chat-selection.mjs` is included in PR/nightly.
Fresh collection i is frozen at `/tmp/dar-polish-coverage-oct08i.xzth7P`. Fast,
PR stages (268 ordinary unit tests / 5,611 assertions, 62 native tests and 23
browser journeys) and the real external DAV round-trip pass. Its driver received
SIGTERM during the expanded parser phase, so that command exited 143. The
remaining nightly stages were rerun against the unchanged compiled snapshot:
268 tests / 112,747 assertions and all three five-test lifecycle rounds pass.
Logs: `/tmp/dar-polish-fast-final-oct08i.log`,
`/tmp/dar-polish-nightly-oct08i.log` and
`/tmp/dar-polish-nightly-tail-oct08i.log`.

The fresh source-mapped gate fails at **81.04% (22,738 / 28,058)**; 24 subsystems
also remain below 80%. Report:
`/tmp/dar-polish-coverage-oct08i.xzth7P/report.json`; diagnostic log:
`/tmp/dar-polish-coverage-report-oct08i.log`. Earlier collections were not merged
into it. Restoration is incomplete despite the passing workflow checks. Physical
on-screen keyboard behavior and exhaustive all-control keyboard/zoom inspection
remain unverified, alongside the app-specific acceptance gaps.

The final CSS-only Email spacing fix has separate integrated proof. Four real
inbox views at 320×568/390×568 in light/dark themes require the first subject
above phone navigation. They pass alongside the complete 130-view app sweep,
bulk/draft/recovery scenarios and existing Email journey in
`/tmp/dar-polish-short-inbox-after-oct08e.log` and
`/tmp/dar-polish-short-inbox-email-oct08e.log`. Screenshots:
`/tmp/dar-polish-screens-466xuR`. Collection i's compiled JavaScript/maps remain
unchanged; the CSS follow-up does not change its unmet coverage gates.

The baseline contains 109 Go test files and passes `./test/run.sh fast` at tag
`dar-go-baseline-2026-08-27`. Port behaviors, not Go implementation details.

## Verification tiers

- `fast`: compile server/browser test builds, static checks, Node-side unit and
  service tests, browser component tests, and focused contract tests.
- `pr`: `fast` plus concurrency/lifecycle stress, full-stack Playwright, raw DAV
  suites, protocol integrations, source-mapped coverage, and plugin scenarios.
- `nightly`: `pr` plus fuzz/property runs, repeated lifecycle races, real hermetic
  `mbsync`, crash recovery, and representative external DAV clients.

All state lives under test-owned temporary roots. Tests never use live databases,
data roots, Codex homes, plugin directories, credentials, or browser profiles.

## Required scenario families

- Core: OIDC state/nonce/PKCE and callback errors, session lifecycle, app-password
  scopes, admin claims, profile/settings, CSRF/origin, security headers, body
  limits, trusted proxies, gzip, rate limits, health, and graceful shutdown.
- SPA: authenticated shell, accessibility, keyboard/focus, dashboard, search,
  notifications, settings, editor interactions, responsive routes, and absence of
  browser-native prompt/confirm dialogs.
- Database/storage: migrations, foreign keys, WAL, serialized writes, atomic
  files, path normalization, traversal, reserved paths, symlink rejection,
  quotas, ETags, stale writes, journals, crash recovery, and reconciliation.
- Search: user scoping, provider generations, staging/reconciliation, incremental
  update/delete, cursor bounds, preferences, and stable result URLs.
- Activity/notifications: recipients, outbox retry, cursor ordering, RSS tokens,
  deduplication, preferences, dismissal, SSE replay/heartbeat, and disconnect.
- Calendar: CRUD, ICS, CalDAV, recurrence, exceptions, alarms, subscriptions,
  birthdays, import/export, widgets, search, activity, assistant tools, and parser
  properties/fuzzing.
- Contacts: CRUD, rich VCF fields, groups, CardDAV, books, import/export,
  preferences, Me-card, recipients, search, activity, assistant tools, and parser
  properties/fuzzing.
- Todo: list/task CRUD, sharing, trash/restore/purge, recurrence occurrences,
  VTODO DAV, preferences, widgets, search, activity, and assistant tools.
- Notes: Markdown IDs, external rename/edit reconciliation, conflicts,
  categories, versions, undo, attachments, preferences, search, widget, activity,
  and assistant tools.
- Files: confinement, symlink rejection, atomic writes, quotas, leases/journal,
  metadata, versions, trash, tags, favorites, public/user/drop shares, passwords,
  rate limits, comments, locks, diff, preview, thumbnail, WebDAV, mail virtual
  mount, search, widget, activity, assistant tools, and path fuzzing.
- News: discovery, fetch/SSRF, folders/feeds/items, OPML, archive, sanitization,
  enclosures/video, refresh lifecycle, search, activity, assistant tools, and
  feed/parser fuzzing.
- Email: discovery, encryption/rotation, Maildir and mbsync generation, indexing,
  threading, MIME, SMTP, drafts, attachments, operations, sync/status,
  notifications, search, widget, activity, assistant tools, parser fuzzing, and a
  hermetic real-mbsync integration.
- Codex: isolation, version/capacity/LRU/idle, crash/restart, environment
  redaction, protocol multiplex/RPC, provider tools, approvals, interruption,
  replayable SSE, sessions, attachments, search, activity, and shutdown.
- Prayer: calculation methods, Asr, high-latitude, DST, polar dates, Todo sync,
  analytics, lifecycle, and widget.
- Weather: geocoding, cache/stale fallback, units, settings, outbound failures,
  and widget.
- Plugins: valid install, invalid manifest, incompatible API, checksum mismatch,
  unsafe Git tree, size limits, migration failure, activation failure, client
  failure, live disable/reload, rollback, uninstall, purge, namespaces, and
  disposer completeness for routes/timers/SSE/assistant/widget contributions.

## Release gates

- At least 90% source-mapped statement coverage overall and 80% per substantive
  subsystem, excluding generated output and declarative schemas.
- No unresolved parity-ledger entries beyond the approved exceptions.
- Full browser verification includes loading, empty, success, validation,
  conflict, unavailable/stale, and destructive-confirmation states with a clean
  console and confirmed persistence.
- Shutdown leaves no listener, timer, SQLite handle, worker, SSE client, Codex
  process, `mbsync` process, Git process, or temporary staging directory behind.

## Coverage collection

`./test/run.sh coverage` builds release bundles with fresh source maps, collects
Node V8 execution from the unit/process suites and detailed Chromium execution
from every browser journey, then enforces the 90%/80% gates. Artifacts remain in
the printed isolated `/tmp/dar-coverage.*` directory after failure for review.
This dedicated gate must pass before restoration can be declared complete;
ordinary `pr`/`nightly` workflow checks currently run separately.

The existing Closure build parser identifies executable JavaScript statements.
Positions map back to Dar ClojureScript; compiler/library statements without a
Dar mapping and named/explicitly marked declarative schema definitions are
excluded. Duplicate source positions emitted by arities or multiple builds merge
their execution. The innermost V8 block determines coverage; a covered parent
cannot hide an unexecuted child. Browser script hashes and map freshness checks
reject inconsistent artifacts. The gate requires both compiled server and real
browser data, and reports each app, client module and platform module separately.
Coverage complements the required workflow and state checks; it cannot establish
behavioral parity by itself.

The first combined collection on 2026-10-05 measured 74.34% and failed the overall
gate. Required app workflows and acceptance work remain open. No threshold was
lowered. The representative external DAV-client run now passes: nightly and
coverage invoke vdirsyncer from the existing locked nixpkgs input, with an
explicit isolated config and local portable directories. `test/dav-client.mjs`
proves CalDAV/CardDAV/VTODO discovery, local uploads and edits, server edits
downloaded after restart, stale-write rejection, local deletion and task
restoration. It never reads personal client settings or uses external accounts.

A fresh collection after native ChatGPT authentication, News media/bulk tools,
settings conflict recovery, Activity widget acceptance and DAV compatibility
passed 229 unit/service tests / 5,034 assertions, 49 process tests, all 13 browser
suites and the real client fixture. Its coverage gate still fails at 75.19%
(19,322 / 25,698 statements). Artifacts: `/tmp/dar-coverage.j3FyRB`; diagnostic
log: `/tmp/dar-restoration-coverage-oct05-latest.log`. This snapshot predates the
following task-zone and server-entry discovery changes. The 90% overall and 80%
module gates remain binding. It also predates Calendar's durable event journals,
assistant workflow fixture and deleted-object navigation proof; do not combine
these raw coverage files with subsequently rebuilt bundles or maps.

The integrated nightly run `/tmp/dar-dav-todo-news-integrated-nightly-oct05.log`
passes the task-zone and server-entry discovery increment: 230 unit/service
tests / 5,056 assertions, 49 process tests, 13 browser journeys, the real DAV
client, expanded parser properties and three repeated lifecycle rounds. The
subsequent Calendar increment passes 232 tests / 5,079 assertions plus focused
DAV/API/concurrency/Activity, all sixteen Calendar assistant tool workflows and
Calendar/Activity browser checks. Its integrated PR run
`/tmp/dar-calendar-journal-owned-startup-pr-oct05.log` passes all 232 tests /
5,079 assertions, 50 process tests and 13 browser suites. This predates the
subsequent Notes recovered-draft revision fix, whose existing Notes journey and
new Files editor → Notes comparison journey pass independently. Overall
restoration and coverage gates remain incomplete. The first full
run failed the Email TLS restart fixture, which passed in isolation; the harness
now waits for its own child's completed startup instead of accepting the earlier
health listener. The next run passed all 50 process tests but stopped at a syntax
error in the expanded Calendar browser test. That error is fixed, and the new
draft-recovery/mobile conflict journey passes independently. Neither failed full
run counts as an integrated pass. A later process run exposed the startup port
probe/bind race; the harness retries that specific pre-ready bind failure with
a fresh port, stops only its own child and propagates other startup failures.

The recovered-draft/detail increment passes fresh fast and nightly verification
in `/tmp/dar-restoration-drafts-integrated-nightly-oct06.log`: 232 tests / 5,079
assertions at ordinary sample counts, 50 native process tests, all 15 browser
journeys, the external DAV client, 232 tests / 112,215 assertions with the
expanded parser samples and three repeated lifecycle rounds. It preserves the
Files/Contacts/Calendar suites and adds scoped Files draft and Files → Notes
journeys. This coherent snapshot predates the shared JSON parsing fix and its
new API-boundary proof. The seeded 4180 instance uses this snapshot; current
coverage remains unverified until a fresh collection completes.

The shared JSON-boundary increment adds `test/api-json.test.mjs`, which passes
malformed/empty-body requests across app mutations, personal/appearance settings,
dashboard layout, app passwords, generic preferences and assistant invocation.
Each returns `400 invalid`, retains a request ID and leaves saved state intact.
Unit proof distinguishes asynchronous/synchronous syntax failures from transport
failures. The first coverage attempt stopped because some existing error paths
passed a null request ID; the shared envelope now generates an ID when one is
absent. Focused native proof passes after that fix. A fresh full collection is
still required; failed collection artifacts are not combined with rebuilt maps.

The repaired JSON-boundary collection passes 233 unit/service tests / 5,084
assertions, 51 native tests, all 15 browser journeys and the external DAV client.
Coverage remains below the binding gates at 76.41% (19,848 / 25,977 statements).
Artifacts: `/tmp/dar-coverage.rJDMPP`; log:
`/tmp/dar-restoration-drafts-json-coverage-fixed-oct06.log`. It predates the new
Prayer/Todo isolation and browser journey, including the requested-date race
fix. Do not merge its raw execution with later rebuilt bundles or maps.

The Prayer/Habit/Files/widget snapshot passes integrated nightly verification in
`/tmp/dar-prayer-habit-files-widget-integrated-nightly-oct06.log`: 233 tests /
5,084 assertions, 52 native tests, all 17 browser journeys, real vdirsyncer,
233 tests / 112,220 assertions at expanded sample counts and three lifecycle
rounds. It includes generated Prayer tracking across Todo and two accounts,
Habit flags/progress, account sharing/clipboard, multi-copy/move, keyboard
actions, picker retry/late-response isolation, rendered local image insertion
and recoverable Weather widget settings. It predates the screenshot follow-up
for synchronous keyboard defaults/widget form spacing and the assistant →
Files/search/Activity browser journey. The latter snapshot passes fast checks
with 233 tests / 5,084 assertions; focused release/browser/native verification
passes. The same mapped collection resumed after Email's lost-connection fixture
failed once; Email passes both its focused rerun and the instrumented rerun,
followed by the real DAV client. Coverage is 76.85% (20,010 / 26,037 statements),
below the binding 90% overall/80% subsystem gates. Artifacts are retained at
`/tmp/dar-coverage.gGoA2a`; report log:
`/tmp/dar-current-restoration-coverage-report-oct06.log`. The single Email
fixture failure remains under investigation. This snapshot predates incoming
Files share browsing; do not combine its data with subsequent rebuilt bundles.

The next snapshot's focused incoming Files share, incoming Email integration and
Workspace settings journeys pass. Files owns shared-path requests in a Cordis
context, verifies cancellation, read/write controls, downloads, multiple uploads,
revocation and owner/recipient isolation; the original Files journey also passes.
Email uses real hermetic mbsync and proves arrival → live Home → notification →
reader → global body search → Activity → reader, with duplicate-delivery checks.
Profile and appearance recovered drafts with missing/stale revisions require
review before saving, retain reviewed revisions across reload and clear on
choosing the current version. Screenshots/console were reviewed; the matching
integrated nightly run at
`/tmp/dar-restoration-shares-mail-settings-nightly-oct06.log` passed the 233
unit/service tests, 52 native tests and the first 13 browser journeys, then
exposed a Prayer day-navigation race. Completion controls still referred to the
previous day while the requested day's times loaded. That run is not an
integrated pass. The controls now disable while loading and the date field
shows the requested selection immediately; the held-response Prayer/Todo
fixture explicitly checks that old rows cannot be completed.

The repaired snapshot passes 237 unit/service tests / 5,138 assertions, 52
process tests, all 19 browser journeys and real vdirsyncer. Discovery proof now
includes bounded HTTPS fallback, capacity and disposal cancellation. Activity
RSS/Home retain title-only objects; Email operation entries retain their message
subjects. Coverage still fails at 76.87% (20,124 / 26,180 statements), with the
90% overall/80% subsystem gates unchanged. Artifacts:
`/tmp/dar-coverage.pTmI4i`; log:
`/tmp/dar-restoration-shares-mail-prayer-coverage-fixed-oct06.log`.
The preceding attempt stopped at a syntax error in the new discovery fixture;
that failed attempt is not combined with this fresh collection. This snapshot
predates Todo/Notes settings draft recovery and live Todo sharing polish.

The same compiled snapshot also passes expanded parser/property samples with
237 tests / 112,274 assertions and three repeated lifecycle rounds after a
test-only fault-injection connection was given a bounded SQLite busy timeout.
Its first lifecycle attempt failed while installing the Chat rollback trigger
beside the running server's short writer transaction. No production database
policy changed. Logs: `/tmp/dar-restoration-shares-prayer-nightly-tail-oct06.log`
and `/tmp/dar-restoration-shares-prayer-lifecycle-fixed-oct06.log`. This completes
that snapshot's applicable nightly checks, while coverage still fails and the
subsequent client changes still require integrated verification.

Later focused checks pass Todo preference/list draft recovery, failed comparison
and member-read retry, two-user live permission changes and revocation, and
closing details during a held successful save. Notes settings recovery/reversal,
failed comparison retry and closed-editor save replies also pass. Activity's
expanded browser journey passes filtered cursor pagination, feed retry,
revision-backed recovered RSS drafts, a held deep-link response after changing
source and live recipient removal. Chat completion/recovery subjects retain
their conversation title in the native turn journey. The unit run passes 238
tests / 5,143 assertions. A fresh integrated collection is required for these
changes; the preceding coverage result is not reused.

That fresh collection passes 238 unit/service tests / 5,143 assertions, all 52
native process tests, all 20 browser journeys and real vdirsyncer. Coverage fails
at 76.64% (20,190 / 26,343 statements), with the binding gates unchanged.
Artifacts: `/tmp/dar-coverage.GN5HIy`; log:
`/tmp/dar-restoration-reviewed-settings-coverage-oct06.log`. Its applicable
nightly tail also passes 238 tests / 112,279 assertions and three rounds of all
five lifecycle fixtures; log:
`/tmp/dar-restoration-reviewed-settings-nightly-tail-oct06.log`. These results
predate the later timed Todo ordering and owned DNS changes.

A hermetic browser fixture reproduced lost precise coverage during immediate
reload/navigation: two executed marker calls were recorded as zero. The browser
collector now drains before `goto`, `reload`, `goBack` and `goForward`, while
retaining its periodic and teardown drains. The same fixture records both calls.
This repairs evidence collection, not the coverage thresholds or denominator.

The current navigation/Settings/DNS snapshot passes the applicable nightly tail:
239 unit/service tests with 112,488 assertions at 4,096 parser samples, plus
three rounds of the five Chat/Email/events/Todo lifecycle fixtures. The original
stdio harness exits after a forced server crash in each round, and no orphaned
turn fixtures remained. Log:
`/tmp/dar-restoration-navigation-nightly-tail-oct07.log`.
The fresh integrated coverage run is still in progress; this tail alone is not
full restoration acceptance.

The collector repair's first complete measurement is 81.50% (21,501 / 26,382
statements), still below the unchanged 90%/80% gates. Artifacts:
`/tmp/dar-coverage.6Tcywh`; logs:
`/tmp/dar-restoration-navigation-complete-coverage-oct07.log` and
`/tmp/dar-restoration-navigation-coverage-tail-oct07.log`. The original browser
pass stopped at an intermittent Prayer Settings loading transition. A rerun
and the remaining journeys, including external vdirsyncer, completed against
the unchanged bundles for this measurement. That rerun does not resolve the
earlier product failure; a deterministic held-analytics proof and fix follow.

A later malformed async fixture exposed another verification boundary: Shadow
can drain without its final reporter and leave Node's default exit code zero.
The unit commands now preload a failure exit code; only Shadow's explicit
completion reporter can declare success. A real Node subprocess proof covers
abandoned execution and successful/failed completion. The corrected fixture
and guarded fast run pass 242 tests / 5,389 assertions. This is a verification
repair, not permission to reinterpret earlier failed checks as successes.
Prior percentage snapshots remain historical; acceptance needs a fresh run.

The Todo time-zone/Notes deep-link/Prayer analytics snapshot passed 242 guarded
unit tests / 5,389 assertions and all 55 process tests. Its integrated command
was interrupted without a test failure after 18 browser journeys; the two
Email journeys and real vdirsyncer subsequently passed against the unchanged
bundles. Coverage is 81.71% (21,727 / 26,592 statements), below the unchanged
90%/80% gates. Artifacts: `/tmp/dar-coverage.ywaAw3`; logs:
`/tmp/dar-restoration-zones-notes-coverage-oct07.log` and
`/tmp/dar-restoration-zones-notes-coverage-tail-oct07.log`.
The same snapshot passed 242 tests / 112,525 assertions at expanded parser
samples and three rounds of the five lifecycle fixtures; log:
`/tmp/dar-restoration-zones-notes-nightly-tail-oct07.log`. Subsequent notification
restoration changes need their own fresh integrated verification.

The desktop-notification/authentication snapshot passed 246 guarded unit tests /
5,437 assertions, all 56 process tests, all 20 browser journeys, and real
vdirsyncer. Two additional News/Email assistant journeys passed against the same
unchanged release before reporting. Coverage is 81.78% (21,917 / 26,799), below
the unchanged 90% overall / 80% subsystem gates. Artifacts:
`/tmp/dar-coverage.Dd9n7e`; logs:
`/tmp/dar-restoration-notification-auth-coverage-oct07.log` and
`/tmp/dar-assistant-news-email-integration-oct07.log`.
The same snapshot passed the expanded property suite and three rounds of the
five lifecycle fixtures; log:
`/tmp/dar-restoration-notification-auth-nightly-tail-oct07.log`.
Subsequent Email bulk-operation and conversation-search changes need fresh
integrated verification. None of these results constitute restoration acceptance.

The Email bulk/conversation/recipient/search and Home normalization snapshot
passed 248 guarded unit tests / 5,486 assertions, all 59 process tests, all 20
browser journeys, and real vdirsyncer. Its 81.81% coverage (22,146 / 27,070)
still fails the unchanged 90%/80% gates. Artifacts: `/tmp/dar-coverage.JPHmmZ`;
log: `/tmp/dar-restoration-email-bulk-home-coverage-oct07.log`.
The unchanged snapshot also passed 248 tests / 112,622 expanded assertions and
three rounds of the five lifecycle fixtures, including the new uncertain-send
organization guard; log:
`/tmp/dar-restoration-email-bulk-home-nightly-tail-oct07.log`.
Email's selection toolbar still needs its maintainer mock selection and browser
acceptance; the overall restoration remains incomplete.

The following recovery work passed 261 guarded unit tests / 5,582 assertions.
Focused tests cover pin load/write failure and previous-session replies,
coalesced Home refreshes, queued work after closing Home, conflict cancellation,
immediate session cache clearing, shared notification badge ownership, scoped
draft storage, storage failure, personal date formatting and listener disposal.
Three affected browser journeys (Workspace, incoming Email, Email) passed the
shell cancellation and Email widget account-selection changes. Four focused
process journeys passed native/assistant draft discard, including consent,
revision/scope checks, idempotent receipts and uncertain-send preservation.
Logs: `/tmp/dar-shared-draft-session-unit-oct07.log`,
`/tmp/dar-session-notification-widget-browser-oct07.log` and
`/tmp/dar-draft-discard-native-oct07.log`.

A delayed Email composer save reproduced loss of a newer reopened draft in
`/tmp/dar-email-composer-ownership-before-oct07.log`. The initial callback
ownership fix passed the real browser in
`/tmp/dar-email-composer-ownership-browser-oct07.log`. Per-composer write controls
then passed the full Email journey in
`/tmp/dar-email-composer-owned-controls-oct07.log`: the submitted composer locks
its fields during filing, Escape remains usable while Save is disabled, and a
reopened composer can edit and retain newer text across the older save reply.
These focused results do not
replace a fresh full restoration coverage/nightly collection.

The subsequent coverage collection passed 261 guarded unit tests / 5,582
assertions before its process ended with signal 143. Against the unchanged
release, its continuation passed all 59 native tests and the first eight browser
journeys, then failed Activity's retry fixture when an SSE-ready refresh removed
the retry button. The fixture now injects failure after the initial successful
load. Activity and the next five journeys passed, but Prayer's completion undo
then failed during its times/analytics transition. No complete coverage or
nightly result exists for this snapshot; `/tmp/dar-coverage.sZTOM6` is an
incomplete historical collection. Logs:
`/tmp/dar-restoration-session-composer-coverage-oct07.log`,
`/tmp/dar-restoration-session-composer-coverage-tail-oct07.log`, and
`/tmp/dar-restoration-session-composer-browser-remainder-oct07.log`.

A held-analytics fixture reproduces Prayer's enabled completion control during
the remaining refresh. Both Prayer browser journeys pass after keeping it
disabled through the full refresh. Email's expanded settings journey passes
held-response recovery for account details, preferences and folder roles;
Prayer's newer calculation draft also survives its older saved response. Logs:
`/tmp/dar-email-settings-owned-browser-oct07.log` and
`/tmp/dar-prayer-settings-owned-browser-oct07.log`. Earlier account-setting
diagnostics targeted an Add account form before selection finished; they do
not count as valid product-failure proof.

Two hermetic OIDC native tests pass in
`/tmp/dar-oidc-security-boundaries-oct07.log`: substituted state/nonce/PKCE,
missing subject, consumed failed flows, successful callback replay, subsequent
valid login, three-cookie preservation, and unconfigured setup errors. These
focused results do not replace fresh integrated restoration acceptance.
