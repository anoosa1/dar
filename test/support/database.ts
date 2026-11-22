// The service credential's view of the database, for tests that set up or inspect state the
// way the runtime does: procedures and reducers called over SpacetimeDB's HTTP API.
import { readFile } from "node:fs/promises";

export class AppError extends Error {
  readonly code: string;
  readonly status: number;
  constructor(code: string, status = 400, message = code) {
    super(message);
    this.code = code;
    this.status = status;
  }
}

/** The owner credential stays in the test process; never send it to a browser. */
export async function database(url: string, name: string, tokenFile: string) {
  const token = (await readFile(tokenFile, "utf8")).trim();
  if (!token) throw new Error("A database service credential is required.");
  async function invoke<T>(nameOfFunction: string, args: unknown[], confirmed: boolean): Promise<T> {
    const response = await fetch(`${url}/v1/database/${encodeURIComponent(name)}/call/${nameOfFunction}${confirmed ? "" : "?confirmed=false"}`, {
      method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
      body: JSON.stringify(args), signal: AbortSignal.timeout(15_000),
    });
    if (!response.ok) {
      const detail = await response.text();
      for (const [code, status] of [["lockedFolder", 423], ["nothingToCommit", 409], ["uncommittedChanges", 409], ["conflict", 409], ["alreadyExists", 409], ["notFound", 404], ["forbidden", 403], ["unauthenticated", 401], ["invalidInput", 400], ["invalidName", 400], ["quotaExceeded", 507], ["operationTooLarge", 413], ["contentUnavailable", 503]] as const) {
        if (detail.includes(code)) throw new AppError(code, status, code === "lockedFolder" ? "Unlock the Locked folder with your password to continue." : code);
      }
      throw new AppError("databaseUnavailable", 503, "The database request could not be completed.");
    }
    const text = await response.text();
    if (!text) return undefined as T;
    const value: unknown = JSON.parse(text);
    return (typeof value === "string" ? JSON.parse(value) : value) as T;
  }
  return {
    url, name, tokenFile,
    /** Runs a procedure or reducer and answers once its changes are durable. */
    call<T = unknown>(nameOfFunction: string, args: unknown[] = []): Promise<T> { return invoke<T>(nameOfFunction, args, true); },
    /** Runs a read-only procedure without waiting for a flush. */
    read<T = unknown>(nameOfFunction: string, args: unknown[] = []): Promise<T> { return invoke<T>(nameOfFunction, args, false); },
  };
}
export type Database = Awaited<ReturnType<typeof database>>;
