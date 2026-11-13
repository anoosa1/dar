import {table,t} from "spacetimedb/server";

// Private tables reached only through servicePhotos/servicePhotosWorker. Assets index
// Files entries (fileId) and never own bytes; timeKey sorts an owner's newest first.
export const photosTables={
  photoAssets:table({}, {fileId:t.string().primaryKey(),ownerId:t.string().index("btree"),blobId:t.string(),timeKey:t.string().index("btree"),kind:t.string(),takenAt:t.f64(),
    width:t.u32(),height:t.u32(),duration:t.f64(),exif:t.string(),hasLocation:t.bool(),latitude:t.f64(),longitude:t.f64(),favorite:t.bool(),archived:t.bool(),
    description:t.string(),thumbnail:t.string(),revision:t.u32(),indexedAt:t.f64()}),
  photoAlbums:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),name:t.string(),description:t.string(),coverId:t.string(),createdAt:t.f64(),updatedAt:t.f64(),revision:t.u32()}),
  // id is `${albumId}/${fileId}`.
  photoAlbumItems:table({}, {id:t.string().primaryKey(),albumId:t.string().index("btree"),fileId:t.string().index("btree"),addedAt:t.f64()}),
  photoState:table({}, {name:t.string().primaryKey(),value:t.string()}),
};
