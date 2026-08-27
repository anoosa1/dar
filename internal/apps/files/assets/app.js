(function () {
	"use strict";

	const apiBase = "/api/files";
	const app = document.getElementById("files-app");
	if (!app) return;

	const state = {
		activeView: app.dataset.activeView || "all",
		path: pathFromURL(),
		files: [],
		selected: new Set(),
		viewMode: localStorage.getItem("dar-files-view") || "list",
		sort: "name",
		sortDir: "asc",
		typeFilter: "",
		rowTarget: null,
			detail: null,
			tab: "sharing",
			tags: {},
			settings: defaultSettings(),
			editorCommittedName: "",
		};

	const el = {
		list: document.getElementById("files-list"),
		head: document.getElementById("files-list-head"),
		breadcrumbs: document.getElementById("files-breadcrumbs"),
		newButton: document.getElementById("files-new-button"),
		newMenu: document.getElementById("files-new-menu"),
		upload: document.getElementById("files-upload-input"),
		folder: document.getElementById("files-folder-input"),
		search: document.getElementById("files-search"),
		viewIcon: document.getElementById("files-view-icon"),
		checkAll: document.getElementById("files-check-all"),
		selection: document.getElementById("files-selection"),
		selectionCount: document.getElementById("files-selection-count"),
		rowMenu: document.getElementById("files-row-menu"),
		details: document.getElementById("files-details"),
		detailsIcon: document.getElementById("files-details-icon"),
		detailsTitle: document.getElementById("files-details-title"),
		detailsSubtitle: document.getElementById("files-details-subtitle"),
			preview: document.getElementById("files-preview"),
			detailsBody: document.getElementById("files-details-body"),
			drop: document.getElementById("files-drop"),
			settingsModal: document.getElementById("files-settings-modal"),
			settingsMessage: document.getElementById("files-settings-message"),
			editorOverlay: document.getElementById("files-editor-overlay"),
			editorName: document.getElementById("files-editor-name"),
			editorPath: document.getElementById("files-editor-path"),
			editorStatus: document.getElementById("files-editor-status"),
			editorRich: document.getElementById("files-editor-rich"),
			editorSource: document.getElementById("files-editor-source"),
			editorModes: document.getElementById("files-editor-modes"),
			editorToolbar: document.getElementById("files-editor-toolbar"),
			editorImage: document.getElementById("files-editor-image"),
			editorConflict: document.getElementById("files-editor-conflict"),
			createTextDialog: document.getElementById("files-create-text-dialog"),
			createTextName: document.getElementById("files-create-text-name"),
			createTextError: document.getElementById("files-create-text-error"),
			editorToast: document.getElementById("files-editor-toast"),
		};

	const editorModeKey = "dar.files.editor.mode:" + app.dataset.userId;
	const textEditor = window.DarTextEditor.create({
		storageKey: "dar.files.editor.drafts.v1:" + app.dataset.userId,
		initialMode: localStorage.getItem(editorModeKey) || "rich",
		elements: {
			rich: el.editorRich,
			source: el.editorSource,
			status: el.editorStatus,
			modeControls: el.editorModes,
			toolbar: el.editorToolbar,
		},
		readMetadata: function () {
			return {name: el.editorName.value};
		},
		applyMetadata: function (metadata, document) {
			el.editorName.value = metadata.name || document.name || baseName(document.key);
			state.editorCommittedName = el.editorName.value;
			el.editorPath.textContent = parentPath(document.key);
			el.editorImage.classList.toggle("hidden", !document.markdown);
		},
		save: saveEditorDraft,
		showConflict: showEditorConflict,
		resolveImage: resolveEditorImage,
		onModeChange: function (mode) {
			localStorage.setItem(editorModeKey, mode);
		},
		onSaved: editorSaved,
		onSaveError: function (error) {
			if (error.status === 409) editorToast("A file with that name already exists.");
			else if (error.status === 507) editorToast("Storage quota exceeded.");
			else if ([400, 413, 415, 428].includes(error.status)) editorToast(error.message || "Could not save file.");
		},
	});

	document.addEventListener("click", onDocumentClick);
	document.addEventListener("keydown", onKeydown);
	document.addEventListener("dragover", onDragOver);
	document.addEventListener("drop", onDrop);
	document.addEventListener("dragleave", onDragLeave);
	window.addEventListener("popstate", function () {
		if (state.detail && history.state?.darDetail !== "files") closeDetails(true);
		state.path = pathFromURL();
		state.activeView = viewFromURL();
		load();
	});

	el.newButton.addEventListener("click", function (ev) {
		ev.stopPropagation();
		el.newMenu.classList.toggle("hidden");
	});
	el.upload.addEventListener("change", function () { uploadInput(el.upload); });
	el.folder.addEventListener("change", function () { uploadInput(el.folder); });
	if (el.search) {
		el.search.addEventListener("keydown", function (ev) {
			if (ev.key === "Enter") runSearch();
		});
	}
	if (el.checkAll) {
		el.checkAll.addEventListener("change", function () {
			state.selected.clear();
			if (el.checkAll.checked) {
				for (const f of state.files) state.selected.add(f.path);
			}
			renderFiles();
		});
	}

	for (const btn of app.querySelectorAll("[data-action]")) {
		btn.addEventListener("click", function (ev) {
			const action = btn.getAttribute("data-action");
			if (action === "details-menu") {
				ev.stopPropagation();
				if (state.detail) openRowMenu(ev, state.detail.path);
				return;
			}
			handleAction(action);
		});
	}
	for (const btn of app.querySelectorAll("[data-row-action]")) {
		btn.addEventListener("click", function () {
			const action = btn.getAttribute("data-row-action");
			el.rowMenu.classList.add("hidden");
			handleRowAction(action, state.rowTarget);
		});
	}
	for (const btn of app.querySelectorAll("[data-tab]")) {
		btn.addEventListener("click", function () {
			state.tab = btn.getAttribute("data-tab");
			renderTabs();
			renderDetailsTab();
		});
	}
	for (const btn of app.querySelectorAll("[data-sort]")) {
		btn.addEventListener("click", function () {
			setSort(btn.getAttribute("data-sort"));
		});
	}
	for (const btn of app.querySelectorAll("[data-filter-type]")) {
		btn.addEventListener("click", function () {
			state.typeFilter = btn.getAttribute("data-filter-type");
			renderTypeChips();
			load();
		});
	}

		window.openSettings = openSettings;
		initializeSettingsFooter();
		for (const btn of app.querySelectorAll("[data-settings-close]")) {
			btn.addEventListener("click", closeSettings);
		}
		const saveSettingsButton = app.querySelector("[data-settings-save]");
		if (saveSettingsButton) saveSettingsButton.addEventListener("click", saveSettings);
		const copyWebDAVButton = app.querySelector("[data-settings-copy-webdav]");
		if (copyWebDAVButton) copyWebDAVButton.addEventListener("click", copyWebDAVURL);
		if (el.settingsModal) {
			el.settingsModal.addEventListener("click", closeSettings);
			const showHidden = document.getElementById("files-setting-show-hidden");
			if (showHidden) {
				showHidden.addEventListener("change", function () {
					state.settings.show_hidden = showHidden.checked;
					renderFiles();
				});
			}
		}
		document.getElementById("files-editor-close").addEventListener("click", closeEditor);
		document.getElementById("files-editor-close-secondary").addEventListener("click", closeEditor);
		document.getElementById("files-editor-save").addEventListener("click", function () { textEditor.saveNow(); });
		el.editorImage.addEventListener("click", insertEditorImage);
		el.editorName.addEventListener("keydown", function (ev) {
			if (ev.key === "Enter") {
				ev.preventDefault();
				commitEditorName();
			} else if (ev.key === "Escape") {
				ev.preventDefault();
				el.editorName.value = state.editorCommittedName;
				el.editorName.blur();
			}
		});
		el.editorName.addEventListener("blur", commitEditorName);
		document.getElementById("files-create-text-cancel").addEventListener("click", closeCreateTextDialog);
		document.getElementById("files-create-text-form").addEventListener("submit", createTextDocument);

		init();

		async function init() {
			await loadSettings();
			state.activeView = viewFromURL();
			if (state.settings.default_view === "grid" || state.settings.default_view === "list") {
				state.viewMode = localStorage.getItem("dar-files-view") || state.settings.default_view;
			}
			await loadTags();
			await load();
			const openPath = new URLSearchParams(location.search).get("open");
			if (openPath) {
				await openEditor({path: cleanPath(openPath), name: baseName(openPath), is_dir: false});
			}
		}

	function onDocumentClick(ev) {
		if (!ev.target.closest("#files-new-wrap")) el.newMenu.classList.add("hidden");
		if (!ev.target.closest("#files-row-menu")) el.rowMenu.classList.add("hidden");
	}

	function onKeydown(ev) {
		if (ev.key === "Escape" && !el.createTextDialog.classList.contains("hidden")) {
			closeCreateTextDialog();
			return;
		}
		if (ev.key === "Escape" && !el.editorOverlay.classList.contains("hidden") &&
			el.editorConflict.classList.contains("hidden")) {
			closeEditor();
			return;
		}
		const tag = ev.target.tagName;
		if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
		if (ev.key === "Delete" && state.selected.size > 0) {
			ev.preventDefault();
			bulkDelete();
		}
	}

	function handleAction(action) {
		switch (action) {
		case "upload-files":
			el.upload.click();
			break;
		case "upload-folder":
			el.folder.click();
			break;
		case "new-folder":
			newFolder();
			break;
		case "new-text":
			newTextFile();
			break;
		case "new-symlink":
			newSymlink();
			break;
		case "templates-folder":
			createFolder("Templates");
			break;
		case "folder-description":
			addFolderDescription();
			break;
		case "file-request":
			createFileRequest();
			break;
		case "bulk-delete":
			bulkDelete();
			break;
		case "bulk-move":
			bulkMove();
			break;
		case "search":
			runSearch();
			break;
		case "toggle-view":
			state.viewMode = state.viewMode === "list" ? "grid" : "list";
			localStorage.setItem("dar-files-view", state.viewMode);
			renderFiles();
			break;
		case "close-details":
			closeDetails();
			break;
		case "people-filter":
			state.activeView = "shares";
			history.pushState({}, "", "/files?view=shares");
			load();
			break;
		}
	}

	function handleRowAction(action, path) {
		if (!path) return;
		const file = state.files.find(function (f) { return f.path === path; }) || state.detail || {path: path, name: baseName(path)};
		switch (action) {
		case "favorite":
			apiPost("/favorite", {path: path}).then(load);
			break;
		case "details":
			openDetails(file);
			break;
		case "tags":
			manageTags(file);
			break;
		case "view":
			viewFile(file);
			break;
		case "rename":
			renameFile(file);
			break;
		case "move-copy":
			moveOrCopy(file);
			break;
		case "reminder":
			DarUI.toast("Reminder saved for " + file.name + ".");
			break;
		case "open-local":
		case "download":
			location.href = apiBase + "/download?path=" + encodeURIComponent(path);
			break;
		case "delete":
			deleteFile(file);
			break;
		}
	}

	async function load() {
		renderShell();
		state.selected.clear();
		try {
			if (state.activeView === "shares") {
				await loadSharesView();
			} else if (state.activeView === "tags") {
				await loadTagsView();
			} else if (state.activeView === "recent") {
				await loadRecentView();
			} else {
				await loadListView();
			}
		} catch (err) {
			el.list.innerHTML = '<div class="p-6 text-sm text-error">Could not load files.</div>';
		}
	}

	async function loadListView() {
		const params = {path: state.activeView === "trash" ? state.settings.trash_folder : state.path, trash_path: state.settings.trash_folder};
		if (state.activeView === "favorites") params.filter = "favorites";
		if (state.typeFilter) {
			const q = el.search ? el.search.value.trim() : "";
			const d = await apiGet("/search", {q: q, type: state.typeFilter, folder: state.path});
			state.files = d.files || [];
			state.path = d.path || state.path;
		} else {
			const d = await apiGet("/list", params);
			state.files = d.files || [];
			state.path = cleanPath(d.path || state.path);
		}
		renderFiles();
	}

	async function loadRecentView() {
		const response = await fetch("/api/activity?source=files&limit=50");
		if (!response.ok) throw new Error("Could not load recent activity");
		const d = await response.json();
		const acts = d.events || [];
		state.files = acts.map(function (a) {
			return {name: a.object_name || baseName(a.object_id), path: a.object_id, size: 0, updated_at: a.happened_at, is_dir: false, summary: a.summary};
		});
		renderFiles("Recent");
	}

	async function loadSharesView() {
		const own = await apiGet("/shared", {});
		const user = await apiGet("/shared-with-me", {});
		const files = [];
		for (const share of Object.values(own.shares || {})) {
			files.push({name: baseName(share.path), path: share.path, size: 0, updated_at: share.created, is_dir: false, summary: "Public link"});
		}
		for (const share of user.shares || []) {
			files.push({name: baseName(share.path) + " from " + share.from_user, path: "/Shared/" + share.id, size: 0, updated_at: share.created, is_dir: true, summary: "Shared with me · " + share.permission});
		}
		state.files = files;
		renderFiles("Shares");
	}

	async function loadTagsView() {
		await loadTags();
		const tags = Object.values(state.tags);
		if (!tags.length) {
			state.files = [];
			renderFiles("Tags");
			return;
		}
		el.head.classList.add("hidden");
		el.list.innerHTML = tags.map(function (tag) {
			return '<button type="button" data-open-tag="' + escAttr(tag.id) + '" class="w-full h-12 px-4 border-0 bg-transparent hover:bg-surface-container-high cursor-pointer flex items-center gap-3 text-left">' +
				'<i class="fa-solid fa-tag text-primary"></i><span class="text-sm font-medium">' + esc(tag.name) + '</span></button>';
		}).join("");
		for (const btn of el.list.querySelectorAll("[data-open-tag]")) {
			btn.addEventListener("click", async function () {
				const d = await apiGet("/list", {path: "/", filter: "tag", tag: btn.getAttribute("data-open-tag")});
				state.files = d.files || [];
				renderFiles("Tags");
			});
		}
		renderBreadcrumbs("Tags");
	}

	async function runSearch() {
		const q = el.search ? el.search.value.trim() : "";
		if (!q && !state.typeFilter) {
			load();
			return;
		}
		const d = await apiGet("/search", {q: q, type: state.typeFilter});
		state.files = d.files || [];
		state.activeView = "search";
		renderFiles("Search");
	}

	function renderShell() {
		renderBreadcrumbs();
		renderTypeChips();
		syncSidebar();
		el.viewIcon.className = state.viewMode === "grid" ? "fa-solid fa-grip" : "fa-solid fa-list";
	}

	function renderBreadcrumbs(label) {
		const title = label || activeTitle();
		if (state.activeView !== "all" && state.activeView !== "personal" && state.activeView !== "trash") {
			el.breadcrumbs.innerHTML = '<span class="text-on-surface font-medium truncate">' + esc(title) + '</span>';
			return;
		}
		const p = state.activeView === "trash" ? state.settings.trash_folder : cleanPath(state.path);
		const parts = p.split("/").filter(Boolean);
		let acc = "";
		let html = '<button type="button" data-nav="/" class="h-8 px-2 rounded-full border-0 bg-transparent text-primary cursor-pointer hover:bg-surface-container-high"><i class="fa-solid fa-folder mr-1"></i>Files</button>';
		for (const part of parts) {
			acc += "/" + part;
			html += '<span class="text-on-surface-variant">/</span><button type="button" data-nav="' + escAttr(acc) + '" class="h-8 px-2 rounded-full border-0 bg-transparent text-on-surface cursor-pointer hover:bg-surface-container-high max-w-40 truncate">' + esc(part) + '</button>';
		}
		el.breadcrumbs.innerHTML = html;
		for (const btn of el.breadcrumbs.querySelectorAll("[data-nav]")) {
			btn.addEventListener("click", function () { navigateTo(btn.getAttribute("data-nav")); });
		}
	}

	function renderTypeChips() {
		for (const btn of app.querySelectorAll("[data-filter-type]")) {
			const active = btn.getAttribute("data-filter-type") === state.typeFilter;
			btn.classList.toggle("bg-primary-container", active);
			btn.classList.toggle("text-primary", active);
			btn.classList.toggle("bg-surface-container", !active);
			btn.classList.toggle("text-on-surface-variant", !active);
		}
	}

	function renderFiles(label) {
		updateSelection();
		renderBreadcrumbs(label);
		syncSidebar();
		el.viewIcon.className = state.viewMode === "grid" ? "fa-solid fa-grip" : "fa-solid fa-list";
		const isTrash = state.activeView === "trash" || pathUnder(state.path, state.settings.trash_folder);
		const files = sortedFiles(state.files);
		el.head.classList.toggle("hidden", state.viewMode !== "list");
		el.head.classList.toggle("flex", state.viewMode === "list");
		if (el.checkAll) el.checkAll.checked = files.length > 0 && files.every(function (f) { return state.selected.has(f.path); });
		if (!files.length) {
			el.list.innerHTML = document.getElementById("files-empty-template").innerHTML;
			return;
		}
		el.list.innerHTML = state.viewMode === "grid" ? renderGrid(files, isTrash) : renderRows(files, isTrash);
		wireRows();
	}

	function renderRows(files, isTrash) {
		return '<div class="divide-y divide-outline-variant">' + files.map(function (f) {
			const checked = state.selected.has(f.path) ? " checked" : "";
			const icon = fileIcon(f);
			const summary = f.summary ? '<span class="text-xs text-on-surface-variant ml-2">' + esc(f.summary) + '</span>' : "";
			const tags = renderTags(f);
			const primaryAction = f.is_dir && !isTrash ? 'data-nav="' + escAttr(f.path) + '"' : 'data-details="' + escAttr(f.path) + '"';
			let actions = "";
			if (isTrash) {
				actions = '<button type="button" data-restore="' + escAttr(f.trash_path || f.path) + '" class="h-8 px-3 rounded-full border-0 bg-surface-container text-primary text-xs cursor-pointer">Restore</button>' +
					'<button type="button" data-permanent-delete="' + escAttr(f.trash_path || f.path) + '" class="w-8 h-8 rounded-full border-0 bg-transparent text-error cursor-pointer hover:bg-error-container"><i class="fa-solid fa-trash-can"></i></button>';
			} else {
				actions = '<button type="button" data-share="' + escAttr(f.path) + '" class="w-8 h-8 rounded-full border-0 bg-transparent text-on-surface-variant cursor-pointer hover:bg-surface-container-high" title="Share"><i class="fa-solid fa-user-plus"></i></button>' +
					'<button type="button" data-menu="' + escAttr(f.path) + '" class="w-8 h-8 rounded-full border-0 bg-transparent text-on-surface-variant cursor-pointer hover:bg-surface-container-high" title="Actions" aria-label="Actions for ' + escAttr(f.name) + '"><i class="fa-solid fa-ellipsis"></i></button>';
			}
			return '<div class="group h-14 px-4 flex items-center gap-3 hover:bg-surface-container-low">' +
				(isTrash ? '<div class="w-4"></div>' : '<input type="checkbox" data-select="' + escAttr(f.path) + '"' + checked + ' class="w-4 h-4 accent-primary cursor-pointer">') +
				'<i class="' + icon + ' text-lg w-7 text-center"></i>' +
				'<button type="button" ' + primaryAction + ' class="min-w-0 flex-1 border-0 bg-transparent text-left cursor-pointer">' +
				'<span class="text-sm font-medium text-on-surface truncate inline-block max-w-full align-bottom">' + esc(displayName(f)) + '</span>' + summary + tags + '</button>' +
				'<div class="w-24 text-right text-xs text-on-surface-variant hidden md:block">' + (f.is_dir ? "-" : formatSize(f.size || 0)) + '</div>' +
				'<div class="w-32 text-right text-xs text-on-surface-variant hidden lg:block">' + formatDate(f.updated_at) + '</div>' +
				'<div class="w-20 flex justify-end gap-1">' + actions + '</div></div>';
		}).join("") + '</div>';
	}

	function renderGrid(files, isTrash) {
		return '<div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-5 xl:grid-cols-7 gap-3 p-4">' + files.map(function (f) {
			const target = f.is_dir && !isTrash ? 'data-nav="' + escAttr(f.path) + '"' : 'data-details="' + escAttr(f.path) + '"';
			const thumb = f.mime_type && f.mime_type.indexOf("image/") === 0 ?
				'<img src="' + apiBase + '/thumb?path=' + encodeURIComponent(f.path) + '" alt="" class="w-full h-full ' + (state.settings.crop_image_previews ? "object-cover" : "object-contain") + '">' :
				'<i class="' + fileIcon(f) + ' text-4xl"></i>';
			return '<div class="group relative rounded-md border border-outline-variant bg-surface-container-low overflow-hidden hover:bg-surface-container-high">' +
				'<button type="button" ' + target + ' class="w-full aspect-square border-0 bg-transparent cursor-pointer flex items-center justify-center text-on-surface-variant overflow-hidden">' + thumb + '</button>' +
				'<div class="h-12 px-2 flex items-center gap-2">' +
				'<input type="checkbox" data-select="' + escAttr(f.path) + '" class="w-4 h-4 accent-primary cursor-pointer">' +
				'<span class="text-xs font-medium truncate flex-1">' + esc(displayName(f)) + '</span>' +
				'<button type="button" data-menu="' + escAttr(f.path) + '" class="w-7 h-7 rounded-full border-0 bg-transparent text-on-surface-variant cursor-pointer" aria-label="Actions for ' + escAttr(f.name) + '"><i class="fa-solid fa-ellipsis"></i></button>' +
				'</div></div>';
		}).join("") + '</div>';
	}

	function wireRows() {
		for (const btn of el.list.querySelectorAll("[data-nav]")) {
			btn.addEventListener("click", function () { navigateTo(btn.getAttribute("data-nav")); });
		}
		for (const btn of el.list.querySelectorAll("[data-details]")) {
			btn.addEventListener("click", function () {
				const f = state.files.find(function (x) { return x.path === btn.getAttribute("data-details"); });
				if (f) openEditor(f);
			});
		}
		for (const cb of el.list.querySelectorAll("[data-select]")) {
			cb.checked = state.selected.has(cb.getAttribute("data-select"));
			cb.addEventListener("change", function () {
				const p = cb.getAttribute("data-select");
				if (cb.checked) state.selected.add(p); else state.selected.delete(p);
				updateSelection();
			});
		}
		for (const btn of el.list.querySelectorAll("[data-menu]")) {
			btn.addEventListener("click", function (ev) {
				ev.stopPropagation();
				openRowMenu(ev, btn.getAttribute("data-menu"));
			});
		}
		for (const btn of el.list.querySelectorAll("[data-share]")) {
			btn.addEventListener("click", function () {
				const f = state.files.find(function (x) { return x.path === btn.getAttribute("data-share"); });
				if (f) {
					state.tab = "sharing";
					openDetails(f);
				}
			});
		}
		for (const btn of el.list.querySelectorAll("[data-restore]")) {
			btn.addEventListener("click", function () { apiPost("/restore", {id: btn.getAttribute("data-restore"), trash_path: state.settings.trash_folder}).then(load); });
		}
		for (const btn of el.list.querySelectorAll("[data-permanent-delete]")) {
			btn.addEventListener("click", async function () {
				if (await DarUI.confirmDialog({title: "Delete permanently?", message: "This item cannot be restored.", confirmLabel: "Delete", tone: "danger"})) {
					apiPost("/delete-permanent", {id: btn.getAttribute("data-permanent-delete"), trash_path: state.settings.trash_folder}).then(load);
				}
			});
		}
	}

	function openRowMenu(ev, path) {
		state.rowTarget = path;
		el.rowMenu.classList.remove("hidden");
		const rect = ev.currentTarget.getBoundingClientRect();
		const left = Math.min(rect.left, window.innerWidth - 240);
		const top = Math.min(rect.bottom + 4, window.innerHeight - 390);
		el.rowMenu.style.left = left + "px";
		el.rowMenu.style.top = top + "px";
	}

	function openDetails(file) {
		state.detail = file;
		if (DarUI.shell.isCompact() && history.state?.darDetail !== "files") {
			history.pushState(Object.assign({}, history.state, {darDetail:"files"}), "", location.href);
		}
		DarUI.shell.openDetail(el.details, document.activeElement);
		el.detailsIcon.innerHTML = '<i class="' + fileIcon(file) + '"></i>';
		el.detailsTitle.textContent = file.name;
		el.detailsSubtitle.textContent = (file.is_dir ? "Folder" : formatSize(file.size || 0)) + " · " + formatDate(file.updated_at);
		if (file.mime_type && file.mime_type.indexOf("image/") === 0) {
			el.preview.innerHTML = '<img src="' + apiBase + '/download?path=' + encodeURIComponent(file.path) + '" alt="" class="w-full h-full ' + (state.settings.crop_image_previews ? "object-cover" : "object-contain") + '">';
		} else {
			el.preview.innerHTML = '<i class="' + fileIcon(file) + ' text-5xl"></i>';
		}
		renderTabs();
		renderDetailsTab();
	}

	function closeDetails(fromHistory) {
		if (!fromHistory && DarUI.shell.isCompact() && history.state?.darDetail === "files") {
			history.back();
			return;
		}
		state.detail = null;
		DarUI.shell.closeDetail(el.details);
	}

	function renderTabs() {
		for (const btn of app.querySelectorAll("[data-tab]")) {
			const active = btn.getAttribute("data-tab") === state.tab;
			btn.classList.toggle("bg-surface", active);
			btn.classList.toggle("text-primary", active);
			btn.classList.toggle("bg-transparent", !active);
			btn.classList.toggle("text-on-surface-variant", !active);
		}
	}

	async function renderDetailsTab() {
		if (!state.detail) return;
		if (state.tab === "sharing") {
			renderSharingTab();
		} else if (state.tab === "activity") {
			const params = new URLSearchParams({source:"files",object_type:"file",object_id:state.detail.path,limit:"50"});
			const response = await fetch("/api/activity?" + params);
			if (!response.ok) throw new Error("Could not load activity");
			const d = await response.json();
			const rows = d.events || [];
			el.detailsBody.innerHTML = rows.length ? rows.map(function (a) {
				return '<div class="py-2 border-b border-outline-variant"><div class="font-medium">' + esc(a.summary || "updated") + '</div><div class="text-xs text-on-surface-variant">' + esc(a.object_name || baseName(a.object_id)) + ' · ' + formatDate(a.happened_at) + '</div></div>';
			}).join("") : '<div class="text-on-surface-variant">No activity yet.</div>';
		} else {
			const d = await apiGet("/versions", {path: state.detail.path});
			const versions = d.versions || [];
			el.detailsBody.innerHTML = versions.length ? versions.map(function (v) {
				return '<div class="py-3 border-b border-outline-variant flex items-center gap-3"><div class="flex-1"><div class="font-medium">' + formatDate(Math.floor((v.time || 0) / 1000000000)) + '</div><div class="text-xs text-on-surface-variant">' + formatSize(v.size || 0) + '</div></div><button type="button" data-restore-version="' + escAttr(v.id) + '" class="h-8 px-3 rounded-full border-0 bg-surface-container text-primary text-xs cursor-pointer">Restore</button></div>';
			}).join("") : '<div class="text-on-surface-variant">No versions yet.</div>';
			for (const btn of el.detailsBody.querySelectorAll("[data-restore-version]")) {
				btn.addEventListener("click", function () {
					apiPost("/version", {path: state.detail.path, id: btn.getAttribute("data-restore-version")}).then(load);
				});
			}
		}
	}

	async function renderSharingTab() {
		const shared = await apiGet("/shared", {});
		const shares = Object.values(shared.shares || {}).filter(function (s) { return s.path === state.detail.path; });
		el.detailsBody.innerHTML =
			'<div class="space-y-4">' +
			'<div><label class="block text-xs font-medium text-on-surface-variant mb-1">Internal recipient</label><div class="flex gap-2"><input id="files-share-user" class="min-w-0 flex-1 h-10 px-3 rounded-full bg-surface-container border-0 outline-none text-sm" placeholder="User ID or email"><button type="button" data-share-user-submit class="h-10 px-4 rounded-full bg-primary text-on-primary border-0 cursor-pointer text-sm font-medium">Share</button></div></div>' +
			'<div><label class="block text-xs font-medium text-on-surface-variant mb-1">Internal link</label><div class="flex gap-2"><input id="files-internal-link" readonly value="' + escAttr(location.origin + "/files" + state.detail.path) + '" class="min-w-0 flex-1 h-10 px-3 rounded-full bg-surface-container border-0 outline-none text-xs"><button type="button" data-copy-internal class="h-10 px-4 rounded-full bg-surface-container-high border-0 cursor-pointer text-sm font-medium">Copy</button></div></div>' +
			'<div><label class="block text-xs font-medium text-on-surface-variant mb-1">External recipient</label><input id="files-external-recipient" class="w-full h-10 px-3 rounded-full bg-surface-container border-0 outline-none text-sm" placeholder="Email address"></div>' +
			'<div class="flex gap-2"><button type="button" data-create-public-link class="h-10 px-4 rounded-full bg-primary text-on-primary border-0 cursor-pointer text-sm font-medium">Create public link</button><button type="button" data-create-file-drop class="h-10 px-4 rounded-full bg-surface-container-high border-0 cursor-pointer text-sm font-medium">File drop</button></div>' +
			'<div><div class="text-xs font-medium text-on-surface-variant mb-2">Others with access</div><div id="files-share-list">' + renderShareList(shares) + '</div></div>' +
			'</div>';
		const copy = el.detailsBody.querySelector("[data-copy-internal]");
		copy.addEventListener("click", function () {
			const input = document.getElementById("files-internal-link");
			input.select();
			navigator.clipboard.writeText(input.value).catch(function () {});
		});
		el.detailsBody.querySelector("[data-create-public-link]").addEventListener("click", function () { createPublicShare(false); });
		el.detailsBody.querySelector("[data-create-file-drop]").addEventListener("click", function () { createPublicShare(true); });
		el.detailsBody.querySelector("[data-share-user-submit]").addEventListener("click", shareWithUser);
	}

	function renderShareList(shares) {
		if (!shares.length) return '<div class="text-sm text-on-surface-variant">No additional shares.</div>';
		return shares.map(function (s) {
			return '<div class="py-2 border-b border-outline-variant"><div class="text-sm font-medium">' + (s.drop ? "File drop" : "Public link") + '</div><div class="text-xs text-on-surface-variant font-mono">' + esc(location.origin + "/s/" + s.token) + '</div></div>';
		}).join("");
	}

	async function createPublicShare(drop) {
		await fetch(apiBase + "/share", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({path: state.detail.path, drop: drop}),
		});
		renderSharingTab();
	}

	async function shareWithUser() {
		const input = document.getElementById("files-share-user");
		const toUser = input.value.trim();
		if (!toUser) return;
		await fetch(apiBase + "/share-user", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({path: state.detail.path, to_user: toUser, permission: "read"}),
		});
		input.value = "";
		renderSharingTab();
	}

	async function newFolder() {
		const name = await DarUI.inputDialog({title: "New folder", label: "Folder name", confirmLabel: "Create"});
		if (name) createFolder(name);
	}

	async function newSymlink() {
		el.newMenu.classList.add("hidden");
		const values = await DarUI.formDialog({
			title: "New symbolic link",
			message: "Targets stay inside your Files root. Use /Documents/file.txt or a path relative to this folder.",
			confirmLabel: "Create",
			fields: [
				{name: "name", label: "Link name", placeholder: "Shortcut"},
				{name: "target", label: "Target", placeholder: "/Documents/file.txt"},
			],
		});
		if (!values) return;
		const linkPath = cleanPath(state.path + "/" + values.name);
		try {
			const response = await fetch(apiBase + "/symlink", {
				method: "POST",
				headers: {"Content-Type": "application/json"},
				body: JSON.stringify({path: linkPath, target: values.target}),
			});
			if (!response.ok) throw new Error((await response.text()) || "Could not create symbolic link");
			await load();
			DarUI.toast("Symbolic link created");
		} catch (error) {
			DarUI.toast(error.message, {tone: "error"});
		}
	}

	function createFolder(name) {
		el.newMenu.classList.add("hidden");
		apiPost("/mkdir", {path: state.path, name: name}).then(load);
	}

	function newTextFile() {
		el.newMenu.classList.add("hidden");
		el.createTextName.value = "Text document.md";
		el.createTextError.classList.add("hidden");
		el.createTextError.textContent = "";
		el.createTextDialog.classList.remove("hidden");
		el.createTextDialog.classList.add("flex");
		requestAnimationFrame(function () {
			el.createTextName.focus();
			el.createTextName.select();
		});
	}

	function closeCreateTextDialog() {
		el.createTextDialog.classList.add("hidden");
		el.createTextDialog.classList.remove("flex");
	}

	async function createTextDocument(ev) {
		ev.preventDefault();
		const name = el.createTextName.value.trim();
		if (!name || name === "." || name === ".." || /[\/\\\0]/.test(name)) {
			el.createTextError.textContent = "Enter a valid filename.";
			el.createTextError.classList.remove("hidden");
			return;
		}
		try {
			const response = await fetch(apiBase + "/touch?" + new URLSearchParams({path: state.path, name: name}), {method: "POST"});
			if (!response.ok) {
				const message = await response.text();
				throw Object.assign(new Error(message || "Could not create file"), {status: response.status});
			}
			const created = await response.json();
			closeCreateTextDialog();
			await load();
			const file = state.files.find(function (item) { return item.path === cleanPath(created.path); }) ||
				{name: name, path: cleanPath(created.path), size: 0, is_dir: false, mime_type: "text/markdown"};
			await openEditor(file);
		} catch (error) {
			el.createTextError.textContent = error.status === 409 ? "A file with that name already exists." : "Could not create the file.";
			el.createTextError.classList.remove("hidden");
		}
	}

	function asEditorDocument(data) {
		return {
			key: data.path,
			path: data.path,
			name: data.name,
			content: data.content || "",
			etag: data.etag,
			markdown: !!data.markdown,
			mime_type: data.mime_type || "",
			size: data.size || 0,
			metadata: {name: data.name},
		};
	}

	async function editorRequest(path, options) {
		const response = await fetch(apiBase + "/editor?path=" + encodeURIComponent(path), options || {});
		let data = null;
		try {
			data = await response.json();
		} catch (_) {}
		if (!response.ok) {
			const error = new Error(data && data.error ? data.error : response.statusText || "Editor request failed");
			error.status = response.status;
			error.data = data && data.path ? asEditorDocument(data) : data;
			throw error;
		}
		return asEditorDocument(data);
	}

	async function saveEditorDraft(draft) {
		return editorRequest(draft.key, {
			method: "PUT",
			headers: {"Content-Type": "application/json", "If-Match": draft.baseETag},
			body: JSON.stringify({content: draft.content, name: draft.metadata.name}),
		});
	}

	async function openEditor(file) {
		if (!file || file.is_dir) return;
		try {
			const editorDocument = await editorRequest(file.path);
			closeDetails();
			el.editorOverlay.classList.remove("hidden");
			el.editorOverlay.classList.add("flex");
			document.body.classList.add("overflow-hidden");
			await textEditor.open(editorDocument);
			if (editorDocument.markdown) el.editorRich.focus(); else el.editorSource.focus();
		} catch (error) {
			openDetails(file);
			if (error.status === 413) editorToast("This file is larger than the 1 MiB editor limit.");
			else if (error.status === 415) editorToast("This file is not editable UTF-8 text.");
			else editorToast("Could not open the editor.");
		}
	}

	function closeEditor() {
		textEditor.close();
		el.editorOverlay.classList.add("hidden");
		el.editorOverlay.classList.remove("flex");
		document.body.classList.remove("overflow-hidden");
	}

	async function commitEditorName() {
		const current = textEditor.current();
		if (!current) return;
		const next = el.editorName.value.trim();
		const committedName = current.metadata && current.metadata.name ?
			current.metadata.name : state.editorCommittedName;
		if (!next || next === "." || next === ".." || /[\/\\\0]/.test(next)) {
			el.editorName.value = committedName;
			editorToast("Enter a valid filename.");
			return;
		}
		if (next === committedName) {
			el.editorName.value = next;
			return;
		}
		if (state.settings.warn_before_extension_change &&
			extensionOf(next) !== extensionOf(committedName) &&
			!await DarUI.confirmDialog({title: "Change file extension?", message: "Changing the extension can make the file open in a different app.", confirmLabel: "Change"})) {
			el.editorName.value = committedName;
			return;
		}
		el.editorName.value = next;
		textEditor.markDirty();
		textEditor.saveNow();
	}

	function editorSaved(saved, sent) {
		const previousName = state.editorCommittedName;
		state.editorCommittedName = saved.metadata.name;
		const index = state.files.findIndex(function (file) {
			return file.path === sent.key || file.path === saved.key ||
				(file.name === previousName && parentPath(file.path) === parentPath(saved.key));
		});
		if (index >= 0) {
			state.files[index] = Object.assign({}, state.files[index], {
				path: saved.key,
				name: saved.metadata.name,
				size: saved.size,
				mime_type: saved.mime_type,
				updated_at: Math.floor(Date.now() / 1000),
			});
			renderFiles();
		}
		if (state.detail && state.detail.path === sent.key) {
			state.detail.path = saved.key;
			state.detail.name = saved.metadata.name;
		}
	}

	function showEditorConflict(local, server, useLocal, useServer) {
		document.getElementById("files-conflict-local").textContent = local.content || "";
		document.getElementById("files-conflict-server").textContent = server.content || "";
		el.editorConflict.classList.remove("hidden");
		el.editorConflict.classList.add("flex");
		document.getElementById("files-conflict-use-local").onclick = function () {
			el.editorConflict.classList.add("hidden");
			el.editorConflict.classList.remove("flex");
			useLocal();
		};
		document.getElementById("files-conflict-use-server").onclick = function () {
			el.editorConflict.classList.add("hidden");
			el.editorConflict.classList.remove("flex");
			useServer();
		};
	}

	function parentPath(filePath) {
		const cleaned = cleanPath(filePath);
		const index = cleaned.lastIndexOf("/");
		return index <= 0 ? "/" : cleaned.slice(0, index);
	}

	function normalizeVirtualPath(value) {
		const out = [];
		for (const part of String(value || "").split("/")) {
			if (!part || part === ".") continue;
			if (part === "..") out.pop();
			else out.push(part);
		}
		return "/" + out.join("/");
	}

	function resolveEditorImage(rawURL, current) {
		if (/^[a-z][a-z0-9+.-]*:/i.test(rawURL)) return rawURL;
		let target;
		try {
			const decoded = decodeURI(rawURL);
			target = decoded.startsWith("/") ? normalizeVirtualPath(decoded) :
				normalizeVirtualPath(parentPath(current.key) + "/" + decoded);
		} catch (_) {
			return "";
		}
		return apiBase + "/download?path=" + encodeURIComponent(target);
	}

	function relativeVirtualPath(fromDir, toPath) {
		const from = normalizeVirtualPath(fromDir).split("/").filter(Boolean);
		const to = normalizeVirtualPath(toPath).split("/").filter(Boolean);
		let common = 0;
		while (common < from.length && common < to.length && from[common] === to[common]) common++;
		const parts = from.slice(common).map(function () { return ".."; }).concat(to.slice(common));
		return parts.join("/") || baseName(toPath);
	}

	async function insertEditorImage() {
		const current = textEditor.current();
		if (!current || !current.markdown) return;
		try {
			const selected = await window.DarFilePicker.open({
				selection: "files",
				multiple: false,
				path: parentPath(current.key),
				title: "Insert image",
				confirmLabel: "Insert",
				mimePrefixes: ["image/"],
			});
			if (!selected.length) return;
			const image = selected[0];
			const relative = relativeVirtualPath(parentPath(current.key), image.path)
				.split("/").map(encodeURIComponent).join("/");
			const alt = String(image.name || baseName(image.path)).replace(/[\[\]]/g, "");
			textEditor.insertText("![" + alt + "](" + relative + ")");
		} catch (error) {
			editorToast(error.message || "Could not select image.");
		}
	}

	function editorToast(message) {
		el.editorToast.textContent = message;
		el.editorToast.classList.remove("hidden");
		clearTimeout(el.editorToast._timer);
		el.editorToast._timer = setTimeout(function () { el.editorToast.classList.add("hidden"); }, 3500);
	}

	function addFolderDescription() {
		apiPost("/touch", {path: state.path, name: "README.md"}).then(load);
	}

	function createFileRequest() {
		fetch(apiBase + "/share", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({path: state.path, drop: true}),
		}).then(function () {
			DarUI.toast("File request link created in Sharing.");
		});
	}

	function uploadInput(input) {
		if (!input.files || input.files.length === 0) return;
		const form = new FormData();
		for (const file of input.files) {
			const name = file.webkitRelativePath || file.name;
			form.append("file", file, name);
		}
		fetch(apiBase + "/upload?path=" + encodeURIComponent(state.path), {method: "POST", body: form}).then(function () {
			input.value = "";
			load();
		});
	}

	function onDragOver(ev) {
		if (!ev.dataTransfer || !ev.dataTransfer.types || !Array.from(ev.dataTransfer.types).includes("Files")) return;
		ev.preventDefault();
		el.drop.classList.remove("hidden");
		el.drop.classList.add("flex");
	}

	function onDragLeave(ev) {
		if (ev.target === el.drop) {
			el.drop.classList.add("hidden");
			el.drop.classList.remove("flex");
		}
	}

	function onDrop(ev) {
		if (!ev.dataTransfer || !ev.dataTransfer.files.length) return;
		ev.preventDefault();
		el.drop.classList.add("hidden");
		el.drop.classList.remove("flex");
		const form = new FormData();
		for (const file of ev.dataTransfer.files) form.append("file", file);
		fetch(apiBase + "/upload?path=" + encodeURIComponent(state.path), {method: "POST", body: form}).then(load);
	}

	async function renameFile(file) {
		const name = await DarUI.inputDialog({title: "Rename item", label: "Name", value: file.name, confirmLabel: "Rename"});
		if (!name || name === file.name) return;
		if (state.settings.warn_before_extension_change && extensionOf(name) !== extensionOf(file.name) &&
			!await DarUI.confirmDialog({title: "Change file extension?", message: "Changing the extension can make the file open in a different app.", confirmLabel: "Change"})) return;
		const dir = file.path.slice(0, Math.max(0, file.path.lastIndexOf("/"))) || "/";
		const dst = cleanPath(dir + "/" + name);
		apiPost("/rename", {src: file.path, dst: dst}).then(load).catch(error => DarUI.toast(error.message, {tone: "error"}));
	}

	async function chooseTransfer(title, excludePaths) {
		const choice = await DarUI.formDialog({
			title,
			confirmLabel: "Choose destination",
			fields: [{name: "operation", label: "Operation", type: "select", value: "move", options: [
				{value: "move", label: "Move"},
				{value: "copy", label: "Copy"},
			]}],
		});
		if (!choice) return null;
		const folders = await DarFilePicker.open({
			selection: "folders",
			path: state.path || state.settings.personal_folder || "/",
			title: choice.operation === "move" ? "Move to folder" : "Copy to folder",
			confirmLabel: "Use folder",
			allowCurrent: true,
			excludePaths: excludePaths || [],
		});
		if (!folders.length) return null;
		return {operation: choice.operation, folder: folders[0].path};
	}

	async function moveOrCopy(file) {
		const transfer = await chooseTransfer("Move or copy " + file.name, file.is_dir ? [file.path] : []);
		if (!transfer) {
			const rowButton = Array.from(el.list.querySelectorAll("[data-menu]")).find(button => button.getAttribute("data-menu") === file.path);
			rowButton?.focus();
			return;
		}
		const destination = cleanPath(transfer.folder + "/" + file.name);
		if (file.is_dir && (destination === file.path || destination.startsWith(file.path + "/"))) {
			DarUI.toast("Choose a destination outside the source folder.", {tone: "error"});
			return;
		}
		apiPost("/" + transfer.operation, {src: file.path, dst: destination}).then(load).catch(error => DarUI.toast(error.message, {tone: "error"}));
	}

	async function deleteFile(file) {
		if (state.settings.warn_before_delete && !await DarUI.confirmDialog({title: "Delete " + file.name + "?", message: "This item will be moved to Deleted files.", confirmLabel: "Delete", tone: "danger"})) return;
		apiPost("/delete", {path: file.path, trash_path: state.settings.trash_folder}).then(function () {
			if (state.detail && state.detail.path === file.path) closeDetails();
			load();
		}).catch(error => DarUI.toast(error.message, {tone: "error"}));
	}

	async function bulkDelete() {
		if (!state.selected.size) return;
		if (state.settings.warn_before_delete && !await DarUI.confirmDialog({title: "Delete selected items?", message: state.selected.size + " items will be moved to Deleted files.", confirmLabel: "Delete", tone: "danger"})) return;
		const results = await Promise.allSettled(Array.from(state.selected).map(path => apiPost("/delete", {path, trash_path: state.settings.trash_folder})));
		const failures = results.filter(result => result.status === "rejected").length;
		await load();
		if (failures) DarUI.toast((results.length - failures) + " deleted; " + failures + " failed.", {tone: "error"});
	}

	async function bulkMove() {
		if (!state.selected.size) return;
		const selectedDirectories = state.files.filter(file => file.is_dir && state.selected.has(file.path)).map(file => file.path);
		const transfer = await chooseTransfer("Move or copy selected items", selectedDirectories);
		if (!transfer) return;
		const sources = Array.from(state.selected);
		const results = await Promise.allSettled(sources.map(src => apiPost("/" + transfer.operation, {src, dst: cleanPath(transfer.folder + "/" + baseName(src))})));
		const failures = results.filter(result => result.status === "rejected").length;
		await load();
		if (failures) DarUI.toast((results.length - failures) + " succeeded; " + failures + " failed.", {tone: "error"});
	}

	async function manageTags(file) {
		const names = Object.values(state.tags).map(function (t) { return t.name; }).join(", ");
		const name = await DarUI.inputDialog({title: "Add tag", label: "Tag name", message: names ? "Existing tags: " + names : "", confirmLabel: "Add"});
		if (!name) return;
		let existing = Object.values(state.tags).find(function (t) { return t.name.toLowerCase() === name.toLowerCase(); });
		const ensureTag = existing ? Promise.resolve(existing) : fetch(apiBase + "/tags", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({name: name, color: "#f5c2e7"}),
		}).then(function (r) { return r.json(); });
		ensureTag.then(function (tag) {
			return fetch(apiBase + "/tag", {
				method: "POST",
				headers: {"Content-Type": "application/json"},
				body: JSON.stringify({path: file.path, tag: tag.id, add: true}),
			});
		}).then(loadTags).then(load);
	}

	function viewFile(file) {
		if (file.is_dir) {
			navigateTo(file.path);
			return;
		}
		openEditor(file);
	}

	function navigateTo(path) {
		state.path = cleanPath(path);
		state.activeView = activeViewForPath(state.path);
		history.pushState({}, "", pathToURL(state.path));
		load();
	}

	function updateSelection() {
		const count = state.selected.size;
		el.selection.classList.toggle("hidden", count === 0);
		el.selection.classList.toggle("flex", count > 0);
		el.selectionCount.textContent = count + " selected";
		if (el.checkAll) el.checkAll.checked = state.files.length > 0 && state.files.every(function (f) { return state.selected.has(f.path); });
	}

	function setSort(sort) {
		if (state.sort === sort) state.sortDir = state.sortDir === "asc" ? "desc" : "asc";
		else {
			state.sort = sort;
			state.sortDir = "asc";
		}
		renderFiles();
	}

	function sortedFiles(files) {
		return files.filter(function (f) { return state.settings.show_hidden || !hasHiddenSegment(f.path); }).slice().sort(function (a, b) {
			if (state.settings.sort_favorites_first && Boolean(a.favorited) !== Boolean(b.favorited)) return a.favorited ? -1 : 1;
			if (state.settings.sort_folders_before_files && a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
			let cmp = 0;
			if (state.sort === "modified") cmp = (a.updated_at || 0) - (b.updated_at || 0);
			else if (state.sort === "type") cmp = String(a.mime_type || "").localeCompare(String(b.mime_type || ""));
			else cmp = String(a.name || "").localeCompare(String(b.name || ""));
			return state.sortDir === "desc" ? -cmp : cmp;
		});
	}

	async function loadTags() {
		const d = await apiGet("/tags", {});
		state.tags = d.tags || {};
	}

	function defaultSettings() {
		return {
			personal_folder: "/docs",
			trash_folder: "/.local/share/Trash",
			sort_favorites_first: true,
			sort_folders_before_files: true,
			folder_tree: false,
			show_hidden: false,
			show_file_type_column: false,
			show_file_extensions: true,
			crop_image_previews: true,
			show_recommendations: true,
			warn_before_extension_change: true,
			warn_before_delete: false,
			default_view: "list",
		};
	}

	async function loadSettings() {
		try {
			const r = await fetch("/api/settings/files");
			if (!r.ok) throw new Error("settings failed");
			state.settings = Object.assign(defaultSettings(), await r.json());
		} catch (err) {
			state.settings = defaultSettings();
		}
		state.settings.personal_folder = cleanPath(state.settings.personal_folder);
		state.settings.trash_folder = cleanPath(state.settings.trash_folder);
		populateSettingsForm();
		syncSidebar();
	}

	function settingsChecked(id, key) {
		const input = document.getElementById(id);
		if (input) input.checked = Boolean(state.settings[key]);
	}

	function populateSettingsForm() {
		const personal = document.getElementById("files-setting-personal-folder");
		const trash = document.getElementById("files-setting-trash-folder");
		const defaultView = document.getElementById("files-setting-default-view");
		const webdav = document.getElementById("files-setting-webdav");
		if (personal) personal.value = state.settings.personal_folder;
		if (trash) trash.value = state.settings.trash_folder;
		if (defaultView) defaultView.value = state.settings.default_view || "list";
		if (webdav) webdav.value = location.origin + "/remote.php/dav/files/me/";
		settingsChecked("files-setting-sort-favorites", "sort_favorites_first");
		settingsChecked("files-setting-sort-folders", "sort_folders_before_files");
		settingsChecked("files-setting-show-hidden", "show_hidden");
		settingsChecked("files-setting-file-type", "show_file_type_column");
		settingsChecked("files-setting-file-extensions", "show_file_extensions");
		settingsChecked("files-setting-crop-previews", "crop_image_previews");
		settingsChecked("files-setting-recommendations", "show_recommendations");
		settingsChecked("files-setting-warn-extension", "warn_before_extension_change");
		settingsChecked("files-setting-warn-delete", "warn_before_delete");
	}

	function readSettingsForm() {
		function checked(id) {
			const input = document.getElementById(id);
			return Boolean(input && input.checked);
		}
		function value(id, fallback) {
			const input = document.getElementById(id);
			return input ? input.value.trim() : fallback;
		}
		return {
			personal_folder: value("files-setting-personal-folder", state.settings.personal_folder),
			trash_folder: value("files-setting-trash-folder", state.settings.trash_folder),
			sort_favorites_first: checked("files-setting-sort-favorites"),
			sort_folders_before_files: checked("files-setting-sort-folders"),
			folder_tree: false,
			show_hidden: checked("files-setting-show-hidden"),
			show_file_type_column: checked("files-setting-file-type"),
			show_file_extensions: checked("files-setting-file-extensions"),
			crop_image_previews: checked("files-setting-crop-previews"),
			show_recommendations: checked("files-setting-recommendations"),
			warn_before_extension_change: checked("files-setting-warn-extension"),
			warn_before_delete: checked("files-setting-warn-delete"),
			default_view: value("files-setting-default-view", "list"),
		};
	}

	function openSettings() {
		populateSettingsForm();
		if (el.settingsMessage) el.settingsMessage.classList.add("hidden");
		if (!el.settingsModal) return;
		el.settingsModal.classList.remove("hidden");
		el.settingsModal.classList.add("flex");
	}

	function closeSettings() {
		if (!el.settingsModal) return;
		el.settingsModal.classList.add("hidden");
		el.settingsModal.classList.remove("flex");
	}

	async function saveSettings() {
		if (el.settingsMessage) el.settingsMessage.classList.add("hidden");
		try {
			const r = await fetch("/api/settings/files", {
				method: "POST",
				headers: {"Content-Type": "application/json"},
				body: JSON.stringify(readSettingsForm()),
			});
			const data = await r.json().catch(function () { return {}; });
			if (!r.ok) throw new Error(data.error || "Could not save settings");
			state.settings = Object.assign(defaultSettings(), data);
			state.settings.personal_folder = cleanPath(state.settings.personal_folder);
			state.settings.trash_folder = cleanPath(state.settings.trash_folder);
			state.activeView = activeViewForPath(state.path);
			populateSettingsForm();
			syncSidebar();
			renderFiles();
			closeSettings();
			if (window.DarUI) window.DarUI.toast("Files settings saved");
		} catch (err) {
			if (el.settingsMessage) {
				el.settingsMessage.textContent = err.message || String(err);
				el.settingsMessage.className = "text-sm text-error";
				el.settingsMessage.classList.remove("hidden");
			}
		}
	}

	function copyWebDAVURL() {
		const input = document.getElementById("files-setting-webdav");
		if (!input) return;
		input.select();
		navigator.clipboard.writeText(input.value).catch(function () {});
	}

	function initializeSettingsFooter() {
		const sidebar = document.getElementById("sidebar");
		if (!sidebar) return;
		const footer = sidebar.querySelector('a[href="#"]');
		if (!footer) return;
		const label = footer.querySelector("span");
		if (label) label.textContent = "Files settings";
		footer.addEventListener("click", function (ev) {
			ev.preventDefault();
			openSettings();
		});
	}

	function syncSidebar() {
		const sidebar = document.getElementById("sidebar");
		if (!sidebar) return;
		const active = activeViewForPath(state.path);
		for (const link of sidebar.querySelectorAll("a")) {
			const label = (link.textContent || "").trim();
			if (label === "Personal files") link.href = pathToURL(state.settings.personal_folder);
			if (label === "Deleted files") link.href = pathToURL(state.settings.trash_folder);
			const isActive = (label === "All files" && state.activeView === "all" && active === "all") ||
				(label === "Personal files" && state.activeView === "personal") ||
				(label === "Deleted files" && state.activeView === "trash") ||
				(label === "Recent" && state.activeView === "recent") ||
				(label === "Favorites" && state.activeView === "favorites") ||
				(label === "Shares" && state.activeView === "shares") ||
				(label === "Tags" && state.activeView === "tags") ||
				(label === "Search" && state.activeView === "search");
			link.classList.toggle("bg-primary-container", isActive);
			link.classList.toggle("text-primary", isActive);
			link.classList.toggle("font-medium", isActive);
			link.classList.toggle("text-on-surface-variant", !isActive);
		}
	}

	function activeViewForPath(path) {
		const p = cleanPath(path);
		if (pathUnder(p, state.settings.trash_folder)) return "trash";
		if (pathUnder(p, state.settings.personal_folder)) return "personal";
		return "all";
	}

	function pathUnder(path, parent) {
		path = cleanPath(path);
		parent = cleanPath(parent);
		return path === parent || path.indexOf(parent + "/") === 0;
	}

	function hasHiddenSegment(path) {
		return cleanPath(path).split("/").filter(Boolean).some(function (part) { return part.charAt(0) === "."; });
	}

	function displayName(file) {
		const name = file.name || baseName(file.path);
		if (file.is_dir || state.settings.show_file_extensions) return name;
		const i = name.lastIndexOf(".");
		return i > 0 ? name.slice(0, i) : name;
	}

	function extensionOf(name) {
		const i = String(name || "").lastIndexOf(".");
		return i > 0 ? String(name).slice(i).toLowerCase() : "";
	}

	function renderTags(file) {
		const tags = file.tags || [];
		if (!tags.length) return "";
		return '<span class="inline-flex gap-1 ml-2 align-middle">' + tags.map(function (id) {
			const tag = state.tags[id];
			if (!tag) return "";
			return '<span class="px-2 py-0.5 rounded-full bg-tertiary-container text-tertiary text-[10px]">' + esc(tag.name) + '</span>';
		}).join("") + '</span>';
	}

	function fileIcon(file) {
		if (file.is_dir) return "fa-solid fa-folder text-primary";
		const mime = file.mime_type || "";
		if (mime.indexOf("image/") === 0) return "fa-solid fa-file-image text-tertiary";
		if (mime.indexOf("video/") === 0) return "fa-solid fa-file-video text-tertiary";
		if (mime.indexOf("audio/") === 0) return "fa-solid fa-file-audio text-tertiary";
		if (file.name && file.name.match(/\.(txt|md|pdf|doc|docx)$/i)) return "fa-solid fa-file-lines text-on-surface-variant";
		return "fa-solid fa-file text-on-surface-variant";
	}

	function activeTitle() {
		const titles = {
			all: "All files",
			personal: "Personal files",
			trash: "Deleted files",
			favorites: "Favorites",
			search: "Search",
			recent: "Recent",
			shares: "Shares",
			tags: "Tags",
		};
		return titles[state.activeView] || "Files";
	}

	function pathFromURL() {
		const path = location.pathname;
		if (path === "/files" || path === "/files/") return "/";
		const rest = path.replace(/^\/files\/?/, "");
		return cleanPath("/" + rest.split("/").filter(Boolean).map(decodeURIComponent).join("/"));
	}

	function viewFromURL() {
		const q = new URLSearchParams(location.search);
		if (q.get("filter") === "favorites") return "favorites";
		if (q.get("view")) return q.get("view");
		const p = pathFromURL();
		return activeViewForPath(p);
	}

	function pathToURL(path) {
		path = cleanPath(path);
		if (path === "/") return "/files";
		return "/files" + path.split("/").map(encodeURIComponent).join("/");
	}

	function cleanPath(path) {
		path = String(path || "/").trim();
		if (!path.startsWith("/")) path = "/" + path;
		path = path.replace(/\/+/g, "/");
		if (path.length > 1 && path.endsWith("/")) path = path.slice(0, -1);
		return path || "/";
	}

	function baseName(path) {
		const parts = cleanPath(path).split("/").filter(Boolean);
		return parts[parts.length - 1] || "Files";
	}

	function formatSize(bytes) {
		if (bytes < 1024) return bytes + " B";
		if (bytes < 1048576) return (bytes / 1024).toFixed(1) + " KB";
		if (bytes < 1073741824) return (bytes / 1048576).toFixed(1) + " MB";
		return (bytes / 1073741824).toFixed(1) + " GB";
	}

	function formatDate(unix) {
		if (!unix) return "-";
		return new Date(unix * 1000).toLocaleString();
	}

	async function apiGet(path, params) {
		const r = await fetch(apiBase + path + "?" + new URLSearchParams(params || {}));
		if (!r.ok) throw new Error("request failed");
		return r.json();
	}

	async function apiPost(path, params) {
		const r = await fetch(apiBase + path + "?" + new URLSearchParams(params || {}), {method: "POST"});
		if (!r.ok) throw new Error((await r.text()).trim() || "Request failed");
		return r.json().catch(function () { return {}; });
	}

	function esc(value) {
		const d = document.createElement("div");
		d.textContent = value == null ? "" : String(value);
		return d.innerHTML;
	}

	function escAttr(value) {
		return esc(value).replace(/"/g, "&quot;");
	}
})();
