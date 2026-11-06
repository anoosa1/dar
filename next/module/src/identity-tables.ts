import { table, t } from "spacetimedb/server";

export const directoryProfiles = table({}, { userId: t.string().primaryKey(), document: t.string(), revision: t.u32(), modifiedAt: t.f64(), deleted: t.bool() });
export const directoryGroups = table({}, { id: t.string().primaryKey(), document: t.string(), revision: t.u32(), createdAt: t.f64(), modifiedAt: t.f64() });
export const directoryMembers = table({}, { id: t.string().primaryKey(), groupId: t.string().index("btree"), userId: t.string().index("btree") });
export const provisioningTokens = table({}, { id: t.string().primaryKey(), digest: t.string().unique(), ownerId: t.string().index("btree"), label: t.string(), scope: t.string(), expiresAt: t.f64(), revoked: t.bool() });
export const identityProofs = table({}, { digest: t.string().primaryKey(), sessionId: t.string().index("btree"), verifier: t.string(), expiresAt: t.f64() });
export const identityClients = table({}, { id: t.string().primaryKey(), metadata: t.string(), encrypted: t.string(), enabled: t.bool(), revision: t.u32(), modifiedAt: t.f64() });
export const providerModels = table({}, { id: t.string().primaryKey(), model: t.string(), encrypted: t.string(), accountId: t.string().index("btree"), clientId: t.string().index("btree"), grantKey: t.string().index("btree"), sessionKey: t.string().index("btree"), uidKey: t.string().index("btree"), userCodeKey: t.string().index("btree"), nativeSessionId: t.string().index("btree"), expiresAt: t.f64(), expiryKey: t.string().index("btree"), consumed: t.f64() });
export const identityMigrations = table({}, { id: t.u32().primaryKey(), appliedAt: t.f64() });
export const identityTables = { directoryProfiles, directoryGroups, directoryMembers, provisioningTokens, identityProofs, identityClients, providerModels, identityMigrations };
