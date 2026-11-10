# Agent workspace

Status: layout approved November 8, 2026; persistent workspace and initial Codex
execution implemented. See explicit limits below and [setup](../codex.md).

## Maintainer decisions

- Support coding repositories and Dar data, starting with coding.
- Copy the T3 Code workspace composition, using Dar's existing shared controls
  and Twilight themes. Do not explore alternative layout directions.
- Projects/threads on the left, conversation/composer in the center, file changes
  and diffs on the right, collapsible terminal/output below the conversation.
- The November 9 [parity option A](agent-t3-parity-mocks.html) supersedes the
  original shared-header requirement for Agent: compact sidebar app/account
  access and thread search replace the global header here. Other apps retain it.
- Review one faithful static mock before implementing the workspace.
- The maintainer approved that mock, then required both Git repository and Dar
  Files folder projects. Neither source is a host filesystem path.
- Codex with an existing login is the first selected model connection.
- The maintainer requested direct Codex CLI execution. Invoke `codex exec --json`
  with its saved login instead of using the app-server RPC interface.

The [workspace mock](agent-workspace-mock.html) uses illustrative data only.
It is a structural interpretation of the requested layout, not a verified
pixel-for-pixel reproduction of T3 Code. Theme and phone view controls work;
model calls, commands, changes and run states are sample content.

## First release

An optional Agent app opens an allowed Git repository or an authorized Dar Files
folder. A user creates a thread, sends a request and watches the agent read files, apply patches and run
tests in a private workspace. Changes remain available for review and follow-up.
Reloading the browser restores the conversation and current run state.

Start with Codex using the server service user's existing ChatGPT login. Bind that
login to one immutable Dar account ID; never share it with every instance account.
No OpenAI API key or new provider SDK is required. The runtime uses the configured
Codex executable and the packaged Git executable.
Parallel agents, remote runners, scheduling, push/PR/deployment tools and general
interactive terminal access are deferred. The first terminal panel shows bounded
command output; it is not an unrestricted host shell.

## Implementation slices

1. **Layout review (approved).** Review the linked mock in both themes and at desktop/phone
   widths. On phones, open Projects, Conversation, Changes or Terminal as a single
   focused view. Implementation must restore focus and preserve composer drafts
   when switching. Select/revise the mock before building the product UI.
2. **Persistent thread slice (implemented).** Add the optional app's availability, navigation,
   account-scoped projects, threads and messages. Use existing auth and shared
   components. Explicitly review the narrow core schema/API extension: the current
   installed-app SDK cannot add native tables and its 1 MiB/account/app data quota
   is not suitable for agent histories. Do not raise all installed-app quotas.
3. **Provider/run slice (implemented for Codex).** Integrate one provider behind a small typed adapter.
   Persist bounded run/tool snapshots and update the browser after reconnect.
   Full ordered event replay remains future work. Define queued, running,
   cancelling, completed, failed,
   cancelled and interrupted transitions. A browser disconnect does not cancel
   work. A crash does not trigger automatic replay of uncertain commands.
4. **Isolated coding slice (initial implementation).** Add a separately owned runner, explicit project
   provisioning, workspace-only file tools, patch edits, Git status/diff and
   command execution. Give each thread an isolated checkout. Never expose Dar's
   private root or database credentials to it. Broker model calls outside the
   command sandbox. Fail closed if configured isolation cannot be established.
5. **Review and lifecycle (partly implemented).** Wire real diffs, output, cancellation, retry and
   policy-bound approvals. Add history/artifact retention, resource/usage limits,
   backup/restore and Nix packaging. Disabling the app cancels owned runs and
   disposes runners while retaining history.
6. **Further Dar tools.** Add selected attachments and authorized read tools first, then
   revision-safe writes to Files, Notes and Tasks through existing domain
   operations. Calendar follows. Do not expose blob directories or database
   service credentials through tools.

## Implemented workspace slice

Agent is an optional built-in app at `/agent`, integrated with the launcher,
Administration app availability and cross-app search of project/thread names.
It uses private native project/thread/message tables and `/api/v1/agent` APIs;
the installed-app SDK contract and quotas remain unchanged. New projects store
`sourceType` (`git` or `files`) and `sourceRef` (credential-free repository URL or
Files folder ID). Folder browsing and project creation enforce current access
within the database transaction. This records a source; it does not clone,
materialize, send source contents to a provider or give the runner authority.

Thread message appends require the expected thread revision and a client-generated
operation ID. Exact retries return the saved message; altered retries conflict.
History pages contain at most 50 messages with stable sequence cursors. Limits per
account are 32 projects, 128 threads and 1,000 messages of at most 16 KiB each.
No assistant messages or tool results are fabricated. Without a configured Codex
connection, the composer saves messages without sending them to a model. Connected
accounts can choose a model and run, or explicitly save a message without running.

Browser drafts are scoped to account/thread in session storage, with an in-memory
fallback for thread switching and an explicit error when recovery storage fails.
An uncertain append retains its operation ID in session storage for safe retries
after reload. Backup v6 includes Agent data and validates ownership, links, message
sequences and quotas during restore; v1–v5 backups remain readable. Account deletion
removes Agent data. Disabling Agent rejects native operations and removes its
projects/threads from search while retaining their records.

For a Dar Files run, materialize a bounded manifest of accessible file IDs,
version IDs and revisions through the authenticated Files service. Immutable blobs
remain authoritative for bytes; folders remain database entries. Treat the materialized
tree as a working copy. Review proposed edits and use existing conditional Files
writes to create new immutable versions; never update blobs in place. Recheck
grants, detect concurrent changes and report partial/conflicted multi-file writes
honestly. A Git project instead pins its fetched commit and retains an isolated
checkout and diff; pushing is a separately authorized action.

## Codex execution

Dar starts `codex exec --json --ephemeral` directly for each run, passing the
chosen model and working directory as arguments and conversation context on stdin.
The CLI owns its existing ChatGPT login and token refresh; Dar neither extracts
tokens nor sends them to the browser. Configuration binds that login to one Dar
account UUID, which is shown in Agent settings and survives username changes.
Other users can retain private projects/messages but cannot spend that
login's model access. Login status comes from `codex login status`; model choices
come from `codex debug models --bundled`, not hard-coded identifiers. CLI 0.166.0
is tested. No app-server process or RPC transport is used.

Each thread has a private working copy and a baseline outside the model's readable
roots. Files source permissions/revisions are checked before copying bytes through
the existing Files service. Public HTTPS Git sources need an administrator-enabled
host; redirects, file transports and inherited Git credentials/hooks are disabled.
Symlinks, special files and submodules are rejected. Repository `.codex` metadata
is excluded from execution authority. Files working copies receive a local Git
baseline; Git working copies retain their initial shallow checkout.

Codex uses a named permission profile: only its workspace is writable, Git metadata
is read-only, and Codex credentials, repository Codex config, `/proc` and `/sys` are
denied. Linux platform essentials and the public Nix store remain readable. Tool
network access is disabled. Apps, plugins, MCP servers, hooks, multi-agent execution
and shell snapshots are disabled. A real sandbox probe must pass before inference.
User configuration and execpolicy rules are excluded with the CLI's corresponding
ignore flags; saved CLI authentication is still reused. Approval policy is `never`.
Unsupported policy/event behavior fails closed; outside-policy actions are denied.
an approval UI and broader capability grants are not implemented yet.

Runs are queued transactionally with their user message, then claimed by an opaque
lease. One active run per account bounds process concurrency. Checkpoints recheck
the originating session, app availability and Files source-folder access every
500 ms, including provisioning.
They persist bounded output snapshots only when changed. The browser polls durable
state and can disconnect without cancelling. Stop and logout terminate owned process
groups; suspension/revocation/app disabling stop execution through these checks.
Startup and restore never repeat old commands: uncertain active runs are interrupted.
Successful completion appends the actual assistant response with a new thread
revision. Stale leases cannot publish updates or completion.
The CLI's `item.started`, `item.updated` and `item.completed` events update activity,
command output and assistant text. Cumulative updates replace previous text for
that item. Completion requires both `turn.completed` and exit code zero. Failed
turns and zero exits without a completion event are saved as failures, not replies.

The initial executor permits 200 files, 1 MiB/file and 8 MiB of materialized source,
500 inspected source entries, 100 retained runs/account and ten minutes/run. Logs
and response output are bounded, as are stored review diffs. Workspace growth is
checked periodically; Git provisioning has a 60-second timeout and periodic size
guard. These guards are not filesystem disk quotas. NixOS enables a 2 GiB service
memory cap and 256-task cap when execution is enabled, not per-run cgroups.

The complete conversation must fit 50 messages and 128 KiB of input for a run;
otherwise start a new thread rather than silently dropping context. Codex threads
are ephemeral; Dar's own messages and the working copy provide follow-up context.
Files source changes after materialization require a fresh thread. Existing text
files up to 64 KiB may be applied individually through revision-safe Files uploads.
New files/deletions and Git commits/pushes require later review/publishing support.

Backups retain messages, run state and bounded review content, without run leases,
session IDs or Codex credentials. Working copies are caches outside the backup
format. Download review content or apply changes before relying on backup/restore;
large/truncated/unapplied working-copy changes are not fully recoverable from it.
Dedicated immutable artifact retention, workspace-aware backup and cleanup remain
delivery work. No automatic workspace deletion or run-history purge is shipped.

## Boundaries and consistency

- Derive account identity from the current session. Recheck access for tool
  dispatch, stream/replay and artifact download. Define logout, suspension and
  grant-revocation cancellation behavior; runner authority must expire too.
- Durable runs use a claim/lease and a fencing token so a stale executor cannot
  publish results after replacement. Serialize runs within each thread.
- Keep short metadata/events in private, indexed SpacetimeDB tables; store large
  logs and artifacts in immutable blobs with bounded sizes and explicit retention.
  Do not persist every streamed token as its own database transaction.
- Tool schemas and persisted payloads need runtime validation, not just TS types.
  Record pending tool intent before dispatch. Unknown external outcomes remain
  interrupted; never promise exactly-once commands.
- Read repository instructions, including AGENTS.md. Repository/document text is
  untrusted context and cannot grant tools additional authority.
- Workspace edit/test permission can cover a run. Publishing, destructive work
  and access outside that scope require explicit authorization. Bind approvals
  to the immutable action payload, account, thread and run, with expiry.
- Store provider credentials in private runtime files/systemd credentials, outside
  Git and the Nix store. Never send them to the browser, model context or runner.
  Prevent command output from becoming an unbounded or unsanitized HTML surface.
- Network policy, mounts, process/resource limits and process-tree cancellation
  must be enforced by the runner's OS boundary, not prompt instructions.
- Use Cordis ownership for streams, timers, adapters and runner connections.
  Define ordered shutdown and cancellation before releasing runtime resources.
- Explain which selected content leaves Dar for model processing. Read access to
  a document does not imply permission to send the whole account to a provider.

## Focused acceptance

- Isolated repository: request a small edit, inspect the persisted diff, run a
  meaningful test, send a follow-up, reload and recover the same thread.
- Two accounts cannot read each other's threads, events, workspaces or artifacts.
- Test actual filesystem/process isolation, denied paths and configured network
  restrictions. A separate Node process alone is not a sandbox.
- Stop kills active subprocesses and prevents new tool dispatch. Crash/restart
  records interruption without duplicating a possibly completed command.
- Approval cannot authorize a different payload or cross account/run boundaries.
- Reconnect restores ordered events without duplicate messages; history is paged
  and logs are bounded. App disable and session revocation obey defined policy.
- Real-browser checks cover both themes, desktop and phone, independent pane
  scrolling, keyboard/focus behavior, drafts, failure/retry and resulting data.
- Upgrade/backup/restore retains conversations and artifact references. Restore
  does not resume old commands or restore runner credentials as ordinary data.

## Implementation verification

Verified November 8 with `nix develop -c npm --prefix next run verify`, followed
by a focused `nix develop -c node next/scripts/verify.mjs --agent` pass after the
final account-ID, source-access and review refinements. Direct CLI replacement was
then verified with the focused Agent acceptance command and a live CLI fixture.
Both TypeScript projects,
native database/HTTP checks and browser flows passed. NixOS module evaluation
passed with execution disabled and with an explicitly configured Codex package,
account ID, private home and resource limits. No production instance was deployed.

The Agent checks cover source ownership, CSRF, stale revisions, append replay,
pagination, app disable and v5/v6 backup compatibility. An isolated Codex CLI
fixture exercises queued/completed/cancelled/interrupted runs, source-folder access
loss, stale workers, account isolation, model selection, failed/premature CLI exits,
actual Files version writes
after review, drafts/save-only behavior, reconnect and desktop/phone themes.
The full suite also proves saved Agent messages survive forced runtime/database
failure alongside the existing Files/PIM/identity workflows.

A separate actual Codex 0.166.0 check used the existing ChatGPT login and its
bundled default model in a temporary fixture. Direct `codex exec --json` read a README and created the
requested result file. A direct filesystem probe confirmed a sibling fixture file
was unreadable outside the named workspace profile. No live Dar data was used.
This proves the local login/CLI/sandbox integration, not production NixOS
execution or private Git authentication. Provider-flow acceptance uses a CLI
fixture and does not spend live model usage on every verification run.

## Mock verification

Checked November 8 with Chromium from `nix develop` and the existing Playwright
package, loading the local HTML without a server or live Dar data. Both themes
passed at 1440, 728, 390 and 320 CSS pixels: no page-level horizontal overflow,
bounded center/terminal panes, working phone view selection and terminal collapse,
no external requests and no browser errors. Desktop and phone screenshots were
visually inspected; screenshots and the check report remain outside the repository.
T3's HTML preview browser lacked host libraries, so the existing Nix Chromium was
used for verification; the HTML is also presented inline for maintainer review.

The static mock does not satisfy runtime acceptance. No product controls should
ship backed by its sample records.
