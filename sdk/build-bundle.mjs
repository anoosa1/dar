#!/usr/bin/env node
// Builds the installed-app SDK bundle into sdk/dist: /sdk.js and /sdk.css (stable URLs that
// installed apps import), their hashed entry files and shared chunks. `cargo xtask build`
// copies them into the client's dist, which the runtime serves.
import {build} from "esbuild";
import {mkdir, rm, writeFile, copyFile} from "node:fs/promises";
import path from "node:path";
const root = import.meta.dirname, dist = path.join(root, "dist");
// Hashed names are cached forever by browsers, so stale outputs are removed on every build.
await rm(dist, {recursive: true, force: true});
await mkdir(dist, {recursive: true});
const result = await build({absWorkingDir: root, entryPoints: {sdk: path.join(root, "client.tsx")}, bundle: true, outdir: dist, splitting: true,
  entryNames: "[name]-[hash]", chunkNames: "chunks/[name]-[hash]", assetNames: "chunks/[name]-[hash]", metafile: true, format: "esm", platform: "browser",
  target: "es2023", minify: true, jsx: "automatic", define: {"process.env.NODE_ENV": '"production"'}});
const [file, output] = Object.entries(result.metafile.outputs).find(([, value]) => value.entryPoint === "client.tsx");
if (!output.cssBundle) throw new Error("The SDK bundle has no stylesheet.");
await writeFile(path.join(dist, "sdk.js"), `export * from "./${path.basename(file)}";\n`);
await copyFile(path.resolve(root, output.cssBundle), path.join(dist, "sdk.css"));
console.info(`Built the app SDK bundle at ${dist}`);
