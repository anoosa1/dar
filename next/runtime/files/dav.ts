import type { IncomingMessage, ServerResponse } from "node:http";
import { randomUUID } from "node:crypto";
import type { Database } from "../database.ts";
import { digestToken } from "../auth/session.ts";
import { AppError } from "../errors.ts";
import { xml, children, readBody, type Xml } from "../dav-xml.ts";
import type { blobStore, BlobFile } from "./blobs.ts";
import { mediaTypeOf } from "./media-types.ts";
import { maxFileBytes } from "../../shared/limits.ts";

/** WebDAV verbs the database answers without changing anything. */
const davReads: ReadonlySet<string> = new Set(["STAT", "PROPFIND", "PREPARE"]);
const D = "DAV:", microsoft = "urn:schemas-microsoft-com:", owncloud = "http://owncloud.org/ns";
/**
 * Locks are advisory: LOCK always grants a token and writes do not check it. Finder and Windows
 * mount a server read-only unless it locks; a write that races another still keeps the earlier
 * content as a version.
 */
const lockSeconds = 3600;

interface Entry extends BlobFile { kind: string; modifiedAt: number }
interface Listed extends Entry { sha1: string; md5: string }
interface Target { entry?: Entry; parentId: string; name: string }
const escape = (value: string) => value.replace(/[<>&"']/g, c => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;", '"': "&quot;", "'": "&apos;" })[c]!);
const etag = (entry: Entry) => `"${entry.id}:${entry.revision}:${entry.blobId}"`;
/** If-Match is optional: without it a write replaces the current file, whose earlier content stays a version. */
function condition(req: IncomingMessage, entry: Entry) {
  const match = req.headers["if-match"];
  if (match === undefined || match.trim() === "*") return;
  if (!match.split(",").some(value => value.trim() === etag(entry))) throw new AppError("revisionConflict", 412);
}
const lockProperties = `<d:supportedlock>${["exclusive", "shared"].map(scope => `<d:lockentry><d:lockscope><d:${scope}/></d:lockscope><d:locktype><d:write/></d:locktype></d:lockentry>`).join("")}</d:supportedlock><d:lockdiscovery/>`;
/** ownCloud's checksum property, which rclone and the ownCloud and Nextcloud clients read (SHA-1 and MD5 once runtime/files/checksums.ts has them). */
function checksums(entry: Listed) {
  if (entry.kind !== "file") return "";
  return `<oc:checksums><oc:checksum>${entry.sha1 ? `SHA1:${entry.sha1} MD5:${entry.md5} ` : ""}SHA256:${escape(entry.blobId)}</oc:checksum></oc:checksums>`;
}
function propertyResponse(entry: Listed, href: string) {
  return `<d:response><d:href>${escape(href)}</d:href><d:propstat><d:prop><d:displayname>${escape(entry.name)}</d:displayname><d:resourcetype>${entry.kind === "folder" ? "<d:collection/>" : ""}</d:resourcetype><d:getcontentlength>${entry.size}</d:getcontentlength><d:getcontenttype>${escape(entry.mediaType)}</d:getcontenttype><d:getetag>${escape(etag(entry))}</d:getetag><d:getlastmodified>${new Date(entry.modifiedAt).toUTCString()}</d:getlastmodified><d:creationdate>${new Date(entry.modifiedAt).toISOString()}</d:creationdate>${checksums(entry)}${lockProperties}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>`;
}
/** Modified-time properties clients set: DAV:lastmodified (Unix seconds; rclone, ownCloud), getlastmodified and Win32LastModifiedTime (HTTP dates). */
function patch(node: Xml, set: boolean): { status: number; time?: number } {
  const text = node.text.trim();
  if (set && node.uri === D && node.name === "lastmodified" && /^\d{1,12}(\.\d+)?$/.test(text)) return { status: 200, time: Number(text) * 1000 };
  if (set && (node.uri === D && node.name === "getlastmodified" || node.uri === microsoft && node.name === "Win32LastModifiedTime")) {
    const time = Date.parse(text); if (Number.isFinite(time)) return { status: 200, time };
  }
  // Windows sets these on every save; they describe its local copy and change nothing here.
  if (node.uri === microsoft && ["Win32CreationTime", "Win32LastAccessTime", "Win32FileAttributes"].includes(node.name)) return { status: 200 };
  // rclone sends back the checksums it read with each time it sets; Dar computes them from the content.
  if (node.uri === owncloud && node.name === "checksums") return { status: 200 };
  return { status: 403 };
}
const reasons: Record<number, string> = { 200: "OK", 403: "Forbidden", 424: "Failed Dependency" };
/** The client's media type, or one from the name when the client sends none or a generic one. */
function mediaType(req: IncomingMessage, name: string) {
  const declared = req.headers["content-type"]?.split(";")[0].trim().toLowerCase();
  return declared && declared !== "application/octet-stream" && declared.length <= 150 && /^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/.test(declared) ? declared : mediaTypeOf(name);
}
async function* nothing(): AsyncIterable<Buffer> {}

/** Standards paths expose logical Files resources; they never map to disk paths. */
/** `stored` runs after new content is recorded (runtime/files/checksums.ts hashes it). */
export async function handleDav(req: IncomingMessage, res: ServerResponse, db: Database, blobs: ReturnType<typeof blobStore>, origin: string, stored: () => void = () => {}) {
  const method = req.method ?? "GET";
  if (req.headers.origin && req.headers.origin !== origin) throw new AppError("invalidOrigin", 403);
  res.setHeader("allow", "OPTIONS, PROPFIND, PROPPATCH, GET, HEAD, PUT, MKCOL, DELETE, MOVE, COPY, LOCK, UNLOCK");
  res.setHeader("dav", "1, 2");
  res.setHeader("ms-author-via", "DAV");
  res.setHeader("cache-control", "no-store");
  if (method === "OPTIONS") { res.writeHead(204); res.end(); return; }
  const authorization = req.headers.authorization;
  if (!authorization?.startsWith("Basic ") || authorization.length > 2048) { res.setHeader("www-authenticate", 'Basic realm="Dar Files", charset="UTF-8"'); throw new AppError("unauthenticated", 401); }
  const decoded = Buffer.from(authorization.slice(6), "base64").toString("utf8"), separator = decoded.indexOf(":");
  if (separator < 1) throw new AppError("unauthenticated", 401);
  const username = decoded.slice(0, separator), secret = decoded.slice(separator + 1);
  if (!/^[A-Za-z0-9_-]{43}$/.test(secret)) throw new AppError("unauthenticated", 401);
  const authenticated = await db.read<{ sessionId: string; username: string }>("service_protocol_authenticate", [username, digestToken(secret), !["GET", "HEAD", "PROPFIND"].includes(method)]);
  const url = new URL(req.url!, origin), prefix = `/dav/files/${encodeURIComponent(authenticated.username)}`;
  if (url.pathname !== prefix && !url.pathname.startsWith(`${prefix}/`)) throw new AppError("forbidden", 403);
  function logical(pathname: string) {
    if (pathname !== prefix && !pathname.startsWith(`${prefix}/`)) throw new AppError("forbidden", 403);
    const pieces = pathname.slice(prefix.length).split("/").filter(Boolean).map(piece => decodeURIComponent(piece));
    if (pieces.some(piece => /[\x00-\x1f/\\]/.test(piece) || piece === "." || piece === "..")) throw new AppError("invalidPath");
    return pieces.join("/");
  }
  const pathname = logical(url.pathname);
  const operation = <T>(verb: string, extra: { destination?: string; expectedRevision?: number; overwrite?: boolean; depth?: number; modifiedAt?: number } = {}) => db.operation<T>("service_dav", verb, davReads, [authenticated.sessionId, verb, pathname, extra.destination ?? "", extra.expectedRevision ?? 0, extra.overwrite ?? false, extra.depth ?? 0, extra.modifiedAt ?? 0]);
  /** Stores content as the target's next version and returns its ETag. */
  async function store(target: Target, source: AsyncIterable<Buffer>, size: number, type: string, fileTime: number) {
    if (!Number.isSafeInteger(size) || size < 0 || size > maxFileBytes) throw new AppError("uploadTooLarge", 413);
    const id = randomUUID(), fileId = target.entry?.id ?? randomUUID();
    await db.call("service_begin_upload", [authenticated.sessionId, id, fileId, target.parentId, target.name, size, type, target.entry?.revision ?? 0]);
    let digest: string;
    try {
      digest = await blobs.receive(source, id, size);
      await db.call("service_finish_upload", [authenticated.sessionId, id, digest, fileTime]);
    } catch (error) { await db.call("service_cancel_upload", [authenticated.sessionId, id]).catch(() => {}); throw error; }
    stored();
    // Finishing gives the file the next revision, so its ETag is known without another read.
    return `"${fileId}:${(target.entry?.revision ?? 0) + 1}:${digest}"`;
  }
  if (method === "PROPFIND") {
    // No Depth means infinity (RFC 4918); the module takes 2 for it.
    const header = String(req.headers.depth ?? "infinity").toLowerCase(), depth = ({ "0": 0, "1": 1, infinity: 2 } as Record<string, number>)[header];
    if (depth === undefined) throw new AppError("invalidDepth");
    if (Number(req.headers["content-length"] ?? 0) > 16_384) throw new AppError("requestTooLarge", 413);
    req.resume();
    let value: { root: Listed; children: (Listed & { path: string })[] };
    try { value = await operation("PROPFIND", { depth }); }
    catch (error) {
      // A tree past the listing limit is refused as RFC 4918 asks; clients then list folder by folder.
      if (!(depth === 2 && error instanceof AppError && error.code === "operationTooLarge")) throw error;
      res.writeHead(403, { "content-type": "application/xml; charset=utf-8" });
      res.end(`<?xml version="1.0" encoding="UTF-8"?><d:error xmlns:d="DAV:"><d:propfind-finite-depth/></d:error>`); return;
    }
    const encode = (path: string) => path.split("/").map(encodeURIComponent).join("/");
    const href = `${prefix}${pathname ? `/${encode(pathname)}` : ""}${value.root.kind === "folder" ? "/" : ""}`;
    const xml = `<?xml version="1.0" encoding="UTF-8"?><d:multistatus xmlns:d="DAV:" xmlns:oc="${owncloud}">${propertyResponse(value.root, href)}${value.children.map(child => propertyResponse(child, `${href}${encode(child.path)}${child.kind === "folder" ? "/" : ""}`)).join("")}</d:multistatus>`;
    res.writeHead(207, { "content-type": "application/xml; charset=utf-8" }); res.end(xml); return;
  }
  if (method === "PROPPATCH") {
    const entry = await operation<Entry>("STAT"); condition(req, entry);
    const body = xml(await readBody(req, 16_384));
    if (body.uri !== D || body.name !== "propertyupdate") throw new AppError("invalidXml");
    const results: { node: Xml; status: number; time?: number }[] = [];
    for (const action of body.children) if (action.uri === D && (action.name === "set" || action.name === "remove"))
      for (const prop of children(action, D, "prop")) for (const node of prop.children) results.push({ node, ...patch(node, action.name === "set") });
    // The root folder has no record to date; properties apply all together or not at all.
    if (!entry.id) for (const result of results) if (result.time !== undefined) result.status = 403;
    const failed = results.some(result => result.status !== 200), time = results.findLast(result => result.time !== undefined)?.time;
    if (!failed && time !== undefined) await operation("TOUCH", { modifiedAt: time });
    const groups = new Map<number, Xml[]>();
    for (const result of results) { const status = failed && result.status === 200 ? 424 : result.status; groups.set(status, [...groups.get(status) ?? [], result.node]); }
    const element = (node: Xml) => node.uri ? `<x:${node.name} xmlns:x="${escape(node.uri)}"/>` : `<${node.name} xmlns=""/>`;
    const propstats = [...groups].map(([status, nodes]) => `<d:propstat><d:prop>${nodes.map(element).join("")}</d:prop><d:status>HTTP/1.1 ${status} ${reasons[status]}</d:status></d:propstat>`).join("");
    res.writeHead(207, { "content-type": "application/xml; charset=utf-8" });
    res.end(`<?xml version="1.0" encoding="UTF-8"?><d:multistatus xmlns:d="DAV:"><d:response><d:href>${escape(url.pathname)}</d:href>${propstats}</d:response></d:multistatus>`); return;
  }
  if (method === "LOCK") {
    const text = await readBody(req, 16_384), refresh = /<(opaquelocktoken:[^>\s]+)>/.exec(String(req.headers.if ?? ""))?.[1];
    let shared = false;
    if (text.trim()) {
      const body = xml(text);
      if (body.uri !== D || body.name !== "lockinfo") throw new AppError("invalidXml");
      shared = children(children(body, D, "lockscope")[0], D, "shared").length > 0;
    } else if (!refresh) throw new AppError("invalidXml");
    let created = false;
    if (pathname) {
      // Locking an unmapped URL creates an empty file (RFC 4918, section 9.10.4).
      const target = await operation<Target>("PREPARE");
      if (!target.entry) { await store(target, nothing(), 0, mediaTypeOf(target.name), 0); created = true; }
    }
    const requested = Number(/Second-(\d+)/i.exec(String(req.headers.timeout ?? ""))?.[1] ?? lockSeconds), token = refresh ?? `opaquelocktoken:${randomUUID()}`;
    res.writeHead(created ? 201 : 200, { "content-type": "application/xml; charset=utf-8", "lock-token": `<${token}>` });
    res.end(`<?xml version="1.0" encoding="UTF-8"?><d:prop xmlns:d="DAV:"><d:lockdiscovery><d:activelock><d:locktype><d:write/></d:locktype><d:lockscope><d:${shared ? "shared" : "exclusive"}/></d:lockscope><d:depth>${req.headers.depth === "0" ? "0" : "infinity"}</d:depth><d:timeout>Second-${Math.min(Math.max(requested, 1), lockSeconds)}</d:timeout><d:locktoken><d:href>${escape(token)}</d:href></d:locktoken><d:lockroot><d:href>${escape(url.pathname)}</d:href></d:lockroot></d:activelock></d:lockdiscovery></d:prop>`);
    return;
  }
  if (method === "UNLOCK") {
    if (!req.headers["lock-token"]) throw new AppError("lockTokenRequired");
    req.resume(); await operation("STAT"); res.writeHead(204); res.end(); return;
  }
  if (method === "GET" || method === "HEAD") {
    const entry = await operation<Entry>("STAT");
    if (entry.kind !== "file") throw new AppError("isDirectory", 405);
    const file = await db.read<BlobFile>("service_download", [authenticated.sessionId, entry.id, ""]);
    return await blobs.download(req, res, file);
  }
  if (method === "MKCOL") {
    if (Number(req.headers["content-length"] ?? 0)) throw new AppError("unsupportedBody", 415);
    req.resume();
    // An existing resource answers 405, which clients take as "already there". rclone asks before
    // every upload, so a read answers that without waiting for a durable write.
    const exists = await operation("STAT").then(() => true, error => { if (error instanceof AppError && error.code === "notFound") return false; throw error; });
    if (exists || !(await operation<{ created: boolean }>("MKCOL")).created) throw new AppError("alreadyExists", 405);
    res.writeHead(201); res.end(); return;
  }
  if (method === "PUT") {
    const target = await operation<Target>("PREPARE");
    if (target.entry) {
      if (target.entry.kind !== "file") throw new AppError("isDirectory", 405);
      if (req.headers["if-none-match"] === "*") throw new AppError("revisionConflict", 412);
      condition(req, target.entry);
    }
    // Finder streams uploads without Content-Length and states their size in X-Expected-Entity-Length.
    const length = req.headers["content-length"] ?? req.headers["x-expected-entity-length"];
    if (typeof length !== "string") throw new AppError("lengthRequired", 411);
    if (!/^\d{1,16}$/.test(length)) throw new AppError("invalidInput");
    // rclone, ownCloud and Nextcloud clients send the file's modified time in Unix seconds.
    const mtime = req.headers["x-oc-mtime"], fileTime = typeof mtime === "string" && /^\d{1,12}(\.\d+)?$/.test(mtime) ? Number(mtime) * 1000 : 0;
    const tag = await store(target, req, Number(length), mediaType(req, target.name), fileTime);
    if (fileTime) res.setHeader("x-oc-mtime", "accepted");
    res.setHeader("etag", tag); res.writeHead(target.entry ? 204 : 201); res.end(); return;
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
    req.resume();
    const result = await operation<{ replaced?: boolean }>(method, { destination, expectedRevision: entry.revision, overwrite: req.headers.overwrite !== "F" });
    res.writeHead(method === "DELETE" || result.replaced ? 204 : 201); res.end(); return;
  }
  throw new AppError("methodNotAllowed", 405);
}
