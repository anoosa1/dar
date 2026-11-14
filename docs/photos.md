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
- **Upload folder (phase 2):** Photos stores uploads in a Files folder chosen per
  account, default `pics` (as `pics/YYYY/MM`).
- **Locked folder (phase 2):** a real Locked folder in Files, not only a Photos
  view. Locked photos are stored in it, and the account password unlocks it.

## How it works

- **Source:** every image and video the account owns in Files (JPEG, PNG, WebP,
  GIF, AVIF, HEIC/HEIF, TIFF, common RAW formats; MP4, MOV, WebM, MKV and others).
  Shared files from other accounts are not included. Photos stores metadata only;
  bytes, quotas, versions, trash, DAV and backups stay with Files.
- **Uploads** go to Files › `pics/YYYY/MM` (by the file's date; Photos settings
  changes the folder, for example to `Media/Camera`). A same-named file is kept with
  a number. Uploads are indexed at once; files added in Files are found by a bounded
  background scan (every 30 seconds, and soon after Photos opens).
- **Facts:** exiftool reads capture time (zone from `OffsetTimeOriginal`, otherwise
  server-local; QuickTime dates are UTC), dimensions after orientation, camera,
  lens, exposure and GPS; ffprobe reads video codecs. Without a date the file's
  modified time is used.
- **Thumbnails:** libvips makes auto-rotated WebP thumbnails (420 px) and previews
  (1,800 px), ffmpeg a video poster, and RAW files use their embedded preview. They
  are generated on demand into `photo-cache/` under the storage root, keyed by
  immutable blob ID, at most two tool runs at a time with time limits. The cache is
  not backed up and can be deleted. If a tool is missing, browser-decodable images
  fall back to the original and others show a placeholder.
- **Library:** Library (not archived), Favorites, Shared with me and Videos, then albums, then
  Duplicates, Archive, Locked folder and Trash; newest first, with a zoom slider
  (3–12 columns). Select with the tile checkbox or Shift click; the tonal bar offers
  favorite, add to album, remove from album, stack, archive, Move to Locked folder
  and Move to trash, all with Undo. Archived photos stay in albums and Files.
- **Viewer and details:** preview or video player with a filmstrip, ←/→, `f`,
  Delete (to the trash) and Escape; details show date, file, codecs, camera,
  location, albums, an editable description (revision-checked, merged with
  favorite/archive changes) and Show in Files. Phones show the photo full screen
  with an Info button for details.
- **Videos:** MP4 with H.264/VP9/AV1 and WebM play as is; other H.264 files (MOV,
  MKV, AVI) are remuxed into MP4; HEVC, MPEG-4 Part 2 and others are transcoded to
  H.264/AAC (at most 1920 px) the first time someone opens them, one at a time, with
  progress in the viewer. Browsers that decode HEVC play the original. Converted
  copies are kept in `photo-cache/`.
- **Live Photos:** a still (HEIC/JPEG) and its motion video (MOV/MP4 up to 4 s) pair
  by Apple's ContentIdentifier, or by the same name in the same folder. They show as
  one tile with a Live badge and the viewer plays the motion on demand. Trash,
  restore, lock and delete move both files.
- **Stacks:** RAW+JPEG pairs with the same name in a folder stack under the JPEG;
  burst frames stack by Apple's BurstUUID; any selection can be stacked. Timelines
  show the cover with a count; the viewer lists the stack, changes the cover or
  unstacks. Albums show their members individually.
- **Duplicates:** a 64-bit difference hash of each thumbnail groups copies of the
  same picture (same kind and shape; videos also by length), identical or re-saved.
  Stacked pairs are not duplicates. Keep chooses the copies to keep; they gain the
  others' albums, favorite and description, and the rest go to the trash with Undo.
  Not duplicates dismisses a group until the picture changes. Similar shots that are
  not copies come with ML in phase 5.
- **Trash:** Photos › Trash lists the media in Files › Deleted files, newest
  deletion first, with Restore, Delete permanently and Empty trash. Nothing expires
  by itself. Photos inside a trashed folder are hidden until the folder is restored.
- **Locked folder:** each account has one, in Files outside All files. The account
  password opens it for 10 minutes on that browser session (Lock closes it sooner;
  10 tries a minute). Its contents never appear in All files, search, sharing, public
  links, WebDAV, the file picker, other apps or Photos' library, albums and
  duplicates; sharing set up earlier pauses while an item is inside. Deleting inside
  it is permanent, because the trash is not locked. Undo of Move to Locked folder
  returns photos to their folder; Move out sends them to the upload folder.
  Administrator account exports include locked files.
- **Sharing with people:** the album owner shares from the Share pane (right editing
  pane, after mock A's Share album) with Dar usernames, as Can view or Can add. Shared
  albums appear under Shared with me with the owner's name; people who can add put
  their own photos in, and remove only those. Others' photos open read-only (no
  favorite, archive, description or Files location) with Download original; they
  disappear when their owner trashes, locks or removes them. The owner removes
  anyone; members can leave. Up to 100 people per album. Partner sharing (a whole
  library) is not included.
- **Shared links:** to an album (from the Share pane) or to selected photos (selection
  bar or the viewer's menu), at `/s/<token>`. The 256-bit token is stored only as a
  digest plus a copy sealed with the instance keyring, so owners can copy links again
  (a link restored on another installation keeps working but cannot be shown). Each
  link has an expiry (never, 1 day to 1 year), an optional password (4–200
  characters, scrypt, 10 tries a minute per link and address; changing it signs
  visitors out) and switches for download, upload (album links) and metadata.
  Visitors see a minimal page with the Photos grid and viewer; uploads go to the
  owner's Photos folder (numbered if the name is taken) and the album, and count
  toward the owner's storage. Photos settings › Privacy lists every link. Up to 200
  links per account; deleting an album ends its links.
- **Immich mobile app** (protocol of Immich server 3.4.1): enter the Dar address as the
  server URL; `/.well-known/immich` points the app to `/dav/immich/api`. Sign in with
  your username followed by `@` and anything (the app requires an email shape, for
  example `anas@dar`) and an app password with Photos access (read to browse,
  write to back up and change). Backups go to the Photos folder by the file's
  creation month; files Dar already has (same SHA-1, also in the trash) are reported
  as duplicates. The app's sync stream sends your library (favorites, archive, trash
  with its date, Live Photo motion parts hidden), EXIF, albums you own or share with
  their people and photos, and stacks; partners, memories, people, OCR and edits are
  empty. Changes travel incrementally by Dar's change sequence; a trashed, restored
  or moved folder of photos, joining or leaving a shared album, or 30 days without a
  sync make the device start over. Locked photos never sync. Videos play converted
  copies once ready, otherwise the original. Photos settings › Immich lists
  signed-in devices and signs them out. Not verified on a physical device yet; the
  protocol is covered by tests built from the 3.4.1 API and app sources.
- **Immich server connector:** Photos settings › Immich connects to an Immich server
  with an API key (checked first, then sealed with the instance keyring). Import
  copies originals Dar lacks (by SHA-1) into the Photos folder with favorite,
  archive and description, and mirrors album membership; Two-way also uploads Dar
  photos Immich lacks. Runs every 15 minutes to daily or on Sync now, at most 300
  files or 4 GiB per run, resuming next time. Private network addresses are allowed;
  loopback, link-local, multicast and unspecified addresses are not (development
  installations allow loopback). Disconnecting keeps imported photos.
- **Albums:** create (optionally from a selection), rename, delete (photos stay),
  add and remove; the first photo is the cover. Limits: 500 albums, 50,000 items.
- **Search:** file names, descriptions, camera models and album names appear in the
  spotlight search with thumbnails (never locked or trashed photos).
- **Backups** (format v8) include Photos metadata, stacks, album members and links
  (not visitor passes); older v8 backups
  restore without them. Deleting an account deletes its Photos metadata, links and
  memberships.
- **Upgrades:** raising the index version re-indexes existing media once in the
  background (phase 2 added hashes, playback and pairing; phase 4 the SHA-1).
- **Backups** also keep Immich server connections (with their sealed keys) and
  imported-asset mappings; mobile sessions and sync checkpoints are not kept.

Verify with `nix develop -c node next/scripts/verify.mjs --photos`.

## Next phases

5. ML: faces (people, merge, hide, link to Contacts), smart (CLIP) search, OCR and
   memories; places with an offline geocoder and the map; People/Places/Explore tabs.
