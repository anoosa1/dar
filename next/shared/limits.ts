/** Dar's per-file size limit for Files, WebDAV and Immich uploads and imports. Uploads stream to disk. */
export const maxFileBytes=16*1024**3;
export const maxFileLabel="16 GiB";
/**
 * Stored content linked or used again within this long is never reclaimed: an upload stores its
 * bytes before the database records them, and transfers expire an hour after they begin.
 */
export const blobGraceMs=2*3600_000;
