import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import test from "node:test";

import sharp from "sharp";

import {
  loginSession,
  startDarWithOidc,
  stopDarWithOidc,
  waitForHealth,
} from "./helpers/dar-server.mjs";

test("files editor, versions, tags, comments, and locks work over http", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-files-editor-test-");
  const context = await startDarWithOidc(dataRoot);
  try {
    await waitForHealth(context.port, context.running.child, context.running.output);
    const session = await loginSession(context.baseUrl, context.running.output);
    const base = context.baseUrl;
    const docPath = "/Documents/doc.md";
    const editorPath = "/api/v1/files/editor?path=%2FDocuments%2Fdoc.md";
    const get = (path) => fetch(`${base}${path}`, { headers: { cookie: session.cookies } });

    const seed = await fetch(`${base}/api/v1/files/upload?path=${encodeURIComponent(docPath)}`, {
      method: "POST",
      headers: {
        cookie: session.cookies,
        origin: base,
        "x-csrf-token": session.csrfToken,
      },
      body: "first draft",
    });
    assert.equal(seed.status, 201, context.running.output());

    // Editor GET returns the camelCase document with etag headers.
    const read = await get(editorPath);
    assert.equal(read.status, 200, context.running.output());
    assert.equal(read.headers.get("cache-control"), "no-store");
    const doc = await read.json();
    assert.equal(doc.path, docPath);
    assert.equal(doc.name, "doc.md");
    assert.equal(doc.content, "first draft");
    assert.equal(doc.markdown, true);
    assert.equal(doc.mimeType, "text/markdown");
    assert.equal(doc.size, 11);
    assert.match(doc.etag, /^[0-9a-f]{64}$/);
    assert.equal(read.headers.get("etag"), `"${doc.etag}"`);

    // Saving without If-Match is a 428; a stale matcher is a 412.
    const missingMatch = await fetch(`${base}${editorPath}`, {
      method: "PUT",
      headers: session.authHeaders,
      body: JSON.stringify({ content: "second draft" }),
    });
    assert.equal(missingMatch.status, 428, context.running.output());
    assert.equal((await missingMatch.json()).error.code, "preconditionRequired");

    const staleMatch = await fetch(`${base}${editorPath}`, {
      method: "PUT",
      headers: { ...session.authHeaders, "if-match": `"deadbeef${"0".repeat(56)}"` },
      body: JSON.stringify({ content: "second draft" }),
    });
    assert.equal(staleMatch.status, 412, context.running.output());
    assert.equal((await staleMatch.json()).error.code, "preconditionFailed");

    // A matching save versions the replaced content and returns the new doc.
    const saved = await fetch(`${base}${editorPath}`, {
      method: "PUT",
      headers: { ...session.authHeaders, "if-match": `"${doc.etag}"` },
      body: JSON.stringify({ content: "second draft" }),
    });
    assert.equal(saved.status, 200, context.running.output());
    const savedDoc = await saved.json();
    assert.equal(savedDoc.content, "second draft");
    assert.equal(savedDoc.size, 12);
    assert.notEqual(savedDoc.etag, doc.etag);

    // The replaced content is readable as a version and restorable.
    const versions = await get("/api/v1/files/versions?path=%2FDocuments%2Fdoc.md");
    assert.equal(versions.status, 200, context.running.output());
    const versionBody = await versions.json();
    assert.equal(versionBody.nextCursor, null);
    assert.equal(versionBody.items.length, 1);
    const version = versionBody.items[0];
    assert.ok(version.id);
    assert.equal(version.size, 11);
    assert.ok(version.createdAt);

    const versionRead = await get(
      `/api/v1/files/versions/${version.id}/content?path=%2FDocuments%2Fdoc.md`,
    );
    assert.equal(versionRead.status, 200, context.running.output());
    assert.equal(versionRead.headers.get("content-type"), "application/octet-stream");
    assert.equal(await versionRead.text(), "first draft");

    const restore = await fetch(
      `${base}/api/v1/files/versions/${version.id}/restore`,
      {
        method: "POST",
        headers: session.authHeaders,
        body: JSON.stringify({ path: docPath }),
      },
    );
    assert.equal(restore.status, 200, context.running.output());
    assert.deepEqual(await restore.json(), { restored: docPath });

    const restoredDoc = await get(editorPath);
    assert.equal((await restoredDoc.json()).content, "first draft");
    // The restore changed the content, so later saves must match this etag.
    const restoredEtag = restoredDoc.headers.get("etag");
    const versionsAfterRestore = await get("/api/v1/files/versions?path=%2FDocuments%2Fdoc.md");
    // Restoring snapshots the pre-restore content first.
    assert.equal((await versionsAfterRestore.json()).items.length, 2);

    // Tags: create, attach to a file, list, cascade delete.
    const tagCreated = await fetch(`${base}/api/v1/files/tags`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ name: "important", color: "#ff0000" }),
    });
    assert.equal(tagCreated.status, 201, context.running.output());
    const tag = await tagCreated.json();
    assert.ok(tag.id);
    assert.equal(tag.name, "important");
    assert.equal(tag.color, "#ff0000");

    const tagApplied = await fetch(`${base}/api/v1/files/file-tags`, {
      method: "PUT",
      headers: session.authHeaders,
      body: JSON.stringify({ path: docPath, tagId: tag.id, added: true }),
    });
    assert.equal(tagApplied.status, 200, context.running.output());
    assert.deepEqual(await tagApplied.json(), {
      path: docPath,
      tag: tag.id,
      added: true,
    });

    const taggedListing = await get("/api/v1/files?path=%2FDocuments");
    const taggedItem = (await taggedListing.json()).items.find((item) => item.path === docPath);
    assert.deepEqual(taggedItem.tags, [tag.id]);

    const tagDeleted = await fetch(`${base}/api/v1/files/tags/${tag.id}`, {
      method: "DELETE",
      headers: session.authHeaders,
    });
    assert.equal(tagDeleted.status, 200, context.running.output());
    assert.deepEqual(await tagDeleted.json(), { deleted: tag.id });
    const untaggedListing = await get("/api/v1/files?path=%2FDocuments");
    const untaggedItem = (await untaggedListing.json()).items.find(
      (item) => item.path === docPath,
    );
    assert.deepEqual(untaggedItem.tags, []);

    // Comments: create, list, delete.
    const commentCreated = await fetch(`${base}/api/v1/files/comments`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: docPath, body: "first!" }),
    });
    assert.equal(commentCreated.status, 201, context.running.output());
    const comment = await commentCreated.json();
    assert.equal(comment.body, "first!");
    assert.equal(comment.author, session.user.id);
    assert.equal(comment.parentId, "");

    const comments = await get(`/api/v1/files/comments?path=${encodeURIComponent(docPath)}`);
    assert.equal((await comments.json()).items.length, 1);

    const commentDeleted = await fetch(
      `${base}/api/v1/files/comments/${comment.id}?path=${encodeURIComponent(docPath)}`,
      { method: "DELETE", headers: session.authHeaders },
    );
    assert.equal(commentDeleted.status, 200, context.running.output());
    assert.deepEqual(await commentDeleted.json(), { deleted: comment.id });
    const emptied = await get(`/api/v1/files/comments?path=${encodeURIComponent(docPath)}`);
    assert.equal((await emptied.json()).items.length, 0);

    // Locks block mutations until the lock is released.
    const locked = await fetch(`${base}/api/v1/files/lock`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: docPath }),
    });
    assert.equal(locked.status, 200, context.running.output());
    assert.deepEqual(await locked.json(), { locked: true, owner: session.user.id });

    const lockInfo = await get(`/api/v1/files/lock?path=${encodeURIComponent(docPath)}`);
    assert.deepEqual(await lockInfo.json(), { locked: true, owner: session.user.id });

    const blockedSave = await fetch(`${base}${editorPath}`, {
      method: "PUT",
      headers: { ...session.authHeaders, "if-match": `"${savedDoc.etag}"` },
      body: JSON.stringify({ content: "blocked" }),
    });
    assert.equal(blockedSave.status, 423, context.running.output());
    assert.equal((await blockedSave.json()).error.code, "locked");

    const blockedDelete = await fetch(
      `${base}/api/v1/files/files?path=${encodeURIComponent(docPath)}`,
      {
        method: "DELETE",
        headers: { cookie: session.cookies, origin: base, "x-csrf-token": session.csrfToken },
      },
    );
    assert.equal(blockedDelete.status, 423, context.running.output());
    assert.equal((await blockedDelete.json()).error.code, "locked");

    const unlocked = await fetch(
      `${base}/api/v1/files/lock?path=${encodeURIComponent(docPath)}`,
      { method: "DELETE", headers: session.authHeaders },
    );
    assert.equal(unlocked.status, 200, context.running.output());
    assert.deepEqual(await unlocked.json(), { locked: false });

    const resaved = await fetch(`${base}${editorPath}`, {
      method: "PUT",
      headers: { ...session.authHeaders, "if-match": restoredEtag },
      body: JSON.stringify({ content: "writable again" }),
    });
    assert.equal(resaved.status, 200, context.running.output());
    assert.equal((await resaved.json()).content, "writable again");

    // Previews serve UTF-8 text; binaries and images answer 415.
    const png = await sharp({
      create: {
        width: 400,
        height: 300,
        channels: 3,
        background: { r: 20, g: 40, b: 60 },
      },
    })
      .png()
      .toBuffer();
    const pic = await fetch(`${base}/api/v1/files/upload?path=%2FDocuments%2Fpic.png`, {
      method: "POST",
      headers: {
        cookie: session.cookies,
        origin: base,
        "x-csrf-token": session.csrfToken,
      },
      body: png,
    });
    assert.equal(pic.status, 201, context.running.output());

    const textPreview = await get("/api/v1/files/preview?path=%2FDocuments%2Fdoc.md");
    assert.equal(textPreview.status, 200, context.running.output());
    assert.deepEqual(await textPreview.json(), {
      type: "text/markdown",
      content: "writable again",
      name: "doc.md",
    });

    const imagePreview = await get("/api/v1/files/preview?path=%2FDocuments%2Fpic.png");
    assert.equal(imagePreview.status, 415, context.running.output());
    assert.equal((await imagePreview.json()).error.code, "unsupportedMediaType");

    // Thumbnails re-encode decodable images to a small JPEG.
    const thumb = await get("/api/v1/files/thumb?path=%2FDocuments%2Fpic.png");
    assert.equal(thumb.status, 200, context.running.output());
    assert.equal(thumb.headers.get("content-type"), "image/jpeg");
    const thumbBytes = Buffer.from(await thumb.arrayBuffer());
    assert.equal(thumbBytes[0], 0xff);
    assert.equal(thumbBytes[1], 0xd8);
    assert.ok(thumbBytes.length < png.length, "thumbnail is smaller than the source");

    const textThumb = await get("/api/v1/files/thumb?path=%2FDocuments%2Fdoc.md");
    assert.equal(textThumb.status, 415, context.running.output());
    assert.equal((await textThumb.json()).error.code, "unsupportedMediaType");
  } finally {
    await stopDarWithOidc(context);
    await rm(dataRoot, { recursive: true, force: true });
  }
});
