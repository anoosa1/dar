import {table,t} from "spacetimedb/server";
// Trash is private to the notebook owner, even when a writer trashes a shared note.
export const noteTrash=table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),document:t.string(),notebookName:t.string(),trashedAt:t.f64()});
export const workspaceMigrations=table({}, {id:t.u32().primaryKey(),appliedAt:t.f64()});
export const workspaceTables={noteTrash,workspaceMigrations};
