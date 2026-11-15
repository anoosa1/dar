import type { ServerResponse } from "node:http";
import { gzipSync } from "node:zlib";
import { offeredEncodings } from "./static.ts";

/**
 * Responses that may carry credentials or secrets stay uncompressed, so their
 * length cannot hint at their content (BREACH). Session cookies are also
 * SameSite=Strict, which keeps other sites from making these requests.
 */
const sensitive = /^\/api\/v1\/(auth|identity|apps)(\/|$)|^\/api\/v1\/photos\/(links|immich)/;

/** Writes JSON, gzip-compressed when it is a larger read the browser accepts. */
export function writeJson(res: ServerResponse, status: number, text: string, headers: Record<string, string | number> = {}) {
  const req = res.req, body = Buffer.from(text), path = (req.url ?? "").split("?")[0];
  const compress = body.length > 1024 && (req.method === "GET" || req.method === "HEAD") && !sensitive.test(path) && offeredEncodings(req.headers["accept-encoding"]).has("gzip");
  const sent = compress ? gzipSync(body, { level: 4 }) : body;
  res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store", ...headers, ...compress ? { "content-encoding": "gzip", vary: "accept-encoding" } : {}, "content-length": sent.length });
  res.end(req.method === "HEAD" ? undefined : sent);
}
