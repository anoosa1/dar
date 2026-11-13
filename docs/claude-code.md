# Claude Code in Dar

The Agent can run Claude Code alongside (or instead of) [Codex](codex.md). Dar
drives the official `claude` CLI with its existing Claude login on the server and
speaks the same stream-json control protocol the Claude Agent SDK uses, so runs
count as ordinary Claude Code usage on that subscription. There is no API key
flow. The login stays in the configured Claude home (`CLAUDE_CONFIG_DIR`) and is
bound to one immutable Dar account ID. Tested with CLI 2.1.322.

## Connect

**NixOS:**

```nix
services.dar.agent = {
  enable = true;
  claude.package = pkgs.claude-code;   # pinned; nothing is downloaded at runtime
  claude.home = "/var/lib/dar-claude";
  userId = "00000000-0000-0000-0000-000000000000";  # your ID from Agent settings
  gitHosts = [ "github.com" ];
};
```

`package` (Codex) may be left null. Sign in once as the `dar` service user with
`CLAUDE_CONFIG_DIR=/var/lib/dar-claude claude`, then use Refresh connection.
`claude.commands` (default on) puts bubblewrap and socat on the service PATH.

**Other hosts:** an administrator can use **Connect server Claude Code**, which
finds `claude` like Codex discovery does, checks `claude auth status` in
`CLAUDE_CONFIG_DIR` (or `~/.claude`) and binds it. Or write `claude.json` in the
Dar root (or `DAR_CLAUDE_FILE`) with the same keys as `codex.json`. Only
first-party Claude logins are accepted; the account email is never shown.

## How runs work

Model IDs are `claude/<alias>` from the CLI's own catalog (with its effort
levels). Each run starts `claude -p` in the thread's working copy with:

- `--restricted`: file tools confined to the working copy; user and project
  settings, hooks and plugins ignored. `--strict-mcp-config` and
  `--disable-slash-commands`: no MCP servers or skills. Tools are limited to
  Read, Edit, Write, Glob, Grep, TodoWrite, AskUserQuestion and (see below) Bash.
- Permission prompts go to Dar: AskUserQuestion shows the answer card; every
  other request (network, outside paths, configuration writes) is declined.
- **Commands** are offered only when bubblewrap and socat are found, inside
  Claude Code's sandbox with `failIfUnavailable`: no network; `/home`, `/root`,
  `/var/lib`, secrets, Dar's root and the Claude home are unreadable except the
  working copy; writes stay in the working copy and `.git` is read-only. Without
  them Claude reads and edits files only. (In `acceptEdits` mode the CLI would
  otherwise auto-run read-only commands unsandboxed, so Bash is never offered
  without the sandbox.)
- The session ID is saved; later runs `--resume` and send only unseen messages.
  A missing session starts over with the bounded transcript.
- Steering writes into the running turn; plan mode uses `--permission-mode plan`
  and saves the plan for review; review is read-only and receives Dar's diff.
  Claude Code compacts automatically, so `/compact` is not offered.
- Usage limits come from the CLI's `get_usage` report. The terminal and Git
  actions are the same as for Codex (the terminal needs Codex).

Limits and backups match [Codex](codex.md). Tests use an isolated CLI fixture:
`nix develop -c node next/scripts/verify.mjs --agent`.
