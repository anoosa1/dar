import { build } from "esbuild";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
const root = path.resolve(import.meta.dirname, "..");
await mkdir(path.join(root, "dist"), { recursive: true });
await build({ entryPoints: [path.join(root, "client/main.tsx")], bundle: true, outfile: path.join(root, "dist/app.js"), format: "esm", platform: "browser", target: "es2023", minify: true, sourcemap: false, jsx: "automatic", define: { "process.env.NODE_ENV": '"production"' } });
await writeFile(path.join(root, "dist/index.html"), '<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><title>Dar</title><link rel="stylesheet" href="/app.css"><script type="module" src="/app.js"></script></head><body><div id="root"></div></body></html>');
