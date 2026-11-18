# Chat

Optional built-in app at `/chat`: a client for other platforms, not a network of its
own: WhatsApp and Discord. Layout follows the selected
[mock A with account tabs](spacetimedb/chat-mocks.html) (November 17, 2026) and, for
Discord, [mock B, the voice dock](spacetimedb/discord-mocks.html) (November 18); see Chat
in the [design guide](design.md). Administrators can disable it in Administration → Apps;
data is kept and every account disconnects.

## Decisions (maintainer, November 17)

- WhatsApp through [whatsmeow](https://github.com/tulir/whatsmeow) in a Go bridge. Go is
  approved for this bridge; more of Dar may move to Go later.
- No Postgres or SQLite: whatsmeow's device keys and all chat data live in
  SpacetimeDB through a custom whatsmeow store.
- On linking, import as much history as WhatsApp provides.
- Accounts never mix: each linked account is a tab with its own chats, views and counts.

## Decisions (maintainer, November 18): Discord

- Discord through the person's **own account**, not a bot: Dar is a user client in the Go
  bridge (QR sign-in, gateway, REST). Discord's terms forbid this and can lead to the
  account being limited or closed; the maintainer accepted that, and Link Discord says so.
- Voice is wanted, and Discord requires its DAVE end-to-end encryption for calls (since
  March 2026): the bridge uses [dave-go](https://github.com/thomas-vilte/dave-go) (pure Go,
  Apache-2.0, not independently audited) for it, with the browser's microphone and playback.
- Layout: mock B (a voice dock in the sidebar while text channels stay usable), with A's
  full call view when the voice channel itself is opened.

## What works

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
- **Message actions:** the message menu reacts (six quick reactions; chips under the bubble
  show who and remove yours), replies, copies, edits your text within 15 minutes, deletes
  for everyone (yours) or from Dar only (local; WhatsApp and the phone keep it). Every
  change goes to WhatsApp before Dar records it.
- **Chat info:** a fourth sheet (a full screen on phones) with mute (8 hours, 1 week,
  always), pin and archive, which sync to the phone through WhatsApp's app state; the
  messages Dar keeps, recent photos and documents, and a group's description and members.
- **Attachments:** photos and stickers show inline, videos play on request, voice notes
  and audio in a player, documents as cards. Each downloads from WhatsApp the first time
  it is shown (new photos, stickers and voice notes up to 16 MiB as they arrive), once,
  into the content-addressed blob store; HTML and other active types only download.
  Expired media is requested from the phone again. Download and Save to Files (shared
  folder picker; a taken name gets a number) are in the message menu. Deleting a message
  for everyone removes its attachment.
- **Dar contacts:** a chat whose number is in any address book you can read (owned or
  shared) takes the contact's name, in the list, the conversation, group senders and
  group members. Numbers saved without a country code (a trunk 0, or a short number) take
  the country of the account's own WhatsApp number. Chat info shows the contact with Open,
  or Add to Contacts, which saves the number in a Dar address book. New chat lists Dar
  contacts with phone numbers. Contacts are only read: nothing is written to WhatsApp or
  the phone. Names follow contact changes live.
- **History:** Dar asks for a full sync with no day or size limit; the phone decides
  what it sends. Progress shows on the account card. At the start of a chat, "Get earlier messages
  from your phone" asks for 50 more before the oldest one Dar has; they arrive as history.
- **States:** connecting, reconnecting, unlinked (history stays, read-only), and lasting
  failures (another copy using the keys, a temporary ban, an outdated bridge).
- **Live:** the browser subscribes to `my_chat_changes` and reloads what changed.

## Discord (text)

- **Sign-in:** Link an account → Discord shows the terms notice and Discord's QR code
  (remote auth v2: an RSA key made for the sign-in; the phone approves; the token arrives
  encrypted to that key). Dar never sees the password. The token lives in the encrypted
  key store; Sign out ends the session on Discord and deletes it.
- **Servers:** the sidebar lists Direct messages, then servers (in Discord's order) that
  open to their channels by category, text before voice. Only channels the account can
  view (roles and overwrites computed in the bridge) are kept; channels and servers it
  loses go with their messages. Unread channels are bold; mentions show as counts; muted
  ones are dimmed. A channel takes the list sheet's place.
- **Messages:** flat rows with replies, attachments (the first in the message; others are
  named), stickers, mentions shown as names, reactions as counts (several per person,
  each toggled), edits without a time limit and deletions. Sent messages are stored first
  under a nonce that Discord recognises when the message is sent again; the copy moves to
  Discord's ID whichever of the answer and the gateway arrives first. Opening a channel
  fetches what was said while Dar was away; "Load earlier messages from Discord" fetches
  50 more at a time. Reading in Dar sends a read mark; reading elsewhere clears it here.
- **Other:** New message opens a DM with a friend. Mute syncs to Discord (per channel or
  DM); pin and archive are WhatsApp's. Attachment links Discord has expired are refreshed.

## Discord (voice)

- **Who is where:** people in each voice channel show under it in the tree (muted and
  deafened marked, a green ring while they speak). Opening a voice channel shows the call
  view: who is in it and Join voice (optionally muted).
- **In a call:** the dock at the bottom of the sidebar (under the list on a phone) shows the
  channel, mute, deafen and leave while you keep reading and writing; the call view shows a
  tile per person. An end-to-end encrypted call shows a lock; it opens the 30-digit call
  code to compare with others. Unmuting also undeafens, as in Discord. A blocked microphone
  still lets you listen. Calls in direct messages, video, screen share and stage channels
  are not supported.
- **One tab carries the audio:** the tab that joined sends the microphone and plays the
  call; others show "Use this tab". A call nobody hears for 15 seconds (the tab closed or
  left Chat) is left, as when Discord's app closes.
- **How:** the bridge joins through the gateway (voice state), opens the voice gateway (v8),
  finds its address over UDP, and sends and receives Opus in RTP with transport encryption
  (`aead_aes256_gcm_rtpsize`, else XChaCha20) and DAVE (`bridge/discord_voice.go`). Audio is
  held while DAVE agrees keys; a dropped voice connection reconnects up to five times. The
  browser encodes and decodes Opus with WebCodecs; the microphone runs in an AudioWorklet
  (`client/chat/mic-worklet.ts`) and sends only while you speak (a level threshold plus
  300 ms). Between them, `/api/v1/chat/live?accountId=` is a WebSocket (`runtime/chat/live.ts`,
  RFC 6455 without extensions) carrying voice states and the call as JSON and audio as
  binary; it needs the session cookie and Dar's Origin, and the session is checked again
  every minute. Microphone frames reach the bridge as one-way messages (no answer).

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
- Discord: messages up to 4,000 characters; no threads, forums, stage channels, member
  lists, typing or presence yet. Voice only in server voice channels, audio only. Behaviour
  against Discord itself is verified by hand (tests use local fakes).

## Phases

1. Done: bridge, key store, link/unlink, text in and out, history, account tabs.
2. Done: attachments into blobs, download, Save to Files, backups.
3. Done: reactions, edits and deletions from Dar, chat info, mute/pin/archive, older
   history on demand. Typing indicators are left out: they would need a push channel
   beside the database change feed.
4. Done: Dar contacts name chats (read-only towards WhatsApp); chat names and message text
   in the shared search (newest first, up to three messages per chat, 40,000 messages per
   search), and a result opens the chat at its message.
5. Done: Discord text through the person's own account (above).
6. Done: Discord voice in server voice channels (dock, call view, DAVE, browser audio over
   the live socket).

Verify with `nix develop -c node next/scripts/verify.mjs --chat` (Go tests with local fake
Discord servers, a fake bridge for the integration and browser tests; never a real
WhatsApp or Discord account). Link a real phone or Discord account by hand to check
pairing and sign-in.
