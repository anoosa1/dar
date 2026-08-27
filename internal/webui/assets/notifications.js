(() => {
  "use strict";

  const root = document.getElementById("dar-notifications");
  if (!root) return;

  const button = document.getElementById("dar-notifications-button");
  const badge = document.getElementById("dar-notifications-badge");
  const panel = document.getElementById("dar-notifications-panel");
  const closeButton = document.getElementById("dar-notifications-close");
  const list = document.getElementById("dar-notifications-list");
  const desktopButton = document.getElementById("dar-notifications-desktop");
  const dismissAllButton = document.getElementById("dar-notifications-dismiss-all");
  const status = document.getElementById("dar-notifications-status");

  let notifications = [];
  let total = 0;
  let desktopEnabled = false;

  const node = (tag, classes, text) => {
    const value = document.createElement(tag);
    if (classes) value.className = classes;
    if (text !== undefined) value.textContent = text;
    return value;
  };

  async function request(path, options = {}) {
    const response = await fetch(path, options);
    if (response.status === 204) return {};
    let data = {};
    try {
      data = await response.json();
    } catch {
      data = {};
    }
    if (!response.ok) throw new Error(data.error || "Notification request failed");
    return data;
  }

  function safeURL(raw) {
    try {
      const url = new URL(raw || "/home", location.href);
      if (url.origin !== location.origin) return "/home";
      return url.pathname + url.search + url.hash;
    } catch {
      return "/home";
    }
  }

  function relativeTime(seconds) {
    const elapsed = Math.max(0, Math.floor(Date.now() / 1000) - Number(seconds || 0));
    if (elapsed < 60) return "Just now";
    if (elapsed < 3600) return Math.floor(elapsed / 60) + "m ago";
    if (elapsed < 86400) return Math.floor(elapsed / 3600) + "h ago";
    return Math.floor(elapsed / 86400) + "d ago";
  }

  function renderDesktopButton() {
    const label = desktopButton.querySelector("span");
    desktopButton.disabled = false;
    desktopButton.setAttribute("aria-pressed", desktopEnabled ? "true" : "false");
    if (!("Notification" in window)) {
      label.textContent = "Desktop alerts unavailable";
      desktopButton.disabled = true;
      return;
    }
    if (Notification.permission === "denied") {
      label.textContent = "Desktop alerts blocked";
      desktopButton.disabled = true;
      return;
    }
    label.textContent = desktopEnabled && Notification.permission === "granted"
      ? "Disable desktop alerts"
      : "Enable desktop alerts";
  }

  function notificationRow(item) {
    const article = node("article", "group relative flex gap-3 rounded-xl p-3 hover:bg-surface-container-low");
    const iconBox = node("span", "flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-primary-container text-primary");
    const icon = node("i", item.icon || "fa-solid fa-bell");
    icon.setAttribute("aria-hidden", "true");
    iconBox.append(icon);

    const link = node("a", "min-w-0 flex-1 text-on-surface no-underline");
    link.href = safeURL(item.object_url);
    const subject = node("span", "block pr-8 text-sm font-medium", item.subject || "Notification");
    const message = node("span", "mt-1 block text-xs leading-5 text-on-surface-variant", item.message || "");
    const meta = node("span", "mt-1 block text-[11px] text-on-surface-variant", relativeTime(item.created_at));
    link.append(subject);
    if (item.message) link.append(message);
    link.append(meta);

    const dismiss = node("button", "absolute right-2 top-2 h-8 w-8 rounded-full text-on-surface-variant opacity-70 hover:bg-surface-container-high hover:opacity-100");
    dismiss.type = "button";
    dismiss.setAttribute("aria-label", "Dismiss notification");
    const dismissIcon = node("i", "fa-solid fa-xmark");
    dismissIcon.setAttribute("aria-hidden", "true");
    dismiss.append(dismissIcon);
    dismiss.addEventListener("click", async () => {
      dismiss.disabled = true;
      try {
        await request("/api/notifications/" + encodeURIComponent(item.id), {method: "DELETE"});
        notifications = notifications.filter(value => value.id !== item.id);
        total = Math.max(0, total - 1);
        render();
        status.textContent = "Notification dismissed";
      } catch (error) {
        dismiss.disabled = false;
        window.DarUI?.toast(error.message, {tone: "error"});
      }
    });
    article.append(iconBox, link, dismiss);
    return article;
  }

  function render() {
    badge.classList.toggle("hidden", total === 0);
    badge.textContent = total > 99 ? "99+" : String(total);
    button.setAttribute("aria-label", total ? `Notifications, ${total} undismissed` : "Notifications");
    list.replaceChildren();
    if (!notifications.length) {
      const empty = node("div", "flex min-h-32 flex-col items-center justify-center px-6 py-8 text-center text-on-surface-variant");
      const icon = node("i", "fa-regular fa-bell mb-3 text-3xl");
      icon.setAttribute("aria-hidden", "true");
      empty.append(icon, node("p", "text-sm font-medium text-on-surface", "No notifications"), node("p", "mt-1 text-xs", "You’re all caught up."));
      list.append(empty);
    } else {
      notifications.forEach(item => list.append(notificationRow(item)));
    }
    dismissAllButton.disabled = total === 0;
    renderDesktopButton();
  }

  async function load() {
    const data = await request("/api/notifications?limit=50");
    notifications = data.notifications || [];
    total = Number(data.count || 0);
    desktopEnabled = Boolean(data.desktop_enabled);
    render();
  }

  function openPanel() {
    panel.classList.remove("hidden");
    button.setAttribute("aria-expanded", "true");
  }

  function closePanel() {
    panel.classList.add("hidden");
    button.setAttribute("aria-expanded", "false");
  }

  function desktopAlreadyShown(id) {
    const key = "dar.notification.desktop." + id;
    const now = Date.now();
    try {
      const previous = Number(localStorage.getItem(key) || 0);
      if (previous && now - previous < 60000) return true;
      localStorage.setItem(key, String(now));
    } catch {
      return false;
    }
    return false;
  }

  function showDesktop(item) {
    if (!desktopEnabled || !("Notification" in window) || Notification.permission !== "granted") return;
    if (location.pathname === "/email" || location.pathname.startsWith("/email/")) return;
    if (desktopAlreadyShown(item.id)) return;
    const title = item.source === "email" ? "New mail" : "New Dar notification";
    const body = item.source === "email" ? "Open Dar to view the message." : "Open Dar to view it.";
    const desktop = new Notification(title, {body, tag: "dar-" + item.id, renotify: false});
    desktop.onclick = () => {
      window.focus();
      location.assign(safeURL(item.object_url));
      desktop.close();
    };
  }

  button.addEventListener("click", () => {
    panel.classList.contains("hidden") ? openPanel() : closePanel();
  });
  closeButton.addEventListener("click", closePanel);
  document.addEventListener("click", event => {
    if (!panel.classList.contains("hidden") && !root.contains(event.target)) closePanel();
  });
  document.addEventListener("keydown", event => {
    if (event.key === "Escape" && !panel.classList.contains("hidden")) {
      closePanel();
      button.focus();
    }
  });

  dismissAllButton.addEventListener("click", async () => {
    dismissAllButton.disabled = true;
    try {
      await request("/api/notifications", {method: "DELETE"});
      notifications = [];
      total = 0;
      render();
      status.textContent = "All notifications dismissed";
    } catch (error) {
      render();
      window.DarUI?.toast(error.message, {tone: "error"});
    }
  });

  desktopButton.addEventListener("click", async () => {
    if (!("Notification" in window) || Notification.permission === "denied") return;
    try {
      let enabled = desktopEnabled;
      if (!desktopEnabled || Notification.permission !== "granted") {
        enabled = await Notification.requestPermission() === "granted";
      } else {
        enabled = false;
      }
      const saved = await request("/api/notifications/preferences", {
        method: "PUT",
        headers: {"Content-Type": "application/json"},
        body: JSON.stringify({desktop_enabled: enabled}),
      });
      desktopEnabled = Boolean(saved.desktop_enabled);
      renderDesktopButton();
      status.textContent = desktopEnabled ? "Desktop alerts enabled" : "Desktop alerts disabled";
    } catch (error) {
      window.DarUI?.toast(error.message, {tone: "error"});
    }
  });

  let source = null;

  function connect() {
    source?.close();
    source = new EventSource("/api/notifications/stream");
    source.addEventListener("notification.created", event => {
      try {
        const item = JSON.parse(event.data || "{}");
        const existed = notifications.some(value => value.id === item.id);
        notifications = [item, ...notifications.filter(value => value.id !== item.id)].slice(0, 50);
        if (!existed) total += 1;
        render();
        showDesktop(item);
        status.textContent = "New notification";
      } catch {
        load().catch(() => {});
      }
    });
    for (const eventName of ["notification.dismissed", "notification.dismissed_all", "notification.preferences", "notification.resync"]) {
      source.addEventListener(eventName, () => load().catch(() => {}));
    }
  }

  window.addEventListener("pagehide", () => {
    source?.close();
    source = null;
  }, {once: true});

  load().then(connect).catch(error => {
    render();
    status.textContent = error.message;
  });
})();
