import type { User } from "../runtime/auth/service.ts";
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
export async function request<T>(url: string, init: RequestInit = {}, csrf?: string): Promise<T> {
  const response = await fetch(url, { ...init, headers: { ...(init.body instanceof File ? {} : { "content-type": "application/json" }), ...(csrf ? { "x-csrf-token": csrf } : {}), ...init.headers } });
  let value;
  try { value = await response.json(); }
  catch { throw new RequestError("The instance returned an unexpected response. Try again shortly.", response.status); }
  if (!response.ok) throw new RequestError(value.error?.message ?? "The request could not be completed.", response.status);
  return value as T;
}
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let n = bytes / 1024, index = 0;
  while (n >= 1024 && index < units.length - 1) { n /= 1024; index++; }
  return `${n.toFixed(n < 10 ? 1 : 0)} ${units[index]}`;
}
