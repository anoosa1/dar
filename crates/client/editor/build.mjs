#!/usr/bin/env node
// Bundles the Notes editor (notes-editor.js with CodeMirror) into one ES module at the path
// given: `node crates/client/editor/build.mjs <out.js>`. `cargo xtask` (xtask/src/client.rs)
// runs it and names the result after its contents; the Nix package runs it in its npm build.
import {build} from "esbuild";
import path from "node:path";

const out = process.argv[2];
if (!out) throw new Error("usage: build.mjs <out.js>");
await build({
  entryPoints: [path.join(import.meta.dirname, "notes-editor.js")],
  outfile: path.resolve(out),
  bundle: true,
  format: "esm",
  platform: "browser",
  target: "es2022",
  minify: true,
  legalComments: "eof",
  logLevel: "warning",
});
