// The Notes editor: CodeMirror 6 with an Obsidian-style live preview. The Rust client
// (`crates/client/src/workspace/note_editor.rs`) loads this module with a dynamic `import()`
// the first time a note opens and drives it through `create(parent, options)`. Offsets are
// UTF-16 code units, as CodeMirror counts them. `cargo xtask` bundles it with esbuild
// (`build.mjs`) into the client's dist as `notes-editor.<hash>.js`.

import {EditorState, Compartment, StateEffect, Transaction} from "@codemirror/state";
import {EditorView, Decoration, ViewPlugin, WidgetType, keymap, placeholder} from "@codemirror/view";
import {history, defaultKeymap, historyKeymap, undo, redo, undoDepth, redoDepth} from "@codemirror/commands";
import {Language, defineLanguageFacet, syntaxTree, ensureSyntaxTree} from "@codemirror/language";
import {parser as baseParser, GFM} from "@lezer/markdown";
import {StyleModule} from "style-mod";

// Dar's CSP allows no inline <style> elements, which style-mod writes into the document; a
// constructed stylesheet holds CodeMirror's structural rules instead. Dar's own look for the
// editor is in `crates/client/styles/pim/notes.css`.
const adopted = typeof CSSStyleSheet == "function" && "adoptedStyleSheets" in Document.prototype ? new CSSStyleSheet() : null;
const mounted = [];
if (adopted) {
  StyleModule.mount = (root, modules) => {
    let changed = false;
    for (const module of Array.isArray(modules) ? modules : [modules]) {
      if (!mounted.includes(module)) {
        mounted.push(module);
        changed = true;
      }
    }
    if (changed) adopted.replaceSync(mounted.map((m) => m.getRules()).join("\n"));
    const doc = root.ownerDocument || root;
    if (!doc.adoptedStyleSheets.includes(adopted)) doc.adoptedStyleSheets = [...doc.adoptedStyleSheets, adopted];
  };
}

// --- Markdown with Obsidian's inline syntax -------------------------------------------------

const punctuation = /[!"#$%&'()*+,\-.\/:;<=>?@\[\\\]^_`{|}~\p{P}\p{S}]/u;
const HighlightDelim = {resolve: "Highlight", mark: "HighlightMark"};
const tagChar = /[\p{L}\p{N}_\-\/]/u;
const followHint = /Mac|iPhone|iPad/.test(navigator.platform) ? "\u2318+click to open" : "Ctrl+click to open";

const Obsidian = {
  defineNodes: ["WikiLink", "WikiMark", "Highlight", "HighlightMark", "Tag", "NoteComment"],
  parseInline: [
    {
      // `[[target#fragment|label]]` and `![[embed]]`, on one line.
      name: "WikiLink",
      before: "Link",
      parse(cx, next, pos) {
        const embed = next == 33;
        if (embed ? cx.char(pos + 1) != 91 || cx.char(pos + 2) != 91 : next != 91 || cx.char(pos + 1) != 91) return -1;
        const open = pos + (embed ? 3 : 2);
        const rest = cx.slice(open, cx.end);
        const close = rest.indexOf("]]");
        if (close < 0 || /[\n\[]/.test(rest.slice(0, close))) return -1;
        const end = open + close + 2;
        return cx.addElement(cx.elt("WikiLink", pos, end, [cx.elt("WikiMark", pos, open), cx.elt("WikiMark", end - 2, end)]));
      },
    },
    {
      // `==highlight==`, delimited like GFM's strikethrough.
      name: "Highlight",
      after: "Emphasis",
      parse(cx, next, pos) {
        if (next != 61 || cx.char(pos + 1) != 61 || cx.char(pos + 2) == 61) return -1;
        const before = cx.slice(pos - 1, pos), after = cx.slice(pos + 2, pos + 3);
        const sBefore = /\s|^$/.test(before), sAfter = /\s|^$/.test(after);
        const pBefore = punctuation.test(before), pAfter = punctuation.test(after);
        return cx.addDelimiter(HighlightDelim, pos, pos + 2, !sAfter && (!pAfter || sBefore || pBefore), !sBefore && (!pBefore || sAfter || pAfter));
      },
    },
    {
      // `#tag` after a space or at a line start, as `dar_notes::tags_in` reads them.
      name: "Tag",
      before: "Emphasis",
      parse(cx, next, pos) {
        if (next != 35) return -1;
        if (pos > cx.offset) {
          const before = cx.slice(pos - 1, pos);
          if (!/\s/.test(before) && !"([{,;\"'".includes(before)) return -1;
        }
        let end = pos + 1;
        while (end < cx.end && tagChar.test(cx.slice(end, end + 1))) end++;
        while (end > pos + 1 && cx.slice(end - 1, end) == "/") end--;
        const name = cx.slice(pos + 1, end);
        if (!name || /^[\d\/\-_]+$/.test(name)) return -1;
        return cx.addElement(cx.elt("Tag", pos, end));
      },
    },
    {
      // `%%comment%%` (hidden in the preview).
      name: "NoteComment",
      before: "Emphasis",
      parse(cx, next, pos) {
        if (next != 37 || cx.char(pos + 1) != 37) return -1;
        const close = cx.slice(pos + 2, cx.end).indexOf("%%");
        return close < 0 ? -1 : cx.addElement(cx.elt("NoteComment", pos, pos + 2 + close + 2));
      },
    },
  ],
};

const markdownLanguage = new Language(defineLanguageFacet({}), baseParser.configure([GFM, Obsidian]), [], "markdown");

// --- Live preview ---------------------------------------------------------------------------

class Bullet extends WidgetType {
  eq() {
    return true;
  }
  toDOM() {
    const span = document.createElement("span");
    span.className = "cm-dar-bullet";
    span.setAttribute("aria-hidden", "true");
    return span;
  }
}

class Checkbox extends WidgetType {
  constructor(checked, at) {
    super();
    this.checked = checked;
    this.at = at;
  }
  eq(other) {
    return other.checked == this.checked && other.at == this.at;
  }
  toDOM() {
    const box = document.createElement("span");
    box.className = this.checked ? "cm-dar-task is-done" : "cm-dar-task";
    box.setAttribute("role", "checkbox");
    box.setAttribute("aria-checked", String(this.checked));
    box.setAttribute("aria-label", this.checked ? "Completed task" : "Open task");
    box.dataset.at = String(this.at);
    return box;
  }
  ignoreEvent() {
    return false;
  }
}

const hide = Decoration.replace({});
const bullet = Decoration.replace({widget: new Bullet()});
const refresh = StateEffect.define();
const lineClass = (cls, attributes) => Decoration.line(attributes ? {class: cls, attributes} : {class: cls});
const mark = (cls, attributes) => Decoration.mark(attributes ? {class: cls, attributes} : {class: cls});

/// The front matter (`---` … `---` at the very start) as a range, or null.
function frontMatter(doc) {
  if (doc.lines < 2 || !/^---\s*$/.test(doc.line(1).text)) return null;
  for (let n = 2; n <= Math.min(doc.lines, 400); n++) {
    if (/^(---|\.\.\.)\s*$/.test(doc.line(n).text)) return {from: 0, to: doc.line(n).to};
  }
  return null;
}

function livePreview(options) {
  const build = (view) => {
    const {state} = view;
    const doc = state.doc;
    const decorations = [];
    // Lines the selection touches show their Markdown as written while the editor has focus.
    const active = new Set();
    if (view.hasFocus) {
      for (const range of state.selection.ranges) {
        for (let n = doc.lineAt(range.from).number; n <= doc.lineAt(range.to).number; n++) active.add(n);
      }
    }
    const shown = (from, to) => {
      for (let n = doc.lineAt(from).number; n <= doc.lineAt(to).number; n++) if (active.has(n)) return true;
      return false;
    };
    const lines = new Map();
    const addLine = (pos, cls, attributes) => {
      const line = doc.lineAt(pos);
      if (!lines.has(line.from)) lines.set(line.from, []);
      lines.get(line.from).push([cls, attributes]);
    };
    const front = frontMatter(doc);
    if (front) {
      for (let pos = front.from; pos <= front.to; ) {
        const line = doc.lineAt(pos);
        addLine(line.from, "cm-dar-frontmatter");
        pos = line.to + 1;
      }
    }
    const tree = ensureSyntaxTree(state, view.viewport.to, 50) || syntaxTree(state);
    for (const {from, to} of view.visibleRanges) {
      tree.iterate({
        from,
        to,
        enter: (node) => {
          if (front && node.from <= front.to) return node.name == "Document";
          const name = node.name;
          const raw = !shown(node.from, node.to);
          const heading = /^(?:ATX|Setext)Heading(\d)$/.exec(name);
          if (heading) {
            addLine(node.from, `cm-dar-heading cm-dar-h${heading[1]}`);
            return;
          }
          switch (name) {
            case "HeaderMark": {
              if (!raw) return;
              const parent = node.node.parent?.name ?? "";
              const line = doc.lineAt(node.from);
              if (parent.startsWith("Setext")) {
                addLine(node.from, "cm-dar-setext-mark");
              } else if (/^\s*$/.test(doc.sliceString(line.from, node.from))) {
                // The opening `#` run hides with the space after it.
                decorations.push(hide.range(node.from, Math.min(line.to, doc.sliceString(node.to, node.to + 1) == " " ? node.to + 1 : node.to)));
              } else {
                // A closing `#` run hides with the space before it.
                decorations.push(hide.range(doc.sliceString(node.from - 1, node.from) == " " ? node.from - 1 : node.from, node.to));
              }
              return;
            }
            case "Blockquote": {
              const first = doc.lineAt(node.from);
              const callout = /^\s*>\s*\[!([\w-]+)\]/.exec(first.text);
              for (let pos = node.from; pos <= node.to; ) {
                const line = doc.lineAt(pos);
                if (callout) addLine(line.from, line.from == first.from ? "cm-dar-callout cm-dar-callout-title" : "cm-dar-callout", {"data-callout": callout[1].toLowerCase()});
                else addLine(line.from, "cm-dar-quote");
                pos = line.to + 1;
              }
              return;
            }
            case "QuoteMark": {
              if (raw) decorations.push(hide.range(node.from, doc.sliceString(node.to, node.to + 1) == " " ? node.to + 1 : node.to));
              return;
            }
            case "FencedCode":
            case "CodeBlock": {
              const first = doc.lineAt(node.from).number, last = doc.lineAt(node.to).number;
              for (let n = first; n <= last; n++) {
                const edge = (n == first ? " is-first" : "") + (n == last ? " is-last" : "");
                addLine(doc.line(n).from, `cm-dar-codeblock${edge}`);
              }
              return name == "FencedCode";
            }
            case "CodeMark": {
              if (node.node.parent?.name == "FencedCode") {
                decorations.push(mark("cm-dar-fence").range(node.from, node.to));
              } else if (raw) {
                decorations.push(hide.range(node.from, node.to));
              }
              return;
            }
            case "CodeInfo":
              decorations.push(mark("cm-dar-fence").range(node.from, node.to));
              return;
            case "CodeText":
              return false;
            case "InlineCode":
              decorations.push(mark("cm-dar-code").range(node.from, node.to));
              return;
            case "HorizontalRule":
              addLine(node.from, raw ? "cm-dar-hr" : "cm-dar-hr is-editing");
              return;
            case "Table":
              for (let pos = node.from; pos <= node.to; ) {
                const line = doc.lineAt(pos);
                addLine(line.from, "cm-dar-table");
                pos = line.to + 1;
              }
              return;
            case "Emphasis":
              decorations.push(mark("cm-dar-em").range(node.from, node.to));
              return;
            case "StrongEmphasis":
              decorations.push(mark("cm-dar-strong").range(node.from, node.to));
              return;
            case "Strikethrough":
              decorations.push(mark("cm-dar-strike").range(node.from, node.to));
              return;
            case "Highlight":
              decorations.push(mark("cm-dar-mark").range(node.from, node.to));
              return;
            case "EmphasisMark":
            case "StrikethroughMark":
            case "HighlightMark":
              if (raw) decorations.push(hide.range(node.from, node.to));
              return;
            case "NoteComment":
              decorations.push(mark("cm-dar-comment").range(node.from, node.to));
              return false;
            case "Tag": {
              const tag = doc.sliceString(node.from + 1, node.to);
              decorations.push(mark("cm-dar-tag", {"data-tag": tag}).range(node.from, node.to));
              return false;
            }
            case "WikiLink":
              wikiLink(node, raw);
              return false;
            case "Link":
            case "Image":
              markdownLink(node, raw, name == "Image");
              return false;
            case "URL": {
              const parent = node.node.parent?.name;
              if (parent == "Autolink" || parent == "Paragraph" || !parent || parent.startsWith("ATX") || parent == "TableCell") {
                const href = doc.sliceString(node.from, node.to);
                decorations.push(mark(linkClass(href, false), {"data-href": href, "data-wiki": "false", title: followHint}).range(node.from, node.to));
              }
              return;
            }
            case "ListItem": {
              const marker = node.node.getChild("ListMark");
              const task = node.node.getChild("Task");
              if (!marker) return;
              const line = doc.lineAt(marker.from);
              const ordered = node.node.parent?.name == "OrderedList";
              const box = task?.getChild("TaskMarker");
              const checked = !!box && /x/i.test(doc.sliceString(box.from, box.to));
              addLine(line.from, box ? "cm-dar-list cm-dar-task-line" : "cm-dar-list");
              if (checked && box.to < task.to) decorations.push(mark("cm-dar-done").range(box.to, task.to));
              if (!shown(line.from, line.to)) {
                const space = doc.sliceString(marker.to, marker.to + 1) == " " ? marker.to + 1 : marker.to;
                if (box) {
                  decorations.push(hide.range(marker.from, space));
                  decorations.push(Decoration.replace({widget: new Checkbox(checked, box.from)}).range(box.from, box.to));
                } else if (!ordered) {
                  decorations.push(bullet.range(marker.from, marker.to));
                } else {
                  decorations.push(mark("cm-dar-list-number").range(marker.from, marker.to));
                }
              }
              return;
            }
          }
        },
      });
    }
    // Block ids (`^id` closing a line) stay quiet.
    for (const {from, to} of view.visibleRanges) {
      for (let pos = from; pos <= to; ) {
        const line = doc.lineAt(pos);
        const match = /(?:^|\s)(\^[A-Za-z0-9-]+)\s*$/.exec(line.text);
        if (match) {
          const start = line.from + match.index + match[0].indexOf("^");
          decorations.push(mark("cm-dar-block-id").range(start, start + match[1].length));
        }
        pos = line.to + 1;
      }
    }
    for (const [from, list] of lines) {
      const cls = list.map(([c]) => c).join(" ");
      const attributes = Object.assign({}, ...list.map(([, a]) => a || {}));
      decorations.push(lineClass(cls, Object.keys(attributes).length ? attributes : undefined).range(from));
    }
    return Decoration.set(decorations, true);

    function linkClass(dest, wiki, embed) {
      const kind = options.resolve ? options.resolve(dest, wiki) : "";
      return `cm-dar-link${embed ? " cm-dar-embed" : ""}${kind ? " " + kind : ""}`;
    }

    function wikiLink(node, raw) {
      const text = doc.sliceString(node.from, node.to);
      const embed = text.startsWith("!");
      const open = node.from + (embed ? 3 : 2), close = node.to - 2;
      const inner = doc.sliceString(open, close);
      const pipe = inner.indexOf("|");
      const dest = pipe < 0 ? inner : inner.slice(0, pipe);
      const attributes = {"data-href": dest, "data-wiki": "true", title: followHint};
      const cls = linkClass(dest, true, embed);
      if (!raw || close <= open) {
        decorations.push(mark(cls, attributes).range(node.from, node.to));
        return;
      }
      const labelFrom = pipe < 0 ? open : open + pipe + 1;
      if (labelFrom >= close) {
        decorations.push(mark(cls, attributes).range(node.from, node.to));
        return;
      }
      decorations.push(hide.range(node.from, labelFrom));
      decorations.push(mark(cls, attributes).range(labelFrom, close));
      decorations.push(hide.range(close, node.to));
    }

    function markdownLink(node, raw, image) {
      const marks = node.node.getChildren("LinkMark");
      const url = node.node.getChild("URL");
      const dest = url ? doc.sliceString(url.from, url.to) : "";
      const attributes = dest ? {"data-href": dest, "data-wiki": "false", title: followHint} : undefined;
      const cls = dest ? linkClass(dest, false, image) : "cm-dar-link is-reference";
      // `[label](dest)`: the label shows as the link; `![alt](dest)` keeps its alt text.
      const labelFrom = marks[0]?.to, labelTo = marks[1]?.from;
      const oneLine = doc.lineAt(node.from).number == doc.lineAt(node.to).number;
      if (!raw || !oneLine || labelFrom == null || labelTo == null || labelTo <= labelFrom || marks.length < 2) {
        decorations.push(mark(cls, attributes).range(node.from, node.to));
        return;
      }
      decorations.push(hide.range(node.from, labelFrom));
      decorations.push(mark(cls, attributes).range(labelFrom, labelTo));
      decorations.push(hide.range(labelTo, node.to));
    }
  };

  return ViewPlugin.fromClass(
    class {
      constructor(view) {
        this.decorations = build(view);
      }
      update(update) {
        if (update.docChanged || update.viewportChanged || update.selectionSet || update.focusChanged
          || syntaxTree(update.startState) != syntaxTree(update.state)
          || update.transactions.some((tr) => tr.effects.some((e) => e.is(refresh)))) {
          this.decorations = build(update.view);
        }
      }
    },
    {decorations: (plugin) => plugin.decorations},
  );
}

// --- Links and tasks ------------------------------------------------------------------------

/// The link or tag at a document position, from the syntax tree.
function linkAt(state, pos) {
  let node = syntaxTree(state).resolveInner(pos, 1);
  for (let n = node; n; n = n.parent) {
    if (n.name == "WikiLink") {
      const text = state.doc.sliceString(n.from, n.to);
      const inner = text.replace(/^!?\[\[/, "").replace(/\]\]$/, "");
      const pipe = inner.indexOf("|");
      return {kind: "link", dest: pipe < 0 ? inner : inner.slice(0, pipe), wiki: true};
    }
    if (n.name == "Link" || n.name == "Image") {
      const url = n.getChild("URL");
      return url ? {kind: "link", dest: state.doc.sliceString(url.from, url.to), wiki: false} : null;
    }
    if (n.name == "URL") return {kind: "link", dest: state.doc.sliceString(n.from, n.to), wiki: false};
    if (n.name == "Tag") return {kind: "tag", dest: state.doc.sliceString(n.from + 1, n.to)};
  }
  return null;
}

function toggleTask(view, at) {
  if (view.state.readOnly) return false;
  const current = view.state.doc.sliceString(at, at + 3);
  const next = current == "[ ]" ? "[x]" : /^\[[xX]\]$/.test(current) ? "[ ]" : null;
  if (!next) return false;
  view.dispatch({changes: {from: at, to: at + 3, insert: next}, userEvent: "input.toggle"});
  return true;
}

/// The task checkbox on the main selection's line.
function taskOnLine(state) {
  const line = state.doc.lineAt(state.selection.main.head);
  const match = /^\s*(?:[-*+]|\d{1,9}[.)])\s+\[[ xX]\]/.exec(line.text);
  return match ? line.from + match[0].length - 3 : -1;
}

// --- The editor -----------------------------------------------------------------------------

/// Creates the editor in `parent`. `options`: text, markdown, readOnly, spellcheck,
/// placeholder, attributes (on the content element), and the callbacks onUpdate(text | null,
/// canUndo, canRedo, focused), onKeydown(event) -> handled, onFiles(files) -> handled,
/// onLink(dest, wiki), onTag(name), onBlur(), resolve(dest, wiki) -> link class.
export function create(parent, options) {
  const readOnly = new Compartment();
  let locked = !!options.readOnly;
  const readOnlyState = (value) => [
    EditorState.readOnly.of(value),
    value || !options.placeholder ? [] : placeholder(options.placeholder),
  ];
  const report = (update) => {
    const text = update.docChanged ? update.state.doc.toString() : null;
    options.onUpdate?.(text, undoDepth(update.state) > 0, redoDepth(update.state) > 0, update.view.hasFocus);
  };
  const follow = (view, pos) => {
    const found = linkAt(view.state, pos);
    if (!found) return false;
    if (found.kind == "tag") options.onTag?.(found.dest);
    else options.onLink?.(found.dest, found.wiki);
    return true;
  };
  const files = (list) => (list && list.length ? Array.from(list) : []);
  const extensions = [
    EditorState.lineSeparator.of("\n"),
    history(),
    EditorView.lineWrapping,
    EditorView.contentAttributes.of({spellcheck: options.spellcheck ? "true" : "false", autocorrect: "on", autocapitalize: "sentences", ...(options.attributes || {})}),
    EditorView.editorAttributes.of({class: options.markdown ? "is-markdown" : "is-plain"}),
    EditorView.updateListener.of((update) => {
      if (update.docChanged || update.selectionSet) report(update);
      if (update.focusChanged && !update.view.hasFocus) options.onBlur?.();
    }),
    EditorView.domEventHandlers({
      // Text replacing a selection, or spanning lines, goes in as a transaction: Chrome's own
      // editing would keep the replaced text's look with inline `style` spans, which Dar's
      // CSP blocks. Plain typing stays with the browser (spellcheck, IME).
      beforeinput(event, view) {
        const type = event.inputType;
        if ((type != "insertText" && type != "insertReplacementText") || typeof event.data != "string" || view.composing) return false;
        // Target ranges are empty for some input (DevTools' insertText); the selection is the same.
        let [range] = event.getTargetRanges();
        if (!range) {
          const selection = view.root.getSelection ? view.root.getSelection() : document.getSelection();
          if (selection?.rangeCount && view.contentDOM.contains(selection.anchorNode)) range = selection.getRangeAt(0);
        }
        if (!range || (range.collapsed && !event.data.includes("\n"))) return false;
        event.preventDefault();
        if (view.state.readOnly) return true;
        const from = view.posAtDOM(range.startContainer, range.startOffset);
        const to = range.collapsed ? from : view.posAtDOM(range.endContainer, range.endOffset);
        view.dispatch({
          changes: {from: Math.min(from, to), to: Math.max(from, to), insert: event.data},
          selection: {anchor: Math.min(from, to) + event.data.length},
          scrollIntoView: true,
          userEvent: "input.type",
        });
        return true;
      },
      keydown(event, view) {
        if (options.onKeydown?.(event)) {
          event.preventDefault();
          return true;
        }
        return false;
      },
      paste(event) {
        const list = files(event.clipboardData?.files);
        return list.length > 0 && !!options.onFiles?.(list) && (event.preventDefault(), true);
      },
      dragover(event) {
        if (options.markdown && event.dataTransfer?.types.includes("Files")) {
          event.preventDefault();
          return true;
        }
        return false;
      },
      drop(event) {
        const list = files(event.dataTransfer?.files);
        return list.length > 0 && !!options.onFiles?.(list) && (event.preventDefault(), true);
      },
      mousedown(event, view) {
        const box = event.target instanceof Element ? event.target.closest(".cm-dar-task") : null;
        if (box && event.button == 0) {
          event.preventDefault();
          toggleTask(view, Number(box.dataset.at));
          return true;
        }
        if (event.button == 0 && (event.ctrlKey || event.metaKey) && event.target instanceof Element && event.target.closest(".cm-dar-link, .cm-dar-tag")) {
          const pos = view.posAtDOM(event.target.closest(".cm-dar-link, .cm-dar-tag"), 0);
          event.preventDefault();
          return follow(view, pos + 1) || follow(view, pos);
        }
        return false;
      },
    }),
    keymap.of([
      // Obsidian: Mod-Enter toggles the line's checkbox; Alt-Enter (or Mod-Enter on a link) follows it.
      {key: "Mod-Enter", run: (view) => follow(view, view.state.selection.main.head) || (() => {
        const at = taskOnLine(view.state);
        return at >= 0 && toggleTask(view, at);
      })()},
      {key: "Alt-Enter", run: (view) => follow(view, view.state.selection.main.head)},
      // The outliner moves lines (Rust's keydown hook); never copy them.
      {key: "Shift-Alt-ArrowUp", run: () => true},
      {key: "Shift-Alt-ArrowDown", run: () => true},
      ...historyKeymap,
      ...defaultKeymap,
    ]),
  ];
  if (options.markdown) extensions.push(markdownLanguage, livePreview(options));
  // A state with the current read-only setting (a reset keeps it).
  const stateFor = (doc) => EditorState.create({doc, extensions: [extensions, readOnly.of(readOnlyState(locked))]});

  const view = new EditorView({
    parent,
    state: stateFor(options.text || ""),
  });

  /// Replaces the document with `text` by its changed middle, so the selection, scroll
  /// position and undo history stay meaningful.
  const diff = (text) => {
    const current = view.state.doc.toString();
    if (current == text) return null;
    let start = 0;
    const limit = Math.min(current.length, text.length);
    while (start < limit && current.charCodeAt(start) == text.charCodeAt(start)) start++;
    let end = 0;
    while (end < limit - start && current.charCodeAt(current.length - 1 - end) == text.charCodeAt(text.length - 1 - end)) end++;
    return {from: start, to: current.length - end, insert: text.slice(start, text.length - end)};
  };
  const clamp = (pos) => Math.max(0, Math.min(view.state.doc.length, pos));

  const api = {
    view,
    /// The content element (role textbox, the accessible name from `attributes`).
    content: () => view.contentDOM,
    text: () => view.state.doc.toString(),
    /// The main selection as [from, to].
    selection: () => [view.state.selection.main.from, view.state.selection.main.to],
    hasFocus: () => view.hasFocus,
    /// A new document with a fresh history (a note was loaded).
    reset(text) {
      view.setState(stateFor(text));
    },
    /// Takes text changed elsewhere (another mode, a reload) into the document.
    setText(text, remember) {
      const changes = diff(text);
      if (changes) view.dispatch({changes, annotations: remember ? [] : [Transaction.addToHistory.of(false)], userEvent: "set"});
    },
    /// Replaces the document as an edit (formatting, outliner keys, suggestions) and selects.
    replace(text, anchor, head) {
      const changes = diff(text);
      const selection = {anchor: Math.max(0, Math.min(text.length, anchor)), head: Math.max(0, Math.min(text.length, head ?? anchor))};
      view.dispatch({changes: changes || [], selection, scrollIntoView: true, userEvent: "input"});
    },
    /// Inserts text over the selection, leaving the caret after it.
    insert(text) {
      view.dispatch(view.state.replaceSelection(text), {scrollIntoView: true, userEvent: "input"});
    },
    select(anchor, head, scroll) {
      view.dispatch({selection: {anchor: clamp(anchor), head: clamp(head ?? anchor)}, effects: scroll ? EditorView.scrollIntoView(clamp(head ?? anchor), {y: "start", yMargin: 48}) : []});
    },
    focus() {
      view.contentDOM.focus({preventScroll: true});
    },
    /// The viewport rectangle of a position ({left, top, bottom}), or null off screen.
    coords(pos) {
      const rect = view.coordsAtPos(clamp(pos));
      return rect ? {left: rect.left, top: rect.top, bottom: rect.bottom} : null;
    },
    undo: () => undo(view),
    redo: () => redo(view),
    setReadOnly(value) {
      locked = !!value;
      view.dispatch({effects: readOnly.reconfigure(readOnlyState(locked))});
    },
    /// Re-reads link resolution (the note index changed).
    refresh() {
      view.dispatch({effects: refresh.of(null)});
    },
    /// Measures again after the editor was hidden.
    measure() {
      view.requestMeasure();
    },
    destroy() {
      if (view.contentDOM.darEditor === api) delete view.contentDOM.darEditor;
      view.destroy();
    },
  };
  // A deliberate hook for the acceptance suite (`tests/src/ui.rs`): text and selection.
  view.contentDOM.darEditor = api;
  return api;
}
