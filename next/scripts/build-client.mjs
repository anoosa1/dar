import { build } from "esbuild";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
const root = path.resolve(import.meta.dirname, "..");
await mkdir(path.join(root, "dist"), { recursive: true });
await build({ entryPoints: {app:path.join(root,"client/main.tsx"),sdk:path.join(root,"sdk/client.tsx")}, bundle: true, outdir: path.join(root, "dist"),splitting:true,chunkNames:"chunks/[name]-[hash]", format: "esm", platform: "browser", target: "es2023", minify: true, sourcemap: false, jsx: "automatic", define: { "process.env.NODE_ENV": '"production"' } });
await writeFile(path.join(root, "dist/index.html"), '<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><title>Dar</title><link rel="stylesheet" href="/app.css"><script type="module" src="/app.js"></script></head><body><div id="root"></div></body></html>');
