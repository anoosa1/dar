(() => {
  "use strict";
  const input = document.getElementById("search-page-input");
  if (!input) return;
  const results = document.getElementById("search-page-results");
  const status = document.getElementById("search-page-status");
  const customizeButton = document.getElementById("search-customize-button");
  const customize = document.getElementById("search-customize");
  const providerList = document.getElementById("search-provider-list");
  let timer, controller, query = "", preferences = [];
  const node = (tag, classes, text) => { const item = document.createElement(tag); if (classes) item.className = classes; if (text !== undefined) item.textContent = text; return item; };
  const request = async (url, options = {}) => { const response = await fetch(url, options); if (!response.ok) throw new Error(await response.text()); return response.json(); };
  const post = (url, body, signal) => request(url, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body), signal });
  const safeHref = (result, anchor) => { try { const url = new URL(result.url, location.href); if (url.origin !== location.origin) return "#"; if (anchor || result.match?.anchor) url.hash = anchor || result.match.anchor; return url.pathname + url.search + url.hash; } catch { return "#"; } };
  function remember(result, chunk) {
    sessionStorage.setItem("dar.search.query", query);
    sessionStorage.setItem("dar.search.highlight", JSON.stringify({ query, provider: result.provider, object_id: result.object_id, chunk_id: chunk || result.match?.chunk_id || "" }));
  }
  function resultCard(result) {
    const article = node("article", "rounded-2xl px-4 py-4 hover:bg-surface-container-low");
    const top = node("div", "flex items-start gap-3");
    const link = node("a", "min-w-0 flex-1 text-on-surface no-underline");
    link.href = safeHref(result); link.onclick = () => remember(result);
    link.append(node("h3", "text-base font-medium text-primary", result.title), node("p", "mt-0.5 text-xs text-on-surface-variant", result.subtitle || ""));
    const snippet = node("p", "mt-2 text-sm leading-6 text-on-surface-variant"); snippet.innerHTML = result.match?.snippet || ""; link.append(snippet); top.append(link);
    if (result.match_count > 1) {
      const more = node("button", "h-9 shrink-0 rounded-xl px-3 text-xs text-primary hover:bg-primary-container", `${result.match_count} matches`);
      const children = node("div", "mt-3 ml-4 hidden border-l-2 border-outline-variant pl-4"); more.type = "button"; more.setAttribute("aria-expanded", "false");
      more.onclick = async () => { if (more.dataset.loaded) { children.classList.toggle("hidden"); more.setAttribute("aria-expanded", String(!children.classList.contains("hidden"))); return; } more.disabled = true; try { const data = await post("/api/search/matches", { query, provider: result.provider, object_id: result.object_id }); for (const match of data.matches || []) { const child = node("a", "block rounded-xl px-3 py-2 text-sm text-on-surface-variant no-underline hover:bg-surface-container-high"); child.href = safeHref(result, match.anchor); child.innerHTML = match.snippet || match.chunk_id; child.onclick = () => remember(result, match.chunk_id); children.append(child); } more.dataset.loaded = "1"; children.classList.remove("hidden"); more.setAttribute("aria-expanded", "true"); } catch { children.replaceChildren(node("p", "text-sm text-error", "Could not load matches")); children.classList.remove("hidden"); } finally { more.disabled = false; } };
      top.append(more); article.append(top, children);
    } else article.append(top);
    return article;
  }
  function section(title, items) {
    const block = node("section", "focus-panel mb-5 overflow-hidden p-2");
    block.append(node("h2", "px-3 py-2 text-xs font-semibold uppercase tracking-widest text-on-surface-variant", title));
    const list = node("div", "grid gap-1"); list.append(...items.map(resultCard)); block.append(list); return block;
  }
  function render(data) {
    results.replaceChildren();
    if (data.stale_providers?.length) results.append(node("p", "mb-5 rounded-2xl bg-warning-container px-4 py-3 text-sm text-on-warning-container", `Results from ${data.stale_providers.join(", ")} may be out of date.`));
    if (data.recent?.length) results.append(section("Recent", data.recent));
    for (const group of data.groups || []) results.append(section(group.display_name, group.results || []));
    if (!data.recent?.length && !(data.groups || []).some(group => group.results?.length)) results.append(node("div", "focus-empty", "No matching items"));
    status.textContent = `Results for “${query}”`;
  }
  async function run() {
    query = input.value.trim(); sessionStorage.setItem("dar.search.query", query); controller?.abort();
    if (!query) { results.replaceChildren(); status.textContent = "Enter a search above."; return; }
    controller = new AbortController(); status.textContent = "Searching…"; results.replaceChildren(node("p", "py-12 text-center text-on-surface-variant", "Searching across your apps…"));
    try { render(await post("/api/search/query", { query, view: "results" }, controller.signal)); } catch (error) { if (error.name !== "AbortError") { status.textContent = "Search unavailable"; results.replaceChildren(node("p", "focus-empty text-error", "Search is temporarily unavailable.")); } }
  }
  function renderPreferences() {
    providerList.replaceChildren();
    preferences.forEach((provider, index) => { const row = node("div", "flex items-center gap-2 rounded-xl bg-surface-container px-3 py-2"); const name = node("span", "min-w-0 flex-1 truncate text-sm", provider.display_name); const toggle = node("input", "h-5 w-5 accent-primary"); toggle.type = "checkbox"; toggle.checked = provider.enabled; toggle.setAttribute("aria-label", `Include ${provider.display_name}`); toggle.onchange = () => { provider.enabled = toggle.checked; savePreferences(); }; const move = (delta, icon, label) => { const button = node("button", "h-8 w-8 rounded-full hover:bg-surface-container-high"); button.type = "button"; button.setAttribute("aria-label", `${label} ${provider.display_name}`); button.innerHTML = `<i class="fa-solid fa-${icon}"></i>`; button.disabled = index + delta < 0 || index + delta >= preferences.length; button.onclick = () => { const other = index + delta; [preferences[index], preferences[other]] = [preferences[other], preferences[index]]; renderPreferences(); savePreferences(); }; return button; }; row.append(name, toggle, move(-1, "arrow-up", "Move up"), move(1, "arrow-down", "Move down")); providerList.append(row); });
  }
  async function loadPreferences() { const data = await request("/api/search/preferences"); preferences = data.providers || []; renderPreferences(); }
  async function savePreferences() { try { const data = await request("/api/search/preferences", { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ providers: preferences.map(provider => ({ name: provider.name, enabled: provider.enabled })) }) }); preferences = data.providers || preferences; renderPreferences(); if (query) run(); } catch { status.textContent = "Could not save search preferences"; } }
  customizeButton.onclick = () => { customize.classList.toggle("hidden"); customizeButton.setAttribute("aria-expanded", String(!customize.classList.contains("hidden"))); };
  input.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(run, 220); });
  input.addEventListener("keydown", event => { if (event.key === "Enter") { event.preventDefault(); run(); } });
  query = sessionStorage.getItem("dar.search.query") || ""; input.value = query; if (query) run(); loadPreferences().catch(() => {});
})();
