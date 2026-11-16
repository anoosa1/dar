import { build } from "esbuild";
import { mkdir, rm, writeFile, copyFile, readdir, readFile, cp } from "node:fs/promises";
import { brotliCompressSync, gzipSync, constants } from "node:zlib";
import path from "node:path";
const root = path.resolve(import.meta.dirname, "..");
const dist = path.join(root, "dist");
// Hashed names are cached forever by browsers, so stale outputs are removed on every build.
await rm(dist, { recursive: true, force: true });
await mkdir(dist, { recursive: true });
const result = await build({ absWorkingDir: root, entryPoints: {app:path.join(root,"client/main.tsx"),sdk:path.join(root,"sdk/client.tsx")}, bundle: true, outdir: dist, splitting:true, entryNames:"[name]-[hash]", chunkNames:"chunks/[name]-[hash]", assetNames:"chunks/[name]-[hash]", metafile:true, format: "esm", platform: "browser", target: "es2023", minify: true, sourcemap: false, jsx: "automatic", define: { "process.env.NODE_ENV": '"production"' },
  // pdf.js loads a native canvas only under Node; browsers never reach it.
  external: ["@napi-rs/canvas"] });
// pdf.js runs its parser in a worker and reads fonts, character maps, colour profiles and image
// decoders on demand. They are copied under a versioned path, so browsers may keep them for a year.
const pdfjs = path.join(root, "node_modules/pdfjs-dist"), pdfjsVersion = JSON.parse(await readFile(path.join(pdfjs, "package.json"), "utf8")).version;
await mkdir(path.join(dist, "pdfjs", pdfjsVersion), { recursive: true });
await copyFile(path.join(pdfjs, "build/pdf.worker.min.mjs"), path.join(dist, "pdfjs", pdfjsVersion, "pdf.worker.min.mjs"));
for (const folder of ["cmaps", "standard_fonts", "wasm", "iccs"]) await cp(path.join(pdfjs, folder), path.join(dist, "pdfjs", pdfjsVersion, folder), { recursive: true });
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
// Each page lists every chunk it needs as a modulepreload, so a first visit fetches
// them together instead of discovering one layer of imports at a time.
const outputs = result.metafile.outputs, relative = file => "/" + path.relative(dist, path.resolve(root, file));
const closure = (file, seen = new Set()) => { for (const item of outputs[file]?.imports ?? []) if (item.kind === "import-statement" && !item.external && !seen.has(item.path)) { seen.add(item.path); closure(item.path, seen); } return seen; };
const output = source => { const found = Object.entries(outputs).find(([file, value]) => value.entryPoint === source && file.endsWith(".js")); if (!found) throw new Error(`Missing chunk for ${source}.`); return found[0]; };
const appFile = output("client/main.tsx"), shared = closure(appFile);
const html = files => `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><title>Dar</title><link rel="stylesheet" href="/${app.css}"><script type="module" src="/${app.js}"></script>${[...files].map(file => `<link rel="modulepreload" href="${relative(file)}">`).join("")}</head><body><div id="root"></div></body></html>`;
await writeFile(path.join(dist, "index.html"), html(shared));
// Keep in step with pageFor() in client/main.tsx.
const routes = { "/": "client/files.tsx", "/photos": "client/photos/workspace.tsx", "/books": "client/books/workspace.tsx", "/contacts": "client/pim/contacts.tsx", "/calendar": "client/pim/calendar.tsx", "/notes": "client/notes.tsx", "/tasks": "client/pim/workspace.tsx", "/news": "client/news/workspace.tsx", "/agent": "client/agent/workspace.tsx", "/settings": "client/identity.tsx", "/identity": "client/identity.tsx" };
const pages = {};
await mkdir(path.join(dist, "pages"), { recursive: true });
for (const [route, source] of Object.entries(routes)) {
  const page = output(source), name = `pages/${path.basename(source, ".tsx")}-${path.basename(path.dirname(source))}.html`;
  await writeFile(path.join(dist, name), html(new Set([...shared, page, ...closure(page)])));
  pages[route] = name;
}
await writeFile(path.join(dist, "pages.json"), JSON.stringify(pages));
// Lazy chunks repeat CSS that the app stylesheet already contains; nothing loads these copies.
for (const file of Object.keys(result.metafile.outputs)) if (file.endsWith(".css") && path.dirname(path.resolve(root, file)) === path.join(dist, "chunks")) await rm(path.resolve(root, file));
// The server sends these precompressed copies to browsers that accept them.
for (const entry of await readdir(dist, { recursive: true, withFileTypes: true })) {
  if (!entry.isFile() || !/\.(?:js|mjs|css|html|bcmap)$/.test(entry.name)) continue;
  const file = path.join(entry.parentPath, entry.name), bytes = await readFile(file);
  await writeFile(`${file}.br`, brotliCompressSync(bytes, { params: { [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY, [constants.BROTLI_PARAM_SIZE_HINT]: bytes.length } }));
  await writeFile(`${file}.gz`, gzipSync(bytes, { level: 9 }));
}
