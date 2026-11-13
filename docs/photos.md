# Photos

Optional built-in app at `/photos` (administrators can disable it in Administration →
Apps; data is kept). Layout follows [mock B](spacetimedb/photos-mock-b.html)
(November 13, 2026). The goal is Immich feature parity inside Dar's frame, delivered
in phases.

## Maintainer decisions (November 13, 2026)

- **Layout:** mock B (albums in the sidebar, square month-grouped grid, viewer in the
  middle sheet, details in the right sheet).
- **Immich:** both directions. Dar connects to an Immich server with an API key
  (import or two-way), and serves the Immich mobile-app API so the official app
  backs up to Dar with an app password (Photos read/write scope).
- **Authorized dependencies:** Nix-provided media tools (ffmpeg, libvips,
  exiftool) run as subprocesses; bundled ML in the Nix closure (onnxruntime with
  face detection/recognition, CLIP and OCR models); an operator-configured map tile
  URL (no tiles are fetched unless set). No new npm packages.

## How it works (phase 1)

- **Source:** every image and video the account owns in Files (JPEG, PNG, WebP,
  GIF, AVIF, HEIC/HEIF, TIFF, common RAW formats; MP4, MOV, WebM, MKV and others).
  Shared files from other accounts are not included. Photos stores metadata only;
  bytes, quotas, versions, trash, DAV and backups stay with Files.
- **Uploads** go to Files › `Photos/YYYY/MM` (by the file's date); a same-named file
  is kept with a number. Uploads are indexed at once; files added in Files are found
  by a bounded background scan (every 30 seconds, and soon after Photos opens).
- **Facts:** exiftool reads capture time (zone from `OffsetTimeOriginal`, otherwise
  server-local; QuickTime dates are UTC), dimensions after orientation, camera,
  lens, exposure and GPS. Without a date the file's modified time is used.
- **Thumbnails:** libvips makes auto-rotated WebP thumbnails (420 px) and previews
  (1,800 px), ffmpeg a video poster, and RAW files use their embedded preview. They
  are generated on demand into `photo-cache/` under the storage root, keyed by
  immutable blob ID, at most two tool runs at a time with time limits. The cache is
  not backed up and can be deleted. If a tool is missing, browser-decodable images
  fall back to the original and others show a placeholder.
- **Library:** Library (not archived), Favorites, Videos and Archive views, newest
  first, with a zoom slider (3–12 columns). Select with the tile checkbox or Shift
  click; the tonal bar offers favorite, add to album, remove from album and
  archive/restore with Undo. Archived photos stay in albums and Files.
- **Viewer and details:** preview or video player with a filmstrip, ←/→, `f` and
  Escape; details show date, file, camera, location, albums, an editable description
  (revision-checked, merged with favorite/archive changes) and Show in Files. Phones
  show the photo full screen with an Info button for details.
- **Albums:** create (optionally from a selection), rename, delete (photos stay),
  add and remove; the first photo is the cover. Limits: 500 albums, 50,000 items.
- **Search:** file names, descriptions, camera models and album names appear in the
  spotlight search with thumbnails. Trashing a file in Files hides it in Photos; its
  Photos metadata returns if it is restored.
- **Backups** (format v8) include Photos metadata; deleting an account deletes it.

Verify with `nix develop -c node next/scripts/verify.mjs --photos`.

## Next phases

2. Video transcodes for HEVC/MKV playback, Live Photos, stacks, duplicates, a
   Photos trash view and the locked folder.
3. Sharing: albums with Dar users, shared links (expiry, password, download/upload).
4. Immich mobile API (login, upload, sync, albums) with a Photos app-password scope,
   and the Immich server connector.
5. ML: faces (people, merge, hide, link to Contacts), smart (CLIP) search, OCR and
   memories; places with an offline geocoder and the map; People/Places/Explore tabs.
