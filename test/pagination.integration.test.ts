import assert from "node:assert/strict";
import { test } from "node:test";
import { randomUUID } from "node:crypto";
import { connect, login } from "./integration-helpers.ts";
const origin = process.env.DAR_INTEGRATION_ORIGIN;
test("indexed folder windows have stable cursors, bounded subscriptions, prefix search and atomic name collisions", { skip: !origin, timeout: 30_000 }, async () => {
  const session = await login(origin!), db = await connect(session);
  try {
    const folder = randomUUID();
    await db.reducers.createFolder({ id: folder, parentId: "", name: `Pagination ${randomUUID().slice(0, 6)}` });
    const started = performance.now();
    for (let batch = 0; batch < 10; batch++) await Promise.all(Array.from({ length: 25 }, (_, i) => db.reducers.createFolder({ id: randomUUID(), parentId: folder, name: `Item ${String(batch * 25 + i).padStart(3, "0")}` })));
    await db.reducers.browse({ parentId: folder, section: "files", query: "", cursor: "" });
    const first = [...db.db.myFiles.iter()].sort((a, b) => a.listKey.localeCompare(b.listKey));
    assert.equal(first.length, 201);
    const cursor = first[199].listKey;
    await db.reducers.browse({ parentId: folder, section: "files", query: "", cursor });
    const second = [...db.db.myFiles.iter()];
    assert.equal(second.length, 50);
    const picker = JSON.parse(await db.procedures.folderChoices({ parentId: folder, query: "Item 24" })) as { items: { name: string }[] };
    assert.equal(picker.items.length, 10);
    const ids = new Set([...first.slice(0, 200), ...second].map(row => row.id));
    assert.equal(ids.size, 250);
    await db.reducers.browse({ parentId: folder, section: "files", query: "Item 24", cursor: "" });
    assert.equal(db.db.myFiles.count(), 10n);
    const collision = await Promise.allSettled(["Same name", "SAME NAME"].map(name => db.reducers.createFolder({ id: randomUUID(), parentId: folder, name })));
    assert.equal(collision.filter(result => result.status === "fulfilled").length, 1);
    console.info(`250-entry indexed collection proof: ${Math.round(performance.now() - started)} ms including durable creation, pagination, search and collision checks.`);
  } finally { db.disconnect(); }
});
