#!/usr/bin/env node
// Bundles Dar's Markdown rich renderers (KaTeX MathML and Mermaid diagrams) into two
// standalone ES modules in the output directory:
//   `node crates/client/render/build.mjs <out-dir>`
// `cargo xtask` (`xtask/src/client.rs`) runs it and content-hashes each bundle into the
// client dist; the Nix package (`nix/package.nix`) runs it in its npm build step.
import {build} from "esbuild";
import path from "node:path";

const outDir = process.argv[2];
if (!outDir) throw new Error("usage: build.mjs <out-dir>");
const dir = path.resolve(outDir);

// Mermaid v12 bundles elkjs, whose Node entry references `web-worker`; shim it for the
// browser bundle so esbuild never fails to resolve it.
const ignoreWebWorker = {
  name: "ignore-web-worker",
  setup(b) {
    b.onResolve({filter: /^web-worker$/}, (args) => ({
      path: args.path,
      namespace: "web-worker-shim",
    }));
    b.onLoad({filter: /.*/, namespace: "web-worker-shim"}, () => ({
      contents: "export default {};",
      loader: "js",
    }));
  },
};

const common = {
  bundle: true,
  splitting: false,
  format: "esm",
  platform: "browser",
  minify: true,
  legalComments: "eof",
  logLevel: "warning",
};

await Promise.all([
  build({
    ...common,
    target: "es2022",
    entryPoints: [path.join(import.meta.dirname, "katex.js")],
    outfile: path.join(dir, "katex.js"),
  }),
  build({
    ...common,
    target: "es2024",
    plugins: [ignoreWebWorker],
    entryPoints: [path.join(import.meta.dirname, "mermaid.js")],
    outfile: path.join(dir, "mermaid.js"),
  }),
]);
