# WebDAV (Files)

Files is served at `/dav/files/<username>/` with the username and an app password with
Files access (read to list and download, write for everything else). Paths are logical
Files paths; they never map to disk.

## Behaviour

- **Writes:** `PUT`, `DELETE`, `MOVE`, `COPY` and `PROPPATCH` check `If-Match` when a client
  sends it (stale → 412) and otherwise replace or delete the current item; a replaced file
  keeps its earlier content as versions. `If-None-Match: *` refuses to replace.
- **Folders:** a missing parent answers 409 (clients create it and retry); `MKCOL` on an
  existing name answers 405. `PROPFIND` takes `Depth: 0` or `1` only.
- **Modified times:** kept from `X-OC-Mtime` on `PUT` (Unix seconds; rclone, ownCloud and
  Nextcloud clients) and from `PROPPATCH` of `DAV:lastmodified`, `DAV:getlastmodified` or
  `Win32LastModifiedTime`. Windows' other `Win32*` properties are accepted and ignored;
  any other property is refused, and the request then changes nothing.
- **Locks** (class 2) are advisory, so Finder and Windows mount the folder writable:
  `LOCK` always grants a token (at most an hour) and writes do not check it. Locking a new
  name creates an empty file.
- **Uploads** need `Content-Length`, or `X-Expected-Entity-Length` with a chunked body
  (Finder). A generic `application/octet-stream` type is replaced by the type of the name.

## Clients

[rclone](https://rclone.org/webdav/) gives the fastest mount and sync. Use the `owncloud`
vendor so modified times are kept, and lower rclone's default 10 ms pause between requests,
which otherwise limits it to about 100 files a second:

```ini
[dar]
type = webdav
url = https://dar.example/dav/files/<username>/
vendor = owncloud
user = <username>
pass = <rclone obscure of the app password>
pacer_min_sleep = 1ms
```

```sh
rclone mount dar: ~/Dar --vfs-cache-mode full --dir-cache-time 1m
rclone sync ~/Documents dar:Documents
```

Without a hash in listings, rclone compares size and modified time. GNOME Files
(`davs://`), KDE (`webdavs://`), davfs2, Finder and Windows use the requests above; rclone
and the requests in `next/test/dav.integration.test.ts` are what is tested.
