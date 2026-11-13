import { lstat, mkdir, readdir, readFile, realpath } from "node:fs/promises";
import path from "node:path";
import { createDurableFile } from "./atomic.ts";

/** Resolve host directory aliases; the root and managed storage cannot be symlinks. */
export async function openRoot(root: string) {
  root = path.resolve(root);
  const parent = path.dirname(root);
  await mkdir(parent, { recursive: true, mode: 0o700 });
  root = path.join(await realpath(parent), path.basename(root));
  try {
    const stat = await lstat(root);
    if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error("Managed storage root cannot be a symlink or non-directory.");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    await mkdir(root, { mode: 0o700 });
  }
  const marker = path.join(root, "instance.json");
  try {
    if (!(await lstat(marker)).isFile() || JSON.parse(await readFile(marker, "utf8")).format !== "dar-spacetime-v1") throw new Error("Unsupported storage root.");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    if ((await readdir(root)).length) throw new Error("Refusing an unknown non-empty storage root.");
    await createDurableFile(marker, JSON.stringify({ format: "dar-spacetime-v1" }));
  }
  for (const name of ["secrets", "blobs", "uploads", "agent-workspaces", "photo-cache"]) {
    const directory = path.join(root, name);
    await mkdir(directory, { recursive: true, mode: 0o700 });
    const stat = await lstat(directory);
    if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error("Unsafe managed storage directory.");
  }
  return root;
}
