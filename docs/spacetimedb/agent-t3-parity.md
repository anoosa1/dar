# T3 Code parity audit — November 9, 2026

Status: option A selected. The compact frame and existing-data interactions are
implemented; the reference requirements below also include capabilities
that need separate data/runtime work. This is not a claim of complete T3 parity.

The requested target is the T3 Code experience, including interactions and state,
not merely a three-column layout. The previous workflow refinement implemented
only a small subset. This ledger supersedes its implication of broad UX parity.

## Evidence and limits

Reference: the locally installed T3 Code 0.1.6 client, inspected through its bundled
source maps. Reference names below are original client source paths, relative to
`src/`. Extracted reference code is temporary and is not added to Dar. This is a
behavioral specification for an original Dar implementation.

The earlier isolated reference session and its screenshots are recorded in
[the workflow audit](agent-workflow-audit.md). Its loopback service is no longer
running (connection refused during this audit). This pass inspected source and
existing fixture screenshots; it did not exercise every branch in a fresh live
session. Source presence does not prove a feature is enabled by default or
available to every provider. Diff behavior here is source-inspected, not a claim
that the earlier non-Git fixture verified a populated diff.

Dar inspected: `next/client/agent/workspace.tsx`, `workspace.css`,
`next/shared/agent.ts`, and `next/runtime/agent/codex.ts`.

Legend: **UI** can be built on existing data; **State** needs durable state or
contract work; **Runtime** needs execution capabilities; **Shell** is shared Dar
or desktop integration. These are implementation classifications, not completed
work. The tables record the baseline before this change. The implementation
record below identifies what has since changed; remaining rows are not implicitly
complete because their surrounding panel has been redesigned.

## Selected A implementation

- Compact sidebar chrome with the shared application launcher and account actions;
  no full-width global header on Agent. Other apps retain their header. Thread
  search, Working/Settled shelves and project-over-title rows use real saved data.
- Searchable project chooser in the shared dialog, then focused composer; source
  creation still uses the shared Files picker and source-derived names.
- Contextual project/connection card and on-demand wide review, with maximize and
  close controls. Navigation, details, review and command-output visibility persist
  per account in this browser. Phones expose details as a separate focused panel.
- Right-aligned user bubbles, shared safe assistant Markdown and hover/focus copy
  actions. HTML remains text and remote Markdown images are not fetched.
- A work summary sits immediately after its initiating message, using the latest
  run's actual activity. Live output appears once; completed replies come from saved
  messages. Earlier run documents and a typed event timeline are still missing.
- Scroll-follow releases while reading older messages; Jump to latest resumes it.
  Thread switches retain viewport positions during the current workspace visit;
  loading earlier messages preserves the visible anchor. Drafts retain the existing
  session recovery and idempotent-send/conflict behavior.
- Docked composer with circular send and real Stop, Enter to submit, Shift+Enter
  for newline, and a Save only menu command. A draft can be prepared during a run;
  this is not steering/queueing. Model choice is retained per thread in this tab.
- Find in loaded messages, match navigation, copy message/response/thread link,
  and row/heading settlement actions. Settlement remains revisioned account state.
- Recorded unified diffs gain old/new line gutters, added/removed treatment,
  line wrapping, collapse/expand, copy and download. Files writes still require
  explicit review and the original expected revision. Source diffs are full-content
  patches; this change does not claim minimal Git hunks, syntax highlighting or a
  side-by-side comparison engine.
- Command output remains explicitly read only. No attachment, slash-command,
  reasoning-effort, approval, worktree, publishing or terminal input controls are
  shown without working runtime support.

The backend schema, APIs and direct `codex exec --json` execution are unchanged.
The broader runtime parity work remains in sections 2–7 below.

Verification: full `nix develop --command npm --prefix next run verify` passed,
including shared shell behavior, persistence and crash/restart acceptance. The
final `nix develop --command node next/scripts/verify.mjs --agent` also passed
after the last Agent refinements. It covers reviewed Files writes, composer
cancellation and retained drafts, Enter/Shift+Enter, settlement, uncertain-send
replay, conflicts, thread search/find, scroll restoration, saved panel visibility
and desktop/tablet/phone layouts in both themes. Phone navigation labels are
checked for overlap at 320px. Desktop and phone screenshots were visually reviewed.
All verification used isolated temporary roots/accounts; no live data was tested.

## November 11 refinement

A second pass compared the same reference client's sidebar, timeline, composer,
draft and diff surfaces in detail and aligned Dar's measurements and interactions.
Original Dar code and Twilight tokens only; reference sources were read from a
temporary extraction that has been removed.

- Sidebar: 256px column; one-line 36px rows with a deterministic project mark
  (dimmed until hover, focus or selection), truncated title and relative age.
  Settle/restore and the row menu replace the age on hover or keyboard focus.
  Working rows show a live status. Working/Settled shelves sit below active rows.
- Draft-first New thread: a centered "What should we build in <project>?" headline
  above the composer; the project control opens the searchable chooser. The
  thread is created with a stable ID only on submit, then the draft is saved or
  run once its history loads. No projects opens Add project directly.
- Conversation: 768px column with 48px gutters, 80% user bubbles, hover metadata
  (time and copy) below messages, and 14px body text.
- Work log: live "Working for …" timer, then "Worked for/Stopped after/Failed
  after …"; expanded entries show recorded commands ("Ran …") and edits. Completed
  runs with changes add a changed-files card with per-file counts and Review.
- Composer: quiet 16px-radius surface without an accent focus ring; searchable,
  keyboard-operable model menu; 32px circular send/stop; no persistent hint.
- Review: stacked per-file sections with sticky headers, kind and +/− counts,
  collapse per file or all, wrap, per-file copy and download. Whole-file recorded
  patches are converted in the browser to minimal unified hunks with three context
  lines (bounded; very large middles fall back to replacement). Apply still writes
  the recorded full content with the original expected revision.

### Second November 11 pass: lifecycle, composer and typed runs

Authorized by the maintainer ("continue till you get all the features"). Contract
changes, all additive and backward compatible:

- Account preferences: `agentPinnedThreads`, `agentArchivedThreads` (thread
  IDs, max 128), `agentSendKey` (`enter`|`modEnter`), `agentNotify` and
  `agentGrouping` (`recent`|`project`), validated in the preferences route.
- `PATCH /api/v1/agent/threads/:id` renames (title only; message revisions are
  unchanged). `DELETE` removes messages, runs and the private working copy;
  refused while a run is active.
- `GET /api/v1/agent/threads/:id/files?query=` returns up to 50 relative paths
  from the thread working copy (symlinks never followed; .git, .codex and
  node_modules skipped), else the Files project source.
- Provider models carry `efforts`/`defaultEffort` from the CLI catalog's
  `supported_reasoning_levels`. Runs accept an optional `effort`, validated
  against that model and passed as `model_reasoning_effort`; it is stored in the
  run document and part of idempotent retry comparison.
- Run documents gain optional `steps` (command with exit code and 2 KB output
  tail, edit with paths, reasoning, plan items, tool, search; max 100) and
  `usage` (input, cached, output tokens). `activity` remains for older runs.

Client: pinned group, archive (restorable in Settings → Archived threads), inline
rename (menu, double-click, F2), delete confirmation, right-click row menu, unread
markers (per browser), optional completion notifications, project grouping,
Ctrl/Cmd+K command palette, Ctrl/Cmd+Shift+O new thread, Ctrl/Cmd+B sidebar,
resizable sidebar/review (pointer, arrow keys, double-click reset), slash
commands (/new /model /effort /review /terminal /find /rename /settle /settings),
@ file mentions, prompt history (Up/Down), one queued follow-up while running
(sent after a completed run; held with Send now otherwise), model + reasoning
menu, typed work log with expandable command output, lazy earlier-turn logs,
per-command output drawer, token usage and settings sections (General, Chat,
Sidebar, Archived threads, Keyboard shortcuts).

### Third November 11 pass: app-server runner

The maintainer approved replacing `codex exec` with `codex app-server` ("do it the
same"). Implemented and covered by integration and browser tests: resumed Codex
threads, steering into a running turn (Enter while running; Queue remains), blocking
questions with an answer card, plan mode, image attach/paste/drop with thumbnails,
context meter, a sandboxed interactive terminal tab, and local Git status/branch/commit
in the details card. Contract additions: run routes `steer` and `answer`, thread
routes `attachments/:id` (PUT/GET), `terminal` (open/read/input/resize/close) and
`git`; run documents gain mode, attachments, steered IDs, pending question, Codex
thread, context revision and context usage. See [codex.md](../codex.md).

Deliberate differences from T3 for multi-user server safety: approval prompts and
access-mode switching are not offered (all escalations are declined; T3 is a
single-user desktop app), and Git push/pull requests stay disabled (no network or
credentials in workspaces). The terminal renderer handles shells and line programs;
full-screen programs degrade to text. Not implemented: snoozing with timed wake-up,
sharing, timeline minimap, thread fork/compact/review commands, rate-limit display
and T3's device, secret and scheduling settings.

### Fourth November 11 pass: remaining lifecycle features

Implemented and tested (integration plus browser): fork from any message
(`POST /threads/:id/fork`, messages copied up to that sequence; the fork's first run
starts a new Codex thread), Codex code review of uncommitted changes (`/review`,
`review/start`; `/changes` now opens the panel), context compaction (`/compact`,
`thread/compact/start`), usage limits from `account/rateLimits/read` in Settings and
the details card, snooze with presets/custom time, a Snoozed shelf and a Woke badge
(`agentSnoozed` preference; wake notifications use the existing setting), Markdown
copy/export, a timeline minimap, and read-only sharing with other Dar accounts
(`agentShares` table, owner `/threads/:id/shares`, grantee `/shared` routes; included
in V6 backups, older backups restore without shares).

Not implemented, pending a maintainer decision: scheduled tasks (unattended runs
need an execution authority that does not depend on a live browser session), and
T3's device pairing and secrets (Dar is itself the server, and workspaces have no
network for secrets to be useful).

Earlier list, now superseded where implemented above — previously missing because the
direct `codex exec` runner had no channel or capability for them: image/file attachments, steering a running turn, approvals
and structured questions, access-mode switching (the sandbox policy is fixed),
plan mode, resume of a stopped turn, branches/worktrees, Git publishing and PRs,
an interactive terminal, snoozing with timed wake-up, sharing, timeline minimap,
and T3's provider/device/secret/scheduling settings. Each needs an interactive
runtime or new services and should be decided separately.

Verification: `nix develop --command node next/scripts/verify.mjs --agent` passed
(Agent connection, integration, Codex fixture and browser acceptance) with the
draft-first flow covered: no thread is created before submit, save-only and run
from a draft, project change from the headline, review collapse/wrap/apply. Desktop,
review, model menu and phone states were screenshot-reviewed in an isolated
fixture in both themes.

## 1. Frame, hierarchy and density

Reference: `components/AppSidebarLayout.tsx`, `components/ChatView.tsx`,
`components/chat/PanelLayoutControls.tsx`, `components/threadSidebarWidth.ts`.

| Aspect | T3 reference / parity target | Dar gap | Work |
|---|---|---|---|
| Overall frame | Compact application sidebar, thread title bar, conversation canvas | Shared global header plus permanently allocated review column | UI/Shell |
| Title bar | Project identity, thread title, compact surface controls | Title and large lifecycle commands; project context elsewhere | UI |
| Surface hierarchy | Thread details, terminal drawer and right panel are separately controlled | Changes sidebar conflates details and review | UI |
| Panel widths | Resizable workspace regions | Fixed grid widths | UI/State |
| Visibility | Independently open/close surfaces and maximize review | Two temporary booleans | UI/State |
| Spacing | Dense chrome; generous readable conversation column | Uniform pane padding and generic controls | UI |
| Typography | Distinct muted work log, body text, code and chrome | Most content rendered as plain paragraphs | UI |
| Empty states | No-project, no-thread, new draft and unavailable environment differ | Repeated generic welcome | UI |
| Theme | Consistent surfaces, selection, tooltips and focus treatment | Twilight contract must be reconciled with exact T3 chrome | Shell |
| Independent scrolling | Sidebar, timeline, details, diff and terminal scroll independently | Broad structure exists; restoration and resizing absent | UI |

## 2. Sidebar and lifecycle

Reference: `components/Sidebar.tsx`, `components/sidebar/SidebarThreadRow.tsx`,
`components/sidebar/SidebarThreadHeader.tsx`, `components/threadActionMenu.logic.ts`,
`components/ThreadHoverCard.tsx`, `components/ThreadStatusIndicators.tsx`.

| Aspect | T3 reference / parity target | Dar gap | Work |
|---|---|---|---|
| New thread | Draft/project selection flows into composing | Creates durable blank thread before composing | State/UI |
| Row hierarchy | Project identity above contextual title; status affordances | Generic agent icon, title, tiny project subtitle | UI |
| Working shelf | Live activity indicators and counts | Basic run-state text | UI |
| Settled shelf | Explicit settlement; restoring returns to active | Revision preference exists; row action missing | UI |
| Drafts | Distinct draft rows and discard behavior | Only per-thread session draft | State |
| Thread search | Search/filter/navigation through threads | Global search is not thread search | UI/State |
| Project filter | Filter from thread/project actions | No thread filter | UI |
| Optional grouping | Project-folder mode is an option, not assumed default | No mode preference | State/UI |
| Row actions | Hover/context actions without changing current selection | Only settle in heading | UI |
| Pinning | Pin/unpin independently of running/settled state | Absent | State |
| Snoozing | Presets, wake, timed return and notification | Absent | State/Runtime |
| Auto-settle | Per-thread policy and global lifecycle preference | Absent | State |
| Rename | Explicit rename plus generated-title regeneration | First-message truncation only | State/Runtime |
| Read markers | Mark unread; completion/read indication | Absent | State |
| Navigation affordances | Hover context, shortcuts and jump hints | Absent | UI |
| Copy | Thread reference/ID, project path, branch when applicable | Absent | UI |
| Archive | Archived threads remain discoverable/restorable | Absent | State |
| Delete | Permission-aware destructive lifecycle action | Absent | State |
| Undo | Lifecycle undo notice where supported | Absent | State/UI |
| Sharing | Guest grant and explicit share action | No Agent sharing contract | State |
| Branch continuation | New thread on current branch | No branch-aware thread creation | State/Runtime |

## 3. Projects and initial flow

Reference: `components/NoProjectsHero.tsx`, `components/NoActiveThreadState.tsx`,
`components/CommandPaletteAddProject.tsx`, `components/BranchToolbar*.tsx`,
`components/AddProjectToMachineDialog.tsx`, `routes/_chat.new.tsx`.

| Aspect | Parity target | Dar gap | Work |
|---|---|---|---|
| Project chooser | Search existing projects; identity and source context | Plain right-pane list | UI |
| Add project | Source-specific browsing/clone progress and recoverable failure | Files picker and repository URL only | UI/Runtime |
| Naming | Name follows source; contextual generated thread title | Source name works; no generated title job | Runtime |
| Local folder | Host folder is a first-class project source | Dar Files IDs intentionally replace host paths | Runtime |
| No-project work | Explicit standalone work flow where supported | Every thread requires a project | State/Runtime |
| Environment | Machine and execution environment visible | Only source type | State/Runtime |
| Branch/worktree | Choose branch, local/worktree mode and preparation status | One isolated working copy; no branch controls | State/Runtime |
| Open in editor | Editor picker and path-aware opening | Open folder in Files only | Shell |
| Project scripts | Edit/run scripts with terminal association | Absent | Runtime/State |

## 4. Composer

Reference: `components/chat/ChatComposer.tsx`, `ComposerSurface.tsx`,
`ComposerControlsBar.tsx`, `ComposerPrimaryActions.tsx`, `ComposerFooterControls.tsx`,
`ComposerModelEffort.tsx`, `ProviderModelPicker.tsx`, `ComposerCommandMenu.tsx`,
`components/ComposerPromptEditorTiptap.tsx` (all under `components/`).

| Aspect | Parity target | Dar gap | Work |
|---|---|---|---|
| Resting surface | Quiet docked composer, compact controls and circular send | Large action labels, persistent notice and hint | UI |
| Editor | Growing rich prompt editor with code blocks and plain-text fidelity | Resizable textarea | UI |
| Submission keys | Match configured send/newline behavior; respect composition | Ctrl/Cmd Enter only | UI/State |
| Model picker | Searchable model/provider picker with selected state | Flat model select | UI |
| Reasoning | Effort options bound to model capabilities | Model catalog discards effort metadata | State/Runtime |
| Speed/access | Capability-aware speed and permission controls | No such request fields; sandbox policy fixed | State/Runtime |
| Plan mode | Plan selection and implement-plan follow-up | Absent | Runtime/State |
| Attachments | Paste/drop/upload, preview, progress, remove and failure | Text only | State/Runtime |
| References | File/terminal/context chips with inspect/remove behavior | Plain text context only | State/Runtime |
| Mentions | File/context selection inside editor | Absent | UI/Runtime |
| Slash commands | Searchable command layer with keyboard selection | Absent | UI/Runtime |
| Prompt history | Restore previous prompts without losing current text | Absent | UI/State |
| Draft retention | Drafts, attachment state and recovery survive navigation | Text retained in this browser tab only | State |
| During a run | Steer/queue follow-up; alternate dispatch action | Sending blocked while running | State/Runtime |
| Queue editing | Edit/cancel queued prompt with retained context | Absent | State/Runtime |
| Stop | Stop beside send; clear stopping/error state | Stop in heading only | UI |
| Resume | Continue stopped execution when provider supports it | Every run is ephemeral | Runtime |
| Pending questions | Structured options, multiple questions and submission | No question/answer channel | State/Runtime |
| Pending approval | Actual request details and explicit resolution | No interactive approval channel | State/Runtime |
| Validation | Explain blocked send, preparation, limits and provider state | Mostly disabled button | UI |
| Compact controls | Overflow based on measured available space | Wrapping controls only | UI |
| Context meter | Real usage/window values and compaction state | No usage fields | State/Runtime |
| Save-only | Keep truthful distinction between a saved message and execution | Exists; relocation must preserve it | UI |

## 5. Conversation and activity

Reference: `components/chat/ThreadTimeline.tsx`, `MessagesTimeline.tsx`,
`TimelineUserMessageRow.tsx`, `TimelineAssistantRows.tsx`, `TimelineWorkGroupRows.tsx`,
`TimelineWorkEntryRow.tsx`, `WorkLog.tsx`, `useTimelineFollow.ts`,
`useTimelinePositionRestore.ts`, `components/ChatMarkdown.tsx`.

| Aspect | Parity target | Dar gap | Work |
|---|---|---|---|
| User message | Right-aligned bounded bubble, subdued metadata/actions | Full-width labelled paragraph | UI |
| Assistant message | Open document surface, rendered Markdown | Plain pre-wrapped text | UI |
| Markdown | Headings, lists, tables, code copy, safe links and file references | No conversation Markdown renderer | UI |
| Rich outputs | Images, attachments, citations, diagrams with safe boundaries | Plain text only | UI/State |
| Message actions | Copy, prompt link and edit-from-here | Absent | UI/State |
| Chronology | Messages, commentary, work and completion share ordered timeline | Saved messages plus latest-run block | State |
| Tool rows | Typed file/command/tool entries with expand and result state | `activity: string[]` | State/Runtime |
| Grouping | Collapsible work group and duration summary | One Tool activity disclosure | UI/State |
| Streaming | Incremental assistant and tool updates in their timeline positions | Poll latest flattened output each second | State/Runtime |
| Turn boundaries | Completed, failed, stopped and interrupted turns remain inspectable | Historical run documents not presented as timeline | State/UI |
| Scroll following | Follow when at end; scrolling up stops automatic following | No explicit follow model | UI |
| Jump to latest | Appears when reading older content | Absent | UI |
| Position restore | Remember per-thread viewport and anchor across switches | Conversation remount loses position | UI |
| Earlier history | Prepending preserves visible anchor | Fetch exists; no explicit anchor preservation | UI |
| Long sessions | Bounded/virtualized rows and stable streaming layout | Renders all loaded paragraphs | UI |
| Find/minimap | In-thread navigation and timeline overview | Absent | UI |
| Recovery banners | Distinct provider, limit, environment and execution recovery | Generic alert/refresh | State/UI |
| Background completion | Notifications/read-state without stealing focus | Poll sidebar state only | State/Shell |

## 6. Details, review, source control and output

Reference: `components/chat/threadDetailsItems.ts`, `ThreadDetailsControl.tsx`,
`RightPanelSurfaceContent.tsx`, `components/DiffPanel.tsx`,
`components/diffs/*`, `components/ThreadTerminalDrawer.tsx`.

| Aspect | Parity target | Dar gap | Work |
|---|---|---|---|
| Details card | Environment, editor, scripts, branch/PR, Git actions, changes | Project-source block inside Changes | UI/Runtime |
| Optional details | Servers, automations, lineage, side chats, usage/context | Absent | State/Runtime |
| Details visibility | Select individual sections; restore defaults | Absent | State/UI |
| Review opening | Changes opens a proper review surface | Always allocated narrow sidebar | UI |
| Review scope | Turn/working-tree/base comparison and refresh | Latest run changes only | State/Runtime |
| File navigation | Tree, status, counts, reveal and path copy | Flat list with kind | UI/State |
| Diff rendering | Line numbers, add/remove gutters, syntax and hunk structure | One preformatted string | UI |
| Review layout | Unified/stacked or side-by-side; wrap toggle | No controls | UI |
| File expansion | Collapse individual/all files and preserve state | One selected file only | UI |
| Large/binary diffs | Explicit loading, limits and unsupported content | Basic truncated flag | State/UI |
| Review comments | Anchored line annotations, submit into agent context | Absent | State/Runtime |
| Files write-back | Apply actual reviewed content with revision conflict recovery | Existing behavior must survive redesign | UI |
| Git operations | Branch comparison, commits and publishing/PR flow | Publishing explicitly disabled | State/Runtime |
| PR review | Linked PRs, review/activity and comparison surfaces | Absent | State/Runtime |
| Terminal drawer | Resizable interactive terminal, tabs, splits and close rules | Read-only command-output disclosure | Runtime/State |
| Command output | Per-command status and output linked to work entry | One concatenated output string | State |
| Terminal context | Select output and attach as composer context | Absent | UI/State |
| Right surfaces | Files, review and other surfaces selected explicitly | One Changes panel | UI/State |
| Preview/browser | Browser surface and controls bound to thread | Absent | Runtime/Shell |

## 7. Settings and secondary surfaces

Source inventory: `routes/_chat.settings.*.tsx`; source inspection identifies
these routes, not live verification of every setting within them:

- General, Appearance, Chat, Threads, Keybindings, Diffs, Browser.
- Projects, Worktrees, Source control, Archived threads.
- Providers, Connections, Devices, Integrations, Access, Secrets.
- Scheduled tasks, Storage, Maintenance, Diagnostics, Labs, Beta.
- Snap Shot, About, Open-source licenses.
- Separate usage and pull-request routes; command-palette project/thread actions.

Dar currently has General (connection) and Keyboard shortcuts. It needs a
setting-by-setting follow-up ledger before claiming settings parity. Reuse Dar's
account/settings scope for persisted preferences; do not put inactive switches in
the product. Service/device, scheduling, secret and provider management each need
their actual backing capabilities. Desktop updater and OS title-bar integration
are platform-specific and cannot be promised by a browser layout change.

## 8. Cross-cutting acceptance

- Check pointer, keyboard, focus-visible, Escape/outside dismissal and focus return
  for each menu, popover, panel and dialog. Preserve IME composition and selection.
- Match hover/action visibility without making actions inaccessible on touch.
- Verify desktop, narrow desktop and phone; mobile panels retain draft, selected
  thread/file, scroll and explicit back behavior. Check reduced motion and themes.
- Persist lifecycle, model/control choices and layout settings at the intended
  account/thread/device scope; verify after reload and reconnect.
- Handle stale revisions, retry, uncertain sends, disconnect/restart, truncation,
  provider absence and storage failure without duplicate turns or lost text.
- Use isolated fixtures for real running, stopping, failed and completed turns.
  Verify changed content and conflict handling, not screenshots alone.
- Never depict fake tools, terminal interactivity, approval controls, usage or
  publishing as working. Static examples must stay labelled as mock data.

## Implementation order after selection

1. Match chosen frame, sidebar row hierarchy, message layout, composer and details
   placement. Add safe Markdown, real existing-data work presentation, Stop at
   composer, local find/copy and scroll restoration. Preserve all current writes.
2. Define minimal typed, ordered run events and historic-run access; preserve
   replay, event IDs, status, timing and bounded output. Existing flattened data
   needs an explicit compatibility story. Render true work groups and commands.
3. Add model capabilities/effort, durable lifecycle and composer preferences,
   attachments/context and diff navigation with focused persistence checks.
4. Address interactive execution, branches/worktrees, source control, terminal,
   scheduling, sharing and other providers as capability-backed vertical slices.
   Preserve direct Codex CLI execution unless the maintainer explicitly revises
   that standing requirement. An interactive app-server is not implied approval.
5. Re-run this ledger against implemented behavior and list unresolved rows.

## Selected direction

See [static parity mocks](agent-t3-parity-mocks.html). The maintainer selected A:
its compact chrome and contextual details replace the Agent
page's global header. B retains Dar's shared header while copying the core T3
workspace; C gives review a permanent wide split. These are deliberately distinct
static proposals. All content is synthetic, and shown controls imply no new
implemented capabilities. This selection authorizes the compact Agent UI exception
to the shared-header contract; the other apps retain that contract.
