/**
 * Database operations that only read. App-password scopes use the Photos and News
 * sets to tell reads from changes; the runtime also calls every listed operation
 * without waiting for the database to confirm durability, which a read does not
 * need. Only add an operation here if it can never insert, update or delete.
 */
export const photosReadOperations:ReadonlySet<string>=new Set(["overview","page","detail","asset","duplicates","stackDetail","albumSharing"]);
export const newsReadOperations:ReadonlySet<string>=new Set(["access","overview","folders","feeds","items","itemsUpdated","item","exportData","feedInfo"]);
export const pimReadOperations:ReadonlySet<string>=new Set(["resource","collections","preferences","reminders","page","list","grants","read","search"]);
export const agentReadOperations:ReadonlySet<string>=new Set(["source","folders","sharedThreads","sharedHistory","projects","threads","threadShares","history","run"]);
export const workspaceReadOperations:ReadonlySet<string>=new Set(["trash"]);
export const identityReadOperations:ReadonlySet<string>=new Set(["users","user","groups","group","clients","tokens","storage"]);
export const booksReadOperations:ReadonlySet<string>=new Set(["detail"]);
export const historyReadOperations:ReadonlySet<string>=new Set(["overview","changeset","content","branch"]);
/** Git reads (refs and history for a clone or fetch, the Git app's list, clone aliases); pin, pushBegin and push write. */
export const gitReadOperations:ReadonlySet<string>=new Set(["state","export","repositories","resolve","changesetOf"]);
/** Issues and pull requests: lists, one item and labels; everything else writes. */
export const gitTrackerReadOperations:ReadonlySet<string>=new Set(["list","issue","labels"]);
