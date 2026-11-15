import type { IncomingMessage, ServerResponse } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { createHash } from "node:crypto";

/** Built client files: content-hashed names never change, so browsers keep them for a year. */
const hashed = /^(?:(?:app|sdk)-[A-Z0-9]{8}\.(?:js|css)|chunks\/[a-z0-9_-]+-[A-Z0-9]{8}\.js)$/;
/** Stable names that browsers revalidate: the page, the installed-app runtime and server-rendered page styles. */
const stable = new Set(["index.html", "app.css", "sdk.js", "sdk.css"]);
interface Asset { identity: Buffer; br?: Buffer; gzip?: Buffer; etag: string; version: string }
const types: Record<string, string> = { js: "text/javascript; charset=utf-8", css: "text/css; charset=utf-8", html: "text/html; charset=utf-8" };

async function optional(file: URL) { try { return await readFile(file); } catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined; throw error; } }
/** The preferred encoding the client accepts (q > 0), brotli first. */
/** Content encodings the client accepts (q > 0). */
export function offeredEncodings(header: string | string[] | undefined) {
  const offered = new Set<string>();
  for (const part of String(header ?? "").toLowerCase().split(",")) {
    const [token, ...parameters] = part.trim().split(";"), q = parameters.map(value => /^\s*q=([0-9.]+)\s*$/.exec(value)?.[1]).find(Boolean);
    if (token && (q === undefined || Number(q) > 0)) offered.add(token.trim());
  }
  return offered;
}
function accepted(header: string | string[] | undefined) {
  const offered = offeredEncodings(header);
  return offered.has("br") ? "br" : offered.has("gzip") ? "gzip" : undefined;
}

/**
 * Serves the built client from memory with the build's precompressed copies,
 * strong ETags and long-lived caching for hashed names. Stable names are
 * re-read when the build changes them; unknown paths get the app page.
 */
export function staticAssets(directory: URL) {
  const loaded = new Map<string, Promise<Asset | undefined>>();
  async function load(name: string, version: string): Promise<Asset | undefined> {
    const file = new URL(name, directory), identity = await optional(file);
    if (!identity) return undefined;
    const [br, gzip] = await Promise.all([optional(new URL(`${name}.br`, directory)), optional(new URL(`${name}.gz`, directory))]);
    return { identity, br, gzip, etag: `"${createHash("sha256").update(identity).digest("base64url").slice(0, 27)}"`, version };
  }
  async function asset(name: string) {
    // A hashed file's content is fixed by its name; stable names are checked for a newer build.
    let version = "";
    if (!hashed.test(name)) {
      try { const info = await stat(new URL(name, directory)); version = `${info.mtimeMs}:${info.size}`; } catch { return undefined; }
    }
    const cached = loaded.get(name);
    if (cached) { const value = await cached.catch(() => undefined); if (value && value.version === version) return value; }
    const next = load(name, version);
    loaded.set(name, next); next.catch(() => loaded.delete(name));
    return await next;
  }
  // App pages use the build's variant that preloads their code (pages.json), else index.html.
  let routes: { version: string; map: Record<string, string> } | undefined;
  async function page(pathname: string) {
    const listing = await asset("pages.json");
    if (!listing) return "index.html";
    if (routes?.version !== listing.version) { try { routes = { version: listing.version, map: JSON.parse(listing.identity.toString()) }; } catch { return "index.html"; } }
    const name = routes.map[pathname];
    return typeof name === "string" && /^pages\/[a-z-]+\.html$/.test(name) ? name : "index.html";
  }
  return {
    async send(req: IncomingMessage, res: ServerResponse, pathname: string) {
      const requested = pathname.slice(1), name = hashed.test(requested) || stable.has(requested) ? requested : await page(pathname), value = await asset(name);
      if (!value) { res.writeHead(404, { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" }); res.end(req.method === "HEAD" ? undefined : "Not found"); return; }
      const headers: Record<string, string | number> = {
        "content-type": types[name.split(".").at(-1)!], etag: value.etag, vary: "accept-encoding", "x-content-type-options": "nosniff",
        "cache-control": hashed.test(name) ? "public, max-age=31536000, immutable" : "no-cache",
      };
      const match = String(req.headers["if-none-match"] ?? "").split(",").map(tag => tag.trim().replace(/^W\//, ""));
      if (match.includes(value.etag) || match.includes("*")) { res.writeHead(304, headers); res.end(); return; }
      const encoding = accepted(req.headers["accept-encoding"]), body = encoding === "br" ? value.br : encoding === "gzip" ? value.gzip : undefined;
      if (body) headers["content-encoding"] = encoding!;
      headers["content-length"] = (body ?? value.identity).length;
      res.writeHead(200, headers);
      res.end(req.method === "HEAD" ? undefined : body ?? value.identity);
    },
  };
}
