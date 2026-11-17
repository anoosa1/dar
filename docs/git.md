# Git

The Git app (`/git`, selected [mock B](spacetimedb/git-mocks.html)) shows the folders that
keep history as repositories. It owns no data: history, commits, branches and tags are the
[folder history](spacetimedb/folder-history.md) records, and reads come from each folder's
Git cache. Administrators can turn the app off; Git over HTTPS keeps working, since it
belongs to Files.

## Clone addresses

- **Alias:** `/dav/git/<username>/<path from the root>.git`, each part URL-encoded. It
  follows the folder's current path and works only for its owner.
- **Permanent:** `/dav/git/<folderId>.git`, unchanged by renames and moves.

Both sign in with the username and an app password with Files access (read to clone and
fetch, write to push).

## API (`/api/v1/git`, browser sessions)

- `GET /repositories`, `GET /repositories/<id>`: the caller's tracked folders with path,
  README description, commit/branch/tag counts, 12 weeks of activity, uncommitted file
  count, history size and both clone addresses.
- `GET /activity`: commit times over the last year and the newest commits across
  repositories.
- `GET /repositories/<id>/tree?ref=&path=`, `/blob?ref=&path=`: a folder at a branch, tag
  or commit with each entry's newest commit and README; a file (inline text with `view`,
  otherwise a download; sandboxed).
- `GET /repositories/<id>/commits?ref=&author=&source=&skip=&limit=`: newest first, with
  parents, refs, line counts and source (`git`, `dar` for Dar commits, `agent` for Agent
  merges). `ref` empty or `all` lists every branch; `source` filters the whole history.
- `GET /repositories/<id>/commits/<sha>`: message, parents and changed files with unified
  hunks against the first parent (large diffs are left out). A commit on main's history also
  names its history entry (`restore`), which `POST /api/v1/files/history/<id>/restore` takes.
- `GET /repositories/<id>/branches`, `/tags`, `/compare?base=&head=`: divergence from
  main, Agent branches, tags, and the commits and files one branch has over another, whether
  it can fast-forward and whether Git can merge it without conflicts.
- `POST /repositories/<id>/merge` with `{base, head, baseSha, headSha, mode, message?,
  deleteBranch?}`: `mode` is `fast-forward` or `merge` (a merge commit written by Git without a
  working tree, authored by the caller as a Dar commit). The tips must still be the ones the
  caller saw. The ref moves through the same checks as a push: conflicts, a stale view, or files
  with uncommitted Dar edits refuse it, and merging into main changes the folder.
- `GET /repositories/<id>/archive?ref=`: a ZIP of a branch, tag or commit.
- `DELETE /repositories/<id>/refs` with `{name, old}`: deletes a branch or tag through the
  same checks as `git push --delete` (a stale `old` is a conflict; main is never deleted;
  content nothing reaches any more is released).

## Not included

Issues, pull requests, CI, forks and web editing; files are edited in Files. Merges with
conflicts are resolved with Git and pushed.
