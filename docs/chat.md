# Chat

Optional built-in app at `/chat`: a client for other platforms, not a network of its
own. WhatsApp first, Discord later. Layout follows the selected
[mock A with account tabs](spacetimedb/chat-mocks.html) (November 17, 2026); see Chat in
the [design guide](design.md). Administrators can disable it in Administration → Apps;
data is kept and every account disconnects.

## Decisions (maintainer, November 17)

- WhatsApp through [whatsmeow](https://github.com/tulir/whatsmeow) in a Go bridge. Go is
  approved for this bridge; more of Dar may move to Go later.
- No Postgres or SQLite: whatsmeow's device keys and all chat data live in
  SpacetimeDB through a custom whatsmeow store.
- On linking, import as much history as WhatsApp provides.
- Accounts never mix: each linked account is a tab with its own chats, views and counts.

## What works (phase 1)

- **Linking:** Link WhatsApp opens the right editing pane with a QR code (drawn by
  `shared/qr.ts`) or, on phones, a pairing code for the phone number. Dar shows up as
  “Dar” in the phone's Linked devices. Up to 10 accounts; Link again reuses an
  unlinked account's history (a different number starts it afresh).
- **Chats:** account tabs; Chats (pinned first, newest first), Unread, Groups and
  Archived; previews with sender, media label, ticks, pin, mute and unread counts.
  New chat by phone number.
- **Messages:** text with replies, edits and deletions; locations, contacts and polls show
  as labelled placeholders. Sending is stored first as pending under WhatsApp's own message
  ID, so a retry can never duplicate it; failures offer Retry and Delete. Delivered and
  read ticks follow receipts. Opening a chat sends read receipts.
- **Attachments:** photos and stickers show inline, videos play on request, voice notes
  and audio in a player, documents as cards. Each downloads from WhatsApp the first time
  it is shown (new photos, stickers and voice notes up to 16 MiB as they arrive), once,
  into the content-addressed blob store; HTML and other active types only download.
  Expired media is requested from the phone again. Download and Save to Files (shared
  folder picker; a taken name gets a number) are in the message menu. Deleting a message
  for everyone removes its attachment.
- **History:** Dar asks for a full sync with no day or size limit; the phone decides
  what it sends. Progress shows on the account card.
- **States:** connecting, reconnecting, unlinked (history stays, read-only), and lasting
  failures (another copy using the keys, a temporary ban, an outdated bridge).
- **Live:** the browser subscribes to `my_chat_changes` and reloads what changed.

## How it is built

- **Bridge:** `next/bridge` is one Go binary (`dar-chat-bridge`, built by
  `nix/chat-bridge.nix`, or by `scripts/dev.mjs` when Go is available). The runtime
  (`runtime/chat`) starts it with a minimal environment and speaks newline-delimited
  JSON over stdio (no port), restarts it with backoff and reconnects linked accounts.
- **Keys:** `bridge/store.go` implements whatsmeow's stores on a key-value layer
  (`bridge/kv.go`): keys used constantly are loaded at start, bulky ones (message
  secrets, app state MACs, event buffers, retry copies) are read on demand, and
  whatsmeow's decryption transaction is one atomic write. The runtime encrypts each
  value with the identity keyring (`keyringBox`, bound to its account and key) into
  the service-only `chat_keys` table; expiring keys are purged hourly.
- **Data:** `module/src/chat.ts` keeps accounts, chats and messages in private tables
  with newest-first range keys. Attachments are `chat_media` rows; their blob IDs keep the blobs
  through maintenance and put them in backups. The bridge keeps each attachment's download
  keys in its encrypted store and decrypts into `<root>/chat-downloads`. Incoming messages are stored before whatsmeow
  acknowledges them, so a failed write means WhatsApp delivers again.
- **Privacy:** WhatsApp is end-to-end encrypted, but Dar is a linked device, so the
  server stores messages decrypted, like the rest of Dar's data. Account deletion
  removes Chat data. Backups (v12) include accounts, chats, messages and downloaded
  attachments but not keys: a restored account is unlinked and links again, so two
  servers never share a WhatsApp device.

## Limits

- 10 accounts per person, 500,000 stored messages per account, 64 KiB per message.
- Attachments do not count toward Files quotas (a copy saved to Files does).
  View-once media is not downloaded. Passkey-protected WhatsApp accounts cannot link yet.
- A chat known first by its LID and later by its phone number may appear twice.

## Phases

1. Done: bridge, key store, link/unlink, text in and out, history, account tabs.
2. Done: attachments into blobs, download, Save to Files, backups.
3. Reactions, chat info, archive/pin/mute from Dar, typing, older history on demand.
4. Message search in the shared search, linking chats to Dar contacts.
5. Discord, after deciding between a bot (only servers that add it, no personal
   DMs) and automating a personal account, which Discord's terms forbid.

Verify with `nix develop -c node next/scripts/verify.mjs --chat` (Go tests, a fake
bridge for the integration and browser tests; never a real WhatsApp account). Link a
real phone by hand to check pairing.
