import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import { login } from "./integration-helpers.ts";
const origin = process.env.DAR_INTEGRATION_ORIGIN;
test("scoped app passwords: WebDAV create, upload, list, conditional update, move, copy, delete and revocation", { skip: !origin, timeout: 30_000 }, async () => {
  const base = origin!, session = await login(base);
  async function credential(scope: string) {
    const response = await fetch(`${base}/api/v1/auth/app-passwords`, { method: "POST", headers: { ...session.headers, "content-type": "application/json" }, body: JSON.stringify({ label: "Protocol test", scope }) });
    assert.equal(response.status, 201, await response.clone().text());
    return await response.json() as { id: string; secret: string; davUrl: string };
  }
  const write = await credential("write"), read = await credential("read");
  const headers = { authorization: `Basic ${Buffer.from(`admin:${write.secret}`).toString("base64")}` };
  const readHeaders = { authorization: `Basic ${Buffer.from(`admin:${read.secret}`).toString("base64")}` };
  const folder = `${write.davUrl}Protocol-${randomUUID()}/`, file = `${folder}hello.txt`;
  assert.equal((await fetch(folder, { method: "MKCOL", headers })).status, 201);
  const initial = await fetch(file, { method: "PUT", headers, body: "version one" });
  assert.equal(initial.status, 201, await initial.clone().text());
  const etag = initial.headers.get("etag")!;
  const listing = await fetch(folder, { method: "PROPFIND", headers: { ...headers, depth: "1" } });
  assert.equal(listing.status, 207); assert.match(await listing.text(), /hello\.txt/);
  assert.equal(await (await fetch(file, { headers: readHeaders })).text(), "version one");
  assert.equal((await fetch(file, { method: "PUT", headers: readHeaders, body: "denied" })).status, 403);
  // Without If-Match a write replaces the file (its earlier content stays a version); with it, a stale ETag fails.
  const plain = await fetch(file, { method: "PUT", headers, body: "unconditional" });
  assert.equal(plain.status, 204, await plain.clone().text());
  assert.notEqual(plain.headers.get("etag"), etag);
  assert.equal((await fetch(file, { method: "PUT", headers: { ...headers, "if-match": etag }, body: "stale" })).status, 412);
  const update = await fetch(file, { method: "PUT", headers: { ...headers, "if-match": plain.headers.get("etag")! }, body: "version two" });
  assert.equal(update.status, 204, await update.clone().text());
  assert.equal((await fetch(file, { method: "HEAD", headers })).headers.get("etag"), update.headers.get("etag"));
  assert.equal((await fetch(file, { method: "PUT", headers: { ...headers, "if-none-match": "*" }, body: "exists" })).status, 412);
  const moved = `${folder}moved.txt`, copy = `${folder}copy.txt`;
  assert.equal((await fetch(file, { method: "MOVE", headers: { ...headers, "if-match": update.headers.get("etag")!, destination: moved } })).status, 201);
  const currentEtag = (await fetch(moved, { method: "HEAD", headers })).headers.get("etag")!;
  assert.equal((await fetch(moved, { method: "COPY", headers: { ...headers, "if-match": currentEtag, destination: copy } })).status, 201);
  assert.equal(await (await fetch(copy, { headers })).text(), "version two");
  assert.equal((await fetch(moved, { method: "COPY", headers: { ...headers, destination: copy, overwrite: "F" } })).status, 409);
  assert.equal((await fetch(moved, { method: "COPY", headers: { ...headers, destination: copy } })).status, 204);
  assert.equal((await fetch(moved, { method: "DELETE", headers: { ...headers, "if-match": currentEtag } })).status, 204);
  assert.equal((await fetch(moved, { headers })).status, 404);
  assert.equal((await fetch(copy, { method: "MOVE", headers: { ...headers, destination: moved } })).status, 201);
  assert.equal((await fetch(moved, { method: "DELETE", headers })).status, 204);
  const revoked = await fetch(`${base}/api/v1/auth/app-passwords/${write.id}`, { method: "DELETE", headers: session.headers });
  assert.equal(revoked.status, 200);
  assert.equal((await fetch(copy, { headers })).status, 401);
  assert.equal((await fetch(copy, { headers: { authorization: `Basic ${Buffer.from("admin:a long isolated test password").toString("base64")}` } })).status, 401);
});
test("WebDAV clients: parent folders, modified times, advisory locks and streamed uploads", { skip: !origin, timeout: 30_000 }, async () => {
  const base = origin!, session = await login(base);
  const response = await fetch(`${base}/api/v1/auth/app-passwords`, { method: "POST", headers: { ...session.headers, "content-type": "application/json" }, body: JSON.stringify({ label: "Client test", scope: "write" }) });
  const { secret, davUrl } = await response.json() as { secret: string; davUrl: string };
  const headers = { authorization: `Basic ${Buffer.from(`admin:${secret}`).toString("base64")}` };
  const folder = `${davUrl}Clients-${randomUUID()}/`;
  const options = await fetch(davUrl, { method: "OPTIONS" });
  assert.equal(options.headers.get("dav"), "1, 2");
  assert.match(options.headers.get("allow")!, /LOCK, UNLOCK/);
  // A missing parent is 409, which clients answer by creating it; an existing folder is 405.
  assert.equal((await fetch(`${folder}nested/`, { method: "MKCOL", headers })).status, 409);
  assert.equal((await fetch(`${folder}nested/file.txt`, { method: "PUT", headers, body: "x" })).status, 409);
  assert.equal((await fetch(folder, { method: "MKCOL", headers })).status, 201);
  assert.equal((await fetch(folder, { method: "MKCOL", headers })).status, 405);
  const modified = async (url: string) => {
    const listing = await (await fetch(url, { method: "PROPFIND", headers: { ...headers, depth: "0" } })).text();
    return Date.parse(/<d:getlastmodified>([^<]+)</.exec(listing)![1]);
  };
  // rclone, ownCloud and Nextcloud send the file's time with its content.
  const dated = `${folder}dated.txt`, time = Date.UTC(2024, 0, 2, 3, 4, 5);
  const put = await fetch(dated, { method: "PUT", headers: { ...headers, "x-oc-mtime": String(time / 1000) }, body: "dated" });
  assert.equal(put.status, 201); assert.equal(put.headers.get("x-oc-mtime"), "accepted");
  assert.equal(await modified(dated), time);
  // A generic type gives way to the type of the name.
  assert.equal((await fetch(`${folder}notes.md`, { method: "PUT", headers: { ...headers, "content-type": "application/octet-stream" }, body: "# Notes" })).status, 201);
  assert.match(await (await fetch(`${folder}notes.md`, { method: "PROPFIND", headers: { ...headers, depth: "0" } })).text(), /<d:getcontenttype>text\/markdown</);
  const proppatch = (body: string) => fetch(dated, { method: "PROPPATCH", headers: { ...headers, "content-type": "application/xml" }, body: `<?xml version="1.0"?><d:propertyupdate xmlns:d="DAV:" xmlns:z="urn:schemas-microsoft-com:"><d:set><d:prop>${body}</d:prop></d:set></d:propertyupdate>` });
  let patched = await proppatch("<d:lastmodified>1600000000</d:lastmodified>");
  assert.equal(patched.status, 207); assert.match(await patched.text(), /lastmodified[^]*HTTP\/1.1 200 OK/);
  assert.equal(await modified(dated), 1600000000_000);
  // Windows sets its file times and attributes after each save.
  patched = await proppatch("<z:Win32LastModifiedTime>Wed, 02 Mar 2022 10:00:00 GMT</z:Win32LastModifiedTime><z:Win32FileAttributes>00000020</z:Win32FileAttributes>");
  assert.doesNotMatch(await patched.text(), /403|424/);
  assert.equal(await modified(dated), Date.UTC(2022, 2, 2, 10));
  // Properties apply together: an unsupported one fails the request and changes nothing.
  patched = await proppatch("<d:lastmodified>1500000000</d:lastmodified><d:author>someone</d:author>");
  const failed = await patched.text();
  assert.match(failed, /author[^]*403 Forbidden/); assert.match(failed, /lastmodified[^]*424 Failed Dependency/);
  assert.equal(await modified(dated), Date.UTC(2022, 2, 2, 10));
  // Locks are advisory: Finder and Windows mount writable, and a lock on a new name creates an empty file.
  const lockBody = `<?xml version="1.0"?><d:lockinfo xmlns:d="DAV:"><d:lockscope><d:exclusive/></d:lockscope><d:locktype><d:write/></d:locktype><d:owner><d:href>test</d:href></d:owner></d:lockinfo>`;
  const lock = await fetch(dated, { method: "LOCK", headers: { ...headers, timeout: "Second-600" }, body: lockBody });
  assert.equal(lock.status, 200);
  const token = lock.headers.get("lock-token")!;
  assert.match(token, /^<opaquelocktoken:[0-9a-f-]+>$/);
  assert.match(await lock.text(), /<d:timeout>Second-600<\/d:timeout>/);
  const refresh = await fetch(dated, { method: "LOCK", headers: { ...headers, if: `(${token})` } });
  assert.equal(refresh.status, 200); assert.equal(refresh.headers.get("lock-token"), token);
  assert.equal((await fetch(dated, { method: "PUT", headers: { ...headers, if: `(${token})` }, body: "locked write" })).status, 204);
  assert.equal((await fetch(dated, { method: "UNLOCK", headers: { ...headers, "lock-token": token } })).status, 204);
  const placeholder = `${folder}placeholder.docx`;
  assert.equal((await fetch(placeholder, { method: "LOCK", headers, body: lockBody })).status, 201);
  const empty = await fetch(placeholder, { headers });
  assert.equal(empty.status, 200); assert.equal(await empty.text(), "");
  assert.match((await (await fetch(folder, { method: "PROPFIND", headers: { ...headers, depth: "1" } })).text()), /<d:supportedlock>/);
  // Finder streams uploads in chunks and states the size in X-Expected-Entity-Length.
  const streamed = `${folder}streamed.bin`, content = Buffer.alloc(200_000, 7);
  const body = new ReadableStream({ start(controller) { controller.enqueue(content.subarray(0, 100_000)); controller.enqueue(content.subarray(100_000)); controller.close(); } });
  const chunked = await fetch(streamed, { method: "PUT", headers: { ...headers, "x-expected-entity-length": String(content.length) }, body, duplex: "half" } as RequestInit);
  assert.equal(chunked.status, 201, await chunked.clone().text());
  assert.deepEqual(Buffer.from(await (await fetch(streamed, { headers })).arrayBuffer()), content);
  const unsized = new ReadableStream({ start(controller) { controller.enqueue(new Uint8Array(3)); controller.close(); } });
  assert.equal((await fetch(`${folder}unsized.bin`, { method: "PUT", headers, body: unsized, duplex: "half" } as RequestInit)).status, 411);
  assert.equal((await fetch(folder, { method: "DELETE", headers })).status, 204);
});
