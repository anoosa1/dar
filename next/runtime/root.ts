import { lstat, mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { createDurableFile } from "./atomic.ts";

/** Fresh roots only. Every existing ancestor must be a real directory. */
export async function openRoot(root: string) {
  root = path.resolve(root);
  let component = path.parse(root).root;
  for (const segment of root.slice(component.length).split(path.sep)) {
    component = path.join(component, segment);
    try {
      const stat = await lstat(component);
      if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error("Managed storage cannot contain symlinks or non-directory ancestors.");
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      await mkdir(component, { mode: 0o700 });
    }
  }
  const marker = path.join(root, "instance.json");
  try {
    if (!(await lstat(marker)).isFile() || JSON.parse(await readFile(marker, "utf8")).format !== "dar-spacetime-v1") throw new Error("Unsupported storage root.");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    if ((await readdir(root)).length) throw new Error("Refusing an unknown non-empty storage root.");
    await createDurableFile(marker, JSON.stringify({ format: "dar-spacetime-v1" }));
  }
  for (const name of ["secrets", "blobs", "uploads"]) {
    const directory = path.join(root, name);
    await mkdir(directory, { recursive: true, mode: 0o700 });
    const stat = await lstat(directory);
    if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error("Unsafe managed storage directory.");
  }
  return root;
}
