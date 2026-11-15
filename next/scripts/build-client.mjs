import { build } from "esbuild";
import { mkdir, rm, writeFile, copyFile, readdir, readFile } from "node:fs/promises";
import { brotliCompressSync, gzipSync, constants } from "node:zlib";
import path from "node:path";
const root = path.resolve(import.meta.dirname, "..");
const dist = path.join(root, "dist");
// Hashed names are cached forever by browsers, so stale outputs are removed on every build.
await rm(dist, { recursive: true, force: true });
await mkdir(dist, { recursive: true });
const result = await build({ absWorkingDir: root, entryPoints: {app:path.join(root,"client/main.tsx"),sdk:path.join(root,"sdk/client.tsx")}, bundle: true, outdir: dist, splitting:true, entryNames:"[name]-[hash]", chunkNames:"chunks/[name]-[hash]", assetNames:"chunks/[name]-[hash]", metafile:true, format: "esm", platform: "browser", target: "es2023", minify: true, sourcemap: false, jsx: "automatic", define: { "process.env.NODE_ENV": '"production"' } });
const entry = source => {
  const found = Object.entries(result.metafile.outputs).find(([, output]) => output.entryPoint === source);
  if (!found?.[1].cssBundle) throw new Error(`Missing output for ${source}.`);
  return { js: path.basename(found[0]), css: path.basename(found[1].cssBundle) };
};
const app = entry("client/main.tsx"), sdk = entry("sdk/client.tsx");
// Installed apps import the shared runtime from the stable /sdk.js and /sdk.css URLs;
// server-rendered identity pages link the stable /app.css.
await writeFile(path.join(dist, "sdk.js"), `export * from "./${sdk.js}";\n`);
await copyFile(path.join(dist, sdk.css), path.join(dist, "sdk.css"));
await copyFile(path.join(dist, app.css), path.join(dist, "app.css"));
await writeFile(path.join(dist, "index.html"), `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><title>Dar</title><link rel="stylesheet" href="/${app.css}"><script type="module" src="/${app.js}"></script></head><body><div id="root"></div></body></html>`);
// Lazy chunks repeat CSS that the app stylesheet already contains; nothing loads these copies.
for (const file of Object.keys(result.metafile.outputs)) if (file.endsWith(".css") && path.dirname(path.resolve(root, file)) === path.join(dist, "chunks")) await rm(path.resolve(root, file));
// The server sends these precompressed copies to browsers that accept them.
for (const entry of await readdir(dist, { recursive: true, withFileTypes: true })) {
  if (!entry.isFile() || !/\.(?:js|css|html)$/.test(entry.name)) continue;
  const file = path.join(entry.parentPath, entry.name), bytes = await readFile(file);
  await writeFile(`${file}.br`, brotliCompressSync(bytes, { params: { [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY, [constants.BROTLI_PARAM_SIZE_HINT]: bytes.length } }));
  await writeFile(`${file}.gz`, gzipSync(bytes, { level: 9 }));
}
