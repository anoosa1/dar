# Codex in Dar

Dar Agent runs the CLI's `codex app-server` with its existing ChatGPT login.
There is no API client or API-key flow.
The login stays in Codex's configured home on the server and is bound to exactly
one immutable Dar account ID. A desktop user's login is not copied into a different Linux
service account. Use an existing login readable by the account running Dar.

## Development and other hosts

The Linux sandbox boundary has been checked with Codex CLI 0.163.0 and 0.166.0;
live model execution was checked with 0.166.0. Other versions need CLI flag,
JSONL event and isolation verification before deployment; unsupported behavior fails closed.
In Agent settings, an administrator can use **Connect server Codex** to discover
`codex` on the Dar service's PATH, standard system paths (including NixOS's
`/run/current-system/sw/bin`), or the service user's `.nix-profile/bin` and
`.local/bin`. It checks the existing login in `CODEX_HOME` (or
`~/.codex` for that Linux account). The action checks the login and creates a
private binding to the current immutable Dar account ID. It never replaces an
existing binding or copies credentials. Discovery initially permits public Git
repositories on github.com. For other hosts or explicitly managed deployments,
create `codex.json` in the private Dar root, or set `DAR_CODEX_FILE` to a regular
JSON configuration file:

```json
{
  "executable": "/absolute/path/to/codex",
  "home": "/home/you/.codex",
  "userId": "00000000-0000-0000-0000-000000000000",
  "gitHosts": ["github.com"]
}
```

Replace the example user ID with your account ID, shown in Agent settings. Binding
uses the immutable ID, so renaming a user cannot transfer access to another account.
These are configuration paths and host names, not credentials. Never put tokens,
passwords or credential-bearing repository URLs in this file. Keep the actual
Codex home private. Configuration changes take effect on the next check. In Agent settings,
Refresh connection runs `codex login status` and reads the installed CLI's bundled
model catalog through `codex debug models --bundled`. Model access is only
proven when a turn succeeds. If login has expired, use the normal Codex login on the
server as the account running Dar, then refresh the connection. Each refresh
re-reads configuration and checks the CLI immediately. It distinguishes a missing
binding, another account's binding, missing login, CLI failure and catalog failure.
Active runs retain the configuration with which they started.

Use New thread to choose a project or add a Git repository/Dar Files folder.
The composer opens immediately; the first message supplies its initial title.
Working threads have their own shelf. Settle thread parks idle conversations;
Restore thread or new conversation activity brings them back. Settlement is
saved in account preferences with revision checks and included in backups.
Select a model
and use Run with Codex. Save only retains a message without invoking the model.
Running sends that conversation and selected project contents to Codex. Stop ends
the owned process tree; reconnecting shows the saved run status and output.

Each run starts one `codex app-server` process in the thread's isolated working
directory and speaks newline-delimited JSON-RPC on stdio (`initialize` with the
experimental API, `thread/resume` or `thread/start`, `turn/start`). The Codex
thread ID and the last Dar message revision it has seen are stored on the run, so a
later run resumes that thread and sends only unseen messages; if resume fails, a new
thread receives the whole bounded transcript. Item, delta, plan and token-usage
notifications become typed steps, live output and the context meter.

- Steering: `turn/steer` delivers a follow-up into the running turn; Dar saves it
  as a user message and records its ID on the run. Retries are idempotent.
- Questions: `item/tool/requestUserInput` is stored as the run's pending question
  and answered by its owner through the run's answer route.
- Plan mode: `collaborationMode` `plan`. Reasoning: `effort` from the CLI catalog.
- Images: uploaded per thread (PNG/JPEG/GIF/WebP, 8 MiB, four per turn) into the
  thread's private store outside the working copy, passed as `localImage`.
- Approvals: every approval request (sandbox escapes, extra writes, network) is
  declined and shown as a declined step. `approval_policy` is `never`. Granting them
  would run commands as the Dar service user with access to all accounts.
- Terminal: a PTY through `command/exec` with the same permission profile (no
  network, no Codex home, workspace writes only), read by offset with long polling,
  closed after 15 idle minutes, on thread deletion and on shutdown.
- Git: status, new branch and commit are run by Dar in the working copy (the sandbox
  keeps `.git` read-only). Push and pull requests are disabled.

app-server cannot ignore user configuration the way `exec` did. Dar therefore pins
every policy setting it relies on with `-c` (permission profile, network, MCP
servers, approvals, `notify=[]`, disabled hooks/plugins/apps) and refuses to run while
the Dar Codex home contains a `rules` directory with files. Keep that home dedicated to
Dar. Success requires a completed-turn notification; a process exit before it fails
the run, and failed turns never create a successful assistant reply.

## NixOS

```nix
services.dar.agent = {
  enable = true;
  package = pkgs.codex; # Supply a compatible pinned package; no runtime download.
  home = "/var/lib/dar-codex";
  userId = "00000000-0000-0000-0000-000000000000"; # Your ID from Agent settings.
  gitHosts = [ "github.com" ];
};
```

The module creates a private Codex home for the `dar` service user. Reuse that
identity's existing Codex login, or sign it in through Codex's normal server login
flow once. The generated Nix configuration contains only paths and host metadata;
login credentials never enter Nix expressions or the store. The service still
protects home directories and the wider filesystem. Its configured Codex home is
writable for Codex token refresh. Named Linux permission profiles need working
unprivileged sandbox support; the executor checks it before every model run.

Agent-enabled units allow `AF_NETLINK` in `RestrictAddressFamilies`: bubblewrap
needs a NETLINK_ROUTE socket to configure loopback in its private network namespace.
Without it, the login succeeds but the isolation probe fails before inference.
Agent-enabled units also set `ProtectKernelTunables=false`: the locked `/proc`
submounts it creates otherwise prevent bubblewrap from mounting procfs in its
user namespace (`Can't mount proc on /proc: Operation not permitted`).
This conflict was reproduced on 0.166.0; 0.163.0 passed with protected procfs.
`ProtectSystem=strict`, `ProtectHome`, private devices and Codex's explicit
`/proc` and `/sys` denies remain enabled. Units without Agent retain
`ProtectKernelTunables=true`. Update/rebuild the module and restart Dar if an older unit has
only `AF_UNIX AF_INET AF_INET6`. Custom units need the same additional family.
An optional real boundary check uses a temporary workspace, no login or model call:

```sh
nix develop -c node next/test/codex-sandbox-systemd.mjs /absolute/path/to/codex
```

It reproduces the socket-family and missing metadata-directory failures, checks
the installed CLI's protected-proc compatibility and verifies the corrected policy under all Dar
service hardening settings, including PrivateTmp and ProtectSystem. It uses a
user transient unit rather than accessing a live Dar instance or its data.

Cached threads created by older Dar versions may also lack the `.codex` deny
directory. Both tested CLI versions fail to mask this missing directory. Dar now
ensures it exists on every workspace preparation, including reuse, without
replacing source files or the baseline. Starting a new thread also provisions a
fresh complete workspace. Known failures have distinct, sanitized messages;
arbitrary CLI diagnostics and credentials are not sent to the browser.

Installing Codex in an SSH user's shell does not add it to systemd's PATH or
share that user's login. The module adds `agent.package` to Dar's service PATH
and writes an absolute executable path into its configuration. If discovery
still fails, use this explicit NixOS setup, or an absolute `executable` in
`codex.json` on other hosts. Do not disable `ProtectHome` or copy a personal
credential file to make discovery work. Sign in as the Linux service account
using the configured `CODEX_HOME`; then use Refresh connection.

The packaged runtime provides Git and Node. Additional project test tools can be
provided through `systemd.services.dar.path`. Commands cannot access the network;
prepare dependencies through a separate trusted workflow. Enabling Agent applies
service-wide memory/task limits, which can be overridden in the NixOS unit when
sizing a larger installation.

## Current scope

- Git provisioning supports public HTTPS repositories on exact configured hosts.
  Private Git credential provisioning, SSH, submodules and push/PR/deployment
  operations are not wired up. Git clone redirects and file transports are denied.
- Files sources preserve immutable blobs. Reviewed modifications to existing UTF-8
  text files up to 64 KiB use existing conditional upload/version operations.
  Concurrent writes produce a conflict. New files and deletions stay in the working
  copy and review output. Applying one file is not an atomic multi-file transaction.
- Each thread retains its working copy. Start a fresh thread when a Files source
  changes. Do not edit working copies through host paths while a run is active.
- Workspace size, run time, output and history are bounded. This is not a general
  host terminal, dependency installer or per-run container/cgroup service.
- Backups contain Agent history and bounded review data, not Codex credentials or
  working-copy caches. Apply/export work before restoring elsewhere. Large or
  truncated unapplied changes cannot be recovered fully from the backup alone.
- Approvals outside workspace policy are declined. There are no parallel agents,
  remote runners, scheduled runs or automatic publishing.

See the [delivery contract](spacetimedb/agent-workspace.md) for limits and acceptance.
Run focused acceptance with `nix develop -c node next/scripts/verify.mjs --agent`;
the standard `npm --prefix next run verify` also runs these checks. Automated
provider-flow tests use an isolated CLI fixture, not a live login. A separate
direct CLI check used the existing login with a temporary README/result file and verified
that a sibling fixture file was outside the sandbox; no live Dar data was used.

## CLI sources

The [official non-interactive CLI documentation](https://developers.openai.com/codex/noninteractive)
describes saved CLI authentication and JSONL event output.
The [configuration reference](https://developers.openai.com/codex/config-reference)
documents named permissions and disabling optional tools. Dar verifies its actual
installed CLI flags, event output and filesystem behavior instead of assuming current web
examples apply to every CLI version.
