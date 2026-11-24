// Mermaid diagram renderer for Dar's Markdown previews (`crates/client/src/rich_render.rs`).
// Bundled with esbuild (`build.mjs`) into `mermaid-render.<hash>.js` and loaded lazily the
// first time a preview contains a ```mermaid block.
//
// Dar's CSP enforces `style-src 'self'` (no inline `<style>` elements or `style="..."`
// attributes). During `mermaid.render()`, Mermaid inserts a temporary `<style>` element and
// calls `setAttribute("style", ...)` inside its scratch container; we intercept those into a
// temporary constructed stylesheet and CSSOM `setProperty()` calls, then parse the returned
// SVG with `DOMParser`, move its `<style>` rules into a per-diagram ShadowRoot's
// `adoptedStyleSheets`, and apply any remaining `style` attributes via CSSOM before mounting.

import mermaid from "mermaid";
import {applyStyleDeclarations} from "./style.js";

let nextId = 0;
let queue = Promise.resolve();
const activeDiagrams = new Set();
let themeObserverStarted = false;

function enqueue(fn) {
  const run = queue.then(fn, fn);
  queue = run.then(() => {}, () => {});
  return run;
}

function isDarkTheme() {
  const attr = document.documentElement.getAttribute("data-theme");
  if (attr === "mocha") return true;
  if (attr === "latte") return false;
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
}

function rgbToHex(color, fallback) {
  const match = /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)/.exec(color);
  if (!match) return fallback;
  const hex = (value) => Math.max(0, Math.min(255, Number(value))).toString(16).padStart(2, "0");
  return `#${hex(match[1])}${hex(match[2])}${hex(match[3])}`;
}

/// Resolves Dar's `light-dark(...)` CSS custom properties to concrete `#rrggbb` values so
/// Mermaid's color engine (`khroma`) can derive borders and shades without parsing `light-dark()`.
function configureMermaid(scratch) {
  const dark = isDarkTheme();
  const probe = document.createElement("span");
  scratch.appendChild(probe);
  const color = (varName, fallback) => {
    probe.style.setProperty("color", `var(${varName})`);
    return rgbToHex(getComputedStyle(probe).color, fallback);
  };

  const panel = color("--panel", dark ? "#301c2b" : "#fffafb");
  const surface = color("--surface-soft", dark ? "#26111f" : "#fbf4f6");
  const raised = color("--raised", dark ? "#3a2536" : "#ffffff");
  const text = color("--text", dark ? "#faeaf3" : "#352138");
  const muted = color("--muted", dark ? "#cfb4ca" : "#755d74");
  const border = color("--border", dark ? "#80617a" : "#c9b4c7");
  const accent = color("--accent", dark ? "#f2a2a9" : "#73396f");
  const tonal = color("--tonal", dark ? "#4d3c73" : "#ede3f5");
  const onTonal = color("--on-tonal", dark ? "#f3eafd" : "#4d3c73");
  probe.remove();

  const fontFamily = getComputedStyle(document.body).fontFamily || "system-ui, sans-serif";
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    suppressErrorRendering: true,
    htmlLabels: false,
    flowchart: {htmlLabels: false},
    theme: "base",
    darkMode: dark,
    fontFamily,
    themeVariables: {
      darkMode: dark,
      fontFamily,
      background: panel,
      primaryColor: tonal,
      primaryTextColor: onTonal,
      primaryBorderColor: accent,
      secondaryColor: surface,
      secondaryTextColor: text,
      secondaryBorderColor: border,
      tertiaryColor: raised,
      tertiaryTextColor: text,
      tertiaryBorderColor: border,
      lineColor: muted,
      textColor: text,
      mainBkg: tonal,
      nodeBorder: accent,
      clusterBkg: surface,
      clusterBorder: border,
      edgeLabelBackground: panel,
    },
  });
}

function stripImports(css) {
  return css.replace(/@import\s+[^;]+;/gi, "");
}

/// Runs `mermaid.render()` inside a hidden Light DOM scratch container while redirecting
/// temporary `<style>` insertions and `setAttribute("style", ...)` calls to CSP-safe CSSOM.
async function renderSvgCspSafe(id, source, scratch) {
  const capturedStyles = [];
  const tempSheet =
    typeof CSSStyleSheet === "function" && "adoptedStyleSheets" in Document.prototype
      ? new CSSStyleSheet()
      : null;
  if (tempSheet) {
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, tempSheet];
  }

  const origSetAttr = Element.prototype.setAttribute;
  const origInsertBefore = Node.prototype.insertBefore;
  const origAppendChild = Node.prototype.appendChild;

  const syncTempSheet = () => {
    if (!tempSheet) return;
    const css = stripImports(capturedStyles.map((s) => s.textContent || "").join("\n"));
    try {
      tempSheet.replaceSync(css);
    } catch {
      // Fall back to page font metrics if Mermaid emits a non-standard rule.
    }
  };

  const isScratchNode = (node) => node === scratch || (node instanceof Node && scratch.contains(node));

  const interceptStyle = (parent, child) => {
    if (
      child &&
      child.nodeType === 1 &&
      child.localName === "style" &&
      (isScratchNode(parent) || (!parent.isConnected && parent.nodeType === 1 && parent.localName === "svg"))
    ) {
      capturedStyles.push(child);
      syncTempSheet();
      return true;
    }
    return false;
  };

  Element.prototype.setAttribute = function (name, value) {
    if (
      typeof name === "string" &&
      name.toLowerCase() === "style" &&
      (!this.isConnected || isScratchNode(this)) &&
      this.style &&
      typeof this.style.setProperty === "function"
    ) {
      this.removeAttribute("style");
      applyStyleDeclarations(this, String(value ?? ""));
      return;
    }
    return origSetAttr.call(this, name, value);
  };

  Node.prototype.insertBefore = function (child, ref) {
    if (interceptStyle(this, child)) return child;
    return origInsertBefore.call(this, child, ref);
  };

  Node.prototype.appendChild = function (child) {
    if (interceptStyle(this, child)) return child;
    return origAppendChild.call(this, child);
  };

  try {
    const result = await mermaid.render(id, source, scratch);
    const capturedCss = capturedStyles.map((s) => s.textContent || "").join("\n");
    return {svg: result.svg, capturedCss};
  } finally {
    Element.prototype.setAttribute = origSetAttr;
    Node.prototype.insertBefore = origInsertBefore;
    Node.prototype.appendChild = origAppendChild;
    if (tempSheet) {
      document.adoptedStyleSheets = document.adoptedStyleSheets.filter((s) => s !== tempSheet);
    }
  }
}

/// Parses the rendered SVG with `DOMParser`, moves `<style>` rules into a ShadowRoot's
/// `adoptedStyleSheets`, strips all `style="..."` attributes and re-applies them via CSSOM.
function mountDiagram(host, svgMarkup, capturedCss) {
  const doc = new DOMParser().parseFromString(svgMarkup, "image/svg+xml");
  if (doc.querySelector("parsererror")) {
    throw new Error("Mermaid produced invalid SVG.");
  }
  const svgEl = doc.documentElement;
  if (!svgEl || svgEl.localName !== "svg") {
    throw new Error("Mermaid did not produce an SVG element.");
  }

  let css = capturedCss ? `${capturedCss}\n` : "";
  for (const styleEl of Array.from(svgEl.querySelectorAll("style"))) {
    css += `${styleEl.textContent || ""}\n`;
    styleEl.remove();
  }
  const safeCss = stripImports(css);

  const styledNodes = [svgEl, ...Array.from(svgEl.querySelectorAll("[style]"))];
  const saved = new Map();
  for (const node of styledNodes) {
    const raw = node.getAttribute("style");
    if (raw !== null) {
      node.removeAttribute("style");
      if (raw.trim()) saved.set(node, raw);
    }
  }

  const shadow = host.shadowRoot || host.attachShadow({mode: "open"});
  if (typeof CSSStyleSheet === "function" && "adoptedStyleSheets" in ShadowRoot.prototype) {
    const sheet = new CSSStyleSheet();
    const baseRules = ":host{display:block;max-width:100%}svg{display:block;max-width:100%;height:auto;margin:0 auto}\n";
    try {
      sheet.replaceSync(baseRules + safeCss);
    } catch {
      sheet.replaceSync(baseRules);
    }
    shadow.adoptedStyleSheets = [sheet];
  }

  const imported = document.importNode(svgEl, true);
  if (saved.size > 0) {
    const origWalk = [svgEl, ...Array.from(svgEl.querySelectorAll("*"))];
    const impWalk = [imported, ...Array.from(imported.querySelectorAll("*"))];
    for (let i = 0; i < origWalk.length && i < impWalk.length; i++) {
      const raw = saved.get(origWalk[i]);
      if (raw) applyStyleDeclarations(impWalk[i], raw);
    }
  }
  shadow.replaceChildren(imported);
}

function pruneDisconnected() {
  for (const entry of activeDiagrams) {
    const host = entry.ref.deref();
    if (!host || !host.isConnected) {
      activeDiagrams.delete(entry);
    }
  }
}

function ensureThemeObserver() {
  if (themeObserverStarted) return;
  themeObserverStarted = true;
  const rerenderAll = () => {
    pruneDisconnected();
    for (const entry of Array.from(activeDiagrams)) {
      const host = entry.ref.deref();
      if (!host || !host.isConnected) {
        activeDiagrams.delete(entry);
        continue;
      }
      renderQueued(host, entry.source).catch(() => {});
    }
  };
  if (typeof MutationObserver === "function") {
    const observer = new MutationObserver(rerenderAll);
    observer.observe(document.documentElement, {attributes: true, attributeFilter: ["data-theme"]});
  }
  window.matchMedia?.("(prefers-color-scheme: dark)")?.addEventListener?.("change", rerenderAll);
}

function renderQueued(host, source) {
  return enqueue(async () => {
    if (!host.isConnected) return;
    const scratch = document.createElement("div");
    scratch.className = "mermaid-scratch";
    scratch.setAttribute("aria-hidden", "true");
    document.body.appendChild(scratch);
    const id = `dar-mmd-${++nextId}`;
    try {
      configureMermaid(scratch);
      const {svg, capturedCss} = await renderSvgCspSafe(id, source, scratch);
      if (!host.isConnected) return;
      mountDiagram(host, svg, capturedCss);
    } finally {
      scratch.remove();
      document.getElementById(`d${id}`)?.remove();
      document.getElementById(id)?.remove();
    }
  });
}

/// Renders a Mermaid diagram `source` into `host`'s ShadowRoot. Throws on parse or render
/// error before attaching a shadow root so the light DOM `<pre>` fallback stays visible.
export async function render(host, source) {
  const text = String(source ?? "").trim();
  if (!text) throw new Error("Empty Mermaid diagram.");
  await renderQueued(host, text);
  if (host.isConnected) {
    pruneDisconnected();
    for (const entry of activeDiagrams) {
      if (entry.ref.deref() === host) activeDiagrams.delete(entry);
    }
    activeDiagrams.add({ref: new WeakRef(host), source: text});
    ensureThemeObserver();
  }
}
