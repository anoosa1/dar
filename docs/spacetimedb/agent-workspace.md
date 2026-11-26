# Agent workspace

Optional built-in app at `/agent` for coding work on a Git repository or a Dar
Files folder, using Codex or Claude Code. Layout follows the approved T3 Code–style composition
(November 8–9, 2026) inside Dar's shared frame (November 11). Server setup, the
run protocol and limits are in [Codex](../codex.md) and [Claude Code](../claude-code.md); UI rules are in the
[design guide](../design.md).

## Features

- **Projects & Scratch:** a credential-free Git URL, an authorized Files folder (picked
  with the shared Files picker), or an instant `"Scratch"` workspace (*Start without a
  project*), named from the source. Never a host path.
- **Threads & Sidebar:** New thread opens a draft with a project control; the thread is
  created on save or run and titled from the first message. Pin, rename (menu,
  double-click, F2), settle/restore, snooze with a Woke badge, archive, delete,
  fork from any message, copy link, Markdown copy/export and read-only sharing
  with other Dar accounts. Filter pills (`All`, `Working`, `Unread`, `Pinned`),
  Working/Snoozed/Settled shelves, unread markers, optional completion notifications,
  and a bottom **Utility Dock** (`Threads`, `Usage`, `PRs`, `Automations`).
- **Composer:** Enter sends (configurable), Shift+Enter adds a line, save-only in
  the menu, two-tab provider Model & Reasoning picker with starred models, plan mode
  (with a Proposed Plan action card + *Implement plan*), image attachments, inline
  **context chips** (from Files, Split Diff line comments, Side Chat, PRs, or Terminal
  output), **Prompt Stash**, `/` commands, `@` file mentions, prompt history,
  configurable follow-up behavior (multi-item **Queue** tray or immediate **Steer**),
  auto-clearing transient notices, and a context meter. Blocking questions render as
  horizontal selectable option cards with numeric key hints.
- **Conversation:** safe Markdown replies, a typed work log with clickable changed files
  and command output, find in thread, scroll restoration, jump to latest and a timeline minimap.
- **Multi-Surface Right Panel & Multi-Tab Terminal:**
  - Tabbed right panel with `Changes` (**Unified / Split** diff viewer, inline line review
    comments attached to composer, **Revert file** to baseline, collapse, wrap, copy, download,
    and Files History branch commit/merge), `Files` (working-copy browser + code/Markdown/image
    viewer), `Side chat` (parallel scratchpad), `PRs & Issues` (live Dar Git integration),
    and `Preview` (strictly sandboxed HTML/SVG/Markdown preview).
  - **Project Scripts** saved per project in the Details card and launched directly into
    dedicated tabs inside the **Multi-Tab & Split-Pane** sandboxed terminal drawer
    (with *Send to composer* output capture alongside read-only Agent output).
- **Top-level Workspace Pages & Settings:**
  - `Usage & limits` (styled Twilight rate-limit bars, provider connection cards, 7-day
    activity chart, model starring), `Pull requests & issues` (Dar Git hub), and
    `Automations` (saved reusable prompt schedules with one-click *Run now*).
  - Searchable categorized **Agent settings** modal on desktop and native full-width
    **Hub → Section drill-down** on phones.
- **Keys:** Ctrl/Cmd+P palette, Ctrl/Cmd+Shift+O new thread, Ctrl/Cmd+B sidebar,
  Ctrl/Cmd+F find. Ctrl/Cmd+K is the shared cross-app search.

## Deliberate differences from T3

Dar is multi-user and server-side, so approvals, access-mode switching, remote T3 Connect
sync, device pairing and host-level secrets are not offered (every escalation is declined),
workspaces have no network, and remote Git push is disabled. Full-screen terminal programs
degrade to text.

## Data and safety

- Native private tables for projects, threads, messages, runs and shares, under
  `/api/v1/agent`. Limits per account: 32 projects, 128 threads, 1,000 messages of
  at most 16 KiB; history pages of 50.
- Messages append with the expected thread revision and a client operation ID, so
  retries are idempotent and conflicting writes fail. Drafts and uncertain sends
  are kept per account/thread in the tab's session storage.
- Preferences (pinned, archived, settled, snoozed, send key, grouping,
  notifications) are revision-checked account preferences.
- Runs are queued transactionally and claimed with a lease; one active run per
  account. Every 500 ms the run rechecks the session, app availability and source
  access, so logout, suspension, revocation or disabling stops it. Restarts mark
  uncertain runs interrupted and never replay commands.
- Each thread has an isolated working copy outside Dar's data; Codex credentials,
  Dar's root and database credentials are never exposed to it.
- Backups (v6) keep projects, threads, messages, runs, review data and shares,
  not working copies or credentials. Deleting an account deletes its Agent data.
  Disabling Agent keeps data and hides it from search.

Verify with `nix develop -c node next/scripts/verify.mjs --agent` (isolated CLI
fixture; no live model use).
