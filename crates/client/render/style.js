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

export const DATA_STYLE_ATTR = "data-dar-style";

/// Rewrites `style=...` attributes inside opening tags to `data-dar-style=...` before
/// passing markup to `DOMParser`, because Chromium enforces `style-src 'self'` while
/// parsing inline `style` attributes even in a detached document.
export function neutralizeInlineStyleAttrs(markup) {
  const input = typeof markup === "string" ? markup : String(markup ?? "");
  if (!/style\s*=/i.test(input)) return input;
  let out = "";
  let i = 0;
  const n = input.length;
  while (i < n) {
    const lt = input.indexOf("<", i);
    if (lt < 0) {
      out += input.slice(i);
      break;
    }
    out += input.slice(i, lt);
    if (input.startsWith("<!--", lt)) {
      const end = input.indexOf("-->", lt + 4);
      const next = end < 0 ? n : end + 3;
      out += input.slice(lt, next);
      i = next;
      continue;
    }
    const nextCh = input[lt + 1] || "";
    if (!/[a-zA-Z]/.test(nextCh)) {
      out += "<";
      i = lt + 1;
      continue;
    }
    let j = lt + 1;
    while (j < n && /[\w:-]/.test(input[j])) j++;
    out += input.slice(lt, j);
    while (j < n && input[j] !== ">") {
      const wsStart = j;
      while (j < n && /[\s/]/.test(input[j])) j++;
      out += input.slice(wsStart, j);
      if (j >= n || input[j] === ">") break;
      const nameStart = j;
      while (j < n && !/[\s=/>]/.test(input[j])) j++;
      const attrName = input.slice(nameStart, j);
      const afterName = j;
      while (j < n && /\s/.test(input[j])) j++;
      if (j < n && input[j] === "=") {
        out += attrName.toLowerCase() === "style" ? DATA_STYLE_ATTR : attrName;
        out += input.slice(afterName, j + 1);
        j++;
        const preVal = j;
        while (j < n && /\s/.test(input[j])) j++;
        out += input.slice(preVal, j);
        if (j < n && (input[j] === '"' || input[j] === "'")) {
          const quote = input[j];
          const valStart = j;
          j++;
          while (j < n && input[j] !== quote) j++;
          if (j < n) j++;
          out += input.slice(valStart, j);
        } else {
          const valStart = j;
          while (j < n && !/[\s>]/.test(input[j])) j++;
          out += input.slice(valStart, j);
        }
      } else {
        out += attrName + input.slice(afterName, j);
      }
    }
    if (j < n && input[j] === ">") {
      out += ">";
      j++;
    }
    i = j;
  }
  return out;
}
