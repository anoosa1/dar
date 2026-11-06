import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import { connect, login, upload } from "./integration-helpers.ts";
import type { FileEntries } from "../client/module_bindings/types.ts";
const origin = process.env.DAR_INTEGRATION_ORIGIN;

test("immutable upload, ranges, revisions, versions, copy, cycles, trash and restore on real storage", { skip: !origin, timeout: 40_000 }, async () => {
  const base = origin!, session = await login(base), db = await connect(session);
  try {
    const name = `proof-${randomUUID()}.txt`;
    const first = await upload(base, session, name, "hello 🌍 from immutable storage");
    const contentUrl = `${base}/api/v1/files/${first.fileId}/content`;
    const content = await fetch(contentUrl, { headers: session.headers });
    assert.equal(await content.text(), "hello 🌍 from immutable storage");
    assert.match(content.headers.get("content-disposition")!, /attachment/);
    const partial = await fetch(contentUrl, { headers: { ...session.headers, range: "bytes=0-4" } });
    assert.equal(partial.status, 206); assert.equal(await partial.text(), "hello");
    assert.equal((await fetch(contentUrl, { method: "HEAD", headers: session.headers })).headers.get("content-length"), String(Buffer.byteLength("hello 🌍 from immutable storage")));
    const row = () => [...db.db.myFiles.iter()].find(item => item.id === first.fileId)!;
    assert.equal(row().revision, 1);
    await db.reducers.renameEntry({ id: first.fileId, name: `renamed-${name}`, expectedRevision: 1 });
    await assert.rejects(db.reducers.renameEntry({ id: first.fileId, name: "stale", expectedRevision: 1 }));
    const second = await upload(base, session, name, "replacement", "", first.fileId, 2);
    assert.equal(await (await fetch(contentUrl, { headers: session.headers })).text(), "replacement");
    await db.reducers.restoreVersion({ id: first.fileId, versionId: first.id, expectedRevision: 3 });
    assert.equal(await (await fetch(contentUrl, { headers: session.headers })).text(), "hello 🌍 from immutable storage");
    assert.equal(await (await fetch(`${contentUrl}?version=${second.id}`, { headers: session.headers })).text(), "replacement");
    const folderId = randomUUID();
    await db.reducers.createFolder({ id: folderId, parentId: "", name: `folder-${randomUUID()}` });
    await assert.rejects(db.reducers.moveEntry({ id: folderId, parentId: folderId, expectedRevision: 1 }));
    await db.reducers.copyEntry({ id: first.fileId, newId: randomUUID(), name: `copy-${name}`, parentId: folderId, expectedRevision: 4 });
    await db.reducers.trashEntry({ id: first.fileId, expectedRevision: 4 });
    assert.equal((await fetch(contentUrl, { headers: session.headers })).status, 404);
    await db.reducers.browse({ parentId: "", section: "trash", query: "", cursor: "" });
    assert.equal([...db.db.myFiles.iter()].find(item => item.id === first.fileId)?.trashed, true);
    await db.reducers.restoreEntry({ id: first.fileId, expectedRevision: 5 });
    assert.equal(await (await fetch(contentUrl, { headers: session.headers })).text(), "hello 🌍 from immutable storage");
    await db.reducers.trashEntry({ id: first.fileId, expectedRevision: 6 });
    await db.reducers.deleteEntry({ id: first.fileId, expectedRevision: 7 });
    assert.equal((await fetch(contentUrl, { headers: session.headers })).status, 404);
    const empty = await upload(base, session, `empty-${randomUUID()}.txt`, "");
    assert.equal((await fetch(`${base}/api/v1/files/${empty.fileId}/content`, { headers: session.headers })).headers.get("content-length"), "0");
    const unicode = await upload(base, session, `🌿 café-${randomUUID()}.txt`, "Unicode filenames stay visible.");
    await db.reducers.browse({ parentId: "", section: "files", query: "🌿", cursor: "" });
    assert.equal([...db.db.myFiles.iter()].some(entry => entry.id === unicode.fileId), true);
  } finally { db.disconnect(); }
});

test("user isolation, read-only grants, live revocation and public-link revocation", { skip: !origin, timeout: 40_000 }, async () => {
  const base = origin!, owner = await login(base), db = await connect(owner);
  let recipient: Awaited<ReturnType<typeof connect>> | undefined;
  try {
    const username = `user-${randomUUID().slice(0, 8)}`;
    const create = await fetch(`${base}/api/v1/auth/users`, { method: "POST", headers: { ...owner.headers, "content-type": "application/json" }, body: JSON.stringify({ username, displayName: "Sharing recipient", password: "a separate recipient password" }) });
    assert.equal(create.status, 201, await create.clone().text());
    const session = await login(base, username, "a separate recipient password");
    recipient = await connect(session);
    const file = await upload(base, owner, `sharing-${randomUUID()}.txt`, "private content");
    const url = `${base}/api/v1/files/${file.fileId}/content`;
    assert.equal((await fetch(url, { headers: session.headers })).status, 404);
    const grantId = randomUUID();
    await db.reducers.grantAccess({ id: grantId, fileId: file.fileId, username, role: "read", expectedRevision: 1 });
    await recipient.reducers.browse({ parentId: "", section: "shared", query: "", cursor: "" });
    assert.equal([...recipient.db.myFiles.iter()].some(item => item.id === file.fileId), true);
    assert.equal(await (await fetch(url, { headers: session.headers })).text(), "private content");
    await assert.rejects(recipient.reducers.renameEntry({ id: file.fileId, name: "not permitted", expectedRevision: 2 }));
    const removed = new Promise<void>(resolve => recipient!.db.myFiles.onDelete((_ctx, entry) => { if (entry.id === file.fileId) resolve(); }));
    await db.reducers.revokeGrant({ id: grantId }); await removed;
    const denied = JSON.parse(await recipient.procedures.fileDetails({ id: file.fileId }));
    assert.equal(denied.error.code, "notFound");
    assert.equal((await fetch(url, { headers: session.headers })).status, 404);
    const { createOpaqueToken } = await import("../runtime/auth/session.ts");
    const token = createOpaqueToken(), shareId = randomUUID();
    await db.reducers.createPublicShare({ id: shareId, fileId: file.fileId, digest: token.digest, expiresAt: Date.now() + 60_000, expectedRevision: 2 });
    const publicUrl = `${base}/public/files/${token.secret}`;
    assert.equal(await (await fetch(publicUrl)).text(), "private content");
    await db.reducers.revokePublicShare({ id: shareId });
    assert.equal((await fetch(publicUrl)).status, 404);
    assert.equal((await fetch(`${base}/api/v1/files/uploads`, { method: "POST", headers: { cookie: owner.headers.cookie, origin: "https://attacker.example", "content-type": "application/json" }, body: "{}" })).status, 403);
  } finally { db.disconnect(); recipient?.disconnect(); }
});
