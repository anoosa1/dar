# Codex in Dar

Dar Agent invokes `codex exec --json` directly with the CLI's existing ChatGPT
login. There is no app-server connection, API client or API-key flow.
The login stays in Codex's configured home on the server and is bound to exactly
one immutable Dar account ID. A desktop user's login is not copied into a different Linux
service account. Use an existing login readable by the account running Dar.

## Development and other hosts

Use Codex CLI 0.166.0 with named permission profiles. Newer versions need CLI flag,
JSONL event and isolation verification before deployment; unsupported behavior fails closed.
Create `codex.json` in the private Dar root, or set `DAR_CODEX_FILE` to a regular
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
Codex home private. Restart Dar after changing the configuration. In Agent settings,
Refresh connection runs `codex login status` and reads the installed CLI's bundled
model catalog through `codex debug models --bundled`. Model access is only
proven when a turn succeeds. If login has expired, use the normal Codex login on the
server as the account running Dar, then refresh the connection. Status is cached
for up to 30 seconds.

Choose a Git repository or a folder in Dar Files, create a thread, select a model
and use Run with Codex. Save only retains a message without invoking the model.
Running sends that conversation and selected project contents to Codex. Stop ends
the owned process tree; reconnecting shows the saved run status and output.

Each run starts `codex exec --json --ephemeral` with its chosen model and isolated
working directory. Dar sends conversation context through stdin and consumes the
CLI's JSONL item/turn events from stdout. Normal CLI authentication and token refresh
use the configured Codex home. `--ignore-user-config` and `--ignore-rules` keep
execution policy under Dar's control while preserving that saved authentication.
Success requires both a successful process exit and a completed-turn event;
premature exits and failed turns never create a successful assistant reply.

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
