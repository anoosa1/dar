import {table,t} from "spacetimedb/server";

// Private tables reached only through servicePhotos/servicePhotosWorker. Assets index
// Files entries (fileId) and never own bytes; timeKey sorts an owner's newest first.
// Phase 2 columns are appended with defaults so existing rows migrate in place:
// indexVersion (rows below photoIndexVersion are re-indexed), phash and dupKey
// (`${ownerId}/${kind}:${phash}:${shape}`, "" when dismissed or unknown) for duplicates,
// playback for videos, liveKey (`${ownerId}/${contentIdentifier}`) for Live Photos and
// stackId for stacks.
export const photosTables={
  photoAssets:table({}, {fileId:t.string().primaryKey(),ownerId:t.string().index("btree"),blobId:t.string(),timeKey:t.string().index("btree"),kind:t.string(),takenAt:t.f64(),
    width:t.u32(),height:t.u32(),duration:t.f64(),exif:t.string(),hasLocation:t.bool(),latitude:t.f64(),longitude:t.f64(),favorite:t.bool(),archived:t.bool(),
    description:t.string(),thumbnail:t.string(),revision:t.u32(),indexedAt:t.f64(),
    indexVersion:t.u32().default(1),phash:t.string().default(""),dupKey:t.string().index("btree").default(""),playback:t.string().default(""),
    liveKey:t.string().index("btree").default(""),stackId:t.string().index("btree").default(""),
    // Phase 4: SHA-1 of the bytes (base64, as Immich clients send it) and the sync sequence.
    // syncKey is `${ownerId}/${16-digit seq}`; "" until the row first changes after the upgrade.
    sha1:t.string().index("btree").default(""),syncSeq:t.f64().default(0),syncKey:t.string().index("btree").default("")}),
  photoAlbums:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),name:t.string(),description:t.string(),coverId:t.string(),createdAt:t.f64(),updatedAt:t.f64(),revision:t.u32(),syncSeq:t.f64().default(0)}),
  // id is `${albumId}/${fileId}`.
  photoAlbumItems:table({}, {id:t.string().primaryKey(),albumId:t.string().index("btree"),fileId:t.string().index("btree"),addedAt:t.f64(),syncSeq:t.f64().default(0)}),
  photoState:table({}, {name:t.string().primaryKey(),value:t.string()}),
  // Stacks group a burst, a RAW+JPEG pair or a manual selection behind one cover.
  // key is `${ownerId}/burst:${uuid}` for bursts and "" otherwise.
  photoStacks:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),key:t.string().index("btree"),primaryId:t.string(),createdAt:t.f64(),updatedAt:t.f64(),syncSeq:t.f64().default(0)}),
  // Albums shared with Dar users; id is `${albumId}/${userId}`, role "viewer" or "editor".
  photoAlbumMembers:table({}, {id:t.string().primaryKey(),albumId:t.string().index("btree"),userId:t.string().index("btree"),role:t.string(),addedAt:t.f64(),syncSeq:t.f64().default(0)}),
  // Shared links to an album (albumId) or to chosen photos (photoLinkItems). digest is the
  // SHA-256 of the URL token; sealed is the token encrypted with the instance keyring so
  // the owner can copy it again. expiresAt 0 never expires; passwordVerifier "" has none.
  photoLinks:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),digest:t.string().unique(),sealed:t.string(),albumId:t.string().index("btree"),
    expiresAt:t.f64(),passwordVerifier:t.string(),allowDownload:t.bool(),allowUpload:t.bool(),showMetadata:t.bool(),createdAt:t.f64(),revision:t.u32()}),
  // id is `${linkId}/${fileId}`.
  photoLinkItems:table({}, {id:t.string().primaryKey(),linkId:t.string().index("btree"),fileId:t.string().index("btree")}),
  // A visitor who entered a link's password; digest of the pass cookie. Not backed up.
  photoLinkPasses:table({}, {digest:t.string().primaryKey(),linkId:t.string().index("btree"),expiresAt:t.f64()}),
};
// Phase 4, Immich compatibility. Deleted sync entities, newest by seq; audience lists the
// user IDs that may hear of it (comma-separated). Pruned after 31 days, as Immich does.
export const immichTables={
  // id is the 16-digit zero-padded seq, so the primary key orders tombstones.
  photoSyncDeletes:table({}, {id:t.string().primaryKey(),key:t.string().index("btree"),kind:t.string(),entityId:t.string(),extraId:t.string(),audience:t.string(),seq:t.f64(),at:t.f64()}),
  // Immich mobile sessions: an access token (digest) bound to an app password's protocol session.
  immichSessions:table({}, {digest:t.string().primaryKey(),id:t.string().unique(),userId:t.string().index("btree"),sessionId:t.string().index("btree"),deviceType:t.string(),deviceOS:t.string(),
    createdAt:t.f64(),usedAt:t.f64(),epochSeen:t.f64()}),
  // id is `${immichSessionId}/${SyncEntityType}`; ack is Immich's "Type|updateId[|extra]".
  immichCheckpoints:table({}, {id:t.string().primaryKey(),immichId:t.string().index("btree"),type:t.string(),ack:t.string()}),
  // Dar's connection to an Immich server, per account. sealedKey is the API key encrypted with the keyring.
  immichConnections:table({}, {userId:t.string().primaryKey(),url:t.string(),sealedKey:t.string(),direction:t.string(),intervalMinutes:t.u32(),enabled:t.bool(),
    lastRunAt:t.f64(),lastError:t.string(),summary:t.string(),revision:t.u32()}),
  // Remote Immich assets already imported or exported; id is `${userId}/${remoteId}`.
  immichRemote:table({}, {id:t.string().primaryKey(),userId:t.string().index("btree"),remoteId:t.string(),fileId:t.string().index("btree"),checksum:t.string()}),
};
