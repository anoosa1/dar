# Codex in Dar

The Agent runs the Codex CLI's `codex app-server` with an existing ChatGPT login
on the server. There is no API key flow. The login stays in the configured Codex
home and is bound to exactly one immutable Dar account ID (shown in Agent
settings), so renaming a user cannot transfer it. Tested with CLI 0.163.0 and
0.166.0 (live model runs on 0.166.0); other versions need re-verification, and
unsupported behavior fails closed.

## Connect

**NixOS:**

```nix
services.dar.agent = {
  enable = true;
  package = pkgs.codex;           # pinned; nothing is downloaded at runtime
  home = "/var/lib/dar-codex";
  userId = "00000000-0000-0000-0000-000000000000";  # your ID from Agent settings
  gitHosts = [ "github.com" ];
};
```

Sign in once as the `dar` service user with that `CODEX_HOME`, then use Refresh
connection in Agent settings. Installing Codex in your own shell does not give
the service your login or put it on systemd's PATH.

**Other hosts:** an administrator can use **Connect server Codex**, which looks
for `codex` on the service PATH, system paths and the service user's
`.nix-profile/bin` and `.local/bin`, checks the login in `CODEX_HOME` (or
`~/.codex`) and binds it to their account. It never replaces a binding or copies
credentials. Or write `codex.json` in the Dar root (or `DAR_CODEX_FILE`):

```json
{"executable": "/abs/path/codex", "home": "/home/you/.codex",
 "userId": "00000000-0000-0000-0000-000000000000", "gitHosts": ["github.com"]}
```

It holds paths and host names only, never tokens. Refresh connection re-reads it,
runs `codex login status` and reads models from `codex debug models --bundled`,
reporting missing binding, another account's binding, missing login and CLI or
catalog failures separately. Running turns keep their starting configuration.

## How runs work

Each run starts one `codex app-server` in the thread's isolated working copy and
speaks JSON-RPC on stdio. The Codex thread ID and the last message it saw are
stored, so later runs resume and send only new messages (a failed resume starts
over with the bounded transcript). Notifications become typed steps, live output
and the context meter. Success needs a completed-turn notification.

- **Steering** sends a follow-up into the running turn; **questions** from Codex
  show an answer card; **plan mode** and **reasoning effort** come from the CLI
  catalog.
- **Images:** PNG/JPEG/GIF/WebP, 8 MiB each, four per turn, stored per thread
  outside the working copy.
- **Approvals are always declined** (`approval_policy = never`): granting one would
  run commands as the service user with access to every account.
- **Terminal:** a PTY with the same profile (no network, no Codex home, writes
  only in the workspace), closed after 15 idle minutes or on deletion/shutdown.
- **Review and compaction** use `review/start` and `thread/compact/start`; usage
  limits come from `account/rateLimits/read`.
- **Git:** Dar runs status, new branch and commit itself (`.git` is read-only to
  Codex). Push and pull requests are disabled.

Dar pins every policy it relies on with `-c` flags (permission profile, network,
MCP servers, approvals, notifications, hooks, plugins, apps) and refuses to run if
the Codex home has a `rules` directory with files. Keep that home for Dar only.

## systemd sandbox notes

Bubblewrap needs `AF_NETLINK` in `RestrictAddressFamilies` and
`ProtectKernelTunables=false` on Agent-enabled units (otherwise loopback setup or
mounting `/proc` fails). The module sets both only when Agent is enabled;
`ProtectSystem=strict`, `ProtectHome`, private devices and Codex's `/proc`/`/sys`
denies remain. Custom units need the same. Check a CLI without a login or model:

```sh
nix develop -c node next/test/codex-sandbox-systemd.mjs /absolute/path/to/codex
```

Enabling Agent also sets service-wide memory and task limits. Extra project tools
can be added through `systemd.services.dar.path`; commands have no network.

## Limits

- Git sources: public HTTPS repositories on configured hosts. No private
  credentials, SSH, submodules, redirects or file transports.
- Files sources copy authorized versions; reviewed edits to existing UTF-8 text
  files up to 64 KiB are applied as new Files versions with revision checks. New
  files and deletions stay in the working copy. Apply is per file, not atomic.
- Workspaces: 200 files, 1 MiB per file, 8 MiB total; 10 minutes per run;
  50 messages and 128 KiB of context per thread. One active run per account.
- Backups hold Agent history and bounded review data, not credentials or working
  copies; apply or export changes before restoring elsewhere.
- No parallel agents, remote runners, scheduled runs or automatic publishing.

Tests use an isolated CLI fixture: `nix develop -c node next/scripts/verify.mjs --agent`.
CLI references: [non-interactive use](https://developers.openai.com/codex/noninteractive)
and [configuration](https://developers.openai.com/codex/config-reference).
