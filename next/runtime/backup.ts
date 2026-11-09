import { constants } from "node:fs";
import { mkdir, open, readFile, readdir, lstat } from "node:fs/promises";
import { createHash, randomUUID } from "node:crypto";
import path from "node:path";
import type { Database } from "./database.ts";
import { createDurableFile } from "./atomic.ts";
import { blobPath } from "./files/blobs.ts";
import { openRoot } from "./root.ts";
import { lockRoot } from "./lock.ts";

interface Manifest { format: string; id: string; blobs: { id: string; size: number; digest: string }[] }
async function copyVerified(source: string, destination: string, expected?: { size: number; digest: string }) {
  const input = await open(source, constants.O_RDONLY | constants.O_NOFOLLOW);
  const output = await open(destination, constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY | constants.O_NOFOLLOW, 0o600);
  try {
    if (!(await input.stat()).isFile()) throw new Error("Backup content is not a regular file.");
    const hash = createHash("sha256"), buffer = Buffer.alloc(128 * 1024); let size = 0;
    while (true) {
      const { bytesRead } = await input.read(buffer);
      if (!bytesRead) break;
      const chunk = buffer.subarray(0, bytesRead); hash.update(chunk); size += bytesRead;
      let offset = 0; while (offset < bytesRead) offset += (await output.write(chunk, offset, bytesRead - offset)).bytesWritten;
    }
    const digest = hash.digest("hex");
    if (expected && (expected.size !== size || expected.digest !== digest)) throw new Error("Backup content failed its integrity check.");
    await output.sync();
  } finally { await input.close(); await output.close(); }
}
async function syncDirectory(directory: string) {
  const file = await open(directory, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
  try { await file.sync(); } finally { await file.close(); }
}

/** A consistent database snapshot pins its blobs until independent durable copies exist. */
export async function backup(db: Database, root: string, destination: string) {
  const id = randomUUID();
  await mkdir(destination, { mode: 0o700 });
  const snapshot = await db.call<Manifest>("service_backup", [id]);
  await createDurableFile(path.join(destination, "pending.json"), JSON.stringify({ format: "dar-backup-v1", id }));
  await mkdir(path.join(destination, "blobs"), { mode: 0o700 });
  for (const blob of snapshot.blobs) await copyVerified(blobPath(root, blob.id), blobPath(destination, blob.id), blob);
  await copyVerified(path.join(root, "secrets/signing-key.json"), path.join(destination, "signing-key.json"));
  await copyVerified(path.join(root, "secrets/identity-keyring.key"), path.join(destination, "identity-keyring.key"));
  if(["dar-backup-v3","dar-backup-v4","dar-backup-v5","dar-backup-v6"].includes(snapshot.format))await copyVerified(path.join(root,"secrets/oidc-keyring.json"),path.join(destination,"oidc-keyring.json"));
  const text = JSON.stringify(snapshot);
  await createDurableFile(path.join(destination, "snapshot.json"), text);
  await syncDirectory(path.join(destination, "blobs"));
  await createDurableFile(path.join(destination, "complete.json"), JSON.stringify({ format: "dar-backup-v1", snapshotDigest: createHash("sha256").update(text).digest("hex") }));
  await syncDirectory(destination); await syncDirectory(path.dirname(destination));
  // Completed backups own their byte copies and no longer depend on the live
  // blob directory. A failed backup keeps its pins until an operator releases it.
  await db.call("service_release_backup", [id]);
  return { id, blobs: snapshot.blobs.length };
}

/** Restore only into empty database/content storage; browser sessions are never restored. */
export async function restore(db: Database, source: string, root: string) {
  if (!(await lstat(path.join(source, "snapshot.json"))).isFile() || !(await lstat(path.join(source, "complete.json"))).isFile()) throw new Error("Unsafe backup manifest.");
  const text = await readFile(path.join(source, "snapshot.json"), "utf8");
  const complete = JSON.parse(await readFile(path.join(source, "complete.json"), "utf8"));
  if (complete.format !== "dar-backup-v1" || complete.snapshotDigest !== createHash("sha256").update(text).digest("hex")) throw new Error("Incomplete or corrupted backup.");
  const snapshot = JSON.parse(text) as Manifest;
  if (!["dar-backup-v1", "dar-backup-v2", "dar-backup-v3", "dar-backup-v4", "dar-backup-v5", "dar-backup-v6"].includes(snapshot.format)) throw new Error("Unsupported backup format.");
  await openRoot(root); const release = await lockRoot(root);
  try {
    const bootstrapFiles = new Set(["bootstrap-token", "database-token"]);
    if ((await readdir(path.join(root, "blobs"))).length || (await readdir(path.join(root,"agent-workspaces"))).length || (await readdir(path.join(root, "secrets"))).some(name => !bootstrapFiles.has(name))) throw new Error("Restore requires an empty content and identity root.");
    for (const blob of snapshot.blobs) await copyVerified(blobPath(source, blob.id), blobPath(root, blob.id), blob);
    await copyVerified(path.join(source, "signing-key.json"), path.join(root, "secrets/signing-key.json"));
    await copyVerified(path.join(source, "identity-keyring.key"), path.join(root, "secrets/identity-keyring.key"));
    if(["dar-backup-v3","dar-backup-v4","dar-backup-v5","dar-backup-v6"].includes(snapshot.format))await copyVerified(path.join(source,"oidc-keyring.json"),path.join(root,"secrets/oidc-keyring.json"));
    await syncDirectory(path.join(root, "blobs")); await syncDirectory(path.join(root, "secrets"));
    await db.call("service_restore", [text]);
  } finally { await release(); }
  return { blobs: snapshot.blobs.length };
}

/** Portable content plus a readable hierarchy manifest, without account secrets. */
export async function exportFiles(db: Database, root: string, username: string, destination: string) {
  const id = randomUUID();
  const value = await db.call<{ username: string; files: { id: string; parentId: string; name: string; kind: string; blobId: string; size: number; mediaType: string; modifiedAt: number }[]; blobs: { id: string; size: number; digest: string }[] }>("service_export", [id, username]);
  try {
    await mkdir(destination, { mode: 0o700 }); await mkdir(path.join(destination, "files"), { mode: 0o700 });
    const manifest = [];
    for (const entry of value.files) {
      if (!/^[a-f0-9-]{36}$/.test(entry.id) || /[\x00/\\]/.test(entry.name) || entry.name === "." || entry.name === "..") throw new Error("Unsafe export metadata.");
      const { blobId, ...metadata } = entry;
      let contentFile: string | undefined;
      if (entry.kind === "file") {
        let shortName = "";
        for (const character of entry.name) { if (Buffer.byteLength(shortName + character) > 190) break; shortName += character; }
        const extension = shortName !== entry.name ? /\.[a-zA-Z0-9]{1,12}$/.exec(entry.name)?.[0] ?? "" : "";
        contentFile = `files/${entry.id}-${shortName}${extension}`;
        const blob = value.blobs.find(blob => blob.id === blobId)!;
        await copyVerified(blobPath(root, blobId), path.join(destination, contentFile), blob);
      }
      manifest.push({ ...metadata, contentFile });
    }
    await createDurableFile(path.join(destination, "manifest.json"), JSON.stringify({ format: "dar-files-export-v1", exportId: id, username: value.username, exportedAt: new Date().toISOString(), files: manifest }, null, 2));
    await syncDirectory(path.join(destination, "files")); await syncDirectory(destination);
    return { files: manifest.length };
  } finally { await db.call("service_release_backup", [id]); }
}
