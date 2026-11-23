// Dar's locator engine, installed in every frame of a test page (browser/page.rs). It resolves
// a locator's steps the way Playwright does: CSS, role and accessible name (Chromium's own,
// from computedRole/computedName), text, label, placeholder, title, filters and nth, and
// descends into same-origin frames. Visibility and hit testing follow Playwright's rules.
(() => {
  if (globalThis.__darEngine) return;
  const norm = value => String(value ?? "").replace(/[​­]/g, "").replace(/\s+/g, " ").trim();
  const matches = (text, pattern) => {
    if (!pattern) return true;
    if (pattern.kind === "regex") return new RegExp(pattern.source, pattern.flags).test(norm(text));
    const value = norm(pattern.value), actual = norm(text);
    return pattern.kind === "exact" ? actual === value : actual.toLowerCase().includes(value.toLowerCase());
  };
  const style = element => element.ownerDocument.defaultView.getComputedStyle(element);
  function visible(element) {
    if (!element.isConnected) return false;
    const computed = style(element);
    if (computed.display === "contents") {
      for (let child = element.firstChild; child; child = child.nextSibling) {
        if (child.nodeType === 1 && visible(child)) return true;
        if (child.nodeType === 3 && norm(child.textContent)) {
          const range = element.ownerDocument.createRange(); range.selectNodeContents(child);
          const rect = range.getBoundingClientRect(); if (rect.width > 0 && rect.height > 0) return true;
        }
      }
      return false;
    }
    if (!element.checkVisibility()) return false;
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0 && computed.visibility === "visible";
  }
  const parentOf = node => node.parentElement ?? (node.parentNode instanceof ShadowRoot ? node.parentNode.host : null);
  function hiddenForAria(element) {
    for (let node = element; node; node = parentOf(node)) if (node.getAttribute("aria-hidden") === "true") return true;
    if (element.localName === "option" && element.closest("select")) return false;
    return !element.checkVisibility() || style(element).visibility !== "visible";
  }
  const ROLE_ALIASES = {image: "img", graphics_document: "graphics-document"};
  const role = element => { const value = element.computedRole || ""; return ROLE_ALIASES[value] ?? value; };
  const text = element => {
    if (element.localName === "input" && ["button", "submit", "reset"].includes(element.type)) return element.value;
    return element.textContent ?? "";
  };
  const SKIP_TEXT = new Set(["script", "style", "noscript", "template", "head", "title"]);
  function all(root) {
    const out = [];
    const walk = node => {
      for (let child = node.firstElementChild; child; child = child.nextElementSibling) {
        out.push(child); if (child.shadowRoot) walk(child.shadowRoot); walk(child);
      }
    };
    walk(root); return out;
  }
  const disabled = element => {
    if (["button", "input", "select", "textarea", "option", "optgroup", "fieldset"].includes(element.localName) && element.matches(":disabled")) return true;
    for (let node = element; node; node = parentOf(node)) if (node.getAttribute("aria-disabled") === "true") return true;
    return false;
  };
  const checkedState = element => {
    const aria = element.getAttribute("aria-checked");
    if (aria === "true") return true; if (aria === "mixed") return "mixed"; if (aria === "false") return false;
    if (element.localName === "input" && ["checkbox", "radio"].includes(element.type)) return element.indeterminate ? "mixed" : element.checked;
    return false;
  };
  const level = element => {
    const aria = Number(element.getAttribute("aria-level")); if (aria) return aria;
    const match = /^h([1-6])$/.exec(element.localName); return match ? Number(match[1]) : 0;
  };
  const labels = element => {
    const out = [];
    const ids = element.getAttribute("aria-labelledby");
    if (ids) for (const id of ids.split(/\s+/)) { const label = element.getRootNode().getElementById?.(id) ?? element.ownerDocument.getElementById(id); if (label) out.push(label.textContent); }
    if (element.labels) for (const label of element.labels) out.push(label.textContent);
    const aria = element.getAttribute("aria-label"); if (aria) out.push(aria);
    return out;
  };
  function roleStep(scope, step) {
    return all(scope).filter(element => {
      if (role(element) !== step.role) return false;
      if (!step.includeHidden && hiddenForAria(element)) return false;
      if (step.level !== undefined && level(element) !== step.level) return false;
      if (step.checked !== undefined && checkedState(element) !== step.checked) return false;
      if (step.pressed !== undefined && (element.getAttribute("aria-pressed") === "true") !== step.pressed) return false;
      if (step.selected !== undefined && ((element.getAttribute("aria-selected") ?? String(!!element.selected)) === "true") !== step.selected) return false;
      if (step.expanded !== undefined && (element.getAttribute("aria-expanded") === "true") !== step.expanded) return false;
      if (step.disabled !== undefined && disabled(element) !== step.disabled) return false;
      return matches(element.computedName, step.name);
    });
  }
  function textStep(scope, pattern) {
    const out = [];
    for (const element of all(scope)) {
      if (SKIP_TEXT.has(element.localName) || !matches(text(element), pattern)) continue;
      let child = false;
      for (let node = element.firstElementChild; node && !child; node = node.nextElementSibling) child = !SKIP_TEXT.has(node.localName) && matches(text(node), pattern);
      if (!child) out.push(element);
    }
    return out;
  }
  // Same-origin frames are searched as part of their iframe (frameLocator).
  function enter(element) {
    const document = element.contentDocument;
    if (!document) throw new Error("The frame is not same-origin, so its content cannot be located.");
    return document.documentElement ? [document.documentElement] : [];
  }
  function apply(scopes, step) {
    const unique = list => [...new Set(list)];
    switch (step.kind) {
      case "css": return unique(scopes.flatMap(scope => [...scope.querySelectorAll(step.selector)]));
      case "role": return unique(scopes.flatMap(scope => roleStep(scope, step)));
      case "text": return unique(scopes.flatMap(scope => textStep(scope, step.pattern)));
      case "label": return unique(scopes.flatMap(scope => all(scope).filter(element => labels(element).some(label => matches(label, step.pattern)))));
      case "placeholder": return unique(scopes.flatMap(scope => all(scope).filter(element => element.hasAttribute("placeholder") && matches(element.getAttribute("placeholder"), step.pattern))));
      case "title": return unique(scopes.flatMap(scope => all(scope).filter(element => element.hasAttribute("title") && matches(element.getAttribute("title"), step.pattern))));
      case "frame": return scopes.flatMap(enter);
      case "hasText": return scopes.filter(element => matches(text(element), step.pattern));
      case "hasNotText": return scopes.filter(element => !matches(text(element), step.pattern));
      case "has": return scopes.filter(element => resolve([element], step.steps).length > 0);
      case "hasNot": return scopes.filter(element => resolve([element], step.steps).length === 0);
      case "visible": return scopes.filter(element => visible(element) === step.value);
      case "nth": { const index = step.index < 0 ? scopes.length + step.index : step.index; return scopes[index] ? [scopes[index]] : []; }
      case "or": return [...new Set([...scopes, ...resolve(roots(), step.steps)])].sort((a, b) => a === b ? 0 : a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1);
      case "and": { const other = new Set(resolve(roots(), step.steps)); return scopes.filter(element => other.has(element)); }
      default: throw new Error(`Unknown locator step ${step.kind}`);
    }
  }
  const roots = () => [document.documentElement];
  function resolve(scopes, steps) {
    let current = scopes;
    for (const step of steps) current = apply(current, step);
    return current;
  }
  // Coordinates in the top-level viewport, through any frames.
  function box(element) {
    const rect = element.getBoundingClientRect();
    let x = rect.x, y = rect.y;
    for (let win = element.ownerDocument.defaultView; win && win.frameElement; win = win.parent) {
      const frame = win.frameElement, outer = frame.getBoundingClientRect(), computed = style(frame);
      x += outer.x + parseFloat(computed.borderLeftWidth) + parseFloat(computed.paddingLeft);
      y += outer.y + parseFloat(computed.borderTopWidth) + parseFloat(computed.paddingTop);
    }
    return {x, y, width: rect.width, height: rect.height};
  }
  // What receives a click at (x, y) in the top-level viewport: null when it is the element or
  // inside it, otherwise a short description of what covers it.
  function hitTarget(element, x, y) {
    let doc = document, hit = null;
    for (;;) {
      hit = doc.elementFromPoint(x, y);
      if (hit && hit !== element && hit.localName === "iframe" && hit.contentDocument) {
        const outer = hit.getBoundingClientRect(), computed = style(hit);
        x -= outer.x + parseFloat(computed.borderLeftWidth) + parseFloat(computed.paddingLeft);
        y -= outer.y + parseFloat(computed.borderTopWidth) + parseFloat(computed.paddingTop);
        doc = hit.contentDocument; continue;
      }
      break;
    }
    for (let node = hit; node; node = node.parentNode instanceof ShadowRoot ? node.parentNode.host : node.parentNode) if (node === element) return null;
    if (!hit) return "nothing";
    const classes = typeof hit.className === "string" && hit.className.trim() ? "." + hit.className.trim().split(/\s+/).join(".") : "";
    return hit.localName + (hit.id ? "#" + hit.id : "") + classes;
  }
  globalThis.__darEngine = {
    resolve: steps => resolve(roots(), steps), visible, hiddenForAria, box, hitTarget, disabled, checkedState, text, norm,
    describe: element => { const html = element.outerHTML; return html.length > 160 ? html.slice(0, 160) + "…" : html; },
    editable: element => !disabled(element) && !element.readOnly && (element.isContentEditable || ["input", "textarea", "select"].includes(element.localName)),
    // A box that stays the same over two animation frames, like Playwright's stability check.
    stable: element => new Promise(resolve => requestAnimationFrame(() => { const first = box(element); requestAnimationFrame(() => { const second = box(element); resolve(first.x === second.x && first.y === second.y && first.width === second.width && first.height === second.height); }); })),
  };
})();
