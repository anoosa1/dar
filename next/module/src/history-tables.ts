import {table,t} from "spacetimedb/server";

/**
 * Folder history (docs/spacetimedb/folder-history.md). Private tables, reached only
 * through serviceHistory. Changes point at file versions, which carry the bytes and quota.
 */
export const historyTables={
  /** One row per tracked folder; id is the folder's entry ID. revision guards every history change. */
  fileRepos:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),headId:t.string(),sequence:t.u32(),revision:t.u32(),createdAt:t.f64(),modifiedAt:t.f64()}),
  /** branch is "" on main; a branch changeset lists its changes against the folder, not a parent tree. */
  fileChangesets:table({}, {id:t.string().primaryKey(),repoId:t.string().index("btree"),branch:t.string(),parentId:t.string(),sequence:t.u32(),kind:t.string(),message:t.string(),authorId:t.string(),fileCount:t.u32(),createdAt:t.f64()}),
  /** previousVersionId is the version the path held before (or, for branches, unused); expectRevision is the live revision a branch change applies to. */
  fileChanges:table({}, {id:t.string().primaryKey(),changesetId:t.string().index("btree"),path:t.string(),previousPath:t.string(),kind:t.string(),fileId:t.string(),versionId:t.string().index("btree"),previousVersionId:t.string(),expectRevision:t.u32()}),
  /** The paths and versions at main's head, keyed `${repoId}/${path}`. */
  fileTrees:table({}, {id:t.string().primaryKey(),repoId:t.string().index("btree"),path:t.string(),fileId:t.string(),versionId:t.string()}),
  fileBranches:table({}, {id:t.string().primaryKey(),repoId:t.string().index("btree"),name:t.string(),threadId:t.string(),changesetId:t.string(),authorId:t.string(),createdAt:t.f64(),modifiedAt:t.f64()}),
};
