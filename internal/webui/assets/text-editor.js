(function () {
	"use strict";

	function esc(value) {
		const node = document.createElement("div");
		node.textContent = value == null ? "" : String(value);
		return node.innerHTML;
	}

	function safeURL(value) {
		try {
			const url = new URL(value, location.origin);
			if (["http:", "https:", "mailto:", "tel:"].includes(url.protocol) ||
				value.startsWith("./") || value.startsWith("../") ||
				value.startsWith("/") || !value.includes(":")) return value;
		} catch (_) {}
		return "";
	}

	function inlineMarkdown(markdown, options) {
		let text = esc(markdown);
		const code = [];
		text = text.replace(/`([^`]+)`/g, function (_, value) {
			code.push('<code class="bg-surface-container-high px-1 rounded text-sm">' + value + "</code>");
			return "@@DARCODE" + (code.length - 1) + "@@";
		});
		text = text.replace(/!\[([^\]]*)\]\(([^)]+)\)/g, function (_, alt, rawURL) {
			const safe = safeURL(rawURL);
			if (!safe) return esc("![" + alt + "](" + rawURL + ")");
			const resolved = options && options.resolveImage ? options.resolveImage(safe) : safe;
			if (!resolved) return esc("![" + alt + "](" + rawURL + ")");
			return '<img src="' + esc(resolved) + '" data-markdown-src="' + esc(rawURL) +
				'" alt="' + esc(alt) + '" class="max-w-full rounded-md my-3">';
		});
		text = text.replace(/\[([^\]]+)\]\(([^)]+)\)/g, function (_, label, rawURL) {
			const safe = safeURL(rawURL);
			return safe ? '<a href="' + esc(safe) +
				'" class="text-primary underline" rel="noreferrer noopener">' + label + "</a>" :
				esc("[" + label + "](" + rawURL + ")");
		});
		text = text.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
			.replace(/__([^_]+)__/g, "<strong>$1</strong>")
			.replace(/(^|[^*])\*([^*]+)\*/g, "$1<em>$2</em>")
			.replace(/  \n/g, "<br>");
		return text.replace(/@@DARCODE(\d+)@@/g, function (_, index) {
			return code[Number(index)];
		});
	}

	function renderMarkdown(markdown, options) {
		const lines = String(markdown || "").replace(/\r/g, "").split("\n");
		const out = [];
		for (let i = 0; i < lines.length;) {
			const line = lines[i];
			if (/^```/.test(line)) {
				const language = line.slice(3).trim();
				const body = [];
				i++;
				while (i < lines.length && !/^```/.test(lines[i])) body.push(lines[i++]);
				if (i < lines.length) i++;
				out.push('<pre data-block="code" data-lang="' + esc(language) +
					'" class="bg-surface-container-low rounded-md p-3 overflow-x-auto my-3"><code>' +
					esc(body.join("\n")) + "</code></pre>");
				continue;
			}
			if (/^\s*<[^>]+>/.test(line) || /^:::+/.test(line) || /^\[\^[^\]]+\]:/.test(line)) {
				out.push('<pre data-block="raw" class="whitespace-pre-wrap bg-surface-container-low rounded-md p-3 my-3">' +
					esc(line) + "</pre>");
				i++;
				continue;
			}
			const heading = line.match(/^(#{1,6})\s+(.*)$/);
			if (heading) {
				const level = heading[1].length;
				out.push("<h" + level + ' data-block="heading" class="font-semibold mt-5 mb-2 ' +
					(level === 1 ? "text-2xl" : level === 2 ? "text-xl" : "text-lg") + '">' +
					inlineMarkdown(heading[2], options) + "</h" + level + ">");
				i++;
				continue;
			}
			if (/^>\s?/.test(line)) {
				const body = [];
				while (i < lines.length && /^>\s?/.test(lines[i])) body.push(lines[i++].replace(/^>\s?/, ""));
				out.push('<blockquote data-block="quote" class="border-l-4 border-primary pl-4 my-3 text-on-surface-variant">' +
					body.map(function (value) { return inlineMarkdown(value, options); }).join("<br>") + "</blockquote>");
				continue;
			}
			if (/^\s*[-*+]\s+/.test(line)) {
				const items = [];
				while (i < lines.length && /^\s*[-*+]\s+/.test(lines[i])) {
					const raw = lines[i++].replace(/^\s*[-*+]\s+/, "");
					const task = raw.match(/^\[([ xX])\]\s+(.*)$/);
					items.push(task ?
						'<li data-task class="flex gap-2"><input type="checkbox" ' +
						(task[1].toLowerCase() === "x" ? "checked" : "") + "><span>" +
						inlineMarkdown(task[2], options) + "</span></li>" :
						"<li>" + inlineMarkdown(raw, options) + "</li>");
				}
				out.push('<ul data-block="ul" class="list-disc ml-6 my-3 space-y-1">' + items.join("") + "</ul>");
				continue;
			}
			if (/^\s*\d+\.\s+/.test(line)) {
				const items = [];
				while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) {
					items.push("<li>" + inlineMarkdown(lines[i++].replace(/^\s*\d+\.\s+/, ""), options) + "</li>");
				}
				out.push('<ol data-block="ol" class="list-decimal ml-6 my-3 space-y-1">' + items.join("") + "</ol>");
				continue;
			}
			if (i + 1 < lines.length && line.includes("|") && /^\s*\|?\s*:?-+/.test(lines[i + 1])) {
				const cells = function (value) {
					return value.replace(/^\||\|$/g, "").split("|").map(function (cell) { return cell.trim(); });
				};
				const headers = cells(line);
				const rows = [];
				i += 2;
				while (i < lines.length && lines[i].includes("|")) rows.push(cells(lines[i++]));
				out.push('<div data-block="table" class="overflow-x-auto my-3"><table class="w-full border-collapse text-sm"><thead><tr>' +
					headers.map(function (cell) {
						return '<th class="border border-outline-variant p-2 text-left">' +
							inlineMarkdown(cell, options) + "</th>";
					}).join("") + "</tr></thead><tbody>" + rows.map(function (row) {
						return "<tr>" + row.map(function (cell) {
							return '<td class="border border-outline-variant p-2">' +
								inlineMarkdown(cell, options) + "</td>";
						}).join("") + "</tr>";
					}).join("") + "</tbody></table></div>");
				continue;
			}
			if (line.trim() === "") {
				out.push('<p data-block="blank"><br></p>');
				i++;
				continue;
			}
			const paragraph = [line];
			i++;
			while (i < lines.length && lines[i].trim() !== "" &&
				!/^(#{1,6})\s|^```|^>\s?|^\s*[-*+]\s+|^\s*\d+\.\s+/.test(lines[i])) {
				paragraph.push(lines[i++]);
			}
			out.push('<p data-block="p" class="my-2">' +
				inlineMarkdown(paragraph.join("\n"), options) + "</p>");
		}
		return out.join("");
	}

	function inlineFromDOM(node) {
		if (node.nodeType === Node.TEXT_NODE) return node.nodeValue;
		const children = Array.from(node.childNodes).map(inlineFromDOM).join("");
		switch (node.nodeName) {
		case "STRONG":
		case "B":
			return "**" + children + "**";
		case "EM":
		case "I":
			return "*" + children + "*";
		case "CODE":
			return "`" + children + "`";
		case "A":
			return "[" + children + "](" + (safeURL(node.getAttribute("href")) || "") + ")";
		case "IMG":
			return "![" + (node.getAttribute("alt") || "") + "](" +
				(node.getAttribute("data-markdown-src") || safeURL(node.getAttribute("src")) || "") + ")";
		case "BR":
			return "  \n";
		default:
			return children;
		}
	}

	function richToMarkdown(root) {
		const lines = [];
		for (const block of root.children) {
			const type = block.dataset.block || block.tagName.toLowerCase();
			if (type === "blank") {
				lines.push(block.textContent ? inlineFromDOM(block) : "");
				continue;
			}
			if (type === "raw") {
				lines.push(block.textContent);
				continue;
			}
			if (type === "code") {
				lines.push("```" + (block.dataset.lang || ""), block.textContent, "```");
				continue;
			}
			if (type === "heading") {
				lines.push("#".repeat(Number(block.tagName.slice(1))) + " " + inlineFromDOM(block));
				continue;
			}
			if (type === "quote") {
				lines.push.apply(lines, inlineFromDOM(block).split("\n").map(function (line) { return "> " + line; }));
				continue;
			}
			if (type === "ul" || type === "ol") {
				Array.from(block.querySelectorAll(":scope > li")).forEach(function (item, index) {
					const task = item.hasAttribute("data-task");
					const content = inlineFromDOM(task ? item.querySelector("span") : item);
					lines.push(type === "ol" ? (index + 1) + ". " + content :
						"- " + (task ? "[" + (item.querySelector("input").checked ? "x" : " ") + "] " : "") + content);
				});
				continue;
			}
			if (type === "table") {
				const rows = Array.from(block.querySelectorAll("tr")).map(function (row) {
					return Array.from(row.children).map(inlineFromDOM);
				});
				if (rows.length) {
					lines.push("| " + rows[0].join(" | ") + " |",
						"| " + rows[0].map(function () { return "---"; }).join(" | ") + " |");
					rows.slice(1).forEach(function (row) { lines.push("| " + row.join(" | ") + " |"); });
				}
				continue;
			}
			lines.push(inlineFromDOM(block));
		}
		return lines.join("\n").replace(/\n{3,}/g, "\n\n");
	}

	function create(config) {
		const elements = config.elements;
		let drafts = {};
		let storageOK = true;
		let current = null;
		let mode = config.initialMode || "rich";
		let dirty = false;
		let draftTimer = null;
		const queues = new Map();

		try {
			drafts = JSON.parse(localStorage.getItem(config.storageKey) || "{}") || {};
			if (config.migrateDraft) {
				Object.keys(drafts).forEach(function (key) {
					drafts[key] = config.migrateDraft(drafts[key], key);
				});
			}
		} catch (_) {
			drafts = {};
			storageOK = false;
		}

		function persistDrafts() {
			try {
				localStorage.setItem(config.storageKey, JSON.stringify(drafts));
				storageOK = true;
			} catch (_) {
				storageOK = false;
				setStatus("Local backup unavailable");
			}
		}

		function scheduleDraftStorage() {
			clearTimeout(draftTimer);
			draftTimer = setTimeout(persistDrafts, 75);
		}

		function documentKey(document) {
			return String(document && document.key != null ? document.key : "");
		}

		function setStatus(text) {
			if (elements.status) elements.status.textContent = text;
			if (config.onStatus) config.onStatus(text, current);
		}

		function content() {
			if (!current) return "";
			if (!current.markdown) return elements.source.value;
			if (mode === "source") return elements.source.value;
			if (mode === "rich") return richToMarkdown(elements.rich);
			return current.content || "";
		}

		function renderCurrent() {
			if (!current) return;
			const options = {
				resolveImage: function (url) {
					return config.resolveImage ? config.resolveImage(url, current) : url;
				},
			};
			elements.source.value = current.content || "";
			if (elements.rich) elements.rich.innerHTML = renderMarkdown(current.content || "", options);
			if (elements.preview) elements.preview.innerHTML = renderMarkdown(current.content || "", options);
			const markdown = !!current.markdown;
			if (!markdown) mode = "source";
			if (elements.modeControls) elements.modeControls.classList.toggle("hidden", !markdown);
			if (elements.toolbar) elements.toolbar.classList.toggle("hidden", !markdown || mode !== "rich");
			if (elements.rich) elements.rich.classList.toggle("hidden", !markdown || mode !== "rich");
			elements.source.classList.toggle("hidden", markdown && mode !== "source");
			if (elements.preview) elements.preview.classList.toggle("hidden", !markdown || mode !== "preview");
			if (elements.modeControls) {
				for (const button of elements.modeControls.querySelectorAll("[data-editor-mode]")) {
					const active = button.dataset.editorMode === mode;
					button.classList.toggle("bg-surface", active);
					button.classList.toggle("shadow-1", active);
				}
			}
		}

		function setMode(next, remember) {
			if (current && !current.markdown) next = "source";
			if (current) current.content = content();
			mode = next;
			if (current) renderCurrent();
			if (remember !== false && config.onModeChange) config.onModeChange(mode);
		}

		function applyDocument(document, isDirty) {
			current = Object.assign({}, document);
			dirty = !!isDirty;
			if (config.applyMetadata) config.applyMetadata(current.metadata || {}, current);
			if (current.markdown && mode === "preview" && !elements.preview) mode = "rich";
			renderCurrent();
			setStatus(dirty ? "Unsaved" : "Saved");
			if (config.onDocument) config.onDocument(current, dirty);
		}

		function draftMatchesDocument(draft, document) {
			return draft.content === document.content &&
				JSON.stringify(draft.metadata || {}) === JSON.stringify(document.metadata || {});
		}

		function queueFor(key) {
			let queue = queues.get(key);
			if (!queue) {
				queue = {saving: false, pending: false, timer: null, retry: 0};
				queues.set(key, queue);
			}
			return queue;
		}

		function removeDraft(key) {
			delete drafts[key];
			persistDrafts();
			const queue = queues.get(key);
			if (queue && queue.timer) clearTimeout(queue.timer);
			queues.delete(key);
		}

		function captureDraft() {
			if (!current) return null;
			current.content = content();
			const key = documentKey(current);
			const previous = drafts[key];
			const draft = {
				key: key,
				baseETag: previous ? previous.baseETag : current.etag,
				content: current.content,
				metadata: config.readMetadata ? config.readMetadata(current) : (current.metadata || {}),
				markdown: current.markdown,
				revision: (previous ? previous.revision : 0) + 1,
				updatedAt: Date.now(),
			};
			drafts[key] = draft;
			current.metadata = draft.metadata;
			dirty = true;
			scheduleDraftStorage();
			return draft;
		}

		function queueSave(key, delay) {
			const queue = queueFor(key);
			if (queue.saving) {
				queue.pending = true;
				return;
			}
			clearTimeout(queue.timer);
			queue.timer = setTimeout(function () { runSave(key); }, delay == null ? 1000 : delay);
		}

		function showConflict(local, server) {
			const useLocal = function () {
				const key = local.key;
				const draft = drafts[key] || local;
				draft.baseETag = server.etag;
				draft.revision = (draft.revision || 0) + 1;
				drafts[key] = draft;
				persistDrafts();
				queueSave(key, 0);
			};
			const useServer = function () {
				removeDraft(local.key);
				applyDocument(server, false);
			};
			config.showConflict(local, server, useLocal, useServer);
		}

		async function runSave(key) {
			const queue = queueFor(key);
			const draft = drafts[key];
			if (!draft || queue.saving) return;
			queue.saving = true;
			queue.timer = null;
			queue.pending = false;
			const sent = Object.assign({}, draft);
			setStatus("Saving");
			let retryDelay = 0;
			try {
				const saved = await config.save(sent, current);
				queue.retry = 0;
				const latest = drafts[key];
				if (latest && latest.revision === sent.revision) {
					removeDraft(key);
					if (saved && documentKey(saved) !== key) removeDraft(documentKey(saved));
					if (current && (documentKey(current) === key || documentKey(current) === documentKey(saved))) {
						applyDocument(saved, false);
					}
				} else if (latest) {
					latest.baseETag = saved.etag;
					if (documentKey(saved) !== key) {
						delete drafts[key];
						latest.key = documentKey(saved);
						drafts[latest.key] = latest;
					}
					scheduleDraftStorage();
					queue.pending = true;
					if (current && (documentKey(current) === key || documentKey(current) === documentKey(saved))) {
						current = Object.assign({}, saved, {content: latest.content, metadata: latest.metadata});
					}
				}
				if (config.onSaved) config.onSaved(saved, sent);
			} catch (error) {
				if (error.status === 412 && error.data) {
					if (current && documentKey(current) === key) showConflict(sent, error.data);
				} else if ([400, 409, 413, 415, 428, 507].includes(error.status)) {
					setStatus(error.message || "Could not save");
					if (config.onSaveError) config.onSaveError(error, current);
				} else {
					const delays = [3, 6, 12, 24, 30];
					retryDelay = delays[Math.min(queue.retry, delays.length - 1)] * 1000;
					queue.retry++;
					setStatus(storageOK ? "Saved locally — retrying" : "Local backup unavailable");
					if (config.onSaveError) config.onSaveError(error, current);
				}
			} finally {
				queue.saving = false;
				if (retryDelay) queueSave(key, retryDelay);
				else if (queue.pending) {
					queue.pending = false;
					queueSave(documentKey(current), 0);
				}
			}
		}

		function markDirty() {
			const draft = captureDraft();
			if (!draft) return;
			setStatus(storageOK ? "Unsaved" : "Local backup unavailable");
			queueSave(draft.key, 1000);
			if (config.onDirty) config.onDirty(current);
		}

		function saveNow() {
			const draft = captureDraft();
			if (!draft) return Promise.resolve();
			persistDrafts();
			queueSave(draft.key, 0);
			return Promise.resolve();
		}

		function insertText(before, after) {
			after = after || "";
			if (!current || !current.markdown) return;
			if (mode !== "source") {
				document.execCommand(before === "**" ? "bold" : before === "*" ? "italic" : "insertText", false, before);
				markDirty();
				return;
			}
			const target = elements.source;
			const start = target.selectionStart;
			const end = target.selectionEnd;
			target.value = target.value.slice(0, start) + before + target.value.slice(start, end) +
				after + target.value.slice(end);
			target.selectionStart = start + before.length;
			target.selectionEnd = end + before.length;
			target.focus();
			markDirty();
		}

		function replaceContent(value) {
			if (!current) return;
			current.content = String(value == null ? "" : value);
			renderCurrent();
			markDirty();
		}

		function retryDrafts() {
			Object.keys(drafts).forEach(function (key) { queueSave(key, 0); });
		}

		async function open(document) {
			const key = documentKey(document);
			const draft = drafts[key];
			if (!draft) {
				applyDocument(document, false);
				return;
			}
			if (draftMatchesDocument(draft, document)) {
				removeDraft(key);
				applyDocument(document, false);
				return;
			}
			const local = Object.assign({}, document, {
				content: draft.content,
				metadata: draft.metadata,
			});
			applyDocument(local, true);
			if (draft.baseETag === document.etag) queueSave(key, 0);
			else showConflict(draft, document);
		}

		function close() {
			clearTimeout(draftTimer);
			if (current && dirty) saveNow();
			persistDrafts();
			current = null;
		}

		if (elements.rich) {
			elements.rich.addEventListener("input", markDirty);
			elements.rich.addEventListener("change", markDirty);
		}
		elements.source.addEventListener("input", markDirty);
		if (elements.modeControls) {
			for (const button of elements.modeControls.querySelectorAll("[data-editor-mode]")) {
				button.addEventListener("click", function () { setMode(button.dataset.editorMode); });
			}
		}
		if (elements.toolbar) {
			for (const button of elements.toolbar.querySelectorAll("[data-editor-format]")) {
				button.addEventListener("click", function () {
					switch (button.dataset.editorFormat) {
					case "bold": insertText("**", "**"); break;
					case "italic": insertText("*", "*"); break;
					case "h2": insertText("## "); break;
					case "link": insertText("[", "](https://)"); break;
					case "ul": insertText("- "); break;
					case "task": insertText("- [ ] "); break;
					case "quote": insertText("> "); break;
					default: insertText("`", "`");
					}
				});
			}
		}
		window.addEventListener("online", retryDrafts);
		window.addEventListener("keydown", function (event) {
			if (!current || !(event.ctrlKey || event.metaKey)) return;
			const key = event.key.toLowerCase();
			if (key === "s") {
				event.preventDefault();
				saveNow();
			} else if (current.markdown && key === "b") {
				event.preventDefault();
				insertText("**", "**");
			} else if (current.markdown && key === "i") {
				event.preventDefault();
				insertText("*", "*");
			}
		});
		window.addEventListener("beforeunload", function (event) {
			clearTimeout(draftTimer);
			persistDrafts();
			if (!storageOK && Object.keys(drafts).length) {
				event.preventDefault();
				event.returnValue = "";
			}
		});

		return {
			open: open,
			close: close,
			content: content,
			current: function () { return current; },
			isDirty: function () { return dirty; },
			markDirty: markDirty,
			saveNow: saveNow,
			setMode: setMode,
			insertText: insertText,
			replaceContent: replaceContent,
			retryDrafts: retryDrafts,
			discard: removeDraft,
			renderMarkdown: renderMarkdown,
		};
	}

	window.DarTextEditor = {
		create: create,
		renderMarkdown: renderMarkdown,
		richToMarkdown: richToMarkdown,
		safeURL: safeURL,
	};
}());
