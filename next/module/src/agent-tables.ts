import {table,t} from "spacetimedb/server";

// Private tables: native sessions enter only through serviceAgent's account check.
export const agentTables={
  agentProjects:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),name:t.string(),sourceType:t.string(),sourceRef:t.string(),createdAt:t.f64()}),
  agentThreads:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),projectId:t.string().index("btree"),title:t.string(),revision:t.u32(),createdAt:t.f64(),modifiedAt:t.f64()}),
  agentMessages:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),threadId:t.string().index("btree"),sequence:t.u32(),role:t.string(),content:t.string(),createdAt:t.f64()}),
  // Read-only thread grants to other accounts. Grantees never see runs or working copies.
  agentShares:table({}, {id:t.string().primaryKey(),threadId:t.string().index("btree"),ownerId:t.string().index("btree"),granteeId:t.string().index("btree"),createdAt:t.f64()}),
  agentRuns:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),threadId:t.string().index("btree"),sessionId:t.string(),messageId:t.string(),model:t.string(),lease:t.string(),status:t.string(),document:t.string(),createdAt:t.f64(),modifiedAt:t.f64()}),
};
