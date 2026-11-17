import {table,t} from "spacetimedb/server";

/**
 * Folder history (docs/spacetimedb/folder-history.md). Private tables, reached only
 * through serviceHistory and serviceGit. Changes point at file versions, which carry the bytes and quota.
 */
export const historyTables={
  /** One row per tracked folder; id is the folder's entry ID. revision guards every history change. */
  fileRepos:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),headId:t.string(),sequence:t.u32(),revision:t.u32(),createdAt:t.f64(),modifiedAt:t.f64()}),
  /**
   * branch is "" on main, "git" for commits only Git branches and tags reach, or an agent branch name.
   * A changeset's changes are against parentId's tree (main: the previous main changeset; git: its
   * first Git parent); an agent branch lists its changes against the live folder.
   */
  fileChangesets:table({}, {id:t.string().primaryKey(),repoId:t.string().index("btree"),branch:t.string(),parentId:t.string(),sequence:t.u32(),kind:t.string(),message:t.string(),authorId:t.string(),fileCount:t.u32(),createdAt:t.f64()}),
  /**
   * previousVersionId is the version the path held before (or, for branches, unused); expectRevision is the live revision a
   * branch change applies to. mode and previousMode are Git file modes; "" is a regular file (100644).
   */
  fileChanges:table({}, {id:t.string().primaryKey(),changesetId:t.string().index("btree"),path:t.string(),previousPath:t.string(),kind:t.string(),fileId:t.string(),versionId:t.string().index("btree"),previousVersionId:t.string(),expectRevision:t.u32(),mode:t.string().default(""),previousMode:t.string().default("")}),
  /** The paths and versions at main's head, keyed `${repoId}/${path}`. */
  fileTrees:table({}, {id:t.string().primaryKey(),repoId:t.string().index("btree"),path:t.string(),fileId:t.string(),versionId:t.string(),mode:t.string().default("")}),
  fileBranches:table({}, {id:t.string().primaryKey(),repoId:t.string().index("btree"),name:t.string(),threadId:t.string(),changesetId:t.string(),authorId:t.string(),createdAt:t.f64(),modifiedAt:t.f64()}),
  /**
   * Git commits and annotated tags, keyed `${repoId}/${sha}`. raw is the object's exact bytes in base64, so
   * clones always get the same IDs. A Dar commit is written down the first time Git sees it.
   * parents lists parent commits (or a tag's target), space-separated; author is the display name.
   */
  fileGitObjects:table({}, {id:t.string().primaryKey(),repoId:t.string().index("btree"),sha:t.string(),type:t.string(),changesetId:t.string().index("btree"),parents:t.string(),author:t.string(),raw:t.string(),createdAt:t.f64()}),
  /** Git branches and tags other than main (main follows the folder's history), keyed `${repoId}/${name}`. */
  fileGitRefs:table({}, {id:t.string().primaryKey(),repoId:t.string().index("btree"),name:t.string(),target:t.string(),updatedAt:t.f64()}),
  /** Git blob IDs of stored content, keyed `${repoId}/${sha}`, so a push stores each file content once. */
  fileGitBlobs:table({}, {id:t.string().primaryKey(),repoId:t.string().index("btree"),sha:t.string(),blobId:t.string().index("btree"),size:t.f64()}),
};
