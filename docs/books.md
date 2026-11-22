# Books

Optional built-in app at `/books` (administrators can turn it off in Administration →
Apps; data is kept). Layout follows [mock A](spacetimedb/books-mocks.html) (November 16,
2026): a Kavita-style library with books read inside the shared frame.

## Maintainer decisions (November 16, 2026)

- **Layout:** mock A (home shelves, details in the right sheet, the reader in the middle
  sheet with contents and bookmarks on the right).
- **PDF:** pdf.js (`pdfjs-dist`, bundled into the client; its optional Node canvas is
  replaced by an empty placeholder in `vendor/no-native-canvas`). Files' PDF preview
  uses the same viewer, since browsers' own viewers do not run in Dar's sandboxed previews.
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
  or the PDF info (via libvips), else the file name. Results and WebP covers are cached in
  `book-cache/` under the storage root, keyed by immutable blob ID; the cache is not backed
  up and can be deleted. A new book shows its file name until it has been read (a few
  seconds); the library asks again while any are pending.
- **EPUB reader:** each chapter loads from the archive into a same-origin iframe whose
  document policy is `script-src 'none'` (no forms, popups or other origins). The reader
  styles it (theme, font, size, line spacing) and in Pages layout splits it into CSS columns
  of at most about 36 em; Scroll layout scrolls each chapter. Malformed XHTML falls back to
  the HTML parser. Links inside the book stay in the reader; others open in a new tab.
- **PDF reader:** one scrolling column, pages drawn only near the viewport, zoom and fit
  width, the outline as contents. **Comics:** one page at a time, fitted, with tap zones and
  keys in the reading direction (right to left from `ComicInfo.xml` Manga or settings).
- **Reading state** (want to read, reading, finished; place; progress; device) and
  bookmarks are per account and per Files entry, so renames and moves keep them. A place
  saves a moment after each page turn with the state's revision; if another device saved a
  different place first, the reader asks "Go there" or "Stay here" instead of overwriting.
  Reaching the end marks a book finished. Reader settings are account preferences.
- **Access** follows Files: every request resolves the file through Files' download check
  (owner or grant, not deleted, Locked folder rules). App passwords cannot reach Books.
- Backups (format v10) include reading state and bookmarks.

## Later

OPDS catalog, highlights and notes, collections, series stacks in the grid, search in a
book, and Books results in the cross-app search.
