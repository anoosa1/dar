import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import path from "node:path";
import test from "node:test";

import {
  loginSession,
  startDarWithOidc,
  stopDarWithOidc,
  waitForHealth,
} from "./helpers/dar-server.mjs";

test("app passwords are created once, listed without secrets, and revoked by revision", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-app-passwords-test-");
  const context = await startDarWithOidc(dataRoot);
  try {
    await waitForHealth(context.port, context.running.child, context.running.output);
    const session = await loginSession(context.baseUrl, context.running.output);
    const base = context.baseUrl;

    const anonymous = await fetch(`${base}/api/v1/app-passwords`);
    assert.equal(anonymous.status, 401);
    assert.equal((await anonymous.json()).error.code, "unauthenticated");

    const withoutCsrf = await fetch(`${base}/api/v1/app-passwords`, {
      method: "POST",
      headers: { cookie: session.cookies, origin: base, "content-type": "application/json" },
      body: JSON.stringify({ label: "Laptop", scopes: ["files"] }),
    });
    assert.equal(withoutCsrf.status, 403);

    const created = await fetch(`${base}/api/v1/app-passwords`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ label: "Laptop", scopes: ["files", "calendar"] }),
    });
    assert.equal(created.status, 201, context.running.output());
    const createdBody = await created.json();
    assert.match(createdBody.token, /^[A-Za-z0-9_-]{43}$/);
    assert.equal(createdBody.label, "Laptop");
    assert.deepEqual(createdBody.scopes, ["files", "calendar"]);
    assert.equal(createdBody.revision, 1);
    assert.equal(createdBody.lastUsedAt, null);
    assert.match(createdBody.prefix, /^[A-Za-z0-9_-]{12}…$/);
    assert.ok(createdBody.id);

    const invalidScope = await fetch(`${base}/api/v1/app-passwords`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ label: "Bad", scopes: ["email"] }),
    });
    assert.equal(invalidScope.status, 400);
    assert.equal((await invalidScope.json()).error.code, "invalid");

    const duplicateScopes = await fetch(`${base}/api/v1/app-passwords`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ label: "Bad", scopes: ["files", "files"] }),
    });
    assert.equal(duplicateScopes.status, 400);

    const listed = await fetch(`${base}/api/v1/app-passwords`, {
      headers: { cookie: session.cookies },
    });
    assert.equal(listed.status, 200);
    const listedBody = await listed.json();
    assert.equal(listedBody.items.length, 1);
    assert.equal(listedBody.items[0].id, createdBody.id);
    assert.equal(listedBody.items[0].token, undefined);

    const deleteWithoutIfMatch = await fetch(
      `${base}/api/v1/app-passwords/${createdBody.id}`,
      { method: "DELETE", headers: session.authHeaders },
    );
    assert.equal(deleteWithoutIfMatch.status, 400);

    const deleteWithStaleRevision = await fetch(
      `${base}/api/v1/app-passwords/${createdBody.id}`,
      { method: "DELETE", headers: { ...session.authHeaders, "if-match": '"7"' } },
    );
    assert.equal(deleteWithStaleRevision.status, 409);
    assert.equal((await deleteWithStaleRevision.json()).error.code, "conflict");

    const deleteUnknown = await fetch(`${base}/api/v1/app-passwords/does-not-exist`, {
      method: "DELETE",
      headers: { ...session.authHeaders, "if-match": '"1"' },
    });
    assert.equal(deleteUnknown.status, 404);

    const revoked = await fetch(`${base}/api/v1/app-passwords/${createdBody.id}`, {
      method: "DELETE",
      headers: { ...session.authHeaders, "if-match": '"1"' },
    });
    assert.equal(revoked.status, 200);
    assert.deepEqual(await revoked.json(), { ok: true });

    const afterRevocation = await fetch(`${base}/api/v1/app-passwords`, {
      headers: { cookie: session.cookies },
    });
    assert.deepEqual(await afterRevocation.json(), { items: [] });

    const revokedAgain = await fetch(`${base}/api/v1/app-passwords/${createdBody.id}`, {
      method: "DELETE",
      headers: { ...session.authHeaders, "if-match": '"1"' },
    });
    // The record still exists but moved to revision 2 when it was revoked.
    assert.equal(revokedAgain.status, 409);
    assert.equal((await revokedAgain.json()).error.code, "conflict");
  } finally {
    await stopDarWithOidc(context);
    await rm(dataRoot, { recursive: true, force: true });
  }
});
