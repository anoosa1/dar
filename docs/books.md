# Books

Optional built-in app at `/books` (administrators can turn it off in Administration →
Apps; data is kept). Layout follows [mock A](spacetimedb/books-mocks.html) (November 16,
2026): a Kavita-style library with books read inside the shared frame.

## Maintainer decisions (November 16 & November 26, 2026)

- **Layout:** [mock A](spacetimedb/books-mocks.html) refreshed in the
  [Books redesign](spacetimedb/books-redesign-mock.html) (November 26, 2026): Now Reading
  spotlight + horizontal scrolling shelves on Home, `Covers / List / Series` catalog switcher with
  format/status filters and bulk actions, `Highlights` browser and Details bridge into Notes,
  focus reading mode (`Z`), EPUB 1-page/2-page column toggle, PDF zoom `%` / `Fit page` / paper
  tint (`Paper`, `Sepia`, `Night`), and Comic `Single / Spread / Strip` + inline `RTL / LTR`
  controls.
- **PDF:** pure-Rust `hayro` (`crates/pdf`), compiled to WebAssembly in a dedicated Web
  Worker (`crates/pdf-worker`, replacing `pdf.js`). Files' PDF preview uses the same
  viewer, since browsers' own viewers do not run in Dar's sandboxed previews.
- **EPUB:** Dar's own reader, no EPUB library.
- **Comics:** CBZ is read directly; CBR, CB7 and CBT use `bsdtar` (libarchive) from the
  Nix package.
- **Formats:** MOBI/AZW and DRM-protected books are not converted; they show a clear
  "cannot open" state. An OPDS catalog for e-reader apps is planned for later.

## How it works

- **Source:** EPUB, PDF, CBZ, CBR, CB7 and CBT files under a Files folder per account
  (`docs/books` by default, set in Books settings). Its top folders are libraries; hidden
  and deleted files are skipped. Books stores no bytes: Files keeps versions, quotas,
  sharing, deleted files, DAV and backups.
- **Metadata** (title, authors, series, language, publisher, date, description, subjects,
  pages, cover, reading direction) comes from the EPUB package document, `ComicInfo.xml`
  or the PDF info (via `dar-pdf` in an isolated child process, which also renders page 1 for
  the cover before `libvips` encodes the WebP), else the file name. Results and WebP covers
  are cached in `book-cache/` under the storage root, keyed by immutable blob ID; the cache
  is not backed up and can be deleted. A new book shows its file name until it has been read
  (a few seconds); the library asks again while any are pending.
- **EPUB reader:** each chapter loads from the archive into a same-origin iframe whose
  document policy is `script-src 'none'` (no forms, popups or other origins). The reader
  styles it (theme, font, size, line spacing) and in Pages layout splits it into CSS columns
  of at most about 36 em; Scroll layout scrolls each chapter. Malformed XHTML falls back to
  the HTML parser. Links inside the book stay in the reader; others open in a new tab.
- **PDF reader:** one scrolling column, pages drawn only near the viewport, zoom and fit
  width, the outline as contents. Each drawn page has an invisible, selectable text layer
  (`dar-pdf` reads each glyph's Unicode from the font's `ToUnicode` map or glyph names and
  groups glyphs into runs on one baseline; one stretched span per run, a line break after each
  line), so selecting and copying gives the text with its lines. **Find** (the search button or
  Ctrl/Cmd+F in the reader) reads page text as needed (cached per page), counts matches over the
  whole book, starts at the current page, and moves with Enter, Shift+Enter or the arrows.
  Pages that are only images (scans without OCR text) have no text. Files' PDF preview has the
  text layer too, but no Find or highlights. **Comics:** one page at a time, fitted, with tap zones and
  keys in the reading direction (right to left from `ComicInfo.xml` Manga or settings).
- **Reading state** (want to read, reading, finished; place; progress; device) and
  bookmarks are per account and per Files entry, so renames and moves keep them. A place
  saves a moment after each page turn with the state's revision; if another device saved a
  different place first, the reader asks "Go there" or "Stay here" instead of overwriting.
  Reaching the end marks a book finished. Reader settings are account preferences.
- **Access** follows Files: every request resolves the file through Files' download check
  (owner or grant, not deleted, Locked folder rules). App passwords cannot reach Books.
- **Highlights** (PDF): selecting text offers Highlight, which appends the quote to
  `Highlights/<book title>.md` in the Notes folder (folder and note made on first use, written
  through Files with the note's revision so edits elsewhere are kept): `> quoted lines`, then
  `> — [Book title, p. N](/books?book=<id>&page=N) ^hl-…`, then a Markdown link reference
  definition `[hl-…]: /books?… "dar-highlight N:x,y,w,h …"` holding the boxes as fractions of the
  page. Markdown shows no unused definition (an HTML comment would show as text in Notes), so
  the note reads as quotes. Opening the book reads the note and draws its highlights again;
  editing or deleting a block there changes them. `/books?book=<id>&page=N` opens the reader at
  page N (PDF and comics) instead of the saved place.
- Backups (format v10) include reading state and bookmarks.

## Later

OPDS catalog, EPUB highlights and search, collections, series stacks in the grid, and Books
results in the cross-app search.
