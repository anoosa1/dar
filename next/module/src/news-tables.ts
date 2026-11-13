import {table,t} from "spacetimedb/server";

// Private tables: browser sessions and Nextcloud News app passwords enter only through serviceNews.
// Integer IDs come from newsSequences so Nextcloud News clients can sync them.
export const newsTables={
  newsFolders:table({}, {id:t.u32().primaryKey(),ownerId:t.string().index("btree"),name:t.string(),opened:t.bool(),createdAt:t.f64()}),
  newsFeeds:table({}, {id:t.u32().primaryKey(),ownerId:t.string().index("btree"),folderId:t.u32(),url:t.string(),urlKey:t.string().unique(),title:t.string(),link:t.string(),added:t.f64(),
    ordering:t.u32(),pinned:t.bool(),fullText:t.bool(),paused:t.bool(),etag:t.string(),lastModifiedHeader:t.string(),lastUpdated:t.f64(),nextUpdate:t.f64(),dueKey:t.string().index("btree"),errorCount:t.u32(),lastError:t.string()}),
  // ownerKey/feedKey sort newest first; unreadKey/starredKey hold the owner only while set.
  newsItems:table({}, {id:t.u32().primaryKey(),ownerId:t.string(),feedId:t.u32().index("btree"),dedupeKey:t.string().unique(),ownerKey:t.string().index("btree"),feedKey:t.string().index("btree"),
    unreadKey:t.string().index("btree"),starredKey:t.string().index("btree"),modifiedKey:t.string().index("btree"),guid:t.string(),guidHash:t.string(),url:t.string(),title:t.string(),author:t.string(),
    pubDate:t.f64(),updatedDate:t.f64(),excerpt:t.string(),enclosureMime:t.string(),enclosureLink:t.string(),mediaThumbnail:t.string(),mediaDescription:t.string(),rtl:t.bool(),
    fingerprint:t.string(),contentHash:t.string(),unread:t.bool(),starred:t.bool(),lastModified:t.f64(),position:t.f64(),fetchedAt:t.f64()}),
  newsBodies:table({}, {itemId:t.u32().primaryKey(),body:t.string()}),
  newsCounts:table({}, {ownerId:t.string().primaryKey(),items:t.u32()}),
  newsSequences:table({}, {name:t.string().primaryKey(),value:t.u32()}),
};
