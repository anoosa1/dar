# Agent workspace

Optional built-in app at `/agent` for coding work on a Git repository or a Dar
Files folder, using Codex or Claude Code. Layout follows the approved T3 Code–style composition
(November 8–9, 2026) inside Dar's shared frame (November 11). Server setup, the
run protocol and limits are in [Codex](../codex.md) and [Claude Code](../claude-code.md); UI rules are in the
[design guide](../design.md).

## Features

- **Projects:** a credential-free Git URL or an authorized Files folder (picked
  with the shared Files picker), named from the source. Never a host path.
- **Threads:** New thread opens a draft with a project control; the thread is
  created on save or run and titled from the first message. Pin, rename (menu,
  double-click, F2), settle/restore, snooze with a Woke badge, archive, delete,
  fork from any message, copy link, Markdown copy/export and read-only sharing
  with other Dar accounts. Working, Snoozed and Settled shelves; unread markers
  and optional completion notifications per browser.
- **Composer:** Enter sends (configurable), Shift+Enter adds a line, save-only in
  the menu, model and reasoning choice, plan mode, image attachments, `/` commands,
  `@` file mentions, prompt history, steering a running turn, one queued follow-up
  and a context meter. Codex questions appear as an answer card.
- **Conversation:** safe Markdown replies, a typed work log with command output,
  find in thread, scroll restoration, jump to latest and a timeline minimap.
- **Review and output:** changed-files card and a review panel with per-file
  diffs (collapse, wrap, copy, download), Codex code review and compaction, a
  read-only output drawer and a sandboxed terminal, Git status/branch/commit,
  usage limits and token counts. Text edits to Files are applied only after review.
- **Keys:** Ctrl/Cmd+P palette, Ctrl/Cmd+Shift+O new thread, Ctrl/Cmd+B sidebar,
  Ctrl/Cmd+F find. Ctrl/Cmd+K is the shared cross-app search.

## Deliberate differences from T3

Dar is multi-user and server-side, so approvals and access-mode switching are not
offered (every escalation is declined), workspaces have no network, and Git push,
pull requests, scheduled tasks, device pairing and secrets are not provided.
Full-screen terminal programs degrade to text.

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
