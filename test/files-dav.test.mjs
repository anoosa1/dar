import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { startDarWithOidc, waitForHealth, loginSession, stopDarWithOidc } from "./helpers/dar-server.mjs";

// WebDAV files: scoped Basic app passwords, PROPFIND depth, conditional
// PUT/GET, ranges, and copy/move under /dav/files/{username}.
test("files webdav serves protocol clients with app passwords", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-files-dav-test-");
  const context = await startDarWithOidc(dataRoot);
  try {
    await waitForHealth(context.port, context.running.child, context.running.output);
    const session = await loginSession(context.baseUrl, context.running.output);
    const base = context.baseUrl;

    const created = await fetch(`${base}/api/v1/app-passwords`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ label: "Dav", scopes: ["files"] }),
    });
    assert.equal(created.status, 201, context.running.output());
    const { token } = await created.json();
    const auth = `Basic ${Buffer.from(`user-123:${token}`).toString("base64")}`;

    const scoped = await fetch(`${base}/api/v1/app-passwords`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ label: "Calendar only", scopes: ["calendar"] }),
    });
    assert.equal(scoped.status, 201, context.running.output());
    const calendarOnly = `Basic ${Buffer.from(`user-123:${(await scoped.json()).token}`).toString("base64")}`;

    const dav = async (path, options = {}) =>
      fetch(`${base}${path}`, { ...options, headers: { authorization: auth, ...(options.headers ?? {}) } });

    // Anonymous and unauthorized clients get the Basic challenge.
    const anonymous = await fetch(`${base}/dav/files/user-123/`);
    assert.equal(anonymous.status, 401);
    assert.match(anonymous.headers.get("www-authenticate"), /^Basic/);

    const wrongToken = await fetch(`${base}/dav/files/user-123/`, {
      headers: { authorization: `Basic ${Buffer.from("user-123:wrong").toString("base64")}` },
    });
    assert.equal(wrongToken.status, 401);

    // Credentials for one subject never authorize another subject's URL.
    const wrongSubject = await fetch(`${base}/dav/files/someone-else/`, {
      headers: { authorization: auth },
    });
    assert.equal(wrongSubject.status, 401);

    const forbidden = await fetch(`${base}/dav/files/user-123/`, {
      method: "PROPFIND",
      headers: { authorization: calendarOnly, depth: "0" },
    });
    assert.equal(forbidden.status, 403, context.running.output());

    // MKCOL creates collections; recreating one conflicts.
    const mkcol = await dav("/dav/files/user-123/Documents", { method: "MKCOL" });
    assert.equal(mkcol.status, 201, context.running.output());
    const mkcolAgain = await dav("/dav/files/user-123/Documents", { method: "MKCOL" });
    assert.equal(mkcolAgain.status, 409, context.running.output());

    // PUT creates then updates; If-None-Match guards creation.
    const put = await dav("/dav/files/user-123/Documents/dav.txt", {
      method: "PUT",
      body: "dav bytes",
    });
    assert.equal(put.status, 201, context.running.output());
    const putOverwrite = await dav("/dav/files/user-123/Documents/dav.txt", {
      method: "PUT",
      body: "dav bytes two",
    });
    assert.equal(putOverwrite.status, 204, context.running.output());
    const putIfNoneMatch = await dav("/dav/files/user-123/Documents/dav.txt", {
      method: "PUT",
      headers: { "if-none-match": "*" },
      body: "nope",
    });
    assert.equal(putIfNoneMatch.status, 412, context.running.output());

    // GET returns the bytes, an ETag, and honors ranges.
    const get = await dav("/dav/files/user-123/Documents/dav.txt");
    assert.equal(get.status, 200, context.running.output());
    assert.equal(await get.text(), "dav bytes two");
    assert.match(get.headers.get("etag"), /^"[0-9a-f-]+"$/);
    assert.equal(get.headers.get("accept-ranges"), "bytes");

    const notModified = await dav("/dav/files/user-123/Documents/dav.txt", {
      headers: { "if-none-match": get.headers.get("etag") },
    });
    assert.equal(notModified.status, 304, context.running.output());

    const range = await dav("/dav/files/user-123/Documents/dav.txt", {
      headers: { range: "bytes=4-7" },
    });
    assert.equal(range.status, 206, context.running.output());
    assert.equal(await range.text(), "byte");
    assert.equal(range.headers.get("content-range"), "bytes 4-7/13");

    const head = await dav("/dav/files/user-123/Documents/dav.txt", { method: "HEAD" });
    assert.equal(head.status, 200, context.running.output());
    assert.equal(await head.text(), "");

    // PROPFIND reports the target at depth 0 and children at depth 1.
    const propfind0 = await dav("/dav/files/user-123/Documents", {
      method: "PROPFIND",
      headers: { depth: "0" },
    });
    assert.equal(propfind0.status, 207, context.running.output());
    const depth0Xml = await propfind0.text();
    assert.match(depth0Xml, /<d:multistatus xmlns:d="DAV:">/);
    assert.match(depth0Xml, /<d:resourcetype><d:collection\/><\/d:resourcetype>/);
    assert.ok(!depth0Xml.includes("dav.txt"), "depth 0 must not list children");

    const propfind1 = await dav("/dav/files/user-123/Documents", {
      method: "PROPFIND",
      headers: { depth: "1" },
    });
    assert.equal(propfind1.status, 207, context.running.output());
    const depth1Xml = await propfind1.text();
    assert.match(depth1Xml, /dav\.txt/);
    assert.match(depth1Xml, /<d:getcontentlength>13<\/d:getcontentlength>/);
    assert.match(depth1Xml, /<d:getlastmodified>/);

    // COPY duplicates, MOVE relocates; both accept a full Destination URL.
    const copy = await dav("/dav/files/user-123/Documents/dav.txt", {
      method: "COPY",
      headers: { destination: `${base}/dav/files/user-123/Documents/dav-copy.txt` },
    });
    assert.equal(copy.status, 201, context.running.output());
    const copyGet = await dav("/dav/files/user-123/Documents/dav-copy.txt");
    assert.equal(copyGet.status, 200, context.running.output());
    assert.equal(await copyGet.text(), "dav bytes two");

    const move = await dav("/dav/files/user-123/Documents/dav-copy.txt", {
      method: "MOVE",
      headers: { destination: `${base}/dav/files/user-123/Documents/dav-moved.txt` },
    });
    assert.equal(move.status, 201, context.running.output());
    const oldLocation = await dav("/dav/files/user-123/Documents/dav-copy.txt");
    assert.equal(oldLocation.status, 404, context.running.output());

    // LOCK blocks writes; UNLOCK with the derived token releases them.
    const lock = await dav("/dav/files/user-123/Documents/dav.txt", { method: "LOCK" });
    assert.equal(lock.status, 200, context.running.output());
    assert.match(await lock.text(), /<d:lockdiscovery>/);
    const lockTokenHeader = lock.headers.get("lock-token");
    assert.match(lockTokenHeader, /^<opaquelocktoken:dar-[0-9a-f]{64}>$/);

    const lockedPut = await dav("/dav/files/user-123/Documents/dav.txt", {
      method: "PUT",
      body: "blocked",
    });
    assert.equal(lockedPut.status, 423, context.running.output());

    const unlockWrong = await dav("/dav/files/user-123/Documents/dav.txt", {
      method: "UNLOCK",
      headers: { "lock-token": "<opaquelocktoken:dar-wrong>" },
    });
    assert.equal(unlockWrong.status, 409, context.running.output());

    const unlock = await dav("/dav/files/user-123/Documents/dav.txt", {
      method: "UNLOCK",
      headers: { "lock-token": lockTokenHeader },
    });
    assert.equal(unlock.status, 204, context.running.output());

    const unlockedPut = await dav("/dav/files/user-123/Documents/dav.txt", {
      method: "PUT",
      body: "dav bytes three",
    });
    assert.equal(unlockedPut.status, 204, context.running.output());

    // DELETE removes the file for protocol clients too.
    const del = await dav("/dav/files/user-123/Documents/dav.txt", { method: "DELETE" });
    assert.equal(del.status, 204, context.running.output());
    const gone = await dav("/dav/files/user-123/Documents/dav.txt");
    assert.equal(gone.status, 404, context.running.output());
  } finally {
    await stopDarWithOidc(context);
    await rm(dataRoot, { recursive: true, force: true });
  }
});
