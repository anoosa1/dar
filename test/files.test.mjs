import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import test from "node:test";

import {
  loginSession,
  startDarWithOidc,
  stopDarWithOidc,
  waitForHealth,
} from "./helpers/dar-server.mjs";

test("files api covers browse, mutations, trash, favorites, and quota reporting", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-files-test-");
  const context = await startDarWithOidc(dataRoot);
  try {
    await waitForHealth(context.port, context.running.child, context.running.output);
    const session = await loginSession(context.baseUrl, context.running.output);
    const base = context.baseUrl;
    const get = (url) => fetch(`${base}${url}`, { headers: { cookie: session.cookies } });

    const anonymous = await fetch(`${base}/api/v1/files`);
    assert.equal(anonymous.status, 401);
    assert.equal((await anonymous.json()).error.code, "unauthenticated");

    const empty = await get("/api/v1/files");
    assert.equal(empty.status, 200);
    assert.deepEqual(await empty.json(), { items: [], nextCursor: null });

    // Folders: create once, conflict on repeat, invalid path rejected.
    const folder = await fetch(`${base}/api/v1/files/folders`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/Documents" }),
    });
    assert.equal(folder.status, 201, context.running.output());
    assert.deepEqual(await folder.json(), { path: "/Documents" });

    const duplicateFolder = await fetch(`${base}/api/v1/files/folders`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/Documents" }),
    });
    assert.equal(duplicateFolder.status, 409);
    assert.equal((await duplicateFolder.json()).error.code, "conflict");

    const unsafeFolder = await fetch(`${base}/api/v1/files/folders`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/../etc" }),
    });
    assert.equal(unsafeFolder.status, 400);
    assert.equal((await unsafeFolder.json()).error.code, "invalid");

    // Touch creates an empty file.
    const touched = await fetch(`${base}/api/v1/files/files`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/Documents/notes.md" }),
    });
    assert.equal(touched.status, 201, context.running.output());
    const touchedBody = await touched.json();
    assert.equal(touchedBody.path, "/Documents/notes.md");
    assert.equal(touchedBody.size, 0);

    // Upload replaces content; listing reports camelCase entries.
    const upload = await fetch(`${base}/api/v1/files/upload?path=/Documents/notes.md`, {
      method: "POST",
      headers: {
        cookie: session.cookies,
        origin: base,
        "content-type": "application/octet-stream",
        "x-csrf-token": session.csrfToken,
      },
      body: "hello dar",
    });
    assert.equal(upload.status, 201, context.running.output());
    assert.deepEqual(await upload.json(), {
      path: "/Documents/notes.md",
      size: 9,
    });

    const listing = await get("/api/v1/files?path=/Documents");
    assert.equal(listing.status, 200);
    const { items } = await listing.json();
    assert.equal(items.length, 1);
    assert.equal(items[0].name, "notes.md");
    assert.equal(items[0].path, "/Documents/notes.md");
    assert.equal(items[0].isDir, false);
    assert.equal(items[0].size, 9);
    assert.equal(items[0].mimeType, "text/markdown");
    assert.equal(items[0].favorited, false);
    assert.deepEqual(items[0].tags, []);

    // Favorites show up in listings.
    const favorite = await fetch(`${base}/api/v1/files/favorite`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/Documents/notes.md", favorited: true }),
    });
    assert.equal(favorite.status, 200, context.running.output());
    const favorited = await get("/api/v1/files?path=/Documents");
    assert.equal((await favorited.json()).items[0].favorited, true);

    // Rename and copy.
    const rename = await fetch(`${base}/api/v1/files/rename`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ from: "/Documents/notes.md", to: "/Documents/renamed.md" }),
    });
    assert.equal(rename.status, 200, context.running.output());
    assert.deepEqual(await rename.json(), { renamed: "/Documents/renamed.md" });

    const copy = await fetch(`${base}/api/v1/files/copy`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ from: "/Documents/renamed.md", to: "/Copy.md" }),
    });
    assert.equal(copy.status, 200, context.running.output());
    assert.deepEqual(await copy.json(), { copied: "/Copy.md" });

    const rootListing = await get("/api/v1/files");
    const rootItems = (await rootListing.json()).items;
    // Baseline ordering: directories first, then case-insensitive names.
    assert.deepEqual(rootItems.map((item) => item.name), ["Documents", "Copy.md"]);
    assert.equal(rootItems[0].isDir, true);

    // Delete sends the file to trash; restore brings it back; purge removes it.
    const deleted = await fetch(`${base}/api/v1/files/files?path=/Copy.md`, {
      method: "DELETE",
      headers: { cookie: session.cookies, origin: base, "x-csrf-token": session.csrfToken },
    });
    assert.equal(deleted.status, 200, context.running.output());
    const deletedBody = await deleted.json();
    assert.equal(deletedBody.path, "/Copy.md");
    assert.ok(deletedBody.id);

    const restored = await fetch(
      `${base}/api/v1/files/trash/${deletedBody.id}/restore`,
      {
        method: "POST",
        headers: session.authHeaders,
      },
    );
    assert.equal(restored.status, 200, context.running.output());
    assert.deepEqual(await restored.json(), { restored: "/Copy.md" });

    const afterRestore = await get("/api/v1/files");
    assert.ok((await afterRestore.json()).items.some((item) => item.path === "/Copy.md"));

    const deletedAgain = await fetch(`${base}/api/v1/files/files?path=/Copy.md`, {
      method: "DELETE",
      headers: { cookie: session.cookies, origin: base, "x-csrf-token": session.csrfToken },
    });
    const deletedAgainBody = await deletedAgain.json();
    const purged = await fetch(
      `${base}/api/v1/files/trash/${deletedAgainBody.id}`,
      {
        method: "DELETE",
        headers: { cookie: session.cookies, origin: base, "x-csrf-token": session.csrfToken },
      },
    );
    assert.equal(purged.status, 200, context.running.output());

    const missingRestore = await fetch(
      `${base}/api/v1/files/trash/${deletedAgainBody.id}/restore`,
      { method: "POST", headers: session.authHeaders },
    );
    assert.equal(missingRestore.status, 404);
    assert.equal((await missingRestore.json()).error.code, "notFound");

    // Quota reporting is available even without a quota configured.
    const quota = await get("/api/v1/files/quota");
    assert.equal(quota.status, 200);
    const quotaBody = await quota.json();
    assert.equal(quotaBody.total, 0);
    assert.ok(quotaBody.used >= 9);
  } finally {
    await stopDarWithOidc(context);
    await rm(dataRoot, { recursive: true, force: true });
  }
});

test("files quota rejects uploads that exceed the configured limit", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-files-quota-test-");
  const context = await startDarWithOidc(dataRoot, { DAR_FILES_QUOTA_BYTES: "100" });
  try {
    await waitForHealth(context.port, context.running.child, context.running.output);
    const session = await loginSession(context.baseUrl, context.running.output);
    const base = context.baseUrl;

    const small = await fetch(`${base}/api/v1/files/upload?path=/small.txt`, {
      method: "POST",
      headers: {
        cookie: session.cookies,
        origin: base,
        "x-csrf-token": session.csrfToken,
      },
      body: "12345678",
    });
    assert.equal(small.status, 201, context.running.output());

    const oversized = await fetch(`${base}/api/v1/files/upload?path=/big.txt`, {
      method: "POST",
      headers: {
        cookie: session.cookies,
        origin: base,
        "x-csrf-token": session.csrfToken,
      },
      body: "x".repeat(101),
    });
    assert.equal(oversized.status, 507, context.running.output());
    assert.equal((await oversized.json()).error.code, "quotaExceeded");

    // Replacing an existing file frees its previous size first.
    const replace = await fetch(`${base}/api/v1/files/upload?path=/small.txt`, {
      method: "POST",
      headers: {
        cookie: session.cookies,
        origin: base,
        "x-csrf-token": session.csrfToken,
      },
      body: "87654321",
    });
    assert.equal(replace.status, 201, context.running.output());
  } finally {
    await stopDarWithOidc(context);
    await rm(dataRoot, { recursive: true, force: true });
  }
});

test("file uploads enforce their dedicated body limit", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-files-limit-test-");
  const context = await startDarWithOidc(dataRoot, { DAR_UPLOAD_BODY_LIMIT: "1024" });
  try {
    await waitForHealth(context.port, context.running.child, context.running.output);
    const session = await loginSession(context.baseUrl, context.running.output);
    const base = context.baseUrl;

    const oversized = await fetch(`${base}/api/v1/files/upload?path=/big.txt`, {
      method: "POST",
      headers: {
        cookie: session.cookies,
        origin: base,
        "x-csrf-token": session.csrfToken,
      },
      body: "x".repeat(2048),
    });
    assert.equal(oversized.status, 413, context.running.output());
    assert.equal((await oversized.json()).error.code, "tooLarge");

    const fits = await fetch(`${base}/api/v1/files/upload?path=/small.txt`, {
      method: "POST",
      headers: {
        cookie: session.cookies,
        origin: base,
        "x-csrf-token": session.csrfToken,
      },
      body: "x".repeat(512),
    });
    assert.equal(fits.status, 201, context.running.output());
  } finally {
    await stopDarWithOidc(context);
    await rm(dataRoot, { recursive: true, force: true });
  }
});

test("files settings return defaults and update under If-Match", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-files-settings-test-");
  const context = await startDarWithOidc(dataRoot);
  try {
    await waitForHealth(context.port, context.running.child, context.running.output);
    const session = await loginSession(context.baseUrl, context.running.output);
    const base = context.baseUrl;

    const initial = await fetch(`${base}/api/v1/files/settings`, {
      headers: { cookie: session.cookies },
    });
    assert.equal(initial.status, 200, context.running.output());
    const initialBody = await initial.json();
    assert.equal(initialBody.settings.defaultView, "list");
    assert.equal(initialBody.settings.sortFavoritesFirst, true);
    assert.equal(initialBody.revision, 0);

    const rejected = await fetch(`${base}/api/v1/files/settings`, {
      method: "PUT",
      headers: { ...session.authHeaders, "if-match": '"5"' },
      body: JSON.stringify({ settings: { defaultView: "grid" } }),
    });
    assert.equal(rejected.status, 412, context.running.output());

    const updated = await fetch(`${base}/api/v1/files/settings`, {
      method: "PUT",
      headers: {
        ...session.authHeaders,
        "if-match": `"${initialBody.revision}"`,
      },
      body: JSON.stringify({
        settings: { defaultView: "grid", personalFolder: "/Docs" },
      }),
    });
    assert.equal(updated.status, 200, context.running.output());
    const updatedBody = await updated.json();
    assert.equal(updatedBody.settings.defaultView, "grid");
    assert.equal(updatedBody.settings.personalFolder, "/Docs");
    assert.equal(updatedBody.settings.trashFolder, "/.local/share/Trash");
    assert.equal(updatedBody.revision, 1);

    const invalid = await fetch(`${base}/api/v1/files/settings`, {
      method: "PUT",
      headers: session.authHeaders,
      body: JSON.stringify({ settings: { defaultView: "mosaic" } }),
    });
    assert.equal(invalid.status, 400, context.running.output());

    const refetched = await fetch(`${base}/api/v1/files/settings`, {
      headers: { cookie: session.cookies },
    });
    assert.equal(refetched.headers.get("etag"), `"${updatedBody.revision}"`);
  } finally {
    await stopDarWithOidc(context);
    await rm(dataRoot, { recursive: true, force: true });
  }
});
