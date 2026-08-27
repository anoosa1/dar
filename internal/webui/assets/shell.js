(() => {
  "use strict";
  const menuButton = document.getElementById("dar-app-menu-button"), menu = document.getElementById("dar-app-menu"), label = document.getElementById("dar-current-app");
  const items = [...(menu?.querySelectorAll('[role="menuitem"]') || [])];
  const active = items.find(item => location.pathname === item.dataset.appPath || location.pathname.startsWith(item.dataset.appPath + "/"));
  if (active) { active.classList.add("bg-primary-container", "text-primary", "font-medium"); active.setAttribute("aria-current", "page"); if (label) label.textContent = active.dataset.appLabel; }
  const closeMenu = () => { menu?.classList.add("hidden"); menuButton?.setAttribute("aria-expanded", "false"); };
  const openMenu = () => { menu?.classList.remove("hidden"); menuButton?.setAttribute("aria-expanded", "true"); items[0]?.focus(); };
  menuButton?.addEventListener("click", () => menu.classList.contains("hidden") ? openMenu() : closeMenu());
  document.getElementById("global-apps-mobile")?.addEventListener("click", () => menu.classList.contains("hidden") ? openMenu() : closeMenu());
  menu?.addEventListener("keydown", event => { const index = items.indexOf(document.activeElement); if (event.key === "Escape") { closeMenu(); menuButton.focus(); } else if (event.key === "ArrowDown") { event.preventDefault(); items[(index + 1) % items.length].focus(); } else if (event.key === "ArrowUp") { event.preventDefault(); items[(index - 1 + items.length) % items.length].focus(); } else if (event.key === "Home") { event.preventDefault(); items[0].focus(); } else if (event.key === "End") { event.preventDefault(); items.at(-1).focus(); } });
  document.addEventListener("click", event => { if (menu && !menu.contains(event.target) && !menuButton?.contains(event.target) && event.target?.id !== "global-apps-mobile") closeMenu(); });

  const desktopInput = document.getElementById("global-search-input"), desktopPanel = document.getElementById("global-search-panel"), desktopResults = document.getElementById("global-search-results");
  const mobileButton = document.getElementById("global-search-mobile"), mobileRow = document.getElementById("global-search-mobile-row"), mobileInput = document.getElementById("global-search-mobile-input"), mobileResults = document.getElementById("global-search-mobile-results");
  let timer, controller, selected = -1;
  const node = (tag, classes, text) => { const item = document.createElement(tag); if (classes) item.className = classes; if (text !== undefined) item.textContent = text; return item; };
  const safeHref = result => { try { const url = new URL(result.url, location.href); if (url.origin !== location.origin) return "#"; if (result.match?.anchor) url.hash = result.match.anchor; return url.pathname + url.search + url.hash; } catch { return "#"; } };
  const remember = (query, result) => { sessionStorage.setItem("dar.search.query", query); if (result) sessionStorage.setItem("dar.search.highlight", JSON.stringify({ query, provider: result.provider, object_id: result.object_id, chunk_id: result.match?.chunk_id || "" })); };
  function resultLink(result, query) {
    const link = node("a", "search-suggestion flex items-start gap-3 rounded-xl px-3 py-2 text-on-surface no-underline hover:bg-surface-container-high"); link.href = safeHref(result); link.dataset.searchResult = "";
    const icon = node("i", "fa-solid fa-magnifying-glass mt-1 w-5 text-center text-primary"); const text = node("span", "min-w-0 flex-1"); text.append(node("span", "block truncate text-sm font-medium", result.title), node("span", "block truncate text-xs text-on-surface-variant", result.subtitle || result.provider)); link.append(icon, text); link.onclick = () => remember(query, result); return link;
  }
  function render(target, data, query) {
    target.replaceChildren(); if (data.stale_providers?.length) target.append(node("p", "mx-2 mb-2 rounded-xl bg-warning-container px-3 py-2 text-xs text-on-warning-container", "Some results may be out of date."));
    const sections = []; if (data.recent?.length) sections.push({ name: "Recent", results: data.recent }); for (const group of data.groups || []) if (group.results?.length) sections.push({ name: group.display_name, results: group.results });
    if (!sections.length) { target.append(node("p", "p-5 text-center text-sm text-on-surface-variant", "No matching items")); return; }
    for (const section of sections) { const box = node("section", "mb-1"), heading = node("h2", "px-3 pt-2 pb-1 text-xs font-semibold uppercase tracking-wide text-on-surface-variant", section.name); box.append(heading, ...section.results.map(result => resultLink(result, query))); target.append(box); } selected = -1;
  }
  async function search(input, target, panel) {
    const query = input.value.trim(); controller?.abort(); if (!query) { target.replaceChildren(node("p", "p-5 text-center text-sm text-on-surface-variant", "Start typing to search every app")); panel?.classList.add("hidden"); return; }
    controller = new AbortController(); panel?.classList.remove("hidden"); target.replaceChildren(node("p", "p-4 text-sm text-on-surface-variant", "Searching…"));
    try { const response = await fetch("/api/search/query", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ query, view: "suggestions" }), signal: controller.signal }); if (!response.ok) throw new Error(); render(target, await response.json(), query); } catch (error) { if (error.name !== "AbortError") target.replaceChildren(node("p", "p-4 text-sm text-error", "Search is temporarily unavailable")); }
  }
  function bind(input, target, panel) { input?.addEventListener("focus", () => { if (input.value.trim()) panel?.classList.remove("hidden"); }); input?.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(() => search(input, target, panel), 180); }); input?.addEventListener("keydown", event => { const links = [...target.querySelectorAll("[data-search-result]")]; if ((event.key === "ArrowDown" || event.key === "ArrowUp") && links.length) { event.preventDefault(); selected = (selected + (event.key === "ArrowDown" ? 1 : -1) + links.length) % links.length; links[selected]?.focus(); } else if (event.key === "Enter") { event.preventDefault(); remember(input.value.trim()); location.assign("/search"); } else if (event.key === "Escape") { panel?.classList.add("hidden"); input.blur(); } }); }
  bind(desktopInput, desktopResults, desktopPanel); bind(mobileInput, mobileResults, mobileRow);
  mobileButton?.addEventListener("click", () => { mobileRow.classList.toggle("hidden"); if (!mobileRow.classList.contains("hidden")) mobileInput.focus(); });
  document.querySelectorAll("[data-search-page]").forEach(link => link.addEventListener("click", () => remember(desktopInput?.value.trim() || mobileInput?.value.trim() || "")));
  document.addEventListener("click", event => { if (desktopPanel && !desktopPanel.contains(event.target) && event.target !== desktopInput) desktopPanel.classList.add("hidden"); });
  let highlightID = location.hash.length > 1 ? decodeURIComponent(location.hash.slice(1)) : "";
  if (!highlightID) { try { const saved = JSON.parse(sessionStorage.getItem("dar.search.highlight") || "null"), fallback = { files: "files-app", notes: "editor-shell", contacts: "contacts-app" }; if (saved && location.pathname.startsWith("/" + saved.provider)) highlightID = fallback[saved.provider] || ""; } catch {} }
  if (highlightID) { const id = highlightID; let highlighted = false; const highlight = () => { const target = document.getElementById(id); if (!target) return false; target.classList.add("ring-2", "ring-primary", "ring-offset-2", "ring-offset-surface"); target.scrollIntoView({ block: "center", behavior: "smooth" }); sessionStorage.removeItem("dar.search.highlight"); setTimeout(() => target.classList.remove("ring-2", "ring-primary", "ring-offset-2", "ring-offset-surface"), 5000); highlighted = true; return true; }; if (!highlight()) { const observer = new MutationObserver(() => { if (highlighted || highlight()) observer.disconnect(); }); observer.observe(document.body, { childList: true, subtree: true }); setTimeout(() => observer.disconnect(), 10000); } }
})();
