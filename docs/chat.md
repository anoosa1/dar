# Chat

Planned app at `/chat`: a client for other platforms, not a network of its own.
Layout follows the selected [mock A with account tabs](spacetimedb/chat-mocks.html)
(November 17, 2026); see Chat in the [design guide](design.md). Not built yet.

## Decisions (maintainer, November 17)

- WhatsApp first, through [whatsmeow](https://github.com/tulir/whatsmeow) in a Go
  bridge. Go is approved for this bridge; more of Dar may move to Go later.
- On linking, import as much history as WhatsApp provides.
- Accounts never mix: each linked account is a tab with its own chats, views,
  counts, notifications and media settings.

## Plan

- **Bridge:** one `dar-chat-bridge` Go binary, built by Nix (`buildGoModule`) and
  wrapped into `dar`. A Cordis-owned runtime service starts it, speaks
  newline-delimited JSON over stdio (no listening port), restarts it with backoff and
  stops it on dispose. One process serves every linked account.
- **Device keys without SQLite:** whatsmeow's `sqlstore` is not used. The bridge
  implements whatsmeow's store interfaces (identity, sessions, pre-keys, sender keys,
  app state, contacts, LID map) by calling Node, which keeps them in service-only
  SpacetimeDB tables, encrypted with the existing AES-GCM keyring. Keys never reach
  logs, the browser or the Nix store.
- **Linking:** QR codes and phone pairing codes stream from the bridge to the right
  editing pane. Unlink logs the device out and keeps history read-only until deleted.
- **History:** link with full sync requested and whatsmeow's history limits raised
  as far as it allows; the phone decides what it sends (often a year or more). Older
  messages are requested per chat on demand when scrolling past the start. Import
  progress shows in the sidebar.
- **Storage:** private SpacetimeDB tables per account: accounts, chats, messages
  (with edits, deletions and receipts), reactions and participants. Browsers
  subscribe to one account's chat list and a window of one chat, never whole
  histories. Media downloads into content-addressed blobs; Save to Files links the
  same blob into a Files folder at no extra cost. Media WhatsApp no longer serves is
  re-requested from the phone, else marked unavailable.
- **Privacy:** WhatsApp is end-to-end encrypted, but Dar is a linked device, so the
  server stores messages decrypted, like the rest of Dar's data. Backups and
  account deletion include Chat.
- **Tests:** never a live WhatsApp account. The bridge has a fake mode that replays
  recorded protocol events; the maintainer verifies linking with a real phone.

## Phases

1. Bridge, key store, link/unlink, receive and send text, the account tab and list.
2. Full history import and media (photos, voice notes, documents), Save to Files.
3. Groups, replies, reactions, edits, deletes, receipts, typing, chat info, states.
4. Message search in the shared search, linking chats to Dar contacts, phones.
5. Discord, after deciding between a bot (only servers that add it, no personal
   DMs) and automating a personal account, which Discord's terms forbid.
