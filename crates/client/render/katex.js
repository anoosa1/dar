// KaTeX MathML renderer for Dar's Markdown previews (`crates/client/src/rich_render.rs`).
// Bundled with esbuild (`build.mjs`) into `katex-render.<hash>.js` and loaded lazily the
// first time a preview contains math. Uses `output: "mathml"` so no KaTeX stylesheet or
// web fonts are needed, and parses in a detached document so no inline styles hit the CSP.

import katex from "katex";
import {applyStyleDeclarations} from "./style.js";

/// Renders `tex` into `host` as MathML. Throws on invalid TeX so the caller keeps the
/// accessible source fallback visible.
export function render(host, tex, displayMode) {
  const html = katex.renderToString(String(tex ?? ""), {
    displayMode: !!displayMode,
    output: "mathml",
    throwOnError: true,
    strict: "ignore",
    trust: false,
  });
  const doc = new DOMParser().parseFromString(html, "text/html");
  for (const styleEl of Array.from(doc.body.querySelectorAll("style"))) {
    styleEl.remove();
  }
  const pending = [];
  for (const el of Array.from(doc.body.querySelectorAll("[style]"))) {
    const raw = el.getAttribute("style");
    el.removeAttribute("style");
    if (raw) pending.push([el, raw]);
  }
  const nodes = Array.from(doc.body.childNodes).map((node) => document.adoptNode(node));
  for (const [el, raw] of pending) {
    applyStyleDeclarations(el, raw);
  }
  host.replaceChildren(...nodes);
}
