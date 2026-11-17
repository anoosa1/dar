# Git

The Git app (`/git`, selected [mock B](spacetimedb/git-mocks.html)) shows the folders that
keep history as repositories, yours and those shared with you (a folder shared with you, or one
inside it). History, commits, branches and tags are the [folder history](spacetimedb/folder-history.md)
records, and reads come from each folder's Git cache; issues and pull requests are its own data. Administrators can turn the app off; Git over HTTPS keeps working, since it
belongs to Files.

## Clone addresses

- **Alias:** `/dav/git/<username>/<path from the root>.git`, each part URL-encoded. It
  follows the folder's current path and works only for its owner; shared repositories show
  the permanent address.
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

## Issues and pull requests

Added November 17, following mock B's patterns (no new mock): Issues and Pull requests tabs
after Code (open counts), rows with a state icon, labels and #number, one item in the right
sheet, and New/Edit and Labels in the right editing pane.

- **Numbers:** issues and pull requests share one sequence per repository (#1, #2…). Titles,
  Markdown descriptions and comments; `#12` links an item and records "mentioned this" on it.
- **Access:** anyone who can read the folder opens issues, comments, and edits or closes their
  own issues and comments. Write access also labels, opens, closes, reopens and merges pull
  requests and deletes any comment. App passwords cannot reach them.
- **Labels:** per repository, a name, description and one of nine named colours.
- **Pull requests:** a branch into main or another branch, or an Agent branch into main. The
  sheet has Conversation, Commits and Files (with diffs) and the web merge (Agent's merge for
  Agent branches). Whatever moves a base past its head merges the pull request in the same
  transaction: the web merge or a plain `git push`; "Closes/Fixes/Resolves #n" in its
  description then closes those issues. Deleting the branch closes it; pushes to it are recorded.
  Merged and closed ones keep their tips, so their commits stay readable while Git has them.
- **Lifetime:** they go with the folder's history when it stops keeping history.

API (browser sessions), under `/repositories/<id>`: `GET /issues?kind=issue|pull&state=open|closed&label=`,
`POST /issues` `{kind,title,body?,labels?,base?,head?}`, `GET|PATCH /issues/<n>`
(`{revision,title?,body?,state?,labels?}`), `GET /issues/<n>/changes` (a pull request's
comparison or Agent changes), `POST /issues/<n>/comments`, `PATCH|DELETE /comments/<id>`,
`GET|POST /labels`, `PATCH|DELETE /labels/<id>`. Repository summaries add `role`, `owner`,
`openIssues` and `openPulls`.

## Not included

CI, forks, reviews and web editing; files are edited in Files. Merges with conflicts are
resolved with Git and pushed.
