import {table,t} from "spacetimedb/server";

/**
 * Private Chat tables; browsers reach them only through serviceChat and are told
 * about changes through myChatChanges. Keys are the bridge's whatsmeow store,
 * encrypted by the runtime: scanKey is "<account>/c/<key>" for keys loaded at
 * start and "<account>/l/<key>" for keys read on demand; expiryKey orders keys
 * that expire (empty when they never do).
 */
export const chatChanges=table({}, {userId:t.string().primaryKey(),revision:t.u32()});
export const chatTables={
  chatAccounts:table({}, {id:t.string().primaryKey(),ownerId:t.string().index("btree"),platform:t.string(),name:t.string(),jid:t.string(),state:t.string(),detail:t.string(),
    createdAt:t.f64(),linkedAt:t.f64(),syncedAt:t.f64(),historyProgress:t.u32(),importing:t.bool(),messages:t.u32()}),
  chatKeys:table({}, {id:t.string().primaryKey(),scanKey:t.string().index("btree"),expiryKey:t.string().index("btree"),value:t.string()}),
  // listKey is "<account>/<newest-first time>/<jid>" for paging a chat list.
  chatThreads:table({}, {id:t.string().primaryKey(),accountId:t.string().index("btree"),listKey:t.string().index("btree"),jid:t.string(),kind:t.string(),name:t.string(),at:t.f64(),
    preview:t.string(),previewKind:t.string(),previewFromMe:t.bool(),previewSender:t.string(),previewStatus:t.string(),unread:t.u32(),archived:t.bool(),pinned:t.bool(),mutedUntil:t.f64(),
    // Who sent the previewed group message, so a Dar contact can name them.
    previewSenderJid:t.string().default(""),
    // Discord: the server a channel belongs to (empty for direct messages), its category, an
    // order key for the channel list, the channel topic, and mentions of the account since read.
    serverId:t.string().default(""),section:t.string().default(""),order:t.string().default(""),topic:t.string().default(""),mentions:t.u32().default(0)}),
  // Discord servers ("guilds"), id "<account>/<server>".
  chatServers:table({}, {id:t.string().primaryKey(),accountId:t.string().index("btree"),serverId:t.string(),name:t.string(),order:t.string(),mutedUntil:t.f64()}),
  // threadKey is "<thread>/<newest-first time>/<message>" for paging a conversation.
  chatMessages:table({}, {id:t.string().primaryKey(),accountId:t.string().index("btree"),threadKey:t.string().index("btree"),messageId:t.string(),sender:t.string(),senderName:t.string(),
    fromMe:t.bool(),at:t.f64(),kind:t.string(),text:t.string(),fileName:t.string(),quotedId:t.string(),quotedSender:t.string(),quotedText:t.string(),status:t.string(),edited:t.bool(),deleted:t.bool()}),
  // One attachment per message (id is the message row id). blobId is set once it is downloaded; it keeps
  // the blob through maintenance and puts it in backups. state: remote, stored, retrying, expired, failed.
  chatMedia:table({}, {id:t.string().primaryKey(),accountId:t.string().index("btree"),blobId:t.string().index("btree"),kind:t.string(),mime:t.string(),size:t.f64(),
    fileName:t.string(),seconds:t.u32(),width:t.u32(),height:t.u32(),state:t.string(),at:t.f64()}),
  // One reaction per person per message (id "<message row>|<sender or me>"); an empty emoji removes it.
  // Discord allows several per person: ids are "<message row>|me|<emoji>" and "<message row>|*|<emoji>".
  chatReactions:table({}, {id:t.string().primaryKey(),messageRowId:t.string().index("btree"),accountId:t.string().index("btree"),sender:t.string(),senderName:t.string(),
    fromMe:t.bool(),emoji:t.string(),at:t.f64(),
    // Discord gives counts without names: one row per emoji for everyone else (sender "*") with its count.
    count:t.u32().default(1)}),
  chatChanges,
};
