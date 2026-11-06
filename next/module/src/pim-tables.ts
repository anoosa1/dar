import { table, t } from "spacetimedb/server";

// Shared collection mechanics; contact/event validation belongs to their own
// standards adapters. These private tables never expose raw content to SQL users.
export const pimCollections = table({}, { id: t.string().primaryKey(), ownerId: t.string().index("btree"), kind: t.string(), name: t.string(), color: t.string(), revision: t.u32(), modifiedAt: t.f64() });
export const pimResources = table({}, { id: t.string().primaryKey(), collectionId: t.string().index("btree"), href: t.string(), hrefKey: t.string().unique(), uid: t.string(), uidKey: t.string().unique(), content: t.string(), projection: t.string(), revision: t.u32(), modifiedAt: t.f64() });
export const pimGrants = table({}, { id: t.string().primaryKey(), collectionId: t.string().index("btree"), userId: t.string().index("btree"), role: t.string() });
export const pimChanges = table({}, { userId: t.string().primaryKey(), revision: t.u32() });
export const pimPreferences = table({}, { userId: t.string().primaryKey(), value: t.string(), revision: t.u32() });
export const pimReminders = table({}, { id: t.string().primaryKey(), userId: t.string().index("btree"), resourceId: t.string().index("btree"), resourceRevision: t.u32(), dueAt: t.f64(), title: t.string(), acknowledged: t.bool() });
export const pimReminderCursors = table({}, { resourceId: t.string().primaryKey(), revision: t.u32(), at: t.f64(), runAt: t.string().index("btree").default("") });
export const pimTables = { pimCollections, pimResources, pimGrants, pimChanges, pimPreferences, pimReminders, pimReminderCursors };
