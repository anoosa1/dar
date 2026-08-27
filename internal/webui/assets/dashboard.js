(function () {
	"use strict";

	const grid = document.getElementById("dashboard-grid");
	if (!grid) return;

	const catalog = document.getElementById("dashboard-catalog");
	const editButton = document.getElementById("dashboard-edit");
	const status = document.getElementById("dashboard-status");
	const settingsModal = document.getElementById("dashboard-settings-modal");
	const settingsForm = document.getElementById("dashboard-settings-form");
	const settingsFields = document.getElementById("dashboard-settings-fields");
	const settingsProvider = document.getElementById("dashboard-settings-provider");
	const settingsSizeOptions = document.getElementById("dashboard-settings-size-options");
	const settingsMessage = document.getElementById("dashboard-settings-message");
	const settingsSubmit = settingsForm.querySelector('button[type="submit"]');

	let layout = [];
	let definitions = [];
	let presets = [];
	let editing = false;
	let drag = null;
	let settingsWidget = "";
	let settingsTrigger = null;
	let providerFields = [];
	let saveQueue = Promise.resolve();
	const widgetLoads = new Map();

	function node(tag, classes, text) {
		const element = document.createElement(tag);
		if (classes) element.className = classes;
		if (text !== undefined) element.textContent = text;
		return element;
	}

	function icon(classes) {
		return node("i", String(classes || ""));
	}

	async function api(url, options) {
		const response = await fetch(url, options || {});
		if (!response.ok) {
			const message = (await response.text()).trim();
			throw new Error(message || "Request failed");
		}
		return response.json();
	}

	function announce(message) {
		status.textContent = "";
		window.requestAnimationFrame(function () {
			status.textContent = message;
		});
	}

	function timezone() {
		try {
			return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
		} catch (_) {
			return "UTC";
		}
	}

	function widgetURL(id, size, suffix) {
		const query = new URLSearchParams({timezone: timezone()});
		if (size) query.set("size", size);
		return "/api/dashboard/widgets/" + encodeURIComponent(id) + (suffix || "") + "?" + query.toString();
	}

	function safeHref(raw) {
		try {
			const url = new URL(String(raw || ""), location.href);
			if (url.origin === location.origin && (url.protocol === "http:" || url.protocol === "https:")) return url.href;
		} catch (_) {}
		return "#";
	}

	function definitionFor(id) {
		return definitions.find(function (definition) { return definition.id === id; });
	}

	function presetFor(size) {
		return presets.find(function (preset) { return preset.size === size; });
	}

	function supportedPresets(definition) {
		return presets.filter(function (preset) {
			return (definition.supportedSizes || []).includes(preset.size);
		});
	}

	function geometryClasses(item) {
		const preset = presetFor(item.size);
		if (!preset) return [];
		return [
			"min-h-52",
			"md:col-span-" + preset.w,
			"md:col-start-" + (item.x + 1),
			"md:row-start-[" + (item.y + 1) + "]",
			"md:row-span-" + preset.h
		];
	}

	function cardClasses(item) {
		return [
			"focus-widget", "col-span-1", "min-w-0", "overflow-hidden",
			"flex", "flex-col"
		].concat(geometryClasses(item)).join(" ");
	}

	function rectanglesOverlap(left, right) {
		const leftPreset = presetFor(left.size);
		const rightPreset = presetFor(right.size);
		return left.x < right.x + rightPreset.w &&
			left.x + leftPreset.w > right.x &&
			left.y < right.y + rightPreset.h &&
			left.y + leftPreset.h > right.y;
	}

	function sortLayout(items) {
		return items.sort(function (left, right) {
			return left.y === right.y ? left.x - right.x : left.y - right.y;
		});
	}

	function reflow(items, activeID) {
		const sorted = sortLayout(items.map(function (item) { return Object.assign({}, item); }));
		const ordered = [];
		if (activeID) {
			const active = sorted.find(function (item) { return item.id === activeID; });
			if (active) ordered.push(active);
		}
		sorted.forEach(function (item) {
			if (item.id !== activeID) ordered.push(item);
		});
		const placed = [];
		ordered.forEach(function (item) {
			const preset = presetFor(item.size);
			item.x = Math.min(Math.max(item.x, 0), 12 - preset.w);
			item.y = Math.max(item.y, 0);
			while (placed.some(function (other) { return rectanglesOverlap(item, other); })) item.y++;
			placed.push(item);
		});
		placed.forEach(function (item, index) {
			if (item.id === activeID) return;
			while (item.y > 0) {
				const candidate = Object.assign({}, item, {y: item.y - 1});
				const blocked = placed.some(function (other, otherIndex) {
					return otherIndex !== index && rectanglesOverlap(candidate, other);
				});
				if (blocked) break;
				item.y--;
			}
		});
		return sortLayout(placed);
	}

	function applyGeometry() {
		grid.querySelectorAll("article[data-widget]").forEach(function (card) {
			const item = layout.find(function (candidate) { return candidate.id === card.dataset.widget; });
			if (!item) return;
			card.className = cardClasses(item);
			card.dataset.size = item.size;
			const badge = card.querySelector("[data-size-badge]");
			const preset = presetFor(item.size);
			if (badge && preset) badge.textContent = preset.label;
		});
	}

	function primaryActionControl(action, id, compact) {
		if (action.kind === "settings") {
			if (!editing) return null;
			const button = node("button", compact ?
				"h-8 w-8 rounded-full text-primary hover:bg-primary-container" :
				"inline-flex h-9 items-center gap-2 rounded-full px-3 text-sm text-primary hover:bg-primary-container");
			button.type = "button";
			button.setAttribute("aria-label", action.label || "Configure");
			button.appendChild(icon(action.icon));
			if (!compact) button.appendChild(document.createTextNode(String(action.label || "Configure")));
			button.onclick = function () { openSettings(id, button); };
			return button;
		}
		const link = node("a", compact ?
			"inline-flex h-8 w-8 items-center justify-center rounded-full text-primary no-underline hover:bg-primary-container" :
			"inline-flex h-9 items-center gap-2 rounded-full px-3 text-sm text-primary no-underline hover:bg-primary-container");
		link.href = safeHref(action.url);
		link.setAttribute("aria-label", action.label || "Open");
		link.appendChild(icon(action.icon));
		if (!compact) link.appendChild(document.createTextNode(String(action.label || "")));
		return link;
	}

	function itemNode(item, definition) {
		const card = node("article", cardClasses(item));
		card.dataset.widget = String(item.id);
		card.dataset.size = String(item.size);

		const header = node("header", "h-11 shrink-0 border-b border-outline-variant px-3 flex items-center gap-2");
		header.dataset.drag = "";
		const appIcon = icon(definition.icon);
		appIcon.classList.add("text-primary");
		header.append(appIcon, node("h2", "min-w-0 flex-1 truncate text-sm font-semibold", definition.title));

		const compactAction = node("span", "flex items-center");
		compactAction.dataset.compactAction = "";
		header.appendChild(compactAction);

		const sizeBadge = node("span", "hidden rounded-full bg-surface-container-high px-2 py-1 text-[11px] text-on-surface-variant");
		sizeBadge.dataset.sizeBadge = "";
		sizeBadge.textContent = presetFor(item.size).label;
		header.appendChild(sizeBadge);

		const settings = node("button", "hidden h-8 w-8 rounded-full hover:bg-surface-container-high");
		settings.type = "button";
		settings.dataset.settings = "";
		settings.title = "Widget settings";
		settings.setAttribute("aria-label", definition.title + " settings");
		settings.appendChild(icon("fa-solid fa-gear"));
		settings.onpointerdown = function (event) { event.stopPropagation(); };
		settings.onclick = function (event) {
			event.stopPropagation();
			openSettings(item.id, settings);
		};
		header.appendChild(settings);

		const remove = node("button", "hidden h-8 w-8 rounded-full text-error hover:bg-error-container");
		remove.type = "button";
		remove.dataset.remove = "";
		remove.title = "Remove widget";
		remove.setAttribute("aria-label", "Remove " + definition.title);
		remove.appendChild(icon("fa-solid fa-xmark"));
		remove.onpointerdown = function (event) { event.stopPropagation(); };
		remove.onclick = function () { removeWidget(item.id); };
		header.appendChild(remove);

		const body = node("div", "min-h-0 flex-1 overflow-hidden p-2");
		body.dataset.content = "";
		body.appendChild(node("p", "p-2 text-sm text-on-surface-variant", "Loading…"));

		const resize = node("button", "hidden h-8 shrink-0 self-end px-2 text-on-surface-variant cursor-nwse-resize");
		resize.type = "button";
		resize.dataset.resize = "";
		resize.title = "Snap widget size";
		resize.setAttribute("aria-label", "Resize " + definition.title);
		resize.append(icon("fa-solid fa-up-right-and-down-left-from-center"), node("span", "ml-2 text-xs", presetFor(item.size).label));
		card.append(header, body, resize);
		return card;
	}

	function appendItems(parent, items, size) {
		const list = node("div", size === "wide" || size === "large" ? "grid grid-cols-2 gap-1" : "flex flex-col");
		(items || []).forEach(function (item) {
			const link = node("a", "flex min-w-0 items-center gap-2 rounded-md px-2 " +
				(size === "compact" || size === "standard" ? "py-1" : "py-2") +
				" text-on-surface no-underline hover:bg-surface-container-high");
			link.href = safeHref(item.url);
			const itemIcon = icon(item.icon);
			itemIcon.classList.add("w-5", "shrink-0", "text-center", "text-on-surface-variant");
			const labels = node("span", "min-w-0 flex-1");
			labels.append(node("span", "block truncate text-sm", item.title));
			if (item.subtitle) labels.append(node("span", "block truncate text-xs text-on-surface-variant", item.subtitle));
			link.append(itemIcon, labels);
			if (item.favorite) link.appendChild(icon("fa-solid fa-star shrink-0 text-xs text-warning"));
			list.appendChild(link);
		});
		parent.appendChild(list);
	}

	function appendSections(parent, sections, size) {
		const columns = node("div", size === "wide" || size === "large" ? "grid grid-cols-2 gap-3" : "space-y-2");
		(sections || []).forEach(function (section) {
			if (!section.items || !section.items.length) return;
			const group = node("section", "min-w-0");
			group.appendChild(node("h3", "px-2 pb-1 pt-2 text-xs font-semibold uppercase tracking-wide text-on-surface-variant", section.title));
			appendItems(group, section.items, size === "large" ? "standard" : "standard");
			columns.appendChild(group);
		});
		parent.appendChild(columns);
		return columns.childElementCount > 0;
	}

	function weatherNumber(value) {
		return Math.round(Number(value || 0)) + "°";
	}

	function renderWeather(parent, data, size) {
		const weather = data.weather;
		const wrap = node("div", size === "wide" || size === "large" ?
			"grid h-full min-h-0 grid-cols-1 gap-4 md:grid-cols-2" :
			"flex h-full min-h-0 flex-col");
		const current = node("div", "flex min-w-0 items-center gap-4 p-2");
		const currentIcon = icon(weather.current.icon);
		currentIcon.className = weather.current.icon + " w-14 shrink-0 text-center text-4xl text-primary";
		const details = node("div", "min-w-0 flex-1");
		details.append(
			node("p", "truncate text-sm text-on-surface-variant", weather.location),
			node("p", size === "large" ? "text-4xl font-semibold" : "text-3xl font-semibold", weatherNumber(weather.current.temperature)),
			node("p", "truncate text-sm", weather.current.label)
		);
		if (size === "large") {
			details.append(
				node("p", "mt-1 text-xs text-on-surface-variant", "Feels like " + weatherNumber(weather.current.apparent)),
				node("p", "text-xs text-on-surface-variant", "Updated " + new Date(weather.updatedAt * 1000).toLocaleTimeString([], {hour: "numeric", minute: "2-digit"}))
			);
		}
		current.append(currentIcon, details);
		if (weather.stale) current.appendChild(node("span", "self-start rounded-full bg-warning/20 px-2 py-1 text-xs text-warning", "Cached"));
		wrap.appendChild(current);

		if (weather.forecast && weather.forecast.length) {
			const days = node("div", "grid min-w-0 grid-cols-3 gap-1 border-t border-outline-variant pt-3 " +
				(size === "wide" || size === "large" ? "md:grid-cols-5 md:border-l md:border-t-0 md:pl-3 md:pt-0" : ""));
			weather.forecast.forEach(function (day) {
				const cell = node("div", "min-w-0 text-center");
				cell.append(
					node("p", "truncate text-[11px] text-on-surface-variant", new Date(day.date + "T12:00:00").toLocaleDateString(undefined, {weekday: "short"})),
					icon(day.icon + " my-2 text-primary"),
					node("p", "text-xs", weatherNumber(day.high) + " / " + weatherNumber(day.low))
				);
				days.appendChild(cell);
			});
			wrap.appendChild(days);
		}
		parent.appendChild(wrap);
	}

	function appendActions(shell, actions, id, size, card) {
		if (!actions || !actions.length) return;
		if (size === "compact") {
			if (editing) return;
			const control = primaryActionControl(actions[0], id, true);
			if (control) card.querySelector("[data-compact-action]").appendChild(control);
			return;
		}
		const row = node("div", "mt-2 shrink-0 border-t border-outline-variant pt-2");
		actions.forEach(function (action) {
			const control = primaryActionControl(action, id, false);
			if (control) row.appendChild(control);
		});
		if (row.childElementCount) shell.appendChild(row);
	}

	async function loadWidget(card, item) {
		const previous = widgetLoads.get(item.id);
		if (previous) previous.abort();
		const controller = new AbortController();
		widgetLoads.set(item.id, controller);
		const body = card.querySelector("[data-content]");
		body.replaceChildren(node("p", "p-2 text-sm text-on-surface-variant", "Loading…"));
		card.querySelector("[data-compact-action]").replaceChildren();
		try {
			const data = await api(widgetURL(item.id, item.size), {signal: controller.signal});
			if (controller.signal.aborted || !card.isConnected) return;
			const shell = node("div", "flex h-full min-h-0 flex-col");
			const content = node("div", "min-h-0 flex-1 overflow-auto");
			if (data.weather) {
				renderWeather(content, data, item.size);
			} else if (data.sections && data.sections.length) {
				if (!appendSections(content, data.sections, item.size)) {
					content.appendChild(node("p", "p-3 text-sm text-on-surface-variant", data.emptyState || "Nothing to show"));
				}
			} else if (data.items && data.items.length) {
				appendItems(content, data.items, item.size);
			} else {
				content.appendChild(node("p", "p-3 text-sm text-on-surface-variant", data.emptyState || "Nothing to show"));
			}
			shell.appendChild(content);
			appendActions(shell, data.actions, item.id, item.size, card);
			body.replaceChildren(shell);
		} catch (error) {
			if (error.name === "AbortError") return;
			body.replaceChildren(node("p", "p-3 text-sm text-error", "Widget unavailable"));
			const retry = node("button", "mx-3 h-9 rounded-full bg-error-container px-4 text-sm text-error", "Retry");
			retry.type = "button";
			retry.onclick = function () { loadWidget(card, item); };
			body.appendChild(retry);
		} finally {
			if (widgetLoads.get(item.id) === controller) widgetLoads.delete(item.id);
		}
	}

	function renderCatalog() {
		catalog.classList.toggle("hidden", !editing);
		if (!editing) {
			catalog.replaceChildren();
			return;
		}
		catalog.replaceChildren();
		definitions.forEach(function (definition) {
			const added = layout.some(function (item) { return item.id === definition.id; });
			const button = node("button", "mr-2 h-9 rounded-full px-3 text-sm " +
				(added ? "bg-surface-container text-on-surface-variant" : "bg-primary-container text-primary"));
			button.type = "button";
			button.disabled = added;
			button.dataset.add = definition.id;
			const appIcon = icon(definition.icon);
			appIcon.classList.add("mr-2");
			button.append(appIcon, document.createTextNode(definition.title + (added ? " Added" : "")));
			if (!added) button.onclick = function () { addWidget(definition); };
			catalog.appendChild(button);
		});
	}

	function render() {
		widgetLoads.forEach(function (controller) { controller.abort(); });
		widgetLoads.clear();
		grid.replaceChildren();
		layout.forEach(function (item) {
			const definition = definitionFor(item.id);
			if (!definition) return;
			const card = itemNode(item, definition);
			grid.appendChild(card);
			const header = card.querySelector("[data-drag]");
			const resize = card.querySelector("[data-resize]");
			card.querySelector("[data-settings]").classList.toggle("hidden", !editing);
			card.querySelector("[data-remove]").classList.toggle("hidden", !editing);
			card.querySelector("[data-size-badge]").classList.toggle("hidden", !editing);
			resize.classList.add("hidden");
			resize.classList.toggle("md:flex", editing);
			header.classList.toggle("cursor-move", editing);
			header.onpointerdown = function (event) { startPointer(event, item.id, false); };
			resize.onpointerdown = function (event) { startPointer(event, item.id, true); };
			loadWidget(card, item);
		});
		renderCatalog();
	}

	function setEditing(value) {
		editing = value;
		editButton.setAttribute("aria-pressed", String(editing));
		editButton.setAttribute("aria-label", editing ? "Finish editing dashboard" : "Edit dashboard");
		editButton.title = editing ? "Finish editing dashboard" : "Edit dashboard";
		editButton.classList.toggle("bg-primary-container", editing);
		editButton.replaceChildren(icon(editing ? "fa-solid fa-check" : "fa-solid fa-pen"));
		render();
		announce(editing ? "Dashboard editing enabled" : "Dashboard editing finished");
	}

	function startPointer(event, id, resizing) {
		if (!editing || !window.matchMedia("(min-width: 768px)").matches || event.button !== 0) return;
		if (!resizing && event.target.closest("button,a,input,select")) return;
		event.preventDefault();
		const item = layout.find(function (candidate) { return candidate.id === id; });
		const rect = grid.getBoundingClientRect();
		drag = {
			id: id,
			resizing: resizing,
			startX: event.clientX,
			startY: event.clientY,
			startItem: Object.assign({}, item),
			startLayout: layout.map(function (candidate) { return Object.assign({}, candidate); }),
			columnPitch: (rect.width + 12) / 12,
			rowPitch: 84
		};
		window.addEventListener("pointermove", movePointer);
		window.addEventListener("pointerup", endPointer, {once: true});
	}

	function nearestPreset(definition, width, height) {
		let best = supportedPresets(definition)[0];
		let bestDistance = Infinity;
		let bestArea = Infinity;
		supportedPresets(definition).forEach(function (preset) {
			const distance = Math.abs(width - preset.w) + Math.abs(height - preset.h);
			const area = Math.abs(width * height - preset.w * preset.h);
			if (distance < bestDistance || (distance === bestDistance && area < bestArea)) {
				best = preset;
				bestDistance = distance;
				bestArea = area;
			}
		});
		return best;
	}

	function movePointer(event) {
		if (!drag) return;
		const dx = Math.round((event.clientX - drag.startX) / drag.columnPitch);
		const dy = Math.round((event.clientY - drag.startY) / drag.rowPitch);
		const candidate = drag.startLayout.map(function (item) { return Object.assign({}, item); });
		const active = candidate.find(function (item) { return item.id === drag.id; });
		if (drag.resizing) {
			const startPreset = presetFor(drag.startItem.size);
			const snapped = nearestPreset(definitionFor(active.id), startPreset.w + dx, startPreset.h + dy);
			active.size = snapped.size;
		} else {
			const preset = presetFor(active.size);
			active.x = Math.min(Math.max(drag.startItem.x + dx, 0), 12 - preset.w);
			active.y = Math.max(drag.startItem.y + dy, 0);
		}
		layout = reflow(candidate, drag.id);
		applyGeometry();
	}

	async function endPointer() {
		if (!drag) return;
		const activeID = drag.id;
		drag = null;
		window.removeEventListener("pointermove", movePointer);
		try {
			await persistLayout(activeID);
			announce("Dashboard layout saved");
		} catch (error) {
			announce("Dashboard layout could not be saved");
		}
	}

	async function persistLayout(activeID) {
		const snapshot = layout.map(function (item) {
			return {id: item.id, x: item.x, y: item.y, size: item.size};
		});
		const operation = saveQueue.catch(function () {}).then(async function () {
			const response = await api("/api/dashboard/layout", {
				method: "PUT",
				headers: {"Content-Type": "application/json"},
				body: JSON.stringify({layout: snapshot, activeId: activeID || ""})
			});
			layout = response.layout || [];
			render();
			return response;
		});
		saveQueue = operation;
		try {
			return await operation;
		} catch (error) {
			try {
				const authoritative = await api("/api/dashboard/layout");
				layout = authoritative.layout || [];
				render();
			} catch (_) {}
			throw error;
		}
	}

	async function addWidget(definition) {
		const bottom = layout.reduce(function (value, item) {
			return Math.max(value, item.y + presetFor(item.size).h);
		}, 0);
		layout.push({id: definition.id, x: 0, y: bottom, size: definition.defaultSize});
		layout = reflow(layout, definition.id);
		await persistLayout(definition.id);
		announce(definition.title + " added");
	}

	async function removeWidget(id) {
		const definition = definitionFor(id);
		layout = reflow(layout.filter(function (item) { return item.id !== id; }), "");
		try {
			await persistLayout("");
			announce(definition.title + " removed");
		} catch (_) {
			announce(definition.title + " could not be removed");
		}
	}

	function renderSizeOptions(definition, selected) {
		settingsSizeOptions.replaceChildren();
		supportedPresets(definition).forEach(function (preset) {
			const label = node("label", "cursor-pointer");
			const input = node("input", "peer sr-only");
			input.type = "radio";
			input.name = "widget_size";
			input.value = preset.size;
			input.checked = preset.size === selected;
			const surface = node("span", "flex min-h-16 flex-col rounded-2xl border border-outline-variant p-3 peer-checked:border-primary peer-checked:bg-primary-container peer-checked:text-primary");
			surface.append(
				node("span", "font-medium", preset.label),
				node("span", "text-xs text-on-surface-variant", preset.w + " × " + preset.h)
			);
			label.append(input, surface);
			settingsSizeOptions.appendChild(label);
		});
	}

	function renderSettingsFields(fields) {
		settingsFields.replaceChildren();
		fields.forEach(function (field) {
			const wrap = node("label", "block text-sm font-medium", field.label || field.name);
			let input;
			if (field.type === "select") {
				input = node("select", "mt-1 h-11 w-full rounded-xl border-0 bg-surface-container px-3");
				(field.options || []).forEach(function (option) {
					const element = node("option", "", option.label);
					element.value = option.value;
					element.selected = option.value === field.value;
					input.appendChild(element);
				});
			} else {
				input = node("input", "mt-1 h-11 w-full rounded-full border-0 bg-surface-container px-4");
				input.type = "text";
				input.value = field.value || "";
				input.placeholder = field.placeholder || "";
			}
			input.name = field.name;
			input.required = Boolean(field.required);
			wrap.appendChild(input);
			if (field.help) wrap.appendChild(node("span", "mt-1 block text-xs font-normal text-on-surface-variant", field.help));
			settingsFields.appendChild(wrap);
		});
	}

	async function openSettings(id, trigger) {
		if (!editing) return;
		const definition = definitionFor(id);
		const item = layout.find(function (candidate) { return candidate.id === id; });
		settingsWidget = id;
		settingsTrigger = trigger || document.activeElement;
		providerFields = [];
		settingsMessage.className = "hidden mt-3 text-sm";
		settingsMessage.textContent = "";
		document.getElementById("dashboard-settings-title").textContent = definition.title + " settings";
		renderSizeOptions(definition, item.size);
		settingsProvider.classList.toggle("hidden", !definition.configurable);
		settingsFields.replaceChildren();
		if (definition.configurable) settingsFields.appendChild(node("p", "text-sm text-on-surface-variant", "Loading settings…"));
		settingsModal.classList.remove("hidden");
		settingsModal.classList.add("flex");
		if (definition.configurable) {
			try {
				const response = await api(widgetURL(id, item.size, "/settings"));
				providerFields = response.fields || [];
				renderSettingsFields(providerFields);
			} catch (error) {
				settingsFields.replaceChildren(node("p", "text-sm text-error", error.message));
			}
		}
		const selected = settingsSizeOptions.querySelector('input[name="widget_size"]:checked');
		if (selected) selected.focus();
	}

	function closeSettings() {
		settingsModal.classList.add("hidden");
		settingsModal.classList.remove("flex");
		settingsWidget = "";
		settingsSubmit.disabled = false;
		if (settingsTrigger) settingsTrigger.focus();
		settingsTrigger = null;
	}

	settingsForm.onsubmit = async function (event) {
		event.preventDefault();
		const id = settingsWidget;
		const definition = definitionFor(id);
		const item = layout.find(function (candidate) { return candidate.id === id; });
		const form = new FormData(settingsForm);
		const requestedSize = String(form.get("widget_size") || item.size);
		settingsSubmit.disabled = true;
		settingsMessage.className = "mt-3 text-sm text-on-surface-variant";
		settingsMessage.textContent = "Saving…";
		settingsMessage.classList.remove("hidden");
		let providerSaved = false;
		try {
			if (definition.configurable) {
				const values = {};
				providerFields.forEach(function (field) {
					values[field.name] = String(form.get(field.name) || "");
				});
				const response = await api(widgetURL(id, item.size, "/settings"), {
					method: "PUT",
					headers: {"Content-Type": "application/json"},
					body: JSON.stringify({values: values})
				});
				if (response.warning) {
					settingsMessage.className = "mt-3 text-sm text-warning";
					settingsMessage.textContent = response.warning;
					providerFields = response.fields || [];
					renderSettingsFields(providerFields);
					settingsSubmit.disabled = false;
					return;
				}
				providerSaved = true;
			}
			item.size = requestedSize;
			layout = reflow(layout, id);
			await persistLayout(id);
			closeSettings();
			announce(definition.title + " settings saved");
		} catch (error) {
			settingsMessage.className = "mt-3 text-sm text-error";
			settingsMessage.textContent = providerSaved ?
				"Content settings were saved, but the widget size was not. " + error.message :
				error.message;
			settingsSubmit.disabled = false;
		}
	};

	document.getElementById("dashboard-settings-close").onclick = closeSettings;
	document.getElementById("dashboard-settings-cancel").onclick = closeSettings;
	settingsModal.onclick = function (event) {
		if (event.target === settingsModal) closeSettings();
	};
	document.addEventListener("keydown", function (event) {
		if (settingsModal.classList.contains("hidden")) return;
		if (event.key === "Escape") {
			event.preventDefault();
			closeSettings();
			return;
		}
		if (event.key !== "Tab") return;
		const focusable = Array.from(settingsForm.querySelectorAll("button,input,select")).filter(function (element) {
			return !element.disabled && element.offsetParent !== null;
		});
		if (!focusable.length) return;
		const first = focusable[0];
		const last = focusable[focusable.length - 1];
		if (event.shiftKey && document.activeElement === first) {
			event.preventDefault();
			last.focus();
		} else if (!event.shiftKey && document.activeElement === last) {
			event.preventDefault();
			first.focus();
		}
	});

	editButton.onclick = function () { setEditing(!editing); };

	Promise.all([api("/api/dashboard/catalog"), api("/api/dashboard/layout")]).then(function (responses) {
		presets = responses[0].presets || [];
		definitions = responses[0].widgets || [];
		layout = responses[1].layout || [];
		render();
	}).catch(function () {
		grid.replaceChildren(node("p", "text-error", "Dashboard unavailable"));
	});
})();
