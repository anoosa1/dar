import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, readFile, writeFile, access } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import { createServer } from "node:net";
import { database } from "../runtime/database.ts";
import { backup, restore, exportFiles } from "../runtime/backup.ts";
import { startServer } from "../runtime/server.ts";
import { login, upload } from "./integration-helpers.ts";
import {identityAdmin,authorize,exchange} from "./identity-helpers.ts";
import * as oidc from "openid-client";

const origin = process.env.DAR_INTEGRATION_ORIGIN;
test("consistent backup, corruption rejection, empty-root restore, session invalidation and byte recovery", { skip: !origin || !process.env.DAR_TEST_ROOT, timeout: 40_000 }, async () => {
  const sourceRoot = process.env.DAR_TEST_ROOT!, base = origin!;
  const session = await login(base);
  const identity=await identityAdmin(base),registeredClient=await identity.client(),provisioning=await identity.credential();
  const beforeTokens=await exchange(registeredClient,await authorize(base,identity.session,registeredClient));
  const beforeKeys=await (await fetch(`${base}/oidc/jwks`)).json();
  const file = await upload(base, session, `backup-${randomUUID()}.txt`, "This content must survive a full restore.");
  const noteHeaders={...session.headers,"content-type":"application/json"},trashBook=await (await fetch(`${base}/api/v1/notes/notebooks`,{method:"POST",headers:noteHeaders,body:JSON.stringify({name:"Backup trash proof"})})).json() as {id:string};
  const trashResponse=await fetch(`${base}/api/v1/notes/items`,{method:"POST",headers:noteHeaders,body:JSON.stringify({collectionId:trashBook.id,title:"Retained in trash",content:"Trash must survive restore."})}),trashNote=await trashResponse.json() as {id:string};assert.equal(trashResponse.status,201);
  assert.equal((await fetch(`${base}/api/v1/notes/items/${trashNote.id}`,{method:"DELETE",headers:{...noteHeaders,"if-match":trashResponse.headers.get("etag")!}})).status,200);
  const notes=await (await fetch(`${base}/api/v1/notes/items?view=all`,{headers:session.headers})).json() as {items:{id:string}[]};
  const trash=await (await fetch(`${base}/api/v1/notes/trash`,{headers:session.headers})).json() as {items:{id:string}[]};
  const noteProof=notes.items[0]?await (await fetch(`${base}/api/v1/notes/items/${notes.items[0].id}`,{headers:session.headers})).json() as {id:string;content:string}:undefined;
  const beforePim = await (await fetch(`${base}/api/v1/contacts/books`, { headers: session.headers })).json() as { items: { id: string; name: string }[] };
  const sampleBook = beforePim.items[0];
  const beforeCards = sampleBook ? await (await fetch(`${base}/api/v1/contacts/books/${sampleBook.id}/export`, { headers: session.headers })).text() : "";
  const beforeCalendars = await (await fetch(`${base}/api/v1/calendar/calendars`, { headers: session.headers })).json() as { items: { id: string }[] };
  let eventProof: { collectionId: string; href: string; content: string } | undefined;
  for (const calendar of beforeCalendars.items) {
    const rows = await (await fetch(`${base}/api/v1/calendar/calendars/${calendar.id}/items`, { headers: session.headers })).json() as { items: { href: string }[] };
    if (rows.items[0]) { const event = await (await fetch(`${base}/api/v1/calendar/calendars/${calendar.id}/items/${rows.items[0].href}`, { headers: session.headers })).json() as { content: string }; eventProof = { collectionId: calendar.id, href: rows.items[0].href, content: event.content }; break; }
  }
  const source = await database(process.env.DAR_DATABASE_URL!, process.env.DAR_DATABASE_NAME!, process.env.DAR_DATABASE_TOKEN_FILE!);
  const temporary = await mkdtemp(path.join(tmpdir(), "dar-backup-proof-")), directory = path.join(temporary, "backup");
  const result = await backup(source, sourceRoot, directory);
  assert.ok(result.blobs > 0);
  const exported = path.join(temporary, "export"); await exportFiles(source, sourceRoot, "admin", exported);
  const manifest = JSON.parse(await readFile(path.join(exported, "manifest.json"), "utf8")) as { files: { id: string; contentFile?: string }[] };
  assert.equal(await readFile(path.join(exported, manifest.files.find(item => item.id === file.fileId)!.contentFile!), "utf8"), "This content must survive a full restore.");
  assert.doesNotMatch(JSON.stringify(manifest), /verifier|csrfDigest|signing-key/);
  const saved = await readFile(path.join(directory, "snapshot.json"), "utf8");
  assert.match(saved, /dar-backup-v5/);
  assert.doesNotMatch(saved,/"providerModels"|"provisioningTokens"|"identityProofs"/);
  const listener = createServer(); await new Promise<void>(resolve => listener.listen(0, "127.0.0.1", resolve));
  const port = (listener.address() as import("node:net").AddressInfo).port;
  await new Promise<void>(resolve => listener.close(() => resolve()));
  const newOrigin = `http://127.0.0.1:${port}`, name = `dar-restore-${randomUUID().slice(0, 8)}`;
  const project = path.resolve(import.meta.dirname, "..");
  const publish = spawnSync(process.env.DAR_SPACETIME_BIN!, ["--root-dir", process.env.DAR_SPACETIME_CLI_ROOT!, "publish", name, "--js-path", path.join(project, "module/dist/bundle.js"), "--server", process.env.DAR_DATABASE_URL!, "--yes", "--no-config"], { env: { ...process.env, DAR_ISSUER: `${newOrigin}/auth/issuer` }, encoding: "utf8" });
  assert.equal(publish.status, 0, publish.stdout + publish.stderr);
  const target = await database(process.env.DAR_DATABASE_URL!, name, process.env.DAR_DATABASE_TOKEN_FILE!);
  const root = path.join(temporary, "restored");
  await writeFile(path.join(directory, "snapshot.json"), `${saved} `);
  await assert.rejects(restore(target, directory, root), /corrupted/);
  await writeFile(path.join(directory, "snapshot.json"), saved);
  await restore(target, directory, root);
  const runtime = await startServer({ host: "127.0.0.1", port, origin: newOrigin, root, databaseUrl: process.env.DAR_DATABASE_URL!, databaseName: name,
    serviceTokenFile: process.env.DAR_DATABASE_TOKEN_FILE!, bootstrapTokenFile: process.env.DAR_BOOTSTRAP_TOKEN_FILE!, development: true });
  try {
    assert.equal((await fetch(`${newOrigin}/api/v1/auth/session`, { headers: session.headers })).status, 401);
    const recovered = await login(newOrigin);
    if(noteProof){const after=await (await fetch(`${newOrigin}/api/v1/notes/items/${noteProof.id}`,{headers:recovered.headers})).json() as {content:string};assert.equal(after.content,noteProof.content);}
    const restoredTrash=await (await fetch(`${newOrigin}/api/v1/notes/trash`,{headers:recovered.headers})).json() as typeof trash;assert.deepEqual(restoredTrash.items.map(r=>r.id).sort(),trash.items.map(r=>r.id).sort());
    assert.deepEqual(await (await fetch(`${newOrigin}/oidc/jwks`)).json(),beforeKeys);
    assert.equal((await fetch(`${newOrigin}/scim/v2/Users`,{headers:{authorization:`Bearer ${provisioning.secret}`}})).status,401);
    assert.equal((await fetch(`${newOrigin}/oidc/me`,{headers:{authorization:`Bearer ${beforeTokens.access_token}`}})).status,401);
    const registered=await (await fetch(`${newOrigin}/api/v1/identity/clients`,{headers:recovered.headers})).json() as {items:{id:string}[]};assert.ok(registered.items.some(c=>c.id===registeredClient.id));
    const configuration=await oidc.discovery(new URL(`${newOrigin}/oidc`),registeredClient.id,registeredClient.secret,oidc.ClientSecretBasic(registeredClient.secret!),{execute:[oidc.allowInsecureRequests]});
    const afterClient={...registeredClient,config:configuration};assert.ok((await exchange(afterClient,await authorize(newOrigin,recovered,afterClient))).id_token);
    assert.equal(await (await fetch(`${newOrigin}/api/v1/files/${file.fileId}/content`, { headers: recovered.headers })).text(), "This content must survive a full restore.");
    const afterPim = await (await fetch(`${newOrigin}/api/v1/contacts/books`, { headers: recovered.headers })).json() as typeof beforePim;
    assert.deepEqual(afterPim.items.map(row => row.id).sort(), beforePim.items.map(row => row.id).sort());
    if (sampleBook) assert.equal(await (await fetch(`${newOrigin}/api/v1/contacts/books/${sampleBook.id}/export`, { headers: recovered.headers })).text(), beforeCards);
    if (eventProof) { const event = await (await fetch(`${newOrigin}/api/v1/calendar/calendars/${eventProof.collectionId}/items/${eventProof.href}`, { headers: recovered.headers })).json() as { content: string }; assert.equal(event.content, eventProof.content); }
    await assert.rejects(restore(target, directory, root), /Another runtime/);
  } finally { await runtime.close(); }
  await assert.rejects(access(path.join(root, "runtime.lock")));
});
