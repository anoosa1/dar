import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import test from "node:test";

import {
  loginSession,
  startDarWithOidc,
  stopDarWithOidc,
  waitForHealth,
} from "./helpers/dar-server.mjs";

test("assistant tool API requires a session and lists registered tools", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-assistant-test-");
  const context = await startDarWithOidc(dataRoot);
  try {
    await waitForHealth(context.port, context.running.child, context.running.output);
    const session = await loginSession(context.baseUrl, context.running.output);
    const base = context.baseUrl;

    const anonymous = await fetch(`${base}/api/v1/assistant/tools`);
    assert.equal(anonymous.status, 401);
    assert.equal((await anonymous.json()).error.code, "unauthenticated");

    const catalog = await fetch(`${base}/api/v1/assistant/tools`, {
      headers: { cookie: session.cookies },
    });
    assert.equal(catalog.status, 200);
    // Built-in apps register tools on boot; the catalog carries them.
    const catalogBody = await catalog.json();
    assert.ok(Array.isArray(catalogBody.items));
    assert.ok(catalogBody.items.every((tool) =>
      typeof tool.name === "string" && typeof tool.title === "string" &&
      typeof tool.effect === "string" && tool.parameters));

    const unknownTool = await fetch(`${base}/api/v1/assistant/tools/no.such/invoke`, {
      method: "POST",
      headers: session.authHeaders,
      body: JSON.stringify({ arguments: {} }),
    });
    assert.equal(unknownTool.status, 404);
    assert.equal((await unknownTool.json()).error.code, "notFound");

    const withoutConfirmation = await fetch(
      `${base}/api/v1/assistant/tools/test.destroy/invoke`,
      {
        method: "POST",
        headers: session.authHeaders,
        body: JSON.stringify({ arguments: {}, confirm: false }),
      },
    );
    assert.equal(withoutConfirmation.status, 404);
  } finally {
    await stopDarWithOidc(context);
    await rm(dataRoot, { recursive: true, force: true });
  }
});
