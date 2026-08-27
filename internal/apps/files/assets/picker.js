/* Shared, dependency-free Files picker. Load once from /files/assets/picker.js
 * and call: await DarFilePicker.open({selection:'files', multiple:true}). */
(function () {
	'use strict';

	function node(tag, classes, text) {
		const element = document.createElement(tag);
		if (classes) element.className = classes;
		if (text !== undefined) element.textContent = text;
		return element;
	}
	function api(path, options) {
		return fetch('/api/files/picker' + path, options).then(async response => {
			if (!response.ok) throw new Error((await response.text()) || 'Files picker request failed');
			return response.json();
		});
	}
	function parent(path) {
		return path === '/' ? '/' : path.split('/').slice(0, -1).join('/') || '/';
	}
	function normalized(path) {
		const parts = String(path || '/').split('/').filter(part => part && part !== '.');
		const clean = [];
		for (const part of parts) {
			if (part === '..') clean.pop();
			else clean.push(part);
		}
		return '/' + clean.join('/');
	}

	function open(options) {
		const opts = Object.assign({
			selection: 'files',
			multiple: false,
			path: '/',
			title: 'Select files',
			confirmLabel: 'Select',
			mimePrefixes: [],
			allowCurrent: false,
			excludePaths: [],
		}, options || {});
		if (!['files', 'folders', 'both'].includes(opts.selection)) return Promise.reject(new Error('Invalid picker selection mode'));
		if (!Array.isArray(opts.mimePrefixes) || !Array.isArray(opts.excludePaths)) return Promise.reject(new Error('Invalid picker options'));
		const excluded = opts.excludePaths.map(normalized);
		const blocked = path => excluded.some(exclude => path === exclude || path.startsWith(exclude + '/'));

		return new Promise((resolve, reject) => {
			let current = normalized(opts.path);
			let rows = [];
			let selected = new Map();
			let closed = false;
			let triedRootFallback = current === '/';
			const restoreFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;

			const dialog = node('dialog', 'h-[min(42rem,calc(100dvh-2rem))] w-[min(48rem,calc(100vw-2rem))] rounded-3xl bg-surface p-0 text-on-surface shadow-2xl backdrop:bg-scrim/50');
			dialog.setAttribute('aria-labelledby', 'dar-file-picker-title');
			const panel = node('section', 'flex h-full min-h-0 flex-col');
			const header = node('header', 'flex items-center gap-3 border-b border-outline-variant px-4 py-3');
			const title = node('h2', 'min-w-0 flex-1 truncate text-lg font-semibold', opts.title);
			title.id = 'dar-file-picker-title';
			const closeButton = node('button', 'h-10 w-10 shrink-0 rounded-full hover:bg-surface-container-high');
			closeButton.type = 'button';
			closeButton.setAttribute('aria-label', 'Close file picker');
			closeButton.append(node('i', 'fa-solid fa-xmark'));
			header.append(title, closeButton);

			const tools = node('div', 'space-y-3 border-b border-outline-variant px-4 py-3');
			const breadcrumbs = node('nav', 'flex min-h-8 flex-wrap items-center gap-1 text-sm text-primary');
			breadcrumbs.setAttribute('aria-label', 'Folder breadcrumbs');
			const search = node('input', 'h-11 w-full rounded-full bg-surface-container px-4 outline-none focus:ring-2 focus:ring-primary');
			search.type = 'search';
			search.placeholder = 'Search this folder';
			search.setAttribute('aria-label', 'Search this folder');
			tools.append(breadcrumbs, search);

			const status = node('p', 'hidden px-5 py-3 text-sm text-error');
			status.setAttribute('role', 'alert');
			const list = node('div', 'min-h-0 flex-1 overflow-y-auto p-2');
			list.setAttribute('role', 'listbox');
			list.setAttribute('aria-multiselectable', String(!!opts.multiple));

			const footer = node('footer', 'flex flex-wrap items-center gap-2 border-t border-outline-variant px-4 py-3');
			const count = node('span', 'min-w-0 flex-1 text-sm text-on-surface-variant');
			const cancel = node('button', 'h-10 rounded-full px-4 hover:bg-surface-container-high', 'Cancel');
			cancel.type = 'button';
			const selectCurrent = node('button', 'h-10 rounded-full px-4 text-primary hover:bg-primary-container', 'Choose this folder');
			selectCurrent.type = 'button';
			const confirm = node('button', 'h-10 rounded-full bg-primary px-5 font-medium text-on-primary', opts.confirmLabel);
			confirm.type = 'button';
			footer.append(count, cancel);
			if (opts.allowCurrent && opts.selection !== 'files') footer.append(selectCurrent);
			footer.append(confirm);
			panel.append(header, tools, status, list, footer);
			dialog.append(panel);

			function finish(value, error) {
				if (closed) return;
				closed = true;
				if (dialog.open) dialog.close();
				dialog.remove();
				restoreFocus?.focus();
				error ? reject(error) : resolve(value);
			}
			function permitted(entry) {
				return opts.selection === 'both' || (opts.selection === 'files' && !entry.is_dir) || (opts.selection === 'folders' && entry.is_dir);
			}
			function choose(entry) {
				if (!permitted(entry) || blocked(entry.path)) return;
				if (!opts.multiple) selected.clear();
				if (selected.has(entry.path)) selected.delete(entry.path);
				else selected.set(entry.path, entry);
				render();
			}
			function renderBreadcrumbs() {
				breadcrumbs.replaceChildren();
				const root = node('button', 'rounded-full px-2 py-1 hover:bg-primary-container', 'Files');
				root.type = 'button';
				root.dataset.path = '/';
				breadcrumbs.append(root);
				let path = '';
				for (const part of current.split('/').filter(Boolean)) {
					breadcrumbs.append(node('span', 'text-on-surface-variant', '/'));
					path += '/' + part;
					const button = node('button', 'rounded-full px-2 py-1 hover:bg-primary-container', part);
					button.type = 'button';
					button.dataset.path = path;
					breadcrumbs.append(button);
				}
			}
			function render() {
				renderBreadcrumbs();
				list.replaceChildren();
				if (current !== '/') {
					const up = node('button', 'flex w-full items-center gap-3 rounded-2xl p-3 text-left hover:bg-surface-container-high');
					up.type = 'button';
					up.dataset.up = 'true';
					up.append(node('i', 'fa-solid fa-arrow-turn-up text-on-surface-variant'), node('span', '', 'Parent folder'));
					list.append(up);
				}
				const visible = rows.filter(entry => !blocked(entry.path));
				for (const entry of visible) {
					const row = node('div', 'flex items-center gap-2 rounded-2xl p-1 ' + (selected.has(entry.path) ? 'bg-primary-container text-on-primary-container' : 'hover:bg-surface-container-high'));
					row.setAttribute('role', 'option');
					row.setAttribute('aria-selected', String(selected.has(entry.path)));
					const openEntry = node('button', 'flex min-w-0 flex-1 items-center gap-3 rounded-xl p-2 text-left');
					openEntry.type = 'button';
					openEntry.append(node('i', entry.is_dir ? 'fa-solid fa-folder text-primary' : 'fa-regular fa-file text-on-surface-variant'), node('span', 'truncate', entry.name));
					if (entry.is_dir) {
						openEntry.setAttribute('aria-label', 'Open folder ' + entry.name);
						openEntry.onclick = () => { current = entry.path; search.value = ''; load(); };
					} else {
						openEntry.onclick = () => choose(entry);
					}
					row.append(openEntry);
					if (permitted(entry)) {
						const toggle = node('button', 'h-10 w-10 shrink-0 rounded-full hover:bg-surface-container-highest');
						toggle.type = 'button';
						toggle.setAttribute('aria-label', (selected.has(entry.path) ? 'Deselect ' : 'Select ') + entry.name);
						toggle.append(node('i', selected.has(entry.path) ? 'fa-solid fa-circle-check text-primary' : 'fa-regular fa-circle'));
						toggle.onclick = () => choose(entry);
						row.append(toggle);
					}
					list.append(row);
				}
				if (!visible.length && current === '/') list.append(node('p', 'p-8 text-center text-on-surface-variant', 'No matching files'));
				count.textContent = selected.size ? selected.size + (selected.size === 1 ? ' selected' : ' selected') : current;
				selectCurrent.disabled = blocked(current);
				selectCurrent.classList.toggle('opacity-40', blocked(current));
			}
			async function load() {
				status.classList.add('hidden');
				list.setAttribute('aria-busy', 'true');
				try {
					const params = new URLSearchParams({path: current, selection: opts.selection});
					const query = search.value.trim();
					if (query) params.set('q', query);
					opts.mimePrefixes.forEach(prefix => params.append('mime_prefix', prefix));
					const data = await api('?' + params.toString());
					rows = data.files || [];
					render();
				} catch (error) {
					if (!triedRootFallback && current !== '/') {
						triedRootFallback = true;
						current = '/';
						search.value = '';
						await load();
						return;
					}
					status.textContent = error.message;
					status.classList.remove('hidden');
					list.replaceChildren(node('p', 'p-8 text-center text-on-surface-variant', 'Could not load this folder.'));
				} finally {
					list.setAttribute('aria-busy', 'false');
				}
			}
			async function validate(paths) {
				if (!paths.length) {
					status.textContent = 'Choose at least one item.';
					status.classList.remove('hidden');
					return;
				}
				confirm.disabled = true;
				selectCurrent.disabled = true;
				try {
					const result = await api('/validate', {
						method: 'POST',
						headers: {'Content-Type': 'application/json'},
						body: JSON.stringify({paths, selection: opts.selection, multiple: opts.multiple, mime_prefixes: opts.mimePrefixes}),
					});
					finish(result.files || []);
				} catch (error) {
					status.textContent = error.message;
					status.classList.remove('hidden');
					confirm.disabled = false;
					selectCurrent.disabled = blocked(current);
				}
			}

			closeButton.onclick = cancel.onclick = () => finish([]);
			confirm.onclick = () => validate([...selected.keys()]);
			selectCurrent.onclick = () => validate([current]);
			breadcrumbs.onclick = event => {
				const button = event.target.closest('[data-path]');
				if (!button) return;
				current = button.dataset.path;
				search.value = '';
				load();
			};
			list.onclick = event => {
				const up = event.target.closest('[data-up]');
				if (!up) return;
				current = parent(current);
				search.value = '';
				load();
			};
			let searchTimer;
			search.oninput = () => {
				clearTimeout(searchTimer);
				searchTimer = setTimeout(load, 200);
			};
			dialog.addEventListener('cancel', event => {
				event.preventDefault();
				finish([]);
			});
			dialog.addEventListener('keydown', event => {
				if (event.key === 'Escape') {
					event.preventDefault();
					finish([]);
				}
			});
			dialog.addEventListener('close', () => finish([]));
			document.body.append(dialog);
			dialog.showModal();
			search.focus();
			load();
		});
	}

	window.DarFilePicker = {open};
}());
