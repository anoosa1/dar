import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { tmpdir } from "node:os";
import { signingJwk } from "../runtime/auth/keyring.ts";
test("signing keys survive reload without exposing private JWK fields on disk; tampering fails closed", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "dar-keyring-proof-"));
  const first = await signingJwk(root), second = await signingJwk(root);
  assert.equal(first.d, second.d); assert.equal(first.kid, second.kid);
  const filename = path.join(root, "signing-key.json");
  const encoded = await readFile(filename, "utf8");
  assert.equal(encoded.includes(first.d!), false);
  const envelope = JSON.parse(encoded); envelope.tag = Buffer.alloc(16).toString("base64url");
  await writeFile(filename, JSON.stringify(envelope));
  await assert.rejects(signingJwk(root), /cannot be decrypted/);
});
