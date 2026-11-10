# Agent workflow refinement — November 9

Selected option B in `agent-flow-refinement-mocks.html`. This replaces the
project-grouped sidebar with account-scoped active threads and Working/Settled
shelves. New thread chooses a project, then focuses the composer. The first
message supplies the initial title. Adding a project infers its name from the
Git URL or the currently browsed Files folder, with one final Add action.

## Live T3 reference

Ran installed T3 Code 0.1.6 on loopback port 44321, with separate temporary state
and synthetic projects under `/tmp/dar-t3-reference`. Inspected source selection,
local folder browsing, Git URL entry, multi-project chooser, model/reasoning
selector, settings, provider status, thread actions, Working state, completion,
terminal/surface selection and settlement. A live task created `hello.txt` in
the fixture only. Its automatically generated title became “Create Temporary
Hello Fixture”. Screenshots and the full local report are in
`/tmp/dar-t3-reference/workflow.html`; provider screenshots containing account
information are excluded from that report.

T3's default sidebar is thread-focused; project grouping is an optional Labs
mode. Working threads return to active work after completion. Settling is an
explicit lifecycle action, rather than a synonym for every completed turn.

## Implementation

- `DarFilePicker` is an inline shared component. Its caller owns the final action.
  Folder mode selects the current directory; file mode selects a file row.
  It returns logical IDs, supports root opt-in, breadcrumbs, bounded paging,
  page filtering, loading/error/retry, exclusions and keyboard use. Loading or
  access loss clears selection. Agent and Files copy/move use the same component.
- `/api/v1/files/picker` is independent of Agent availability. Native-session
  ownership and inherited read access are checked inside the database. Responses
  contain names, kinds and IDs, never blob paths or service credentials.
- Thread listing supports the whole current account and reports latest run state.
  Settlement uses revisioned account preferences. Later messages invalidate its
  recorded revision and return the thread to active work. No table migration.
- Provider checks re-read server configuration and CLI login immediately. Active
  runs retain their configuration snapshot. Login/catalog/configuration errors
  are distinct, and browser refresh shows progress. Administrator-only connection
  discovers the service CLI/home and exclusively creates the account binding.
  It never overwrites another binding or imports browser-supplied paths.

## Deliberate differences

Dar keeps direct `codex exec` as requested, although T3 0.1.6 uses app-server.
Dar currently derives its initial title from the first message; T3 uses separate
model-generated titles. Worktrees, publishing, snoozing, pinning, scheduled runs,
multiple providers and title regeneration remain outside this change. The T3
fixture was not a Git repository: the review surface was inspected, but a populated
Git diff was not verified there. Dar's actual reviewed Files write-back is covered
by its isolated Codex browser acceptance.

Focused acceptance covers immediate configuration/login recovery, binding
isolation, contextual titles, settlement persistence, drafts/replays/conflicts,
shared picker isolation, Files copy persistence, Codex runs/write-back and both
themes at desktop/tablet/phone sizes. Run `nix develop -c npm --prefix next run verify`
for the complete suite.

Verification completed November 9: full `npm --prefix next run verify` passed,
including shared UI, Files, account preferences, backup/restore and crash/restart
checks. A final focused `--agent` rerun passed after bounding Unicode-derived
titles to the backup format's byte limit. Typechecks and `git diff --check` pass.
All Dar acceptance used isolated temporary roots and accounts. No deployment.
