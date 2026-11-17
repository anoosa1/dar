# Folder history

Git-like history for a Files folder, built on Files' own entries, versions and
blobs (maintainer decision, November 16, 2026). Scion-shaped: the server owns
history and commits are reducers. Git clients clone, fetch and push over HTTPS
(see Git below; maintainer decision, November 16).

## Model

- **Tracked folder (repository):** any folder you can write that is not in the
  Locked folder and not inside another tracked folder. A tracked folder nested
  in another is left out of the outer one's history.
- **Working tree:** the live folder. Files, WebDAV, Notes and other apps keep
  writing it normally; nothing is committed automatically.
- **Changeset:** an atomic, ordered entry on `main` listing the paths that changed
  against its parent (added, modified, deleted, renamed), each pointing at a
  file version. Kinds: commit, snapshot (automatic message), restore and merge.
- **Head tree:** the paths and versions at `main`, kept so status and commits
  never replay history. Older trees are rebuilt by walking back from the head.
- **Branches:** `agent/<thread>` holds one pending changeset proposed by an Agent
  thread (the Agent project must be the tracked folder). Committing again
  replaces it. Merge applies it to the live folder and commits it to `main`;
  discard drops it.

## Hybrid commits

Commits are explicit. The History tab always shows the uncommitted changes with
diffs against `main`; Commit takes a message and optionally a subset of paths,
Snapshot commits everything with an automatic message. Per-file versions remain
the safety net for anything not committed.

## Storage and quota

- History references file **versions**, the existing quota unit, so a commit adds
  no bytes. Permanently deleting a file keeps the versions history references
  (still counted); they leave quota when the history goes.
- Agent branch content is uploaded as staged versions that belong to no file
  (`staged:<branch>`) and count from upload. Merge hands them to the files.
- Stop tracking deletes the history; versions of live files stay in Versions,
  the rest are released. Deleting the tracked folder permanently does the same.
- Blob reclaim already keeps any blob a version references.

## Rules

- Read access to the folder shows its history (as in Git, shared readers see past
  content). Write access commits, restores and merges; only the owner stops
  tracking.
- Commits carry a status token; a change to the folder after the dialog loaded is
  a conflict. Restore needs a clean folder and replaces changed paths in place,
  recreates missing files and moves extra files to deleted files, then commits.
  Merge checks each file still has the revision the agent started from.
- Limits: 2,000 files and depth 32 per tracked folder, 2,000 paths per changeset,
  10,000 changesets, 50 tracked folders per account, 2 KiB messages.
- Backups (format v9) include history and its Git objects and refs; restores accept
  versions kept by history. Backups from before Git support restore without them.

## Git

Each tracked folder is a Git repository at `/dav/git/<folderId>.git` (smart HTTP,
protocol v0 and v2; shallow and partial clones). It is also reachable by alias at
`/dav/git/<username>/<path from the root>.git` (each part URL-encoded), which follows
the folder's current path; only its owner can use it. The ID address never changes. Git signs in with the username and
an app password: Files read access clones and fetches, Files write access pushes.

- **Same IDs everywhere:** a Dar commit becomes a Git commit (author name and
  `<username@dar.invalid>`, UTC) the first time Git sees it, and its exact bytes are
  stored, as are pushed commits and annotated tags. Clones, bundles and rebuilt
  caches always give the same IDs.
- **Cache:** `<root>/git/<folderId>.git` is a bare repository rebuilt from the
  database when missing; maintenance removes those of folders that stopped keeping
  history. It is not backed up.
- **Pushing main:** fast-forward only and never deleted, like a protected branch.
  Its new commits (first-parent line) join the history, keeping their authors,
  times and messages, and change the live folder in the same transaction: renames
  move files, deletions go to deleted files. A push is refused when a file it
  changes has uncommitted changes in Dar (commit them, pull, push again); other
  uncommitted edits stay as they are.
- **Other branches, tags and notes** (`refs/heads`, `refs/tags`, `refs/notes`) are
  kept without changing the folder; force pushes and deletions work. Their content
  belongs to no file and counts against the quota until nothing reaches it.
  `refs/heads/agent/*` belongs to Agent.
- **Kept from Git:** executable files and symbolic links keep their Git modes in
  history; in Files a link is a regular file holding its target.
- **Refused:** submodules, names Dar cannot keep (a backslash, case-only
  differences), shallow pushes, packs over 4 GiB, objects over 256 KiB, and more
  than 5,000 commits or 50,000 changed paths in one push (push in parts). Pushed
  objects are checked with `git index-pack --strict`.
- **Uploads:** like Files uploads, a push may take as long as it keeps moving: a body is
  dropped after a minute without new bytes, below 16 KiB a minute, or after 6 hours
  (`runtime/upload-pace.ts`). While Dar checks and stores a push it sends Git keep-alive
  packets, so proxies do not time out.

## API

Under `/api/v1/files/history/<folderId>`: `GET` overview (repository, status,
branches, changesets in pages of 50), `POST` track or untrack, `POST /commits`,
`GET /commits/<id>`, `POST /restore`, `GET /versions/<id>/content`,
`GET /bundle` (Git bundle of every branch and tag) and `POST /branches/merge|discard` with the
branch name and repository revision. Agent: `GET|POST /api/v1/agent/threads/<id>/branch`.

## UI

Folder details replace the empty Versions tab with History: Track history, the
uncommitted changes with Commit and Snapshot, agent branches with Merge and
Discard, and the changeset list. A changeset or the commit form opens in the
right editing pane with per-file diffs (shared with Agent's review). Agent's
review offers Commit to branch for tracked folders. A Git card shows the clone
address and the pushed branches and tags; commits Git has seen show their short ID.
