(function () {
  "use strict";
  const $ = id => document.getElementById(id);
  const state = {source: "all", cursor: "", loading: false, sources: []};
  const icons = {codex:"fa-terminal",files:"fa-folder",contacts:"fa-address-book",calendar:"fa-calendar",todo:"fa-check-double",notes:"fa-note-sticky",news:"fa-newspaper",email:"fa-envelope",system:"fa-shield-halved"};

  async function request(path, options) {
    const response = await fetch(path, options);
    let data = {};
    try { data = await response.json(); } catch (_) {}
    if (!response.ok) throw new Error(data.error || "Activity request failed");
    return data;
  }
  function node(tag, classes, text) {
    const el = document.createElement(tag);
    if (classes) el.className = classes;
    if (text !== undefined) el.textContent = text;
    return el;
  }
  function announce(message) { $("activity-status").textContent = message; }
  function sourceLabel(id) {
    if (id === "all") return "All activities";
    return state.sources.find(source => source.id === id)?.label || id;
  }
  function makeTab(source, mobile) {
    const button = node("button", (mobile ? "shrink-0 " : "w-full ")+"flex h-10 items-center gap-3 rounded-full px-4 text-left text-sm hover:bg-surface-container-high");
    button.type = "button";
    button.dataset.source = source.id;
    button.append(node("i","fa-solid "+(source.id === "all" ? "fa-bolt" : icons[source.id] || "fa-circle")+" w-5 text-center"),node("span","truncate",source.label));
    button.onclick = () => selectSource(source.id);
    return button;
  }
  function renderTabs() {
    const sources = [{id:"all",label:"All activities"}].concat(state.sources);
    $("activity-tabs").replaceChildren(...sources.map(source => makeTab(source,false)));
    $("activity-tabs-mobile").replaceChildren(...sources.map(source => makeTab(source,true)));
    document.querySelectorAll("[data-source]").forEach(button => {
      const active = button.dataset.source === state.source;
      button.classList.toggle("bg-primary-container",active);
      button.classList.toggle("text-primary",active);
      if (active) button.setAttribute("aria-current","page"); else button.removeAttribute("aria-current");
    });
  }
  function selectSource(source) {
    state.source = source;
    state.cursor = "";
    $("activity-title").textContent = sourceLabel(source);
    renderTabs();
    load(false);
  }
  function dateKey(timestamp) {
    return new Intl.DateTimeFormat(undefined,{year:"numeric",month:"long",day:"numeric"}).format(new Date(timestamp*1000));
  }
  function eventRow(event) {
    const row = node("article","flex gap-3 rounded-2xl px-3 py-3 hover:bg-surface-container-low");
    const icon = node("span","flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-secondary-container text-on-secondary-container");
    icon.append(node("i","fa-solid "+(icons[event.source]||"fa-bolt")));
    const body = node("div","min-w-0 flex-1");
    const summary = node("p","text-sm");
    const actor = node("strong","font-semibold",event.actor_is_you ? "You" : event.actor_name);
    summary.append(actor,document.createTextNode(" "+event.summary));
    const meta = node("p","mt-1 text-xs text-on-surface-variant");
    const when = new Date(event.happened_at*1000);
    const relative = new Intl.RelativeTimeFormat(undefined,{numeric:"auto"});
    const delta = Math.round((when.getTime()-Date.now())/60000);
    const relativeText = Math.abs(delta)<60 ? relative.format(delta,"minute") : relative.format(Math.round(delta/1440),"day");
    meta.textContent = sourceLabel(event.source)+" · "+event.origin+" · "+relativeText;
    meta.title = when.toLocaleString();
    body.append(summary,meta);
    if (event.object_url) {
      const link = node("a","mt-2 inline-flex text-sm font-medium text-primary no-underline hover:underline",event.object_name ? "Open "+event.object_name : "Open item");
      link.href = event.object_url;
      body.append(link);
    }
    row.append(icon,body);
    return row;
  }
  function appendEvents(events, reset) {
    const list = $("activity-list");
    if (reset) list.replaceChildren();
    if (reset && !events.length) {
      const empty = node("section","rounded-3xl bg-surface-container-low p-10 text-center");
      empty.append(node("i","fa-solid fa-bolt mb-3 text-3xl text-primary"),node("h2","font-semibold","No activity yet"),node("p","mt-1 text-sm text-on-surface-variant","Completed actions will appear here."));
      list.append(empty); return;
    }
    let current = list.lastElementChild?.dataset.date || "";
    let group = list.lastElementChild;
    for (const event of events) {
      const key = dateKey(event.happened_at);
      if (key !== current) {
        group = node("section","space-y-1");
        group.dataset.date = key;
        group.append(node("h2","mb-2 text-sm font-semibold text-on-surface-variant",key));
        list.append(group); current = key;
      }
      group.append(eventRow(event));
    }
  }
  async function load(more) {
    if (state.loading) return;
    state.loading = true;
    $("activity-more").disabled = true;
    if (!more) {
      $("activity-list").replaceChildren(node("p","py-12 text-center text-sm text-on-surface-variant","Loading activity…"));
    }
    try {
      const params = new URLSearchParams({source:state.source,limit:"50"});
      if (more && state.cursor) params.set("cursor",state.cursor);
      const data = await request("/api/activity?"+params);
      appendEvents(data.events||[],!more);
      state.cursor = data.next_cursor || "";
      $("activity-more").classList.toggle("hidden",!state.cursor);
      announce((data.events||[]).length+" activities loaded");
    } catch (error) {
      if (!more) {
        const box = node("section","rounded-3xl bg-error-container p-8 text-center text-error");
        box.append(node("p","font-medium",error.message));
        const retry = node("button","mt-4 h-10 rounded-full bg-error px-5 text-on-error","Retry");
        retry.onclick=()=>load(false); box.append(retry); $("activity-list").replaceChildren(box);
      } else {
        window.DarUI?.toast(error.message,{tone:"error"});
      }
    } finally {
      state.loading=false; $("activity-more").disabled=false;
    }
  }
  async function loadSettings(openDialog) {
    const data = await request("/api/activity/settings");
    state.sources = data.available || [];
    renderTabs();
    $("activity-rss-enabled").checked = !!data.rss_enabled;
    $("activity-rss-sources").replaceChildren(...state.sources.map(source => {
      const label=node("label","flex items-center gap-2 rounded-xl bg-surface-container-low px-3 py-2 text-sm");
      const input=node("input","h-4 w-4 accent-primary"); input.type="checkbox"; input.value=source.id; input.checked=(data.sources||[]).includes(source.id);
      label.append(input,node("span","",source.label)); return label;
    }));
    $("activity-feed-regenerate").classList.toggle("hidden",!data.rss_enabled);
    $("activity-feed-url-box").classList.add("hidden");
    if (openDialog !== false) $("activity-settings-dialog").showModal();
  }
  function selectedSources() {
    return [...$("activity-rss-sources").querySelectorAll('input:checked')].map(input=>input.value);
  }
  function showFeedURL(url) {
    if (!url) return;
    $("activity-feed-result-url").value=url;
    $("activity-feed-result-dialog").showModal();
  }
  async function saveSettings(event) {
    event.preventDefault();
    const button=$("activity-settings-save"); button.disabled=true; button.textContent="Saving…";
    try {
      const data=await request("/api/activity/settings",{method:"PUT",headers:{"Content-Type":"application/json"},body:JSON.stringify({rss_enabled:$("activity-rss-enabled").checked,sources:selectedSources()})});
      $("activity-feed-regenerate").classList.toggle("hidden",!data.rss_enabled);
      window.DarUI?.toast("Activity settings saved");
      $("activity-settings-dialog").close();
      showFeedURL(data.feed_url);
    } catch(error) {
      const message=$("activity-settings-message"); message.textContent=error.message; message.className="mt-3 text-sm text-error";
    } finally { button.disabled=false; button.textContent="Save"; }
  }
  async function regenerate() {
    const accepted=await window.DarUI.confirmDialog({title:"Regenerate RSS URL?",message:"The current feed URL will stop working immediately.",confirmLabel:"Regenerate",tone:"danger"});
    if (!accepted) return;
    try { const data=await request("/api/activity/rss/regenerate",{method:"POST"}); $("activity-settings-dialog").close(); showFeedURL(data.feed_url); window.DarUI.toast("New RSS URL created"); }
    catch(error){ window.DarUI.toast(error.message,{tone:"error"}); }
  }
  $("activity-more").onclick=()=>load(true);
  $("activity-settings-open").onclick=()=>loadSettings(true);
  $("activity-settings-mobile").onclick=()=>loadSettings(true);
  $("activity-settings-close").onclick=$("activity-settings-cancel").onclick=()=>$("activity-settings-dialog").close();
  $("activity-settings-form").onsubmit=saveSettings;
  $("activity-feed-regenerate").onclick=regenerate;
  $("activity-feed-copy").onclick=async()=>{await navigator.clipboard.writeText($("activity-feed-url").value);window.DarUI?.toast("Feed URL copied")};
  $("activity-feed-result-copy").onclick=async()=>{await navigator.clipboard.writeText($("activity-feed-result-url").value);window.DarUI?.toast("Feed URL copied")};
  $("activity-feed-result-close").onclick=()=>$("activity-feed-result-dialog").close();
  loadSettings(false).then(()=>load(false)).catch(error=>{state.sources=[{id:"codex",label:"Codex"},{id:"files",label:"Files"},{id:"contacts",label:"Contacts"},{id:"calendar",label:"Calendar"},{id:"todo",label:"Todo"},{id:"notes",label:"Notes"},{id:"news",label:"News"},{id:"email",label:"Email"},{id:"system",label:"System"}];renderTabs();load(false);window.DarUI?.toast(error.message,{tone:"error"})});
})();
