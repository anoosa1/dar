import {table,t} from "spacetimedb/server";

/**
 * Issues and pull requests for tracked folders (docs/git.md#issues-and-pull-requests). Private
 * tables, reached only through serviceGitTracker; Git pushes and Agent merges update pull requests
 * in the same transaction. Every row belongs to a fileRepos row and goes with it.
 */
export const trackerTables={
  /**
   * id is `${repoId}/${number}`; issues and pull requests share one number sequence per repository.
   * kind is "issue" or "pull"; state is "open", "closed" or "merged". labels lists label IDs, space-separated.
   * A pull request merges head into base (branch names; an Agent branch is its agent/… name). Once merged
   * or closed, baseSha and headSha keep the tips it had and mergedSha the commit that took it.
   */
  gitIssues:table({}, {id:t.string().primaryKey(),repoId:t.string().index("btree"),number:t.u32(),kind:t.string(),title:t.string(),body:t.string(),state:t.string(),
    authorId:t.string(),createdAt:t.f64(),updatedAt:t.f64(),closedAt:t.f64(),closedBy:t.string(),labels:t.string(),
    base:t.string(),head:t.string(),baseSha:t.string(),headSha:t.string(),mergedSha:t.string(),revision:t.u32()}),
  /**
   * The timeline: comments (kind "comment", Markdown body) and events (kind is the event; data is a small
   * JSON object such as {"number":4} or {"from":"Old title"}). issueId is the gitIssues ID; sequence orders
   * the timeline, since one change can add several entries at the same time.
   */
  gitComments:table({}, {id:t.string().primaryKey(),issueId:t.string().index("btree"),repoId:t.string().index("btree"),sequence:t.u32(),kind:t.string(),authorId:t.string(),body:t.string(),data:t.string(),createdAt:t.f64(),editedAt:t.f64()}),
  /** A repository's labels; color is one of gitLabelColors. */
  gitLabels:table({}, {id:t.string().primaryKey(),repoId:t.string().index("btree"),name:t.string(),color:t.string(),description:t.string(),createdAt:t.f64()}),
};
