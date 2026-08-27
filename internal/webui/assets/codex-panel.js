(() => {
  "use strict";
  const panel = document.getElementById("dar-codex-panel");
  if (!panel) return;
  const $ = id => document.getElementById(id);
  const state = { session: null, sessions: [], source: null, assistant: null, streaming: false, attachments: [], opener: null };
  const api = async (path, options = {}) => {
    options.headers = Object.assign({ Accept: "application/json" }, options.headers || {});
    const response = await fetch("/api/codex" + path, options);
    if (response.status === 204) return null;
    const text = await response.text();
    let data;
    try { data = JSON.parse(text); } catch { data = { error: text }; }
    if (!response.ok) throw new Error(data.error || "Codex request failed");
    return data;
  };
  const show = (id, visible) => $(id)?.classList.toggle("hidden", !visible);
  const message = (text, error = false) => {
    const node = $("dar-codex-status");
    node.textContent = text || "";
    node.className = "px-4 py-2 text-xs " + (error ? "text-error" : "text-on-surface-variant");
  };
  function openPanel() {
    state.opener = document.activeElement;
    panel.classList.remove("hidden");
    panel.setAttribute("aria-hidden", "false");
    document.body.classList.add("dar-codex-open");
    $("dar-codex-close")?.focus();
    load().catch(error => message(error.message, true));
  }
  function closePanel() {
    panel.classList.add("hidden");
    panel.setAttribute("aria-hidden", "true");
    document.body.classList.remove("dar-codex-open");
    state.source?.close();
    state.source = null;
    show("dar-codex-settings", false);
    state.opener?.focus?.();
  }
  function bubble(role, text) {
    const row = document.createElement("div");
    row.className = "flex " + (role === "user" ? "justify-end" : "justify-start");
    const card = document.createElement("div");
    card.className = (role === "user" ? "bg-primary-container" : "bg-surface-container") + " max-w-[88%] rounded-2xl px-4 py-3 text-sm whitespace-pre-wrap";
    card.textContent = text;
    row.append(card);
    $("dar-codex-messages").append(row);
    $("dar-codex-messages").scrollTop = $("dar-codex-messages").scrollHeight;
    return card;
  }
  function renderSessions() {
    const root = $("dar-codex-sessions");
    root.replaceChildren();
    for (const session of state.sessions.slice(0, 8)) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "w-full truncate rounded-xl px-3 py-2 text-left text-sm hover:bg-surface-container" + (state.session?.id === session.id ? " bg-primary-container text-primary" : "");
      button.textContent = session.title || "New chat";
      button.onclick = () => openSession(session.id);
      root.append(button);
    }
    show("dar-codex-session-empty", !state.sessions.length);
  }
  function textOf(item) {
    if (typeof item.text === "string") return item.text;
    if (typeof item.message === "string") return item.message;
    if (typeof item.content === "string") return item.content;
    if (Array.isArray(item.content)) return item.content.map(value => value.text || value.content || "").join("");
    return "";
  }
  function renderHistory(data) {
    const root = $("dar-codex-messages");
    root.replaceChildren();
    const thread = data?.thread?.thread || data?.thread || {};
    for (const turn of thread.turns || []) for (const item of turn.items || []) {
      const type = String(item.type || "").toLowerCase(), text = textOf(item);
      if (!text) continue;
      if (type.includes("user")) bubble("user", text);
      else if (type.includes("agent") || type.includes("assistant")) bubble("assistant", text);
    }
  }
  function connect(sessionID, after = 0) {
    state.source?.close();
    const source = new EventSource(`/api/codex/sessions/${encodeURIComponent(sessionID)}/events?after=${encodeURIComponent(after)}`);
    state.source = source;
    for (const type of ["message.delta", "turn.started", "turn.completed", "turn.interrupted", "approval.required", "error", "resync", "session.renamed"]) {
      source.addEventListener(type, event => {
        if (source !== state.source) return;
        try { onEvent(type, JSON.parse(event.data || "{}")); } catch (error) { message(error.message, true); }
      });
    }
  }
  function approvalCard(data) {
    const box = document.createElement("section");
    box.className = "rounded-2xl border border-outline-variant bg-surface-container p-4 text-sm";
    const title = document.createElement("strong");
    title.textContent = `Approve ${data.namespace}.${data.tool}?`;
    const argumentsView = document.createElement("pre");
    argumentsView.className = "mt-2 max-h-36 overflow-auto whitespace-pre-wrap rounded-xl bg-surface-container-low p-3 text-xs";
    argumentsView.textContent = JSON.stringify(data.arguments || {}, null, 2);
    const actions = document.createElement("div");
    actions.className = "mt-3 flex gap-2";
    for (const [decision, label] of [["approve", "Approve"], ["deny", "Deny"]]) {
      const button = document.createElement("button");
      button.className = decision === "approve" ? "rounded-full bg-primary px-4 py-2 text-on-primary" : "rounded-full px-4 py-2 text-error";
      button.textContent = label;
      button.onclick = async () => {
        try { await api(`/approvals/${encodeURIComponent(data.call_id)}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ decision }) }); box.remove(); }
        catch (error) { message(error.message, true); }
      };
      actions.append(button);
    }
    box.append(title, argumentsView, actions);
    $("dar-codex-messages").append(box);
  }
  function onEvent(type, data) {
    if (type === "message.delta") {
      if (!state.assistant) state.assistant = bubble("assistant", "");
      state.assistant.textContent += data.delta || "";
      $("dar-codex-messages").scrollTop = $("dar-codex-messages").scrollHeight;
    } else if (type === "turn.started") {
      state.streaming = true; message("Codex is working…");
    } else if (type === "turn.completed" || type === "turn.interrupted") {
      state.streaming = false; state.assistant = null; message(type === "turn.interrupted" ? "Response stopped." : ""); loadSessions();
    } else if (type === "approval.required") approvalCard(data);
    else if (type === "error") { state.streaming = false; message(data.message || "Codex failed", true); }
    else if (type === "resync" && state.session) openSession(state.session.id);
    else if (type === "session.renamed") loadSessions();
  }
  async function loadSessions() {
    const data = await api("/sessions");
    state.sessions = data.sessions || [];
    renderSessions();
  }
  async function load() {
    const status = await api("/status");
    show("dar-codex-unavailable", !status.available || !status.authenticated);
    show("dar-codex-ready", status.available && status.authenticated);
    if (status.available && status.authenticated) await loadSessions();
  }
  function renderAttachments() {
    const root = $("dar-codex-attachments");
    root.replaceChildren();
    root.classList.toggle("hidden", !state.attachments.length);
    root.classList.toggle("flex", Boolean(state.attachments.length));
    for (const attachment of state.attachments) {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "rounded-full bg-secondary-container px-3 py-1 text-xs text-on-secondary-container";
      chip.textContent = `${attachment.name} ×`;
      chip.setAttribute("aria-label", `Remove ${attachment.name}`);
      chip.onclick = () => { state.attachments = state.attachments.filter(value => value.id !== attachment.id); renderAttachments(); };
      root.append(chip);
    }
  }
  async function upload(file) {
    const form = new FormData(); form.append("file", file);
    const attachment = await api("/attachments/upload", { method: "POST", body: form });
    state.attachments.push(attachment); renderAttachments();
  }
  async function ensurePicker() {
    if (window.DarFilePicker) return;
    await new Promise((resolve, reject) => { const script = document.createElement("script"); script.src = "/files/assets/picker.js"; script.onload = resolve; script.onerror = () => reject(new Error("Files picker is unavailable")); document.head.append(script); });
  }
  async function selectFiles() {
    await ensurePicker();
    const files = await DarFilePicker.open({ selection: "files", multiple: true, title: "Add Dar files" });
    if (!files.length) return;
    const data = await api("/attachments/select", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ paths: files.map(file => file.path) }) });
    state.attachments.push(...(data.attachments || [])); renderAttachments();
  }
  async function openSettings() {
    show("dar-codex-settings", true); $("dar-codex-settings").classList.add("flex");
    $("dar-codex-settings-status").textContent = "Loading…";
    try { const preferences = await api("/preferences"); $("dar-codex-approval-mode").value = preferences.approval_mode; $("dar-codex-web-search").value = preferences.web_search_mode; $("dar-codex-settings-status").textContent = ""; $("dar-codex-settings-close").focus(); }
    catch (error) { $("dar-codex-settings-status").textContent = error.message; }
  }
  function closeSettings() { const settings = $("dar-codex-settings"); settings.classList.add("hidden"); settings.classList.remove("flex"); $("dar-codex-settings-open").focus(); }
  async function saveSettings() {
    const status = $("dar-codex-settings-status"); status.textContent = "Saving…";
    try { await api("/preferences", { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ approval_mode: $("dar-codex-approval-mode").value, web_search_mode: $("dar-codex-web-search").value }) }); status.textContent = "Saved"; setTimeout(closeSettings, 350); }
    catch (error) { status.textContent = error.message; }
  }
  async function openSession(id) {
    message("Loading chat…");
    try {
      const data = await api("/sessions/" + encodeURIComponent(id));
      state.session = data.session;
      state.assistant = null;
      $("dar-codex-title").textContent = data.session.title || "New chat";
      renderHistory(data); renderSessions(); connect(id, data.event_cursor || 0); message("");
    } catch (error) { message(error.message, true); }
  }
  function newChat() {
    state.source?.close(); state.source = null; state.session = null; state.assistant = null;
    $("dar-codex-title").textContent = "New chat";
    state.attachments = []; renderAttachments(); $("dar-codex-messages").replaceChildren(); renderSessions(); $("dar-codex-input").focus();
  }
  async function send() {
    const input = $("dar-codex-input"), text = input.value.trim(), attachments = [...state.attachments];
    if ((!text && !attachments.length) || state.streaming) return;
    input.value = ""; bubble("user", text || "(attachments)"); state.assistant = null; state.streaming = true; message("Codex is working…");
    try {
      const body = JSON.stringify({ text, attachments: attachments.map(attachment => attachment.id) });
      if (state.session) await api(`/sessions/${encodeURIComponent(state.session.id)}/turns`, { method: "POST", headers: { "Content-Type": "application/json" }, body });
      else {
        const data = await api("/sessions", { method: "POST", headers: { "Content-Type": "application/json" }, body });
        state.session = data.session; state.sessions.unshift(data.session); $("dar-codex-title").textContent = data.session.title || "New chat"; renderSessions(); connect(data.session.id);
      }
      state.attachments = []; renderAttachments();
    } catch (error) { state.streaming = false; state.attachments = attachments; renderAttachments(); message(error.message, true); input.value = text; }
  }
  document.querySelectorAll("[data-dar-codex-open]").forEach(button => button.addEventListener("click", openPanel));
  $("dar-codex-close")?.addEventListener("click", closePanel);
  $("dar-codex-new")?.addEventListener("click", newChat);
  $("dar-codex-upload")?.addEventListener("click", () => $("dar-codex-file").click());
  $("dar-codex-file")?.addEventListener("change", async event => { const file = event.target.files[0]; if (!file) return; try { await upload(file); message(""); } catch (error) { message(error.message, true); } finally { event.target.value = ""; } });
  $("dar-codex-picker")?.addEventListener("click", () => selectFiles().catch(error => message(error.message, true)));
  $("dar-codex-settings-open")?.addEventListener("click", openSettings);
  $("dar-codex-settings-close")?.addEventListener("click", closeSettings);
  $("dar-codex-settings-save")?.addEventListener("click", saveSettings);
  $("dar-codex-send")?.addEventListener("click", send);
  $("dar-codex-input")?.addEventListener("keydown", event => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); send(); } });
  document.addEventListener("keydown", event => {
    if (panel.classList.contains("hidden")) return;
    if (event.key === "Escape") { if (!$("dar-codex-settings").classList.contains("hidden")) closeSettings(); else closePanel(); return; }
    if (event.key !== "Tab") return;
    const focusable = [...panel.querySelectorAll('a[href],button:not([disabled]),textarea:not([disabled]),select:not([disabled]),input:not([disabled]):not([type="hidden"])')].filter(item => item.offsetParent !== null);
    if (!focusable.length) return;
    const first = focusable[0], last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
  });
})();
