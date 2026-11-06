import { readFile } from "node:fs/promises";
import { AppError } from "./errors.ts";

/** The owner credential stays in the integration process; never send it to a browser. */
export async function database(url: string, name: string, tokenFile: string) {
  const token = (await readFile(tokenFile, "utf8")).trim();
  if (!token) throw new Error("A database service credential is required.");
  return {
    url, name,
    async call<T = unknown>(nameOfFunction: string, args: unknown[] = []): Promise<T> {
      const response = await fetch(`${url}/v1/database/${encodeURIComponent(name)}/call/${nameOfFunction}`, {
        method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
        body: JSON.stringify(args), signal: AbortSignal.timeout(15_000),
      });
      if (!response.ok) {
        const detail = await response.text();
        for (const [code, status] of [["conflict", 409], ["notFound", 404], ["forbidden", 403], ["unauthenticated", 401], ["invalidInput", 400], ["invalidName", 400], ["quotaExceeded", 507], ["operationTooLarge", 413], ["contentUnavailable", 503]] as const) {
          if (detail.includes(code)) throw new AppError(code, status);
        }
        throw new AppError("databaseUnavailable", 503, "The database request could not be completed.");
      }
      const text = await response.text();
      if (!text) return undefined as T;
      const value: unknown = JSON.parse(text);
      return (typeof value === "string" ? JSON.parse(value) : value) as T;
    },
  };
}
export type Database = Awaited<ReturnType<typeof database>>;
