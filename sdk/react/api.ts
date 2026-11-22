/** The signed-in person, as the runtime reports them. */
export interface User { id: string; username: string; displayName: string; administrator: boolean }
export interface BrowserSession { user: User; csrfToken: string }
export class RequestError extends Error {
  status: number;
  constructor(message: string, status: number) { super(message); this.status = status; }
}
export function readProcedure<T>(value: string): T {
  const result = JSON.parse(value);
  if (result?.error) throw new Error(result.error.code ?? "The request could not be completed.");
  return result as T;
}
// Contacts, Calendar, Notes and Tasks responses carry the account's change
// revision, so a page can tell whether live updates are newer than what it loaded.
const pimPath = /^\/api\/v1\/(contacts|calendar|notes|tasks|pim)(\/|$)/, pimSettled: (() => void)[] = [];
let pimRevision = 0, pimPending = 0, pimAccount = "";
/** The newest PIM revision loaded on this page for `account`; starts over for another account. */
export function loadedPimRevision(account: string) { if (account !== pimAccount) { pimAccount = account; pimRevision = 0; } return pimRevision; }
/** Runs once no Contacts, Calendar, Notes or Tasks request is in flight. */
export function whenPimSettled(run: () => void) { if (pimPending) pimSettled.push(run); else run(); }
export async function request<T>(url: string, init: RequestInit = {}, csrf?: string): Promise<T> {
  const pim = typeof location !== "undefined" && pimPath.test(new URL(url, location.href).pathname);
  if (pim) pimPending++;
  try {
    const response = await fetch(url, { ...init, headers: { ...(init.body instanceof File ? {} : { "content-type": "application/json" }), ...(csrf ? { "x-csrf-token": csrf } : {}), ...init.headers } });
    let value;
    try { value = await response.json(); }
    catch { throw new RequestError("The instance returned an unexpected response. Try again shortly.", response.status); }
    if (!response.ok) throw new RequestError(value.error?.message ?? "The request could not be completed.", response.status);
    if (pim) pimRevision = Math.max(pimRevision, Number(response.headers.get("x-dar-pim-revision")) || 0);
    return value as T;
  } finally { if (pim && !--pimPending) for (const run of pimSettled.splice(0)) run(); }
}
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let n = bytes / 1024, index = 0;
  while (n >= 1024 && index < units.length - 1) { n /= 1024; index++; }
  return `${n.toFixed(n < 10 ? 1 : 0)} ${units[index]}`;
}
