import {table,t} from "spacetimedb/server";
export const installedApps=table({}, {id:t.string().primaryKey(),document:t.string()});
// JSON tuple keys are unambiguous; indexed owner scope bounds every listing/quota scan.
export const appData=table({}, {id:t.string().primaryKey(),appId:t.string(),ownerId:t.string().index("btree"),ownerKey:t.string().index("btree"),key:t.string(),revision:t.u32(),schemaVersion:t.u32(),value:t.string(),modifiedAt:t.f64()});
export const appTables={installedApps,appData};
