// Shared CSP-safe CSSOM helpers for the Markdown rich renderers (KaTeX and Mermaid).
// Dar's CSP forbids inline `style="..."` attributes (`style-src 'self'`), whereas CSSOM
// `element.style.setProperty()` from a same-origin module is allowed.

/// Parses a CSS declaration list (as found in a `style="..."` attribute) and applies each
/// property to `element` via CSSOM `setProperty`.
export function applyStyleDeclarations(element, raw) {
  if (!element || !element.style || typeof element.style.setProperty !== "function" || !raw) return;
  let start = 0;
  let quote = "";
  let parens = 0;

  const applyOne = (chunk) => {
    const colon = chunk.indexOf(":");
    if (colon < 0) return;
    const prop = chunk.slice(0, colon).trim();
    let val = chunk.slice(colon + 1).trim();
    if (!prop || !val) return;
    let priority = "";
    if (/!important\s*$/i.test(val)) {
      priority = "important";
      val = val.replace(/!important\s*$/i, "").trim();
    }
    try {
      element.style.setProperty(prop, val, priority);
    } catch {
      // Ignore invalid or unsupported declarations.
    }
  };

  for (let i = 0; i < raw.length; i++) {
    const ch = raw[i];
    if (quote) {
      if (ch === "\\" && i + 1 < raw.length) {
        i++;
      } else if (ch === quote) {
        quote = "";
      }
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (ch === "(") {
      parens++;
    } else if (ch === ")" && parens > 0) {
      parens--;
    } else if (ch === ";" && parens === 0) {
      applyOne(raw.slice(start, i));
      start = i + 1;
    }
  }
  if (start < raw.length) applyOne(raw.slice(start));
}
