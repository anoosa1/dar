import type { IncomingMessage, ServerResponse } from "node:http";
import { randomUUID } from "node:crypto";
import type { Database } from "../database.ts";
import { digestToken } from "../auth/session.ts";
import { AppError } from "../errors.ts";
import type { blobStore, BlobFile } from "./blobs.ts";

interface Entry extends BlobFile { kind: string; modifiedAt: number }
const escape = (value: string) => value.replace(/[<>&"']/g, c => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;", '"': "&quot;", "'": "&apos;" })[c]!);
const etag = (entry: Entry) => `"${entry.id}:${entry.revision}:${entry.blobId}"`;
function condition(req: IncomingMessage, entry: Entry) {
  const match = req.headers["if-match"];
  if (!match) throw new AppError("preconditionRequired", 428);
  if (match !== "*" && match !== etag(entry)) throw new AppError("revisionConflict", 412);
}
function propertyResponse(entry: Entry, href: string) {
  return `<d:response><d:href>${escape(href)}</d:href><d:propstat><d:prop><d:displayname>${escape(entry.name)}</d:displayname><d:resourcetype>${entry.kind === "folder" ? "<d:collection/>" : ""}</d:resourcetype><d:getcontentlength>${entry.size}</d:getcontentlength><d:getcontenttype>${escape(entry.mediaType)}</d:getcontenttype><d:getetag>${escape(etag(entry))}</d:getetag><d:getlastmodified>${new Date(entry.modifiedAt).toUTCString()}</d:getlastmodified><d:creationdate>${new Date(entry.modifiedAt).toISOString()}</d:creationdate></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>`;
}

/** Standards paths expose logical Files resources; they never map to disk paths. */
export async function handleDav(req: IncomingMessage, res: ServerResponse, db: Database, blobs: ReturnType<typeof blobStore>, origin: string) {
  const method = req.method ?? "GET";
  if (req.headers.origin && req.headers.origin !== origin) throw new AppError("invalidOrigin", 403);
  res.setHeader("allow", "OPTIONS, PROPFIND, GET, HEAD, PUT, MKCOL, DELETE, MOVE, COPY");
  res.setHeader("dav", "1");
  res.setHeader("cache-control", "no-store");
  if (method === "OPTIONS") { res.writeHead(204); res.end(); return; }
  const authorization = req.headers.authorization;
  if (!authorization?.startsWith("Basic ") || authorization.length > 2048) { res.setHeader("www-authenticate", 'Basic realm="Dar Files", charset="UTF-8"'); throw new AppError("unauthenticated", 401); }
  const decoded = Buffer.from(authorization.slice(6), "base64").toString("utf8"), separator = decoded.indexOf(":");
  if (separator < 1) throw new AppError("unauthenticated", 401);
  const username = decoded.slice(0, separator), secret = decoded.slice(separator + 1);
  if (!/^[A-Za-z0-9_-]{43}$/.test(secret)) throw new AppError("unauthenticated", 401);
  const authenticated = await db.call<{ sessionId: string; username: string }>("service_protocol_authenticate", [username, digestToken(secret), !["GET", "HEAD", "PROPFIND"].includes(method)]);
  const url = new URL(req.url!, origin), prefix = `/dav/files/${encodeURIComponent(authenticated.username)}`;
  if (url.pathname !== prefix && !url.pathname.startsWith(`${prefix}/`)) throw new AppError("forbidden", 403);
  function logical(pathname: string) {
    if (pathname !== prefix && !pathname.startsWith(`${prefix}/`)) throw new AppError("forbidden", 403);
    const pieces = pathname.slice(prefix.length).split("/").filter(Boolean).map(piece => decodeURIComponent(piece));
    if (pieces.some(piece => /[\x00-\x1f/\\]/.test(piece) || piece === "." || piece === "..")) throw new AppError("invalidPath");
    return pieces.join("/");
  }
  const pathname = logical(url.pathname);
  const operation = <T>(verb: string, extra: { destination?: string; expectedRevision?: number; overwrite?: boolean; depth?: number } = {}) => db.call<T>("service_dav", [authenticated.sessionId, verb, pathname, extra.destination ?? "", extra.expectedRevision ?? 0, extra.overwrite ?? false, extra.depth ?? 0]);
  if (method === "PROPFIND") {
    if (req.headers.depth !== "0" && req.headers.depth !== "1") throw new AppError("finiteDepthRequired", 403);
    if (Number(req.headers["content-length"] ?? 0) > 16_384) throw new AppError("requestTooLarge", 413);
    req.resume();
    const value = await operation<{ root: Entry; children: Entry[] }>("PROPFIND", { depth: Number(req.headers.depth) });
    const href = `${prefix}${pathname ? `/${pathname.split("/").map(encodeURIComponent).join("/")}` : ""}${value.root.kind === "folder" ? "/" : ""}`;
    const xml = `<?xml version="1.0" encoding="UTF-8"?><d:multistatus xmlns:d="DAV:">${propertyResponse(value.root, href)}${value.children.map(child => propertyResponse(child, `${href}${encodeURIComponent(child.name)}${child.kind === "folder" ? "/" : ""}`)).join("")}</d:multistatus>`;
    res.writeHead(207, { "content-type": "application/xml; charset=utf-8" }); res.end(xml); return;
  }
  if (method === "GET" || method === "HEAD") {
    const entry = await operation<Entry>("STAT");
    if (entry.kind !== "file") throw new AppError("isDirectory", 405);
    const file = await db.call<BlobFile>("service_download", [authenticated.sessionId, entry.id, ""]);
    return await blobs.download(req, res, file);
  }
  if (method === "MKCOL") {
    if (Number(req.headers["content-length"] ?? 0)) throw new AppError("unsupportedBody", 415);
    req.resume(); await operation("MKCOL"); res.writeHead(201); res.end(); return;
  }
  if (method === "PUT") {
    const target = await operation<{ entry?: Entry; parentId: string; name: string }>("PREPARE");
    if (target.entry) {
      if (target.entry.kind !== "file") throw new AppError("isDirectory", 405);
      if (req.headers["if-none-match"] === "*") throw new AppError("revisionConflict", 412);
      condition(req, target.entry);
    }
    if (req.headers["content-length"] === undefined) throw new AppError("lengthRequired", 411);
    const size = Number(req.headers["content-length"]);
    if (!Number.isSafeInteger(size) || size < 0 || size > 1024 ** 3) throw new AppError("uploadTooLarge", 413);
    const id = randomUUID(), fileId = target.entry?.id ?? randomUUID();
    await db.call("service_begin_upload", [authenticated.sessionId, id, fileId, target.parentId, target.name, size, req.headers["content-type"]?.split(";")[0] ?? "application/octet-stream", target.entry?.revision ?? 0]);
    try {
      const digest = await blobs.receive(req, id, size);
      await db.call("service_finish_upload", [authenticated.sessionId, id, digest]);
    } catch (error) { await db.call("service_cancel_upload", [authenticated.sessionId, id]).catch(() => {}); throw error; }
    res.setHeader("etag", etag(await operation<Entry>("STAT"))); res.writeHead(target.entry ? 204 : 201); res.end(); return;
  }
  if (["DELETE", "MOVE", "COPY"].includes(method)) {
    const entry = await operation<Entry>("STAT"); condition(req, entry);
    let destination = "";
    if (method !== "DELETE") {
      if (typeof req.headers.destination !== "string") throw new AppError("destinationRequired");
      const target = new URL(req.headers.destination, origin);
      if (target.origin !== origin || target.search || target.hash) throw new AppError("invalidDestination");
      destination = logical(target.pathname);
    }
    req.resume(); await operation(method, { destination, expectedRevision: entry.revision, overwrite: req.headers.overwrite !== "F" });
    res.writeHead(method === "COPY" ? 201 : 204); res.end(); return;
  }
  throw new AppError("methodNotAllowed", 405);
}
