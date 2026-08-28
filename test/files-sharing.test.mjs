import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { mkdtemp, rm } from "node:fs/promises";
import test from "node:test";

import {
  loginSession,
  startDarWithOidc,
  stopDarWithOidc,
  waitForHealth,
} from "./helpers/dar-server.mjs";

test("public shares gate on password, rate limits, and drop uploads over http", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-files-share-test-");
  // Password-protected shares derive digests from the credential keyring,
  // so the test instance runs with a deterministic key.
  const credentialKeys =
    `test:${Buffer.from("dar-share-test-key-0000000000000", "utf8").toString("base64")}`;
  const context = await startDarWithOidc(dataRoot, {
    DAR_CREDENTIAL_ENCRYPTION_KEYS: credentialKeys,
  });
  try {
    await waitForHealth(context.port, context.running.child, context.running.output);
    const session = await loginSession(context.baseUrl, context.running.output);
    const base = context.baseUrl;
    const get = (path) => fetch(`${base}${path}`, { headers: { cookie: session.cookies } });

    // Seed content: a drop folder and a document to share.
    const folder = await fetch(`${base}/api/v1/files/folders`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/Drops" }),
    });
    assert.equal(folder.status, 201, context.running.output());

    const upload = await fetch(`${base}/api/v1/files/upload?path=%2FDrops%2Finner.txt`, {
      method: "POST",
      headers: {
        cookie: session.cookies,
        origin: base,
        "x-csrf-token": session.csrfToken,
      },
      body: "inner bytes",
    });
    assert.equal(upload.status, 201, context.running.output());

    const doc = await fetch(`${base}/api/v1/files/upload?path=%2Fdoc.md`, {
      method: "POST",
      headers: {
        cookie: session.cookies,
        origin: base,
        "x-csrf-token": session.csrfToken,
      },
      body: "shared bytes",
    });
    assert.equal(doc.status, 201, context.running.output());

    // Shares are listed and created through the session API.
    const sharesEmpty = await get("/api/v1/files/shares");
    assert.deepEqual(await sharesEmpty.json(), { items: [], nextCursor: null });

    const pastExpiry = await fetch(`${base}/api/v1/files/shares`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/Drops", expiresAt: "2020-01-01T00:00:00.000Z" }),
    });
    assert.equal(pastExpiry.status, 400, context.running.output());
    assert.equal((await pastExpiry.json()).error.code, "invalid");

    const dropShareRes = await fetch(`${base}/api/v1/files/shares`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/Drops", password: "s3cret", drop: true }),
    });
    assert.equal(dropShareRes.status, 201, context.running.output());
    const dropShare = await dropShareRes.json();
    assert.match(dropShare.token, /^[0-9a-f]{64}$/);
    assert.equal(dropShare.hasPassword, true);
    assert.equal(dropShare.drop, true);

    const listed = await get("/api/v1/files/shares");
    assert.equal((await listed.json()).items.length, 1);

    // Anonymous access without the unlock cookie asks for the password.
    const locked = await fetch(`${base}/share/${dropShare.token}`);
    assert.equal(locked.status, 401, context.running.output());
    assert.equal((await locked.json()).error.code, "unauthenticated");

    const badUnlock = await fetch(`${base}/share/${dropShare.token}/unlock`, {
      method: "POST",
      headers: { "content-type": "application/json", origin: base },
      body: JSON.stringify({ password: "wrong" }),
    });
    assert.equal(badUnlock.status, 401, context.running.output());
    assert.equal((await badUnlock.json()).error.code, "unauthenticated");

    const unlock = await fetch(`${base}/share/${dropShare.token}/unlock`, {
      method: "POST",
      headers: { "content-type": "application/json", origin: base },
      body: JSON.stringify({ password: "s3cret" }),
    });
    assert.equal(unlock.status, 200, context.running.output());
    const shareCookie = (unlock.headers.getSetCookie().find((value) =>
      value.startsWith("dar_share_access="),
    ));
    assert.match(shareCookie, /Path=\/share\/[0-9a-f]{64}/);
    const shareCookiePair = shareCookie.split(";")[0];

    // Drop shares expose no listing; uploads land in the shared folder.
    const dropPage = await fetch(`${base}/share/${dropShare.token}`, {
      headers: { cookie: shareCookiePair },
    });
    assert.equal(dropPage.status, 200, context.running.output());
    const dropMeta = await dropPage.json();
    assert.equal(dropMeta.drop, true);
    assert.equal(dropMeta.items, undefined);

    const dropped = await fetch(
      `${base}/share/${dropShare.token}/upload?name=${encodeURIComponent("dropped.txt")}`,
      {
        method: "POST",
        headers: { "content-type": "application/octet-stream", cookie: shareCookiePair },
        body: "dropped bytes",
      },
    );
    assert.equal(dropped.status, 201, context.running.output());
    assert.deepEqual(await dropped.json(), { saved: "/Drops/dropped.txt" });

    const ownerView = await get("/api/v1/files?path=%2FDrops");
    const names = (await ownerView.json()).items.map((item) => item.name).sort();
    assert.deepEqual(names, ["dropped.txt", "inner.txt"]);

    const forged = await fetch(
      `${base}/share/${dropShare.token}/upload?name=${encodeURIComponent("x.txt")}`,
      {
        method: "POST",
        headers: { "content-type": "application/octet-stream" },
        body: "x",
      },
    );
    assert.equal(forged.status, 403, context.running.output());
    assert.equal((await forged.json()).error.code, "forbidden");

    // The upload window is twenty per minute per token and peer.
    let limited;
    for (let i = 0; i < 21 && !limited; i += 1) {
      const attempt = await fetch(
        `${base}/share/${dropShare.token}/upload?name=${encodeURIComponent(`rate-${i}.txt`)}`,
        {
          method: "POST",
          headers: { "content-type": "application/octet-stream", cookie: shareCookiePair },
          body: "r",
        },
      );
      if (attempt.status === 429) {
        limited = attempt;
      } else {
        assert.equal(attempt.status, 201, context.running.output());
      }
    }
    assert.ok(limited, "expected uploads to be rate limited within the window");
    assert.equal((await limited.json()).error.code, "rateLimited");
    assert.match(limited.headers.get("retry-after"), /^\d+$/);

    // Non-drop shares serve a one-level listing with downloads.
    const readShareRes = await fetch(`${base}/api/v1/files/shares`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/Drops" }),
    });
    assert.equal(readShareRes.status, 201, context.running.output());
    const readShare = await readShareRes.json();

    const listing = await fetch(`${base}/share/${readShare.token}`);
    assert.equal(listing.status, 200, context.running.output());
    const listingBody = await listing.json();
    assert.equal(listingBody.name, "Drops");
    // Rate-window uploads from the block above are also listed; the
    // pre-existing files are the interesting part here.
    assert.deepEqual(
      listingBody.items
        .map((item) => item.name)
        .filter((name) => !name.startsWith("rate-"))
        .sort(),
      ["dropped.txt", "inner.txt"],
    );
    assert.deepEqual(
      listingBody.items
        .filter((item) => item.name === "inner.txt")
        .map((item) => [item.name, item.isDir]),
      [["inner.txt", false]],
    );

    const download = await fetch(
      `${base}/share/${readShare.token}/download?name=${encodeURIComponent("inner.txt")}`,
    );
    assert.equal(download.status, 200, context.running.output());
    assert.equal(download.headers.get("content-type"), "application/octet-stream");
    assert.equal(await download.text(), "inner bytes");

    // Sharing a file serves the file itself.
    const docShareRes = await fetch(`${base}/api/v1/files/shares`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/doc.md" }),
    });
    const docShare = await docShareRes.json();
    const docDownload = await fetch(`${base}/share/${docShare.token}/download`);
    assert.equal(await docDownload.text(), "shared bytes");

    const docUpload = await fetch(
      `${base}/share/${docShare.token}/upload?name=${encodeURIComponent("x.txt")}`,
      {
        method: "POST",
        headers: { "content-type": "application/octet-stream" },
        body: "x",
      },
    );
    assert.equal(docUpload.status, 403, context.running.output());

    // Revoked tokens disappear from the public namespace.
    const revoke = await fetch(
      `${base}/api/v1/files/shares/${docShare.token}`,
      { method: "DELETE", headers: session.authHeaders },
    );
    assert.equal(revoke.status, 200, context.running.output());
    assert.deepEqual(await revoke.json(), { deleted: docShare.token });
    const gone = await fetch(`${base}/share/${docShare.token}`);
    assert.equal(gone.status, 404);
    assert.equal((await gone.json()).error.code, "notFound");

    // User shares resolve recipients by email and are removable.
    const seedUser = new DatabaseSync(`${dataRoot}/dar.sqlite`);
    seedUser.exec(
      "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES ('user-b', 'sub-b', 'User B', 'b@example.test', 0, datetime(), datetime())",
    );
    seedUser.close();

    const noRecipient = await fetch(`${base}/api/v1/files/user-shares`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/Drops", toUser: "ghost@example.test" }),
    });
    assert.equal(noRecipient.status, 404, context.running.output());
    assert.equal((await noRecipient.json()).error.code, "notFound");

    const badPermission = await fetch(`${base}/api/v1/files/user-shares`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/Drops", toUser: "b@example.test", permission: "admin" }),
    });
    assert.equal(badPermission.status, 400, context.running.output());

    const granted = await fetch(`${base}/api/v1/files/user-shares`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ path: "/Drops", toUser: "b@example.test", permission: "write" }),
    });
    assert.equal(granted.status, 201, context.running.output());
    const grantedBody = await granted.json();
    assert.equal(grantedBody.toUser, "user-b");
    assert.equal(grantedBody.permission, "write");

    const sharedWithMe = await get("/api/v1/files/shared-with-me");
    assert.equal((await sharedWithMe.json()).items.length, 0);

    const removed = await fetch(
      `${base}/api/v1/files/user-shares/${grantedBody.id}`,
      { method: "DELETE", headers: session.authHeaders },
    );
    assert.equal(removed.status, 200, context.running.output());
    assert.deepEqual(await removed.json(), { deleted: grantedBody.id });

    const removedAgain = await fetch(
      `${base}/api/v1/files/user-shares/${grantedBody.id}`,
      { method: "DELETE", headers: session.authHeaders },
    );
    assert.equal(removedAgain.status, 404);
  } finally {
    await stopDarWithOidc(context);
    await rm(dataRoot, { recursive: true, force: true });
  }
});
