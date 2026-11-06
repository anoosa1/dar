import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { tmpdir } from "node:os";
import { signingJwk } from "../runtime/auth/keyring.ts";
import {keyringBox} from "../runtime/auth/keyring.ts";
import {providerKeys} from "../runtime/identity/keys.ts";
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
test("external signing rotation survives reload, retires old private keys and rejects cross-purpose ciphertext",async(t)=>{
  const root=await mkdtemp(path.join(tmpdir(),"dar-provider-keyring-"));await signingJwk(root);const keys=await providerKeys(root),first=keys.current().active.kid;
  await keys.rotate();const reloaded=await providerKeys(root);assert.notEqual(reloaded.current().active.kid,first);assert.equal(reloaded.current().retired[0].jwk.kid,first);
  t.mock.timers.enable({apis:["Date"],now:Date.now()+601000});await reloaded.prune();assert.equal((await providerKeys(root)).current().retired.length,0);t.mock.timers.reset();
  const a=await keyringBox(root,"provider-model"),b=await keyringBox(root,"provider-client"),cipher=a.encrypt({secret:"a test value"});assert.throws(()=>b.decrypt(cipher));assert.deepEqual(a.decrypt(cipher),{secret:"a test value"});
});
