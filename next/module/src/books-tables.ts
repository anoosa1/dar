import {table,t} from "spacetimedb/server";

// Private tables reached only through serviceBooks. Books never own bytes: rows point at
// Files entries (fileId), which keep their ID across renames and moves.
export const booksTables={
  // id is `${ownerId}/${fileId}`. status: "", "want", "reading" or "finished"; position is a
  // BookPosition as JSON ("" when never opened); progress runs from 0 to 1.
  bookStates:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),fileId:t.string().index("btree"),status:t.string(),position:t.string(),
    progress:t.f64(),device:t.string(),updatedAt:t.f64(),revision:t.u32()}),
  // Bookmarks; key is `${ownerId}/${fileId}`.
  bookMarks:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),key:t.string().index("btree"),fileId:t.string(),position:t.string(),label:t.string(),createdAt:t.f64()}),
};
